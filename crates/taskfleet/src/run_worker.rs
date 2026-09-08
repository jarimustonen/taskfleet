//! `taskfleet run-worker <run-id> <node-id> -- <cmd> [args…]` — the thin
//! launcher shim (design.md §2.1 / A1, issue `thin-exit-status-launcher`).
//!
//! The 0.2 thin supervisor stops *guessing* a worker's completion from a
//! cross-product of pid × pane × activity proxies and instead consumes **told
//! facts**. This shim supplies the first such fact: an autonomous worker is
//! launched *through* it (`run-worker <run> <node> -- pi …`), so the shim
//! `wait()`s on the real agent process and records its **true exit status** —
//! a normal `exit_code` or a terminating `signal` — as a durable `worker.exited`
//! event under the run lock. The supervisor then reads a recorded status, not a
//! liveness inference (see [`crate::supervise`] exit-status consumption):
//!
//! - non-zero exit / killed by signal → `failed` (branch preserved),
//! - exit 0 **and** an `explicit-merge` transition exists → `done` + teardown,
//! - exit 0 **and** no merge → stays non-terminal / attention-required (the
//!   finished-but-unmerged case handed to the manual finish skill), NOT
//!   auto-failed.
//!
//! This is a *shim, not a protocol*: it needs no per-SKILL churn — the worker
//! command is simply wrapped at launch. It inherits stdio so the wrapped agent's
//! interactive TUI shares the pane exactly as an un-wrapped launch would, and it
//! never times the child out — the agent owns its own runtime.

use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::Command;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use clap::Args as ClapArgs;
use serde_json::{json, Value};

use taskfleet_core::{
    append_and_apply_event, read_manifest_opt, read_node_opt, NodeId, RunLock, RunPaths,
};

use crate::error::CliError;
use crate::run::{from_core, parse_node_id, parse_run_id, run_paths_exact};

/// Signals the shim IGNORES while it waits on the child, restoring their default
/// disposition in the child via `pre_exec` (below). A worker launched into an
/// interactive PTY shares the foreground process group, so a `SIGINT` (Ctrl-C) —
/// or a `SIGHUP`/`SIGTERM` from a pane/window teardown — is delivered to the shim
/// as well as the child. If the shim died from it before recording, the told-fact
/// guarantee would collapse to the pid-guess it exists to replace. Ignoring these
/// in the shim (while the child keeps its default disposition) lets the child take
/// the signal, die, and be reaped so the shim records its TRUE status and exits
/// with it. `SIGKILL` cannot be caught — that is the residual case the crash
/// backstop (design.md §2.1a) covers.
const SHIM_FORWARDED_SIGNALS: [libc::c_int; 4] =
    [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT];

static WORKER_PID: AtomicI32 = AtomicI32::new(0);
static PENDING_SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn forward_worker_signal(signal: libc::c_int) {
    let pid = WORKER_PID.load(Ordering::Relaxed);
    if pid > 0 {
        // SAFETY: positive pid means one process, never a process group. `kill`
        // is async-signal-safe and the child restores default dispositions.
        unsafe { libc::kill(pid, signal) };
    } else {
        PENDING_SIGNAL.store(signal, Ordering::Relaxed);
    }
}

/// Synthetic exit code recorded when the worker could not be *launched* at all
/// (e.g. the program is missing / not executable). Mirrors the shell's
/// "command not found" convention so the told fact is a plain non-zero failure —
/// the supervisor terminalizes `failed` (branch preserved) instead of falling
/// back to the pid-guess this shim exists to eliminate.
const SPAWN_FAILURE_EXIT_CODE: i32 = 127;

#[derive(ClapArgs, Debug)]
pub struct RunWorkerArgs {
    /// Run id the worker belongs to (full ULID).
    pub run_id: String,
    /// Node id inside the run whose worker this is (e.g. `n-0001`).
    pub node_id: String,
    /// Absolute worker-attempt generation (zero for legacy launchers).
    #[arg(long, default_value_t = 0)]
    pub attempt: u32,
    /// The worker command and its arguments, given after `--`
    /// (e.g. `run-worker <run> <node> -- pi --task …`).
    #[arg(last = true, required = true)]
    pub command: Vec<String>,
}

