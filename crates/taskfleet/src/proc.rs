//! Bounded subprocess execution shared across the CLI.
//!
//! One timeout + process-group-kill + capped-capture implementation, lifted
//! from the original tmux-specific watchdog `run_timed` (the
//! `spinoff-issuectl-subprocess-bounds` work). The supervisor's tmux
//! shell-outs — the liveness watchdog ([`crate::supervise::watchdog`]) and
//! pane-output capture ([`crate::supervise::capture`]) — drive foreign binaries
//! (`tmux`) that can wedge, stream unbounded output, or fork children, so every
//! such spawn goes through here rather than `Command::output()`:
//!
//! - **Timeout.** The std library has no built-in process timeout, so this
//!   polls [`std::process::Child::try_wait`] against a deadline and SIGKILLs the
//!   child's process *group* on overrun (the child is placed in its own group
//!   via `process_group(0)`, so the group kill reaps any subprocess it forked
//!   and closes the pipes, releasing the reader threads).
//! - **Output cap.** stdout and stderr are each drained on a helper thread (so a
//!   child that fills one pipe cannot dead-lock waiting for us to read the
//!   other) but only the first `cap` bytes are retained — a runaway producer
//!   bounds our memory instead of `OOMing`. Draining continues past the cap so the
//!   writer never blocks; the deadline still bounds total wall-clock.

use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::supervise::pid_file;

/// How often the wait loop polls the child while counting down to the deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// One captured output stream, retained up to a cap.
#[derive(Debug, Clone, Default)]
pub struct CappedStream {
    /// The captured bytes — at most `cap` of them.
    pub bytes: Vec<u8>,
    /// `true` if the child produced more than `cap` bytes and the tail was
    /// dropped. Callers should warn and treat the bytes as a prefix only.
    /// Retained as part of the capture contract even though the sole production
    /// reader (the removed `spinoff approve` issuectl shell-out) is gone; still
    /// asserted by this module's tests.
    #[allow(dead_code)]
    pub truncated: bool,
}

impl CappedStream {
    fn empty() -> Self {
        Self::default()
    }
}

/// Outcome of a bounded subprocess run.
pub enum TimedOutcome {
    /// The child exited on its own (zero or non-zero); both streams captured
    /// (each capped).
    Exited {
        status: ExitStatus,
        stdout: CappedStream,
        stderr: CappedStream,
    },
    /// The deadline was exceeded; the child's process group was `SIGKILLed`.
    TimedOut,
    /// The child could not be spawned (e.g. binary not on `PATH`) or an
    /// unexpected `wait` error occurred.
    SpawnErr(std::io::Error),
}

/// Spawn `cmd` and wait at most `timeout`, capturing stdout and stderr each up
/// to `cap` bytes. `stdin`/`stdout`/`stderr` and the process group are set here
/// authoritatively — the caller only supplies the program, args, env, and
/// `current_dir`.
///
/// On overrun the child's process *group* is `SIGKILLed` (reaping any subprocess
/// it forked) and the run reports [`TimedOutcome::TimedOut`]. The deadline is
/// polled every [`POLL_INTERVAL`]; latency is bounded by it.
pub fn run_with_timeout(mut cmd: Command, timeout: Duration, cap: usize) -> TimedOutcome {
    use std::os::unix::process::CommandExt;

    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // Own process group so an early stop can reap the whole tree, not just the
    // direct child (which may be a shell that forked the real binary).
    cmd.process_group(0);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return TimedOutcome::SpawnErr(e),
    };
    let pid = child.id();

    // Drain each pipe on its own thread so a child that fills one pipe cannot
    // dead-lock waiting for us to read the other.
    let mut out_reader: Option<JoinHandle<CappedStream>> = child
        .stdout
        .take()
        .map(|s| std::thread::spawn(move || read_capped(s, cap)));
    let mut err_reader: Option<JoinHandle<CappedStream>> = child
        .stderr
        .take()
        .map(|s| std::thread::spawn(move || read_capped(s, cap)));

    // `checked_add` so an absurd `timeout` cannot overflow `Instant` and panic —
    // an un-representable deadline is treated as "no deadline" (the run is then
    // bounded only by the child exiting on its own). Every caller passes a small
    // hardcoded constant, so this fallback is not reachable in practice.
    let deadline = Instant::now().checked_add(timeout);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    // The child may have raced us to exit between `try_wait`
                    // above and here — report its real status rather than
                    // fabricating a killed result.
                    if let Ok(Some(status)) = child.try_wait() {
                        let (stdout, stderr) =
                            drain_readers(pid, out_reader.take(), err_reader.take());
                        return TimedOutcome::Exited {
                            status,
                            stdout,
                            stderr,
                        };
                    }
                    kill_group(pid, &mut child);
                    let _ = child.wait();
                    let _ = drain_readers(pid, out_reader.take(), err_reader.take());
                    return TimedOutcome::TimedOut;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => {
                // A `wait` error is unexpected; make sure the child cannot linger
                // (and hang a subsequent blocking wait) before returning.
                kill_group(pid, &mut child);
                let _ = child.wait();
                let _ = drain_readers(pid, out_reader.take(), err_reader.take());
                return TimedOutcome::SpawnErr(e);
            }
        }
    };

    // The child exited; collect what the readers drained. A backgrounded
    // descendant that inherited the pipe fds can outlive the leader and keep the
    // pipes open, so this is bounded (see `drain_readers`) — never an unbounded
    // join.
    let (stdout, stderr) = drain_readers(pid, out_reader.take(), err_reader.take());
    TimedOutcome::Exited {
        status,
        stdout,
        stderr,
    }
}

