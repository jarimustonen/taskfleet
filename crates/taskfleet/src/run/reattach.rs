//! `run reattach` — restart the supervisor for an existing run.
//!
//! Refuses if `<run-dir>/supervisor.pid` is still alive (use `run
//! cancel` or kill the existing supervisor instead). Otherwise: emits
//! `supervisor.reattached`, fork+exec a new `taskfleet supervise
//! <run-id>` with stdout/stderr redirected to
//! `<run-dir>/supervisor.stderr.log`, and waits briefly for the new
//! supervisor's PID file to appear.

use serde::Serialize;
use serde_json::json;

use taskfleet_core::{append_and_apply_event, read_manifest_opt};

use crate::error::CliError;
use crate::output::{self, OutputFormat, OutputSpec};
use crate::run::supervisor_spawn;
use crate::run::{from_core, run_paths_from_cli_arg};
use crate::supervise::pid_file;

#[derive(Serialize)]
struct ReattachPayload<'a> {
    run_id: &'a str,
    action: &'static str,
    supervisor_pid: u32,
}

pub fn run(
    run_id: &str,
    once: bool,
    max_iter: Option<u32>,
    spec: &OutputSpec,
    warnings: &[String],
) -> Result<(), CliError> {
    let root = crate::home::root_dir()?;
    let paths = run_paths_from_cli_arg(&root, run_id)?;
    if read_manifest_opt(&paths).map_err(from_core)?.is_none() {
        return Err(
            CliError::user("run_not_found", format!("no run with id {run_id}"))
                .with_invalid_value(run_id),
        );
    }

    let recorded_pid = spawn_supervisor(&paths, run_id, once, max_iter)?;

    let payload = ReattachPayload {
        run_id,
        action: "reattached",
        supervisor_pid: recorded_pid,
    };
    match spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => {
            output::emit_envelope(&payload, spec, warnings)?;
        }
        OutputFormat::Text => {
            println!("reattached run {run_id} (supervisor pid {recorded_pid})");
            output::emit_text_warnings(warnings);
        }
    }
    Ok(())
}

/// Restart the run's supervisor: refuse if one is already alive, record the
/// dead prior incarnation + the reattach request, fork+exec a fully-detached
/// `taskfleet supervise <run-id>`, and return the PID the new supervisor
/// recorded for itself (`0` if the PID file did not appear before the deadline
/// — "spawned, pid unconfirmed"; the supervisor's own `supervisor.pid` remains
/// the source of truth).
///
/// Extracted from [`run`] so `run merge` can reuse the exact same recovery when
/// it finds the recorded supervisor dead — restoring the invariant that a live
/// supervisor consumes the terminal report and tears the worktree down, rather
/// than leaving it orphaned. Errors with `supervisor_already_running` if a live
/// supervisor exists (the caller treats that as "a consumer already exists").
///
/// The caller must have already confirmed the run's manifest exists.
pub fn spawn_supervisor(
    paths: &taskfleet_core::RunPaths,
    run_id: &str,
    once: bool,
    max_iter: Option<u32>,
) -> Result<u32, CliError> {
    // Also covers the merge recovery caller. Do not wait for admission while
    // holding a per-run lock (both callers release read locks before entry).
    let root = crate::home::root_dir()?;
    let _admission = super::admission::admit(&root)?;
    let pid_path = paths.supervisor_pid();
    if let Some((existing, start_time)) = pid_file::read_pid_record(&pid_path) {
        if pid_file::pid_live_with_identity(existing, start_time) {
            return Err(CliError::system(
                "supervisor_already_running",
                format!("supervisor pid {existing} for run {run_id} is alive (no reattach needed)"),
            ));
        }
        // Stale PID file (dead or recycled PID): record the dead prior
        // incarnation. Keyed on the dead pid + its recorded start-time so a
        // retried recovery (e.g. `run merge` re-invoked several times while the
        // supervisor stays dead) collapses to a single `supervisor.exited`
        // instead of storming the log with one per attempt.
        let exited_key = match start_time {
            Some(st) => format!("stale-on-reattach:{existing}:{st}"),
            None => format!("stale-on-reattach:{existing}"),
        };
        let _ = append_and_apply_event(
            paths,
            "supervisor.exited",
            None,
            Some(&exited_key),
            json!({"pid": existing, "reason": "stale-on-reattach"}),
        );
    }

    // Record the request, then spawn.
    append_and_apply_event(
        paths,
        "supervisor.reattach-requested",
        None,
        None,
        json!({}),
    )
    .map_err(from_core)?;

    // Fork+exec a fully-detached supervisor (setsid + double-fork; see
    // `supervisor_spawn`). The spawned supervisor performs the atomic
    // `claim_pid_atomic` in its own startup, so even if two reattaches race
    // past the stale-pid pre-check above, exactly one spawned supervisor
    // wins the flock-guarded claim and the loser exits.
    let log_path = paths.root.join("supervisor.stderr.log");
    // `run reattach` is lenient (no readiness-pipe confirmation): it reads the
    // supervisor's own pid file directly, so no readiness fd is threaded in.
    let mut cmd = supervisor_spawn::detached_supervise_command(run_id, &log_path, None)?;
    if once {
        cmd.arg("--once");
    }
    if let Some(n) = max_iter {
        cmd.arg("--max-iter").arg(n.to_string());
    }
    supervisor_spawn::spawn_and_reap(&mut cmd, run_id)?;

    // Wait briefly for the new supervisor to write its own PID file. We may
    // see the PID it claimed or — if a human or test reattach raced us — a
    // different one; either way the contract is that *some* live supervisor
    // now owns the run. With the double-fork we have no usable spawned PID to
    // report on timeout, so report 0 ("spawned, pid unconfirmed") rather than
    // a stale/dead value; the supervisor's own supervisor.pid is the truth.
    let recorded_pid = supervisor_spawn::await_recorded_pid(paths).unwrap_or(0);

    let _ = append_and_apply_event(
        paths,
        "supervisor.reattached",
        None,
        None,
        json!({"pid": recorded_pid}),
    );

    Ok(recorded_pid)
}
