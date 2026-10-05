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
            String::from_utf8_lossy(&output.stderr).contains(if verb == "merge" {
                "invalid_node_id"
            } else {
                "invalid_settlement_key"
            }),
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
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains(if verb == "merge" {
                "invalid_node_id"
            } else {
                "invalid_settlement_key"
            })
        );
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

#[cfg(target_os = "linux")]
#[test]
fn reservation_precedes_native_file_and_requires_held_matching_fds() {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    let (home, workspace, tools) = fixture();
    let created = create(&home, &workspace, &tools, "reserve", None);
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
    let timestamp = "2026-09-04T10:20:30.123Z";
    let path = store.join(format!(
        "{}_{}.jsonl",
        timestamp.replace([':', '.'], "-"),
        id
    ));
    let dir = home.path().join("runs").join(run);
    let info = Command::new(env!("CARGO_BIN_EXE_taskfleet"))
        .args([
            "--output",
            "json",
            "run",
            "session",
            "fence",
            run,
            "--node",
            "n-0001",
            "--checkout",
            checkout,
        ])
        .env("TASKFLEET_HOME", home.path())
        .output()
        .unwrap();
    assert!(info.status.success(), "{info:?}");
    let info: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
    assert_eq!(info["data"]["launch_permission"], false);
    assert_eq!(
        info["data"]["writer"]["identity"]["ino"],
        v["data"]["writer_fence"]["writer"]["ino"]
    );
    let gate = std::fs::File::open(dir.join("launch-gate.lock")).unwrap();
    let writer = std::fs::File::open(dir.join("writer.lock")).unwrap();
    let invoke = |generation: &str, supplied: bool, crash: bool| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
        cmd.args([
            "--output",
            "json",
            "run",
            "session",
            "reserve",
            run,
            "--node",
            "n-0001",
            "--generation",
            generation,
            "--pi-session-id",
            id,
            "--checkout",
            checkout,
            "--gate-fd",
            "80",
            "--writer-fd",
            "81",
        ])
        .env("HOME", &native_home)
        .env("TASKFLEET_HOME", home.path());
        if crash {
            cmd.env("TASKFLEET_TEST_CALLER_RESERVE_CRASH_AFTER_APPEND", "1");
        }
        if supplied {
            // SAFETY: dup2 only in the child between fork and exec; parent retains both locks.
            let gate_raw = gate.as_raw_fd();
            let writer_raw = writer.as_raw_fd();
            unsafe {
                cmd.pre_exec(move || {
                    if libc::dup2(gate_raw, 80) < 0 || libc::dup2(writer_raw, 81) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        cmd.output().unwrap()
    };
    assert!(!invoke("1", false, false).status.success());
    assert_eq!(
        unsafe { libc::flock(gate.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    assert_eq!(
        unsafe { libc::flock(writer.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) },
        0
    );
    let killed = invoke("1", true, true);
    assert_eq!(killed.status.code(), Some(71));
    assert!(!path.exists());
    let first = invoke("1", true, false);
    assert!(first.status.success(), "{first:?}");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&first.stdout).unwrap()["data"]
            ["launch_permission"],
        false
    );
    assert!(!path.exists());
    let replay = invoke("1", true, false);
    assert!(replay.status.success(), "{replay:?}");
    std::thread::scope(|s| {
        let a = s.spawn(|| invoke("1", true, false));
        let b = s.spawn(|| invoke("1", true, false));
        for result in [a.join().unwrap(), b.join().unwrap()] {
            assert!(result.status.success(), "{result:?}");
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&result.stdout).unwrap()["data"]
                    ["idempotent_replay"],
                true
            );
        }
    });
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&replay.stdout).unwrap()["data"]
            ["idempotent_replay"],
        true
    );
    assert!(!invoke("2", true, false).status.success());
    // A held FD cannot conceal replacement of its recorded pathname.
    let writer_path = dir.join("writer.lock");
    let moved = dir.join("writer.saved");
    std::fs::rename(&writer_path, &moved).unwrap();
    std::fs::write(&writer_path, "").unwrap();
    std::fs::set_permissions(&writer_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(!invoke("1", true, false).status.success());
    std::fs::remove_file(&writer_path).unwrap();
    std::fs::rename(&moved, &writer_path).unwrap();
    let command = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_taskfleet"))
            .args(["--output", "json", "run"])
            .args(args)
            .env("HOME", &native_home)
            .env("TASKFLEET_HOME", home.path())
            .env("GIT_BIN", fixture_git_binary())
            .output()
            .unwrap()
    };
    let shown = command(&["show", run]);
    let before_bind: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(before_bind["data"]["caller_agent"]["state"], "reserved");
    assert!(before_bind["data"]["caller_agent"]
        .get("session_path")
        .is_none());
    assert!(!path.exists());
    assert!(!command(&[
        "session",
        "update",
        run,
        "--node",
        "n-0001",
        "--generation",
        "1",
        "--pi-session-id",
        id,
        "--checkout",
        checkout,
        "--state",
        "started"
    ])
    .status
    .success());
    let pending = command(&["wait", run, "--timeout", "0", "--fail-on-error"]);
    assert_eq!(pending.status.code(), Some(2));
    std::fs::write(
        &path,
        format!("{{\"type\":\"session\",\"version\":3,\"id\":\"{id}\",\"timestamp\":\"{timestamp}\",\"cwd\":\"{checkout}\"}}\n"),
    )
    .unwrap();
    assert!(!command(&[
        "session",
        "bind",
        run,
        "--node",
        "n-0001",
        "--pi-session-id",
        "b30d3508-a8d4-4aa7-bafa-7f5dfef72015",
        "--generation",
        "1",
        "--session-path",
        path.to_str().unwrap(),
        "--checkout",
        checkout
    ])
    .status
    .success());
    let wrong = store.join("wrong.jsonl");
    std::fs::copy(&path, &wrong).unwrap();
    assert!(!command(&[
        "session",
        "bind",
        run,
        "--node",
        "n-0001",
        "--pi-session-id",
        id,
        "--generation",
        "1",
        "--session-path",
        wrong.to_str().unwrap(),
        "--checkout",
        checkout
    ])
    .status
    .success());
    assert!(!command(&[
        "session",
        "bind",
        run,
        "--node",
        "n-0001",
        "--pi-session-id",
        id,
        "--generation",
        "2",
        "--session-path",
        path.to_str().unwrap(),
        "--checkout",
        checkout
    ])
    .status
    .success());
    assert!(command(&[
        "session",
        "bind",
        run,
        "--node",
        "n-0001",
        "--pi-session-id",
        id,
        "--generation",
        "1",
        "--session-path",
        path.to_str().unwrap(),
        "--checkout",
        checkout
    ])
    .status
    .success());
    let rebound = command(&[
        "session",
        "bind",
        run,
        "--node",
        "n-0001",
        "--pi-session-id",
        id,
        "--generation",
        "1",
        "--session-path",
        path.to_str().unwrap(),
        "--checkout",
        checkout,
    ]);
    assert!(rebound.status.success(), "{rebound:?}");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&rebound.stdout).unwrap()["data"]
            ["idempotent_replay"],
        true
    );
    let bound = command(&["show", run]);
    let bound: serde_json::Value = serde_json::from_slice(&bound.stdout).unwrap();
    assert_eq!(bound["data"]["caller_agent"]["state"], "reserved");
    assert_eq!(
        bound["data"]["caller_agent"]["session_path"],
        path.to_str().unwrap()
    );
    let still_pending = command(&["wait", run, "--timeout", "0", "--fail-on-error"]);
    assert_eq!(still_pending.status.code(), Some(2));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&still_pending.stdout).unwrap()["data"]["runs"]
            [0]["caller_agent"]["state"],
        "reserved"
    );
    assert!(command(&[
        "session",
        "update",
        run,
        "--node",
        "n-0001",
        "--generation",
        "1",
        "--pi-session-id",
        id,
        "--session-path",
        path.to_str().unwrap(),
        "--checkout",
        checkout,
        "--state",
        "started"
    ])
    .status
    .success());
    assert!(!invoke("1", true, false).status.success());
    std::fs::rename(&path, store.join("original.jsonl")).unwrap();
    std::fs::write(&path, format!("{{\"type\":\"session\",\"version\":3,\"id\":\"{id}\",\"timestamp\":\"{timestamp}\",\"cwd\":\"{checkout}\"}}\n")).unwrap();
    assert!(!command(&[
        "session",
        "bind",
        run,
        "--node",
        "n-0001",
        "--pi-session-id",
        id,
        "--generation",
        "1",
        "--session-path",
        path.to_str().unwrap(),
        "--checkout",
        checkout
    ])
    .status
    .success());
    assert!(!workspace.path().join("forbidden").exists());
    drop(gate);
    drop(writer);
    git(
        &workspace.path().join("repo"),
        &["worktree", "remove", "--force", checkout],
    );
}

