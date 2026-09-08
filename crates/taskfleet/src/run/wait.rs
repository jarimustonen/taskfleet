//! `run wait` — block until one or more runs reach a terminal state.
//!
//! A blocking completion primitive so callers stop hand-rolling
//! `while ... run show ... case` poll loops (and re-introducing the
//! shell-portability / wrong-field bugs that motivated this issue). The
//! correct, tested loop lives here in the binary instead of in every
//! caller's shell.
//!
//! ## Implementation: thin poll (v0.1.0 first cut)
//!
//! This is the issue's "smallest viable first cut": an internal poll of
//! each run's `manifest.status` projection with bounded exponential
//! backoff. It is a strictly **read-only** caller of the existing
//! `manifest.status` projection — it never appends events, never spawns or
//! signals a supervisor, and never mutates run state. The supervisor knows
//! precisely when a run goes terminal; a future revision could subscribe to
//! the event stream for zero-lag wakeups, but the poll already removes the
//! whole bug class.
//!
//! Each per-run read wraps `manifest.json` (and, at the end, the terminal
//! `node` projection) in `RunLock::with_shared_lock` so a concurrent reducer
//! can never expose a half-applied projection set (CLAUDE.md state-integrity
//! invariant 3).
//!
//! ## Stalled early-exit (stillborn + orphaned)
//!
//! A run is normally settled by reaching a terminal `manifest.status`. But a run
//! whose supervisor died can never reach one on its own and would otherwise
//! block the whole `--timeout` (a real stillborn incident waited ~6h). The poll
//! treats two such shapes as settled, via [`stall_kind`] over the manifest plus
//! a single-file supervisor-pid probe:
//!
//! - **stillborn** — supervisor died *before* creating any worker node
//!   (`node_count == 0`, no progress since creation; issue
//!   `run-wait-stillborn-run-not-detected`);
//! - **orphaned** — supervisor died *mid-run*, after creating ≥1 node
//!   (`node_count > 0`, `pending`/`running`, idle past a grace window; issue
//!   `run-wait-still`).
//!
//! Either way the run cannot become terminal on its own, so counting it as
//! settled lets the wait return promptly instead of blocking; the outcome
//! carries `stalled: true` with a per-kind `error` reason and, under
//! `--fail-on-error`, grades as a failure (exit `3`). The remediation both point
//! at is `run reattach`, which revives the supervisor.
//!
//! ## Exit codes
//!
//! A run counts as *settled* when it reaches a terminal status OR when it is
//! stalled (supervisor dead — stillborn before any node, or orphaned mid-run —
//! so it can never become terminal on its own and settles the wait rather than
//! blocking the whole timeout; see the "Stalled early-exit" section).
//!
//! - `0` — wait condition satisfied (`--all` → every run settled; `--any`
//!   → ≥1 settled). With `--fail-on-error`, every settled run finished `done`.
//! - `1` — usage / unknown run id / internal error (a `CliError`).
//! - `2` — `--timeout` reached before the wait condition was met.
//! - `3` — `--fail-on-error` and the wait condition was met but ≥1 settled
//!   run was `failed`/`cancelled`/stalled/attention-required. A stalled or
//!   attention-required run is still `pending`/`running`, so exit `3` can
//!   accompany a non-terminal status.
//!
//! Exit codes `2` and `3` still emit the normal success-shaped data
//! envelope on stdout (the summary is the answer); they cannot ride the
//! shared `Result<(), CliError>` path (which emits an *error* envelope on
//! stderr), so they flush logs and `std::process::exit` directly, mirroring
//! `event tail`'s signal-exit precedent.

use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use taskfleet_core::{
    read_manifest_opt, read_node_opt, AwaitingInput, NodeId, RunLock, RunPaths, Status, WorkerExit,
};

use crate::error::CliError;
use crate::output::{self, OutputFormat, OutputSpec};
use crate::run::dto::SupervisorView;
use crate::run::stalled::{stall_kind, StallKind};
use crate::run::{from_core, run_paths_from_cli_arg, status_kebab};

/// Reporting node whose terminal `node.report` carries the run's outcome
/// summary. Every single-worker worktree kind has exactly one node
/// (`n-0001`); mirrors `run merge`'s `DEFAULT_NODE_ID`.
const DEFAULT_NODE_ID: &str = "n-0001";

/// Backoff cadence (documented at the call site in [`wait_loop`]): start at
/// 100ms and double each idle poll up to a 2s ceiling. Snappy for the common
/// case (a run that settles within a second or two of the call) without
/// hammering the filesystem on a long wait.
const BACKOFF_START: Duration = Duration::from_millis(100);
const BACKOFF_CAP: Duration = Duration::from_secs(2);

/// Which settle condition ends the wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition {
    /// Return once *every* listed run is settled (terminal or stalled).
    All,
    /// Return as soon as *one* listed run is settled (terminal or stalled).
    Any,
}

impl Condition {
    fn wire(self) -> &'static str {
        match self {
            Condition::All => "all",
            Condition::Any => "any",
        }
    }
}

pub struct Args<'a> {
    pub run_ids: Vec<String>,
    /// `--any` was passed (return on the first terminal run). When false the
    /// default `--all` applies.
    pub any: bool,
    /// Wait budget. The CLI now supplies a `6h` default (see `run::mod`'s
    /// `--timeout` arg) so a stuck run can't block an orchestrator forever;
    /// `None` (only reachable by a library caller that omits it) keeps the
    /// original block-until-terminal behaviour.
    pub timeout: Option<Duration>,
    pub fail_on_error: bool,
    pub progress: bool,
    /// `--poll-interval` override. When set, the fixed cadence replaces the
    /// default exponential backoff (callers shouldn't normally need it).
    pub poll_interval: Option<Duration>,
    pub spec: &'a OutputSpec,
    pub warnings: &'a [String],
}

