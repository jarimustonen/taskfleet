//! Terminal-completion notification hook (`run create --notify <cmd>`).
//!
//! When a run reaches a terminal state (`done | failed | cancelled`) the
//! supervisor runs the caller-registered command exactly once, BEFORE the
//! teardown path removes the worktree/window/branch. This is the push signal
//! a spawning session needs so it can learn of completion without polling
//! (issue `no-completion-notification-to-parent`): the parent registers e.g.
//! a line-append to a file its harness watches, a `terminal-notifier` /
//! `notify-send` desktop toast, or a FIFO write.
//!
//! ## At-least-once
//!
//! Firing is deduped on a durable `run.notified` marker event carrying the
//! deterministic idempotency key `supervisor-notify:<run-id>`. Under one
//! exclusive-lock critical section the supervisor scans for the marker; if it
//! is absent it spawns the command FIRST and records the marker only AFTER.
//! So a crash in the window between the spawn and the marker append leaves no
//! marker, and the next supervisor (restart / reattach) re-fires — a duplicate
//! notification. This is the owner's deliberate call (2026-07-24): a missed
//! completion signal defeats the whole feature, so "tell twice" beats "miss
//! it". In the common no-crash case the marker is recorded, so a later tick or
//! a fresh supervisor dedups to exactly one fire; duplicates arise only from an
//! actual crash between spawn and marker.
//!
//! ## Non-blocking
//!
//! The command runs via `sh -c` and the supervisor never `wait`s on it inline,
//! so a slow or hung hook can never wedge the single-threaded supervisor tick.
//! A dedicated reaper thread blocks on the child so a fast hook is collected
//! promptly rather than lingering as a zombie until the supervisor exits (which
//! is unbounded for a cancelled parent that keeps ticking until its children
//! settle).

use serde_json::{json, Value};
use tracing::{info, warn};

use taskfleet_core::{
    append_and_apply_unlocked, find_prior_with_key, read_manifest_opt, read_node_opt,
    AwaitingInput, NodeId, RunLock, RunPaths, Status,
};

use crate::run::from_core;

/// Reporting node whose terminal `node.report` carries the run's outcome
/// summary. Every single-worker worktree kind has exactly one node
/// (`n-0001`); mirrors `run wait`'s and `run merge`'s `DEFAULT_NODE_ID`.
const DEFAULT_NODE_ID: &str = "n-0001";

/// Cap on the `TASKFLEET_SUMMARY` env value (bytes-ish, counted in chars). A
/// `node.report` summary is arbitrary agent-authored text; an unbounded value
/// risks `E2BIG` at `spawn` time, which would drop that fire's notification
/// (the marker is still recorded afterwards, so it is not retried). Bounded
/// well under the platform `ARG_MAX`/env ceiling with headroom for the rest of
/// the environment.
const SUMMARY_MAX_CHARS: usize = 4096;
/// Cap on the `TASKFLEET_RUN_TITLE` env value; a title is short by construction, but
/// bound it defensively for the same reason.
const TITLE_MAX_CHARS: usize = 512;

