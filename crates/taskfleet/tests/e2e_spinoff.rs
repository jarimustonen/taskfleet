//! One complete native materialization → supervise → merge → teardown roundtrip.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

mod common;
use common::{NativeSpawnTools, TestHome};

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn run_ok(command: &mut Command) -> Value {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn wait_event(path: &Path, kind: &str) {
    wait_event_count(path, kind, 1);
}

fn wait_event_count(path: &Path, kind: &str, wanted: usize) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let count = std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter(|line| {
                serde_json::from_str::<Value>(line).is_ok_and(|event| event["kind"] == kind)
            })
            .count();
        if count >= wanted {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "timed out waiting for {wanted} {kind} events: {}",
        std::fs::read_to_string(path).unwrap_or_default()
    );
}

#[test]
fn native_spinoff_round_trip_reaches_done_and_tears_down() {
    terminal_history_round_trip(false, "done");
}

#[test]
fn interactive_merge_retains_history_after_teardown_and_restart() {
    terminal_history_round_trip(true, "done");
}

#[test]
fn cancelled_work_retains_history_after_teardown_and_restart() {
    terminal_history_round_trip(false, "cancelled");
}

#[test]
fn failed_work_retains_history_after_teardown_and_restart() {
    terminal_history_round_trip(false, "failed");
}

