//! `run merge` — own the full merge lifecycle of one worktree run.
//!
//! Closes the spawn → work → merge → cleanup loop end-to-end inside
//! Taskfleet (issue `bundle-worktree-merge`). Before this verb the
//! merge half lived in an external `/worktree-merge` bash skill, which
//! had no knowledge of the run: it merged, but never submitted a terminal
//! `node.report`, so the supervisor kept polling and (for interactive
//! kinds) never tore the window down. `run merge` does both halves in one
//! call:
//!
//!   1. **Merge mechanics** — shell out to the bundled `merge.sh`
//!      (embedded below, materialized to a temp file at runtime). It owns
//!      the rebase, the cross-worktree `flock`, and the `workmux merge`. v1
//!      deliberately wraps the script rather than re-implementing git
//!      wrappers in Rust (issue §4); v2 can move it into core. The worktree /
//!      window / branch teardown is NOT merge.sh's — it belongs to the
//!      supervisor (state-integrity invariant #5).
//!   2. **Terminal report** — on a clean merge, append a `node.report`
//!      with `via: "explicit-merge"`. That flag is the signal the
//!      supervisor's cleanup gate checks to extend teardown to
//!      *interactive* kinds: a user who runs `run merge` is done with the
//!      review window, so it may close (see supervise/cleanup.rs).
//!   3. **Ensure the supervisor tears down.** The supervisor is the SOLE
//!      teardown actor (state-integrity invariant #5); `run merge` never reclaims
//!      resources itself. Once its `via: "explicit-merge"` report is appended, the
//!      taskfleet-core reducer ADOPTS it — even against a node an earlier watchdog
//!      `agent-died` false positive already terminalized (issue
//!      `reducer-adopt-explicit-merge`) — so the cleanup gate sees the merge marker
//!      on every path and warrants teardown. The one path with no LIVE supervisor
//!      is exactly that swallowed case: the watchdog terminal rolled the run up and
//!      its supervisor exited before the merge. [`ensure_report_consumer`]
//!      reattaches one there (reattaching on a terminal-but-teardown-warranted run
//!      when the reducer freshly adopted the report), so the reattached supervisor
//!      runs the same `cleanup_terminal_nodes` and exits. This replaced the inline
//!      worktree/branch reclaim shipped for `merge-skips-teardown` /
//!      `agent-died-merge-no-teardown-interactive`, restoring single-owner teardown.
//!
//! On a merge failure (conflicts, dirty tree, lock timeout) the report is
//! NOT submitted — the node stays live so the agent can recover (e.g.
//! `/complex-rebase`) and re-run `run merge`.

use std::io::{Read as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;
use serde::Serialize;
use serde_json::{json, Value};

use taskfleet_core::report::sanitize_report_advisory;
use taskfleet_core::{
    append_and_apply_event, read_all_events, read_manifest_opt, read_node_opt, AdvisoryWarning,
    MergeTxn, Node, NodeId, RunLock,
};

use crate::run::merge_recovery;
use crate::supervise::cleanup::git_bin;

use crate::error::{CliError, ExitKind};
use crate::output::{self, OutputFormat, OutputSpec};
use crate::run::dto::SupervisorView;
use crate::run::{from_core, parse_node_id, reattach, require_nonempty, run_paths_from_cli_arg};
use crate::supervise::cleanup;

/// The bundled merge backend, embedded at compile time so the binary is
/// self-contained (the external `merge.sh` is sunset). Materialized to a
/// temp file and executed per invocation. Tests override the resolved
/// script via `TASKFLEET_MERGE_SH`.
const MERGE_SH: &str = include_str!("../../scripts/merge.sh");

/// Default reporting node for a single-worker run. Every worktree kind
/// `run merge` targets has exactly one node.
const DEFAULT_NODE_ID: &str = "n-0001";

/// Cap on `--report-file` size, mirroring `node report`'s 1 MiB bound.
const MAX_REPORT_BYTES: u64 = 1024 * 1024;

/// The exit status `merge.sh` reserves for "could not acquire the merge lock in
/// time" — another self-merge into the SAME target branch held the serializing
/// lock past the timeout (issue `concurrent-self-merge-race`). It is the sole
/// producer of this status in the script (the mkdir-lock acquire loop is the
/// only path that emits 75, and the `workmux` invocation normalizes its exit so
/// a downstream 75 can't leak), so mapping it to a distinct,
/// retryable `merge_in_progress` code is unambiguous. Value is `EX_TEMPFAIL`
/// (75) from sysexits(3): "temporary failure, the user is invited to retry".
const MERGE_SH_LOCK_TIMEOUT_EXIT: i32 = 75;

/// The exit status `merge.sh` reserves for a compare-and-swap mismatch — the
/// target branch moved off the recorded `expected_source_oid` between the moment
/// `run merge` opened the transaction and the moment merge.sh held the merge lock
/// (design.md §2.1b / A2). Distinct from the dirty-tree/conflict failure (1) and
/// the lock-timeout retry (75), so it maps to its own retryable `merge_source_moved`
/// code: the agent rebases onto the moved source and re-runs `run merge`.
const MERGE_SH_CAS_MISMATCH_EXIT: i32 = 76;

pub struct Args<'a> {
    pub run_id: String,
    /// Override the merge target branch. Falls back to the manifest's
    /// `source_branch`, then to merge.sh's own main/master auto-detection.
    pub source: Option<String>,
    /// Reporting node id; defaults to `n-0001`.
    pub node_id: Option<String>,
    /// Optional §7.3 report payload to submit on a clean merge. When set,
    /// `run merge` stamps it with `via: "explicit-merge"` and submits it —
    /// so an autonomous kind can carry its rich `discussion_items` /
    /// `spinoff_proposals` / `wrap_up_recommendations` in the SAME call
    /// that merges. When absent, a minimal `{success, summary, via}` report
    /// is submitted (enough for a simple spinoff).
    pub report_file: Option<PathBuf>,
    pub dry_run: bool,
    pub spec: &'a OutputSpec,
    pub warnings: &'a [String],
}