/// Fire the run's `--notify` hook once, if one is registered and this is the
/// first time the run is observed terminal.
///
/// `status` is the run's terminal manifest status; `kind`/`title` are surfaced
/// to the hook for a richer message. Best-effort throughout: any failure is
/// logged and swallowed — a broken notification must never block teardown or
/// crash the supervisor.
///
/// # Return
///
/// `true` when there is nothing left to do for this run — no hook was
/// registered, the hook was already fired-and-recorded (by this process on an
/// earlier tick or by a prior supervisor), or the hook was spawned just now.
/// `false` only when the exclusive-lock critical section could not be entered
/// (lock contention) or the marker scan failed transiently (I/O): the caller
/// keeps the run's `notified` flag unset so a later tick retries.
#[must_use]
pub fn maybe_fire(
    paths: &RunPaths,
    run_id: &str,
    notify_cmd: Option<&str>,
    status: Status,
    kind: &str,
    title: &str,
) -> bool {
    let Some(cmd) = notify_cmd else {
        return true;
    };
    // Defensive: never fire (and never record the marker) for a non-terminal
    // status. The sole caller guards on `status.is_terminal()`, but a future
    // caller that forgets to must not be able to fire on a still-running run.
    if !status.is_terminal() {
        return true;
    }

    // Read the summary under its own (shared) lock BEFORE we take the exclusive
    // lock below — flock is not reentrant within a process, so nesting would
    // deadlock.
    let summary = env_safe(&read_summary(paths).unwrap_or_default(), SUMMARY_MAX_CHARS);
    let title = env_safe(title, TITLE_MAX_CHARS);
    let status_str = status_kebab(status);
    let key = format!("supervisor-notify:{run_id}");

    // One exclusive-lock critical section: scan for the marker, and only if it
    // is absent spawn the hook and THEN record the marker. Spawn-before-record
    // is what makes this at-least-once — a crash after the spawn but before the
    // append leaves no marker, so a later supervisor re-fires (a duplicate,
    // which the owner prefers over a missed notification). The scan + append
    // share the one lock so two supervisors can't both spawn (the exclusive
    // lock serialises them; the loser sees the marker and skips).
    let guard = match RunLock::acquire(&paths.lock()) {
        Ok(g) => g,
        Err(e) => {
            warn!(
                target: "taskfleet::supervise",
                run_id = %run_id,
                error = %e,
                "could not lock run to fire notify hook; will retry on a later tick"
            );
            return false;
        }
    };
    let lock = guard.witness();
    match find_prior_with_key(&lock, paths, "run.notified", &key) {
        Ok(Some(_)) => {
            // A prior process already spawned-and-recorded. Dedup: skip.
            drop(guard);
            return true;
        }
        Ok(None) => { /* no marker yet — fire below */ }
        Err(e) => {
            warn!(
                target: "taskfleet::supervise",
                run_id = %run_id,
                error = %e,
                "could not scan for run.notified marker; will retry on a later tick"
            );
            drop(guard);
            return false;
        }
    }

    // Fire FIRST, record SECOND (at-least-once ordering).
    spawn_hook(cmd, run_id, status_str, &summary, kind, &title);
    if let Err(e) = append_and_apply_unlocked(
        &lock,
        paths,
        "run.notified",
        None,
        Some(&key),
        json!({ "status": status_str }),
    ) {
        // The hook already fired. Failing to record the marker only means a
        // restart may re-fire (at-least-once tolerates that). Surface it.
        warn!(
            target: "taskfleet::supervise",
            run_id = %run_id,
            error = %e,
            "notify hook fired but recording the run.notified marker failed (a restart may re-fire)"
        );
    }
    drop(guard);
    true
}

/// Fire the same registered hook for an unresolved human-decision request once
/// its grace window has elapsed. The marker key includes the opening event seq,
/// so resolve-then-reopen generations notify independently. The exclusive-lock
/// re-read prevents a resolve racing the supervisor tick from producing a stale
/// page.
#[must_use]
pub fn maybe_fire_awaiting_input(
    paths: &RunPaths,
    run_id: &str,
    notify_cmd: Option<&str>,
    candidate: &AwaitingInput,
    kind: &str,
    title: &str,
) -> bool {
    let Some(cmd) = notify_cmd else {
        return true;
    };
    if !crate::run::awaiting_input::is_escalated(candidate.opened_at, chrono::Utc::now()) {
        // Not settled: caller must keep this generation eligible on later ticks.
        return false;
    }
    let guard = match RunLock::acquire(&paths.lock()) {
        Ok(g) => g,
        Err(e) => {
            warn!(target: "taskfleet::supervise", run_id = %run_id, error = %e,
                "could not lock run to fire awaiting-input hook; will retry");
            return false;
        }
    };
    let lock = guard.witness();
    let node_id = NodeId::parse_str(DEFAULT_NODE_ID).expect("valid default node id");
    let manifest_terminal = match read_manifest_opt(paths) {
        Ok(Some(m)) => m.status.is_terminal(),
        Ok(None) => return true,
        Err(e) => {
            warn!(target: "taskfleet::supervise", run_id = %run_id, error = %e,
                "could not re-read manifest for awaiting-input notification; will retry");
            return false;
        }
    };
    if manifest_terminal {
        return true;
    }
    let fresh = match read_node_opt(paths, &node_id) {
        Ok(Some(n)) if !n.status.is_terminal() && n.worker_exit.is_none() => n.awaiting_input,
        Ok(Some(_) | None) => None,
        Err(e) => {
            warn!(target: "taskfleet::supervise", run_id = %run_id, error = %e,
                "could not re-read awaiting-input state; will retry");
            return false;
        }
    };
    let Some(fresh) = fresh.filter(|v| v.event_seq == candidate.event_seq) else {
        return true;
    };
    if !crate::run::awaiting_input::is_escalated(fresh.opened_at, chrono::Utc::now()) {
        return true;
    }
    let key = format!("supervisor-awaiting-input:{run_id}:{}", fresh.event_seq);
    match find_prior_with_key(&lock, paths, "run.awaiting_input_notified", &key) {
        Ok(Some(_)) => return true,
        Ok(None) => {}
        Err(e) => {
            warn!(target: "taskfleet::supervise", run_id = %run_id, error = %e,
                "could not scan awaiting-input notification marker; will retry");
            return false;
        }
    }
    let details = serde_json::to_string(&fresh.discussion_items).unwrap_or_default();
    let summary = fresh
        .discussion_items
        .first()
        .and_then(|v| v.get("topic"))
        .and_then(Value::as_str)
        .unwrap_or("human decision required");
    if !spawn_awaiting_hook(cmd, run_id, summary, &details, kind, title) {
        return false;
    }
    if let Err(e) = append_and_apply_unlocked(
        &lock,
        paths,
        "run.awaiting_input_notified",
        None,
        Some(&key),
        json!({ "event_seq": fresh.event_seq }),
    ) {
        warn!(target: "taskfleet::supervise", run_id = %run_id, error = %e,
            "awaiting-input hook fired but marker append failed (a restart may re-fire)");
        return false;
    }
    true
}