fn terminal_history_round_trip(interactive: bool, status: &str) {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let tools = NativeSpawnTools::new();
    let worker = scratch.path().join("worker.sh");
    executable(&worker, "#!/bin/sh\nexec /bin/sleep 120\n");
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            r#"[profiles.e2e]
description="e2e"
capability="fast"
residency="local"
agents=[{{harness="pi",command=["{}"],telemetry="worker-v1"}}]
[profile]
default="e2e"
"#,
            worker.display()
        ),
    )
    .unwrap();
    let worktree = tools.worktree("worktree");

    let mut create = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    create
        .env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path());
    tools.configure(&mut create, &worktree, "headless");
    create.env(
        "TASKFLEET_CREATOR_SESSION",
        "pi:018f5f64-b137-7d44-b2b4-4f02c3f646e8",
    );
    create.args([
        "--output",
        "json",
        "run",
        "create",
        "--source-branch",
        "main",
        "--source-repo",
        tools.repo_path().to_str().unwrap(),
    ]);
    if interactive {
        create.arg("--interactive");
    }
    let created = run_ok(create.args([
        "--kind",
        "spinoff",
        "--headless",
        "--title",
        "native e2e",
        "--task",
        "do work",
    ]));
    let run_id = created["data"]["run_id"].as_str().unwrap();
    let run_dir = home.path().join("runs").join(run_id);
    let events = run_dir.join("events.jsonl");
    wait_event(&events, "supervisor.started");
    std::fs::write(worktree.join("WORK"), "committed work").unwrap();
    let commit = Command::new("git")
        .current_dir(&worktree)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "add",
            "-A",
        ])
        .output()
        .unwrap();
    assert!(commit.status.success());
    let commit = Command::new("git")
        .current_dir(&worktree)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "worker work",
        ])
        .output()
        .unwrap();
    assert!(
        commit.status.success(),
        "{}",
        String::from_utf8_lossy(&commit.stderr)
    );

    let worker_oid = git(&worktree, &["rev-parse", "HEAD"]);
    // Force a genuine rebase: the source advances independently after the
    // worker fork. Rebasing must change the worker commit's parent and hash.
    std::fs::write(tools.repo_path().join("SOURCE"), "independent source work").unwrap();
    git(tools.repo_path(), &["add", "SOURCE"]);
    git(
        tools.repo_path(),
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "source advance",
        ],
    );
    let expected_source = git(tools.repo_path(), &["rev-parse", "main"]);

    if status == "done" {
        let merged = run_ok(
            Command::new(env!("CARGO_BIN_EXE_taskfleet"))
                .env("TASKFLEET_HOME", home.path())
                .env("HOME", home.path())
                .args([
                    "--output", "json", "run", "merge", run_id, "--source", "main",
                ]),
        );
        assert_eq!(merged["data"]["merged"], true);
    } else if status == "cancelled" {
        run_ok(
            Command::new(env!("CARGO_BIN_EXE_taskfleet"))
                .env("TASKFLEET_HOME", home.path())
                .env("HOME", home.path())
                .args(["run", "cancel", run_id, "--json"]),
        );
    } else {
        let report = scratch.path().join("report.json");
        std::fs::write(
            &report,
            r#"{"success":false,"summary":"history fixture failure"}"#,
        )
        .unwrap();
        run_ok(
            Command::new(env!("CARGO_BIN_EXE_taskfleet"))
                .env("TASKFLEET_HOME", home.path())
                .env("HOME", home.path())
                .args([
                    "node",
                    "report",
                    run_id,
                    "n-0001",
                    "--from-file",
                    report.to_str().unwrap(),
                    "--json",
                ]),
        );
    }
    wait_event(&events, "supervisor.exited");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(run_dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["status"], status);
    assert_eq!(worktree.exists(), status != "done");
    let before = assert_terminal_history(home.path(), run_id, status, interactive);
    // Re-enter the actual supervisor against durable terminal state. No worker
    // is launched and no ambient manager/server is consulted.
    let mut restart = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    restart
        .env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path());
    tools.configure(&mut restart, &worktree, "headless");
    run_ok(restart.args(["supervise", run_id, "--once", "--json"]));
    let after = assert_terminal_history(home.path(), run_id, status, interactive);
    assert_eq!(before["report"], after["report"]);
    if status == "done" {
        let origin = &after["report"]["origin"];
        let integrated = origin["integrated_oid"]
            .as_str()
            .expect("exact native evidence");
        assert_ne!(integrated, worker_oid, "a rebase actually changed the hash");
        assert_eq!(origin["worker_oid"], worker_oid);
        let stream: Vec<Value> = std::fs::read_to_string(&events)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(!stream.iter().any(|e| e["kind"] == "merge.rebased"));
        let start = stream
            .iter()
            .find(|e| e["kind"] == "merge.started")
            .unwrap();
        let prepared = stream
            .iter()
            .find(|e| e["kind"] == "merge.native_prepared")
            .unwrap();
        assert_eq!(origin["op_id"], start["data"]["op_id"]);
        assert_eq!(prepared["data"]["op_id"], origin["op_id"]);
        assert_eq!(prepared["data"]["integrated_oid"], integrated);
        assert_eq!(start["data"]["expected_source_oid"], expected_source);
        // Advance source again: history reads must not substitute today's tip.
        git(
            tools.repo_path(),
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "later",
            ],
        );
        assert_ne!(git(tools.repo_path(), &["rev-parse", "main"]), integrated);
        let retained = assert_terminal_history(home.path(), run_id, status, interactive);
        assert_eq!(retained["report"]["origin"], *origin);
        let range = format!("{expected_source}..{integrated}");
        assert_eq!(
            git(tools.repo_path(), &["rev-list", "--count", &range]),
            "1"
        );
        assert_eq!(
            git(tools.repo_path(), &["diff", "--name-only", &range]),
            "WORK"
        );
        assert_eq!(
            git(tools.repo_path(), &["diff", "--numstat", &range]),
            "1\t0\tWORK"
        );
    }

    assert_eq!(before["node_transcripts"], after["node_transcripts"]);
    let kinds: Vec<String> = std::fs::read_to_string(events)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|event| event["kind"].as_str().map(str::to_owned))
        .collect();
    for required in [
        "run.created",
        "node.created",
        "supervisor.started",
        "node.report",
        "run.status",
        "supervisor.exited",
    ] {
        assert!(
            kinds.iter().any(|kind| kind == required),
            "missing {required}: {kinds:?}"
        );
    }
}