#[derive(Serialize)]
struct MergePayload<'a> {
    run_id: &'a str,
    node_id: &'a str,
    branch: &'a str,
    /// The resolved merge target, or `null` when left to merge.sh's
    /// main/master auto-detection.
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a str>,
    merged: bool,
    /// `node.report` seq, when a terminal report was appended.
    #[serde(skip_serializing_if = "Option::is_none")]
    report_seq: Option<u64>,
    /// Outcome of ensuring a live consumer for the terminal report — the
    /// machine-readable companion to any `warnings` entry, so an agent reads a
    /// `state` instead of regex-parsing prose. Present on a real merge, absent
    /// on `--dry-run`. Additive (no `schema_version` bump).
    #[serde(skip_serializing_if = "Option::is_none")]
    supervisor: Option<ConsumerOutcome>,
    /// Advisory report fields/elements the lenient sanitizer dropped rather than
    /// block the merge (issue `merge-report-schema-lenience`). Omitted when empty
    /// so a clean report's envelope is byte-identical to before.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    report_advisory_warnings: &'a [AdvisoryWarning],
    #[serde(skip_serializing_if = "Option::is_none")]
    dry_run: Option<bool>,
}

/// Machine-readable result of [`ensure_report_consumer`]: what state the run's
/// per-run supervisor was left in after the terminal report was appended. The
/// non-silent counterpart to the merge `warnings`.
#[derive(Serialize, Clone)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub(crate) enum ConsumerOutcome {
    /// A live supervisor is already consuming the report on its next tick.
    Alive,
    /// The run was already terminal — a supervisor rolled it up and exited, so
    /// nothing remains to consume.
    Terminal,
    /// The run was never supervised (e.g. a skeleton/test run) — there is no
    /// teardown actor to restart, and spawning one would be wrong.
    NotSupervised,
    /// The dead supervisor was restarted; teardown is (re)running. `pid` is the
    /// new supervisor's pid, or `null` when the spawn was not yet confirmed.
    Reattached { pid: Option<u32> },
    /// No live consumer could be ensured; teardown is deferred until the caller
    /// runs `recovery_command`.
    Deferred { recovery_command: String },
}

/// The owned, emit-independent result of driving the full merge lifecycle for
/// one run. Produced by [`execute`] and rendered by [`run`] (via [`emit`]).
///
/// It exists so a second caller — `run salvage` (issue `run-salvage-command`,
/// design.md §2.2) — can drive the *identical* merge machinery (crash-recovery,
/// CAS-guarded `merge.sh`, the `via: "explicit-merge"` terminal report,
/// supervisor reattach) and then fold this result into its OWN envelope, instead
/// of re-implementing a raw git self-merge that would bypass the merge-transaction
/// record and the teardown gate. Every field is owned so it outlives the borrowed
/// [`Args`].
pub(crate) struct MergeOutcome {
    pub run_id: String,
    pub node_id: String,
    pub branch: String,
    pub source: Option<String>,
    pub merged: bool,
    pub report_seq: Option<u64>,
    pub supervisor: Option<ConsumerOutcome>,
    pub dry_run: bool,
    /// The caller's base warnings plus any appended by [`ensure_report_consumer`].
    pub warnings: Vec<String>,
    /// Machine-readable record of every advisory report field/element dropped by
    /// the lenient sanitizer (issue `merge-report-schema-lenience`). The structured
    /// companion to the human-readable `dropped …` entries in `warnings`. Empty
    /// when the report validated cleanly.
    pub report_advisory_warnings: Vec<AdvisoryWarning>,
}

pub fn run(args: Args<'_>) -> Result<(), CliError> {
    let spec = args.spec;
    let outcome = execute(&args)?;
    let payload = MergePayload {
        run_id: &outcome.run_id,
        node_id: &outcome.node_id,
        branch: &outcome.branch,
        source: outcome.source.as_deref(),
        merged: outcome.merged,
        report_seq: outcome.report_seq,
        supervisor: outcome.supervisor.clone(),
        report_advisory_warnings: &outcome.report_advisory_warnings,
        dry_run: outcome.dry_run.then_some(true),
    };
    emit(&payload, spec, &outcome.warnings)
}