/// Fire the registered hook once per resident round report (ADR 0003) with
/// `TASKFLEET_STATUS=round-reported` and the round summary, so a parent that
/// listens on `--notify` wakes when the round ends. Non-terminal; deduped on a
/// `run.round_notified` marker keyed by the report's event seq (at-least-once,
/// like the other hooks). Returns `true` when this report needs no further
/// attempts.
#[must_use]
pub fn maybe_fire_round_reported(
    paths: &RunPaths,
    run_id: &str,
    notify_cmd: Option<&str>,
    round_seq: u64,
    kind: &str,
    title: &str,
) -> bool {
    let Some(cmd) = notify_cmd else {
        return true;
    };
    let guard = match RunLock::acquire(&paths.lock()) {
        Ok(g) => g,
        Err(e) => {
            warn!(target: "taskfleet::supervise", run_id = %run_id, error = %e,
                "could not lock run to fire round-reported hook; will retry");
            return false;
        }
    };
    let lock = guard.witness();
    let node_id = NodeId::parse_str(DEFAULT_NODE_ID).expect("valid default node id");
    let open = match read_node_opt(paths, &node_id) {
        Ok(Some(n)) if !n.status.is_terminal() => n
            .resident
            .as_deref()
            .and_then(taskfleet_core::ResidentState::open_round)
            .filter(|r| r.event_seq == round_seq)
            .cloned(),
        Ok(_) => None,
        Err(e) => {
            warn!(target: "taskfleet::supervise", run_id = %run_id, error = %e,
                "could not re-read round state; will retry");
            return false;
        }
    };
    let Some(open) = open else {
        return true;
    };
    let key = format!("supervisor-round-reported:{run_id}:{round_seq}");
    match find_prior_with_key(&lock, paths, "run.round_notified", &key) {
        Ok(Some(_)) => return true,
        Ok(None) => {}
        Err(e) => {
            warn!(target: "taskfleet::supervise", run_id = %run_id, error = %e,
                "could not scan round notification marker; will retry");
            return false;
        }
    }
    let mut command = std::process::Command::new("sh");
    command
        .arg("-c")
        .arg(cmd)
        .env("TASKFLEET_RUN_ID", run_id)
        .env("TASKFLEET_STATUS", "round-reported")
        .env(
            "TASKFLEET_SUMMARY",
            env_safe(&open.summary, SUMMARY_MAX_CHARS),
        )
        .env("TASKFLEET_RUN_KIND", kind)
        .env("TASKFLEET_RUN_TITLE", env_safe(title, TITLE_MAX_CHARS))
        .env("TASKFLEET_ROUND", open.round.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match command.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => {
            warn!(target: "taskfleet::supervise", run_id = %run_id, error = %e,
                "round-reported notify hook failed to spawn; will retry");
            return false;
        }
    }
    if let Err(e) = append_and_apply_unlocked(
        &lock,
        paths,
        "run.round_notified",
        None,
        Some(&key),
        json!({ "event_seq": round_seq, "round": open.round }),
    ) {
        warn!(target: "taskfleet::supervise", run_id = %run_id, error = %e,
            "round-reported hook fired but marker append failed (a restart may re-fire)");
        return false;
    }
    true
}

