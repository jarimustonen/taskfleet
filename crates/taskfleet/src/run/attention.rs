//! Read-time detection of an *attention-required* run (design.md §2.5 / A5,
//! issue `attention-required-run-surface`).
//!
//! An attention-required run is the design's deliberate "finished but skipped
//! `run merge`" case: the worker process exited **cleanly** (the launcher shim
//! recorded a `worker.exited` with `code == 0` and no signal) yet the node is
//! still **non-terminal** — no `explicit-merge` transition ever landed, so the
//! typed outcome table (§2.6) leaves it as
//! [`LiveVerdict::AttentionRequired`](crate::supervise::outcome::LiveVerdict::AttentionRequired) and
//! the supervisor does nothing further. Without this surface such a run sits
//! `pending`/`running` forever: `run wait` would block the whole timeout, and the
//! PO-review cadence that is supposed to discover the jam never sees it.
//!
//! Crucially this is **not a terminal state** and this module **never mutates**
//! anything — like its sibling [`crate::run::stalled`], it is a *computed*
//! read-time hint over a durable told fact (`Node::worker_exit`) plus the node's
//! current status. Terminalizing the run here would defeat the whole point: the
//! manual-finish path (§2.2 — drive `run merge` from the worktree, or
//! `run cancel`) needs the run left exactly where it is, work preserved.
//!
//! ## Why this is a durable fact, not a race
//!
//! The shim records `worker.exited` only **after** `wait()` on the agent process
//! returns — i.e. after the agent has exited. The *original* worker cannot go on
//! to run `run merge` after its exit is recorded, so at the moment of detection a
//! clean-exit-and-no-merge observation is unambiguous and needs no grace window
//! (unlike the orphaned-supervisor shape, whose "is it just restarting?"
//! ambiguity forces one). The computed *hint* is nonetheless not durable: the
//! whole point of the surface is that a human or a re-driven agent then runs
//! `run merge` (or `run cancel`), which drives the node terminal and clears the
//! hint. So "the fact is stable" means "the exited process can't itself merge",
//! not "the run stays attention-required forever".
//!
//! ## Precedence over the stall shapes
//!
//! Attention-required is checked **before** the [`crate::run::stalled`] verdicts.
//! A worker that exited cleanly and whose supervisor *also* later died would
//! otherwise match `orphaned` (dead supervisor, ≥1 node, idle) — but the correct
//! remediation is the manual finish (`run merge` from the worktree), NOT
//! `run reattach` (reviving the supervisor just re-observes the same clean exit
//! and does nothing). The told clean-exit fact is the more specific truth, so it
//! wins.

use chrono::{DateTime, Utc};

use taskfleet_core::{Status, WorkerExit};

/// Is this node in the attention-required state — a clean worker exit with the
/// node still working (so `run merge` was skipped)?
///
/// Returns `true` only when both hold:
///
/// - the node's status is `Pending` or `Running` — a node still nominally
///   *working*. A `Done`/`Failed`/`Cancelled` node already settled, and a
///   `Blocked` node is the design's DISTINCT "blocked handoff" row (§2.6) with its
///   own manual-action semantics — neither is the "finished but forgot to merge"
///   shape this detects, so both are excluded (a blocked node is surfaced by its
///   own terminal report, not conflated with attention).
/// - the launcher shim recorded a **clean** `worker.exited`
///   ([`WorkerExit::is_clean`] — `code == 0`, no signal). A failing exit is the
///   supervisor's `failed` path, not attention; an absent exit means the worker
///   is still running (or was never launched through the shim) — neither is
///   attention-required.
///
/// A merge, had it happened, would have driven the node terminal, so a still-
/// working node with a clean exit is exactly "finished but did not `run merge`".
/// `worker_exit` is taken by reference (a merged/still-working node is inspected,
/// never moved). Pure over its inputs; touches no event/reducer/schema path.
#[must_use]
pub fn is_attention_required(node_status: Status, worker_exit: Option<&WorkerExit>) -> bool {
    matches!(node_status, Status::Pending | Status::Running)
        && worker_exit.is_some_and(|e| e.is_clean())
}

/// Node-aware attention verdict, including resident rounds (ADR 0003).
///
/// A resident whose current attempt reported its round is *waiting*, not
/// attention-required, whatever its exit was: the round report is the told
/// end of the round. Idle expiry that found unlanded work raises attention
/// explicitly (`node.resident_attention`). Every other node uses
/// [`is_attention_required`]. Returns the reason and the anchoring exit.
#[must_use]
pub fn node_attention(node: &taskfleet_core::Node) -> Option<(&'static str, &WorkerExit)> {
    let exit = node.worker_exit.as_ref()?;
    if !matches!(node.status, Status::Pending | Status::Running) {
        return None;
    }
    if let Some(state) = node.resident.as_deref() {
        let reported = state
            .open_round()
            .is_some_and(|r| r.attempt == node.retry_attempts);
        if reported {
            return state
                .expiry_attention_at
                .is_some()
                .then_some((RESIDENT_IDLE_REASON, exit));
        }
    }
    is_attention_required(node.status, Some(exit)).then_some((ATTENTION_REASON, exit))
}