/// Drive the full merge lifecycle and return the owned [`MergeOutcome`] without
/// emitting anything. `run` (this file) and `salvage::run` are the two callers:
/// each renders the result into its own envelope.
pub(crate) fn execute(args: &Args<'_>) -> Result<MergeOutcome, CliError> {
    let run_id = args.run_id.clone();
    let node_id = parse_node_id(args.node_id.as_deref().unwrap_or(DEFAULT_NODE_ID))?;
    let source = match &args.source {
        Some(s) => Some(require_nonempty(s, "source")?),
        None => None,
    };

    let root = crate::home::root_dir()?;
    let paths = run_paths_from_cli_arg(&root, &run_id)?;
    // `run_id` may have been an unambiguous PREFIX; from here on use the fully
    // resolved id so the terminal-report idempotency key
    // (`explicit-merge:{run_id}:{node_id}`) is stable across a prefix invocation
    // and a later full-id retry — otherwise a crash-retry with the full id would
    // compute a different key and double-append the report.
    let run_id = paths.run_id.as_str().to_string();

    let manifest = read_manifest_opt(&paths)
        .map_err(from_core)?
        .ok_or_else(|| {
            CliError::user("run_not_found", format!("no run with id {run_id}"))
                .with_invalid_value(&run_id)
        })?;

    // A run recorded under a removed kind is read-only (ADR §D7) — refuse before
    // any merge/append so we never rewrite its manifest (and destroy its
    // provenance) or self-merge a legacy human-reviewed `code` run.
    crate::run::reject_legacy_kind(manifest.kind, &run_id)?;

    let node = read_node_opt(&paths, &node_id)
        .map_err(from_core)?
        .ok_or_else(|| {
            CliError::user(
                "node_not_found",
                format!("no node {node_id} in run {run_id}"),
            )
            .with_invalid_value(node_id.as_str())
        })?;

    // The worktree directory is the cwd the merge backend needs: it derives
    // the branch and the source-side worktree from `git` run there. A node
    // with no materialized worktree (a driver node) cannot be merged.
    let worktree_path = node.worktree_path.as_deref().ok_or_else(|| {
        CliError::user(
            "no_worktree",
            format!("node {node_id} has no worktree to merge (driver node?)"),
        )
        .with_invalid_value(node_id.as_str())
    })?;
    let branch = branch_for(&node).ok_or_else(|| {
        CliError::user(
            "no_branch",
            format!("node {node_id} has no branch recorded; cannot merge"),
        )
        .with_invalid_value(node_id.as_str())
    })?;

    // Terminal-state / torn-down-worktree guard (issue `merge-terminal-misleading`).
    //
    // A run that is already done merges its worktree away: the supervisor tears
    // the worktree down and exits when the run rolls up terminal (invariant #5).
    // A later `run merge` would then materialize merge.sh and `cd` into a
    // directory that no longer exists, failing with a misleading
    // `merge_spawn_failed: … No such file or directory` that fingers the temp
    // script instead of the real cause (the run is already finished). Refuse up
    // front with a clear code and NO spawn attempt.
    //
    // The discriminator is deliberately worktree EXISTENCE, not "was there an
    // explicit-merge report" — the latter would (a) wrongly refuse the ONE
    // legitimate merge against a terminal run: the watchdog `agent-died`
    // false-positive path (issue `reducer-adopt-explicit-merge`), where the run
    // is terminal (`Failed`) but the still-alive agent's worktree survives (a
    // blocked handoff PRESERVES it) and its `run merge` must proceed and be
    // adopted; and (b) break the documented crash-safe idempotent retry — if a
    // merge appended its report then crashed before `ensure_report_consumer`,
    // the worktree still exists, so this falls through to the idempotent
    // re-merge + reattach path that completes teardown. Worktree existence
    // cleanly separates both survivors (worktree present → fall through) from
    // the genuinely-finished run (worktree gone → refuse), and it also catches
    // a terminal run torn down WITHOUT an explicit merge (a genuine autonomous
    // failure), which the marker check would have missed. Branch on
    // `manifest.status` (invariant #4), never `lifecycle`.
    //
    // A `Cancelled` run is refused regardless of its worktree, and even under
    // `--dry-run` (so a preview never claims a merge is planned for a run that
    // would hard-fail). The load-bearing reason is the reducer's adoption
    // whitelist — a late `explicit-merge` report is adopted only against a
    // `Failed | Done` node (see `reduce_node_report` / `reducer-adopt-explicit-merge`),
    // never `Cancelled` — so merging a cancelled run would land git state the
    // run state can never reflect, then strand teardown. Refuse instead.
    if manifest.status == taskfleet_core::Status::Cancelled {
        return Err(CliError::user(
            "run_already_terminal",
            format!(
                "run {run_id} is cancelled — merging is not permitted; a cancelled run \
                 never adopts a merge, so `run merge` would do nothing."
            ),
        )
        .with_invalid_value(&run_id));
    }

    // `--dry-run` is a read-only preview that never spawns merge.sh, so it is
    // exempt from the remaining worktree-existence checks.
    if !args.dry_run {
        // try_exists returns Ok(false) only for a definitely-absent path; an
        // Err (e.g. a permission error mid-path) is "cannot tell", and
        // `unwrap_or(true)` treats that as present so we fall through to a real
        // merge attempt (which surfaces the true error) rather than a false
        // "already terminal".
        let worktree_gone = !Path::new(worktree_path).try_exists().unwrap_or(true);
        if worktree_gone {
            // The top-of-function manifest read is unlocked and may predate a
            // supervisor rollup that removed this very worktree. Re-read the
            // status fresh under the shared lock so the terminal-vs-live
            // classification (and thus the error) is correct even under that
            // race (invariant #3 for the read).
            let status = RunLock::with_shared_lock(&paths.lock(), || {
                Ok(read_manifest_opt(&paths)?.map(|m| m.status))
            })
            .map_err(from_core)?
            .unwrap_or(manifest.status);

            if status.is_terminal() {
                let verb = if status == taskfleet_core::Status::Done {
                    "already done"
                } else {
                    "failed"
                };
                return Err(CliError::user(
                    "run_already_terminal",
                    format!(
                        "run {run_id} is {verb} and its worktree has been torn down — there is \
                         no worktree left to merge. If teardown looks incomplete (tmux window \
                         still open, branch still present), run `taskfleet run reattach \
                         {run_id}` to finish it."
                    ),
                )
                .with_invalid_value(&run_id));
            }
            // A still-live run whose worktree has vanished is a distinct,
            // actionable state — surface it plainly instead of the misleading
            // merge.sh spawn failure. If the run actually finished but its
            // supervisor has not rolled the manifest up yet, `run reattach`
            // completes the transition.
            return Err(CliError::user(
                "worktree_missing",
                format!(
                    "worktree {worktree_path} does not exist — cannot merge. If the run has \
                     finished, its supervisor may not have rolled up yet; run \
                     `taskfleet run reattach {run_id}`. Otherwise the worktree was \
                     removed out from under a live run."
                ),
            )
            .with_invalid_value(&run_id));
        }
    }

    // The 0.2 cut removed the `code` kind — the only interactive, human-reviewed
    // topology — so every surviving run is autonomous and self-merges. There is
    // no human-review merge gate left to enforce here.

    // Resolve the merge target: explicit `--source` wins, else the
    // manifest's source_branch (the integration branch for an orchestrated
    // child, `main` for a code worktree), else None → merge.sh detects
    // main/master itself.
    let effective_source = source.clone().or_else(|| manifest.source_branch.clone());

    // Build the terminal report up front — BEFORE the merge — so a malformed
    // `--report-file` is rejected without having already merged. The report
    // is only submitted after a clean merge; here we just validate its shape
    // and stamp the `via: "explicit-merge"` marker.
    let (mut report, advisory_warnings) = build_report(
        args.report_file.as_deref(),
        branch,
        effective_source.as_deref(),
    )?;

    // Fold the dropped-advisory-field warnings (if any) into the caller's base
    // warnings up front, so BOTH the dry-run preview and the real merge surface
    // them. `--dry-run` doubles as a report-file preflight (issue
    // `merge-report-schema-lenience` option 3): an agent can see exactly which
    // advisory entries would be dropped before committing to the merge.
    let base_warnings: Vec<String> = args
        .warnings
        .iter()
        .cloned()
        .chain(advisory_warnings.iter().map(AdvisoryWarning::to_message))
        .collect();

    if args.dry_run {
        return Ok(MergeOutcome {
            run_id: run_id.clone(),
            node_id: node_id.as_str().to_string(),
            branch: branch.to_string(),
            source: effective_source.clone(),
            merged: false,
            report_seq: None,
            supervisor: None,
            dry_run: true,
            warnings: base_warnings,
            report_advisory_warnings: advisory_warnings,
        });
    }

    let git = git_bin();

    // Recover a prior CRASHED merge transaction for this node before starting a
    // fresh merge (design.md §2.1b / A2, issue `merge-transaction-recovery`). If a
    // previous `run merge` recorded `merge.started`, mutated git, then crashed
    // before appending its terminal report, the worker's work already landed in
    // source — complete that transaction here instead of re-merging (a re-merge of
    // already-merged work would fail merge.sh's "refusing to merge into itself" /
    // "0 commits" checks). A rejected/unverifiable prior transaction is cleared and
    // we fall through to a fresh merge. Cheap when nothing is pending.
    match merge_recovery::recover_node(&paths, &node_id, &git) {
        merge_recovery::Recovery::Completed => {
            // The crashed merge is confirmed landed and the node is now terminal.
            // Ensure a teardown actor and report success without re-running the merge.
            let (outcome, warnings) = ensure_report_consumer(&paths, &run_id, &base_warnings);
            return Ok(MergeOutcome {
                run_id: run_id.clone(),
                node_id: node_id.as_str().to_string(),
                branch: branch.to_string(),
                source: effective_source.clone(),
                merged: true,
                report_seq: None,
                supervisor: Some(outcome),
                dry_run: false,
                warnings,
                report_advisory_warnings: advisory_warnings.clone(),
            });
        }
        // A DIFFERENT `run merge` is actively driving a transaction for this node.
        // Starting a fresh merge would `merge.started`-overwrite its `pending_merge`
        // and race it (/llm-review finding) — refuse with a retryable error instead.
        merge_recovery::Recovery::DriverAlive => {
            return Err(CliError::user(
                "merge_in_progress",
                format!(
                    "another `run merge` is already driving a merge for node {node_id}; \
                     retry once it finishes"
                ),
            )
            .with_invalid_value(&run_id));
        }
        // A prior transaction is pending but git could not be consulted to resolve
        // it. Overwriting it with a fresh merge could strand a genuinely-landed
        // merge, so refuse rather than clobber unverifiable recovery state.
        merge_recovery::Recovery::CannotVerify => {
            return Err(CliError::system(
                "merge_recovery_unverifiable",
                format!(
                    "a prior merge transaction for node {node_id} could not be verified via git; \
                     refusing to start a new merge over it — resolve the worktree/source repo and retry"
                ),
            ));
        }
        // Rejected (prior transaction cleared), Superseded, or NothingPending: safe
        // to proceed to a fresh merge.
        merge_recovery::Recovery::Rejected { .. }
        | merge_recovery::Recovery::Superseded
        | merge_recovery::Recovery::NothingPending => {}
    }

    // Record the merge transaction BEFORE mutating git (design.md §2.1b / A2), so a
    // crash after the git merge but before the terminal report can be recovered
    // deterministically by OID. Best-effort: if the source ref can't be read (e.g. a
    // stubbed test git, or no concrete source branch), the transaction is skipped
    // and the merge proceeds exactly as before — no recovery protection, no behavior
    // change.
    let merge_start = record_merge_start(
        &paths,
        &node_id,
        worktree_path,
        branch,
        &node,
        effective_source.as_deref(),
        &git,
    )?;

    // Run the merge. A non-zero exit (conflict, dirty tree, lock timeout)
    // surfaces as a CliError and the report is NOT submitted — the node
    // stays live for the agent to recover and retry. The source ref mutation is
    // guarded by the recorded `expected_source_oid` (compare-and-swap): merge.sh
    // refuses if the source branch moved since we recorded the transaction.
    if let Err(e) = run_merge_sh(
        Path::new(worktree_path),
        branch,
        effective_source.as_deref(),
        merge_start.as_ref().map(|h| h.expected_source_oid.as_str()),
    ) {
        // The merge did not complete (conflict, dirty tree, CAS mismatch, lock
        // timeout). Clear the transaction we opened so a dangling `merge.started`
        // does not later trip recovery — the work was NOT merged, and the node
        // stays live for the agent to resolve and retry.
        if let Some(h) = &merge_start {
            abort_merge_start(&paths, &node_id, &h.op_id, "merge did not complete");
        }
        return Err(e);
    }

    // Merge succeeded: submit the terminal report (built above, stamped with
    // `via: "explicit-merge"`) so the supervisor's cleanup gate extends
    // teardown to interactive kinds and any rich `discussion_items` /
    // `spinoff_proposals` reach the parent.
    //
    // Idempotent: a retried `run merge` (e.g. the report append failed but
    // the merge already landed) re-uses the same key and returns the prior
    // seq instead of double-appending. The merge itself is also a clean
    // no-op on retry (the branch is already merged, worktree may be gone).
    // Stamp the typed report origin (issue `typed-report-origin`): `run merge` is
    // the sole authority for a `RunMerge` origin, carrying the transaction's
    // immutable OIDs for provenance when a transaction was recorded (absent only on
    // the legacy unguarded path — no concrete source branch / stubbed git). This is
    // parallel to the `via` marker `build_report` already stamped; the origin is the
    // higher-fidelity signal `supervise::outcome::classify` now prefers.
    taskfleet_core::ReportOrigin::RunMerge {
        op_id: merge_start.as_ref().map(|h| h.op_id.clone()),
        worker_oid: merge_start.as_ref().map(|h| h.worker_oid.clone()),
    }
    .stamp(&mut report);

    let idem_key = format!("explicit-merge:{run_id}:{node_id}");
    let result = append_and_apply_event(
        &paths,
        "node.report",
        Some(&node_id),
        Some(&idem_key),
        report,
    )
    .map_err(from_core)?;

    // The terminal report is only useful if a supervisor consumes it: the
    // supervisor is the canonical teardown actor (close tmux window, remove
    // worktree, delete branch) AND the roller-up of `manifest.status` (state
    // integrity invariant #5). `run merge` no longer tears anything down itself:
    // the taskfleet-core reducer now ADOPTS a late `via: "explicit-merge"` report even
    // against a node an earlier watchdog `agent-died` false positive already
    // terminalized (issue `reducer-adopt-explicit-merge`). So `last_report` carries
    // the merge marker, `any_node_merged_explicitly` sees it, and the supervisor
    // warrants teardown on every path — restoring single-owner teardown and
    // retiring the inline reclaim of `merge-skips-teardown` /
    // `agent-died-merge-no-teardown-interactive`.
    //
    // The one path where no live supervisor exists is exactly that swallowed case:
    // the watchdog terminal already rolled the run up and its supervisor exited, so
    // there is nothing running to consume our now-adopted report. `ensure_report_
    // consumer` reattaches one — including on a terminal-but-teardown-warranted run
    // — so the reattached supervisor runs the same `cleanup_terminal_nodes` and
    // exits. A bare `merged: true` never returns while teardown has no actor.
    //
    // `result` (seq / idempotent_replay / applied) is intentionally NOT used to
    // gate the reattach: the reattach is driven by durable projection state
    // (`manifest.status` + the adopted merge marker), read fresh under the shared
    // lock, so an idempotent `run merge` retry after a crash between append and
    // reattach still reattaches and completes teardown (the reducer's adoption is
    // durable even though this call's `applied` is false). See the 4-model review
    // (`reducer-adopt-explicit-merge`) — gating on `applied` here was a crash-retry
    // leak. (`result.seq` is still surfaced in the envelope below.)
    let (outcome, warnings) = ensure_report_consumer(&paths, &run_id, &base_warnings);

    Ok(MergeOutcome {
        run_id: run_id.clone(),
        node_id: node_id.as_str().to_string(),
        branch: branch.to_string(),
        source: effective_source.clone(),
        merged: true,
        report_seq: Some(result.seq),
        supervisor: Some(outcome),
        dry_run: false,
        warnings,
        report_advisory_warnings: advisory_warnings,
    })
}

