//! Cold read-contract fixtures: no harness, tmux, supervisor, or user manager.
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use taskfleet_core::{
    append_and_apply_event, append_and_apply_unlocked, NodeId, RunLock, RunPaths,
};
use tempfile::TempDir;

const RUN: &str = "01m08c08v5jxzfqf3r36n0sgzd";
const SESSION: &str = "018f5f64-b137-7d44-b2b4-4f02c3f646e8";
const NEXT: &str = "018f5f64-b137-7d44-b2b4-4f02c3f646e9";

fn seed(home: &Path, harness: Option<&str>) -> RunPaths {
    let paths = RunPaths::new(home.join("runs").join(RUN), RUN).unwrap();
    append_and_apply_event(
        &paths,
        "run.created",
        None,
        None,
        json!({
            "kind":"spinoff", "lifecycle":"autonomous", "title":"cold transcript", "harness":harness
        }),
    )
    .unwrap();
    paths
}

fn spawn_data(home: &Path, session: &str, attempt: u32) -> Value {
    json!({"kind":"spinoff", "worktree_path":home.join("checkout"),
        "pi_session_id":session, "pi_session_cwd":home.join("checkout"),
        "pi_session_path":format!(".creating/pi-sessions/{RUN}/pi-session-{session}.jsonl"),
        "attempt":attempt})
}

fn node(paths: &RunPaths, id: &str, data: Value) {
    append_and_apply_event(
        paths,
        "node.created",
        Some(&NodeId::parse_str(id).unwrap()),
        None,
        data,
    )
    .unwrap();
}

fn archive_data(session: &str, attempt: u32) -> Value {
    json!({"session_id":session, "attempt":attempt,
        "transcript_path":"evidence/n-0001/pi-session.original.jsonl",
        "resume_path":"evidence/n-0001/pi-session.resume.jsonl",
        "pane_path":"evidence/n-0001/final-pane.log",
        "report_path":"evidence/n-0001/terminal-report.json",
        "transcript_sha256":"0".repeat(64)})
}

fn command(home: &Path, verb: &str) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("TASKFLEET_HOME", home)
        .env("HOME", home)
        .args(["run", verb, "--json"]);
    if verb == "show" {
        cmd.arg(RUN);
    }
    cmd
}

fn decode(output: std::process::Output, verb: &str) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    if verb == "show" {
        v["data"].clone()
    } else {
        v["data"]["runs"][0].clone()
    }
}

fn row(home: &Path, verb: &str) -> Value {
    decode(command(home, verb).output().unwrap(), verb)
}

fn both(home: &Path) -> Value {
    let show = row(home, "show");
    let list = row(home, "list");
    assert_eq!(show["node_transcripts"], list["node_transcripts"]);
    show["node_transcripts"].clone()
}

#[test]
fn cold_legacy_empty_and_unsupported_runs_are_explicit() {
    for (harness, support, availability) in [
        (None, "unknown", "not-recorded"),
        (Some("pi"), "supported", "not-recorded"),
        (Some("claude"), "unsupported", "unsupported"),
    ] {
        let home = TempDir::new().unwrap();
        let paths = seed(home.path(), harness);
        assert_eq!(both(home.path()), json!([]));
        node(&paths, "n-0002", json!({"kind":"spinoff"}));
        node(&paths, "n-0001", json!({"kind":"spinoff"}));
        let views = both(home.path());
        assert_eq!(views.as_array().unwrap().len(), 2);
        assert_eq!(views[0]["node_id"], "n-0001");
        for view in views.as_array().unwrap() {
            assert_eq!(view["support"], support);
            assert_eq!(view["harness"], json!(harness));
            assert!(view["session_id"].is_null());
            assert!(view["attempt"].is_null());
            for artifact in ["live", "original"] {
                assert_eq!(view[artifact]["availability"], availability);
                assert!(view[artifact]["path"].is_null());
            }
        }
    }
}