/// The stable `reason` for a resident whose idle expiry preserved unlanded work.
pub const RESIDENT_IDLE_REASON: &str =
    "resident worker passed its idle expiry with unlanded work (preserved, not merged)";

/// The human/JSON resume hint for an attention-required run: the two ways a PO
/// can settle it — the fenced manual finish (`run salvage`, which finds the
/// worktree, verifies/fences the prior worker, and drives the skipped merge
/// itself), or lay it to rest (`run cancel`). Kept as one shared helper so
/// `run wait` / `run show` / `run list` phrase it identically.
///
/// The run id is single-quoted via [`shell_single_quote`] so the emitted string
/// stays a safe copy-paste even if it carries a shell metacharacter (ids are
/// tool-generated, but the hint is meant to be pasted, so it must not break or
/// inject). `worktree_path` is retained in the signature (surfaced separately by
/// `run show`/`run list`) but no longer needs interpolating — `run salvage`
/// resolves the worktree from the run itself.
#[must_use]
pub fn resume_hint(run_id: &str, _worktree_path: Option<&str>) -> String {
    let q_run = shell_single_quote(run_id);
    format!(
        "worker finished but skipped `run merge`; finish it with \
         `taskfleet run salvage {q_run}` (verifies the prior worker is gone — or fences \
         it with `--fence` — then merges from its worktree), or `run cancel {q_run}` to lay it \
         to rest"
    )
}

/// Wrap `s` in single quotes for safe shell copy-paste, escaping any embedded
/// single quote with the standard `'\''` idiom (close-quote, escaped quote,
/// reopen-quote). A tool-generated worktree path won't normally need it, but the
/// hint is user-facing copy-paste, so it must never break on a stray space or
/// metacharacter.
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The resume-context fields `run show` / `run list` surface for an
/// attention-required run so a PO can find and finish the stuck worktree without
/// re-deriving them from the node/manifest projections (design.md §2.5).
///
/// Built only when [`is_attention_required`] holds; a `None` on the summary means
/// the run is not attention-required.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
pub struct AttentionView {
    /// Why the run needs attention. A stable machine string a JSON consumer can
    /// branch on without parsing [`Self::resume_hint`].
    pub reason: &'static str,
    /// Seconds the run has been *awaiting intervention* — `now - worker_exit.at`,
    /// the time since the worker actually finished, NOT since the run started.
    /// This is the actionable "how long has this been sitting done-but-unmerged"
    /// duration; a worker that ran for hours and exited seconds ago reads as a
    /// small age, not a false multi-hour jam. Clamped at 0 so clock skew never
    /// prints negative.
    pub pending_age_secs: i64,
    /// The instant the worker exited (`worker_exit.at`) — the anchor for
    /// [`Self::pending_age_secs`], surfaced so a JSON consumer can compute its own
    /// freshness against its own clock.
    pub exited_at: DateTime<Utc>,
    /// The last-observed worker PID (`node.agent_pid`) — the process the shim
    /// wrapped, now exited. Surfaced as evidence of which worker finished; `null`
    /// when the node never recorded a pid.
    pub worker_pid: Option<i32>,
    /// The node's git worktree, where the manual finish (`run merge`) is driven.
    /// `null` when unrecorded.
    pub worktree_path: Option<String>,
    /// The run's source/target branch the skipped merge would land on. `null` for
    /// a legacy run that never recorded one.
    pub source_branch: Option<String>,
    /// One-line resume hint (see [`resume_hint`]) — the two ways to settle it.
    pub resume_hint: String,
}

/// The stable `reason` string on [`AttentionView`].
pub const ATTENTION_REASON: &str = "worker exited cleanly without running `run merge`";

impl AttentionView {
    /// Assemble the resume-context view for an attention-required run. `now` is
    /// injected so callers share one per-scan clock and tests are deterministic.
    /// `worker_exit` is the node's told clean exit ([`is_attention_required`]
    /// guarantees it is present and clean at every call site) — its `.at` anchors
    /// the awaiting-intervention age.
    #[cfg(test)]
    #[must_use]
    pub fn build(
        run_id: &str,
        now: DateTime<Utc>,
        worker_exit: &WorkerExit,
        worker_pid: Option<i32>,
        worktree_path: Option<String>,
        source_branch: Option<String>,
    ) -> Self {
        Self::build_for(
            ATTENTION_REASON,
            run_id,
            now,
            worker_exit,
            worker_pid,
            worktree_path,
            source_branch,
        )
    }