/// Guarantee the terminal `node.report` just appended has a live consumer, or
/// surface why not — never silent success. Returns the machine-readable
/// [`ConsumerOutcome`] plus the caller's `base` warnings with (at most) one
/// human-readable entry appended.
///
/// The recovery decision reasons across FOUR facts, read together under one
/// shared lock (state-integrity invariant #3): the manifest status, whether
/// teardown is warranted (`any_node_merged_explicitly`, since a `run merge`
/// always makes teardown warranted once its report is adopted), the
/// `supervisor.pid` liveness, and whether a supervisor was EVER started. The
/// discriminator for "reattach or not" is deliberately NOT "is a dead pid file
/// present" — an orphan can also have NO pid file (the `claim_pid_atomic` hint
/// tells an operator to delete a stale `supervisor.pid`, after which a merge
/// would otherwise silently strand the run). The sound rule is:
///
///   reattach  ⟺  no live supervisor  ∧  ever supervised
///                ∧  ( run not yet terminal  ∨  (terminal ∧ warranted) )
///
/// where "ever supervised" = a recorded pid (stale file) OR a `supervisor.started`
/// event in the log, and "warranted" = the adopted merge (or an autonomous kind)
/// means the supervisor will tear the worktree/branch/window down. The terminal
/// clause is what closes the swallowed-report leak (issues `merge-skips-teardown`,
/// `agent-died-merge-no-teardown-interactive`): a watchdog `agent-died` false
/// positive rolled the run up and its supervisor exited BEFORE the merge, so
/// reattaching one is the ONLY way teardown runs — and the reattached supervisor,
/// seeing the now-adopted `via: "explicit-merge"` report, warrants it.
///
/// The reattach is driven purely by DURABLE state (`manifest.status` + the adopted
/// merge marker), never by whether THIS call freshly adopted the report. An
/// earlier revision gated it on `AppendResult::applied`, but the 4-model review of
/// `reducer-adopt-explicit-merge` showed that leaks: if `run merge` adopts, then
/// crashes before reattaching, the retried merge is an idempotent replay
/// (`applied == false`) and would skip the reattach, stranding the worktree
/// forever. Reattaching whenever teardown is warranted-but-unmanned is safe
/// because the supervisor's cleanup is idempotent — a redundant reattach on an
/// already-torn-down run boots, finds nothing to remove, and exits — and
/// `spawn_supervisor`'s `supervisor_already_running` guard prevents a double
/// reattach race.
///
/// A never-materialized skeleton run (e.g. a `--skip-materialize` test run) has
/// no recorded pid and no `supervisor.started`, so it is left untouched
/// (`NotSupervised`) — spawning a supervisor for it would be wrong. This is never
/// a production worktree run: `run create` spawns + confirms a supervisor before
/// returning, so a real run always reads `ever supervised`.
///
/// KNOWN RESIDUAL: a legacy bare-integer `supervisor.pid` whose pid has been
/// recycled by an unrelated live process reads as `alive` (§7.6 identity check
/// cannot fire without a recorded start-time), so this returns `Alive` and skips
/// reattach. Modern pid files (the norm) carry a start-time and are immune.
fn ensure_report_consumer(
    paths: &taskfleet_core::RunPaths,
    run_id: &str,
    base: &[String],
) -> (ConsumerOutcome, Vec<String>) {
    // One shared-locked read of manifest.json + nodes/* + supervisor.pid +
    // events.jsonl: the reattach decision is a multi-projection read, so it must
    // not observe a half-applied set (invariant #3). The event scan is
    // short-circuited when a pid file is already present, so the common
    // (signal-death) path pays only the manifest read + node scan + pid probe.
    let probed = RunLock::with_shared_lock(&paths.lock(), || {
        let manifest = read_manifest_opt(paths)?;
        let terminal = manifest.as_ref().is_some_and(|m| m.status.is_terminal());
        // Teardown is warranted for an autonomous kind unconditionally, or for any
        // kind once an explicit `run merge` report has been adopted onto a node —
        // exactly the supervisor's own `cleanup` gate. Read under the same shared
        // lock as the manifest (the fold scans every node projection).
        let warranted = manifest
            .as_ref()
            .is_some_and(|m| m.kind.lifecycle() == taskfleet_core::Lifecycle::Autonomous)
            || cleanup::any_node_merged_explicitly(paths);
        let live = SupervisorView::probe(paths);
        let ever_supervised = live.pid.is_some()
            || read_all_events(&paths.events())?
                .iter()
                .any(|e| e.kind == "supervisor.started");
        Ok((terminal, warranted, live, ever_supervised))
    });

    let (terminal, warranted, live, ever_supervised) = match probed {
        Ok(v) => v,
        // A locked read failed (corrupt log / I/O). Don't guess the run's health
        // — the merge itself already landed; surface a deferred-teardown warning
        // so the caller can recover rather than assuming a clean close.
        Err(e) => {
            let mut warnings = base.to_vec();
            warnings.push(format!(
                "could not verify supervisor liveness after merge ({e}); if teardown \
                 does not complete, run `taskfleet run reattach {run_id}`"
            ));
            return (
                ConsumerOutcome::Deferred {
                    recovery_command: format!("taskfleet run reattach {run_id}"),
                },
                warnings,
            );
        }
    };

    // A live supervisor will consume the report on its next tick.
    if live.alive {
        return (ConsumerOutcome::Alive, base.to_vec());
    }
    // Never supervised (skeleton run): no teardown actor to restart.
    if !ever_supervised {
        return (ConsumerOutcome::NotSupervised, base.to_vec());
    }
    // Already rolled up by a supervisor that has since exited. When teardown is
    // NOT warranted (a terminal interactive run that ended without an explicit
    // merge — the human owns that window), there is nothing to tear down, so
    // report `Terminal`. But when teardown IS warranted (the adopted merge, or an
    // autonomous kind) and no supervisor is live, the exited supervisor never ran
    // it — fall through to reattach, the ONLY actor that will tear the worktree /
    // branch / window down (the swallowed-report path). Idempotent, so a retry
    // whose teardown already completed simply reattaches, no-ops, and exits.
    if terminal && !warranted {
        return (ConsumerOutcome::Terminal, base.to_vec());
    }

    // Orphaned: was supervised, no live supervisor remains to consume the
    // terminal report — either the run is still non-terminal, or it is terminal
    // but this call freshly adopted a merge that warrants teardown the exited
    // supervisor never saw. Restore the invariant by reattaching.
    let who = live.pid.map_or_else(
        || "the supervisor".to_string(),
        |p| format!("supervisor (pid {p})"),
    );
    let mut warnings = base.to_vec();
    let outcome = match reattach::spawn_supervisor(paths, run_id, false, None) {
        Ok(0) => {
            warnings.push(format!(
                "{who} was not running; restarted it to consume the terminal report \
                 and complete teardown (new pid not yet confirmed — check \
                 `taskfleet run show {run_id}`)"
            ));
            ConsumerOutcome::Reattached { pid: None }
        }
        Ok(new_pid) => {
            warnings.push(format!(
                "{who} was not running; restarted it (pid {new_pid}) to consume the \
                 terminal report and complete teardown"
            ));
            ConsumerOutcome::Reattached { pid: Some(new_pid) }
        }
        // A live supervisor appeared between the probe and the spawn attempt:
        // there is a consumer after all, so this is not a warning condition.
        Err(e) if e.code == "supervisor_already_running" => ConsumerOutcome::Alive,
        Err(e) => {
            let recovery_command = format!("taskfleet run reattach {run_id}");
            warnings.push(format!(
                "{who} is not running and auto-reattach failed ({}); teardown (tmux \
                 window, worktree, branch) is deferred — run `{recovery_command}` to \
                 complete it",
                e.message
            ));
            ConsumerOutcome::Deferred { recovery_command }
        }
    };
    (outcome, warnings)
}