/// Launch the wrapped worker, wait for it, record its exit status durably, and
/// exit with the worker's own status. Never returns on the success path — it
/// `process::exit`s with the child's code (or `128 + signal`, the shell
/// convention) so the pane's exit status mirrors the agent's.
pub fn dispatch(args: RunWorkerArgs) -> Result<(), CliError> {
    // Validate ids and resolve the run + node BEFORE spawning: a typo must fail
    // loudly here rather than launch a worker whose exit the reducer would fold to
    // nothing (a `worker.exited` for an unknown node is a silent no-op).
    let run_id = parse_run_id(&args.run_id)?;
    let node_id = parse_node_id(&args.node_id)?;

    // Install forwarding before publication wait: cancellation may arrive while
    // the creator is still materializing. The handler retains one pending
    // signal and delivers it immediately once the child exists.
    for sig in SHIM_FORWARDED_SIGNALS {
        // SAFETY: zeroed sigaction is initialized before installation;
        // sigemptyset/sigaction are the platform signal APIs.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = forward_worker_signal as *const () as libc::sighandler_t;
            libc::sigemptyset(std::ptr::addr_of_mut!(action.sa_mask));
            action.sa_flags = 0;
            libc::sigaction(sig, std::ptr::addr_of!(action), std::ptr::null_mut());
        }
    }

    let await_publication = std::env::var_os("TASKFLEET_INTERNAL_WORKER_AWAIT_PUBLICATION")
        .is_some_and(|value| value == "1");
    let root = match std::env::var_os("TASKFLEET_INTERNAL_WORKER_STATE_ROOT") {
        Some(root) if await_publication => std::path::PathBuf::from(root),
        _ => crate::home::root_dir()?,
    };
    let paths = run_paths_exact(&root, &run_id)?;
    if !await_publication {
        require_published_node(&paths, &node_id, &args)?;
    }

    // `required = true` guarantees at least the program; split off its args.
    let (program, prog_args) = args
        .command
        .split_first()
        .ok_or_else(|| CliError::user("missing_worker_command", "no worker command after `--`"))?;

    // Inherit stdio (the default): the wrapped agent runs its interactive TUI in
    // this pane's PTY. No timeout — the worker owns its own lifecycle; the shim
    // only observes the exit. The child restores DEFAULT signal dispositions (via
    // `pre_exec`, after fork / before exec) so it still takes a Ctrl-C / teardown
    // signal normally when the shim forwards it.
    let mut cmd = Command::new(program);
    cmd.args(prog_args);
    // SAFETY: the closure only calls the async-signal-safe `libc::signal` in the
    // forked child before `exec`; it touches no shared state and allocates nothing.
    unsafe {
        cmd.pre_exec(|| {
            for sig in SHIM_FORWARDED_SIGNALS {
                libc::signal(sig, libc::SIG_DFL);
            }
            Ok(())
        });
    }

    cmd.env_remove("TASKFLEET_INTERNAL_WORKER_AWAIT_PUBLICATION")
        .env_remove("TASKFLEET_INTERNAL_WORKER_STATE_ROOT")
        .env_remove("TASKFLEET_TEST_WORKER_PUBLICATION_WAIT_MS");
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            // The worker could not even be launched. Record a told FAILURE (rather
            // than let the supervisor fall back to pid-guessing) and then surface
            // the error normally.
            let mut data = serde_json::Map::new();
            data.insert("attempt".into(), json!(args.attempt));
            data.insert("exit_code".into(), json!(SPAWN_FAILURE_EXIT_CODE));
            record_worker_exit(&paths, &node_id, &args, Value::Object(data));
            return Err(CliError::system(
                "worker_spawn_failed",
                format!("could not launch worker `{program}`: {e}"),
            ));
        }
    };
    WORKER_PID.store(child.id() as i32, Ordering::Relaxed);
    // During create, poll publication and the child together. Reaping an early
    // exit is load-bearing: a zombie can still answer kill(0) and expose its
    // start identity, which would otherwise let a dead candidate be published.
    if await_publication {
        if let Err(error) = await_published_node(&paths, &node_id, &args, &mut child) {
            terminate_child(&mut child);
            WORKER_PID.store(0, Ordering::Relaxed);
            return Err(error);
        }
    }
    let pending = PENDING_SIGNAL.swap(0, Ordering::Relaxed);
    if pending > 0 {
        unsafe { libc::kill(child.id() as libc::pid_t, pending) };
    }
    let status = child.wait().map_err(|e| {
        CliError::system(
            "worker_wait_failed",
            format!("could not wait for worker `{program}`: {e}"),
        )
    })?;
    WORKER_PID.store(0, Ordering::Relaxed);

    // Exactly one of code / signal is meaningful on Unix: a normal return carries
    // a code (signal None); a signal death carries a signal (code None). Prefer the
    // signal when present so the fact is never the ambiguous both-fields shape the
    // reducer rejects.
    let exit_code = status.code();
    let signal = status.signal();
    let mut data = serde_json::Map::new();
    data.insert("attempt".into(), json!(args.attempt));
    match (signal, exit_code) {
        (Some(s), _) => {
            data.insert("signal".into(), json!(s));
        }
        (None, Some(c)) => {
            data.insert("exit_code".into(), json!(c));
        }
        // Neither present is unreachable on Unix (ExitStatus always carries one),
        // but the reducer rejects an empty payload, so record a synthetic
        // non-zero code rather than lose the fact.
        (None, None) => {
            data.insert("exit_code".into(), json!(-1));
        }
    }

    record_worker_exit(&paths, &node_id, &args, Value::Object(data));

    // Propagate the worker's own exit status. `process::exit` runs no
    // destructors, so flush the buffered tracing log first (parity with the
    // supervisor's signal-exit path).
    let code = exit_code.unwrap_or_else(|| 128 + signal.unwrap_or(1));
    crate::cli::flush_logs();
    std::process::exit(code);
}

