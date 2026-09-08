//! State-machine invariants for the reducer (reducer-state-machine-hardening):
//! the success-xor-cancelled requirement on `node.report` and the
//! terminal-state guard on every status reducer.

use serde_json::{json, Value};
use taskfleet_core::{
    append_and_apply_event, ensure_root, read_manifest, read_node_opt, run_dir, Error, Node,
    NodeId, RunId, RunPaths, Status,
};
use tempfile::TempDir;

struct Harness {
    _tmp: TempDir,
    paths: RunPaths,
}

impl Harness {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        ensure_root(root).unwrap();
        let run_id = "01jxsnap000000000000000000".to_string();
        let dir = run_dir(root, &RunId::parse_str(&run_id).unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        Self {
            paths: RunPaths::new(dir, run_id).unwrap(),
            _tmp: tmp,
        }
    }

    /// Append + apply, panicking if the reducer rejects the event.
    fn append(&mut self, kind: &str, node_id: Option<&str>, data: Value) {
        self.try_append(kind, node_id, data).unwrap();
    }

    /// Append + apply through the canonical path, surfacing the reducer's
    /// `Result` to the caller. The append is now transactional: the event is
    /// validated under the lock BEFORE any durable write, so an `Err` here
    /// means the event was rejected and *never* written — `events.jsonl` is
    /// left poison-free (see `events_len`).
    fn try_append(
        &mut self,
        kind: &str,
        node_id: Option<&str>,
        data: Value,
    ) -> taskfleet_core::Result<()> {
        // The fixtures carry node ids as `&str`; parse to the typed envelope id
        // the append API now takes.
        let node_id = node_id.map(|s| NodeId::parse_str(s).unwrap());
        append_and_apply_event(&self.paths, kind, node_id.as_ref(), None, data).map(|_| ())
    }

    /// Number of events durably written to `events.jsonl`.
    fn events_len(&self) -> usize {
        taskfleet_core::read_all_events(&self.paths.events())
            .expect("events.jsonl is poison-free and re-readable")
            .len()
    }

    fn node(&self, node_id: &str) -> Node {
        let nid = NodeId::parse_str(node_id).unwrap();
        read_node_opt(&self.paths, &nid).unwrap().unwrap()
    }

    fn bootstrap_node(&mut self) {
        self.append(
            "run.created",
            None,
            json!({ "kind": "spinoff", "lifecycle": "autonomous", "title": "fixture" }),
        );
        self.append("run.status", None, json!({ "status": "running" }));
        self.append(
            "node.created",
            Some("n-0001"),
            json!({ "kind": "spinoff", "task": "do the thing" }),
        );
        self.append(
            "node.status",
            Some("n-0001"),
            json!({ "status": "running" }),
        );
    }
}

/// Cancelled → late success report → no-op: status stays Cancelled and
/// `last_report` keeps the cancel payload (the late report does not even
/// decorate the projection).
#[test]
fn cancelled_node_ignores_late_success_report() {
    let mut h = Harness::new();
    h.bootstrap_node();

    let cancel_report = json!({
        "success": false,
        "cancelled": true,
        "reason": "cancelled by user",
        "summary": "Run cancelled before agent reported.",
    });
    h.append("node.report", Some("n-0001"), cancel_report.clone());
    assert_eq!(h.node("n-0001").status, Status::Cancelled);

    // Late agent success payload arrives after the cancel landed.
    h.append("node.report", Some("n-0001"), json!({ "success": true }));

    let n = h.node("n-0001");
    assert_eq!(n.status, Status::Cancelled, "cancel must stick");
    assert_eq!(
        n.last_report,
        Some(cancel_report),
        "late report must not overwrite last_report"
    );
}

/// Done → late failure report → no-op: status stays Done, `last_report`
/// unchanged.
#[test]
fn done_node_ignores_late_failure_report() {
    let mut h = Harness::new();
    h.bootstrap_node();

    let success_report = json!({ "success": true, "summary": "all good" });
    h.append("node.report", Some("n-0001"), success_report.clone());
    assert_eq!(h.node("n-0001").status, Status::Done);

    h.append("node.report", Some("n-0001"), json!({ "success": false }));

    let n = h.node("n-0001");
    assert_eq!(n.status, Status::Done, "done must stick");
    assert_eq!(n.last_report, Some(success_report));
}