    /// [`Self::build`] for a reason from [`node_attention`].
    #[must_use]
    pub fn build_for(
        reason: &'static str,
        run_id: &str,
        now: DateTime<Utc>,
        worker_exit: &WorkerExit,
        worker_pid: Option<i32>,
        worktree_path: Option<String>,
        source_branch: Option<String>,
    ) -> Self {
        let exited_at = worker_exit.at;
        let pending_age_secs = now.signed_duration_since(exited_at).num_seconds().max(0);
        let resume_hint = if reason == RESIDENT_IDLE_REASON {
            let q_run = shell_single_quote(run_id);
            format!(
                "resident worker idle with unlanded work; land it with `taskfleet run land {q_run}`,                  send it the next message with `taskfleet run message {q_run} <text>`, or finish it                  with `run merge {q_run}` / `run cancel {q_run}`"
            )
        } else {
            resume_hint(run_id, worktree_path.as_deref())
        };
        Self {
            reason,
            pending_age_secs,
            exited_at,
            worker_pid,
            worktree_path,
            source_branch,
            resume_hint,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exit(code: Option<i32>, signal: Option<i32>) -> WorkerExit {
        exit_at(code, signal, Utc::now())
    }

    fn exit_at(code: Option<i32>, signal: Option<i32>, at: DateTime<Utc>) -> WorkerExit {
        WorkerExit { code, signal, at }
    }

    /// The exact attention signature: a still-working node with a clean (0) exit.
    #[test]
    fn clean_exit_working_node_is_attention() {
        for status in [Status::Pending, Status::Running] {
            assert!(
                is_attention_required(status, Some(&exit(Some(0), None))),
                "status {status:?} with a clean exit must be attention-required"
            );
        }
    }

    /// A terminal node already settled — never attention, even with a clean exit
    /// recorded (a merged node exits 0 too, but it is Done, not awaiting a human).
    #[test]
    fn terminal_node_is_not_attention() {
        for status in [Status::Done, Status::Failed, Status::Cancelled] {
            assert!(
                !is_attention_required(status, Some(&exit(Some(0), None))),
                "terminal status {status:?} must not be attention-required"
            );
        }
    }

    /// A `Blocked` node is the DISTINCT blocked-handoff row (design §2.6), not
    /// attention — even with a clean worker exit recorded. It is surfaced by its
    /// own terminal report, never conflated with "forgot to merge".
    #[test]
    fn blocked_node_is_not_attention() {
        assert!(!is_attention_required(
            Status::Blocked,
            Some(&exit(Some(0), None))
        ));
    }

    /// A failing exit is the supervisor's `failed` path, not attention — including
    /// the mixed `code: Some(0)` WITH a signal (a signalled death is never clean).
    #[test]
    fn failing_exit_is_not_attention() {
        assert!(!is_attention_required(
            Status::Running,
            Some(&exit(Some(2), None))
        ));
        assert!(!is_attention_required(
            Status::Running,
            Some(&exit(None, Some(9)))
        ));
        // code 0 but signalled → not clean, not attention.
        assert!(!is_attention_required(
            Status::Running,
            Some(&exit(Some(0), Some(15)))
        ));
    }

    /// No recorded exit means the worker is still running (or was never launched
    /// through the shim) — not attention-required.
    #[test]
    fn no_exit_is_not_attention() {
        assert!(!is_attention_required(Status::Running, None));
        assert!(!is_attention_required(Status::Pending, None));
    }

    /// `pending_age_secs` is `now - worker_exit.at` (time awaiting intervention,
    /// NOT since the run started), clamped at 0 under clock skew.
    #[test]
    fn pending_age_anchors_on_worker_exit_and_clamps() {
        let now: DateTime<Utc> = "2026-08-15T12:00:00Z".parse().unwrap();
        // Exited 30 min ago → 1800s, regardless of how long the run had been going.
        let exited = exit_at(Some(0), None, "2026-08-15T11:30:00Z".parse().unwrap());
        let v = AttentionView::build("r", now, &exited, None, None, None);
        assert_eq!(v.pending_age_secs, 30 * 60);
        assert_eq!(v.exited_at, exited.at);

        // Clock skew (future exit) clamps to 0, never negative.
        let future = exit_at(Some(0), None, "2026-08-15T13:00:00Z".parse().unwrap());
        let v = AttentionView::build("r", now, &future, None, None, None);
        assert_eq!(v.pending_age_secs, 0);
    }

    /// The resume hint names both remediations (`run salvage` and `run cancel`)
    /// and single-quotes the run id — identically whether or not a worktree path
    /// is known (salvage resolves the worktree itself).
    #[test]
    fn resume_hint_mentions_both_actions() {
        for wt in [Some("/tmp/wt/foo"), None] {
            let hint = resume_hint("01run", wt);
            assert!(hint.contains("run salvage '01run'"), "got: {hint}");
            assert!(hint.contains("run cancel '01run'"), "got: {hint}");
        }
    }

    /// A run id with a shell metacharacter is single-quoted so the emitted command
    /// is a safe copy-paste, not a broken or injecting one.
    #[test]
    fn resume_hint_quotes_hostile_ids() {
        let hint = resume_hint("01'; rm -rf /", None);
        // Single quote is escaped via the '\'' idiom.
        assert!(hint.contains("'01'\\''; rm -rf /'"), "got: {hint}");
    }
}