fn assert_terminal_history(home: &Path, id: &str, status: &str, interactive: bool) -> Value {
    let read = |verb: &str| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
        cmd.env("TASKFLEET_HOME", home)
            .env("HOME", home)
            .args(["run", verb, "--json"]);
        if verb == "show" {
            cmd.arg(id);
        }
        run_ok(&mut cmd)
    };
    let shown = read("show")["data"].clone();
    let listed = read("list");
    let row = listed["data"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["run_id"] == id)
        .unwrap();
    for view in [&shown, row] {
        assert_eq!(view["status"], status);
        assert_eq!(
            view["lifecycle"],
            if interactive {
                "interactive"
            } else {
                "autonomous"
            }
        );
        assert_eq!(view["attention_required"], false);
        assert_eq!(view["source_branch"], "main");
        assert!(view["source_repo"].is_string());
        assert!(view["created_at"].is_string());
        assert_eq!(
            view["created_by_session"]["session_id"],
            "018f5f64-b137-7d44-b2b4-4f02c3f646e8"
        );
        assert_eq!(
            view["node_transcripts"][0]["original"]["availability"],
            "available"
        );
    }
    assert_eq!(shown["landed"], status == "done");
    assert_eq!(shown["report"]["success"], status == "done");
    assert!(shown["manifest"]["updated_at"].is_string());
    if status == "done" {
        assert!(shown["report"]["origin"]["worker_oid"].is_string());
        assert_eq!(shown["preserved_work"], serde_json::json!([]));
    } else {
        assert_eq!(shown["preserved_work"][0]["worktree_present"], true);
        assert_eq!(shown["preserved_work"][0]["branch_present"], true);
        assert_eq!(shown["preserved_work"][0]["unmerged_commits"], 1);
        assert!(shown["report"]["summary"].is_string());
    }
    let archive = home
        .join("runs")
        .join(id)
        .join("evidence/n-0001/terminal-report.json");
    let report: Value = serde_json::from_slice(&std::fs::read(archive).unwrap()).unwrap();
    assert_eq!(report, shown["report"]);
    shown
}

