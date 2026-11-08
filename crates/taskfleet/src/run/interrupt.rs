//! `run interrupt` — stop a resident worker's current round without ending
//! the run (issue `resident-interrupt-round`, ADR 0003 amendment).
//!
//! The parent agent's equivalent of pressing Esc on its own agent. The verb
//! records the told fact first and only then signals:
//!
//! 1. Under the resident operation lock (so no resume, land, or idle expiry
//!    acts on the run meanwhile) and the run lock, it reads the node and
//!    appends `node.round_reported` with `interrupted: true` for the current
//!    attempt. From that moment the round is ended by a told fact, so the exit
//!    the signal causes is read by the supervisor exactly like the exit after
//!    a worker's own round report: `waiting`, never `failed` or attention
//!    (ADR 0003's outcome table, unchanged). Recording after the signal would
//!    leave a crash window in which a signalled worker fails the run.
//! 2. It signals the recorded agent process, but only after re-verifying its
//!    recorded start identity immediately before the `kill`, the same fence
//!    `run salvage` uses: a recycled pid is never signalled, and an alive pid
//!    whose identity cannot be read is a refusal before anything is recorded.
//!    Claude gets `SIGINT` (its `--print` turn aborts and the process exits);
//!    Pi gets `SIGTERM`, which Pi handles as its graceful shutdown (it emits
//!    `session_shutdown`, so the bundled adapter removes its socket, and kills
//!    the tool processes it started). Both keep their native session on disk.
//!    A Claude worker still alive after [`ESCALATE_AFTER`] gets `SIGTERM`.
//! 3. It releases the locks and waits (bounded by `--wait-exit`) for the
//!    shim's told `worker.exited`, the same fact every other resident verb
//!    waits for.
//!
//! Nothing touches the worktree, branch, or session: uncommitted changes stay
//! where the worker left them, and the next `run message` resumes the same
//! session (its preamble says the round was interrupted). The signal targets
//! the recorded agent pid only, not its process group, like salvage's fence;
//! Pi reaps its own tool processes, and an orphaned Claude tool subprocess is
//! the same known residual salvage documents.
//!
//! Re-running the verb is safe: on a round it already interrupted whose worker
//! is still alive it signals again (`SIGTERM`); on a worker that reported its
//! own round or already exited it does nothing and says so.

use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;
use taskfleet_core::{
    append_and_apply_unlocked, read_manifest_opt, read_node_opt, NodeId, ResidentState, RunLock,
    RunPaths,
};

use crate::error::CliError;
use crate::output::{self, OutputFormat, OutputSpec};
use crate::run::resident::{self, ResidentLock};
use crate::run::worker::{classify_worker, WorkerState};
use crate::run::{from_core, parse_node_id};
use crate::supervise::{pid_file, watchdog};

/// A Claude worker that survives `SIGINT` this long gets `SIGTERM`.
const ESCALATE_AFTER: Duration = Duration::from_secs(10);

const POLL: Duration = Duration::from_millis(100);

/// How long a provably gone worker may lack its told exit before the round
/// counts as ended anyway (its shim was lost too, e.g. to a hard kill). The
/// shim records the exit within milliseconds of reaping the worker.
const GONE_GRACE: Duration = Duration::from_secs(3);

/// The round summary Taskfleet records for an interrupted round.
pub(crate) const INTERRUPTED_SUMMARY: &str =
    "Interrupted by the parent agent (`taskfleet run interrupt`) before the \
     worker reported this round. Uncommitted work, if any, is still in the worktree.";

pub struct Args<'a> {
    pub run: String,
    pub node: String,
    pub wait_exit: Duration,
    pub dry_run: bool,
    pub spec: &'a OutputSpec,
    pub warnings: &'a [String],
}