fn intent_command(home: &TestHome, run: &str, key: &str) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    c.args([
        "--output",
        "json",
        "run",
        "settlement-intent",
        run,
        "--node",
        "n-0001",
        "--operation",
        "merge",
        "--key",
        key,
        "--actor",
        "fixture",
    ])
    .env("TASKFLEET_HOME", home.path());
    c
}

#[test]
fn intent_is_sticky_after_lost_reply_and_writer_contention() {
    use std::os::fd::AsRawFd;
    let (home, workspace, tools) = fixture();
    let created = create(&home, &workspace, &tools, "intent", None);
    assert!(created.status.success(), "{created:?}");
    let value: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let run = value["data"]["run_id"].as_str().unwrap();
    let checkout = value["data"]["worktree_path"].as_str().unwrap();
    let dir = home.path().join("runs").join(run);
    let writer = std::fs::File::open(dir.join("writer.lock")).unwrap();
    assert_eq!(unsafe { libc::flock(writer.as_raw_fd(), libc::LOCK_SH) }, 0);
    let busy = intent_command(&home, run, "stable-key").output().unwrap();
    assert!(!busy.status.success());
    let error: serde_json::Value = serde_json::from_slice(&busy.stderr).unwrap();
    assert_eq!(error["error"]["code"], "writer_active");
    assert_eq!(error["error"]["details"]["phase"], "intent-recorded");
    let seq = error["error"]["details"]["intent"]["seq"].as_u64().unwrap();
    let denied = intent_command(&home, run, "other-key").output().unwrap();
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("settlement_conflict"));
    let reserve = Command::new(env!("CARGO_BIN_EXE_taskfleet"))
        .args([
            "--output",
            "json",
            "run",
            "session",
            "reserve",
            run,
            "--node",
            "n-0001",
            "--generation",
            "1",
            "--pi-session-id",
            "b30d3508-a8d4-4aa7-bafa-7f5dfef72014",
            "--checkout",
            checkout,
            "--gate-fd",
            "3",
            "--writer-fd",
            "4",
        ])
        .env("TASKFLEET_HOME", home.path())
        .output()
        .unwrap();
    assert!(!reserve.status.success());
    assert!(String::from_utf8_lossy(&reserve.stderr).contains("pi_session_conflict"));
    assert!(String::from_utf8_lossy(&reserve.stderr).contains("settlement intent"));
    drop(writer);
    let retry = intent_command(&home, run, "stable-key").output().unwrap();
    assert!(retry.status.success(), "{retry:?}");
    let result: serde_json::Value = serde_json::from_slice(&retry.stdout).unwrap();
    assert_eq!(result["data"]["phase"], "quiesced");
    assert_eq!(result["data"]["intent"]["seq"], seq);
    assert_eq!(result["data"]["destructive_permission"], false);
    let shown = Command::new(env!("CARGO_BIN_EXE_taskfleet"))
        .args(["--output", "json", "run", "show", run])
        .env("TASKFLEET_HOME", home.path())
        .output()
        .unwrap();
    assert!(shown.status.success(), "{shown:?}");
    let v: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(
        v["data"]["manifest"]["caller_settlement_intent"]["seq"],
        seq
    );
    assert!(!workspace.path().join("forbidden").exists());
}