/// Bare `{}` report expresses neither success nor cancellation → `CorruptEventLog`.
#[test]
fn bare_report_payload_is_corrupt() {
    let mut h = Harness::new();
    h.bootstrap_node();

    let before = h.events_len();
    let err = h
        .try_append("node.report", Some("n-0001"), json!({}))
        .unwrap_err();
    assert!(matches!(err, Error::CorruptEventLog { .. }), "got {err:?}");

    // The node is untouched: still running, no report decorated.
    let n = h.node("n-0001");
    assert_eq!(n.status, Status::Running);
    assert_eq!(n.last_report, None);
    // And the rejected event was never appended (no poison line).
    assert_eq!(h.events_len(), before, "rejected event must not be written");
}

/// `success: true` + `cancelled: true` is a contradiction → `CorruptEventLog`.
#[test]
fn success_and_cancelled_both_true_is_corrupt() {
    let mut h = Harness::new();
    h.bootstrap_node();

    let before = h.events_len();
    let err = h
        .try_append(
            "node.report",
            Some("n-0001"),
            json!({ "success": true, "cancelled": true, "reason": "x" }),
        )
        .unwrap_err();
    assert!(matches!(err, Error::CorruptEventLog { .. }), "got {err:?}");

    let n = h.node("n-0001");
    assert_eq!(n.status, Status::Running);
    assert_eq!(n.last_report, None);
    assert_eq!(h.events_len(), before, "rejected event must not be written");
}

/// `run cancel <run>` settles the run, then a late `node.report` success
/// arrives — the cancel must stick at both the run and node level.
#[test]
fn run_cancel_then_late_report_keeps_cancel() {
    let mut h = Harness::new();
    h.bootstrap_node();

    // `run cancel` flips the run to cancelled and synthesizes a cancel
    // report on each live node (design.md §7.7).
    h.append("run.status", None, json!({ "status": "cancelled" }));
    h.append(
        "node.report",
        Some("n-0001"),
        json!({ "success": false, "cancelled": true, "reason": "cancelled by user" }),
    );
    assert_eq!(read_manifest(&h.paths).unwrap().status, Status::Cancelled);
    assert_eq!(h.node("n-0001").status, Status::Cancelled);

    // Agent was mid-write when cancel landed; its success report arrives late.
    h.append("node.report", Some("n-0001"), json!({ "success": true }));
    // A late `run.status running` (e.g. a stale supervisor) must also no-op.
    h.append("run.status", None, json!({ "status": "running" }));

    assert_eq!(
        read_manifest(&h.paths).unwrap().status,
        Status::Cancelled,
        "run cancel must stick"
    );
    assert_eq!(
        h.node("n-0001").status,
        Status::Cancelled,
        "node cancel must stick"
    );
}

/// ADOPTION (issue `reducer-adopt-explicit-merge`): a watchdog `agent-died`
/// terminalizes the node FAILED, then the still-alive agent's `run merge` appends
/// a confirmed `via: "explicit-merge"` report. The reducer adopts it even though
/// the node is terminal — overwriting `last_report` and reconciling status to
/// `Done` — so the supervisor's `any_node_merged_explicitly` gate sees the merge
/// and can warrant teardown (invariant #5), instead of the CLI compensating inline.
#[test]
fn failed_node_adopts_late_explicit_merge_report() {
    let mut h = Harness::new();
    h.bootstrap_node();

    // Watchdog false positive: agent-died terminalizes the node FAILED.
    let agent_died = json!({
        "success": false, "failed": true, "reason": "agent-died",
        "summary": "Agent stopped responding.",
    });
    h.append("node.report", Some("n-0001"), agent_died);
    assert_eq!(h.node("n-0001").status, Status::Failed);

    // The still-alive agent's explicit merge lands late.
    let merge = json!({
        "success": true, "summary": "merged wt/foo into main", "via": "explicit-merge",
    });
    h.append("node.report", Some("n-0001"), merge.clone());

    let n = h.node("n-0001");
    assert_eq!(
        n.status,
        Status::Done,
        "a confirmed late explicit merge reconciles the watchdog-FAILED node to Done"
    );
    assert_eq!(
        n.last_report,
        Some(merge),
        "the merge report must be adopted onto the projection so teardown is warranted"
    );
}

