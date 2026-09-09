//! The typed supervisor outcome table (design.md §2.6 / A6, issue
//! `typed-supervisor-outcomes`).
//!
//! `run merge` is the only **success** truth, but not the only **terminal**
//! truth. Before this module, the supervisor's terminal verdict was spread
//! across a cascade of `if` branches in [`super::watchdog_tick`] and a second,
//! parallel field-sniffing gate in [`super::cleanup`] — each re-deriving the
//! outcome from a cross-product of proxies (pid × pane × branch × report ×
//! activity clocks). That cross-product is exactly the inference the thin model
//! deletes.
//!
//! This module replaces it with two small, pure, exhaustively-tested tables:
//!
//! - [`TerminalOutcome`] classifies a node's **terminal `node.report`** into one
//!   typed outcome, and [`TerminalOutcome::teardown`] maps it to the single
//!   [`Teardown`] policy it authorizes. This is the invariant-critical table:
//!   it is what prevents a future change from silently re-introducing heuristic
//!   teardown of unmerged work (invariant 5). `cleanup` reads it instead of
//!   re-sniffing `last_report` JSON.
//! - [`LiveVerdict`] classifies a **non-terminal** node from its *told facts*
//!   (the A1 `worker.exited` status) plus a residual [`DeathObservation`] — the
//!   pid crash backstop, the ONLY place pid liveness still governs an outcome.
//!   The watchdog reads it to decide what, if anything, to synthesize this tick.
//!
//! Both tables are pure functions of durable inputs, so the reducer's
//! append/replay/fsync correctness and the lock invariants are untouched — the
//! caller still threads the [`taskfleet_core::RunLock`] witness through every append.

use serde_json::Value;
use taskfleet_core::{Node, ReportOrigin, WorkerExit};

/// The teardown a [`TerminalOutcome`] authorizes — the "Teardown?" column of the
/// design §2.6 table, made explicit and total.
///
/// This enum is the whole point of the typed table: teardown is a function of a
/// *typed outcome*, never of a signal combination. Adding a new terminal outcome
/// forces a deliberate teardown choice here (the [`TerminalOutcome::teardown`]
/// match is exhaustive), so a future outcome can never default into destroying a
/// worker's branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Teardown {
    /// A confirmed explicit `run merge`: the work is in the run's source branch,
    /// so the worktree is disposable and the branch may be **force-deleted**
    /// (`git branch -D`). The only outcome that earns the force teardown.
    Full,
    /// Preserve the worker's branch **and** worktree — only the tmux window winds
    /// down. Every non-merge negative terminal: a blocked human handoff, a told
    /// worker-exit failure, the confirmed-death crash backstop. The committed
    /// work is left exactly where the human/salvage skill can pick it up
    /// (invariant 5, issue `blocked-report-deletes-branch`).
    PreserveWork,
    /// Source-relative teardown: wind the window down, and remove the worktree +
    /// delete the branch **only if** the branch carries no commits unreachable
    /// from the run's source branch (`git branch -d` refuses an unmerged branch
    /// as the last-resort backstop). Used for `run cancel` — which explicitly
    /// preserves work-bearing branches, never a teardown authorization
    /// (design.md §2.6) — and for the (unusual) plain success that skipped
    /// `run merge`.
    SourceRelative,
}