#[test]
fn intent_crash_then_retry_and_changed_inode_fail_closed() {
    use std::os::unix::fs::symlink;
    let (home, workspace, tools) = fixture();
    let created = create(&home, &workspace, &tools, "intent-crash", None);
    assert!(created.status.success(), "{created:?}");
    let v: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let run = v["data"]["run_id"].as_str().unwrap();
    let crashed = intent_command(&home, run, "crash-key")
        .env("TASKFLEET_TEST_CALLER_INTENT_CRASH_AFTER_SYNC", "1")
        .output()
        .unwrap();
    assert_eq!(crashed.status.code(), Some(73));
    let retry = intent_command(&home, run, "crash-key").output().unwrap();
    assert!(retry.status.success(), "{retry:?}");
    let r: serde_json::Value = serde_json::from_slice(&retry.stdout).unwrap();
    assert!(r["data"]["intent"]["seq"].as_u64().unwrap() > 0);
    let events =
        std::fs::read_to_string(home.path().join("runs").join(run).join("events.jsonl")).unwrap();
    assert_eq!(
        events
            .lines()
            .filter(|line| line.contains("\"caller.settlement_intent\""))
            .count(),
        1
    );
    let dir = home.path().join("runs").join(run);
    let writer = dir.join("writer.lock");
    std::fs::rename(&writer, dir.join("old-writer.lock")).unwrap();
    symlink("old-writer.lock", &writer).unwrap();
    let replaced = intent_command(&home, run, "crash-key").output().unwrap();
    assert!(!replaced.status.success());
    assert!(String::from_utf8_lossy(&replaced.stderr).contains("writer_fence_unavailable"));
    assert!(!workspace.path().join("forbidden").exists());
}

