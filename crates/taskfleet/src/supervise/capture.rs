//! Durable capture of an agent's tmux pane to `<run-dir>/agent.log`.
//!
//! An autonomous worker's stdout/stderr goes ONLY to its tmux pane, which the
//! supervisor kills on cleanup — so a genuine death (hang, API cutoff, crash)
//! left zero trace of the cause once the window was torn down (issue
//! `worker-process-hang`, 2026-07-26 corroboration). This module tees each
//! worker's pane to a durable `agent.log` in the RUN DIR (not the worktree, so
//! it survives `git worktree remove`), via `tmux pipe-pane`, right after the
//! supervisor first observes the node's `tmux_identity`.
//!
//! **Best-effort, non-fatal.** Capture is a diagnostic, never a spawn blocker:
//! if `pipe-pane` cannot be set up (old tmux, missing identity, server down) we
//! warn and continue. The worker runs regardless. Every tmux shell-out is
//! **time-bounded** ([`crate::proc::run_with_timeout`], the same runner the
//! watchdog uses) so a wedged tmux server can never stall the single-threaded
//! supervisor loop — capture runs before the watchdog and must not delay it.
//!
//! **Armed exactly once per node, tracked durably.** `tmux pipe-pane -o` is a
//! *toggle* (an existing pipe is closed, and with `-o` NOT reopened) — so
//! calling it every tick would flap the tee on and off. We instead use plain
//! `pipe-pane` (append, `head -c <cap> >> agent.log`) and arm each node exactly
//! once. The set of armed nodes lives in [`crate::supervise::state::SupervisorState`]
//! (persisted every tick, like `spawned_children`): the `cat`/`head` pipe is a
//! child of the **tmux server**, not of the supervisor, so it survives a
//! supervisor restart. Persisting "already armed" means a restart does NOT
//! re-run `pipe-pane` — the live pipe just keeps appending, with no
//! close/reopen transition gap.
//!
//! **Bounded retry on transient failure.** A node is marked armed only on a
//! *successful* `pipe-pane`. A failure (e.g. tmux still initializing during the
//! spawn-grace window — exactly the node whose startup output we most want)
//! leaves the node un-armed and it is retried on later ticks, up to
//! [`MAX_CAPTURE_ATTEMPTS`], after which we give up so a permanently-broken tmux
//! is not re-probed forever.
//!
//! **Size cap.** The tee is `head -c <cap>`; once the cap is reached `head`
//! exits, tmux closes the pipe on EOF, and capture stops — bounding disk use
//! without disturbing the agent. See [`CAPTURE_MAX_BYTES`].
//!
//! **Buffering.** `head` block-buffers when its stdout is a file, so `agent.log`
//! lags live pane output by up to a buffer and is not suitable for real-time
//! tailing. This is fine for the post-mortem goal: on teardown the supervisor
//! kills the tmux window, the pane's process exits, `head` sees EOF and flushes,
//! and the run-dir `agent.log` holds the complete transcript up to death.
//!
//! **Pane targeting.** We target [`TmuxIdentity::capture_target`]: the stable
//! `pane_id` (`%NN`) recorded at spawn — the agent's OWN pane — when present,
//! else the `window_id` (`pipe-pane -t <window_id>` resolves to the window's
//! *active* pane). Targeting the pane id directly keeps capture pinned to the
//! agent even in an interactive session where the user splits the window and
//! shifts the active pane; without it `agent.log` could capture the user's own
//! shell (issue `capture-agent-pane-by-pane-id`). A run spawned before native materializer
//! emitted `#{pane_id}` has no `pane_id`, so it falls back to the `window_id`
//! target — correct for the single-pane autonomous headless path.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use tracing::{info, warn};

use taskfleet_core::schema::TmuxIdentity;
use taskfleet_core::{read_node_opt, NodeId, RunLock, RunPaths, Status};

use crate::run::from_core;