#[test]
fn failed_supervised_run_merge_recovers_done_and_tears_down() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let tools = NativeSpawnTools::new();
    let worker = scratch.path().join("worker.sh");
    executable(&worker, "#!/bin/sh\nexec /bin/sleep 120\n");
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            r#"[profiles.e2e]
description="e2e"
capability="fast"
residency="local"
agents=[{{harness="pi",command=["{}"],telemetry="worker-v1"}}]
[profile]
default="e2e"
"#,
            worker.display()
        ),
    )
    .unwrap();
    let worktree = tools.worktree("recovery-worktree");

    let mut create = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    create
        .env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path());
    tools.configure(&mut create, &worktree, "headless");
    let created = run_ok(create.args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--headless",
        "--title",
        "native recovery e2e",
        "--task",
        "do work",
    ]));
    let run_id = created["data"]["run_id"].as_str().unwrap();
    let run_dir = home.path().join("runs").join(run_id);
    let events = run_dir.join("events.jsonl");
    wait_event(&events, "supervisor.started");
    std::fs::write(worktree.join("WORK"), "committed work").unwrap();
    let commit = Command::new("git")
        .current_dir(&worktree)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "add",
            "-A",
        ])
        .output()
        .unwrap();
    assert!(commit.status.success());
    let commit = Command::new("git")
        .current_dir(&worktree)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "worker work",
        ])
        .output()
        .unwrap();
    assert!(
        commit.status.success(),
        "{}",
        String::from_utf8_lossy(&commit.stderr)
    );

    let failed_report = scratch.path().join("failed.json");
    std::fs::write(
        &failed_report,
        r#"{"success":false,"summary":"preserve for recovery"}"#,
    )
    .unwrap();
    run_ok(
        Command::new(env!("CARGO_BIN_EXE_taskfleet"))
            .env("TASKFLEET_HOME", home.path())
            .env("HOME", home.path())
            .args([
                "--output",
                "json",
                "node",
                "report",
                run_id,
                "n-0001",
                "--from-file",
                failed_report.to_str().unwrap(),
            ]),
    );
    wait_for_manifest_status(&run_dir, "failed");
    wait_event(&events, "supervisor.exited");
    assert!(worktree.exists(), "failed work is preserved for recovery");

    let mut merge_command = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    merge_command
        .env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path());
    // The reattached supervisor inherits this command's dependency seams; keep
    // them private and explicit just like the original create invocation.
    tools.configure(&mut merge_command, &worktree, "headless");
    let merged = run_ok(merge_command.args([
        "--output", "json", "run", "merge", run_id, "--source", "main",
    ]));
    assert_eq!(merged["data"]["merged"], true);
    wait_for_manifest_status(&run_dir, "done");
    let shown = run_ok(
        Command::new(env!("CARGO_BIN_EXE_taskfleet"))
            .env("TASKFLEET_HOME", home.path())
            .env("HOME", home.path())
            .args(["--output", "json", "run", "show", run_id]),
    );
    assert_eq!(shown["data"]["manifest"]["status"], "done");
    assert_eq!(shown["data"]["status"], "done");
    assert_eq!(shown["data"]["landed"], true);
    assert_eq!(shown["data"]["report"]["success"], true);
    let node = run_ok(
        Command::new(env!("CARGO_BIN_EXE_taskfleet"))
            .env("TASKFLEET_HOME", home.path())
            .env("HOME", home.path())
            .args(["--output", "json", "node", "show", run_id, "n-0001"]),
    );
    assert_eq!(node["data"]["status"], "done");
    wait_event_count(&events, "supervisor.exited", 2);
    wait_path_missing(&worktree);

    let events: Vec<Value> = std::fs::read_to_string(events)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let merge_seq = events
        .iter()
        .find(|event| {
            event["kind"] == "node.report" && event["data"]["origin"]["kind"] == "run-merge"
        })
        .and_then(|event| event["seq"].as_u64())
        .unwrap();
    let recovery: Vec<&Value> = events
        .iter()
        .filter(|event| {
            event["kind"] == "run.status"
                && event["idempotency_key"]
                    == format!("supervisor-recovery-rollup:{run_id}:merge-report:{merge_seq}")
        })
        .collect();
    assert_eq!(
        recovery.len(),
        1,
        "restart/retry must not duplicate recovery"
    );
    assert_eq!(recovery[0]["data"]["recovery_merge_report_seq"], merge_seq);
}

fn wait_path_missing(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if !path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for teardown of {}", path.display());
}

fn wait_for_manifest_status(run_dir: &Path, wanted: &str) {
    let manifest_path = run_dir.join("manifest.json");
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if std::fs::read(&manifest_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|manifest| manifest["status"] == wanted)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for manifest status {wanted}");
}