#[test]
fn gate_serializes_settlement_before_any_intent_is_persisted() {
    use std::os::fd::AsRawFd;
    let (home, workspace, tools) = fixture();
    let created = create(&home, &workspace, &tools, "gate-serialization", None);
    assert!(created.status.success(), "{created:?}");
    let v: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let run = v["data"]["run_id"].as_str().unwrap();
    let run_dir = home.path().join("runs").join(run);
    let gate = std::fs::File::open(run_dir.join("launch-gate.lock")).unwrap();
    assert_eq!(unsafe { libc::flock(gate.as_raw_fd(), libc::LOCK_EX) }, 0);
    let denied = intent_command(&home, run, "gate-key").output().unwrap();
    assert!(!denied.status.success());
    let err: serde_json::Value = serde_json::from_slice(&denied.stderr).unwrap();
    assert_eq!(err["error"]["code"], "gate_busy");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(run_dir.join("manifest.json")).unwrap()).unwrap();
    assert!(manifest.get("caller_settlement_intent").is_none());
    drop(gate);
    assert!(intent_command(&home, run, "gate-key")
        .output()
        .unwrap()
        .status
        .success());
    assert!(!workspace.path().join("forbidden").exists());
}

#[test]
fn inherited_child_shared_fd_blocks_exclusive_until_exit() {
    let (home, workspace, tools) = fixture();
    let created = create(&home, &workspace, &tools, "lease-child", None);
    assert!(created.status.success(), "{created:?}");
    let v: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let run = v["data"]["run_id"].as_str().unwrap();
    let writer = home.path().join("runs").join(run).join("writer.lock");
    let helper = workspace.path().join("lease-probe");
    assert!(Command::new("cc")
        .args(["-Wall", "-Wextra", "-o"])
        .arg(&helper)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/caller_writer_lease.c"
        ))
        .status()
        .unwrap()
        .success());
    let held = Command::new(&helper)
        .args(["hold"])
        .arg(&writer)
        .arg("2")
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(held.success(), "{held:?}");
    assert_eq!(
        Command::new(&helper)
            .arg("probe")
            .arg(&writer)
            .output()
            .unwrap()
            .status
            .code(),
        Some(1)
    );
    let busy = intent_command(&home, run, "inherited").output().unwrap();
    assert!(!busy.status.success());
    assert!(String::from_utf8_lossy(&busy.stderr).contains("writer_active"));
    // The parent holding process has already exited; the exec'd child owns
    // the last inherited FD. Wait for its fixed bounded duration.
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert_eq!(
        Command::new(&helper)
            .arg("probe")
            .arg(&writer)
            .output()
            .unwrap()
            .status
            .code(),
        Some(0)
    );
    assert!(intent_command(&home, run, "inherited")
        .output()
        .unwrap()
        .status
        .success());
}