/// Wall-clock bound on a single `tmux pipe-pane` shell-out. `pipe-pane` returns
/// as soon as it registers the pipe (the `cat`/`head` runs detached under the
/// tmux server), so a healthy call is sub-millisecond; this bound only fires for
/// a wedged server / socket and keeps the supervisor tick from stalling.
#[cfg(not(test))]
const PIPE_PANE_TIMEOUT: Duration = Duration::from_secs(5);
// Unit tests run fake subprocesses in a highly parallel binary; preserve the
// production deadline while allowing for host scheduler starvation in CI.
#[cfg(test)]
const PIPE_PANE_TIMEOUT: Duration = Duration::from_secs(15);

/// stdout/stderr capture cap for the bounded runner. `pipe-pane` emits almost
/// nothing; a small cap is plenty for the occasional error line.
const PIPE_PANE_OUTPUT_CAP: usize = 8 * 1024;

/// Max `pipe-pane` attempts for one node before capture gives up on it. A node
/// is retried each tick while un-armed; a transient failure (tmux still coming
/// up) self-heals within a few ticks, while a permanently-broken tmux stops
/// being probed after this many tries.
const MAX_CAPTURE_ATTEMPTS: u32 = 10;

/// Byte cap on `agent.log` enforced by `head -c`. Generous enough to hold a full
/// heavy-LLM run's pane output for post-mortem, bounded so a runaway logging
/// loop cannot exhaust the disk. `head` exits at the cap; tmux then closes the
/// pipe on EOF and capture stops cleanly.
const CAPTURE_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// The tmux binary, honoring the `TMUX_BIN` override (tests, non-default
/// installs). Mirrors [`crate::supervise::watchdog`] / [`crate::supervise::cleanup`].
fn tmux_bin() -> String {
    std::env::var("TMUX_BIN").unwrap_or_else(|_| "tmux".to_string())
}

/// Ensure each non-terminal worker node with a recorded `tmux_identity` is
/// teeing its pane to `<run-dir>/agent.log`.
///
/// `armed` is the durable set of nodes already successfully piped (persisted in
/// [`SupervisorState`](crate::supervise::state::SupervisorState)); `attempts` is
/// the in-memory per-node failure counter driving bounded retry. Best-effort
/// throughout: scan errors and failed `pipe-pane` calls warn and are swallowed.
pub(crate) fn capture_tick(
    paths: &RunPaths,
    armed: &mut BTreeSet<String>,
    attempts: &mut BTreeMap<String, u32>,
) {
    capture_tick_with(paths, armed, attempts, &tmux_bin());
}