fn spawn_awaiting_hook(
    cmd: &str,
    run_id: &str,
    summary: &str,
    details: &str,
    kind: &str,
    title: &str,
) -> bool {
    let mut command = std::process::Command::new("sh");
    command
        .arg("-c")
        .arg(cmd)
        .env("TASKFLEET_RUN_ID", run_id)
        .env("TASKFLEET_STATUS", "awaiting-input")
        .env("TASKFLEET_SUMMARY", env_safe(summary, SUMMARY_MAX_CHARS))
        .env("TASKFLEET_RUN_KIND", kind)
        .env("TASKFLEET_RUN_TITLE", env_safe(title, TITLE_MAX_CHARS))
        .env("TASKFLEET_AWAITING_INPUT", "1")
        // Reducer bounds keep this comfortably below environment limits. Never
        // truncate a variable advertised as JSON mid-document.
        .env("TASKFLEET_AWAITING_INPUT_JSON", details)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match command.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            info!(target: "taskfleet::supervise", run_id = %run_id,
                "fired awaiting-input notify hook");
            true
        }
        Err(e) => {
            warn!(target: "taskfleet::supervise", run_id = %run_id, error = %e,
                "awaiting-input notify hook failed to spawn; will retry");
            false
        }
    }
}

/// Spawn `sh -c <cmd>` with the completion context in its environment, and
/// reap it on a detached thread so a fast hook never lingers as a zombie.
///
/// The supervisor is single-threaded and must not block on the hook — a hung
/// command cannot stall the tick — so we do not `wait` inline. But a plain
/// `spawn` + drop of the `Child` does NOT reap: the finished `sh` would sit as
/// a zombie until the supervisor process exits, which is unbounded for a
/// cancelled parent that keeps ticking until its children settle. A dedicated
/// thread that blocks on `child.wait()` reaps it promptly while blocking only
/// itself. Stdout/stderr go to `/dev/null`: the supervisor's own stderr fd can
/// be closed out from under a still-running detached hook (→ `EBADF`/`SIGPIPE`
/// mid-write), and a hook that wants output routes it itself (`>> file 2>&1`).
fn spawn_hook(cmd: &str, run_id: &str, status: &str, summary: &str, kind: &str, title: &str) {
    let mut command = std::process::Command::new("sh");
    command
        .arg("-c")
        .arg(cmd)
        .env("TASKFLEET_RUN_ID", run_id)
        .env("TASKFLEET_STATUS", status)
        .env("TASKFLEET_SUMMARY", summary)
        .env("TASKFLEET_RUN_KIND", kind)
        .env("TASKFLEET_RUN_TITLE", title)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match command.spawn() {
        Ok(child) => {
            // Reap asynchronously: the thread outlives the tick but not the
            // hook, and never blocks the supervisor loop.
            std::thread::spawn(move || {
                let mut child = child;
                let _ = child.wait();
            });
            info!(
                target: "taskfleet::supervise",
                run_id = %run_id,
                status = %status,
                "fired run completion notify hook"
            );
        }
        Err(e) => {
            // The marker is already durably recorded, so we will NOT retry —
            // at-most-once holds. Surface the spawn failure loudly.
            warn!(
                target: "taskfleet::supervise",
                run_id = %run_id,
                error = %e,
                "run completion notify hook failed to spawn (not retried; marker already recorded)"
            );
        }
    }
}