/// A recorded, in-flight merge transaction's identity — what `run merge` needs
/// to CAS-guard the merge and to abort the transaction if the merge fails.
struct MergeStartHandle {
    /// The transaction's unique id, for a targeted `merge.aborted` on failure.
    op_id: String,
    /// The source ref OID recorded before the merge — the compare half of the
    /// compare-and-swap, forwarded to merge.sh.
    expected_source_oid: String,
    /// The worker tip OID the merge integrates — stamped into the terminal
    /// report's typed `RunMerge` origin for provenance (issue `typed-report-origin`).
    worker_oid: String,
}

/// Record the merge transaction (`merge.started`) BEFORE the git mutation, so a
/// crash between the git merge and the terminal report can be recovered
/// deterministically by OID (design.md §2.1b / A2, issue
/// `merge-transaction-recovery`).
///
/// Returns `Ok(None)` — and the merge proceeds exactly as before A2, without CAS
/// or recovery protection — ONLY in the genuinely-unrecoverable cases: no concrete
/// source branch to compare against (merge.sh's main/master auto-detect path), or
/// git cannot resolve the source/worker OIDs (a stubbed test git, a torn-down
/// repo). In those cases the merge itself would need the same git anyway, so a
/// missing transaction reflects a repo that recovery could not act on regardless.
///
/// A durable-append FAILURE (lock/IO) is NOT downgraded: it returns `Err`, failing
/// the merge BEFORE any git mutation (/llm-review finding). If the event log cannot
/// record the transaction, proceeding would reintroduce the exact false-failure this
/// change eliminates (a crash after the git merge with no recorded transaction to
/// recover from). Fail closed so the agent backs off and retries.
fn record_merge_start(
    paths: &taskfleet_core::RunPaths,
    node_id: &NodeId,
    worktree_path: &str,
    worker_branch: &str,
    node: &Node,
    source_branch: Option<&str>,
    git: &str,
) -> Result<Option<MergeStartHandle>, CliError> {
    // A concrete source branch is required: it is the ref recovery reads and the
    // CAS compares. Without it (merge.sh's main/master auto-detect path) we cannot
    // record a recoverable transaction, so fall back to the legacy unguarded merge.
    let Some(source_branch) = source_branch else {
        return Ok(None);
    };
    let Some(expected_source_oid) = merge_recovery::read_oid(git, worktree_path, source_branch)
    else {
        return Ok(None);
    };
    // The worker's tip: prefer the recorded branch, fall back to HEAD.
    let Some(worker_oid) = merge_recovery::read_oid(git, worktree_path, worker_branch)
        .or_else(|| merge_recovery::read_oid(git, worktree_path, "HEAD"))
    else {
        return Ok(None);
    };
    let pid = std::process::id();
    let txn = MergeTxn {
        op_id: taskfleet_core::new_op_id(),
        source_branch: source_branch.to_string(),
        worker_branch: worker_branch.to_string(),
        expected_source_oid: expected_source_oid.clone(),
        worker_oid: worker_oid.clone(),
        base_sha: node.base_sha.clone(),
        driver_pid: Some(pid as i32),
        driver_pid_start_secs: crate::supervise::watchdog::pid_start_time(pid),
        started_at: Utc::now(),
    };
    let data = serde_json::to_value(&txn)
        .map_err(|e| CliError::system("merge_txn_serialize_failed", e.to_string()))?;
    append_and_apply_event(
        paths,
        taskfleet_core::KIND_MERGE_STARTED,
        Some(node_id),
        None,
        data,
    )
    .map_err(|e| {
        CliError::system(
            "merge_txn_record_failed",
            format!(
                "could not durably record the merge transaction before mutating git ({e}); \
                 refusing to merge unguarded — retry"
            ),
        )
    })?;
    Ok(Some(MergeStartHandle {
        op_id: txn.op_id,
        expected_source_oid,
        worker_oid,
    }))
}

