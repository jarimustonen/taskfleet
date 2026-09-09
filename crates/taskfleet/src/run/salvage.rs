//! `run salvage` — the fenced manual resume/finish operation (design.md §2.2 /
//! A3, issue `run-salvage-command`).
//!
//! The thin supervisor deliberately drops the *automatic* rescue of a run that
//! finished-but-skipped-`run merge`, or whose worker wedged. Its replacement is
//! this human-invoked verb: the PO sees an `attention-required` run (design.md
//! §2.5, surfaced by `run list` / `run show` / `run wait`) or a `failed` run
//! whose branch the teardown gate preserved (invariant 5), and drives it to a
//! clean finish WITHOUT talking to the (possibly wedged, possibly already-dead)
//! agent and WITHOUT spawning a second writer into the same worktree.
//!
//! ## What it does (the fenced finish)
//!
//! 1. **Snapshot under the run lock** (invariant #3): read `manifest` + the
//!    single worker node `n-0001` in one shared-locked window, so the refusal
//!    decision never sees a half-applied projection set.
//! 2. **Refuse the cases it must not touch** (see [`run`]): an already-`done` or
//!    `cancelled` run (nothing to salvage / a deliberate teardown), a multi-node
//!    run (ambiguous — fan-out per-node salvage is a follow-up), a run with no
//!    preserved worktree/branch, and a live worker it cannot *safely* fence.
//! 3. **Verify the prior worker's identity, then fence it.** The worker is
//!    classified from durable told facts ([`WorkerState`]): an already-exited
//!    worker (the attention-required happy path) needs no fence; a confirmed-dead
//!    or recycled pid is gone; a *live* worker is fenced with `SIGTERM` — but
//!    ONLY when its recorded start-time identity positively matches, so salvage
//!    can never signal an unrelated process that recycled the pid. Fencing a live
//!    worker requires the explicit `--fence` opt-in (killing a process is
//!    destructive); without it a live worker is a refusal.
//! 4. **Drive `run merge` from the worktree's current git state.** Salvage does
//!    NOT hand-roll a raw git self-merge — that would bypass the
//!    merge-transaction record (invariant 6), the CAS-guarded source fast-forward,
//!    the `via: "explicit-merge"` terminal report, and the supervisor teardown
//!    gate. It delegates to the exact same machinery as `run merge`
//!    ([`crate::run::merge::execute`]), so the terminal report/provenance and
//!    every state-integrity invariant hold identically. Idempotent against a
//!    duplicate merge (the merge path's crash-recovery + idempotency key), so a
//!    re-run after a partial salvage completes cleanly.
//!
//! ## Scope (0.2)
//!
//! This ships the direct finish/merge path plus the explicit refusal modes. The
//! *fresh-agent continuation* variant (design.md §2.2 option (b) — launch one new
//! agent to continue the work instead of merging as-is) is deferred to the
//! follow-up `run-salvage-fresh` so the 0.2 mechanism stays small and
//! safe.
//!
//! ## Known residuals (0.2) — bounded, not closed
//!
//! The fence is a **best-effort process fence, not a durable writer lease**. The
//! clean writer-ownership answer (a durable, exclusive worktree lease revoked/
//! claimed under the run lock) is the same lease the design defers to **0.2.1**
//! (design.md §2.7, the pi.dev plugin). Until then, three windows are bounded but
//! not eliminated — acceptable for a single-user tool whose *primary* case
//! (attention-required) has no live worker at all:
//!
//! - **Parent-only fence.** `SIGTERM` targets the recorded `agent_pid`, not its
//!   process group, so an orphaned *child* of the worker (a `git`/formatter/test
//!   subprocess) could still be touching the worktree when the merge starts.
//!   Fixing this needs launcher-recorded process-group/session identity — tracked
//!   in `run-salvage-fresh`.
//! - **Non-atomic fence→merge.** Identity is re-verified immediately before the
//!   `SIGTERM` (closing the classify→signal recycle window to ~µs), but the fence
//!   and the delegated merge do not share one held lock; a worker respawned by an
//!   external `run reattach` between them is not re-detected. The merge itself is
//!   still crash-atomic and OID-CAS-guarded.
//! - **Concurrent salvage.** Two `run salvage` invocations are not mutually
//!   excluded before the merge; `merge.sh`'s file lock + the merge transaction
//!   serialize the actual git mutation (one wins, the other gets
//!   `merge_in_progress`/`merge_source_moved`), so no double-merge, but both may
//!   redundantly `SIGTERM` the same dying worker.