/// One run's settle outcome (`data.runs[]`).
///
/// The four booleans (`merged`, `landed`, `stalled`, `attention_required`) are
/// each an independent, orthogonal fact about the settle — a landing signal, two
/// git-verification-vs-marker distinctions, and two non-terminal "why it settled
/// without finishing" verdicts — not a state that would collapse into one enum
/// (a run can be `landed` yet not `merged`; `stalled` and `attention_required`
/// are mutually exclusive but distinct remediations). They are the stable wire
/// contract a JSON consumer branches on, so `struct_excessive_bools` is allowed
/// here deliberately.
#[allow(clippy::struct_excessive_bools)]
#[derive(Serialize)]
struct RunOutcome {
    run_id: String,
    status: &'static str,
    /// Report-based landing marker: the terminal `node.report` is a confirmed
    /// `run merge` — a typed `ReportOrigin::RunMerge` origin, or (for a legacy
    /// report with no origin field) `via: "explicit-merge"` (issue
    /// `retire-via-string`). Retained for backward compatibility — prefer
    /// [`Self::landed`], which is git-verified and robust to a caller-side
    /// rebase (issue `landing-signal-reliable-after-rebase`).
    merged: bool,
    /// Rebase-robust landing signal: true when the worker's committed work has
    /// landed in the target, confirmed by patch-id equivalence against the
    /// *current* target tip (`git cherry`) — NOT by branch-ref ancestry, which a
    /// caller-side `git rebase` invalidates. Falls back to the durable merge
    /// marker when git verification is unavailable. See [`landed_method`] for
    /// which. This is the flag callers should trust instead of running
    /// `git merge-base --is-ancestor <branch> <target>` by hand.
    ///
    /// [`landed_method`]: RunOutcome::landed_method
    landed: bool,
    /// How [`Self::landed`] was decided: `git-verified` | `report-marker` |
    /// `unverified`.
    landed_method: &'static str,
    /// Computed hint (never persisted): true when this run's supervisor died and
    /// it can never progress on its own — either *stillborn* (died before any
    /// node; issue `run-wait-stillborn-run-not-detected`) or *orphaned* (died
    /// mid-run with ≥1 node, idle past the grace; issue `run-wait-still`). Either
    /// way it is `pending`/`running` yet stranded. The poll counts such a run as
    /// settled (it returns promptly instead of blocking the whole `--timeout`);
    /// under `--fail-on-error` a stalled run grades as a failure (exit `3`), and
    /// the per-kind reason is in `error`.
    ///
    /// This folds in both supervisor-death shapes but NOT the
    /// undriven-orchestrate-driver hint (`is_stalled`) that `run show`/`run list`
    /// OR in: `run wait` is a single-worker completion primitive, so the
    /// driver-stall shape (a `--kind orchestrate` concern) stays out of it.
    stalled: bool,
    /// Computed hint (never persisted): the reporting node exited cleanly but is
    /// still non-terminal — it skipped `run merge` (design.md §2.5 / A5, issue
    /// `attention-required-run-surface`). Distinct from [`Self::stalled`]: the
    /// supervisor is not necessarily dead; the *worker* finished without merging,
    /// so the run needs a manual finish (`run merge` from the worktree) or
    /// `run cancel`, NOT `run reattach`. Like `stalled`, an attention-required run
    /// settles the wait (it returns promptly instead of blocking the whole
    /// `--timeout`) and — because it did not finish `done` — grades as a settled
    /// error under `--fail-on-error` (exit `3`). The run status is still
    /// `pending`/`running`; this classification NEVER mutates it terminal. The
    /// machine reason rides in [`Self::error`]; the full resume context (worktree
    /// path, resume hint, pid, age) rides in [`Self::attention`].
    attention_required: bool,
    /// Durable open human-decision request. `run wait` settles on this only
    /// after its propagation grace has elapsed.
    awaiting_input: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    awaiting_input_detail: Option<crate::run::awaiting_input::AwaitingInputView>,
    /// True only when the post-grace request itself satisfied the wait, not
    /// merely when an un-escalated sibling is visible in an `--any` outcome.
    #[serde(skip)]
    settled_awaiting_input: bool,
    /// Resume context for an attention-required run — worktree path, source
    /// branch, worker pid, pending age, and a one-line resume hint — so an AI
    /// caller that unblocks on `attention_required` can drive `run merge` from the
    /// worktree WITHOUT a second `run show` call (the AI-first contract). `None`
    /// (omitted from the wire) unless [`Self::attention_required`] is true. The
    /// same `AttentionView` shape `run show` / `run list` expose.
    #[serde(skip_serializing_if = "Option::is_none")]
    attention: Option<crate::run::attention::AttentionView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// The `recoverable_work` block a supervisor stamps into an `agent-died`
    /// FAILED report when the dead agent's branch has unmerged commits ahead of
    /// source (issue `agent-death-strands-recoverable-work`). Surfaced verbatim
    /// so a caller can detect stranded, salvageable work without hand-rolling
    /// `git log <source>..<branch>`. Absent on any other outcome.
    #[serde(skip_serializing_if = "Option::is_none")]
    recoverable_work: Option<Value>,
}

#[derive(Serialize)]
struct WaitData {
    waited_ms: u64,
    condition: &'static str,
    /// The authoritative reason the wait loop returned. This is intentionally
    /// serialized from `Stop` itself rather than re-derived from run outcomes.
    outcome: Stop,
    runs: Vec<RunOutcome>,
}

/// Why the poll loop stopped.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Stop {
    /// The wait condition (`--all`/`--any`) was satisfied.
    ConditionMet,
    /// `--timeout` elapsed first.
    TimedOut,
}

impl Stop {
    const fn wire(self) -> &'static str {
        match self {
            Self::ConditionMet => "condition-met",
            Self::TimedOut => "timed-out",
        }
    }
}