/// A node's typed **terminal** outcome, classified purely from its terminal
/// `node.report` (design.md §2.6). `None` from [`TerminalOutcome::classify`]
/// means the node has no terminal report yet — it is still live, and the
/// [`LiveVerdict`] table governs instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalOutcome {
    /// `run merge` succeeded: `success: true` authorized by the run-merge path —
    /// a typed `ReportOrigin::RunMerge` origin, or (for a legacy report with no
    /// origin field) `via: "explicit-merge"` (issue `retire-via-string`). The one
    /// success truth. → [`Teardown::Full`].
    Merged,
    /// A blocked handoff: the agent hit a wall and handed committed-but-unmerged
    /// work to a human via a plain `node report` (`success: false`, not
    /// cancelled, not an explicit merge). → [`Teardown::PreserveWork`].
    Blocked,
    /// A failure the supervisor recorded: a told `worker.exited` non-zero/signal
    /// (A1) or the confirmed-death crash backstop (`success: false`, a known
    /// supervisor/worker failure reason). → [`Teardown::PreserveWork`].
    Failed,
    /// `run cancel` (run or node): `cancelled: true`. Cancel explicitly PRESERVES
    /// work — it is never a teardown authorization (design.md §2.6). →
    /// [`Teardown::SourceRelative`].
    Cancelled,
    /// A `success: true` terminal report that did NOT come through `run merge`
    /// (no `via: "explicit-merge"`). Unusual under the thin model — success is
    /// only ever told via `run merge` — but a hand-authored/legacy success
    /// report is classified here rather than mistaken for a merge, so it never
    /// earns the force teardown. → [`Teardown::SourceRelative`].
    PlainSuccess,
}

impl TerminalOutcome {
    /// Classify a node's terminal `node.report` into a typed outcome, or `None`
    /// if the node has no terminal report yet.
    ///
    /// The order is the table's precedence: an explicit merge wins over every
    /// other shape; then cancel; then a negative report splits into blocked vs
    /// failed by reason; then a residual plain success. A report is only ever one
    /// of these — the reducer enforces the success-XOR-cancelled invariant on
    /// append — so the branches are mutually exclusive in practice.
    #[must_use]
    pub fn classify(n: &Node) -> Option<Self> {
        let report = n.last_report.as_ref()?;
        let success = report.get("success").and_then(Value::as_bool);
        let cancelled = report.get("cancelled").and_then(Value::as_bool) == Some(true);
        // The typed provenance (issue `typed-report-origin`), when the report
        // carries it. The legacy string-sniffing fallback (`via` / `reason`) is
        // gated on the origin field being genuinely ABSENT — a legacy report — NOT
        // on `from_report` returning `None`. That distinction is load-bearing for
        // security: a report that CARRIES an `origin` field but whose value fails to
        // parse (corrupt / hand-edited / a downgrade attempt) must NOT re-unlock the
        // forgeable legacy `via` merge path (llm-review consensus finding). A
        // present-but-malformed origin is treated as "typed, but unknown authority":
        // never a merge, never a supervisor failure.
        let origin_present = report.get(taskfleet_core::REPORT_ORIGIN_KEY).is_some();
        let origin = ReportOrigin::from_report(report);

        // Cancel wins over EVERYTHING, checked first. The reducer rejects the
        // contradiction `success: true` + `cancelled: true` on append, but
        // `classify` is the teardown authority and must be defensive against a
        // corrupt/legacy/hand-edited projection: a `cancelled: true` report must
        // NEVER earn force deletion, even if a spoofed `success:true`/`via` rides
        // along (design.md §2.6 — cancel is never a teardown authorization).
        if cancelled {
            return Some(TerminalOutcome::Cancelled);
        }
        // Explicit merge: the one confirmed-merge truth shared with the reducer, the
        // `landed` fallback, and `run wait`'s `merged` flag (issue
        // `retire-via-string`). The typed `RunMerge` origin is the authoritative
        // marker (stamped only by `run merge` / its recovery — an agent's
        // `node report` is normalized to an `Agent` origin, so it cannot assert
        // this); the legacy `via` marker is the fallback ONLY for a report carrying
        // no origin field. A merge marker with success:false (malformed/spoofed) or
        // a forged `via` on an origin-bearing report is NOT a merge and falls through
        // to the negative arms below.
        if ReportOrigin::report_is_confirmed_merge(report) {
            return Some(TerminalOutcome::Merged);
        }
        match success {
            // A negative report that is neither cancel nor merge: a blocked
            // handoff or a supervisor/worker-recorded failure. They share a
            // teardown policy (PreserveWork); the split is observability only.
            Some(false) => Some(
                if is_supervisor_failure(report, origin_present, origin.as_ref()) {
                    TerminalOutcome::Failed
                } else {
                    TerminalOutcome::Blocked
                },
            ),
            // A success report that did not come through `run merge`.
            Some(true) => Some(TerminalOutcome::PlainSuccess),
            // No boolean `success` on a present report: the reducer would have
            // rejected such an event as corrupt before it ever set `last_report`,
            // so this is unreachable in practice. Treat it as the safest bucket —
            // preserve the work rather than guess a teardown.
            None => Some(TerminalOutcome::Blocked),
        }
    }

