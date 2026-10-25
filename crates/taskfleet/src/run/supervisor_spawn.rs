//! On Linux, the user manager owns a Type=notify service and boot is
//! confirmed at the same readiness point via `sd_notify`. File descriptors
//! cannot cross the manager boundary. The native pipe/double-fork path below
//! is retained on other platforms only; it cannot escape a Linux cgroup.
//!
//! Spawn a detached `taskfleet supervise <run-id>` process and
//! wait briefly for its PID file to appear.
//!
//! Shared by `run create` (top-level), `run reattach`, and the parent
//! supervisor's child-spawn loop. All three funnel through the same
//! [`detached_supervise_command`] + [`spawn_and_reap`] pair so the process
//! detachment hardening (the double-fork below) lives in exactly one place.
//!
//! # Why double-fork
//!
//! A supervisor must outlive both the terminal that launched it and the
//! process that spawned it, and must never linger as a zombie. The naive
//! `Command::spawn` leaves the child in the spawner's session/process group
//! (so closing the terminal `SIGHUP`s every supervisor) and as a direct
//! child (so an exited supervisor becomes a zombie until the spawner
//! `wait()`s it — and `kill(pid, 0)` reports a zombie as *alive*, corrupting
//! the PID-staleness check).
//!
//! The fix is a classic double-fork, run inside `pre_exec` (after `fork`,
//! before `exec`):
//!   1. `setsid()` — the child leads a new session with no controlling
//!      terminal, so a terminal `SIGHUP` can never reach it.
//!   2. `fork()` again — the intermediate exits *immediately* (`_exit`,
//!      bypassing Rust destructors), so the grandchild is orphaned and
//!      reparented to init (pid 1). Init reaps the grandchild when it
//!      eventually exits, so no zombie ever accrues on our side.
//!
//! The spawner then `wait()`s the short-lived intermediate (it has already
//! `_exit`ed, so the wait returns at once) to reap *it*. Net: nothing the
//! spawner owns can become a zombie, and the real supervisor is fully
//! detached.
//!
//! Because this runs in `pre_exec` — before the supervisor binary's
//! `main`/tracing-subscriber ever initialize — there is no worker thread to
//! be orphaned by the intermediate's exit (a real hazard if you double-fork
//! *after* the runtime starts). The grandchild builds its tracing stack
//! fresh after `exec`.
//!
//! One consequence: the PID `Command::spawn` returns is the intermediate's,
//! not the grandchild's. The authoritative supervisor PID is the one the
//! supervisor writes into its own `supervisor.pid` during
//! [`crate::supervise::pid_file::claim_pid_atomic`]; lenient callers that need
//! it read it back from that file (see [`await_recorded_pid`]).
//!
//! # Confirming boot — readiness pipe vs. pid-file poll
//!
//! `run create` cannot record a run as started until it knows the supervisor
//! booted. It confirms that with a [readiness pipe](crate::run::supervisor_readiness)
//! threaded through the double-fork: the grandchild writes a readiness byte
//! carrying its pid AFTER `claim_pid_atomic` + init, and [`spawn_for_run`]
//! blocks reading it (a byte → confirmed; EOF → the supervisor died during
//! init; a structured error → the real reason). This has no timeout and no
//! orphan window — replacing the old bounded `supervisor.pid` poll that
//! false-failed a slow-but-healthy boot into `supervisor_spawn_failed` while
//! the grandchild kept running (issue `supervisor-confirm-readiness-pipe`).
//! The lenient callers (`run reattach`, child-spawn) do NOT confirm via the
//! pipe; they read `supervisor.pid` directly ([`await_recorded_pid`] /
//! [`read_live_recorded_pid`]) and tolerate "not yet confirmed".

use std::os::fd::RawFd;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use taskfleet_core::RunPaths;

use crate::error::CliError;
#[cfg(not(target_os = "linux"))]
use crate::run::supervisor_readiness::{Readiness, ReadinessPipe, ENV_READINESS_FD};
use crate::supervise::pid_file;

/// Generous backstop for the readiness read: a wedge circuit-breaker, NOT the
/// old confirmation deadline. Default 120s (≈8× the retired 15s pid-file poll)
/// so a merely slow-but-healthy boot is never false-failed; it only bounds a
/// supervisor genuinely stuck during init (e.g. blocked on the run flock).
#[cfg(not(target_os = "linux"))]
const READY_WAIT: Duration = Duration::from_secs(120);