/// Clear a merge transaction we opened when the merge itself fails (conflict,
/// dirty tree, CAS mismatch, lock timeout), so a dangling `merge.started` does not
/// later trip recovery. Best-effort: a failure here is only cosmetic — the next
/// recovery tick would reject the still-pending transaction anyway (the source ref
/// is unchanged), so we log and move on.
fn abort_merge_start(
    paths: &taskfleet_core::RunPaths,
    node_id: &NodeId,
    op_id: &str,
    reason: &str,
) {
    let data = json!({ "op_id": op_id, "reason": reason });
    if let Err(e) = append_and_apply_event(
        paths,
        taskfleet_core::KIND_MERGE_ABORTED,
        Some(node_id),
        Some(&format!(
            "merge-aborted:{}:{node_id}:{op_id}",
            paths.run_id.as_str()
        )),
        data,
    ) {
        tracing::warn!(
            target: "taskfleet::merge",
            error = %e,
            "could not record merge.aborted after a failed merge; recovery will clear it"
        );
    }
}

/// The branch a node works on. Prefers the explicit `branch` field; a
/// well-formed worktree node always has it.
fn branch_for(node: &Node) -> Option<&str> {
    node.branch.as_deref().filter(|s| !s.is_empty())
}

/// Build (and validate) the terminal §7.3 report to submit on a clean merge.
///
/// With `report_file`, read the agent's payload (so an autonomous kind can
/// carry its `discussion_items` / `spinoff_proposals` /
/// `wrap_up_recommendations` in the same call) and stamp it with the
/// `via: "explicit-merge"` marker, overriding any caller-set `via`. Without
/// one, synthesize a minimal `{success, summary, via}` report — enough for a
/// simple spinoff.
///
/// Validation is **lenient on advisory fields** (issue
/// `merge-report-schema-lenience`): `success` and the `cancelled`/`reason`
/// constraints stay strict (a violation returns `schema_violation` and blocks the
/// merge), but a malformed *advisory* entry (`discussion_items` /
/// `spinoff_proposals` / `wrap_up_recommendations` / `summary`) is dropped from
/// the returned report and reported as an [`AdvisoryWarning`] — never a merge
/// blocker. Returns the sanitized report plus the drops.
fn build_report(
    report_file: Option<&Path>,
    branch: &str,
    source: Option<&str>,
) -> Result<(Value, Vec<AdvisoryWarning>), CliError> {
    let mut report = if let Some(path) = report_file {
        read_report_file(path)?
    } else {
        let summary = source.map_or_else(
            || format!("merged {branch} via run merge"),
            |src| format!("merged {branch} into {src} via run merge"),
        );
        json!({ "success": true, "summary": summary })
    };
    // `run merge` owns the marker: stamp it regardless of what the file held.
    let obj = report.as_object_mut().ok_or_else(|| {
        CliError::user(
            "report_not_object",
            "--report-file payload must be a JSON object",
        )
    })?;
    // A clean merge is by definition a SUCCESS. Reject (rather than silently
    // rewrite) a `--report-file` that claims `success: false` or `cancelled: true`:
    // such a report contradicts the merge that just landed, and — stamped with the
    // explicit-merge marker — it would either mis-terminalize a live node as
    // Failed/Cancelled or, against an already-terminal node, fail the reducer's
    // confirmed-merge adoption gate and strand teardown (4-model review of
    // `reducer-adopt-explicit-merge`). Silently changing the caller's outcome
    // fields would be worse than refusing, so refuse.
    if obj
        .get("success")
        .is_some_and(|v| v.as_bool() != Some(true))
        || obj
            .get("cancelled")
            .is_some_and(|v| v.as_bool() == Some(true))
    {
        return Err(CliError::user(
            "invalid_merge_report",
            "--report-file for `run merge` must set `success: true` (a merge is a success) \
             and must not set `cancelled: true`",
        ));
    }
    obj.insert(
        "via".to_string(),
        Value::String(taskfleet_core::VIA_EXPLICIT_MERGE.to_string()),
    );

    // Lenient advisory validation (issue `merge-report-schema-lenience`): a typo
    // in an *advisory* section (`discussion_items` / `spinoff_proposals` /
    // `wrap_up_recommendations` / `summary`) must not reject the terminal report
    // and block a clean, already-committed code merge. Required and
    // correctness-bearing fields (`success`, `cancelled`/`reason`) are still
    // strict — a violation there returns `schema_violation` and no merge happens.
    // Malformed advisory entries are dropped from the report the reducer projects
    // and returned as machine-readable warnings the caller surfaces.
    // A REQUIRED-field violation still errors; map it through the shared boundary
    // translator so the structured `expected` hint (e.g. `{field:"success",
    // type:"boolean"}`) survives to the envelope, exactly as `node report` does.
    let sanitized = sanitize_report_advisory(&report)
        .map_err(crate::node::report::map_report_validation_error)?;
    Ok((sanitized.report, sanitized.warnings))
}