#[derive(Serialize)]
struct Payload<'a> {
    run_id: &'a str,
    node_id: &'a str,
    resident_name: &'a str,
    harness: &'a str,
    attempt: u32,
    /// What this call found and did: `interrupted` (the round is recorded as
    /// interrupted and the worker exited), `between-rounds` (no worker process
    /// was running), or `round-reported` (the worker had already reported its
    /// own round and is stopping by itself). Dry run: `would-interrupt` or one
    /// of the no-op values.
    outcome: &'static str,
    /// The interrupted round's number, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    round: Option<u32>,
    /// Classified recorded worker (`live`, `gone`, `exited`, ...).
    worker_state: &'static str,
    /// Signals sent to the verified worker process, in order.
    signals: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_seq: Option<u64>,
    next: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    dry_run: Option<bool>,
}

fn signal_name(signal: libc::c_int) -> &'static str {
    match signal {
        libc::SIGINT => "SIGINT",
        _ => "SIGTERM",
    }
}

/// Send `signal` to the recorded worker `pid` only if its recorded start
/// identity still matches. `Ok(false)`: the recorded process is gone (exited,
/// or its pid now belongs to another process), so nothing was signalled.
fn signal_verified(
    pid: u32,
    expected: &watchdog::RecordedStart,
    signal: libc::c_int,
) -> Result<bool, CliError> {
    let Some(pid_t) = pid_file::to_pid_t(pid) else {
        return Err(CliError::system(
            "interrupt_signal_failed",
            format!("worker pid {pid} is out of range; refusing to signal"),
        ));
    };
    match expected.matches(pid) {
        Some(true) => {}
        Some(false) => return Ok(false),
        None if !pid_file::pid_alive(pid) => return Ok(false),
        None => {
            return Err(CliError::system(
                "worker_unverifiable",
                format!(
                    "worker pid {pid} is alive but its start identity is unreadable; refusing to \
                     signal a process that may not be the recorded worker"
                ),
            ))
        }
    }
    // SAFETY: `pid_t` is range-checked (never 0 or negative, so never a group
    // or broadcast target) and identity was re-verified just above.
    if unsafe { libc::kill(pid_t, signal) } != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            return Ok(false);
        }
        return Err(CliError::system(
            "interrupt_signal_failed",
            format!("{} to worker pid {pid} failed: {err}", signal_name(signal)),
        ));
    }
    Ok(true)
}

fn read_both(
    paths: &RunPaths,
    node_id: &NodeId,
) -> Result<(taskfleet_core::Manifest, taskfleet_core::Node), CliError> {
    let manifest = read_manifest_opt(paths)
        .map_err(from_core)?
        .ok_or_else(|| {
            CliError::user("run_not_found", format!("run {} not found", paths.run_id))
        })?;
    let node = read_node_opt(paths, node_id)
        .map_err(from_core)?
        .ok_or_else(|| {
            CliError::user("node_not_found", format!("node {node_id} not found"))
                .with_invalid_value(node_id.as_str())
        })?;
    Ok((manifest, node))
}

/// What the wait observed about attempt `attempt` of the node.
enum AttemptState {
    /// A told exit, a newer attempt, or a terminal node.
    Ended,
    /// No told exit yet, but the recorded process is provably gone.
    Gone,
    Running,
}

fn attempt_state(
    paths: &RunPaths,
    node_id: &NodeId,
    attempt: u32,
) -> Result<AttemptState, CliError> {
    let node = RunLock::with_shared_lock(&paths.lock(), || read_node_opt(paths, node_id))
        .map_err(from_core)?;
    let Some(node) = node else {
        return Ok(AttemptState::Ended);
    };
    if node.status.is_terminal() || node.retry_attempts != attempt || node.worker_exit.is_some() {
        return Ok(AttemptState::Ended);
    }
    Ok(match classify_worker(&node) {
        WorkerState::Gone => AttemptState::Gone,
        _ => AttemptState::Running,
    })
}