#[test]
fn public_caller_merge_is_keyed_fenced_and_retriable() {
    use std::os::fd::AsRawFd;
    let (home, workspace, tools) = fixture();
    let created = create(&home, &workspace, &tools, "merge-cli", None);
    assert!(created.status.success(), "{created:?}");
    let value: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let run = value["data"]["run_id"].as_str().unwrap();
    let checkout = value["data"]["worktree_path"].as_str().unwrap();
    std::fs::write(Path::new(checkout).join("change.txt"), "new work\n").unwrap();
    git(Path::new(checkout), &["add", "change.txt"]);
    git(
        Path::new(checkout),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "change",
        ],
    );
    let command = |key: &str, dry: bool| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
        cmd.args([
            "--output",
            "json",
            "run",
            "merge",
            run,
            "--node-id",
            "n-0001",
            "--settlement-key",
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
        .env("PATH", tools.path());
        if dry {
            cmd.arg("--dry-run");
        }
        cmd.output().unwrap()
    };
    let preview = command("stable", true);
    assert!(preview.status.success(), "{preview:?}");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&preview.stdout).unwrap()["data"]
            ["writer_fence"]["state"],
        "not-claimed"
    );
    let lock = std::fs::File::open(home.path().join("runs").join(run).join("writer.lock")).unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_SH) }, 0);
    let busy = command("stable", false);
    assert!(!busy.status.success(), "{busy:?}");
    let err: serde_json::Value = serde_json::from_slice(&busy.stderr).unwrap();
    assert_eq!(err["error"]["code"], "writer_active");
    assert_eq!(err["error"]["details"]["phase"], "intent-recorded");
    assert!(!command("other", false).status.success());
    drop(lock);
    let merged = command("stable", false);
    assert!(merged.status.success(), "{merged:?}");
    let result: serde_json::Value = serde_json::from_slice(&merged.stdout).unwrap();
    assert_eq!(result["data"]["merged"], true);
    assert!(matches!(
        result["data"]["cleanup"].as_str(),
        Some("pending" | "complete")
    ));
    let retried = command("stable", false);
    assert!(retried.status.success(), "{retried:?}");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&retried.stdout).unwrap()["data"]["merged"],
        true
    );
    assert!(!workspace.path().join("forbidden").exists());
    let events =
        std::fs::read_to_string(home.path().join("runs").join(run).join("events.jsonl")).unwrap();
    for line in events.lines() {
        let _: serde_json::Value = serde_json::from_str(line).unwrap();
    }
    assert_eq!(events.matches("\"kind\":\"merge.started\"").count(), 1);
}

#[test]
fn caller_cancel_waits_for_writer_and_preserves_checkout() {
    use std::os::fd::AsRawFd;
    let (home, workspace, tools) = fixture();
    let created = create(&home, &workspace, &tools, "cancel-fixture", None);
    assert!(created.status.success(), "{created:?}");
    let data: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let run = data["data"]["run_id"].as_str().unwrap();
    let checkout = data["data"]["worktree_path"].as_str().unwrap();
    let branch = data["data"]["branch"].as_str().unwrap();
    let run_dir = home.path().join("runs").join(run);
    let fd = std::fs::File::open(run_dir.join("writer.lock")).unwrap();
    assert_eq!(
        unsafe { libc::flock(fd.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) },
        0
    );
    let invoke = |key: &str| {
        Command::new(env!("CARGO_BIN_EXE_taskfleet"))
            .args([
                "--output",
                "json",
                "run",
                "cancel",
                run,
                "--settlement-key",
                key,
                "--note",
                "operator requested stop",
            ])
            .env("TASKFLEET_HOME", home.path())
            .env("GIT_BIN", fixture_git_binary())
            .env("TMUX_BIN", tools.path().join("tmux"))
            .env("WORKMUX_BIN", tools.path().join("workmux"))
            .env("PATH", tools.path())
            .output()
            .unwrap()
    };
    let busy = invoke("stable-key");
    assert!(!busy.status.success());
    let error: serde_json::Value = serde_json::from_slice(&busy.stderr).unwrap();
    assert_eq!(error["error"]["code"], "writer_active");
    assert_eq!(error["error"]["details"]["phase"], "intent-recorded");
    let conflict = invoke("different-key");
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("settlement_conflict"));
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(run_dir.join("manifest.json")).unwrap()).unwrap();
    assert!(!matches!(manifest["status"].as_str(), Some("cancelled")));
    assert!(Path::new(checkout).exists());
    drop(fd);
    let ok = invoke("stable-key");
    assert!(ok.status.success(), "{ok:?}");
    let replay = invoke("stable-key");
    assert!(replay.status.success(), "{replay:?}");
    let events = std::fs::read_to_string(run_dir.join("events.jsonl")).unwrap();
    assert_eq!(
        events
            .matches("\"kind\":\"caller.settlement_intent\"")
            .count(),
        1
    );
    assert_eq!(events.matches("\"kind\":\"run.status\"").count(), 1);
    // The independent supervisor must record preservation and wind down; it
    // cannot run caller-owned tmux/workmux teardown on cancellation.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let current = std::fs::read_to_string(run_dir.join("events.jsonl")).unwrap();
        if current.contains("caller cancelled (branch preserved)") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "supervisor did not record preservation"
        );
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    assert!(Path::new(checkout).exists());
    git(
        &workspace.path().join("repo"),
        &["show-ref", "--verify", &format!("refs/heads/{branch}")],
    );
    assert!(!workspace.path().join("forbidden").exists());
}