use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;

use taskfleet_core::{read_manifest_opt, read_node_opt, NodeId, RunLock, Status};

use crate::error::CliError;
use crate::output::{self, OutputFormat, OutputSpec};
use crate::run::merge::{self, ConsumerOutcome};
use crate::run::worker::{classify_worker, WorkerState};
use crate::run::{from_core, run_paths_from_cli_arg};
use crate::supervise::{pid_file, watchdog};

/// The single reporting node every salvageable (single-worker) run carries.
const DEFAULT_NODE_ID: &str = "n-0001";

/// How long to wait for a fenced worker to actually exit after `SIGTERM` before
/// giving up. A cooperating agent dies well within this; a process that ignores
/// `SIGTERM` past it is reported as an un-fenceable refusal rather than merged
/// out from under a still-live writer.
const FENCE_GRACE: Duration = Duration::from_secs(5);

/// Poll cadence while waiting for a fenced worker to exit.
const FENCE_POLL: Duration = Duration::from_millis(100);

pub struct Args<'a> {
    pub run_id: String,
    /// Override the merge target branch (forwarded to `run merge`). Defaults to
    /// the run's recorded `source_branch`.
    pub source: Option<String>,
    /// Optional §7.3 report payload (JSON file) to submit on the salvage merge,
    /// forwarded verbatim to `run merge` (stamped `via: "explicit-merge"`).
    pub report_file: Option<std::path::PathBuf>,
    /// Permit fencing a *live* worker: `SIGTERM` the recorded agent pid (only
    /// ever when its start-time identity matches). Without it, a live worker is a
    /// refusal — salvage never kills a process implicitly.
    pub fence: bool,
    /// Resolve inputs and report the planned salvage (worker state, whether a
    /// fence would fire, the planned merge) without fencing or merging anything.
    pub dry_run: bool,
    pub spec: &'a OutputSpec,
    pub warnings: &'a [String],
}

/// `SIGTERM` a verified-live worker and wait (bounded) for it to exit. Returns
/// `Ok(())` once the process is gone, or a `fence_failed` error if it survives
/// the grace (so salvage refuses rather than merge out from under a live writer).
///
/// `expected_start` is the worker's verified start-time. It is **re-checked
/// immediately before the signal**: a pid can be recycled in the window between
/// [`classify_worker`] and here, and signalling a recycled pid would hit an
/// unrelated process. If identity no longer holds (recycled, or the process is
/// already gone), the original worker is confirmed gone and there is nothing to
/// fence — return `Ok(())` and let the merge proceed on the preserved worktree.
fn fence_worker(pid: u32, expected_start: u64) -> Result<(), CliError> {
    let Some(pid_t) = pid_file::to_pid_t(pid) else {
        // Defensive range guard: `to_pid_t` rejects 0 / out-of-`pid_t`-range so a
        // corrupt pid can never cast to a negative (group/broadcast) kill target.
        return Err(CliError::system(
            "fence_failed",
            format!("worker pid {pid} is out of range; refusing to signal"),
        ));
    };
    // Re-verify identity right before the kill — closes the classify→signal
    // recycle window. A gone/recycled pid means the original is already dead.
    match watchdog::pid_start_time(pid) {
        Some(actual) if expected_start.abs_diff(actual) <= 1 => {}
        // Recycled (mismatch) or gone (None while previously alive): do NOT
        // signal an unrelated/absent process. The original worker is gone.
        _ => return Ok(()),
    }
    // SAFETY: `pid_t` is range-checked (never 0/negative → no group/broadcast
    // target), and SIGTERM is a routine cooperative-termination signal.
    let rc = unsafe { libc::kill(pid_t, libc::SIGTERM) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        // ESRCH: the worker exited between our identity check and the signal — a
        // benign race, the fence is already effectively done.
        if err.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        return Err(CliError::system(
            "fence_failed",
            format!("SIGTERM to worker pid {pid} failed: {err}"),
        ));
    }
    // Wait for the process to actually exit so the merge below never races a
    // still-live writer.
    let deadline = Instant::now() + FENCE_GRACE;
    while Instant::now() < deadline {
        if !pid_file::pid_alive(pid) {
            return Ok(());
        }
        std::thread::sleep(FENCE_POLL);
    }
    if pid_file::pid_alive(pid) {
        return Err(CliError::user(
            "fence_failed",
            format!(
                "worker pid {pid} did not exit within {}s of SIGTERM — refusing to \
                 merge over a live worker. Investigate the process (it may be blocked \
                 in an uninterruptible state) before retrying.",
                FENCE_GRACE.as_secs()
            ),
        ));
    }
    Ok(())
}