pub fn run(args: Args<'_>) -> Result<(), CliError> {
    let condition = if args.any {
        Condition::Any
    } else {
        Condition::All
    };

    let root = crate::home::root_dir()?;

    // Validate + resolve every run up front so a malformed or unknown id is a
    // fast exit-1, never something we discover mid-poll. `run_paths_from_cli_arg` rejects a
    // malformed ULID (`invalid_run_id`); a well-formed id naming no run on disk
    // surfaces as `unknown_run` here.
    let mut runs: Vec<(String, RunPaths)> = Vec::with_capacity(args.run_ids.len());
    for run_id in &args.run_ids {
        let paths = run_paths_from_cli_arg(&root, run_id)?;
        if current_status(&paths)?.is_none() {
            return Err(
                CliError::user("unknown_run", format!("no run with id {run_id}"))
                    .with_invalid_value(run_id),
            );
        }
        runs.push((run_id.clone(), paths));
    }

    let start = Instant::now();
    // `latched_stall[i]` is run `i`'s stall verdict AT THE POLL THAT ENDED THE
    // WAIT — the decision `wait_loop` actually acted on. `read_outcome` reuses it
    // rather than recomputing from a fresh probe, so the reported outcome (and
    // the exit code derived from it) can never disagree with the stop decision. A
    // stall verdict is not monotonic like a terminal status — a `run reattach` in
    // the gap between the settling poll and the outcome read could revive the
    // supervisor — so recomputing there would let the wait exit `0` on a
    // `pending` run it had just settled as stalled (a hole every reviewer
    // flagged).
    let (stop, latched_settle) = wait_loop(
        &runs,
        condition,
        args.timeout,
        args.poll_interval,
        args.progress,
    )?;
    let waited_ms = start.elapsed().as_millis() as u64;

    // Build the final per-run summary, folding in each terminal node report and
    // the latched stall / attention verdict.
    let mut outcomes = Vec::with_capacity(runs.len());
    for (i, (run_id, paths)) in runs.iter().enumerate() {
        outcomes.push(read_outcome(run_id, paths, latched_settle[i].clone())?);
    }

    // Decide the exit code from the assembled outcomes:
    //   timeout                                     → 2
    //   --fail-on-error & any settled run
    //     failed/cancelled/stalled/attention        → 3
    //   otherwise                                   → 0
    // A timeout takes precedence over fail-on-error: the condition was never
    // met, so there is nothing to grade. A stalled run (stillborn or orphaned) or
    // an attention-required run (clean exit, no `run merge`) settled the wait
    // without ever finishing `done`, so each grades as a failure too.
    let exit_code: u8 = match stop {
        Stop::TimedOut => 2,
        Stop::ConditionMet if args.fail_on_error && any_settled_error(&outcomes) => 3,
        Stop::ConditionMet => 0,
    };

    let data = WaitData {
        waited_ms,
        condition: condition.wire(),
        outcome: stop,
        runs: outcomes,
    };
    emit(&data, args.spec, args.warnings)?;

    if exit_code == 0 {
        return Ok(());
    }
    // Non-zero-but-not-an-error: the data envelope is already on stdout. Flush
    // this process's buffered logs (process::exit skips the LogGuard's Drop)
    // then exit with the graded code.
    crate::cli::flush_logs();
    std::process::exit(i32::from(exit_code));
}

/// Poll until the condition is met or `--timeout` elapses. Returns the reason
/// the loop stopped **and** the per-run stall verdict from the poll that
/// ended the wait (`Vec` indexed like `runs`), so the caller can build outcomes
/// consistent with the decision the loop acted on (see [`run`]).
///
/// Cadence: the sleep between idle polls starts at [`BACKOFF_START`] (100ms)
/// and doubles each round up to [`BACKOFF_CAP`] (2s), unless `--poll-interval`
/// pins a fixed cadence. Every sleep is clamped to the remaining timeout
/// budget so the loop wakes right at the deadline rather than overshooting it
/// (keeps the reported `waited_ms` ≈ the requested timeout).
type ProgressKey = (Status, Option<StallKind>, bool, Option<u64>);

fn wait_loop(
    runs: &[(String, RunPaths)],
    condition: Condition,
    timeout: Option<Duration>,
    poll_interval: Option<Duration>,
    progress: bool,
) -> Result<(Stop, Vec<LatchedSettle>), CliError> {
    let start = Instant::now();
    let mut backoff = poll_interval.unwrap_or(BACKOFF_START);
    // Progress transitions key on `(status, stall, attention)` so a healthy
    // `pending` run going stillborn/orphaned OR attention-required (status
    // unchanged) still emits one progress line.
    let mut prev: Vec<Option<ProgressKey>> = vec![None; runs.len()];

    loop {
        let mut settled = 0usize;
        let mut settle_now: Vec<LatchedSettle> = vec![LatchedSettle::default(); runs.len()];
        // Sample the clock ONCE per poll and share it across every run in this
        // iteration, so a multi-run wait grades all runs' grace windows against
        // the same `now` (rather than a per-run drift as the loop walks the
        // list). Passed into `current_settle` so the settle predicate stays
        // deterministic and test-injectable through `stall_kind`.
        let now = chrono::Utc::now();
        for (i, (run_id, paths)) in runs.iter().enumerate() {
            let settle = current_settle(paths, now)?.ok_or_else(|| {
                // A run dir validated at entry but its manifest vanished
                // mid-poll: report it rather than spin forever.
                CliError::system(
                    "io_error",
                    format!("run {run_id} manifest disappeared while waiting"),
                )
            })?;
            settle_now[i] = LatchedSettle {
                stall: settle.stall,
                attention: settle.attention,
                awaiting_input: settle.awaiting_input.clone(),
            };
            // A run is settled when it reaches a terminal status, OR when it is
            // stalled (a supervisor that died — before creating any node, or
            // mid-run — can never make the run terminal on its own), OR when it is
            // attention-required (the worker exited cleanly but skipped `run
            // merge`, so nothing will drive it terminal either). Waiting for
            // `is_terminal()` alone would block the whole timeout in every one of
            // those cases (issues `run-wait-stillborn-run-not-detected`,
            // `run-wait-still`, `attention-required-run-surface`).
            if settle.status.is_terminal()
                || settle.stall.is_some()
                || settle.attention
                || settle.awaiting_input.is_some()
            {
                settled += 1;
            }
            let key = (
                settle.status,
                settle.stall,
                settle.attention,
                settle.awaiting_input.as_ref().map(|v| v.event_seq),
            );
            if progress && prev[i] != Some(key) {
                emit_progress(
                    run_id,
                    settle.status,
                    settle.stall.is_some(),
                    settle.attention,
                    settle.awaiting_input.is_some(),
                );
            }
            prev[i] = Some(key);
        }

        let met = match condition {
            Condition::All => settled == runs.len(),
            Condition::Any => settled >= 1,
        };
        if met {
            return Ok((Stop::ConditionMet, settle_now));
        }
        if let Some(t) = timeout {
            if start.elapsed() >= t {
                return Ok((Stop::TimedOut, settle_now));
            }
        }

        // Sleep, clamped to whatever timeout budget remains.
        let sleep_dur = match timeout {
            Some(t) => backoff.min(t.saturating_sub(start.elapsed())),
            None => backoff,
        };
        if sleep_dur.is_zero() {
            // Sub-millisecond budget left: yield briefly so we don't busy-spin
            // before the next iteration trips the timeout check above.
            std::thread::sleep(Duration::from_millis(1));
        } else {
            std::thread::sleep(sleep_dur);
        }
        if poll_interval.is_none() {
            backoff = (backoff * 2).min(BACKOFF_CAP);
        }
    }
}