#[test]
fn caller_cancel_refuses_replaced_lock_and_unrelated_terminal_report() {
    use std::os::unix::fs::symlink;
    let (home, workspace, tools) = fixture();
    let created = create(&home, &workspace, &tools, "cancel-fence-replace", None);
    assert!(created.status.success());
    let data: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let run = data["data"]["run_id"].as_str().unwrap();
    let dir = home.path().join("runs").join(run);
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_taskfleet"))
            .args([
                "--output",
                "json",
                "run",
                "cancel",
                run,
                "--settlement-key",
                "key",
                "--note",
                "operator request",
            ])
            .env("TASKFLEET_HOME", home.path())
            .env("GIT_BIN", fixture_git_binary())
            .env("PATH", tools.path())
            .output()
            .unwrap()
    };
    let lock = dir.join("writer.lock");
    std::fs::rename(&lock, dir.join("old-writer.lock")).unwrap();
    symlink("old-writer.lock", &lock).unwrap();
    let refused = invoke();
    assert!(String::from_utf8_lossy(&refused.stderr).contains("writer_fence_unavailable"));
    let events = std::fs::read_to_string(dir.join("events.jsonl")).unwrap();
    assert!(!events.contains("caller.settlement_intent"));
    std::fs::remove_file(&lock).unwrap();
    std::fs::rename(dir.join("old-writer.lock"), &lock).unwrap();
    let paths = taskfleet_core::RunPaths::new(dir, run).unwrap();
    let nid = taskfleet_core::NodeId::parse_str("n-0001").unwrap();
    taskfleet_core::append_and_apply_event(
        &paths,
        "node.report",
        Some(&nid),
        None,
        serde_json::json!({"success":true,"cancelled":false,"summary":"done"}),
    )
    .unwrap();
    let refused = invoke();
    assert!(String::from_utf8_lossy(&refused.stderr).contains("settlement_conflict"));
    assert!(!std::fs::read_to_string(paths.events())
        .unwrap()
        .contains("caller.settlement_intent"));
}