/// SIGKILL the child's process *group* (reaping any subprocess it forked into
/// the group), falling back to a direct `child.kill()` if the pid cannot be
/// narrowed to a signed `pid_t` (pid 0 / out of range — practically impossible
/// for a real child, but never leave it un-killed before a blocking wait).
fn kill_group(pid: u32, child: &mut std::process::Child) {
    if let Some(pgid) = pid_file::to_pid_t(pid) {
        // SAFETY: SIGKILL to `-pgid` signals our own freshly spawned process
        // group (pgid == child pid via `process_group(0)`); `pgid` is
        // range-checked by `to_pid_t`, so the negation can never be `-1`
        // (broadcast). Killing the group releases the pipes so the reader
        // threads unblock.
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
    } else {
        let _ = child.kill();
    }
}

/// Join the stdout/stderr reader threads without ever blocking the caller
/// indefinitely.
///
/// The normal case (child fully exited, pipes closed) returns immediately. But a
/// descendant that inherited the pipe write-ends can outlive the group leader
/// and hold them open — a plain `join()` would then hang the supervisor, which is
/// the exact failure this module exists to prevent. So:
///   1. give the readers a short grace to finish on their own;
///   2. if still blocked, `SIGKILL` the process group to close any pipe a
///      lingering group member is holding, then wait a hard deadline;
///   3. if a reader *still* has not finished (a descendant that escaped the group
///      via `setsid`), detach it and return an empty stream for that pipe —
///      losing a partial capture is acceptable; hanging is not.
fn drain_readers(
    pid: u32,
    out_reader: Option<JoinHandle<CappedStream>>,
    err_reader: Option<JoinHandle<CappedStream>>,
) -> (CappedStream, CappedStream) {
    /// Grace before we escalate to a group kill to unblock a lingering reader.
    const GRACE: Duration = Duration::from_millis(200);
    /// Hard ceiling after the group kill before we detach a still-blocked reader.
    const HARD: Duration = Duration::from_secs(2);

    let reader_done = |r: Option<&JoinHandle<CappedStream>>| r.is_none_or(JoinHandle::is_finished);
    let both_finished = || reader_done(out_reader.as_ref()) && reader_done(err_reader.as_ref());

    if !poll_until(GRACE, both_finished) {
        // A reader is still blocked: a group member is holding a pipe. Close it
        // by killing the group, then give a hard-bounded window to unblock.
        if let Some(pgid) = pid_file::to_pid_t(pid) {
            // SAFETY: see `kill_group` — `-pgid` targets our own group only.
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
        }
        poll_until(HARD, both_finished);
    }

    (finish_or_detach(out_reader), finish_or_detach(err_reader))
}