/// Read a run's current `manifest.status` under the shared lock. `None` when
/// the run dir holds no manifest (unknown/uninitialized run). Holding
/// `LOCK_SH` keeps the single-file read consistent with a concurrent reducer.
fn current_status(paths: &RunPaths) -> Result<Option<Status>, CliError> {
    RunLock::with_shared_lock(&paths.lock(), || {
        Ok(read_manifest_opt(paths)?.map(|m| m.status))
    })
    .map_err(from_core)
}

/// One poll's settle snapshot: the run's `manifest.status`, its read-time stall
/// verdict — [`StallKind::Stillborn`] (supervisor dead before any node) or
/// [`StallKind::Orphaned`] (supervisor died mid-run, ≥1 node, idle past the
/// grace), or `None` for a healthy run — and whether the reporting node is
/// *attention-required* (a clean worker exit that skipped `run merge`,
/// design.md §2.5 / A5). All decided as ONE consistent snapshot under the shared
/// lock — the supervisor-pid probe and the node read do not participate in the
/// projection guards, but reading them inside the lock keeps `status`, the stall
/// verdict, and the attention verdict from disagreeing (mirrors `run show`).
struct Settle {
    status: Status,
    stall: Option<StallKind>,
    /// The reporting node exited cleanly but is still non-terminal — it skipped
    /// `run merge`. A durable told fact ([`taskfleet_core::Node::worker_exit`]), not a
    /// timing guess, so it settles the wait immediately rather than blocking the
    /// whole `--timeout`. Checked with precedence OVER `stall`: a clean-exited
    /// worker is attention-required (manual finish) even if its supervisor later
    /// died, which would otherwise read as `orphaned` (`run reattach`).
    attention: bool,
    /// Exact explicit decision generation whose durable grace has elapsed.
    awaiting_input: Option<AwaitingInput>,
}

/// The two non-terminal settle verdicts a poll can latch for a run — the stall
/// kind (if any) and whether it is attention-required — carried from the poll
/// that ended the wait through to [`read_outcome`], so the reported outcome
/// reflects the decision [`wait_loop`] acted on rather than a fresh probe a
/// concurrent `run reattach` / `run merge` could have flipped.
#[derive(Clone, Default)]
struct LatchedSettle {
    stall: Option<StallKind>,
    attention: bool,
    awaiting_input: Option<AwaitingInput>,
}