/// The adoption is scoped: a NON-explicit-merge late report against the same
/// watchdog-FAILED node stays a dead event — `last_report` keeps the `agent-died`
/// payload and the node stays FAILED. This is the unmerged-work-preservation half:
/// only a confirmed explicit merge overrides a terminal, never a bare success.
#[test]
fn failed_node_ignores_late_plain_success_report() {
    let mut h = Harness::new();
    h.bootstrap_node();

    let agent_died = json!({ "success": false, "failed": true, "reason": "agent-died" });
    h.append("node.report", Some("n-0001"), agent_died.clone());
    assert_eq!(h.node("n-0001").status, Status::Failed);

    // A plain success (no `via: "explicit-merge"`) must NOT resurrect the node.
    h.append("node.report", Some("n-0001"), json!({ "success": true }));

    let n = h.node("n-0001");
    assert_eq!(
        n.status,
        Status::Failed,
        "a non-merge late report stays dead"
    );
    assert_eq!(
        n.last_report,
        Some(agent_died),
        "a non-merge late report must not overwrite last_report"
    );
}

/// A DELIBERATE `run cancel` (Cancelled) is not a watchdog false positive, so a
/// later explicit merge does NOT override it — the cancel sticks. Only a `Failed`
/// terminal is adopted. Guards the tightened prior-status scope.
#[test]
fn cancelled_node_ignores_late_explicit_merge_report() {
    let mut h = Harness::new();
    h.bootstrap_node();

    let cancel = json!({ "success": false, "cancelled": true, "reason": "cancelled by user" });
    h.append("node.report", Some("n-0001"), cancel.clone());
    assert_eq!(h.node("n-0001").status, Status::Cancelled);

    h.append(
        "node.report",
        Some("n-0001"),
        json!({ "success": true, "via": "explicit-merge", "summary": "merged" }),
    );

    let n = h.node("n-0001");
    assert_eq!(
        n.status,
        Status::Cancelled,
        "a deliberate cancel is not overridden by a later merge"
    );
    assert_eq!(
        n.last_report,
        Some(cancel),
        "the cancel report is preserved"
    );
}

/// Re-folding the SAME adopted explicit-merge report is a clean idempotent no-op:
/// status stays Done, `last_report` unchanged, and `updated_at` does not churn
/// (the reducer plans zero ops when the exact report is already projected). This
/// keeps replay stable — a re-fold of an already-applied adoption changes nothing.
#[test]
fn adopted_explicit_merge_replay_is_idempotent() {
    let mut h = Harness::new();
    h.bootstrap_node();

    h.append(
        "node.report",
        Some("n-0001"),
        json!({ "success": false, "failed": true, "reason": "agent-died" }),
    );
    let merge = json!({ "success": true, "via": "explicit-merge", "summary": "merged" });
    h.append("node.report", Some("n-0001"), merge.clone());
    let after_adopt = h.node("n-0001");
    assert_eq!(after_adopt.status, Status::Done);

    // Re-fold the identical report.
    h.append("node.report", Some("n-0001"), merge.clone());
    let after_replay = h.node("n-0001");
    assert_eq!(after_replay.status, Status::Done);
    assert_eq!(after_replay.last_report, Some(merge));
    assert_eq!(
        after_replay.updated_at, after_adopt.updated_at,
        "re-folding the same adopted report must not churn updated_at"
    );
}

/// A genuinely `Done` node that then gets an explicit-merge report (e.g. it
/// succeeded but was not merged, then the user runs `run merge`) adopts the merge
/// marker onto `last_report` so teardown becomes warranted; status stays `Done`.
/// Documents the latest-wins overwrite for a Done node.
#[test]
fn done_node_adopts_explicit_merge_marker() {
    let mut h = Harness::new();
    h.bootstrap_node();

    let first = json!({ "success": true, "summary": "work done, not yet merged" });
    h.append("node.report", Some("n-0001"), first);
    assert_eq!(h.node("n-0001").status, Status::Done);

    let merge = json!({ "success": true, "via": "explicit-merge", "summary": "now merged" });
    h.append("node.report", Some("n-0001"), merge.clone());

    let n = h.node("n-0001");
    assert_eq!(n.status, Status::Done);
    assert_eq!(
        n.last_report,
        Some(merge),
        "the merge marker must be adopted so any_node_merged_explicitly warrants teardown"
    );
}