#[test]
fn claude_headless_stub_merges_and_archives_truthful_evidence() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let tools = NativeSpawnTools::new();
    let worker = scratch.path().join("claude-stub.sh");
    executable(&worker, "#!/bin/sh\n[ \"$1\" = --print ] || exit 42\n[ \"$2\" = -- ] || exit 43\ncase \"$3\" in *'do work'*) ;; *) exit 44 ;; esac\n[ -z \"${TASKFLEET_RUN_ID:-}\" ] || exit 45\nexec /bin/sleep 120\n");
    std::fs::write(home.path().join("config.toml"), format!(
        "[profiles.claude]\ndescription=\"headless fixture\"\ncapability=\"fast\"\nresidency=\"local\"\nagents=[{{harness=\"claude\",command=[\"{}\"]}}]\n[profile]\ndefault=\"claude\"\n", worker.display())).unwrap();
    let worktree = tools.worktree("claude-merged");
    let mut create = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    create
        .env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path());
    tools.configure(&mut create, &worktree, "headless");
    let output = create
        .args([
            "--output",
            "json",
            "run",
            "create",
            "--kind",
            "spinoff",
            "--headless",
            "--title",
            "claude e2e",
            "--task",
            "do work",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "create: {}; stub: {}",
        String::from_utf8_lossy(&output.stderr),
        std::fs::read_to_string(tools.agent_stderr()).unwrap_or_default()
    );
    let created: Value = serde_json::from_slice(&output.stdout).unwrap();
    let id = created["data"]["run_id"].as_str().unwrap();
    assert_eq!(
        created["data"]["selection"]["selected"]["harness"],
        "claude"
    );
    assert_eq!(
        created["data"]["selection"]["selected"]["telemetry"],
        Value::Null
    );
    let dir = home.path().join("runs").join(id);
    let events = dir.join("events.jsonl");
    wait_event(&events, "supervisor.started");
    std::fs::write(worktree.join("WORK"), "committed work").unwrap();
    for args in [
        vec!["add", "-A"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "work",
        ],
    ] {
        let output = Command::new("git")
            .current_dir(&worktree)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let merged = run_ok(
        Command::new(env!("CARGO_BIN_EXE_taskfleet"))
            .env("TASKFLEET_HOME", home.path())
            .env("HOME", home.path())
            .args(["--output", "json", "run", "merge", id, "--source", "main"]),
    );
    assert_eq!(merged["data"]["merged"], true);
    wait_event(&events, "supervisor.exited");
    wait_path_missing(&worktree);
    let evidence = dir.join("evidence/n-0001");
    assert_eq!(
        serde_json::from_slice::<Value>(
            &std::fs::read(evidence.join("claude-headless.json")).unwrap()
        )
        .unwrap()["transcript"],
        "unavailable"
    );
    assert_eq!(
        std::fs::read_to_string(evidence.join("final-pane.log")).unwrap(),
        "fixture final pane\n"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(
            &std::fs::read(evidence.join("terminal-report.json")).unwrap()
        )
        .unwrap()["via"],
        "explicit-merge"
    );
    assert!(!evidence.join("pi-session.original.jsonl").exists());
}

#[test]
fn claude_headless_nonzero_exit_preserves_unmerged_work() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let tools = NativeSpawnTools::new();
    let worker = scratch.path().join("claude-stub.sh");
    executable(
        &worker,
        "#!/bin/sh\n[ \"$1\" = --print ] || exit 42\n/bin/sleep 1\nexit 17\n",
    );
    std::fs::write(home.path().join("config.toml"), format!(
        "[profiles.claude]\ndescription=\"headless fixture\"\ncapability=\"fast\"\nresidency=\"local\"\nagents=[{{harness=\"claude\",command=[\"{}\"]}}]\n[profile]\ndefault=\"claude\"\n", worker.display())).unwrap();
    let worktree = tools.worktree("claude-failed");
    let mut create = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    create
        .env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path());
    tools.configure(&mut create, &worktree, "headless");
    let created = run_ok(create.args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--headless",
        "--title",
        "claude failure",
        "--task",
        "do work",
    ]));
    let dir = home
        .path()
        .join("runs")
        .join(created["data"]["run_id"].as_str().unwrap());
    std::fs::write(worktree.join("UNMERGED"), "retain").unwrap();
    wait_event(&dir.join("events.jsonl"), "worker.exited");
    wait_for_manifest_status(&dir, "failed");
    wait_event(&dir.join("events.jsonl"), "supervisor.exited");
    assert!(worktree.join("UNMERGED").exists());
    let events = std::fs::read_to_string(dir.join("events.jsonl")).unwrap();
    assert!(
        events.lines().any(|l| serde_json::from_str::<Value>(l)
            .is_ok_and(|v| v["kind"] == "worker.exited" && v["data"]["exit_code"] == 17)),
        "{events}"
    );
    assert!(dir.join("evidence/n-0001/claude-headless.json").exists());
}