/// Sanitize a string for use as a process environment value: drop NUL bytes
/// (which `Command::env` rejects — a NUL in an agent-authored summary would
/// otherwise fail `spawn` *after* the durable marker, silently dropping the
/// notification) and bound the length so an oversized value can't trip
/// `E2BIG`. Truncation is on a char boundary with an ellipsis marker.
fn env_safe(s: &str, max_chars: usize) -> String {
    let filtered: String = s.chars().filter(|&c| c != '\0').collect();
    if filtered.chars().count() > max_chars {
        let mut out: String = filtered.chars().take(max_chars).collect();
        out.push('…');
        out
    } else {
        filtered
    }
}

/// Read the primary node's terminal `node.report` summary under the shared
/// lock. `None` when there is no node / no report / no `summary` field.
fn read_summary(paths: &RunPaths) -> Option<String> {
    let node_id = NodeId::parse_str(DEFAULT_NODE_ID).expect("DEFAULT_NODE_ID is a valid node id");
    RunLock::with_shared_lock(&paths.lock(), || {
        Ok(read_node_opt(paths, &node_id)?.and_then(|n| n.last_report))
    })
    .map_err(from_core)
    .ok()
    .flatten()
    .as_ref()
    .and_then(|r: &Value| r.get("summary"))
    .and_then(Value::as_str)
    .map(str::to_string)
}