    /// The teardown policy this outcome authorizes. Exhaustive so a new outcome
    /// forces a deliberate teardown decision here.
    #[must_use]
    pub fn teardown(self) -> Teardown {
        match self {
            TerminalOutcome::Merged => Teardown::Full,
            TerminalOutcome::Blocked | TerminalOutcome::Failed => Teardown::PreserveWork,
            TerminalOutcome::Cancelled | TerminalOutcome::PlainSuccess => Teardown::SourceRelative,
        }
    }

    /// True when this outcome is a confirmed, successful explicit `run merge` —
    /// the only outcome whose branch may be force-deleted. The single caller in
    /// `cleanup` uses this to pick `git branch -D` over the safe `-d`.
    #[must_use]
    pub fn is_explicit_merge(self) -> bool {
        matches!(self, TerminalOutcome::Merged)
    }
}

/// The residual pid-liveness observation, the ONLY place pid liveness still
/// governs an outcome (design.md §2.1a — "pid liveness as a pure crash backstop
/// only, never a primary signal").
///
/// The watchdog computes this from the liveness probe plus the node's durable
/// first-death observation ([`Node::first_death_at`]) and a fixed post-death
/// grace: the grace exists only to let an in-flight `worker.exited` / merge
/// append land before the backstop fires (design.md §2.1a), and it is anchored
/// to a persisted, monotonic first-death timestamp so it survives a supervisor
/// restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeathObservation {
    /// The worker process is (still) alive per the liveness probe.
    Alive,
    /// Confirmed dead, but the post-death grace has not yet elapsed since the
    /// first-death observation — defer, so a racing exit/merge append can win.
    DeadWithinGrace,
    /// Confirmed dead and the post-death grace has elapsed with no exit event and
    /// no merge — the shim was lost (hard kill / host death); fire the backstop.
    DeadGraceElapsed,
}

/// What the watchdog should do about a **non-terminal** node this tick, from its
/// told facts plus the residual death observation (design.md §2.1 / §2.6).
///
/// The caller must have already established that the node is non-terminal and has
/// no `explicit-merge` report — a merged node is terminal and classified by
/// [`TerminalOutcome`], not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveVerdict {
    /// The launcher shim recorded a FAILING `worker.exited` (non-zero / signal).
    /// Synthesize a `failed` `node.report` from the told fact — never a pid guess
    /// (branch preserved via [`Teardown::PreserveWork`]).
    WorkerFailed(WorkerExit),
    /// The shim recorded a CLEAN exit (`0`) but there is no merge — the worker
    /// finished but skipped `run merge`. Stay **non-terminal** (attention-
    /// required); hand to the manual finish skill. NEVER auto-failed
    /// (design.md §2.1) — this is the case the deleted idle-unmerged net used to
    /// wrongly terminalize.
    AttentionRequired,
    /// No told exit, the process is confirmed dead, and the post-death grace has
    /// elapsed — the residual crash backstop. Synthesize `failed`, preserve the
    /// branch/worktree.
    CrashBackstopFailed,
    /// No told exit, confirmed dead, but still inside the post-death grace —
    /// defer this tick so an in-flight exit/merge append can win.
    DeferGrace,
    /// Nothing to do this tick: alive with no told exit, or awaiting its own
    /// terminal signal.
    Alive,
}

/// Classify a non-terminal node into a [`LiveVerdict`] (design.md §2.1 / §2.6).
///
/// Told facts beat guesses: a recorded `worker.exited` — clean or failing — is
/// authoritative and short-circuits the pid backstop entirely. Only when the
/// shim recorded nothing does the residual [`DeathObservation`] govern.
#[must_use]
pub fn classify_live_node(worker_exit: Option<WorkerExit>, death: DeathObservation) -> LiveVerdict {
    if let Some(exit) = worker_exit {
        return if exit.is_failure() {
            LiveVerdict::WorkerFailed(exit)
        } else {
            LiveVerdict::AttentionRequired
        };
    }
    match death {
        DeathObservation::Alive => LiveVerdict::Alive,
        DeathObservation::DeadWithinGrace => LiveVerdict::DeferGrace,
        DeathObservation::DeadGraceElapsed => LiveVerdict::CrashBackstopFailed,
    }
}