/// Read a run's [`Settle`] snapshot under the shared lock. `None` when the run
/// dir holds no manifest (unknown/uninitialized run). `now` is passed in (one
/// value per poll — see [`wait_loop`]) rather than sampled here, so the settle
/// decision is deterministic and every run in a multi-run poll shares one clock.
///
/// The shared lock gives a consistent *manifest/projection* snapshot; it does
/// NOT make the supervisor liveness probe transactionally consistent with the
/// manifest (the pid file is removed without the lock, and process death is
/// unsynchronized). The probe is read here so it is as fresh as the manifest it
/// sits beside — a best-effort point-in-time hint, not a welded pairing.
fn current_settle(
    paths: &RunPaths,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Option<Settle>, CliError> {
    RunLock::with_shared_lock(&paths.lock(), || {
        let Some(m) = read_manifest_opt(paths)? else {
            return Ok(None);
        };
        let supervisor = SupervisorView::probe(paths);
        // Read the reporting node in the same shared-lock window so its
        // `worker_exit` fact and the manifest `status` form one consistent
        // snapshot (state-integrity invariant 3). A run with no `n-0001` node
        // yet, or a node still running, simply is not attention-required.
        let node_id =
            NodeId::parse_str(DEFAULT_NODE_ID).expect("DEFAULT_NODE_ID is a valid node id");
        let node = read_node_opt(paths, &node_id)?;
        let open = node.as_ref().and_then(|n| n.awaiting_input.as_deref());
        let awaiting_input = open
            .filter(|open| crate::run::awaiting_input::is_escalated(open.opened_at, now))
            .cloned();
        let attention = node.as_ref().is_some_and(|n| {
            crate::run::attention::is_attention_required(n.status, n.worker_exit.as_ref())
        });
        // Resolve attention-over-stall precedence HERE (design.md §2.5), at snapshot
        // construction — not only in `read_outcome` — so the `--progress` JSONL and
        // the settled-count decision agree with the final outcome. A clean-exited
        // worker whose supervisor also died must read attention (manual finish), not
        // orphaned (`run reattach`), on every surface.
        let stall = if attention {
            None
        } else {
            stall_kind(
                m.status,
                // Indeterminate supervisor states must not settle a wait as
                // stalled — see `SupervisorView::presumed_working`.
                supervisor.presumed_working(),
                m.node_count,
                m.created_at,
                m.updated_at,
                now,
            )
        };
        Ok(Some(Settle {
            status: m.status,
            stall,
            attention,
            awaiting_input,
        }))
    })
    .map_err(from_core)
}

/// Assemble one run's terminal outcome: its `manifest.status` plus the
/// outcome fields folded from the default node's terminal `node.report`
/// (`summary`, the confirmed-`run merge` marker via the typed report origin,
/// and — for a failed/cancelled settle — a best-effort `error` reason). The manifest and
/// node projections are read in a single shared-lock window so the status and
/// the report it implies cannot disagree (state-integrity invariant 3).
///
/// `latched` is the stall + attention verdict from the poll that ended the wait
/// — the caller passes it in rather than having this function recompute it, so
/// the outcome (and the exit code) reflect the decision `wait_loop` acted on, not
/// a fresh probe that a concurrent `run reattach` / `run merge` could have
/// flipped.
fn read_outcome(
    run_id: &str,
    paths: &RunPaths,
    latched: LatchedSettle,
) -> Result<RunOutcome, CliError> {
    let node_id = NodeId::parse_str(DEFAULT_NODE_ID).expect("DEFAULT_NODE_ID is a valid node id");
    // Read every field the outcome needs — status, the terminal report, and the
    // git-verification inputs (source repo/branch, worker branch/base_sha) — in
    // ONE shared-lock window so the status and the projections that explain it
    // cannot disagree (state-integrity invariant 3). The git shell-out that
    // computes `landed` runs AFTER the lock is released (below): holding the
    // flock across a subprocess would serialize every reader behind git.
    let git_inputs = RunLock::with_shared_lock(&paths.lock(), || {
        let manifest = read_manifest_opt(paths)?;
        let node = read_node_opt(paths, &node_id)?;
        Ok(GitInputs {
            status: manifest.as_ref().map(|m| m.status),
            source_repo: manifest.as_ref().and_then(|m| m.source_repo.clone()),
            source_branch: manifest.as_ref().and_then(|m| m.source_branch.clone()),
            worktree_path: node.as_ref().and_then(|n| n.worktree_path.clone()),
            branch: node.as_ref().and_then(|n| n.branch.clone()),
            base_sha: node.as_ref().and_then(|n| n.base_sha.clone()),
            // Attention resume-context inputs, read in the SAME shared-lock window
            // so the told clean-exit fact is consistent with the status above.
            worker_exit: node.as_ref().and_then(|n| n.worker_exit),
            agent_pid: node.as_ref().and_then(|n| n.agent_pid),
            awaiting_input: node
                .as_ref()
                .and_then(|n| n.awaiting_input.as_deref().cloned()),
            report: node.and_then(|n| n.last_report),
        })
    })
    .map_err(from_core)?;

    let status = git_inputs.status.ok_or_else(|| {
        CliError::system(
            "io_error",
            format!("run {run_id} manifest disappeared while waiting"),
        )
    })?;
    let report = git_inputs.report.clone();

    // Git-verified `landed` (issue `landing-signal-reliable-after-rebase`): the
    // rebase-robust signal the caller should trust instead of hand-rolling
    // `git merge-base --is-ancestor`. Computed outside the shared lock.
    let signal = crate::run::landed::landing_signal(
        &crate::run::landed::LandingInputs {
            source_repo: git_inputs.source_repo.as_deref(),
            source_branch: git_inputs.source_branch.as_deref(),
            worktree_path: git_inputs.worktree_path.as_deref(),
            branch: git_inputs.branch.as_deref(),
            base_sha: git_inputs.base_sha.as_deref(),
            report: report.as_ref(),
        },
        &crate::supervise::cleanup::git_bin(),
    );

    // `merged` reads the same confirmed-merge truth as the reducer, the `landed`
    // fallback, and the supervisor teardown gate: the typed `ReportOrigin::RunMerge`
    // (issue `retire-via-string`), with the legacy `via: "explicit-merge"` string
    // honored only for a legacy report carrying no `origin` field. An agent-authored
    // report (normalized to an `Agent` origin by `node report`) can no longer flip
    // `merged` on a forged `via` string alone.
    let merged = report
        .as_ref()
        .is_some_and(taskfleet_core::ReportOrigin::report_is_confirmed_merge);
    let summary = report
        .as_ref()
        .and_then(|r| r.get("summary"))
        .and_then(Value::as_str)
        .map(str::to_string);
    // A non-terminal verdict (stall / attention) is only reported if the run did
    // NOT reach a terminal status: if a `run reattach` / `run merge` terminalized
    // the run in the gap between the settling poll and this read, its real
    // terminal status is the truth and the hint must not contradict it. In the
    // common case the run is still `pending`/`running` and the latched verdict
    // stands. Attention-required takes precedence over a stall: a worker that
    // exited cleanly needs a manual finish (`run merge`), not `run reattach`, even
    // if its supervisor also died (which would otherwise read as `orphaned`).
    let attention_required = !status.is_terminal() && latched.attention;
    let stall = if status.is_terminal() || attention_required {
        None
    } else {
        latched.stall
    };
    let stalled = stall.is_some();
    let settled_awaiting_input = !status.is_terminal() && latched.awaiting_input.is_some();
    // Preserve the exact generation that woke the wait. On timeout or when an
    // unrelated `--any` sibling settled, still expose the current open request
    // immediately even if its grace has not elapsed.
    let open = if status.is_terminal() {
        None
    } else {
        latched
            .awaiting_input
            .as_ref()
            .or(git_inputs.awaiting_input.as_ref())
    };
    let awaiting_input = open.is_some();
    // `error` explains a non-`done` settle. For failed/cancelled the §7.3 report
    // has no dedicated error field, so surface the cancel `reason` when present;
    // for a stalled settle (a `pending`/`running` run graded as a failure under
    // `--fail-on-error`) synthesize a structured reason — distinct per stall
    // kind — so a JSON grader can tell "supervisor never started" from
    // "supervisor died mid-run" from "worker skipped run merge" without
    // re-deriving it.
    let error = if attention_required {
        Some(crate::run::attention::ATTENTION_REASON.to_string())
    } else if let Some(kind) = stall {
        Some(stall_reason(kind).to_string())
    } else if settled_awaiting_input {
        Some(crate::run::awaiting_input::AWAITING_INPUT_REASON.to_string())
    } else if matches!(status, Status::Failed | Status::Cancelled) {
        report
            .as_ref()
            .and_then(|r| r.get("reason"))
            .and_then(Value::as_str)
            .map(str::to_string)
    } else {
        None
    };
    // The full resume context for an attention-required outcome, so an AI caller
    // that unblocks on `attention_required` drives `run merge` from the worktree
    // without a second `run show` (design.md §2.5, AI-first contract). The told
    // clean exit that SET `attention_required` is normally still present; if a
    // race cleared it, degrade to no block (the `error` reason still explains the
    // outcome). Uses the run's resolved id so the hint's `run merge <id>` is exact.
    let attention = if attention_required {
        git_inputs.worker_exit.as_ref().map(|exit| {
            crate::run::attention::AttentionView::build(
                paths.run_id.as_str(),
                chrono::Utc::now(),
                exit,
                git_inputs.agent_pid,
                git_inputs.worktree_path.clone(),
                git_inputs.source_branch.clone(),
            )
        })
    } else {
        None
    };
    // Surface the supervisor's stranded-work signal verbatim (present only on an
    // `agent-died` failed report whose branch has unmerged commits ahead of
    // source). A caller reads `recoverable_work.recoverable` to decide whether to
    // salvage; the block is otherwise absent. Gated on a `failed` status: the
    // supervisor only ever stamps it on the failed-synthesis path, so a block on
    // a `done`/`cancelled` report is stale or spoofed and must not be surfaced (a
    // regular agent report can carry unknown fields — the validator permits them).
    let awaiting_input_detail = open
        .map(|open| crate::run::awaiting_input::AwaitingInputView::build(open, chrono::Utc::now()));
    let recoverable_work = if matches!(status, Status::Failed) {
        report
            .as_ref()
            .and_then(|r| r.get("recoverable_work"))
            .filter(|v| v.is_object())
            .cloned()
    } else {
        None
    };

    Ok(RunOutcome {
        run_id: run_id.to_string(),
        status: status_kebab(status),
        merged,
        landed: signal.landed,
        landed_method: signal.method.wire(),
        stalled,
        attention_required,
        awaiting_input,
        awaiting_input_detail,
        settled_awaiting_input,
        attention,
        summary,
        error,
        recoverable_work,
    })
}

/// The structured `error` reason for a stalled outcome, distinct per stall kind
/// so a JSON grader (and the human text line) can tell the two supervisor-death
/// shapes apart. Both point the caller at `run reattach` to revive the
/// supervisor, which then rolls the run up or fails it via the no-worker /
/// agent-death backstops.
fn stall_reason(kind: StallKind) -> &'static str {
    match kind {
        StallKind::Stillborn => "supervisor died before creating any worker node",
        StallKind::Orphaned => "supervisor died mid-run; work is stranded and cannot be rolled up",
    }
}