/// Terminal status → the kebab wire string the hook receives in `TASKFLEET_STATUS`.
/// Only terminal statuses reach here; a non-terminal value is defensively
/// rendered rather than panicking.
fn status_kebab(status: Status) -> &'static str {
    match status {
        Status::Done => "done",
        Status::Failed => "failed",
        Status::Cancelled => "cancelled",
        Status::Pending => "pending",
        Status::Running => "running",
        Status::Blocked => "blocked",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use taskfleet_core::{append_and_apply_event, write_node};
    use tempfile::TempDir;

    const RID: &str = "01jxwd0000000000000000000w";

    /// Build a terminal single-node run with a `node.report` summary. Returns
    /// the run's `RunPaths`. The manifest ends up `done`.
    fn terminal_run(tmp: &TempDir, summary: &str) -> RunPaths {
        let dir = tmp.path().join(RID);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = RunPaths::new(dir, RID).unwrap();
        append_and_apply_event(
            &paths,
            "run.created",
            None,
            None,
            json!({ "kind": "spinoff", "lifecycle": "autonomous", "title": "t" }),
        )
        .unwrap();
        append_and_apply_event(
            &paths,
            "node.created",
            Some(&NodeId::parse_str(DEFAULT_NODE_ID).unwrap()),
            None,
            json!({ "kind": "spinoff" }),
        )
        .unwrap();
        append_and_apply_event(
            &paths,
            "node.report",
            Some(&NodeId::parse_str(DEFAULT_NODE_ID).unwrap()),
            None,
            json!({
                "success": true,
                "failed": false,
                "cancelled": false,
                "summary": summary,
                "discussion_items": [],
                "spinoff_proposals": [],
                "wrap_up_recommendations": [],
            }),
        )
        .unwrap();
        append_and_apply_event(
            &paths,
            "run.status",
            None,
            None,
            json!({ "status": "done" }),
        )
        .unwrap();
        paths
    }

    /// Count `run.notified` events in the log, matching the parsed event
    /// `kind` (not a substring, which a data payload could false-match).
    fn notified_count(paths: &RunPaths) -> usize {
        std::fs::read_to_string(paths.events())
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|v| v.get("kind").and_then(Value::as_str) == Some("run.notified"))
            .count()
    }

    /// Poll until `path` holds exactly `expected`, up to ~3s.
    ///
    /// The detached hook runs `sh -c '… > file'`: the shell's `>` redirect
    /// *creates and truncates* the file to empty before `printf` writes into
    /// it. Waiting on mere existence (or non-emptiness) is a TOCTOU — under
    /// suite load the poller can observe the empty intermediate state and read
    /// `""`. Waiting for the exact expected content closes that window while
    /// still proving the hook delivered the right value.
    fn wait_for_content(path: &std::path::Path, expected: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if std::fs::read_to_string(path).is_ok_and(|got| got == expected) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn fires_hook_with_completion_env() {
        let tmp = TempDir::new().unwrap();
        let paths = terminal_run(&tmp, "did the thing");
        let out = tmp.path().join("hook-out.txt");
        // The hook records the env the supervisor handed it.
        let cmd = format!(
            "printf '%s|%s|%s|%s' \"$TASKFLEET_RUN_ID\" \"$TASKFLEET_STATUS\" \"$TASKFLEET_SUMMARY\" \"$TASKFLEET_RUN_KIND\" > {}",
            out.display()
        );

        assert!(
            maybe_fire(&paths, RID, Some(&cmd), Status::Done, "spinoff", "t"),
            "a fired hook settles the notify state"
        );

        let expected = format!("{RID}|done|did the thing|spinoff");
        assert!(
            wait_for_content(&out, &expected),
            "hook must run and write the completion env into its output file"
        );
        assert_eq!(notified_count(&paths), 1, "exactly one marker recorded");
    }

    #[test]
    fn awaiting_input_hook_fires_after_grace_and_dedups_generation() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(RID);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = RunPaths::new(dir, RID).unwrap();
        append_and_apply_event(
            &paths,
            "run.created",
            None,
            None,
            json!({ "kind": "spinoff", "lifecycle": "autonomous", "title": "t" }),
        )
        .unwrap();
        let nid = NodeId::parse_str(DEFAULT_NODE_ID).unwrap();
        append_and_apply_event(
            &paths,
            "node.created",
            Some(&nid),
            None,
            json!({ "kind": "spinoff" }),
        )
        .unwrap();
        append_and_apply_event(
            &paths,
            "node.awaiting_input",
            Some(&nid),
            None,
            json!({
                "discussion_items": [{
                    "topic": "Choose scope", "options": ["small", "large"],
                    "recommended_default": "small"
                }]
            }),
        )
        .unwrap();
        // Age the durable projection anchor without mutating process-global env.
        // This fixture has no concurrent writer and its applied_seq is current.
        let mut node = read_node_opt(&paths, &nid).unwrap().unwrap();
        node.awaiting_input.as_mut().unwrap().opened_at =
            chrono::Utc::now() - chrono::Duration::minutes(4);
        write_node(&paths, &node).unwrap();
        let open = node.awaiting_input.unwrap();
        let out = tmp.path().join("awaiting.txt");
        let cmd = format!(
            "printf '%s|%s|%s' \"$TASKFLEET_STATUS\" \"$TASKFLEET_SUMMARY\" \"$TASKFLEET_AWAITING_INPUT\" >> {}",
            out.display()
        );

        assert!(maybe_fire_awaiting_input(
            &paths,
            RID,
            Some(&cmd),
            &open,
            "spinoff",
            "t"
        ));
        assert!(wait_for_content(&out, "awaiting-input|Choose scope|1"));
        assert!(maybe_fire_awaiting_input(
            &paths,
            RID,
            Some(&cmd),
            &open,
            "spinoff",
            "t"
        ));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "awaiting-input|Choose scope|1"
        );
    }

    #[test]
    fn repeated_ticks_dedup_via_marker() {
        // The common no-crash path: once the hook fires AND records its marker,
        // later ticks (and a fresh supervisor) see the marker and skip — so a
        // healthy run notifies exactly once even under at-least-once semantics.
        // Duplicates arise only from an actual crash between spawn and marker,
        // which this in-process test cannot stage.
        let tmp = TempDir::new().unwrap();
        let paths = terminal_run(&tmp, "s");
        let counter = tmp.path().join("counter.txt");
        // Each invocation appends a byte; a second fire would make it 2 bytes.
        let cmd = format!("printf 'x' >> {}", counter.display());

        // First tick fires; a durable marker is recorded (AFTER the spawn).
        assert!(maybe_fire(
            &paths,
            RID,
            Some(&cmd),
            Status::Done,
            "spinoff",
            "t"
        ));
        assert!(wait_for_content(&counter, "x"));
        assert_eq!(notified_count(&paths), 1);

        // Subsequent ticks (and a simulated supervisor restart) see the marker
        // and must NOT re-fire, and each reports "settled".
        assert!(maybe_fire(
            &paths,
            RID,
            Some(&cmd),
            Status::Done,
            "spinoff",
            "t"
        ));
        assert!(maybe_fire(
            &paths,
            RID,
            Some(&cmd),
            Status::Done,
            "spinoff",
            "t"
        ));

        // Give any (erroneously-spawned) second hook time to land before asserting.
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(notified_count(&paths), 1, "marker recorded once");
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap(),
            "x",
            "the hook ran once despite repeated ticks (marker dedup)"
        );
    }

    #[test]
    fn preexisting_marker_suppresses_refire() {
        // At-least-once dedup: a `run.notified` marker already on the log (a
        // prior process that spawned-and-recorded) makes a fresh call skip.
        let tmp = TempDir::new().unwrap();
        let paths = terminal_run(&tmp, "s");
        let key = format!("supervisor-notify:{RID}");
        append_and_apply_event(
            &paths,
            "run.notified",
            None,
            Some(&key),
            json!({ "status": "done" }),
        )
        .unwrap();
        let out = tmp.path().join("should-not-exist");
        let cmd = format!("touch {}", out.display());
        assert!(maybe_fire(
            &paths,
            RID,
            Some(&cmd),
            Status::Done,
            "spinoff",
            "t"
        ));
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            !out.exists(),
            "a pre-existing marker must suppress the hook"
        );
        assert_eq!(notified_count(&paths), 1, "no second marker recorded");
    }

    #[test]
    fn records_marker_after_firing() {
        // Ordering check underpinning at-least-once: after a fire the marker
        // exists — so a crash BEFORE this point (no marker) would re-fire.
        let tmp = TempDir::new().unwrap();
        let paths = terminal_run(&tmp, "s");
        assert_eq!(notified_count(&paths), 0, "no marker before firing");
        assert!(maybe_fire(
            &paths,
            RID,
            Some("true"),
            Status::Done,
            "spinoff",
            "t"
        ));
        assert_eq!(notified_count(&paths), 1, "marker recorded after firing");
    }

    #[test]
    fn no_notify_cmd_is_a_noop() {
        let tmp = TempDir::new().unwrap();
        let paths = terminal_run(&tmp, "s");
        assert!(
            maybe_fire(&paths, RID, None, Status::Done, "spinoff", "t"),
            "no hook registered is a settled no-op"
        );
        assert_eq!(
            notified_count(&paths),
            0,
            "a run with no --notify records no marker and runs nothing"
        );
    }

    #[test]
    fn non_terminal_status_never_fires_or_marks() {
        // Defensive guard: a non-terminal status must not poison the durable
        // gate — no marker, no hook, and it reports "settled" so a caller does
        // not spin retrying.
        let tmp = TempDir::new().unwrap();
        let paths = terminal_run(&tmp, "s");
        let out = tmp.path().join("should-not-exist");
        let cmd = format!("touch {}", out.display());
        assert!(maybe_fire(
            &paths,
            RID,
            Some(&cmd),
            Status::Running,
            "spinoff",
            "t"
        ));
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            !out.exists(),
            "hook must not fire for a non-terminal status"
        );
        assert_eq!(
            notified_count(&paths),
            0,
            "no marker for a non-terminal run"
        );
    }

    #[test]
    fn returns_true_on_idempotent_replay() {
        let tmp = TempDir::new().unwrap();
        let paths = terminal_run(&tmp, "s");
        let cmd = "true".to_string();
        assert!(maybe_fire(
            &paths,
            RID,
            Some(&cmd),
            Status::Done,
            "spinoff",
            "t"
        ));
        // Second call sees the marker → idempotent replay → settled, no re-fire.
        assert!(
            maybe_fire(&paths, RID, Some(&cmd), Status::Done, "spinoff", "t"),
            "an already-fired hook reports settled"
        );
        assert_eq!(notified_count(&paths), 1);
    }

    #[test]
    fn env_safe_strips_nul_and_bounds_length() {
        // NUL is stripped (Command::env would otherwise reject the value).
        assert_eq!(env_safe("a\0b\0c", 100), "abc");
        // Under the cap is passed through unchanged.
        assert_eq!(env_safe("short", 100), "short");
        // Over the cap is truncated with an ellipsis marker.
        let long = "x".repeat(50);
        let got = env_safe(&long, 10);
        assert_eq!(got.chars().filter(|&c| c == 'x').count(), 10);
        assert!(got.ends_with('…'));
    }

    #[test]
    fn read_summary_returns_report_summary() {
        let tmp = TempDir::new().unwrap();
        let paths = terminal_run(&tmp, "the summary line");
        assert_eq!(read_summary(&paths).as_deref(), Some("the summary line"));
    }
}