/// Split a negative (`success: false`, non-cancel) terminal report into a
/// supervisor-synthesized [`TerminalOutcome::Failed`] versus an agent-authored
/// blocked [`TerminalOutcome::Blocked`]. Used only for observability; both share a
/// teardown policy ([`Teardown::PreserveWork`]), so a mis-split can never change
/// teardown behavior.
///
/// Prefers the typed [`ReportOrigin`] (issue `typed-report-origin`) when present:
/// a `Supervisor` origin is a supervisor failure, an `Agent` origin is a blocked
/// handoff, and — defensively — a `RunMerge` origin on a negative report (which
/// `run merge` refuses to append, but classify stays defensive) is treated as a
/// non-supervisor blocked. The `reason`-string sniff runs ONLY when the origin
/// field is genuinely ABSENT (`origin_present == false`, a legacy report) — a
/// present-but-malformed origin is NOT re-downgraded to string sniffing (parallel
/// to the merge gate; llm-review consensus finding), it is treated as an unknown
/// author and classified `Blocked`. Either way the teardown policy is identical
/// ([`Teardown::PreserveWork`]), so this only governs the observability split.
fn is_supervisor_failure(
    report: &Value,
    origin_present: bool,
    origin: Option<&ReportOrigin>,
) -> bool {
    match origin {
        Some(ReportOrigin::Supervisor) => return true,
        Some(ReportOrigin::Agent | ReportOrigin::RunMerge { .. }) => return false,
        None => {}
    }
    if origin_present {
        // A typed origin field is present but did not parse: treat as an unknown
        // author, not a legacy report — do not fall back to string sniffing.
        return false;
    }
    let Some(reason) = report.get("reason").and_then(Value::as_str) else {
        // A negative report with no reason is more likely an agent handoff than a
        // supervisor failure (the supervisor always stamps a reason); classify as
        // blocked. Teardown is identical either way.
        return false;
    };
    reason == super::WORKER_EXITED_NONZERO_REASON
        || reason == super::WORKER_KILLED_BY_SIGNAL_REASON
        || reason == super::NO_WORKER_REASON
        // The confirmed-death crash backstop and the liveness probe stamp reasons
        // that all describe a dead/gone agent (`agent-died`, `agent-*`). Matched
        // as a prefix so every liveness-reason variant counts without enumerating
        // them here.
        || reason.starts_with(AGENT_DIED_REASON_PREFIX)
}