/// The run/node fields `read_outcome` reads under the shared lock before
/// computing `landed` outside it. Bundling them keeps the single consistent
/// snapshot (invariant 3) explicit and lets the git shell-out run lock-free.
struct GitInputs {
    status: Option<Status>,
    source_repo: Option<String>,
    source_branch: Option<String>,
    worktree_path: Option<String>,
    branch: Option<String>,
    base_sha: Option<String>,
    /// The reporting node's told worker exit — the fact that drives the attention
    /// verdict, read in the same shared-lock window as `status`.
    worker_exit: Option<WorkerExit>,
    /// The reporting node's last-observed worker pid, for the attention resume
    /// context.
    agent_pid: Option<i32>,
    awaiting_input: Option<taskfleet_core::AwaitingInput>,
    report: Option<Value>,
}

/// True iff any *settled* run did not finish cleanly `done` — i.e. it is
/// `failed`, `cancelled`, `stalled` (a `pending`/`running` run that settled the
/// wait only because its supervisor died — stillborn before starting, or orphaned
/// mid-run), or `attention_required` (a `pending`/`running` run whose worker
/// exited cleanly but skipped `run merge`). Non-settled runs (a still-progressing
/// run under `--any`) are not graded: they never settled.
fn any_settled_error(outcomes: &[RunOutcome]) -> bool {
    outcomes.iter().any(|o| {
        matches!(o.status, "failed" | "cancelled")
            || o.stalled
            || o.attention_required
            || o.settled_awaiting_input
    })
}

/// Emit one compact JSONL transition line to **stderr** for `--progress`, so a
/// live UI can follow state changes while the machine summary still lands on
/// stdout at the end. `stalled` / `attention_required` carry the non-terminal
/// verdicts so a run that goes stillborn/orphaned OR attention-required without a
/// status change (it stays `pending`) still surfaces one line. Best-effort: a
/// serialization failure is swallowed rather than aborting the wait.
fn emit_progress(
    run_id: &str,
    status: Status,
    stalled: bool,
    attention_required: bool,
    awaiting_input: bool,
) {
    if let Ok(line) = serde_json::to_string(&serde_json::json!({
        "run_id": run_id,
        "status": status_kebab(status),
        "stalled": stalled,
        "attention_required": attention_required,
        "awaiting_input": awaiting_input,
    })) {
        eprintln!("{line}");
    }
}

