//! Read-time detection of a *suspected false-failed* run — a `failed` run whose
//! worker content is git-verified in source, but which no `run merge`
//! transaction recorded (issue `raw-git-selfmerge-false-failed`, epic
//! `lifecycle-architecture-review`).
//!
//! ## The thin-model tradeoff this surfaces
//!
//! Under the thin supervisor, `run merge` is the ONLY success truth (invariant 6
//! / design §2.1b): it records a `merge.started` transaction, stamps a typed
//! [`ReportOrigin::RunMerge`](taskfleet_core::ReportOrigin) terminal report, and is the
//! only path the recovery machinery can complete. An agent that instead
//! **hand-merges its branch into source with raw git** (`git checkout main &&
//! git merge wt/...`) and then dies leaves NO merge transaction and NO typed
//! merge origin. The crash backstop (design §2.1a) then confirms the worker gone,
//! finds no merge, and synthesizes a `failed` report — even though the worker's
//! content is already integrated into source.
//!
//! This is **not data loss**: the teardown gate (invariant 5) preserves the
//! branch AND worktree on every non-explicit-merge terminal, so the work is
//! exactly where a human left it. It is an **observability tradeoff** — a run
//! reads `failed` while its content is, in fact, in source.
//!
//! ## What this module does — and deliberately does NOT do
//!
//! It is a pure, read-time, **non-mutating** hint (like its siblings
//! [`crate::run::attention`] / [`crate::run::stalled`]) computed by `run show`.
//! It **never terminalizes the run to `done`** and never re-classifies the
//! outcome. Auto-flipping a `failed` run to `done` off a branch-content
//! heuristic is exactly the inference the thin model deletes (the removed
//! git-reconcile-implies-done probe, invariant 7) — resurrecting it would let a
//! forged/coincidental branch state fake a success. So this surfaces a
//! **suspicion + remediation**, not a verdict: the human runs
//! [`run salvage`](crate::run::salvage) — which drives the skipped merge through
//! the real `run merge` machinery (recording the transaction, the typed origin,
//! the CAS-guarded fast-forward), idempotently against the already-integrated
//! content — to record the merge and terminalize the run to `done` honestly.
//!
//! ## The exact firing signature
//!
//! [`is_false_failed_suspected`] fires only when ALL of:
//!
//! - the run's status is `Failed` — a settled negative terminal. A live/pending
//!   run is `attention`/`stall` territory, and a `done`/`cancelled` run has no
//!   false-failed to suspect.
//! - `landed` is `true` **and** its method is
//!   [`LandedMethod::GitVerified`]
//!   — git's *live, authoritative* view says every branch commit is integrated
//!   into the current source tip (patch-id equivalence, rebase-robust). A
//!   `report-marker` landing is deliberately EXCLUDED: that marker only exists on
//!   a confirmed `run merge`, which would have rolled the run to `done`, not
//!   `failed` — so seeing it on a `failed` run would mean a corrupt projection,
//!   not a raw-git self-merge. Requiring `git-verified` means the suspicion rests
//!   on git ground truth, never on a report field.
//! - the terminal report is NOT a confirmed `run merge` (no typed `RunMerge`
//!   origin / legacy `via: "explicit-merge"`). A recorded merge is the honest
//!   `done` path; its absence beside git-verified-landed content is the whole
//!   tell.
//! - a **fork-point (`base_sha`) is recorded** for the node. This is the stricter
//!   burden of proof this consumer demands over the bare `landed` signal (llm
//!   review consensus, gemini + deepseek): without a `base_sha`,
//!   [`crate::run::landed`]'s ancestry safety net cannot tell a genuine landing
//!   from a **never-advanced branch that merged nothing** — a branch trivially an
//!   ancestor of source with zero commits reads `landed: true, git-verified` under
//!   the missing-base fallback (`branch_advanced_past_base` returns `true` when
//!   `base` is `None`). Firing on that would tell a user to `run salvage` a branch
//!   with no work. Every normal spinoff records `base_sha` at `node.created`, so
//!   requiring it suppresses only the ambiguous legacy/corrupt no-fork-point case
//!   — a deliberately conservative false-negative (never a false positive).
//!
//! Every branch of that AND is load-bearing: drop the git-verified requirement
//! and a stale marker could fire it; drop the not-a-merge requirement and an
//! honest `done` run (mis-statused) could; drop the `Failed` gate and a
//! still-running raw-git-merger (not yet dead) would false-flag before the human
//! could even act; drop the `base_sha` requirement and a never-advanced branch
//! that merged nothing would falsely read as landed.
//!
//! ## Accepted blind spots (false negatives, never false positives)
//!
//! The signal rests on git's *history* view of the recorded worker branch, so it
//! deliberately does NOT fire — but also never mis-fires — in these shapes:
//!
//! - **Single-worker only.** `run show` gates this on `node_count == 1` (mirroring
//!   [`crate::run::attention`]): the reporting node is `n-0001`, so for a
//!   multi-node fan-out a *child* worker that raw-selfmerged then died is NOT
//!   surfaced (its `n-0001` may be a driver). Per-node false-failed is the
//!   delegated `per-node-run` follow-up.
//! - **Squash merges.** A `git merge --squash` collapses the branch's commits into
//!   one new patch-id, so `git cherry` sees `+` and ancestry is false — `landed`
//!   reads false and the hint stays silent. The content IS in source but git
//!   cannot prove the *branch's* commits landed.
//! - **Post-merge extra commits.** If the worker raw-merged then committed MORE on
//!   the branch before dying, `git cherry` sees a `+` for the extra commit and
//!   `landed` reads false — the run's *earlier* work is in source but the branch
//!   is no longer fully integrated, so the hint stays silent.
//!
//! In every case a false negative is strictly safer than falsely telling a user
//! their work is already merged.

