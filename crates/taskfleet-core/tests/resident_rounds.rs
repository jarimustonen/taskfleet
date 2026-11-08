//! Reducer folds of the resident round events (ADR 0003): round reports,
//! resumes, landings, and idle-expiry outcomes are told, non-terminal facts.

use serde_json::{json, Value};
use taskfleet_core::{
    append_and_apply_event, ensure_root, read_manifest, read_node_opt, run_dir, Node, NodeId,
    RunId, RunPaths, Status,
};
use tempfile::TempDir;

const RUN: &str = "01jxsnap000000000000000000";
const OID: &str = "0123456789abcdef0123456789abcdef01234567";

struct Run {
    _tmp: TempDir,
    paths: RunPaths,
}

impl Run {
    fn new(resident: bool) -> Self {
        let tmp = TempDir::new().unwrap();
        ensure_root(tmp.path()).unwrap();
        let dir = run_dir(tmp.path(), &RunId::parse_str(RUN).unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        let run = Self {
            paths: RunPaths::new(dir, RUN).unwrap(),
            _tmp: tmp,
        };
        let mut created = json!({"kind": "spinoff", "lifecycle": "autonomous", "title": "t"});
        if resident {
            created["resident"] = json!({"name": "proto", "idle_expiry_secs": 86400});
        }
        run.append("run.created", None, created).unwrap();
        run.append(
            "node.created",
            Some("n-0001"),
            json!({"kind": "spinoff", "branch": "wt/x", "worktree_path": "/tmp/x",
                "claude_session_id": "018f5f64-b137-7d44-b2b4-4f02c3f646e8"}),
        )
        .unwrap();
        run
    }

    fn append(&self, kind: &str, node: Option<&str>, data: Value) -> taskfleet_core::Result<u64> {
        let node = node.map(|n| NodeId::parse_str(n).unwrap());
        append_and_apply_event(&self.paths, kind, node.as_ref(), None, data).map(|o| o.seq)
    }

    fn node(&self) -> Node {
        read_node_opt(&self.paths, &NodeId::parse_str("n-0001").unwrap())
            .unwrap()
            .unwrap()
    }
}

#[test]
fn resident_policy_and_claude_session_are_recorded() {
    let run = Run::new(true);
    let manifest = read_manifest(&run.paths).unwrap();
    assert_eq!(manifest.resident.unwrap().name, "proto");
    assert_eq!(
        run.node().claude_session_id.as_deref(),
        Some("018f5f64-b137-7d44-b2b4-4f02c3f646e8")
    );
}

#[test]
fn round_reports_open_amend_and_resume_closes_them() {
    let run = Run::new(true);
    run.append(
        "node.round_reported",
        Some("n-0001"),
        json!({"attempt": 0, "summary": "one"}),
    )
    .unwrap();
    let state = run.node().resident.unwrap();
    assert_eq!(state.rounds_reported, 1);
    assert_eq!(state.open_round().unwrap().summary, "one");
    // A second report from the same attempt amends the open round.
    run.append(
        "node.round_reported",
        Some("n-0001"),
        json!({"attempt": 0, "summary": "one, amended"}),
    )
    .unwrap();
    let state = run.node().resident.unwrap();
    assert_eq!(state.rounds_reported, 1);
    assert_eq!(state.open_round().unwrap().summary, "one, amended");
    // A report from a superseded attempt is a no-op.
    run.append(
        "node.round_reported",
        Some("n-0001"),
        json!({"attempt": 7, "summary": "stale"}),
    )
    .unwrap();
    assert_eq!(
        run.node().resident.unwrap().open_round().unwrap().summary,
        "one, amended"
    );

    run.append(
        "worker.exited",
        Some("n-0001"),
        json!({"attempt": 0, "exit_code": 0}),
    )
    .unwrap();
    run.append(
        "node.resumed",
        Some("n-0001"),
        json!({"attempt": 1, "message_id": "m-1", "base_sha": OID, "agent_pid": 42}),
    )
    .unwrap();
    let node = run.node();
    assert_eq!(node.status, Status::Pending);
    assert_eq!(node.retry_attempts, 1);
    assert!(node.worker_exit.is_none());
    assert_eq!(node.branch.as_deref(), Some("wt/x"), "branch kept");
    assert_eq!(
        node.worktree_path.as_deref(),
        Some("/tmp/x"),
        "worktree kept"
    );
    assert_eq!(node.base_sha.as_deref(), Some(OID));
    let state = node.resident.unwrap();
    assert!(state.open_round().is_none());
    assert_eq!(state.last_round.unwrap().closed_by.as_deref(), Some("m-1"));
    // A resume can never move backwards to an earlier attempt.
    run.append(
        "node.resumed",
        Some("n-0001"),
        json!({"attempt": 1, "message_id": "m-2"}),
    )
    .unwrap();
    assert_eq!(
        run.node()
            .resident
            .unwrap()
            .last_round
            .unwrap()
            .closed_by
            .as_deref(),
        Some("m-1")
    );
    run.append(
        "node.round_reported",
        Some("n-0001"),
        json!({"attempt": 1, "summary": "two"}),
    )
    .unwrap();
    assert_eq!(run.node().resident.unwrap().open_round().unwrap().round, 2);
}

#[test]
fn landing_and_expiry_are_non_terminal_and_fire_once() {
    let run = Run::new(true);
    run.append(
        "node.round_reported",
        Some("n-0001"),
        json!({"attempt": 0, "summary": "one"}),
    )
    .unwrap();
    run.append(
        "node.resident_attention",
        Some("n-0001"),
        json!({"reason": "unlanded"}),
    )
    .unwrap();
    let first = run.node().resident.unwrap().expiry_attention_at;
    assert!(first.is_some());
    run.append(
        "node.resident_attention",
        Some("n-0001"),
        json!({"reason": "again"}),
    )
    .unwrap();
    assert_eq!(
        run.node().resident.unwrap().expiry_attention_at,
        first,
        "raised once"
    );
    run.append(
        "node.round_landed",
        Some("n-0001"),
        json!({"op_id": "op", "worker_oid": OID, "integrated_oid": OID, "source_branch": "main"}),
    )
    .unwrap();
    let node = run.node();
    assert_eq!(node.status, Status::Pending);
    let state = node.resident.unwrap();
    assert!(
        state.expiry_attention_at.is_none(),
        "landing answers the attention"
    );
    assert_eq!(state.last_landing.unwrap().round, 1);
    run.append(
        "node.resident_dormant",
        Some("n-0001"),
        json!({"reason": "idle-expired"}),
    )
    .unwrap();
    assert!(run.node().resident.unwrap().dormant_since.is_some());
    assert_eq!(run.node().status, Status::Pending);
}

#[test]
fn resident_events_are_rejected_for_ordinary_runs_and_bad_payloads() {
    let plain = Run::new(false);
    assert!(plain
        .append(
            "node.round_reported",
            Some("n-0001"),
            json!({"attempt": 0, "summary": "x"})
        )
        .is_err());
    let run = Run::new(true);
    assert!(run
        .append(
            "node.round_reported",
            Some("n-0001"),
            json!({"attempt": 0, "summary": "  "})
        )
        .is_err());
    assert!(run
        .append(
            "node.round_landed",
            Some("n-0001"),
            json!({"op_id": "op", "integrated_oid": "nothex", "source_branch": "main"}),
        )
        .is_err());
    let tmp = TempDir::new().unwrap();
    ensure_root(tmp.path()).unwrap();
    let dir = run_dir(tmp.path(), &RunId::parse_str(RUN).unwrap());
    std::fs::create_dir_all(&dir).unwrap();
    let paths = RunPaths::new(dir, RUN).unwrap();
    assert!(append_and_apply_event(
        &paths,
        "run.created",
        None,
        None,
        json!({"kind": "spinoff", "lifecycle": "autonomous", "title": "t",
            "resident": {"name": "7up", "idle_expiry_secs": 1}}),
    )
    .is_err());
}

#[test]
fn an_interrupt_opens_a_sticky_round_and_never_amends_a_reported_one() {
    let run = Run::new(true);
    run.append(
        "node.round_reported",
        Some("n-0001"),
        json!({"attempt": 0, "summary": "interrupted by the parent", "interrupted": true}),
    )
    .unwrap();
    let open = run.node().resident.unwrap().open_round().cloned().unwrap();
    assert!(open.interrupted);
    assert_eq!(open.round, 1);
    // A worker report racing the interrupt amends the summary; the round stays
    // interrupted.
    run.append(
        "node.round_reported",
        Some("n-0001"),
        json!({"attempt": 0, "summary": "half done"}),
    )
    .unwrap();
    let open = run.node().resident.unwrap().open_round().cloned().unwrap();
    assert!(open.interrupted);
    assert_eq!(open.summary, "half done");
    assert_eq!(open.round, 1);

    // An interrupt after the worker's own report changes nothing.
    let run = Run::new(true);
    run.append(
        "node.round_reported",
        Some("n-0001"),
        json!({"attempt": 0, "summary": "finished"}),
    )
    .unwrap();
    run.append(
        "node.round_reported",
        Some("n-0001"),
        json!({"attempt": 0, "summary": "interrupted", "interrupted": true}),
    )
    .unwrap();
    let state = run.node().resident.unwrap();
    let open = state.open_round().unwrap();
    assert!(!open.interrupted);
    assert_eq!(open.summary, "finished");
    assert_eq!(state.rounds_reported, 1);

    // A non-boolean flag is a corrupt event.
    assert!(run
        .append(
            "node.round_reported",
            Some("n-0001"),
            json!({"attempt": 0, "summary": "x", "interrupted": "yes"}),
        )
        .is_err());
}