/// Strict boolean typing on the adoption gate: a malformed explicit-merge payload
/// (non-boolean `cancelled`) that a LIVE node would reject as `CorruptEventLog` must
/// NOT sneak an adoption in through the terminal-only exception — it stays a dead
/// no-op (and, being terminal, does not error). Mirrors the live-node strict
/// contract (`non_boolean_cancelled_is_corrupt`).
#[test]
fn malformed_explicit_merge_against_terminal_node_is_not_adopted() {
    let mut h = Harness::new();
    h.bootstrap_node();

    let agent_died = json!({ "success": false, "failed": true, "reason": "agent-died" });
    h.append("node.report", Some("n-0001"), agent_died.clone());
    assert_eq!(h.node("n-0001").status, Status::Failed);

    // `cancelled: "true"` (string, not bool) — malformed. Must not adopt.
    h.try_append(
        "node.report",
        Some("n-0001"),
        json!({ "success": true, "via": "explicit-merge", "cancelled": "true" }),
    )
    .expect("a malformed dead report against a terminal node is a clean no-op, not an error");

    let n = h.node("n-0001");
    assert_eq!(n.status, Status::Failed, "malformed merge must not adopt");
    assert_eq!(
        n.last_report,
        Some(agent_died),
        "malformed merge must not overwrite last_report"
    );
}

/// `node.status` terminal guard: a settled node ignores a late status event.
#[test]
fn node_status_terminal_guard() {
    let mut h = Harness::new();
    h.bootstrap_node();

    h.append("node.report", Some("n-0001"), json!({ "success": true }));
    assert_eq!(h.node("n-0001").status, Status::Done);

    h.append(
        "node.status",
        Some("n-0001"),
        json!({ "status": "running" }),
    );
    assert_eq!(
        h.node("n-0001").status,
        Status::Done,
        "terminal node must not transition back to running"
    );
}

/// `run.status` terminal guard, standalone: a `Done` run ignores a late
/// status event (the run-cancel test only exercises the `Cancelled` state).
#[test]
fn run_status_terminal_guard() {
    let mut h = Harness::new();
    h.append(
        "run.created",
        None,
        json!({ "kind": "spinoff", "lifecycle": "autonomous", "title": "fixture" }),
    );
    h.append("run.status", None, json!({ "status": "running" }));
    h.append("run.status", None, json!({ "status": "done" }));
    assert_eq!(read_manifest(&h.paths).unwrap().status, Status::Done);

    h.append("run.status", None, json!({ "status": "running" }));
    assert_eq!(
        read_manifest(&h.paths).unwrap().status,
        Status::Done,
        "terminal run must not transition back to running"
    );
}

/// A conflicting terminal transition (Done → Cancelled via `node.status`) is
/// dropped by the guard, not applied — the first terminal status wins.
#[test]
fn conflicting_terminal_transition_is_noop() {
    let mut h = Harness::new();
    h.bootstrap_node();

    h.append("node.report", Some("n-0001"), json!({ "success": true }));
    assert_eq!(h.node("n-0001").status, Status::Done);

    h.append(
        "node.status",
        Some("n-0001"),
        json!({ "status": "cancelled" }),
    );
    assert_eq!(
        h.node("n-0001").status,
        Status::Done,
        "a conflicting terminal status must not overwrite the settled one"
    );
}

/// Strict boolean typing: a non-boolean `success` on a live node is corrupt
/// (rather than silently coerced to "missing").
#[test]
fn non_boolean_success_is_corrupt() {
    let mut h = Harness::new();
    h.bootstrap_node();

    let before = h.events_len();
    let err = h
        .try_append("node.report", Some("n-0001"), json!({ "success": "true" }))
        .unwrap_err();
    assert!(matches!(err, Error::CorruptEventLog { .. }), "got {err:?}");
    assert_eq!(h.node("n-0001").status, Status::Running);
    assert_eq!(h.events_len(), before, "rejected event must not be written");
}