use serde_json::Value;
use taskfleet_core::ReportOrigin;

use crate::run::landed::LandedMethod;

/// Does this run look like a *false-failed* raw-git self-merge — `failed`, yet
/// git confirms the branch content is in source and no `run merge` recorded it?
///
/// Pure over its inputs (all already computed by `run show`): the manifest
/// status, the git-verified [`crate::run::landed`] signal, and the reporting
/// node's terminal report. Touches no event/reducer/schema/lock path and never
/// mutates — see the module docs for why this must stay a hint, never a verdict.
///
/// `status_is_failed` is passed as a bool rather than a `Status` so the caller
/// keeps the single source of truth for "is this the failed terminal" (and so
/// this stays trivially testable without constructing a `Status`).
/// `base_sha_present` is `node.base_sha.is_some()` — the recorded fork point; see
/// the module docs for why it is a required burden of proof here (it closes the
/// never-advanced-branch false positive the bare `landed` signal admits when the
/// fork point is unknown).
#[must_use]
pub fn is_false_failed_suspected(
    status_is_failed: bool,
    landed: bool,
    landed_method: LandedMethod,
    base_sha_present: bool,
    report: Option<&Value>,
) -> bool {
    status_is_failed
        && landed
        && landed_method == LandedMethod::GitVerified
        && base_sha_present
        && !report.is_some_and(ReportOrigin::report_is_confirmed_merge)
}

/// The stable machine `reason` string on [`FalseFailedView`]. A JSON consumer can
/// branch on this without parsing [`FalseFailedView::resume_hint`].
pub const FALSE_FAILED_REASON: &str =
    "branch content is git-verified in source but no `run merge` recorded it (raw-git self-merge?)";

/// Resume context `run show` surfaces for a suspected false-failed run so a human
/// can settle it honestly without re-deriving the git state by hand.
///
/// Built only when [`is_false_failed_suspected`] holds; a `None` on the payload
/// means the run is not a suspected false-failed.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
pub struct FalseFailedView {
    /// Why the run is suspected false-failed — the stable [`FALSE_FAILED_REASON`]
    /// string.
    pub reason: &'static str,
    /// One-line remediation (see [`resume_hint`]): record the merge via
    /// `run salvage`, or `run cancel` to accept the `failed` and preserve the
    /// branch.
    pub resume_hint: String,
}

impl FalseFailedView {
    /// Assemble the resume-context view for a suspected false-failed run.
    #[must_use]
    pub fn build(run_id: &str) -> Self {
        Self {
            reason: FALSE_FAILED_REASON,
            resume_hint: resume_hint(run_id),
        }
    }
}

/// The human/JSON remediation hint for a suspected false-failed run: the honest
/// way to settle it is [`run salvage`](crate::run::salvage), which drives the
/// skipped merge through the real `run merge` machinery — idempotent against the
/// already-integrated content — so the run terminalizes to `done` with a recorded
/// transaction and typed origin, instead of sitting `failed` while its work is in
/// source. `run cancel` is the alternative when the human accepts the `failed`
/// (the branch stays preserved either way).
///
/// The run id is single-quoted via [`shell_single_quote`] so the emitted command
/// stays a safe copy-paste even if it carries a shell metacharacter (ids are
/// tool-generated, but the hint is meant to be pasted, so it must not break or
/// inject). Phrased identically to [`crate::run::attention::resume_hint`]'s
/// `run salvage` pointer so the two read the same.
#[must_use]
pub fn resume_hint(run_id: &str) -> String {
    let q = shell_single_quote(run_id);
    format!(
        "the run is `failed` but its branch content is already in source with no `run merge` on \
         record — record the merge honestly with `taskfleet run salvage {q}` (drives the \
         skipped merge idempotently and terminalizes the run to `done`), or \
         `taskfleet run cancel {q}` to accept the failure (the branch is preserved either \
         way). Do NOT hand-merge with raw git; always finish through `taskfleet run merge`"
    )
}