/// The salvage-merge sub-result surfaced in the payload — the machine-readable
/// half of the delegated `run merge`.
#[derive(Serialize)]
struct MergeSummary {
    branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    merged: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    report_seq: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    supervisor: Option<ConsumerOutcome>,
}

#[derive(Serialize)]
struct SalvagePayload {
    run_id: String,
    node_id: String,
    /// The classified prior-worker state (`exited` | `no-pid` | `gone` | `live` |
    /// `unverifiable`) at salvage time.
    worker_state: &'static str,
    /// Whether salvage fenced (SIGTERM'd) a live worker. `false` when the worker
    /// was already gone; under `--dry-run` this is what a real run *would* do.
    fenced: bool,
    /// The delegated `run merge` result.
    merge: MergeSummary,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    dry_run: bool,
}

pub fn run(args: Args<'_>) -> Result<(), CliError> {
    let root = crate::home::root_dir()?;
    let paths = run_paths_from_cli_arg(&root, &args.run_id)?;
    // `args.run_id` may have been an unambiguous prefix; report the resolved id.
    let run_id = paths.run_id.as_str().to_string();
    let node_id = NodeId::parse_str(DEFAULT_NODE_ID).expect("DEFAULT_NODE_ID is valid");

    // One shared-locked read of manifest + node (invariant #3): the refusal
    // decision reasons across both projections, so it must not observe a
    // half-applied set.
    let (manifest, node) = RunLock::with_shared_lock(&paths.lock(), || {
        let manifest = read_manifest_opt(&paths)?;
        let node = read_node_opt(&paths, &node_id)?;
        Ok((manifest, node))
    })
    .map_err(from_core)?;

    let manifest = manifest.ok_or_else(|| {
        CliError::user("run_not_found", format!("no run with id {run_id}"))
            .with_invalid_value(&run_id)
    })?;

    // A run recorded under a removed kind is read-only (ADR §D7) — refuse before
    // any fence/merge so we never rewrite its manifest / destroy its provenance.
    crate::run::reject_legacy_kind(manifest.kind, &run_id)?;

    // Refuse the terminal states there is nothing to salvage from.
    match manifest.status {
        // Already succeeded. Two shapes: the normal one (work merged, worktree
        // torn down → nothing to salvage) and the crash-retry one (a prior
        // salvage/merge appended the `explicit-merge` report and rolled the run to
        // Done, but crashed before the supervisor tore the worktree down). Branch
        // on worktree EXISTENCE, not status alone, mirroring `run merge` — a
        // surviving worktree means teardown is the outstanding work, which
        // `run reattach` completes (not a re-merge).
        Status::Done => {
            let worktree_present = node
                .as_ref()
                .and_then(|n| n.worktree_path.as_deref())
                .is_some_and(|p| Path::new(p).try_exists().unwrap_or(false));
            let hint = if worktree_present {
                format!(
                    " but its worktree still exists — teardown did not finish. Run \
                     `taskfleet run reattach {run_id}` to complete it (a re-merge is not \
                     needed; the work already landed)."
                )
            } else {
                " — its work merged and its worktree was torn down; there is nothing to salvage"
                    .to_string()
            };
            return Err(CliError::user(
                "run_already_terminal",
                format!("run {run_id} is already done{hint}"),
            )
            .with_invalid_value(&run_id));
        }
        // A deliberate `run cancel` teardown. The reducer never adopts a merge
        // against a cancelled node (see run merge), so a salvage merge would do
        // nothing but strand state. Cancel is final.
        Status::Cancelled => {
            return Err(CliError::user(
                "run_already_terminal",
                format!(
                    "run {run_id} was cancelled — a cancelled run never adopts a merge, so \
                     salvage cannot finish it"
                ),
            )
            .with_invalid_value(&run_id));
        }
        // Pending / Running (incl. attention-required) / Blocked / Failed with a
        // preserved worktree: salvageable.
        Status::Pending | Status::Running | Status::Blocked | Status::Failed => {}
    }

    // Single-worker only. A fan-out / multi-node run is ambiguous — which node's
    // worktree to fence and finish? Per-node salvage is the delegated follow-up
    // (`per-node-run`); refuse here rather than silently pick n-0001.
    if manifest.node_count != 1 {
        return Err(CliError::user(
            "ambiguous_multi_node",
            format!(
                "run {run_id} has {} nodes — salvage targets a single-worker run only. \
                 Per-node salvage of a fan-out is not yet supported.",
                manifest.node_count
            ),
        )
        .with_invalid_value(&run_id)
        .with_expected(serde_json::json!({ "node_count": 1 })));
    }

    let node = node.ok_or_else(|| {
        CliError::user(
            "node_not_found",
            format!("run {run_id} has no {node_id} node to salvage"),
        )
        .with_invalid_value(node_id.as_str())
    })?;

    // The preserved worktree + branch are what salvage finishes. Their absence is
    // a distinct, actionable refusal (a driver node, or work already torn down).
    let worktree_path = node.worktree_path.as_deref().ok_or_else(|| {
        CliError::user(
            "no_worktree",
            format!(
                "node {node_id} has no preserved worktree — nothing to salvage (a driver \
                 node, or the worktree was already removed)"
            ),
        )
        .with_invalid_value(node_id.as_str())
    })?;
    let has_branch = node.branch.as_deref().is_some_and(|s| !s.is_empty());
    if !has_branch {
        return Err(CliError::user(
            "no_branch",
            format!("node {node_id} has no preserved branch recorded; cannot salvage"),
        )
        .with_invalid_value(node_id.as_str()));
    }
    // The worktree must still be on disk for the merge to `cd` into it. A
    // definitely-absent path is a refusal; an "unknown" (permission) stat falls
    // through so the merge surfaces the true error.
    if !Path::new(worktree_path).try_exists().unwrap_or(true) {
        return Err(CliError::user(
            "worktree_missing",
            format!(
                "worktree {worktree_path} no longer exists — its work was likely already \
                 merged or torn down; there is nothing to salvage"
            ),
        )
        .with_invalid_value(&run_id));
    }

    // Classify + decide the fence. This is the whole safety gate.
    let worker = classify_worker(&node);

    // Refuse a never-started run: a `Pending` node with no recorded pid AND no
    // told exit (NoPid implies no `worker.exited` — see `classify_worker`) never
    // ran a worker, so there is no work to finish. Salvaging it would attempt to
    // merge an untouched worktree; refuse with a precise reason instead (review
    // consensus — `Pending` alone was too broad an eligibility gate).
    if manifest.status == Status::Pending && worker == WorkerState::NoPid {
        return Err(CliError::user(
            "run_not_started",
            format!(
                "run {run_id} is still pending and its worker never started (no recorded \
                 agent pid, no worker exit) — there is no work to salvage. Cancel it with \
                 `taskfleet run cancel {run_id}` if it is stuck."
            ),
        )
        .with_invalid_value(&run_id));
    }

    let needs_fence = match worker {
        WorkerState::Exited | WorkerState::NoPid | WorkerState::Gone => false,
        WorkerState::Unverifiable { pid } => {
            // Alive but unidentifiable — could be a recycled pid owned by an
            // unrelated process. NEVER fence it, even with --fence.
            return Err(CliError::user(
                "worker_unfenceable",
                format!(
                    "run {run_id}'s worker pid {pid} is alive but its identity cannot be \
                     verified (no recorded start-time) — refusing to signal a process that \
                     may have been recycled. Confirm the worker is gone, then retry."
                ),
            )
            .with_invalid_value(&run_id));
        }
        WorkerState::Live { pid, .. } => {
            if !args.fence {
                return Err(CliError::user(
                    "worker_live",
                    format!(
                        "run {run_id}'s original worker (pid {pid}) is still alive. Salvage \
                         will not kill a running worker implicitly — re-run with `--fence` to \
                         SIGTERM it and finish the run, or let it complete on its own."
                    ),
                )
                .with_invalid_value(&run_id));
            }
            true
        }
    };

    // PRE-FENCE VALIDATION (review consensus — never destroy before validating).
    // Run the merge in `--dry-run` first: it resolves `--source`, reads and
    // schema-validates `--report-file`, and refuses a cancelled/legacy run —
    // WITHOUT mutating anything. A malformed argument thus refuses here, BEFORE
    // any SIGTERM, instead of killing the worker and only then discovering the
    // merge cannot proceed. (Recovery-state refusals — `merge_in_progress` /
    // `merge_recovery_unverifiable` — are still checked inside the real merge, a
    // narrower post-fence residual.)
    let preview = merge::execute(&merge::Args {
        run_id: run_id.clone(),
        source: args.source.clone(),
        node_id: None,
        report_file: args.report_file.clone(),
        dry_run: true,
        spec: args.spec,
        warnings: args.warnings,
    })?;

    // Dry run: report the plan (worker state, whether a fence would fire, the
    // planned merge) without fencing or merging.
    if args.dry_run {
        let mut warnings = preview.warnings.clone();
        if let WorkerState::Live { pid, .. } = worker {
            warnings.push(format!(
                "would fence worker pid {pid} (SIGTERM) before finishing the run"
            ));
        }
        return emit(
            &SalvagePayload {
                run_id,
                node_id: node_id.as_str().to_string(),
                worker_state: worker.wire(),
                fenced: needs_fence,
                merge: merge_summary(&preview),
                dry_run: true,
            },
            args.spec,
            &warnings,
        );
    }

    // Fence the (verified-live) worker before merging so no second writer races
    // the worktree's git state.
    let mut base_warnings: Vec<String> = args.warnings.to_vec();
    let fenced = if needs_fence {
        if let WorkerState::Live { pid, start_time } = worker {
            fence_worker(pid, start_time)?;
            base_warnings.push(format!(
                "fenced worker pid {pid} (SIGTERM) before finishing the run"
            ));
        }
        true
    } else {
        false
    };

    // Drive the merge through the exact `run merge` machinery — crash-recovery,
    // CAS-guarded source FF, `via: "explicit-merge"` terminal report, supervisor
    // reattach — so provenance and every state-integrity invariant hold. Our
    // fence note is seeded as the base warnings, so the merge's
    // `ensure_report_consumer` appends to the same list the envelope emits.
    let mo = merge::execute(&merge::Args {
        run_id: run_id.clone(),
        source: args.source.clone(),
        node_id: None,
        report_file: args.report_file.clone(),
        dry_run: false,
        spec: args.spec,
        warnings: &base_warnings,
    })
    .map_err(|mut e| {
        // If the merge fails AFTER we fenced, the operator must know the worker
        // was already killed (the worktree is preserved — merge failures never
        // tear down). Without this the error reads as if salvage refused before
        // touching anything (review finding). Pre-fence merges (never happens on
        // this branch, but harmless) and non-fence paths add nothing.
        if fenced {
            e.message = format!(
                "{} (NOTE: the prior worker was already fenced/SIGTERM'd; the worktree and \
                 branch are preserved — resolve the merge and re-run `run salvage`/`run merge`)",
                e.message
            );
        }
        e
    })?;
    let out_warnings = mo.warnings.clone();

    emit(
        &SalvagePayload {
            run_id,
            node_id: node_id.as_str().to_string(),
            worker_state: worker.wire(),
            fenced,
            merge: merge_summary(&mo),
            dry_run: false,
        },
        args.spec,
        &out_warnings,
    )
}