pub fn run(args: Args<'_>) -> Result<(), CliError> {
    let root = crate::home::root_dir()?;
    let paths = resident::resolve_run_ref(&root, &args.run)?;
    let node_id = parse_node_id(&args.node)?;
    let run_id = paths.run_id.as_str().to_string();
    {
        let manifest = read_manifest_opt(&paths)
            .map_err(from_core)?
            .ok_or_else(|| CliError::user("run_not_found", format!("run {run_id} not found")))?;
        if manifest.resident.is_none() {
            return Err(CliError::user(
                "not_resident",
                format!(
                    "run {run_id} is not a resident run; stop an ordinary run with `run cancel`"
                ),
            )
            .with_invalid_value(&run_id));
        }
        crate::run::require_writer_fence(&manifest)?;
    }

    let resident_lock = ResidentLock::acquire(&paths, Duration::from_secs(30))?;
    let guard = RunLock::acquire_existing(&paths.lock()).map_err(from_core)?;
    let (manifest, node) = read_both(&paths, &node_id)?;
    let policy = manifest
        .resident
        .clone()
        .expect("checked above; never removed");
    let harness = manifest
        .agent_selection
        .as_ref()
        .map_or_else(String::new, |a| a.selected.harness.clone());
    if manifest.status.is_terminal() || node.status.is_terminal() {
        return Err(CliError::user(
            "worker_not_active",
            format!("run {run_id} is terminal; there is no round to interrupt"),
        )
        .with_invalid_value(&run_id));
    }
    let attempt = node.retry_attempts;
    let open = node
        .resident
        .as_deref()
        .and_then(ResidentState::open_round)
        .filter(|r| r.attempt == attempt)
        .cloned();
    let worker = classify_worker(&node);
    let name = policy.name.as_str();
    let mut payload = Payload {
        run_id: &run_id,
        node_id: node_id.as_str(),
        resident_name: name,
        harness: &harness,
        attempt,
        outcome: "interrupted",
        round: open.as_ref().filter(|r| r.interrupted).map(|r| r.round),
        worker_state: worker.wire(),
        signals: Vec::new(),
        event_seq: None,
        next: format!(
            "the worker is between rounds: `taskfleet run message {name} <text>` resumes the same \
             session, `taskfleet run land {name}` lands its committed work"
        ),
        dry_run: args.dry_run.then_some(true),
    };

    // Nothing to stop: the current attempt has exited, or the worker ended its
    // own round and is shutting down by itself.
    let no_op = if node.worker_exit.is_some() {
        Some("between-rounds")
    } else if open.as_ref().is_some_and(|r| !r.interrupted) {
        Some("round-reported")
    } else {
        None
    };
    if let Some(outcome) = no_op {
        drop(guard);
        drop(resident_lock);
        payload.outcome = outcome;
        payload.round = open.map(|r| r.round);
        if outcome == "round-reported" {
            payload.next = format!(
                "the worker already reported its round and is stopping; wait with `taskfleet run \
                 wait {name}`, then message or land it"
            );
        }
        return emit(&payload, args.spec, args.warnings);
    }

    let pid = match worker {
        WorkerState::Live { pid } => Some(pid),
        // Provably gone without a told exit (its shim was lost too); `Exited`
        // implies a recorded exit, handled above. Record, signal nothing.
        WorkerState::Gone | WorkerState::Exited => None,
        WorkerState::NoPid => {
            return Err(CliError::user(
                "worker_not_started",
                format!(
                    "the worker of run {run_id} has no recorded process yet (still starting); \
                     retry shortly"
                ),
            )
            .with_invalid_value(&run_id));
        }
        WorkerState::Unverifiable { pid } => {
            return Err(CliError::user(
                "worker_unverifiable",
                format!(
                    "worker pid {pid} of run {run_id} is alive but its start identity cannot be \
                     verified; refusing to signal or record an interrupt (inspect it with \
                     `taskfleet run attach {run_id}`)"
                ),
            )
            .with_invalid_value(&run_id));
        }
    };
    let first_signal = if open.is_some() || harness != "claude" {
        // A repeated interrupt escalates; Pi's graceful stop is SIGTERM.
        libc::SIGTERM
    } else {
        libc::SIGINT
    };

    if args.dry_run {
        drop(guard);
        drop(resident_lock);
        payload.outcome = "would-interrupt";
        payload.round = Some(open.as_ref().map_or_else(
            || {
                node.resident
                    .as_deref()
                    .map_or(0, |s| s.rounds_reported)
                    .saturating_add(1)
            },
            |r| r.round,
        ));
        if pid.is_some() {
            payload.signals.push(signal_name(first_signal));
        }
        return emit(&payload, args.spec, args.warnings);
    }

    // 1. The told fact first (see the module doc for why this order).
    if open.is_none() {
        let seq = append_and_apply_unlocked(
            &guard.witness(),
            &paths,
            "node.round_reported",
            Some(&node_id),
            None,
            json!({
                "attempt": attempt,
                "summary": INTERRUPTED_SUMMARY,
                "interrupted": true,
                "interrupted_by": std::env::var("USER").unwrap_or_else(|_| "unknown".into()),
                "interrupted_by_uid": unsafe { libc::geteuid() },
            }),
        )
        .map_err(from_core)?;
        payload.event_seq = Some(seq);
    }
    let recorded = read_node_opt(&paths, &node_id).map_err(from_core)?;
    payload.round = recorded
        .as_ref()
        .and_then(|n| n.resident.as_deref())
        .and_then(ResidentState::open_round)
        .map(|r| r.round);
    let expected = watchdog::RecordedStart::of_agent(&node);
    drop(guard);
    let mut warnings = args.warnings.to_vec();
    if harness == "pi" {
        // Same marker `node round-report` writes: should Pi survive the signal,
        // the bundled adapter shuts it down at its next settled boundary.
        let marker = resident::round_marker_path(&root, &run_id, attempt);
        if let Err(e) = std::fs::write(&marker, b"") {
            warnings.push(format!(
                "Pi stop marker {} could not be written ({e})",
                marker.display()
            ));
        }
    }

    // 2. Signal the verified recorded process. The round is already recorded,
    // so a failure here leaves a waiting round whose exit is still pending.
    let recorded_details = json!({"round_recorded": true, "round": payload.round});
    if let Some(pid) = pid {
        if signal_verified(pid, &expected, first_signal)
            .map_err(|e| e.with_details(recorded_details.clone()))?
        {
            payload.signals.push(signal_name(first_signal));
        }
    }
    drop(resident_lock);

    // 3. Wait for the told exit, escalating a Claude SIGINT once.
    let started = Instant::now();
    let mut gone_since: Option<Instant> = None;
    loop {
        match attempt_state(&paths, &node_id, attempt)? {
            AttemptState::Ended => break,
            AttemptState::Gone => {
                // Prefer the shim's told exit; accept a provably gone worker
                // without one only after a grace (the round is recorded, so
                // the next message treats it as ended as well).
                if gone_since.get_or_insert_with(Instant::now).elapsed() >= GONE_GRACE {
                    break;
                }
            }
            AttemptState::Running => gone_since = None,
        }
        if let Some(pid) =
            pid.filter(|_| payload.signals == ["SIGINT"] && started.elapsed() >= ESCALATE_AFTER)
        {
            if signal_verified(pid, &expected, libc::SIGTERM)
                .map_err(|e| e.with_details(recorded_details.clone()))?
            {
                payload.signals.push("SIGTERM");
            }
        }
        if started.elapsed() >= args.wait_exit {
            return Err(CliError::user(
                "worker_still_running",
                format!(
                    "the interrupted round of run {run_id} is recorded, but its worker has not \
                     exited within {}s; re-run `taskfleet run interrupt {run_id}` to signal it \
                     again, or inspect it with `taskfleet run attach {run_id}`",
                    args.wait_exit.as_secs()
                ),
            )
            .with_invalid_value(&run_id)
            .with_details(json!({
                "round_recorded": true,
                "round": payload.round,
                "signals": payload.signals,
            })));
        }
        std::thread::sleep(POLL);
    }
    emit(&payload, args.spec, &warnings)
}

fn emit(payload: &Payload<'_>, spec: &OutputSpec, warnings: &[String]) -> Result<(), CliError> {
    match spec.format {
        OutputFormat::Text => {
            let round = payload
                .round
                .map_or_else(String::new, |r| format!(" round {r}"));
            println!(
                "{} ({}){round}: {}; {}",
                payload.resident_name, payload.run_id, payload.outcome, payload.next
            );
            output::emit_text_warnings(warnings);
            Ok(())
        }
        _ => output::emit_envelope(payload, spec, warnings),
    }
}