/// Wrap `s` in single quotes for safe shell copy-paste, escaping any embedded
/// single quote with the standard `'\''` idiom (close-quote, escaped quote,
/// reopen-quote). Mirrors [`crate::run::attention`]'s helper — a tool-generated
/// run id won't normally need it, but the hint is user-facing copy-paste, so it
/// must never break on a stray metacharacter.
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The exact false-failed signature: `failed` + git-verified landed + a
    /// recorded fork point + a non-merge terminal report (a crash-backstop
    /// `failed`).
    #[test]
    fn failed_git_verified_landed_without_merge_is_suspected() {
        let backstop = json!({ "success": false, "reason": "agent-died" });
        assert!(is_false_failed_suspected(
            true,
            true,
            LandedMethod::GitVerified,
            true,
            Some(&backstop),
        ));
        // Also fires with no terminal report at all (the crash backstop may fire
        // before any report lands) — still `failed` + git-verified landed.
        assert!(is_false_failed_suspected(
            true,
            true,
            LandedMethod::GitVerified,
            true,
            None,
        ));
    }

    /// A missing fork point (`base_sha`) suppresses the hint even when git reads
    /// the branch as landed — the ancestry net cannot distinguish a genuine
    /// landing from a never-advanced branch without a fork point, so the stricter
    /// burden of proof declines rather than risk a false positive (llm-review
    /// consensus, gemini + deepseek).
    #[test]
    fn missing_base_sha_is_not_suspected() {
        let backstop = json!({ "success": false, "reason": "agent-died" });
        assert!(!is_false_failed_suspected(
            true,
            true,
            LandedMethod::GitVerified,
            false, // no base_sha recorded
            Some(&backstop),
        ));
    }

    /// A confirmed `run merge` terminal report is the honest path — never
    /// suspected, even if somehow observed on a `failed` status. Both the typed
    /// `RunMerge` origin and the legacy `via` marker suppress the suspicion.
    #[test]
    fn confirmed_merge_report_is_not_suspected() {
        let mut typed = json!({ "success": true });
        ReportOrigin::RunMerge {
            op_id: Some("op-1".into()),
            worker_oid: Some("abc".into()),
            integrated_oid: None,
        }
        .stamp(&mut typed);
        assert!(!is_false_failed_suspected(
            true,
            true,
            LandedMethod::GitVerified,
            true,
            Some(&typed),
        ));

        let legacy = json!({ "success": true, "via": "explicit-merge" });
        assert!(!is_false_failed_suspected(
            true,
            true,
            LandedMethod::GitVerified,
            true,
            Some(&legacy),
        ));
    }

    /// A `report-marker` landing is NOT git ground truth — it only exists on a
    /// confirmed merge (which would be `done`, not `failed`). Excluded so the
    /// suspicion always rests on git's live view, never a report field.
    #[test]
    fn report_marker_landing_is_not_suspected() {
        let report = json!({ "success": false, "reason": "agent-died" });
        assert!(!is_false_failed_suspected(
            true,
            true,
            LandedMethod::ReportMarker,
            true,
            Some(&report),
        ));
    }

    /// An unlanded / unverified run is never suspected — there is no content in
    /// source to have been silently integrated.
    #[test]
    fn unlanded_or_unverified_is_not_suspected() {
        let report = json!({ "success": false, "reason": "agent-died" });
        // Not landed at all.
        assert!(!is_false_failed_suspected(
            true,
            false,
            LandedMethod::GitVerified,
            true,
            Some(&report),
        ));
        // Unverified method (git couldn't run) — landed can only be false there,
        // but assert the method gate too for defence in depth.
        assert!(!is_false_failed_suspected(
            true,
            false,
            LandedMethod::Unverified,
            true,
            Some(&report),
        ));
    }

    /// Only the `failed` terminal is suspected — a still-running raw-git-merger
    /// (not yet dead) or a `done`/`cancelled` run must not false-flag.
    #[test]
    fn only_failed_status_is_suspected() {
        assert!(!is_false_failed_suspected(
            false,
            true,
            LandedMethod::GitVerified,
            true,
            None,
        ));
    }

    /// The resume hint names both remediations — and BOTH are fully
    /// `taskfleet`-qualified so they are directly copy-pasteable (llm-review:
    /// a bare `run cancel` would fail in the user's shell). It warns against
    /// raw-git merges and single-quotes the run id.
    #[test]
    fn resume_hint_names_salvage_and_cancel() {
        let hint = resume_hint("01run");
        assert!(
            hint.contains("taskfleet run salvage '01run'"),
            "got: {hint}"
        );
        assert!(
            hint.contains("taskfleet run cancel '01run'"),
            "cancel must be fully qualified for copy-paste: {hint}"
        );
        assert!(
            hint.contains("run merge"),
            "must steer to run merge: {hint}"
        );
    }

    /// A run id with a shell metacharacter is single-quoted so the emitted command
    /// is a safe copy-paste, not a broken or injecting one.
    #[test]
    fn resume_hint_quotes_hostile_ids() {
        let hint = resume_hint("01'; rm -rf /");
        assert!(hint.contains("'01'\\''; rm -rf /'"), "got: {hint}");
    }

    /// `FalseFailedView::build` carries the stable reason and the hint.
    #[test]
    fn view_build_carries_reason_and_hint() {
        let v = FalseFailedView::build("01run");
        assert_eq!(v.reason, FALSE_FAILED_REASON);
        assert!(v.resume_hint.contains("run salvage '01run'"));
    }
}