fn merge_summary(mo: &merge::MergeOutcome) -> MergeSummary {
    MergeSummary {
        branch: mo.branch.clone(),
        source: mo.source.clone(),
        merged: mo.merged,
        report_seq: mo.report_seq,
        supervisor: mo.supervisor.clone(),
    }
}

fn emit(payload: &SalvagePayload, spec: &OutputSpec, warnings: &[String]) -> Result<(), CliError> {
    match spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => {
            output::emit_envelope(payload, spec, warnings)?;
        }
        OutputFormat::Text => {
            println!("run-id:       {}", payload.run_id);
            println!("node-id:      {}", payload.node_id);
            println!("worker-state: {}", payload.worker_state);
            println!("fenced:       {}", payload.fenced);
            println!("branch:       {}", payload.merge.branch);
            match &payload.merge.source {
                Some(s) => println!("source:       {s}"),
                None => println!("source:       (auto-detect main/master)"),
            }
            if payload.dry_run {
                println!("note:         --dry-run (no fence, no merge)");
            } else {
                println!("merged:       {}", payload.merge.merged);
                if let Some(seq) = payload.merge.report_seq {
                    println!("report_seq:   {seq}");
                }
            }
            output::emit_text_warnings(warnings);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    use chrono::{DateTime, Utc};
    use taskfleet_core::{Kind, Node, RunId, WorkerExit};

    /// A minimal `n-0001` node with the pid/exit fields under test set.
    fn node(
        agent_pid: Option<i32>,
        start_time: Option<DateTime<Utc>>,
        exit: Option<WorkerExit>,
    ) -> Node {
        Node {
            schema_version: 1,
            node_id: NodeId::parse_str("n-0001").unwrap(),
            run_id: RunId::parse_str("01jxsnap000000000000000000").unwrap(),
            parent_node_id: None,
            kind: Kind::Spinoff,
            status: Status::Running,
            task: None,
            worktree_path: Some("/tmp/wt".into()),
            branch: Some("wt/x".into()),
            base_sha: None,
            tmux_window: None,
            tmux_identity: None,
            evidence: None,
            retained_display: None,
            retention_unavailable: None,
            agent_pid,
            agent_pid_start_time: start_time,
            supervisor_pid: None,
            children: vec![],
            started_at: None,
            updated_at: Utc::now(),
            last_report: None,
            last_processed_report_seq_by_child: serde_json::Map::new(),
            retry_attempts: 0,
            worker_exit: exit,
            pending_merge: None,
            first_death_at: None,
            awaiting_input: None,
        }
    }

    fn spawn_sleeper() -> std::process::Child {
        Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep")
    }

    /// A recorded `worker.exited` is trusted (Exited) when the OS cannot POSITIVELY
    /// prove the recorded pid is still the original worker — here a live pid but
    /// with NO recorded start-time, so its identity can't be confirmed (it may be a
    /// recycled pid). The told exit wins.
    #[test]
    fn told_exit_wins_when_live_pid_identity_unprovable() {
        let mut child = spawn_sleeper();
        let exit = WorkerExit {
            code: Some(0),
            signal: None,
            at: Utc::now(),
        };
        let n = node(Some(child.id() as i32), None, Some(exit));
        assert_eq!(classify_worker(&n), WorkerState::Exited);
        let _ = child.kill();
        let _ = child.wait();
    }

    /// SAFETY OVERRIDE: a recorded `worker.exited` is NOT trusted when the OS
    /// positively proves the recorded pid is still the original worker (alive AND
    /// start-time matches). Merging over a provably-live worker would be silent
    /// corruption, so the told fact is overridden to `Live` (forcing the `--fence`
    /// gate). Review-consensus fix.
    #[test]
    fn live_matching_identity_overrides_a_stale_told_exit() {
        let mut child = spawn_sleeper();
        let pid = child.id();
        let st = watchdog::pid_start_time(pid).expect("read child start_time");
        let recorded = DateTime::from_timestamp(st as i64, 0).unwrap();
        let exit = WorkerExit {
            code: Some(0),
            signal: None,
            at: Utc::now(),
        };
        let n = node(Some(pid as i32), Some(recorded), Some(exit));
        assert_eq!(
            classify_worker(&n),
            WorkerState::Live {
                pid,
                start_time: st
            },
            "OS proof of a live original worker must override a stale told exit"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    /// No recorded pid → nothing to fence.
    #[test]
    fn no_pid_is_no_pid() {
        assert_eq!(classify_worker(&node(None, None, None)), WorkerState::NoPid);
        // A non-positive pid is treated as "no pid" (never a signal target).
        assert_eq!(
            classify_worker(&node(Some(0), None, None)),
            WorkerState::NoPid
        );
    }

    /// A dead recorded pid is `Gone` — safe to salvage, nothing to fence.
    #[test]
    fn dead_pid_is_gone() {
        let mut child = spawn_sleeper();
        let pid = child.id();
        child.kill().unwrap();
        child.wait().unwrap();
        // The pid is now dead (reaped). A recorded start-time is irrelevant.
        assert_eq!(
            classify_worker(&node(Some(pid as i32), None, None)),
            WorkerState::Gone
        );
    }

    /// A live pid whose recorded start-time matches is the original, live worker —
    /// `Live` (fenceable). A live pid with NO recorded start-time is
    /// `Unverifiable` (never fenced).
    #[test]
    fn live_pid_identity_gates_fenceability() {
        let mut child = spawn_sleeper();
        let pid = child.id();
        let st = watchdog::pid_start_time(pid).expect("read child start_time");
        let recorded = DateTime::from_timestamp(st as i64, 0).unwrap();

        assert_eq!(
            classify_worker(&node(Some(pid as i32), Some(recorded), None)),
            WorkerState::Live {
                pid,
                start_time: st
            }
        );
        assert_eq!(
            classify_worker(&node(Some(pid as i32), None, None)),
            WorkerState::Unverifiable { pid }
        );
        // A recorded start-time that disagrees (1970) → the pid was recycled → Gone.
        let bogus = DateTime::from_timestamp(1, 0).unwrap();
        assert_eq!(
            classify_worker(&node(Some(pid as i32), Some(bogus), None)),
            WorkerState::Gone
        );

        let _ = child.kill();
        let _ = child.wait();
    }

    /// `fence_worker` SIGTERMs a live process and returns once it is gone.
    ///
    /// A concurrent reaper thread `wait()`s the child so it does not linger as a
    /// zombie (a zombie still answers `kill(pid, 0)` as alive). In production the
    /// fenced worker is never salvage's own child, so it is reaped by its real
    /// parent and this concern does not arise.
    #[test]
    fn fence_worker_terminates_a_live_process() {
        let mut child = spawn_sleeper();
        let pid = child.id();
        assert!(pid_file::pid_alive(pid));
        let st = watchdog::pid_start_time(pid).expect("read child start_time");
        let reaper = std::thread::spawn(move || {
            let _ = child.wait();
        });
        fence_worker(pid, st).expect("fence succeeds");
        reaper.join().unwrap();
        assert!(!pid_file::pid_alive(pid), "worker must be dead after fence");
    }

    /// The classify→signal recycle guard: `fence_worker` with an identity that no
    /// longer matches the live pid must NOT signal it (it may be a recycled,
    /// unrelated process) — it returns Ok and leaves the process alive.
    #[test]
    fn fence_worker_does_not_signal_a_recycled_pid() {
        let mut child = spawn_sleeper();
        let pid = child.id();
        // A start-time that cannot match the live child (1970).
        fence_worker(pid, 1).expect("mismatched identity is a benign no-op");
        assert!(
            pid_file::pid_alive(pid),
            "an identity-mismatched pid must be left untouched"
        );
        let _ = child.kill();
        let _ = child.wait();
    }
}