/// [`capture_tick`] with the tmux binary injected, so a test can point it at a
/// stub without touching the process-global `TMUX_BIN`.
fn capture_tick_with(
    paths: &RunPaths,
    armed: &mut BTreeSet<String>,
    attempts: &mut BTreeMap<String, u32>,
    tmux: &str,
) {
    let mut targets: Vec<(String, TmuxIdentity)> = Vec::new();

    // Collect candidate (node_id, identity) pairs under the run's shared lock so
    // a concurrent reducer cannot mutate the projection set under the scan
    // (state-integrity invariant #3). Only path/identity data is read here; the
    // slow `pipe-pane` shell-out runs after the lock is dropped.
    let scan = RunLock::with_shared_lock(&paths.lock(), || {
        let entries = match std::fs::read_dir(paths.nodes_dir()) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(taskfleet_core::Error::io(paths.nodes_dir(), e)),
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Some(node_id) = p.file_stem().and_then(|s| s.to_str()).map(str::to_string) else {
                continue;
            };
            // Already piped, or we have exhausted the retry budget → nothing to do.
            if armed.contains(&node_id)
                || attempts
                    .get(&node_id)
                    .is_some_and(|a| *a >= MAX_CAPTURE_ATTEMPTS)
            {
                continue;
            }
            let Ok(nid) = NodeId::parse_str(&node_id) else {
                continue;
            };
            let Ok(Some(n)) = read_node_opt(paths, &nid) else {
                continue;
            };
            // A terminal node's pane is gone (or about to be); nothing to tee.
            if matches!(n.status, Status::Done | Status::Failed | Status::Cancelled) {
                continue;
            }
            // No qualified identity → nothing to target `pipe-pane` at. Leave it
            // uncaptured so a later `node.created`-driven identity can still be
            // picked up on a subsequent tick.
            let Some(identity) = n.tmux_identity.clone() else {
                continue;
            };
            targets.push((node_id, *identity));
        }
        Ok(())
    });
    if let Err(e) = scan.map_err(from_core) {
        warn!(
            target: "taskfleet::supervise",
            error = %e.message,
            "agent-log capture scan failed (continuing)"
        );
        return;
    }

    let log_path = paths.agent_log();
    for (node_id, identity) in targets {
        let attempt_no = attempts.get(&node_id).copied().unwrap_or(0) + 1;
        if setup_pipe_pane(tmux, &identity, &log_path, &node_id) {
            // Armed successfully: record durably so a restart does not re-run
            // pipe-pane (the tmux-server-owned pipe keeps appending), and stop
            // counting attempts for this node.
            armed.insert(node_id.clone());
            attempts.remove(&node_id);
        } else {
            // Transient (or permanent) failure: keep it un-armed for retry, but
            // bound the retries so a broken tmux is not re-probed forever.
            attempts.insert(node_id, attempt_no);
        }
    }
}

/// Issue a single `tmux [-S <socket>] pipe-pane -O -t <pane_id|window_id>
/// 'head -c <cap> >> <log>'` to tee the node's pane into `log_path`. Returns
/// `true` iff tmux accepted the pipe. Lenient: logs and swallows a non-zero exit,
/// spawn error, or timeout.
fn setup_pipe_pane(tmux: &str, identity: &TmuxIdentity, log_path: &Path, node_id: &str) -> bool {
    let cmd = pipe_pane_command(tmux, identity, log_path);
    match run_pipe_pane(cmd) {
        Ok(()) => {
            info!(
                target: "taskfleet::supervise",
                node = node_id,
                window = %identity.window_id,
                pane = %identity.capture_target(),
                log = %log_path.display(),
                "agent-log capture armed"
            );
            true
        }
        Err(detail) => {
            warn!(
                target: "taskfleet::supervise",
                node = node_id,
                window = %identity.window_id,
                pane = %identity.capture_target(),
                detail = %detail,
                "agent-log capture could not be set up (will retry; continuing without it)"
            );
            false
        }
    }
}

/// Build the `pipe-pane` command that tees the agent's pane
/// ([`TmuxIdentity::capture_target`] — the recorded `pane_id`, else the
/// `window_id`'s active pane) into `log_path` with append + size-capped
/// semantics. Pure so a test can assert the exact argv without a live tmux
/// server.
fn pipe_pane_command(tmux: &str, identity: &TmuxIdentity, log_path: &Path) -> Command {
    let mut cmd = Command::new(tmux);
    if let Some(socket) = identity.socket.as_deref() {
        cmd.args(["-S", socket]);
    }
    // `pipe-pane` with a shell-command and no `-o`: any existing pipe on the pane
    // is closed and a fresh one opened. We never re-run for an armed node (the
    // persisted `armed` set gates that), so this is only ever a first-arm or a
    // retry of a previously-failed arm. `-O` makes the direction explicit
    // (pane output → command stdin) rather than relying on the version-sensitive
    // implicit default, and avoids any `-I` input path. `-t <target>` is the
    // agent's stable `pane_id` when recorded (`%NN`) — pinned to the agent even
    // if the user splits the window — else the `window_id` (its active pane).
    cmd.args(["pipe-pane", "-O", "-t", identity.capture_target()]);
    // `head -c <cap>` bounds the file: once the cap is read `head` exits, tmux
    // closes the pipe on EOF, and capture stops — no unbounded `agent.log`.
    let shell = format!(
        "head -c {CAPTURE_MAX_BYTES} >> {}",
        shell_single_quote(&log_path.to_string_lossy())
    );
    cmd.arg(shell);
    cmd
}