#[test]
fn pending_failed_missing_archive_and_attempt_identity() {
    let home = TempDir::new().unwrap();
    let paths = seed(home.path(), Some("pi"));
    node(&paths, "n-0001", spawn_data(home.path(), SESSION, 0));
    let views = both(home.path());
    assert_eq!(
        views[0],
        json!({
            "node_id":"n-0001", "harness":"pi", "attempt":0,
            "session_id":SESSION, "original_cwd":home.path().join("checkout"),
            "worktree_path":home.path().join("checkout"), "support":"supported",
            "capture_status":"pending",
            "live":{"anchor":"state-root","path":null,"availability":"missing"},
            "original":{"anchor":"run-root","path":null,"availability":"not-recorded"}
        })
    );
    let id = NodeId::parse_str("n-0001").unwrap();
    append_and_apply_event(
        &paths,
        "worker.evidence.failed",
        Some(&id),
        None,
        json!({"attempt":0,"session_id":SESSION,"error":"fixture capture failure"}),
    )
    .unwrap();
    assert_eq!(both(home.path())[0]["capture_status"], "failed");
    append_and_apply_event(
        &paths,
        "node.retry",
        Some(&id),
        None,
        spawn_data(home.path(), NEXT, 1),
    )
    .unwrap();
    let view = &both(home.path())[0];
    assert_eq!(view["session_id"], NEXT);
    assert_eq!(view["attempt"], 1);
    // A stale prior-attempt archive cannot replace the current identity.
    append_and_apply_event(
        &paths,
        "worker.evidence.archived",
        Some(&id),
        None,
        archive_data(SESSION, 0),
    )
    .unwrap();
    assert_eq!(both(home.path())[0]["capture_status"], "pending");
    append_and_apply_event(
        &paths,
        "worker.evidence.archived",
        Some(&id),
        None,
        archive_data(NEXT, 1),
    )
    .unwrap();
    let view = &both(home.path())[0];
    assert_eq!(view["capture_status"], "complete");
    assert_eq!(view["original"]["availability"], "missing");
    assert!(view["original"]["path"].is_null());
    // An unusable recorded target is distinct from an absent one.
    std::fs::create_dir_all(paths.root.join("evidence/n-0001/pi-session.original.jsonl")).unwrap();
    assert_eq!(
        both(home.path())[0]["original"]["availability"],
        "unavailable"
    );
}

#[test]
fn readers_observe_one_locked_archive_generation_across_multiple_nodes() {
    for verb in ["show", "list"] {
        let home = TempDir::new().unwrap();
        let paths = seed(home.path(), Some("pi"));
        node(&paths, "n-0001", spawn_data(home.path(), SESSION, 0));
        node(&paths, "n-0002", spawn_data(home.path(), NEXT, 0));
        assert_eq!(both(home.path())[0]["capture_status"], "pending");
        let lock = RunLock::acquire(&paths.lock()).unwrap();
        let child = command(home.path(), verb).spawn().unwrap();
        // Both readers must wait for this generation's exclusive transaction,
        // not combine its manifest status with the prior evidence projection.
        append_and_apply_unlocked(
            &lock.witness(),
            &paths,
            "worker.evidence.archived",
            Some(&NodeId::parse_str("n-0001").unwrap()),
            None,
            archive_data(SESSION, 0),
        )
        .unwrap();
        append_and_apply_unlocked(
            &lock.witness(),
            &paths,
            "run.status",
            None,
            None,
            json!({"status":"done"}),
        )
        .unwrap();
        drop(lock);
        let output = child.wait_with_output().unwrap();
        let view = decode(output, verb);
        assert_eq!(view["status"], "done");
        assert_eq!(view["node_transcripts"][0]["capture_status"], "complete");
        assert_eq!(view["node_transcripts"][1]["session_id"], NEXT);
    }
}

#[test]
fn list_retains_history_row_when_transcript_inventory_is_unreadable() {
    let home = TempDir::new().unwrap();
    let paths = seed(home.path(), Some("pi"));
    node(&paths, "n-0001", spawn_data(home.path(), SESSION, 0));
    std::fs::write(paths.nodes_dir().join("n-0002.json"), b"broken projection").unwrap();
    let output = command(home.path(), "list").output().unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["data"]["runs"][0]["run_id"], RUN);
    assert_eq!(value["data"]["runs"][0]["node_transcripts"], json!([]));
    assert_eq!(
        value["data"]["runs"][0]["worktree_path"],
        json!(home.path().join("checkout"))
    );
    assert!(value["warnings"].as_array().unwrap().iter().any(|w| w
        .as_str()
        .unwrap()
        .contains("transcript discovery unavailable")));
    assert!(!command(home.path(), "show")
        .output()
        .unwrap()
        .status
        .success());
}