/// Reason prefix the confirmed-death crash backstop / liveness probe stamps.
const AGENT_DIED_REASON_PREFIX: &str = "agent-";

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use taskfleet_core::{Kind, NodeId, RunId, Status};

    /// Build a minimal `Node` carrying `report` as its terminal `last_report`.
    fn node_with_report(report: Option<Value>) -> Node {
        Node {
            schema_version: 1,
            node_id: NodeId::parse_str("n-0001").unwrap(),
            run_id: RunId::parse_str("01jxsnap000000000000000000").unwrap(),
            parent_node_id: None,
            kind: Kind::Spinoff,
            status: Status::Failed,
            task: None,
            worktree_path: None,
            branch: None,
            base_sha: None,
            tmux_window: None,
            tmux_identity: None,
            evidence: None,
            retained_display: None,
            retention_unavailable: None,
            agent_pid: None,
            agent_pid_start_time: None,
            supervisor_pid: None,
            children: vec![],
            started_at: None,
            updated_at: Utc::now(),
            last_report: report,
            last_processed_report_seq_by_child: serde_json::Map::new(),
            retry_attempts: 0,
            worker_exit: None,
            pending_merge: None,
            first_death_at: None,
            awaiting_input: None,
        }
    }

    /// The teardown table (design §2.6 "Teardown?" column), asserted row by row
    /// so a future change cannot silently re-introduce heuristic teardown of
    /// unmerged work.
    #[test]
    fn terminal_outcome_teardown_table() {
        let rows: &[(&str, Value, TerminalOutcome, Teardown)] = &[
            (
                "explicit merge succeeds -> done + full teardown",
                json!({ "success": true, "via": "explicit-merge" }),
                TerminalOutcome::Merged,
                Teardown::Full,
            ),
            (
                "blocked handoff -> preserve branch + worktree",
                json!({ "success": false, "reason": "blocked-on-human" }),
                TerminalOutcome::Blocked,
                Teardown::PreserveWork,
            ),
            (
                "told worker-exit failure -> failed, preserve",
                json!({ "success": false, "reason": "worker-exited-nonzero" }),
                TerminalOutcome::Failed,
                Teardown::PreserveWork,
            ),
            (
                "confirmed-death backstop -> failed, preserve",
                json!({ "success": false, "reason": "agent-died" }),
                TerminalOutcome::Failed,
                Teardown::PreserveWork,
            ),
            (
                "run cancel -> cancelled, source-relative preserve",
                json!({ "success": false, "cancelled": true, "reason": "cancelled by user" }),
                TerminalOutcome::Cancelled,
                Teardown::SourceRelative,
            ),
            (
                "plain success without run merge -> source-relative",
                json!({ "success": true }),
                TerminalOutcome::PlainSuccess,
                Teardown::SourceRelative,
            ),
        ];
        for (why, report, want_outcome, want_teardown) in rows {
            let n = node_with_report(Some(report.clone()));
            let got = TerminalOutcome::classify(&n).expect("terminal report classifies");
            assert_eq!(got, *want_outcome, "outcome: {why}");
            assert_eq!(got.teardown(), *want_teardown, "teardown: {why}");
        }
    }

    /// Only an explicit merge is ever force-deletable, and a merge marker with
    /// `success: false` is NOT a merge (never earns the force teardown).
    #[test]
    fn only_explicit_merge_force_deletes() {
        let merged = node_with_report(Some(json!({ "success": true, "via": "explicit-merge" })));
        assert!(TerminalOutcome::classify(&merged)
            .unwrap()
            .is_explicit_merge());

        let spoofed = node_with_report(Some(json!({ "success": false, "via": "explicit-merge" })));
        let outcome = TerminalOutcome::classify(&spoofed).unwrap();
        assert!(!outcome.is_explicit_merge(), "success:false is not a merge");
        assert_eq!(outcome.teardown(), Teardown::PreserveWork);
    }

    /// Regression (issue `raw-git-selfmerge-false-failed`): a worker that
    /// raw-git self-merged into source then DIED yields a crash-backstop `failed`
    /// report (`success: false`, `agent-died`, a supervisor origin) — it must
    /// classify `Failed` → `PreserveWork`, NOT force teardown. The content is in
    /// source but the branch/worktree stay preserved (the observability tradeoff
    /// is surfaced read-time by `run show`'s `false_failed` hint, never a
    /// destructive auto-success here). This is the no-destructive-teardown half
    /// of the tradeoff.
    #[test]
    fn raw_selfmerge_death_backstop_preserves_work() {
        let mut report = json!({ "success": false, "reason": "agent-died" });
        taskfleet_core::ReportOrigin::Supervisor.stamp(&mut report);
        let n = node_with_report(Some(report));
        let outcome = TerminalOutcome::classify(&n).expect("classifies");
        assert_eq!(outcome, TerminalOutcome::Failed);
        assert_eq!(
            outcome.teardown(),
            Teardown::PreserveWork,
            "a raw-selfmerge-then-death failure must preserve the branch + worktree"
        );
        assert!(
            !outcome.is_explicit_merge(),
            "a raw-git self-merge is NOT a recorded run merge — never force-deletable"
        );
    }

    /// A node with no terminal report is not classified — it is still live.
    #[test]
    fn no_report_is_not_terminal() {
        assert_eq!(TerminalOutcome::classify(&node_with_report(None)), None);
    }

    /// Cancel wins over a merge marker: a `cancelled: true` report is never
    /// force-torn-down even if a spoofed `via` — and even a spoofed
    /// `success: true` (the reducer rejects that on append, but `classify` is the
    /// teardown authority and must stay defensive against a corrupt projection) —
    /// rides along.
    #[test]
    fn cancel_beats_spoofed_merge_marker() {
        for report in [
            json!({ "success": false, "cancelled": true, "via": "explicit-merge" }),
            json!({ "success": true, "cancelled": true, "via": "explicit-merge" }),
        ] {
            let n = node_with_report(Some(report.clone()));
            let outcome = TerminalOutcome::classify(&n).unwrap();
            assert_eq!(outcome, TerminalOutcome::Cancelled, "report: {report}");
            assert_eq!(
                outcome.teardown(),
                Teardown::SourceRelative,
                "cancel must never force-delete: {report}"
            );
        }
    }

    /// The typed origin (issue `typed-report-origin`) drives classification when
    /// present: a `RunMerge` origin is the merge authority, a `Supervisor` origin
    /// splits to `Failed`, and an `Agent` origin splits to `Blocked` — WITHOUT any
    /// `reason` / `via` string sniffing.
    #[test]
    fn typed_origin_classifies_without_string_sniffing() {
        // A run-merge origin authorizes Merged even with NO `via` string.
        let merged = node_with_report(Some(json!({
            "success": true,
            "origin": { "kind": "run-merge", "op_id": "op-1", "worker_oid": "abc" }
        })));
        let outcome = TerminalOutcome::classify(&merged).expect("classifies");
        assert_eq!(outcome, TerminalOutcome::Merged);
        assert_eq!(outcome.teardown(), Teardown::Full);

        // A supervisor origin is a Failed split even with a non-standard reason.
        let sup = node_with_report(Some(json!({
            "success": false,
            "reason": "some-future-reason-not-in-the-legacy-list",
            "origin": { "kind": "supervisor" }
        })));
        let outcome = TerminalOutcome::classify(&sup).expect("classifies");
        assert_eq!(outcome, TerminalOutcome::Failed);
        assert_eq!(outcome.teardown(), Teardown::PreserveWork);

        // An agent origin is a Blocked split even when the reason LOOKS like a
        // supervisor reason (`agent-` prefix) — the typed origin wins over the
        // string convention.
        let agent = node_with_report(Some(json!({
            "success": false,
            "reason": "agent-said-so",
            "origin": { "kind": "agent" }
        })));
        let outcome = TerminalOutcome::classify(&agent).expect("classifies");
        assert_eq!(outcome, TerminalOutcome::Blocked);
        assert_eq!(outcome.teardown(), Teardown::PreserveWork);
    }

    /// Security regression: an AGENT-origin report that hand-sets
    /// `via: "explicit-merge"` + `success: true` is NOT a merge. Merge
    /// authorization is tied to the run-merge path (which stamps a `RunMerge`
    /// origin); a forged `via` string on an agent report can never earn the force
    /// teardown. It classifies as `PlainSuccess` (source-relative, preserves
    /// unmerged work).
    #[test]
    fn agent_origin_cannot_forge_a_merge_via_string() {
        let forged = node_with_report(Some(json!({
            "success": true,
            "via": "explicit-merge",
            "origin": { "kind": "agent" }
        })));
        let outcome = TerminalOutcome::classify(&forged).expect("classifies");
        assert_eq!(
            outcome,
            TerminalOutcome::PlainSuccess,
            "an agent-origin report must not be classified Merged on a forged via"
        );
        assert!(!outcome.is_explicit_merge());
        assert_eq!(outcome.teardown(), Teardown::SourceRelative);
    }

    /// Security regression (llm-review consensus): a report that CARRIES an
    /// `origin` field whose value fails to parse must NOT re-unlock the legacy
    /// forgeable `via` merge path. A present-but-malformed origin is "typed but
    /// unknown authority" — never a merge (classifies `PlainSuccess`), never a
    /// supervisor failure (a negative one classifies `Blocked`). Only a genuinely
    /// ABSENT origin falls back to the `via` / `reason` string sniff.
    #[test]
    fn malformed_origin_does_not_downgrade_to_legacy_via_or_reason() {
        // Malformed origin + forged via + success:true must NOT be Merged.
        let forged_merge = node_with_report(Some(json!({
            "success": true,
            "via": "explicit-merge",
            "origin": "garbage-not-an-object"
        })));
        let outcome = TerminalOutcome::classify(&forged_merge).expect("classifies");
        assert_eq!(
            outcome,
            TerminalOutcome::PlainSuccess,
            "a present-but-malformed origin must not unlock the legacy via merge path"
        );
        assert!(!outcome.is_explicit_merge());
        assert_eq!(outcome.teardown(), Teardown::SourceRelative);

        // Malformed origin + a supervisor-looking reason must NOT sniff to Failed;
        // an unknown author is conservatively Blocked (same teardown either way).
        let unknown_neg = node_with_report(Some(json!({
            "success": false,
            "reason": "worker-exited-nonzero",
            "origin": { "kind": "not-a-real-variant" }
        })));
        assert_eq!(
            TerminalOutcome::classify(&unknown_neg),
            Some(TerminalOutcome::Blocked)
        );
    }

    /// Legacy compatibility: a report with NO origin field classifies exactly as
    /// before — the `via` marker still means Merged, and the `reason`-string sniff
    /// still splits Failed vs Blocked.
    #[test]
    fn legacy_reports_without_origin_classify_via_strings() {
        // Legacy merge marker → Merged (backward compat with old on-disk runs).
        let legacy_merge =
            node_with_report(Some(json!({ "success": true, "via": "explicit-merge" })));
        assert_eq!(
            TerminalOutcome::classify(&legacy_merge),
            Some(TerminalOutcome::Merged)
        );

        // Legacy supervisor reason → Failed.
        let legacy_fail = node_with_report(Some(
            json!({ "success": false, "reason": "worker-exited-nonzero" }),
        ));
        assert_eq!(
            TerminalOutcome::classify(&legacy_fail),
            Some(TerminalOutcome::Failed)
        );

        // Legacy blocked handoff (agent reason) → Blocked.
        let legacy_blocked = node_with_report(Some(
            json!({ "success": false, "reason": "blocked-on-human" }),
        ));
        assert_eq!(
            TerminalOutcome::classify(&legacy_blocked),
            Some(TerminalOutcome::Blocked)
        );
    }

    /// The live-node table (design §2.6, non-terminal rows): told facts beat the
    /// pid backstop, a clean exit is attention-required (never failed), and the
    /// backstop only fires after the post-death grace.
    #[test]
    fn live_verdict_table() {
        let clean = WorkerExit {
            code: Some(0),
            signal: None,
            at: Utc::now(),
        };
        let nonzero = WorkerExit {
            code: Some(2),
            signal: None,
            at: Utc::now(),
        };
        let killed = WorkerExit {
            code: None,
            signal: Some(9),
            at: Utc::now(),
        };

        // Told failure -> WorkerFailed regardless of death observation.
        assert_eq!(
            classify_live_node(Some(nonzero), DeathObservation::Alive),
            LiveVerdict::WorkerFailed(nonzero)
        );
        assert_eq!(
            classify_live_node(Some(killed), DeathObservation::DeadGraceElapsed),
            LiveVerdict::WorkerFailed(killed)
        );
        // Clean exit -> AttentionRequired, NEVER failed, even if the pid is gone.
        assert_eq!(
            classify_live_node(Some(clean), DeathObservation::DeadGraceElapsed),
            LiveVerdict::AttentionRequired
        );
        // No told exit: the residual pid backstop governs.
        assert_eq!(
            classify_live_node(None, DeathObservation::Alive),
            LiveVerdict::Alive
        );
        assert_eq!(
            classify_live_node(None, DeathObservation::DeadWithinGrace),
            LiveVerdict::DeferGrace
        );
        assert_eq!(
            classify_live_node(None, DeathObservation::DeadGraceElapsed),
            LiveVerdict::CrashBackstopFailed
        );
    }
}