fn require_published_node(
    paths: &RunPaths,
    node_id: &NodeId,
    args: &RunWorkerArgs,
) -> Result<(), CliError> {
    // The final run directory appears atomically at publication. Avoid opening
    // its lock before that boundary, then read manifest + node under one shared
    // lock so a later projection update cannot interleave the pair.
    if !paths.manifest().exists() {
        return Err(
            CliError::user("run_not_found", format!("no run with id {}", args.run_id))
                .with_invalid_value(&args.run_id),
        );
    }
    let (manifest, node) = RunLock::with_shared_lock(&paths.lock(), || {
        Ok((read_manifest_opt(paths)?, read_node_opt(paths, node_id)?))
    })
    .map_err(from_core)?;
    if manifest.is_none() {
        return Err(
            CliError::user("run_not_found", format!("no run with id {}", args.run_id))
                .with_invalid_value(&args.run_id),
        );
    }
    if node.is_none() {
        return Err(CliError::user(
            "node_not_found",
            format!("no node {} in run {}", args.node_id, args.run_id),
        )
        .with_invalid_value(&args.node_id));
    }
    Ok(())
}

/// Bridge the intentional create transaction ordering: native materializer starts the
/// launcher while the run is private under `.creating`; the creator then writes
/// `node.created` and atomically publishes it under `runs/`. Waiting in the shim
/// keeps the native materializer's PID discovery live without exposing half-created state.
fn await_published_node(
    paths: &RunPaths,
    node_id: &NodeId,
    args: &RunWorkerArgs,
    child: &mut std::process::Child,
) -> Result<(), CliError> {
    const DEFAULT_WAIT: Duration = Duration::from_secs(120);
    #[cfg(debug_assertions)]
    let wait = std::env::var("TASKFLEET_TEST_WORKER_PUBLICATION_WAIT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(DEFAULT_WAIT, Duration::from_millis);
    #[cfg(not(debug_assertions))]
    let wait = DEFAULT_WAIT;
    let deadline = Instant::now() + wait;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| {
            CliError::system(
                "worker_wait_failed",
                format!("poll worker before publication: {e}"),
            )
        })? {
            return Err(CliError::system(
                "worker_exited_before_publication",
                format!("worker exited with {status} before run publication"),
            ));
        }
        match require_published_node(paths, node_id, args) {
            Ok(()) => return Ok(()),
            Err(error) if matches!(error.code.as_str(), "run_not_found" | "node_not_found") => {}
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            return Err(CliError::system(
                "worker_publication_timeout",
                format!(
                    "run {}/{} was not published within {:?}; refusing to launch the worker",
                    args.run_id, args.node_id, wait
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn terminate_child(child: &mut std::process::Child) {
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGKILL) };
    let _ = child.wait();
}

/// Append the durable `worker.exited` fact under the run lock (design.md §2.1).
///
/// No idempotency key: the shim fires once per launched worker, and the reducer's
/// first-write-wins guard already dedups a replay. A KEY scoped to the node would
/// silently swallow a *retried* worker's exit (a second shim for the same node),
/// so it is deliberately absent — attempt-scoped recording is left to the
/// attempt-identity work when retries are wired through the shim.
///
/// A recording failure does not swallow the worker's status (the crash backstop,
/// design.md §2.1a, covers a missing exit event), but it MUST be visible: it is
/// surfaced on stderr (the pane the operator sees) as well as the tracing log.
fn record_worker_exit(paths: &RunPaths, node_id: &NodeId, args: &RunWorkerArgs, data: Value) {
    if let Err(e) = append_and_apply_event(paths, "worker.exited", Some(node_id), None, data) {
        let msg = from_core(e).message;
        eprintln!(
            "taskfleet run-worker: failed to record worker.exited for {}/{}: {msg}",
            args.run_id, args.node_id
        );
        tracing::warn!(
            target: "taskfleet::run_worker",
            run_id = %args.run_id,
            node_id = %args.node_id,
            error = %msg,
            "failed to record worker.exited event; relying on the crash backstop"
        );
    }
}
