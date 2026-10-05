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
fn create_cmd(
    home: &TestHome,
    workspace: &tempfile::TempDir,
    tools: &tempfile::TempDir,
    key: &str,
    crash: Option<&str>,
) -> Command {
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
    cmd
}
fn create(
    home: &TestHome,
    workspace: &tempfile::TempDir,
    tools: &tempfile::TempDir,
    key: &str,
    crash: Option<&str>,
) -> Output {
    create_cmd(home, workspace, tools, key, crash)
        .output()
        .unwrap()
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
    assert_eq!(data["data"]["writer_fence"]["state"], "foundation-only");
    for (name, key) in [
        ("writer.lock", "writer"),
        ("launch-gate.lock", "launch_gate"),
    ] {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(home.path().join("runs").join(run).join(name)).unwrap();
        assert_eq!(m.mode() & 0o7777, 0o600);
        assert_eq!(m.nlink(), 1);
        assert_eq!(data["data"]["writer_fence"][key]["dev"], m.dev());
        assert_eq!(data["data"]["writer_fence"][key]["ino"], m.ino());
    }
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
    assert_eq!(again["data"]["writer_fence"], data["data"]["writer_fence"]);
    let mut historical: serde_json::Value = serde_json::from_slice(
        &std::fs::read(home.path().join("runs").join(run).join("manifest.json")).unwrap(),
    )
    .unwrap();
    historical.as_object_mut().unwrap().remove("agent_owner");
    let old: taskfleet_core::Manifest = serde_json::from_value(historical).unwrap();
    assert_eq!(old.agent_owner, taskfleet_core::AgentOwner::Taskfleet);
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
fn retry_refuses_missing_replaced_and_linked_fence_without_repair() {
    use std::os::unix::fs::{symlink, MetadataExt};
    let (home, workspace, tools) = fixture();
    let first = create(&home, &workspace, &tools, "fence-integrity", None);
    assert!(first.status.success(), "{first:?}");
    let value: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    let run = value["data"]["run_id"].as_str().unwrap();
    let dir = home.path().join("runs").join(run);
    let writer = dir.join("writer.lock");
    let original = std::fs::metadata(&writer).unwrap().ino();
    std::fs::rename(&writer, dir.join("old-writer.lock")).unwrap();
    symlink("old-writer.lock", &writer).unwrap();
    let refused = create(&home, &workspace, &tools, "fence-integrity", None);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("writer_fence_unavailable"));
    std::fs::remove_file(&writer).unwrap();
    std::fs::hard_link(dir.join("old-writer.lock"), &writer).unwrap();
    assert!(!create(&home, &workspace, &tools, "fence-integrity", None)
        .status
        .success());
    std::fs::remove_file(&writer).unwrap();
    std::fs::rename(dir.join("old-writer.lock"), &writer).unwrap();
    assert_eq!(std::fs::metadata(&writer).unwrap().ino(), original);
    assert!(create(&home, &workspace, &tools, "fence-integrity", None)
        .status
        .success());
    std::fs::remove_file(dir.join("writer-fence.json")).unwrap();
    assert!(!create(&home, &workspace, &tools, "fence-integrity", None)
        .status
        .success());
    assert!(
        !dir.join("writer-fence.json").exists(),
        "never enroll old/missing ledger on retry"
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
fn live_pid_without_boot_receipt_is_not_reported_confirmed() {
    let (home, workspace, tools) = fixture();
    let mut child = create_cmd(&home, &workspace, &tools, "slow-boot", None)
        .env("TASKFLEET_TEST_SLOW_BOOT", "3000")
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let pid_file = loop {
        if let Ok(entries) = std::fs::read_dir(home.path().join("runs")) {
            if let Some(path) = entries
                .filter_map(Result::ok)
                .map(|e| e.path().join("supervisor.pid"))
                .find(|p| p.exists())
            {
                break path;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "supervisor did not claim pid"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    child.kill().unwrap();
    child.wait().unwrap();
    let early = create(&home, &workspace, &tools, "slow-boot", None);
    assert!(
        !early.status.success(),
        "early retry must not claim confirmed boot"
    );
    assert!(
        String::from_utf8_lossy(&early.stderr).contains("supervisor_boot_unconfirmed"),
        "{early:?}"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
    let success = loop {
        let result = create(&home, &workspace, &tools, "slow-boot", None);
        if result.status.success() {
            break result;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "readiness not recovered: {result:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(pid_file.exists());
    let parsed: serde_json::Value = serde_json::from_slice(&success.stdout).unwrap();
    assert_eq!(parsed["data"]["supervisor"]["state"], "confirmed");
}

#[test]
fn killed_creator_reuses_the_exact_reservation_and_preserves_dirty_work() {
    for boundary in ["reservation", "git", "run-created", "staged", "published"] {
        let (home, workspace, tools) = fixture();
        let lost = create(&home, &workspace, &tools, "crash-key", Some(boundary));
        assert_eq!(lost.status.code(), Some(71), "{boundary}: {lost:?}");
        if boundary == "git"
            || boundary == "run-created"
            || boundary == "staged"
            || boundary == "published"
        {
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

#[test]
fn native_pi_binding_is_immutable_and_readable_after_checkout_removal() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let (home, workspace, tools) = fixture();
    let created = create(&home, &workspace, &tools, "bind", None);
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let run = value["data"]["run_id"].as_str().unwrap();
    let checkout = value["data"]["worktree_path"].as_str().unwrap();
    let native_home = workspace.path().join("native-home");
    let store = native_home.join(".pi/agent/sessions/project");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::set_permissions(
        native_home.join(".pi/agent/sessions"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let id = "b30d3508-a8d4-4aa7-bafa-7f5dfef72014";
    let path = store.join("session.jsonl");
    std::fs::write(
        &path,
        format!("{{\"type\":\"session\",\"version\":3,\"id\":\"{id}\",\"cwd\":\"{checkout}\"}}\n"),
    )
    .unwrap();
    let call = |run: &str, node: &str, path: &Path, checkout: &str| -> Output {
        Command::new(env!("CARGO_BIN_EXE_taskfleet"))
            .args([
                "--output",
                "json",
                "run",
                "session",
                "bind",
                run,
                "--node",
                node,
                "--pi-session-id",
                id,
                "--session-path",
            ])
            .arg(path)
            .args(["--checkout", checkout])
            .env("HOME", &native_home)
            .env("TASKFLEET_HOME", home.path())
            .output()
            .unwrap()
    };
    let first = call(run, "n-0001", &path, checkout);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let replay = call(run, "n-0001", &path, checkout);
    assert!(replay.status.success());
    let v: serde_json::Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(v["data"]["idempotent_replay"], true);
    let events =
        std::fs::read_to_string(home.path().join("runs").join(run).join("events.jsonl")).unwrap();
    assert_eq!(events.matches("caller.pi.session_bound").count(), 1);
    let bad =
        |o: Output, code: &str| assert!(String::from_utf8_lossy(&o.stderr).contains(code), "{o:?}");
    bad(call(run, "n-0002", &path, checkout), "node_not_found");
    bad(
        call("01arz3ndektsv4rrffq69g5fav", "n-0001", &path, checkout),
        "run_not_found",
    );
    bad(
        call(run, "n-0001", &path, "/different/checkout"),
        "pi_session_conflict",
    );
    let other = store.join("other.jsonl");
    std::fs::copy(&path, &other).unwrap();
    bad(call(run, "n-0001", &other, checkout), "pi_session_conflict");
    std::fs::rename(&path, store.join("held.jsonl")).unwrap();
    std::fs::copy(&other, &path).unwrap();
    bad(call(run, "n-0001", &path, checkout), "pi_session_conflict");
    std::fs::write(&path, "not-json\n").unwrap();
    bad(call(run, "n-0001", &path, checkout), "invalid_pi_session");
    std::fs::remove_file(&path).unwrap();
    symlink(&other, &path).unwrap();
    bad(call(run, "n-0001", &path, checkout), "invalid_pi_session");
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, "x".repeat(16385)).unwrap();
    bad(call(run, "n-0001", &path, checkout), "invalid_pi_session");
    std::fs::remove_file(&path).unwrap();
    let linked = store.parent().unwrap().join("linked");
    symlink(&store, &linked).unwrap();
    bad(
        call(run, "n-0001", &linked.join("other.jsonl"), checkout),
        "invalid_pi_session",
    ); // symlinked parent is never followed
    std::fs::write(
        &path,
        format!("{{\"type\":\"session\",\"version\":3,\"id\":\"{id}\",\"cwd\":\"{checkout}\"}}\n"),
    )
    .unwrap();
    git(
        &workspace.path().join("repo"),
        &["worktree", "remove", "--force", checkout],
    );
    let shown = Command::new(env!("CARGO_BIN_EXE_taskfleet"))
        .args(["--output", "json", "node", "show", run, "n-0001"])
        .env("TASKFLEET_HOME", home.path())
        .output()
        .unwrap();
    assert!(shown.status.success());
    let v: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(v["data"]["caller_pi_session"]["original_cwd"], checkout);
    let run_shown = Command::new(env!("CARGO_BIN_EXE_taskfleet"))
        .args(["--output", "json", "run", "show", run])
        .env("TASKFLEET_HOME", home.path())
        .env("GIT_BIN", fixture_git_binary())
        .output()
        .unwrap();
    assert!(
        run_shown.status.success(),
        "{}",
        String::from_utf8_lossy(&run_shown.stderr)
    );
    let run_view: serde_json::Value = serde_json::from_slice(&run_shown.stdout).unwrap();
    assert_eq!(
        run_view["data"]["caller_pi_session"]["original_cwd"],
        checkout
    );
    assert!(v["data"]["evidence"].is_null());
    bad(call(run, "n-0001", &path, checkout), "pi_session_conflict"); // no new binds after teardown
}

#[test]
fn caller_pi_told_wait_and_generation_contract() {
    use std::os::unix::fs::PermissionsExt;
    let (home, workspace, tools) = fixture();
    let created = create(&home, &workspace, &tools, "wait", None);
    assert!(created.status.success(), "{created:?}");
    let v: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let run = v["data"]["run_id"].as_str().unwrap();
    let checkout = v["data"]["worktree_path"].as_str().unwrap();
    let native_home = workspace.path().join("native-home");
    let store = native_home.join(".pi/agent/sessions/project");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::set_permissions(
        native_home.join(".pi/agent/sessions"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let id = "b30d3508-a8d4-4aa7-bafa-7f5dfef72014";
    let path = store.join("session.jsonl");
    std::fs::write(
        &path,
        format!("{{\"type\":\"session\",\"version\":3,\"id\":\"{id}\",\"cwd\":\"{checkout}\"}}\n"),
    )
    .unwrap();
    let command = |args: &[&str]| -> Output {
        Command::new(env!("CARGO_BIN_EXE_taskfleet"))
            .args(["--output", "json", "run"])
            .args(args)
            .env("HOME", &native_home)
            .env("TASKFLEET_HOME", home.path())
            .env("GIT_BIN", fixture_git_binary())
            .output()
            .unwrap()
    };
    let path_str = path.to_str().unwrap();
    let binding = command(&[
        "session",
        "bind",
        run,
        "--node",
        "n-0001",
        "--pi-session-id",
        id,
        "--session-path",
        path_str,
        "--checkout",
        checkout,
    ]);
    assert!(binding.status.success(), "{binding:?}");
    let wait = |runs: &[&str]| -> (i32, serde_json::Value) {
        let output = command(runs);
        let code = output.status.code().unwrap();
        let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{output:?}"));
        (code, value)
    };
    let (code, pending) = wait(&["wait", run, "--timeout", "0", "--fail-on-error"]);
    assert_eq!(code, 2);
    assert_eq!(pending["data"]["outcome"], "timed-out");
    assert!(pending["data"]["runs"][0].get("caller_agent").is_none());
    let transition = |generation: &str, state: &str, reason: Option<&str>| -> Output {
        let mut args = vec![
            "session",
            "update",
            run,
            "--node",
            "n-0001",
            "--generation",
            generation,
            "--pi-session-id",
            id,
            "--session-path",
            path_str,
            "--checkout",
            checkout,
            "--state",
            state,
        ];
        if let Some(reason) = reason {
            args.extend(["--reason", reason]);
        }
        command(&args)
    };
    let start = transition("1", "started", None);
    assert!(start.status.success(), "{start:?}");
    let (code, active) = wait(&["wait", run, "--timeout", "0"]);
    assert_eq!(code, 2);
    assert_eq!(
        active["data"]["runs"][0]["caller_agent"]["state"],
        "started"
    );
    assert_eq!(
        active["data"]["runs"][0]["caller_agent"]["control"],
        "unknown"
    );
    assert!(!transition("2", "started", None).status.success());
    let stop = transition("1", "exited", Some("reaped child"));
    assert!(stop.status.success(), "{stop:?}");
    let retry = transition("1", "exited", Some("reaped child"));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&retry.stdout).unwrap()["data"]
            ["idempotent_replay"],
        true
    );
    assert!(!transition("1", "exited", Some("other reason"))
        .status
        .success());
    let (code, stopped) = wait(&["wait", run, "--timeout", "0", "--fail-on-error"]);
    assert_eq!(code, 3);
    assert_eq!(stopped["data"]["runs"][0]["status"], "pending");
    assert_eq!(
        stopped["data"]["runs"][0]["caller_agent"]["state"],
        "stopped-unmerged"
    );
    assert_eq!(
        stopped["data"]["runs"][0]["caller_agent"]["settled_generation"],
        1
    );
    let sibling = create(&home, &workspace, &tools, "wait-sibling", None);
    assert!(sibling.status.success(), "{sibling:?}");
    let sibling_value: serde_json::Value = serde_json::from_slice(&sibling.stdout).unwrap();
    let sibling_run = sibling_value["data"]["run_id"].as_str().unwrap();
    let sibling_checkout = sibling_value["data"]["worktree_path"].as_str().unwrap();
    let (code, mixed) = wait(&[
        "wait",
        sibling_run,
        run,
        "--any",
        "--timeout",
        "0",
        "--fail-on-error",
        "--progress",
    ]);
    assert_eq!(code, 3);
    assert_eq!(mixed["data"]["runs"][0]["run_id"], sibling_run);
    assert!(mixed["data"]["runs"][0].get("caller_agent").is_none());
    assert_eq!(
        mixed["data"]["runs"][1]["caller_agent"]["settled_generation"],
        1
    );
    let (code, all) = wait(&[
        "wait",
        sibling_run,
        run,
        "--timeout",
        "0",
        "--fail-on-error",
    ]);
    assert_eq!(code, 2);
    assert_eq!(all["data"]["outcome"], "timed-out");
    assert!(transition("2", "started", None).status.success());
    assert!(!transition("1", "exited", Some("reaped child"))
        .status
        .success());
    let (code, restarted) = wait(&["wait", run, "--timeout", "0", "--fail-on-error"]);
    assert_eq!(code, 2);
    assert_eq!(
        restarted["data"]["runs"][0]["caller_agent"]["current_generation"],
        2
    );
    assert!(transition(
        "2",
        "control-uncertain",
        Some("daemon restarted without child proof")
    )
    .status
    .success());
    let (code, uncertain) = wait(&["wait", run, "--timeout", "0", "--fail-on-error"]);
    assert_eq!(code, 3);
    assert_eq!(
        uncertain["data"]["runs"][0]["caller_agent"]["state"],
        "control-unknown"
    );
    assert!(!transition("3", "started", None).status.success());
    let show = command(&["show", run]);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&show.stdout).unwrap()["data"]["caller_agent"]
            ["generation"],
        2
    );
    let (code, multiple) = wait(&[
        "wait",
        run,
        run,
        "--any",
        "--timeout",
        "0",
        "--fail-on-error",
    ]);
    assert_eq!(code, 3);
    assert_eq!(multiple["data"]["runs"].as_array().unwrap().len(), 2);
    for verb in ["cancel", "merge"] {
        let refused = command(&[verb, run]);
        assert!(String::from_utf8_lossy(&refused.stderr).contains("writer_fence_unavailable"));
    }
    let events =
        std::fs::read_to_string(home.path().join("runs").join(run).join("events.jsonl")).unwrap();
    assert_eq!(events.matches("caller.pi.lifecycle").count(), 4);
    git(
        &workspace.path().join("repo"),
        &["worktree", "remove", "--force", sibling_checkout],
    );
    git(
        &workspace.path().join("repo"),
        &["worktree", "remove", "--force", checkout],
    );
}