/// [`READY_WAIT`] in production; tests point `TASKFLEET_READY_WAIT_MS` at a short
/// value so the wedge-backstop path is exercisable in milliseconds. An
/// unparseable value falls back to the production default.
#[cfg(not(target_os = "linux"))]
fn ready_wait() -> Duration {
    std::env::var("TASKFLEET_READY_WAIT_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(READY_WAIT, Duration::from_millis)
}

/// How long the *lenient* pid-file poll ([`await_recorded_pid`], used by
/// `run reattach`) waits for a freshly-forked supervisor to write its live pid
/// file before giving up and reporting pid 0 ("spawned, pid unconfirmed").
///
/// `run create`'s confirmation path no longer uses this: it uses the readiness
/// pipe ([`spawn_for_run`]), which has no timeout and no orphan window. This
/// deadline governs only the lenient callers that tolerate an unconfirmed pid
/// and rely on `supervisor.pid` as the durable truth.
const PID_FILE_WAIT: Duration = Duration::from_secs(15);
const POLL_TICK: Duration = Duration::from_millis(200);

/// How long [`await_recorded_pid`] waits for the supervisor's pid file.
/// [`PID_FILE_WAIT`] in production; tests point `TASKFLEET_PID_FILE_WAIT_MS` at a
/// short value so the fail-loud confirmation path is exercisable in
/// milliseconds. An unparseable value falls back to the production default.
fn pid_file_wait() -> Duration {
    std::env::var("TASKFLEET_PID_FILE_WAIT_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(PID_FILE_WAIT, Duration::from_millis)
}

/// The binary a detached supervisor is spawned from. Production always uses the
/// current executable (`taskfleet supervise <run-id>`); tests override via
/// `TASKFLEET_SUPERVISE_BIN` to point at a stub that never writes a pid file, so the
/// silent-spawn-failure path can be tested deterministically. Production
/// callers never set it.
fn supervise_command() -> Result<Command, CliError> {
    if let Ok(v) = std::env::var("TASKFLEET_SUPERVISE_BIN") {
        return Ok(Command::new(v));
    }
    crate::self_exec::command()
        .map_err(|e| CliError::system("io_error", format!("current_exe: {e}")))
}

/// Outcome of a supervisor spawn. An enum (not a `{ pid, confirmed }` struct)
/// so the two states are mutually exclusive by construction — there is no way
/// to represent the contradictory "confirmed with pid 0" that reintroduced the
/// original silent-success bug.
pub enum SupervisorSpawn {
    /// The supervisor confirmed boot down the readiness pipe, carrying its own
    /// pid (identical to the value it wrote into `supervisor.pid` under the run
    /// flock during `claim_pid_atomic`).
    Confirmed { pid: u32 },
    /// The supervisor never confirmed boot: the readiness pipe closed without a
    /// ready signal (it died during init), it reported a structured boot error,
    /// or the fork/exec itself failed. `reason` carries the specific cause.
    /// `run create` surfaces this as a loud `supervisor_spawn_failed` envelope
    /// rather than recording a bogus success (issue
    /// `supervisor-spawn-fails-silently-at-run-create`). Only `run create`
    /// constructs/consumes this — the lenient callers read the pid file directly.
    Unconfirmed { reason: String },
}

/// Attach the detach hardening (`setsid` + double-fork) to `cmd`'s child via
/// `pre_exec`. See the module docs for the full rationale.
///
/// `readiness_write_fd`, when set, is the readiness pipe's write end. The parent
/// creates the pipe with `FD_CLOEXEC` on both ends (so a concurrent `exec` on
/// another thread cannot leak it); this closure clears CLOEXEC on that fd inside
/// the forked child — right before `exec` — so exactly the intended grandchild
/// inherits it. `fcntl` is async-signal-safe, and the captured `Option<RawFd>`
/// is `Copy` (no allocation, no `Drop` between fork and exec).
#[cfg(not(target_os = "linux"))]
fn apply_detach(cmd: &mut Command, readiness_write_fd: Option<RawFd>) {
    use std::os::unix::process::CommandExt;
    // SAFETY: the closure is async-signal-safe — it calls only `fcntl`,
    // `setsid`, `fork`, and `_exit` (all on the POSIX async-signal-safe list)
    // and does no allocation, locking, or other Rust-runtime work between
    // `fork` and `exec`.
    unsafe {
        cmd.pre_exec(move || {
            // Uncloak the readiness write end for the exec that follows. Clearing
            // all fd flags (only FD_CLOEXEC is defined) makes it survive exec
            // into the grandchild supervisor.
            if let Some(fd) = readiness_write_fd {
                if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            // New session: detach from the controlling terminal and the
            // spawner's process group so a terminal SIGHUP cannot reach us.
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            // Double-fork: the intermediate exits now so the grandchild is
            // reparented to init (pid 1), which reaps it on exit. Use `_exit`
            // (not `return`/`exit`) so no Rust destructors or atexit hooks run
            // in this forked-but-not-exec'd process.
            match libc::fork() {
                -1 => Err(std::io::Error::last_os_error()),
                0 => Ok(()),         // grandchild: continue to exec the supervisor
                _ => libc::_exit(0), // intermediate: vanish, orphaning the grandchild
            }
        });
    }
}

/// Build a detach-hardened `supervise <run-id>` command with stdout/stderr
/// redirected to `log_path`. Callers may append extra args (e.g. `--once`)
/// before handing it to [`spawn_and_reap`].
///
/// `readiness_write_fd` is `Some` only for `run create`'s confirmation path
/// ([`spawn_for_run`]), which also sets `TASKFLEET_READINESS_FD` to that fd number;
/// the lenient callers pass `None`.
pub fn detached_supervise_command(
    run_id: &str,
    log_path: &Path,
    readiness_write_fd: Option<RawFd>,
) -> Result<Command, CliError> {
    #[cfg(target_os = "linux")]
    {
        let _ = readiness_write_fd;
        linux_supervise_command(run_id, log_path, None)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let stderr_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .map_err(|e| {
                CliError::system("io_error", format!("open {}: {}", log_path.display(), e))
            })?;
        let stderr_clone = stderr_file
            .try_clone()
            .map_err(|e| CliError::system("io_error", format!("dup fd: {e}")))?;
        let mut cmd = supervise_command()?;
        cmd.arg("supervise")
            .arg(run_id)
            // A detached daemon must not keep the launching terminal's stdin: an
            // inherited TTY fd 0 can deliver SIGTTIN or let a stray read consume
            // terminal input. setsid drops the controlling-terminal relationship
            // but not the fd itself.
            .stdin(std::process::Stdio::null())
            .stdout(stderr_file)
            .stderr(stderr_clone)
            // Clear any inherited readiness-fd hint. Only `run create`'s
            // confirmation path ([`spawn_for_run`]) sets `TASKFLEET_READINESS_FD`, and it
            // does so on its OWN command AFTER this builder. Without this clear, a
            // running supervisor (which itself inherited the variable from its
            // parent `run create`) would leak it to every child supervisor it forks,
            // and each child would write a readiness signal to an fd it never
            // inherited.
            .env_remove(ENV_READINESS_FD);
        apply_detach(&mut cmd, readiness_write_fd);
        Ok(cmd)
    }
}

#[cfg(target_os = "linux")]
fn linux_supervise_command(
    run_id: &str,
    log_path: &Path,
    ready_path: Option<&Path>,
) -> Result<Command, CliError> {
    let log_path =
        std::path::absolute(log_path).map_err(|e| CliError::system("io_error", e.to_string()))?;
    // append: properties use systemd specifiers, so escape literal percent.
    let log = log_path.to_string_lossy().replace('%', "%%");
    let mut cmd = crate::linux_lifetime::service(
        &format!("taskfleet-supervise-{run_id}-{}", ulid::Ulid::new()),
        "notify",
    )?;
    if let Some(path) = ready_path {
        cmd.arg(format!(
            "--setenv=TASKFLEET_READINESS_FILE={}",
            path.display()
        ));
    }
    // Keep asynchronous notify hooks alive after the supervisor's main PID
    // exits; the unit is collected when its last child finishes, without
    // blocking the supervisor tick or adding another teardown actor.
    cmd.arg("--property=ExitType=cgroup")
        .arg(format!("--property=StandardOutput=append:{log}"))
        .arg(format!("--property=StandardError=append:{log}"))
        .arg("--");
    let inner = supervise_command()?;
    cmd.arg(inner.get_program()).arg("supervise").arg(run_id);
    Ok(cmd)
}

/// Spawn a detach-hardened supervisor `cmd` and reap the short-lived
/// double-fork intermediate so it never lingers as a zombie. The real
/// supervisor (the grandchild) is already reparented to init by the time
/// this returns.
pub fn spawn_and_reap(cmd: &mut Command, run_id: &str) -> Result<(), CliError> {
    #[cfg(target_os = "linux")]
    {
        let _ = run_id;
        crate::linux_lifetime::run_service(cmd)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let mut child = cmd.spawn().map_err(|e| {
            CliError::system("spawn_failed", format!("spawn supervise {run_id}: {e}"))
        })?;
        // The intermediate `_exit`s immediately after forking the grandchild, so
        // this wait reaps it without blocking on the actual supervisor. A wait
        // error (e.g. ECHILD if it was already reaped) is non-fatal — the
        // grandchild is independent — but surface it rather than swallow it.
        if let Err(e) = child.wait() {
            tracing::warn!(
                target: "taskfleet::supervise",
                run = %run_id,
                error = %e,
                "failed to reap double-fork intermediate (grandchild unaffected)"
            );
        }
        Ok(())
    }
}

/// Read `<run-dir>/supervisor.pid` ONCE and return the recorded pid iff it is
/// a live process whose start-time still matches the record (§7.6 identity
/// check). Non-blocking. Used where the caller must not stall — e.g. the
/// parent supervisor's tick — and is content with "pid not yet confirmed"
/// (the child writes its own pid file as the durable source of truth).
pub fn read_live_recorded_pid(paths: &RunPaths) -> Option<u32> {
    let (pid, start) = pid_file::read_pid_record(&paths.supervisor_pid())?;
    pid_file::pid_live_with_identity(pid, &start).then_some(pid)
}

/// Poll `<run-dir>/supervisor.pid` for up to [`PID_FILE_WAIT`] and return the
/// live, identity-verified supervisor PID it records. `None` if none appears
/// in time — with double-fork we have no usable spawned PID to fall back to
/// (the intermediate we reaped is gone), so callers decide how to degrade. In
/// practice the supervisor writes its pid file under the run flock within
/// milliseconds of `exec`, so the deadline is reached only if the supervisor
/// failed to boot.
///
/// Identity matters: a stale pid file from a prior generation whose pid has
/// been recycled by an unrelated live process must NOT be accepted as "our"
/// supervisor — hence `read_pid_record` + `pid_live_with_identity`, not a bare
/// liveness probe.
pub fn await_recorded_pid(paths: &RunPaths) -> Option<u32> {
    let deadline = Instant::now() + pid_file_wait();
    loop {
        if let Some(pid) = read_live_recorded_pid(paths) {
            return Some(pid);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(POLL_TICK);
    }
}

/// Append a diagnostic line to the supervisor's stderr log so a spawn that
/// never got far enough to boot the tracing subscriber still leaves a trace on
/// disk (issue `supervisor-spawn-fails-silently-at-run-create`, suggested-fix
/// #2 "always write supervisor.stderr.log … capture the fork/exec failure
/// reason"). Best-effort: a log-write failure must never mask the spawn error
/// the caller is already returning.
fn append_spawn_diag(log_path: &Path, msg: &str) {
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        let _ = writeln!(f, "[taskfleet run create] {msg}");
    }
}

/// Fork+exec a fully-detached supervisor with stdout/stderr redirected to
/// `<run-dir>/supervisor.stderr.log`, then confirm its boot over a
/// [readiness pipe](crate::run::supervisor_readiness) — no timeout, no orphan
/// window.
///
/// The stderr log is opened (created, possibly empty) *before* the fork by
/// [`detached_supervise_command`], so a trace file always exists on disk from
/// the moment of spawn. A fork/exec failure, or a supervisor that never
/// confirms boot, is additionally recorded into that log via
/// [`append_spawn_diag`] — otherwise a silent spawn failure leaves zero trace
/// to diagnose from (the original bug signature).
///
/// Confirmation mechanism (issue `supervisor-confirm-readiness-pipe`): the
/// parent creates a pipe whose write end the grandchild inherits across
/// `exec`. The grandchild writes a readiness signal carrying its pid AFTER it
/// has claimed `supervisor.pid` and finished init, then closes the write end.
/// The parent closes its own write-end copy and reads the read end, bounded by
/// a generous wedge backstop (`ready_wait`):
///   - a `ready` signal → [`SupervisorSpawn::Confirmed`] with the supervisor pid;
///   - EOF with no signal → the supervisor died during init (fate-sharing) →
///     [`SupervisorSpawn::Unconfirmed`];
///   - a structured error signal → `Unconfirmed` carrying the real reason;
///   - deadline elapsed → `Unconfirmed` (supervisor wedged, e.g. on the run
///     flock — alive but not progressing).
///
/// Confirmation is edge-triggered: the read returns the moment the grandchild
/// signals OR the write end closes, so a slow-but-healthy boot is confirmed
/// whenever it finishes and a genuinely dead supervisor is detected at once —
/// the ambiguity of the old bounded pid-file poll is gone. The backstop only
/// bounds a true hang (a purely unbounded read would freeze `run create`
/// forever behind a stuck `claim_pid_atomic` flock). `run create` turns every
/// `Unconfirmed` into a loud `supervisor_spawn_failed`; lenient callers never
/// use this path.
pub fn spawn_for_run(paths: &RunPaths, run_id: &str) -> Result<SupervisorSpawn, CliError> {
    let log_path = paths.root.join("supervisor.stderr.log");

    #[cfg(target_os = "linux")]
    {
        let receipt = tempfile::NamedTempFile::new_in(crate::linux_lifetime::private_dir()?)
            .map_err(|e| CliError::system("io_error", e.to_string()))?;
        let mut cmd = linux_supervise_command(run_id, &log_path, Some(receipt.path()))?;
        let launch = spawn_and_reap(&mut cmd, run_id);
        let frame = std::fs::read(receipt.path()).unwrap_or_default();
        use crate::run::supervisor_readiness::{parse_readiness, Readiness};
        let outcome = match (launch, parse_readiness(&frame)) {
            (Ok(()), Readiness::Ready { pid }) => SupervisorSpawn::Confirmed { pid },
            (_, Readiness::Error { code, message }) => SupervisorSpawn::Unconfirmed {
                reason: format!("supervisor reported a boot error: {code}: {message}"),
            },
            (Err(e), _) => SupervisorSpawn::Unconfirmed { reason: e.message },
            (Ok(()), _) => SupervisorSpawn::Unconfirmed {
                reason: "user service confirmed boot without a valid private PID receipt; inspect supervisor.stderr.log".into(),
            },
        };
        if let SupervisorSpawn::Unconfirmed { reason } = &outcome {
            append_spawn_diag(&log_path, reason);
        }
        Ok(outcome)
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Readiness pipe first, so its write-end fd can be uncloaked in `pre_exec`
        // (CLOEXEC cleared only for the intended grandchild — see `apply_detach`).
        let mut pipe = match ReadinessPipe::new() {
            Ok(p) => p,
            Err(e) => {
                // Falling back to a pid-file poll here would reintroduce the very
                // timeout ambiguity this fix removes; a pipe() failure is a real,
                // rare resource exhaustion worth surfacing loudly instead.
                let reason = format!("failed to create supervisor readiness pipe: {e}");
                append_spawn_diag(&log_path, &reason);
                return Ok(SupervisorSpawn::Unconfirmed { reason });
            }
        };
        let write_fd = pipe.write_fd();
        let mut cmd = detached_supervise_command(run_id, &log_path, Some(write_fd))?;
        cmd.env(ENV_READINESS_FD, write_fd.to_string());

        if let Err(e) = spawn_and_reap(&mut cmd, run_id) {
            append_spawn_diag(
                &log_path,
                &format!("fork/exec of supervisor failed: {}", e.message),
            );
            return Err(e);
        }

        // The parent must stop being a writer, or `await_ready` never observes EOF
        // on the grandchild's death. After this, the only remaining write-end copy
        // is the grandchild's (the double-fork intermediate already `_exit`ed and
        // was reaped by `spawn_and_reap`).
        pipe.close_write();

        match pipe.await_ready(ready_wait()) {
            Readiness::Ready { pid } => Ok(SupervisorSpawn::Confirmed { pid }),
            Readiness::Died => {
                let reason = "supervisor died during init: the readiness pipe closed \
                          without a ready signal (see supervisor.stderr.log)"
                    .to_string();
                append_spawn_diag(&log_path, &reason);
                Ok(SupervisorSpawn::Unconfirmed { reason })
            }
            Readiness::Timeout => {
                let reason = format!(
                    "supervisor did not confirm boot within {:?}: it is wedged during init \
                 (e.g. blocked acquiring the run lock) — check for a stuck supervisor holding \
                 the run flock, then inspect supervisor.stderr.log",
                    ready_wait()
                );
                append_spawn_diag(&log_path, &reason);
                Ok(SupervisorSpawn::Unconfirmed { reason })
            }
            Readiness::Error { code, message } => {
                let reason = if message.is_empty() {
                    format!("supervisor reported a boot error: {code}")
                } else {
                    format!("supervisor reported a boot error: {code}: {message}")
                };
                append_spawn_diag(&log_path, &reason);
                Ok(SupervisorSpawn::Unconfirmed { reason })
            }
            Readiness::Malformed(raw) => {
                let reason = format!(
                    "supervisor sent a malformed/truncated readiness signal ({raw:?}); \
                 treating boot as failed (see supervisor.stderr.log)"
                );
                append_spawn_diag(&log_path, &reason);
                Ok(SupervisorSpawn::Unconfirmed { reason })
            }
        }
    }
}