/// Poll `done` every [`POLL_INTERVAL`] up to `budget`, returning whether it
/// became true within the budget.
fn poll_until(budget: Duration, done: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Join a reader that has (almost certainly) finished, or detach it and return an
/// empty stream if it is still blocked on an escaped descendant's pipe.
fn finish_or_detach(reader: Option<JoinHandle<CappedStream>>) -> CappedStream {
    match reader {
        Some(h) if h.is_finished() => h.join().unwrap_or_else(|_| CappedStream::empty()),
        // Still blocked: detach (drop the handle). The process group was already
        // SIGKILLed, so the escapee will eventually die and the thread exit; we
        // do not wait for it.
        Some(_) | None => CappedStream::empty(),
    }
}

/// Read `r` to EOF but retain only the first `cap` bytes, flagging truncation.
/// Reading continues past the cap (discarding) so the writer never blocks on a
/// full pipe — the caller's deadline bounds a truly endless producer.
fn read_capped(mut r: impl Read, cap: usize) -> CappedStream {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 8192];
    loop {
        match r.read(&mut chunk) {
            Ok(n) if n > 0 => {
                if bytes.len() < cap {
                    let take = (cap - bytes.len()).min(n);
                    bytes.extend_from_slice(&chunk[..take]);
                    if take < n {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
            }
            // EOF (`Ok(0)`) or a read error mid-stream (e.g. the pipe torn down
            // by a group kill) ends capture with whatever we have so far.
            Ok(_) | Err(_) => break,
        }
    }
    CappedStream { bytes, truncated }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `/bin/sh -c <script>` as a `Command`, ready for [`run_with_timeout`].
    fn sh(script: &str) -> Command {
        let mut c = Command::new("/bin/sh");
        c.arg("-c").arg(script);
        c
    }

    #[test]
    fn captures_stdout_and_exit_status() {
        let out = run_with_timeout(sh("printf hello; exit 0"), Duration::from_secs(5), 1 << 20);
        match out {
            TimedOutcome::Exited {
                status,
                stdout,
                stderr,
            } => {
                assert!(status.success());
                assert_eq!(stdout.bytes, b"hello");
                assert!(!stdout.truncated);
                assert_eq!(stderr.bytes, Vec::<u8>::new());
            }
            _ => panic!("expected Exited"),
        }
    }

    #[test]
    fn captures_stderr_and_nonzero_status() {
        let out = run_with_timeout(
            sh("printf oops 1>&2; exit 7"),
            Duration::from_secs(5),
            1 << 20,
        );
        match out {
            TimedOutcome::Exited { status, stderr, .. } => {
                assert_eq!(status.code(), Some(7));
                assert_eq!(stderr.bytes, b"oops");
            }
            _ => panic!("expected Exited"),
        }
    }

    #[test]
    fn caps_oversized_output_and_flags_truncation() {
        // Produce ~64 KiB but cap at 1 KiB.
        let out = run_with_timeout(
            sh("yes AAAAAAAA | head -c 65536"),
            Duration::from_secs(10),
            1024,
        );
        match out {
            TimedOutcome::Exited { stdout, .. } => {
                assert_eq!(stdout.bytes.len(), 1024, "retained exactly the cap");
                assert!(stdout.truncated, "overflow must flag truncation");
            }
            _ => panic!("expected Exited"),
        }
    }

    #[test]
    fn kills_group_on_timeout() {
        let start = Instant::now();
        // A child that would sleep far past the deadline; also forks a grandchild
        // so the group-kill path is exercised.
        let out = run_with_timeout(
            sh("sleep 30 & sleep 30"),
            Duration::from_millis(200),
            1 << 20,
        );
        assert!(matches!(out, TimedOutcome::TimedOut));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timeout must fire promptly, not wait for the child"
        );
    }

    #[test]
    fn escaped_background_child_does_not_hang_on_deadline() {
        // The shell exits immediately, so `try_wait` reports Exited — but the
        // backgrounded `sleep` inherited the pipe fds and is still alive, holding
        // them open. A naive `join()` on the reader threads would block forever.
        // `drain_readers` must bound it (grace → group kill → hard deadline) and
        // return promptly.
        let start = Instant::now();
        let out = run_with_timeout(sh("sleep 30 & exit 0"), Duration::from_millis(100), 1 << 20);
        // The leader exited cleanly; the group kill in `drain_readers` reaps the
        // lingering `sleep`. Either way, the call returns without hanging.
        assert!(matches!(
            out,
            TimedOutcome::Exited { .. } | TimedOutcome::TimedOut
        ));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "must not hang on a backgrounded descendant holding the pipes"
        );
    }

    #[test]
    fn missing_binary_is_spawn_err() {
        let out = run_with_timeout(
            Command::new("/nonexistent/taskfleet-no-such-binary"),
            Duration::from_secs(5),
            1 << 20,
        );
        match out {
            TimedOutcome::SpawnErr(e) => {
                assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
            }
            _ => panic!("expected SpawnErr"),
        }
    }
}