/// Render a one-line human summary of a `recoverable_work` block for text
/// output (`run wait`, `run show`). `None` when the block is absent or malformed
/// (its presence is optional and best-effort). Shared so both surfaces phrase
/// the stranded-work signal identically.
pub(crate) fn recoverable_summary(block: Option<&Value>) -> Option<String> {
    let obj = block?.as_object()?;
    let unmerged = obj.get("unmerged_commits").and_then(Value::as_u64)?;
    let recoverable = obj
        .get("recoverable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let branch = obj.get("branch").and_then(Value::as_str).unwrap_or("?");
    // Agree the noun AND the verb in number so "1 unmerged commit merges" reads
    // correctly alongside "3 unmerged commits merge".
    let (noun, merge_verb, not_merge_verb) = if unmerged == 1 {
        ("commit", "merges", "does NOT merge")
    } else {
        ("commits", "merge", "do NOT merge")
    };
    if recoverable {
        Some(format!(
            "recoverable={unmerged} unmerged {noun} {merge_verb} cleanly on {branch}"
        ))
    } else {
        Some(format!(
            "recoverable=false ({unmerged} unmerged {noun} on {branch} {not_merge_verb} cleanly)"
        ))
    }
}

fn emit(data: &WaitData, spec: &OutputSpec, warnings: &[String]) -> Result<(), CliError> {
    match spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => {
            output::emit_envelope(data, spec, warnings)?;
        }
        OutputFormat::Text => {
            println!("condition:  {}", data.condition);
            println!("outcome:    {}", data.outcome.wire());
            println!("waited_ms:  {}", data.waited_ms);
            for r in &data.runs {
                print!(
                    "{}  status={} landed={} ({}) merged={}",
                    r.run_id, r.status, r.landed, r.landed_method, r.merged
                );
                if r.stalled {
                    // A remediation hint only; the specific per-kind reason is
                    // carried in the `error=` field below, printed unconditionally
                    // so a `grep error=` scraper finds a stalled run's reason the
                    // same way it finds a failed/cancelled one.
                    print!(
                        "  stalled=true (`run reattach {id}` or `run cancel {id}`)",
                        id = r.run_id
                    );
                }
                if r.awaiting_input {
                    let count = r
                        .awaiting_input_detail
                        .as_ref()
                        .map_or(0, |v| v.open_discussion_count);
                    print!("  awaiting_input=true ({count} open decision(s))");
                }
                if r.attention_required {
                    // Non-terminal, deliberate: the worker finished but skipped
                    // `run merge`. The manual finish (not `run reattach`) is the
                    // fix; name the worktree when known so the PO can `cd` to it.
                    // The reason rides in `error=` below.
                    match r
                        .attention
                        .as_ref()
                        .and_then(|a| a.worktree_path.as_deref())
                    {
                        Some(wt) => print!(
                            "  attention_required=true (worktree {wt}; \
                             `run merge {id}` there, or `run cancel {id}`)",
                            id = r.run_id
                        ),
                        None => print!(
                            "  attention_required=true (`run merge {id}` from its worktree, \
                             or `run cancel {id}`)",
                            id = r.run_id
                        ),
                    }
                }
                if let Some(s) = &r.summary {
                    print!("  summary={}", output::escape_one_line(s));
                }
                if let Some(e) = &r.error {
                    print!("  error={}", output::escape_one_line(e));
                }
                if let Some(line) = recoverable_summary(r.recoverable_work.as_ref()) {
                    print!("  {line}");
                }
                println!();
            }
            output::emit_text_warnings(warnings);
        }
    }
    Ok(())
}