/// Run a `pipe-pane` command under a wall-clock bound, returning `Ok(())` on a
/// clean exit or the captured failure detail otherwise. Bounded via the shared
/// [`crate::proc::run_with_timeout`] runner (the watchdog's) so a wedged tmux
/// server can never stall the single-threaded supervisor tick.
fn run_pipe_pane(cmd: Command) -> Result<(), String> {
    match crate::proc::run_with_timeout(cmd, PIPE_PANE_TIMEOUT, PIPE_PANE_OUTPUT_CAP) {
        crate::proc::TimedOutcome::Exited { status, stderr, .. } if status.success() => {
            let _ = stderr;
            Ok(())
        }
        crate::proc::TimedOutcome::Exited { status, stderr, .. } => {
            let detail = String::from_utf8_lossy(&stderr.bytes).trim().to_string();
            Err(if detail.is_empty() {
                format!("non-zero exit {:?}", status.code())
            } else {
                detail
            })
        }
        crate::proc::TimedOutcome::TimedOut => {
            Err(format!("timed out after {PIPE_PANE_TIMEOUT:?}"))
        }
        crate::proc::TimedOutcome::SpawnErr(e) => Err(format!("spawn failed: {e}")),
    }
}

/// Wrap `s` in single quotes for a POSIX shell, escaping embedded single quotes
/// via the `'\''` idiom. Run dirs live under `~/.taskfleet/runs/<id>` and
/// never contain quotes in practice, but the tee command is handed to `sh -c`
/// by tmux, so we quote defensively.
fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use taskfleet_core::append_and_apply_event;
    use tempfile::TempDir;

    fn id(socket: Option<&str>, window_id: &str) -> TmuxIdentity {
        TmuxIdentity {
            socket: socket.map(str::to_string),
            session: "taskfleet".to_string(),
            window_id: window_id.to_string(),
            pane_id: None,
            server_pid: None,
            server_pid_start_secs: None,
            server_marker: None,
        }
    }

    /// Like [`id`], but with a recorded `pane_id` — the agent's own pane.
    fn id_with_pane(socket: Option<&str>, window_id: &str, pane_id: &str) -> TmuxIdentity {
        TmuxIdentity {
            pane_id: Some(pane_id.to_string()),
            ..id(socket, window_id)
        }
    }

    fn argv(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn command_includes_socket_window_and_append_target() {
        let identity = id(Some("/private/tmp/tmux-501/default"), "@42");
        let log = PathBuf::from("/home/x/.taskfleet/runs/abc/agent.log");
        let cmd = pipe_pane_command("tmux", &identity, &log);
        assert_eq!(cmd.get_program().to_string_lossy(), "tmux");
        assert_eq!(
            argv(&cmd),
            vec![
                "-S".to_string(),
                "/private/tmp/tmux-501/default".to_string(),
                "pipe-pane".to_string(),
                "-O".to_string(),
                "-t".to_string(),
                "@42".to_string(),
                format!(
                    "head -c {CAPTURE_MAX_BYTES} >> \
                     '/home/x/.taskfleet/runs/abc/agent.log'"
                ),
            ]
        );
    }

    #[test]
    fn command_omits_socket_flag_when_absent() {
        let identity = id(None, "@7");
        let log = PathBuf::from("/runs/z/agent.log");
        let cmd = pipe_pane_command("tmux", &identity, &log);
        let args = argv(&cmd);
        assert!(!args.iter().any(|a| a == "-S"), "no -S flag: {args:?}");
        assert_eq!(args[0], "pipe-pane");
        assert_eq!(args[1], "-O");
        assert_eq!(args[2], "-t");
        assert_eq!(args[3], "@7");
        assert_eq!(
            args[4],
            format!("head -c {CAPTURE_MAX_BYTES} >> '/runs/z/agent.log'")
        );
    }

    #[test]
    fn command_targets_recorded_pane_id_over_window_id() {
        // With a recorded pane_id (`%NN`), `pipe-pane -t` targets the agent's
        // OWN pane, not the window's active pane — so a user splitting the
        // window can't shift capture onto their shell.
        let identity = id_with_pane(None, "@42", "%7");
        let log = PathBuf::from("/runs/z/agent.log");
        let cmd = pipe_pane_command("tmux", &identity, &log);
        let args = argv(&cmd);
        assert_eq!(args[2], "-t");
        assert_eq!(args[3], "%7", "targets pane_id, not window_id: {args:?}");
        assert!(
            !args.iter().any(|a| a == "@42"),
            "window_id must not appear as the target when a pane_id is recorded: {args:?}"
        );
    }

    #[test]
    fn command_falls_back_to_window_id_when_pane_id_absent() {
        // A run spawned before native materializer emitted `#{pane_id}` has no pane_id;
        // capture falls back to the window's active pane (`@NNNN`).
        let identity = id(None, "@9");
        let cmd = pipe_pane_command("tmux", &identity, &PathBuf::from("/runs/z/agent.log"));
        let args = argv(&cmd);
        assert_eq!(args[2], "-t");
        assert_eq!(args[3], "@9", "falls back to window_id: {args:?}");
    }

    #[test]
    fn capture_target_prefers_pane_id() {
        assert_eq!(id_with_pane(None, "@42", "%7").capture_target(), "%7");
        assert_eq!(id(None, "@42").capture_target(), "@42");
    }

    #[test]
    fn shell_quote_escapes_embedded_single_quote() {
        assert_eq!(shell_single_quote("a"), "'a'");
        assert_eq!(shell_single_quote("a'b"), "'a'\\''b'");
        assert_eq!(
            shell_single_quote("/tmp/it's here/agent.log"),
            "'/tmp/it'\\''s here/agent.log'"
        );
    }

    #[test]
    fn run_pipe_pane_reports_spawn_failure_leniently() {
        // A nonexistent binary yields a spawn error, surfaced as detail — never
        // a panic (capture is best-effort).
        let cmd = Command::new("/nonexistent/definitely/not/tmux");
        let err = run_pipe_pane(cmd).unwrap_err();
        assert!(err.starts_with("spawn failed:"), "err={err:?}");
    }

    /// A fake `tmux` that logs its argv to `<dir>/tmux.log` and exits 0.
    fn fake_tmux(dir: &Path) -> String {
        use std::os::unix::fs::PermissionsExt as _;
        let p = dir.join("fake-tmux.sh");
        let log = dir.join("tmux.log");
        let script = format!(
            "#!/bin/bash\nprintf '%s ' \"$@\" >> '{log}'\nprintf '\\n' >> '{log}'\nexit 0\n",
            log = log.display(),
        );
        std::fs::write(&p, &script).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.to_str().unwrap().to_string()
    }

    fn fresh_run(tmp: &TempDir) -> RunPaths {
        let run_id = "01jxsnap000000000000000000";
        let dir = tmp.path().join(run_id);
        std::fs::create_dir_all(&dir).unwrap();
        RunPaths::new(dir, run_id).unwrap()
    }

    /// End-to-end scan → dispatch: a node carrying a `tmux_identity` is armed
    /// exactly once, `tmux pipe-pane` is invoked with the run-dir `agent.log`
    /// size-capped append target, and the node lands in `armed` so a second tick
    /// is a no-op (the toggle hazard).
    #[test]
    fn capture_tick_arms_node_once_and_targets_agent_log() {
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
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
            Some(&NodeId::parse_str("n-0001").unwrap()),
            None,
            json!({
                "kind": "spinoff",
                "tmux_socket": "/private/tmp/tmux-501/default",
                "tmux_session": "headless",
                "tmux_window_id": "@42",
            }),
        )
        .unwrap();

        let tmux = fake_tmux(tmp.path());
        let mut armed = BTreeSet::new();
        let mut attempts = BTreeMap::new();

        capture_tick_with(&paths, &mut armed, &mut attempts, &tmux);
        assert!(armed.contains("n-0001"), "node marked armed");
        assert!(
            !attempts.contains_key("n-0001"),
            "success clears the attempt counter"
        );

        let log = std::fs::read_to_string(tmp.path().join("tmux.log")).unwrap();
        let expected_target = format!(
            "head -c {CAPTURE_MAX_BYTES} >> '{}'",
            paths.agent_log().display()
        );
        assert!(
            log.contains("-S /private/tmp/tmux-501/default"),
            "socket: {log:?}"
        );
        assert!(
            log.contains("pipe-pane -O -t @42"),
            "pipe-pane target: {log:?}"
        );
        assert!(log.contains(&expected_target), "append target: {log:?}");

        // Second tick is a no-op: the node is already armed, so no new tmux
        // invocation (which would toggle the tee off).
        let invocations_before = log.lines().count();
        capture_tick_with(&paths, &mut armed, &mut attempts, &tmux);
        let log2 = std::fs::read_to_string(tmp.path().join("tmux.log")).unwrap();
        assert_eq!(
            log2.lines().count(),
            invocations_before,
            "already-armed node must not be re-armed: {log2:?}"
        );
    }

    /// A node whose `node.created` recorded a `tmux_pane_id` is captured by
    /// targeting that pane id (`%NN`), not the window id — the whole point of
    /// the fix. End-to-end through the reducer's identity reconstruction.
    #[test]
    fn capture_tick_targets_recorded_pane_id_end_to_end() {
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
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
            Some(&NodeId::parse_str("n-0001").unwrap()),
            None,
            json!({
                "kind": "spinoff",
                "tmux_session": "headless",
                "tmux_window_id": "@42",
                "tmux_pane_id": "%7",
            }),
        )
        .unwrap();

        let tmux = fake_tmux(tmp.path());
        let mut armed = BTreeSet::new();
        let mut attempts = BTreeMap::new();
        capture_tick_with(&paths, &mut armed, &mut attempts, &tmux);

        assert!(armed.contains("n-0001"), "node marked armed");
        let log = std::fs::read_to_string(tmp.path().join("tmux.log")).unwrap();
        assert!(
            log.contains("pipe-pane -O -t %7"),
            "targets the recorded pane_id: {log:?}"
        );
        assert!(
            !log.contains("-t @42"),
            "window_id must not be the target when a pane_id was recorded: {log:?}"
        );
    }

    /// A terminal node's pane is gone — capture must not arm it.
    #[test]
    fn capture_tick_skips_terminal_node() {
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        append_and_apply_event(
            &paths,
            "run.created",
            None,
            None,
            json!({ "kind": "spinoff", "lifecycle": "autonomous", "title": "t" }),
        )
        .unwrap();
        let node = NodeId::parse_str("n-0001").unwrap();
        append_and_apply_event(
            &paths,
            "node.created",
            Some(&node),
            None,
            json!({ "kind": "spinoff", "tmux_session": "headless", "tmux_window_id": "@42" }),
        )
        .unwrap();
        append_and_apply_event(
            &paths,
            "node.report",
            Some(&node),
            None,
            json!({ "success": true, "via": "explicit-merge" }),
        )
        .unwrap();

        let tmux = fake_tmux(tmp.path());
        let mut armed = BTreeSet::new();
        let mut attempts = BTreeMap::new();
        capture_tick_with(&paths, &mut armed, &mut attempts, &tmux);

        assert!(armed.is_empty(), "terminal node not armed");
        assert!(
            !tmp.path().join("tmux.log").exists(),
            "no tmux invocation for a terminal node"
        );
    }

    /// A node without a `tmux_identity` is left un-armed so a later tick can
    /// still pick it up once the identity is recorded — and it does NOT burn a
    /// retry attempt (it never reached `pipe-pane`).
    #[test]
    fn capture_tick_leaves_node_without_identity_for_retry() {
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
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
            Some(&NodeId::parse_str("n-0001").unwrap()),
            None,
            json!({ "kind": "spinoff" }),
        )
        .unwrap();

        let tmux = fake_tmux(tmp.path());
        let mut armed = BTreeSet::new();
        let mut attempts = BTreeMap::new();
        capture_tick_with(&paths, &mut armed, &mut attempts, &tmux);

        assert!(
            armed.is_empty(),
            "no identity → not armed (retry on a later tick)"
        );
        assert!(
            !attempts.contains_key("n-0001"),
            "a node that never reached pipe-pane must not burn a retry attempt"
        );
    }

    /// A failing `pipe-pane` (non-zero tmux exit) leaves the node un-armed and
    /// increments the attempt counter each tick, then stops probing once the
    /// bounded budget is exhausted — a broken tmux is not re-probed forever.
    #[test]
    fn capture_tick_retries_then_gives_up_on_persistent_failure() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
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
            Some(&NodeId::parse_str("n-0001").unwrap()),
            None,
            json!({ "kind": "spinoff", "tmux_session": "headless", "tmux_window_id": "@42" }),
        )
        .unwrap();

        // A tmux that always fails: logs the invocation and exits 1.
        let failing = tmp.path().join("failing-tmux.sh");
        let log = tmp.path().join("tmux.log");
        std::fs::write(
            &failing,
            format!(
                "#!/bin/bash\nprintf 'call\\n' >> '{}'\necho 'no server' >&2\nexit 1\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tmux = failing.to_str().unwrap();

        let mut armed = BTreeSet::new();
        let mut attempts = BTreeMap::new();

        // Tick until the retry budget is exhausted; then a few more ticks.
        for _ in 0..(MAX_CAPTURE_ATTEMPTS + 3) {
            capture_tick_with(&paths, &mut armed, &mut attempts, tmux);
        }

        assert!(armed.is_empty(), "persistent failure never arms");
        assert_eq!(
            attempts.get("n-0001").copied(),
            Some(MAX_CAPTURE_ATTEMPTS),
            "attempts cap at the budget"
        );
        let calls = std::fs::read_to_string(&log).unwrap().lines().count();
        assert_eq!(
            calls, MAX_CAPTURE_ATTEMPTS as usize,
            "pipe-pane is invoked exactly MAX_CAPTURE_ATTEMPTS times, then never again"
        );
    }

    /// A wedged tmux (a shell-command that sleeps forever) must not stall the
    /// tick: `run_pipe_pane` returns a timeout error within its bound.
    #[test]
    fn run_pipe_pane_times_out_on_a_wedged_tmux() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = TempDir::new().unwrap();
        let hang = tmp.path().join("hang-tmux.sh");
        // Sleep well past PIPE_PANE_TIMEOUT; the bounded runner must kill it.
        std::fs::write(&hang, "#!/bin/bash\nsleep 60\n").unwrap();
        std::fs::set_permissions(&hang, std::fs::Permissions::from_mode(0o755)).unwrap();

        let start = std::time::Instant::now();
        let err = run_pipe_pane(Command::new(&hang)).unwrap_err();
        let elapsed = start.elapsed();

        assert!(err.starts_with("timed out"), "err={err:?}");
        assert!(
            elapsed < PIPE_PANE_TIMEOUT + Duration::from_secs(3),
            "returned in {elapsed:?}, expected ~{PIPE_PANE_TIMEOUT:?}"
        );
    }
}
