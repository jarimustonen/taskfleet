mod common;
use common::{fixture_git_binary, TestHome};
use std::path::Path;
use std::process::{Command, Output};

fn git(repo: &Path, args: &[&str]) {
    assert!(Command::new(fixture_git_binary())
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .unwrap()
        .success());
}
fn fixture() -> (TestHome, tempfile::TempDir, tempfile::TempDir) {
    let home = TestHome::new();
    let workspace = tempfile::TempDir::new().unwrap();
    let tools = tempfile::TempDir::new().unwrap();
    let repo = workspace.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ],
    );
    for name in ["workmux", "tmux", "pi", "claude"] {
        let path = tools.path().join(name);
        std::fs::write(
            &path,
            format!("#!/bin/sh\necho {name} >> \"$TASKFLEET_TEST_FORBIDDEN\"\nexit 99\n"),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    (home, workspace, tools)
}
fn create(
    home: &TestHome,
    workspace: &tempfile::TempDir,
    tools: &tempfile::TempDir,
    key: &str,
    crash: Option<&str>,
) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    cmd.args([
        "--output",
        "json",
        "run",
        "create",
        "--agent-owner",
        "caller",
        "--kind",
        "spinoff",
        "--source-repo",
    ])
    .arg(workspace.path().join("repo"))
    .args([
        "--source-branch",
        "main",
        "--title",
        "Caller fixture",
        "--task",
        "Do something",
        "--idempotency-key",
        key,
    ])
    .env("TASKFLEET_HOME", home.path())
    .env("GIT_BIN", fixture_git_binary())
    .env("TMUX_BIN", tools.path().join("tmux"))
    .env("WORKMUX_BIN", tools.path().join("workmux"))
    .env(
        "TASKFLEET_TEST_FORBIDDEN",
        workspace.path().join("forbidden"),
    )
    .env("PATH", tools.path())
    .env_remove("TMUX");
    if let Some(boundary) = crash {
        cmd.env("TASKFLEET_TEST_CALLER_CRASH_AT", boundary);
    }
    cmd.output().unwrap()
}
#[test]
fn git_only_create_and_replay_fence_settlement() {
    let (home, workspace, tools) = fixture();
    let first = create(&home, &workspace, &tools, "same-key", None);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let data: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    let run = data["data"]["run_id"].as_str().unwrap();
    let checkout = data["data"]["worktree_path"].as_str().unwrap();
    assert!(Path::new(checkout).is_dir());
    assert_eq!(data["data"]["node_id"], "n-0001");
    assert_eq!(data["data"]["checkout_verified"], true);
    assert_eq!(data["data"]["supervisor"]["state"], "confirmed");
    let replay = create(&home, &workspace, &tools, "same-key", None);
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let again: serde_json::Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(again["data"]["run_id"], run);
    assert_eq!(again["data"]["worktree_path"], checkout);
    assert_eq!(again["data"]["idempotent_replay"], true);
    let node: serde_json::Value = serde_json::from_slice(
        &std::fs::read(home.path().join("runs").join(run).join("nodes/n-0001.json")).unwrap(),
    )
    .unwrap();
    assert!(node["agent_pid"].is_null());
    for verb in ["merge", "cancel"] {
        let output = Command::new(env!("CARGO_BIN_EXE_taskfleet"))
            .args(["--output", "json", "run", verb, run])
            .env("TASKFLEET_HOME", home.path())
            .env("GIT_BIN", fixture_git_binary())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("writer_fence_unavailable"),
            "{verb}: {output:?}"
        );
    }
    assert!(!workspace.path().join("forbidden").exists());
    git(
        &workspace.path().join("repo"),
        &["worktree", "remove", "--force", checkout],
    );
}
#[test]
fn concurrent_same_key_only_publishes_one_run() {
    let (home, workspace, tools) = fixture();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..5)
            .map(|_| scope.spawn(|| create(&home, &workspace, &tools, "race", None)))
            .collect();
        let mut ids = Vec::new();
        for handle in handles {
            let result = handle.join().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
            ids.push(value["data"]["run_id"].as_str().unwrap().to_owned());
        }
        assert!(ids.iter().all(|id| id == &ids[0]));
        assert_eq!(
            std::fs::read_dir(home.path().join("runs")).unwrap().count(),
            1
        );
    });
    assert!(!workspace.path().join("forbidden").exists());
}

#[test]
fn killed_creator_reuses_the_exact_reservation_and_preserves_dirty_work() {
    for boundary in ["reservation", "git", "staged", "published"] {
        let (home, workspace, tools) = fixture();
        let lost = create(&home, &workspace, &tools, "crash-key", Some(boundary));
        assert_eq!(lost.status.code(), Some(71), "{boundary}: {lost:?}");
        if boundary == "git" || boundary == "staged" || boundary == "published" {
            let listing = Command::new(fixture_git_binary())
                .arg("-C")
                .arg(workspace.path().join("repo"))
                .args(["worktree", "list", "--porcelain"])
                .output()
                .unwrap();
            let content = String::from_utf8_lossy(&listing.stdout);
            let checkout = content
                .lines()
                .find_map(|line| {
                    line.strip_prefix("worktree ")
                        .filter(|p| p.contains(".taskfleet-worktrees"))
                })
                .unwrap();
            std::fs::write(Path::new(checkout).join("unsaved.txt"), "keep me").unwrap();
        }
        let result = create(&home, &workspace, &tools, "crash-key", None);
        assert!(
            result.status.success(),
            "{boundary}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let parsed: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        let checkout = parsed["data"]["worktree_path"].as_str().unwrap();
        assert!(boundary == "reservation" || Path::new(checkout).join("unsaved.txt").exists());
        git(
            &workspace.path().join("repo"),
            &["worktree", "remove", "--force", checkout],
        );
        assert!(!workspace.path().join("forbidden").exists());
    }
}
