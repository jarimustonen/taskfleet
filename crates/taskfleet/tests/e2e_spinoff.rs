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
    let merge = scratch.path().join("merge.sh");
    executable(&merge, "#!/bin/sh\nexit 0\n");
    let worktree = tools.worktree("worktree");

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
        "native e2e",
        "--task",
        "do work",
    ]));
    let run_id = created["data"]["run_id"].as_str().unwrap();
    let run_dir = home.path().join("runs").join(run_id);
    let events = run_dir.join("events.jsonl");
    wait_event(&events, "supervisor.started");

    let merged = run_ok(
        Command::new(env!("CARGO_BIN_EXE_taskfleet"))
            .env("TASKFLEET_HOME", home.path())
            .env("HOME", home.path())
            .env("TASKFLEET_MERGE_SH", &merge)
            .args(["--output", "json", "run", "merge", run_id]),
    );
    assert_eq!(merged["data"]["merged"], true);
    wait_event(&events, "supervisor.exited");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(run_dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["status"], "done");
    assert!(
        !worktree.exists(),
        "supervisor teardown removed the worktree"
    );
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
    let merge = scratch.path().join("merge.sh");
    executable(&merge, "#!/bin/sh\nexit 0\n");
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
        .env("HOME", home.path())
        .env("TASKFLEET_MERGE_SH", &merge);
    // The reattached supervisor inherits this command's dependency seams; keep
    // them private and explicit just like the original create invocation.
    tools.configure(&mut merge_command, &worktree, "headless");
    let merged = run_ok(merge_command.args(["--output", "json", "run", "merge", run_id]));
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