/// Read and JSON-parse a `--report-file`, enforcing the size cap during the
/// read (TOCTOU-safe, mirroring `node report`'s `read_capped`).
fn read_report_file(path: &Path) -> Result<Value, CliError> {
    let mut f = std::fs::File::open(path).map_err(|e| {
        CliError::user(
            "report_file_unreadable",
            format!("open {}: {}", path.display(), e),
        )
        .with_invalid_value(path.display().to_string())
    })?;
    let mut buf = Vec::new();
    std::io::Read::by_ref(&mut f)
        .take(MAX_REPORT_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| {
            CliError::user(
                "report_file_unreadable",
                format!("read {}: {}", path.display(), e),
            )
            .with_invalid_value(path.display().to_string())
        })?;
    if buf.len() as u64 > MAX_REPORT_BYTES {
        return Err(CliError::user(
            "report_file_too_large",
            format!("--report-file exceeds maximum of {MAX_REPORT_BYTES} bytes"),
        )
        .with_invalid_value(path.display().to_string()));
    }
    serde_json::from_slice(&buf).map_err(|e| {
        CliError::user(
            "report_file_invalid_json",
            format!("parse {}: {}", path.display(), e),
        )
        .with_invalid_value(path.display().to_string())
    })
}

/// Resolve the merge backend: `TASKFLEET_MERGE_SH` override (tests) or the
/// embedded script materialized to a temp file with the exec bit set.
/// Returns the temp-file guard so it lives until the command has run.
fn materialize_merge_sh() -> Result<MergeScript, CliError> {
    if let Ok(path) = std::env::var("TASKFLEET_MERGE_SH") {
        return Ok(MergeScript::External(path.into()));
    }
    let mut tmp = tempfile::Builder::new()
        .prefix("taskfleet-merge-")
        .suffix(".sh")
        .tempfile()
        .map_err(|e| {
            CliError::system("tempfile_failed", format!("create merge.sh tempfile: {e}"))
        })?;
    tmp.write_all(MERGE_SH.as_bytes())
        .map_err(|e| CliError::system("write_failed", format!("write merge.sh tempfile: {e}")))?;
    tmp.flush()
        .map_err(|e| CliError::system("write_failed", format!("flush merge.sh tempfile: {e}")))?;
    let perms = std::fs::Permissions::from_mode(0o700);
    std::fs::set_permissions(tmp.path(), perms)
        .map_err(|e| CliError::system("chmod_failed", format!("chmod merge.sh tempfile: {e}")))?;

    // Linux refuses to execute a script whose inode is still open for writing
    // (`ETXTBSY`). Converting to `TempPath` closes the writable descriptor while
    // retaining ownership of the private pathname, so it remains present for the
    // child invocation and is removed automatically afterward.
    Ok(MergeScript::Temp(tmp.into_temp_path()))
}