/// Strict boolean typing: a non-boolean `cancelled` is corrupt even when a
/// valid `success` is present — it must not be coerced to `false`.
#[test]
fn non_boolean_cancelled_is_corrupt() {
    let mut h = Harness::new();
    h.bootstrap_node();

    let before = h.events_len();
    let err = h
        .try_append(
            "node.report",
            Some("n-0001"),
            json!({ "success": false, "cancelled": "true" }),
        )
        .unwrap_err();
    assert!(matches!(err, Error::CorruptEventLog { .. }), "got {err:?}");
    assert_eq!(h.node("n-0001").status, Status::Running);
    assert_eq!(h.events_len(), before, "rejected event must not be written");
}

/// Guard-before-validate: a malformed (bare `{}`) report against an already
/// terminal node is a clean no-op, not a `CorruptEventLog`. A dead event must
/// not be able to brick replay of a settled node's log.
#[test]
fn corrupt_report_against_terminal_node_is_noop() {
    let mut h = Harness::new();
    h.bootstrap_node();

    let report = json!({ "success": true, "summary": "done" });
    h.append("node.report", Some("n-0001"), report.clone());
    assert_eq!(h.node("n-0001").status, Status::Done);

    // Bare `{}` would be CorruptEventLog against a live node, but the node is
    // terminal, so the guard short-circuits before validation.
    h.try_append("node.report", Some("n-0001"), json!({}))
        .expect("malformed report against terminal node must be a no-op");

    let n = h.node("n-0001");
    assert_eq!(n.status, Status::Done);
    assert_eq!(n.last_report, Some(report), "last_report must be untouched");
}

/// Failed -> Done is the one run-level recovery: it needs a merge report that
/// the shared node fold actually adopted before the recovery event.
#[test]
fn failed_run_recovers_only_from_adopted_confirmed_merge() {
    let mut h = Harness::new();
    h.bootstrap_node();
    h.append("node.report", Some("n-0001"), json!({ "success": false }));
    h.append("run.status", None, json!({ "status": "failed" }));
    h.append(
        "node.report",
        Some("n-0001"),
        json!({
            "success": true,
            "origin": { "kind": "run-merge", "op_id": "op-1", "worker_oid": "abc" },
            "via": 17
        }),
    );
    let facts = taskfleet_core::read_node_status_facts(&h.paths, None).unwrap();
    let merge_seq = facts[0].confirmed_merge_seq.expect("merge adopted");
    assert_eq!(facts[0].status, Status::Done, "fold and projection agree");
    assert_eq!(h.node("n-0001").status, Status::Done);

    h.append(
        "run.status",
        None,
        json!({
            "status": "done",
            "recovery_merge_report_seq": merge_seq,
            "children_successful": true
        }),
    );
    assert_eq!(read_manifest(&h.paths).unwrap().status, Status::Done);
}

/// The real race is legal: a supervisor may compute Failed, lose the append
/// race to merge adoption, and append that stale Failed before the next tick
/// emits the coherent recovery.
#[test]
fn stale_failed_rollup_after_merge_does_not_block_recovery() {
    let mut h = Harness::new();
    h.bootstrap_node();
    h.append("node.report", Some("n-0001"), json!({ "success": false }));
    h.append(
        "node.report",
        Some("n-0001"),
        json!({ "success": true, "origin": { "kind": "run-merge" } }),
    );
    // The first run-status append carries a stale decision computed while the
    // node was Failed, but arrives only after merge adoption made it Done.
    h.append("run.status", None, json!({ "status": "failed" }));
    let merge_seq = taskfleet_core::read_node_status_facts(&h.paths, None).unwrap()[0]
        .confirmed_merge_seq
        .unwrap();
    h.append(
        "run.status",
        None,
        json!({
            "status": "done",
            "recovery_merge_report_seq": merge_seq,
            "children_successful": true
        }),
    );
    assert_eq!(read_manifest(&h.paths).unwrap().status, Status::Done);
}