#[test]
fn failed_caller_salvage_adopts_merge_and_cleans_without_host_tools() {
    use std::os::fd::AsRawFd;
    use taskfleet_core::{append_and_apply_event, NodeId, RunPaths};
    let (home, workspace, tools) = fixture();
    let created = create(&home, &workspace, &tools, "salvage-failed", None);
    assert!(created.status.success(), "{created:?}");
    let data: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let run = data["data"]["run_id"].as_str().unwrap();
    let checkout = data["data"]["worktree_path"].as_str().unwrap();
    let branch = data["data"]["branch"].as_str().unwrap();
    let dir = home.path().join("runs").join(run);
    let paths = RunPaths::new(dir.clone(), run).unwrap();
    let nid = NodeId::parse_str("n-0001").unwrap();
    std::fs::write(Path::new(checkout).join("change.txt"), "salvaged\n").unwrap();
    git(Path::new(checkout), &["add", "change.txt"]);
    git(
        Path::new(checkout),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "change",
        ],
    );
    append_and_apply_event(
        &paths,
        "node.report",
        Some(&nid),
        None,
        serde_json::json!({"success":false,"summary":"writer failed"}),
    )
    .unwrap();
    append_and_apply_event(
        &paths,
        "run.status",
        None,
        None,
        serde_json::json!({"status":"failed"}),
    )
    .unwrap();
    let command = |key: &str, extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_taskfleet"))
            .args([
                "--output",
                "json",
                "run",
                "salvage",
                run,
                "--settlement-key",
                key,
            ])
            .args(extra)
            .env("TASKFLEET_HOME", home.path())
            .env("GIT_BIN", fixture_git_binary())
            .env("TMUX_BIN", tools.path().join("tmux"))
            .env("WORKMUX_BIN", tools.path().join("workmux"))
            .env(
                "TASKFLEET_TEST_FORBIDDEN",
                workspace.path().join("forbidden"),
            )
            .env("PATH", tools.path())
            .output()
            .unwrap()
    };
    assert!(!command("key", &["--fence"]).status.success());
    let dry = command("key", &["--dry-run"]);
    assert!(dry.status.success(), "{dry:?}");
    assert!(!std::fs::read_to_string(paths.events())
        .unwrap()
        .contains("caller.settlement_intent"));
    let fd = std::fs::File::open(dir.join("writer.lock")).unwrap();
    assert_eq!(unsafe { libc::flock(fd.as_raw_fd(), libc::LOCK_SH) }, 0);
    let busy = command("key", &[]);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&busy.stderr).unwrap()["error"]["code"],
        "writer_active"
    );
    assert!(Path::new(checkout).exists());
    drop(fd);
    let ok = command("key", &[]);
    assert!(ok.status.success(), "{ok:?}");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&ok.stdout).unwrap()["data"]["merged"],
        true
    );
    assert!(!command("different", &[]).status.success());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        let m = taskfleet_core::read_manifest_opt(&paths).unwrap().unwrap();
        if m.status == taskfleet_core::Status::Done
            && !Path::new(checkout).exists()
            && !Command::new(fixture_git_binary())
                .args([
                    "-C",
                    workspace.path().join("repo").to_str().unwrap(),
                    "show-ref",
                    "--verify",
                    &format!("refs/heads/{branch}"),
                ])
                .output()
                .unwrap()
                .status
                .success()
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "salvage did not converge: {m:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    assert!(!workspace.path().join("forbidden").exists());
    let events = std::fs::read_to_string(paths.events()).unwrap();
    assert_eq!(events.matches("\"kind\":\"merge.started\"").count(), 1);
    assert!(events.contains("cleanup.caller_verified"));
    assert!(!Command::new(fixture_git_binary())
        .args([
            "-C",
            workspace.path().join("repo").to_str().unwrap(),
            "show-ref",
            "--verify",
            &format!("refs/heads/{branch}")
        ])
        .status()
        .unwrap()
        .success());
}

fn caller_merge_cmd(home: &TestHome, tools: &tempfile::TempDir, run: &str) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    cmd.args([
        "--output",
        "json",
        "run",
        "merge",
        run,
        "--node-id",
        "n-0001",
        "--settlement-key",
        "landing-proof",
    ])
    .env("TASKFLEET_HOME", home.path())
    .env("GIT_BIN", fixture_git_binary())
    .env("PATH", tools.path());
    cmd
}

fn commit_file(repo: &Path, file: &str, body: &str) {
    std::fs::write(repo.join(file), body).unwrap();
    git(repo, &["add", file]);
    git(
        repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            file,
        ],
    );
}

#[test]
fn caller_merge_noop_already_integrated_reports_and_cleans() {
    let (home, workspace, tools) = fixture();
    let created = create(&home, &workspace, &tools, "already-integrated", None);
    let data: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let run = data["data"]["run_id"].as_str().unwrap();
    let checkout = data["data"]["worktree_path"].as_str().unwrap();
    let branch = data["data"]["branch"].as_str().unwrap();
    let merged = caller_merge_cmd(&home, &tools, run).output().unwrap();
    assert!(merged.status.success(), "{merged:?}");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&merged.stdout).unwrap()["data"]["merged"],
        true
    );
    let repo = workspace.path().join("repo");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while Path::new(checkout).exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "caller no-op cleanup did not converge"
        );
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    let retry = caller_merge_cmd(&home, &tools, run).output().unwrap();
    assert!(retry.status.success(), "{retry:?}");
    let response: serde_json::Value = serde_json::from_slice(&retry.stdout).unwrap();
    assert_eq!(response["data"]["cleanup"], "complete");
    assert!(!String::from_utf8_lossy(
        &Command::new(fixture_git_binary())
            .arg("-C")
            .arg(repo)
            .args(["worktree", "list", "--porcelain"])
            .output()
            .unwrap()
            .stdout
    )
    .contains(checkout));
    assert!(!String::from_utf8_lossy(&retry.stderr).contains(branch));
}