/// Where the materialized merge backend lives — an external override path
/// or an owned temp path that must outlive the command invocation. The temp
/// path deliberately owns no open file descriptor: Linux rejects exec of a
/// write-open script with `ETXTBSY`.
enum MergeScript {
    External(std::path::PathBuf),
    Temp(tempfile::TempPath),
}

impl MergeScript {
    fn path(&self) -> &Path {
        match self {
            MergeScript::External(p) => p.as_path(),
            MergeScript::Temp(t) => t.as_ref(),
        }
    }
}

/// Invoke the merge backend from inside `worktree_path`, inheriting the
/// environment (notably `$TMUX`/`$TMUX_PANE`, which the backend uses to
/// close the agent's window). On a non-zero exit, the captured stderr
/// becomes the error message and the report is skipped by the caller.
fn run_merge_sh(
    worktree_path: &Path,
    branch: &str,
    source: Option<&str>,
    expected_source_oid: Option<&str>,
) -> Result<(), CliError> {
    let script = materialize_merge_sh()?;
    let mut cmd = Command::new(script.path());
    cmd.current_dir(worktree_path);
    if let Some(src) = source {
        cmd.arg("--target").arg(src);
    }
    // Compare-and-swap guard (design.md §2.1b / A2): merge.sh verifies the target
    // branch is still at this OID after acquiring the merge lock, and refuses the
    // FF if it moved — so the source ref mutation only lands when the compare half
    // still holds.
    if let Some(oid) = expected_source_oid {
        cmd.arg("--expected-source-oid").arg(oid);
    }
    // Orphan-race guard (/llm-review finding): merge.sh is our child but survives if
    // we (the driver) are killed. Pass our PID so merge.sh re-checks our liveness
    // immediately before the source-ref mutation and aborts if we died — otherwise a
    // recovery run, seeing the driver dead but the orphaned merge.sh still about to
    // fast-forward, could reject a merge that then lands, stranding the work.
    cmd.arg("--driver-pid").arg(std::process::id().to_string());
    cmd.arg(branch);

    let output = cmd.output().map_err(|e| {
        // The pre-flight guard in `run` refuses a torn-down worktree up front,
        // but the worktree can still vanish in the TOCTOU window between that
        // check and this spawn (the supervisor tearing a just-terminalized run
        // down). A `NotFound` here can be the missing `current_dir` OR a missing
        // executable (e.g. an `TASKFLEET_MERGE_SH` override pointing nowhere, or a
        // missing shebang interpreter), so disambiguate by stat-ing the worktree
        // instead of blindly blaming either one.
        if e.kind() == std::io::ErrorKind::NotFound
            && worktree_path.try_exists().is_ok_and(|exists| !exists)
        {
            CliError::user(
                "worktree_missing",
                format!(
                    "worktree {} no longer exists — it was likely torn down as the run \
                     finished; no merge is needed",
                    worktree_path.display()
                ),
            )
        } else {
            CliError::system(
                "merge_spawn_failed",
                format!("invoke merge.sh ({}): {}", script.path().display(), e),
            )
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        // merge.sh refuses on preconditions (on main, dirty tree, same
        // branch, lock timeout) and fails on a rebase conflict from
        // `workmux merge`. Both are user-actionable: the agent recovers
        // (commit / resolve / `/complex-rebase`) and retries `run merge`.
        let detail = stderr.trim();
        let detail = if detail.is_empty() {
            stdout.trim()
        } else {
            detail
        };
        let exit_code = output.status.code().unwrap_or(-1);
        // merge.sh reserves EX_TEMPFAIL for "could not acquire the merge lock in
        // time" — a concurrent self-merge into the SAME target branch held the
        // serializing lock past the timeout (issue concurrent-self-merge-race).
        // Surface it under a distinct `merge_in_progress` code so a caller can
        // tell a transient serialization conflict (retry) apart from a genuine
        // dirty tree / conflict (commit / resolve). It is retryable, not a hard
        // failure. See MERGE_SH_LOCK_TIMEOUT_EXIT for why 75 is unambiguous.
        let code = if exit_code == MERGE_SH_LOCK_TIMEOUT_EXIT {
            "merge_in_progress"
        } else if exit_code == MERGE_SH_CAS_MISMATCH_EXIT {
            "merge_source_moved"
        } else {
            "merge_failed"
        };
        return Err(CliError {
            kind: ExitKind::User,
            code: code.to_string(),
            message: format!("merge.sh exited {exit_code} merging {branch}: {detail}"),
            invalid_value: Some(branch.to_string()),
            expected: None,
            details: None,
        });
    }
    Ok(())
}

fn emit(
    payload: &MergePayload<'_>,
    spec: &OutputSpec,
    warnings: &[String],
) -> Result<(), CliError> {
    match spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => {
            output::emit_envelope(payload, spec, warnings)?;
        }
        OutputFormat::Text => {
            println!("run-id:     {}", payload.run_id);
            println!("node-id:    {}", payload.node_id);
            println!("branch:     {}", payload.branch);
            match payload.source {
                Some(s) => println!("source:     {s}"),
                None => println!("source:     (auto-detect main/master)"),
            }
            if payload.dry_run == Some(true) {
                println!("note:       --dry-run (no merge, no report)");
            } else {
                println!("merged:     {}", payload.merged);
                if let Some(seq) = payload.report_seq {
                    println!("report_seq: {seq}");
                }
            }
            output::emit_text_warnings(warnings);
        }
    }
    Ok(())
}