/// clap value parser for `--timeout` / `--poll-interval`. Accepts humanly
/// written durations (`30s`, `5m`, `1h`, `2h 30m`); a malformed value is
/// rejected up front by clap as `invalid_arguments` (AGENTS-AI-FIRST-CLI §4),
/// never silently coerced.
///
/// A **bare unsigned integer** (all ASCII digits, e.g. `2400`) is interpreted
/// as **seconds** — so `--timeout 2400` == `--timeout 2400sec`. This closes a
/// silent-instant-exit trap: `humantime` alone rejects a unit-less integer, and
/// a backgrounded `run wait` that exits on that error looks "completed" to the
/// wrapping shell (exit 0) even though it never waited. Any value carrying a
/// unit falls through to the existing unit-aware parse unchanged.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let trimmed = s.trim();
    // Bare integer → seconds. Gate on an explicit all-ASCII-digits check rather
    // than trusting `u64::from_str`: the latter also accepts a leading `+`
    // (`+30`), which would diverge from the unit-bearing form (`+30s`, which
    // humantime rejects) and from the "unit-less integer" grammar we advertise.
    // A digit-only string that overflows `u64` is still unambiguously a bare
    // count, so report the range explicitly instead of letting it fall through
    // to a confusing "time unit needed" from humantime.
    if !trimmed.is_empty() && trimmed.bytes().all(|b| b.is_ascii_digit()) {
        let secs = trimmed
            .parse::<u64>()
            .map_err(|_| format!("invalid duration '{s}': bare seconds exceed {}", u64::MAX))?;
        return Ok(Duration::from_secs(secs));
    }
    // Anything with a unit (or otherwise non-numeric) defers to the unit-aware
    // parse. Use `trimmed` so surrounding whitespace is normalized on both
    // paths, not just the bare-integer one.
    humantime::parse_duration(trimmed).map_err(|e| format!("invalid duration '{s}': {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_accepts_human_units() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(
            parse_duration("2400sec").unwrap(),
            Duration::from_secs(2400)
        );
        assert_eq!(parse_duration("40min").unwrap(), Duration::from_secs(2400));
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
    }

    #[test]
    fn parse_duration_bare_integer_is_seconds() {
        // The unit-less-integer trap: `--timeout 2400` now waits 2400 seconds
        // instead of erroring out and letting a backgrounded wait exit instantly.
        assert_eq!(parse_duration("2400").unwrap(), Duration::from_secs(2400));
        assert_eq!(parse_duration("0").unwrap(), Duration::from_secs(0));
        // Leading zeros are decimal (not octal) and carry no unit ambiguity.
        assert_eq!(parse_duration("00030").unwrap(), Duration::from_secs(30));
        // A bare integer is exactly equivalent to its explicit-`sec` form.
        assert_eq!(
            parse_duration("30").unwrap(),
            parse_duration("30sec").unwrap()
        );
        // Surrounding whitespace is tolerated and normalized on both paths.
        assert_eq!(parse_duration("  30  ").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("  30s  ").unwrap(), Duration::from_secs(30));
    }

    #[test]
    fn parse_duration_bare_integer_overflow_is_a_clear_error() {
        // Digit-only but > u64::MAX: report the range explicitly rather than
        // deferring to humantime's misleading "time unit needed".
        let err = parse_duration("18446744073709551616").unwrap_err();
        assert!(err.contains("bare seconds exceed"), "got: {err}");
    }

    #[test]
    fn parse_duration_rejects_garbage() {
        assert!(parse_duration("soon").is_err());
        assert!(parse_duration("").is_err());
        assert!(parse_duration("   ").is_err());
        assert!(parse_duration("-5s").is_err());
        // A bare negative integer is not a valid unsigned count and has no unit,
        // so it stays an error rather than silently coercing.
        assert!(parse_duration("-5").is_err());
        // A leading `+` is not part of the advertised digits-only grammar, so it
        // is rejected (it is NOT silently read as `+30` == 30) — keeping the
        // bare and unit-bearing (`+30s`, also rejected) forms consistent.
        assert!(parse_duration("+30").is_err());
        // Internal whitespace does not make a bare integer.
        assert!(parse_duration("2 400").is_err());
    }

    #[test]
    fn any_settled_error_only_counts_terminal_failures() {
        let mk = |status: &'static str| RunOutcome {
            run_id: "r".into(),
            status,
            merged: false,
            landed: false,
            landed_method: "unverified",
            stalled: false,
            attention_required: false,
            awaiting_input: false,
            awaiting_input_detail: None,
            settled_awaiting_input: false,
            attention: None,
            summary: None,
            error: None,
            recoverable_work: None,
        };
        let mk_stalled = || RunOutcome {
            stalled: true,
            ..mk("pending")
        };
        let mk_attention = || RunOutcome {
            attention_required: true,
            ..mk("pending")
        };
        let mk_awaiting = || RunOutcome {
            awaiting_input: true,
            settled_awaiting_input: true,
            ..mk("pending")
        };
        assert!(!any_settled_error(&[mk("done"), mk("done")]));
        assert!(any_settled_error(&[mk("done"), mk("failed")]));
        assert!(any_settled_error(&[mk("cancelled")]));
        // A still-running run under --any is not a settled error.
        assert!(!any_settled_error(&[mk("done"), mk("running")]));
        // A stillborn (stalled) run grades as a settled error even though its
        // status is still `pending`.
        assert!(any_settled_error(&[mk_stalled()]));
        assert!(any_settled_error(&[mk("done"), mk_stalled()]));
        // An attention-required run (clean exit, no `run merge`) also grades as a
        // settled error even though its status stays `pending`.
        assert!(any_settled_error(&[mk_attention()]));
        assert!(any_settled_error(&[mk("done"), mk_attention()]));
        assert!(any_settled_error(&[mk_awaiting()]));
        // Visible but pre-grace awaiting input on an unsettled `--any` sibling
        // does not grade the completed sibling as an error.
        assert!(!any_settled_error(&[RunOutcome {
            awaiting_input: true,
            ..mk("running")
        }]));
    }

    #[test]
    fn recoverable_summary_phrasing() {
        // Absent / non-object → no line.
        assert_eq!(recoverable_summary(None), None);
        assert_eq!(recoverable_summary(Some(&serde_json::json!("x"))), None);

        // Clean, singular.
        let clean = serde_json::json!({
            "recoverable": true,
            "unmerged_commits": 1,
            "merges_cleanly": true,
            "branch": "wt/foo",
        });
        assert_eq!(
            recoverable_summary(Some(&clean)).as_deref(),
            Some("recoverable=1 unmerged commit merges cleanly on wt/foo"),
        );

        // Clean, plural.
        let many = serde_json::json!({
            "recoverable": true, "unmerged_commits": 3, "merges_cleanly": true, "branch": "wt/bar",
        });
        assert_eq!(
            recoverable_summary(Some(&many)).as_deref(),
            Some("recoverable=3 unmerged commits merge cleanly on wt/bar"),
        );

        // Unmerged but conflicting → flagged, not recoverable.
        let dirty = serde_json::json!({
            "recoverable": false, "unmerged_commits": 2, "merges_cleanly": false, "branch": "wt/baz",
        });
        assert_eq!(
            recoverable_summary(Some(&dirty)).as_deref(),
            Some("recoverable=false (2 unmerged commits on wt/baz do NOT merge cleanly)"),
        );

        // Singular conflicting → the negative verb agrees in number too.
        let dirty1 = serde_json::json!({
            "recoverable": false, "unmerged_commits": 1, "merges_cleanly": false, "branch": "wt/q",
        });
        assert_eq!(
            recoverable_summary(Some(&dirty1)).as_deref(),
            Some("recoverable=false (1 unmerged commit on wt/q does NOT merge cleanly)"),
        );
    }
}