#[test]
fn caller_merge_rebased_tip_lands_and_cleans() {
    let (home, workspace, tools) = fixture();
    let repo = workspace.path().join("repo");
    commit_file(
        &repo,
        "context.txt",
        "alpha\none\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nbeta\ngamma\n",
    );
    let created = create(&home, &workspace, &tools, "rebased-tip", None);
    let data: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let run = data["data"]["run_id"].as_str().unwrap();
    let checkout = Path::new(data["data"]["worktree_path"].as_str().unwrap());
    // Worker and source edit nearby context, so rebase changes the immutable
    // worker commit and its patch-id rather than only its parent.
    commit_file(
        checkout,
        "context.txt",
        "alpha\none\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nbeta worker\ngamma\n",
    );
    let original = Command::new(fixture_git_binary())
        .arg("-C")
        .arg(checkout)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap()
        .stdout;
    // A clean nearby source edit alters context while keeping the patch applicable.
    commit_file(
        &repo,
        "context.txt",
        "alpha source\none\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nbeta\ngamma\n",
    );
    let merged = caller_merge_cmd(&home, &tools, run).output().unwrap();
    assert!(merged.status.success(), "{merged:?}");
    let events =
        std::fs::read_to_string(home.path().join("runs").join(run).join("events.jsonl")).unwrap();
    assert!(events.contains("merge.rebased"));
    let retry = caller_merge_cmd(&home, &tools, run).output().unwrap();
    assert!(retry.status.success(), "{retry:?}");
    assert_ne!(
        original,
        Command::new(fixture_git_binary())
            .arg("-C")
            .arg(&repo)
            .args(["rev-parse", "main"])
            .output()
            .unwrap()
            .stdout
    );
}

#[test]
fn caller_same_key_retries_rejected_dead_transaction_in_one_call() {
    let (home, workspace, tools) = fixture();
    let created = create(&home, &workspace, &tools, "stale-txn", None);
    let data: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let run = data["data"]["run_id"].as_str().unwrap();
    let checkout = Path::new(data["data"]["worktree_path"].as_str().unwrap());
    let branch = data["data"]["branch"].as_str().unwrap();
    commit_file(checkout, "change.txt", "work\n");
    let intent = intent_command(&home, run, "landing-proof")
        .output()
        .unwrap();
    assert!(intent.status.success(), "{intent:?}");
    let paths = taskfleet_core::RunPaths::new(home.path().join("runs").join(run), run).unwrap();
    let manifest = taskfleet_core::read_manifest_opt(&paths).unwrap().unwrap();
    let intent = manifest.caller_settlement_intent.unwrap();
    let repo = workspace.path().join("repo");
    let oid = |rev: &str| {
        String::from_utf8(
            Command::new(fixture_git_binary())
                .arg("-C")
                .arg(&repo)
                .args(["rev-parse", rev])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string()
    };
    let node_id = taskfleet_core::NodeId::parse_str("n-0001").unwrap();
    taskfleet_core::append_and_apply_event(
        &paths,
        taskfleet_core::KIND_MERGE_STARTED,
        Some(&node_id),
        None,
        serde_json::json!({
            "op_id":"dead-operation", "source_branch":"main", "worker_branch":branch,
            "expected_source_oid":oid("main"), "worker_oid":oid(branch),
            "base_sha":oid("main"), "driver_pid":null, "driver_pid_start_secs":null,
            "started_at":chrono::Utc::now(),
            "caller_authority":{"intent_seq":intent.seq,"intent_key":intent.key,
                "writer_dev":intent.writer_dev,"writer_ino":intent.writer_ino}
        }),
    )
    .unwrap();
    let mut cmd = caller_merge_cmd(&home, &tools, run);
    cmd.args(["--actor", "fixture"]);
    let merged = cmd.output().unwrap();
    assert!(merged.status.success(), "{merged:?}");
    let events = std::fs::read_to_string(paths.events()).unwrap();
    assert!(events.contains("merge.aborted"));
    assert_eq!(events.matches("\"kind\":\"merge.started\"").count(), 2);
}