/// False/malformed/forged authority, incomplete topology, and cancellation do
/// not gain a generic terminal reopen path.
#[test]
fn recovery_refuses_forged_failed_and_cancelled_topologies() {
    let mut h = Harness::new();
    h.bootstrap_node();
    h.append("node.report", Some("n-0001"), json!({ "success": false }));
    h.append("run.status", None, json!({ "status": "failed" }));
    h.append(
        "node.report",
        Some("n-0001"),
        json!({
            "success": true,
            "via": "explicit-merge",
            "origin": { "kind": "agent" }
        }),
    );
    h.append(
        "run.status",
        None,
        json!({
            "status": "done",
            "recovery_merge_report_seq": 999,
            "children_successful": true
        }),
    );
    assert_eq!(read_manifest(&h.paths).unwrap().status, Status::Failed);
    assert_eq!(h.node("n-0001").status, Status::Failed);

    let mut cancelled = Harness::new();
    cancelled.bootstrap_node();
    cancelled.append(
        "node.report",
        Some("n-0001"),
        json!({ "success": false, "cancelled": true, "reason": "operator" }),
    );
    cancelled.append("run.status", None, json!({ "status": "cancelled" }));
    cancelled.append(
        "node.report",
        Some("n-0001"),
        json!({ "success": true, "origin": { "kind": "run-merge" } }),
    );
    cancelled.append(
        "run.status",
        None,
        json!({
            "status": "done",
            "recovery_merge_report_seq": cancelled.events_len() as u64 - 1,
            "children_successful": true
        }),
    );
    assert_eq!(
        read_manifest(&cancelled.paths).unwrap().status,
        Status::Cancelled
    );
    assert_eq!(cancelled.node("n-0001").status, Status::Cancelled);
}

/// One recovered node cannot erase an independent failed or live sibling.
#[test]
fn recovery_requires_every_own_node_done() {
    for sibling_report in [Some(json!({ "success": false })), None] {
        let mut h = Harness::new();
        h.bootstrap_node();
        h.append(
            "node.created",
            Some("n-0002"),
            json!({ "kind": "spinoff", "task": "sibling" }),
        );
        h.append("node.report", Some("n-0001"), json!({ "success": false }));
        if let Some(report) = sibling_report.clone() {
            h.append("node.report", Some("n-0002"), report);
        }
        h.append("run.status", None, json!({ "status": "failed" }));
        h.append(
            "node.report",
            Some("n-0001"),
            json!({ "success": true, "origin": { "kind": "run-merge" } }),
        );
        let merge_seq = taskfleet_core::read_node_status_facts(&h.paths, None)
            .unwrap()
            .into_iter()
            .find_map(|fact| fact.confirmed_merge_seq)
            .unwrap();
        h.append(
            "run.status",
            None,
            json!({
                "status": "done",
                "recovery_merge_report_seq": merge_seq,
                "children_successful": true
            }),
        );
        assert_eq!(read_manifest(&h.paths).unwrap().status, Status::Failed);
    }
}

/// Replaying an earlier recovery event cannot look ahead to a later merge, and
/// malformed report fields after the bound are not interpreted by that fold.
#[test]
fn recovery_replay_is_bounded_before_current_event() {
    let mut h = Harness::new();
    h.bootstrap_node();
    h.append("node.report", Some("n-0001"), json!({ "success": false }));
    h.append("run.status", None, json!({ "status": "failed" }));
    let before_recovery = h.events_len() as u64;
    h.append(
        "run.status",
        None,
        json!({
            "status": "done",
            "recovery_merge_report_seq": before_recovery + 2,
            "children_successful": true
        }),
    );
    h.append(
        "node.report",
        Some("n-0001"),
        json!({
            "success": true,
            "origin": { "kind": "run-merge" },
            "via": 17
        }),
    );
    h.append(
        "node.report",
        Some("n-0001"),
        json!({ "success": "true", "origin": { "kind": "run-merge" } }),
    );

    // The bounded skim never decodes future report fields into typed booleans.
    let facts = taskfleet_core::read_node_status_facts(&h.paths, Some(before_recovery + 1))
        .expect("future malformed fields are outside the fold");
    assert_eq!(facts[0].status, Status::Failed);
    assert_eq!(facts[0].confirmed_merge_seq, None);

    // Rewind only the manifest watermark/status and trigger normal catch-up.
    // The node projection intentionally remains Done from the future merge: the
    // run-status reducer must still decide from the bounded log fold, not it.
    let mut manifest = read_manifest(&h.paths).unwrap();
    manifest.applied_seq = before_recovery;
    manifest.status = Status::Failed;
    std::fs::write(
        h.paths.manifest(),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    h.append(
        "orchestrator.decision",
        None,
        json!({ "summary": "replay" }),
    );
    assert_eq!(
        read_manifest(&h.paths).unwrap().status,
        Status::Failed,
        "future merge authority must not authorize the earlier recovery event"
    );
}
