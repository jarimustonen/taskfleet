//! Daemonization **readiness pipe** for confirming a detached supervisor booted.
//!
//! `run create` must learn — without an arbitrary timeout — whether the
//! supervisor it double-forks (see [`crate::run::supervisor_spawn`]) actually
//! came up. The old mechanism polled `supervisor.pid` for a bounded window
//! (`PID_FILE_WAIT`); a healthy-but-slow boot under load could exceed the
//! deadline and be false-failed into `supervisor_spawn_failed` while the
//! grandchild kept running — an orphan supervising a run the caller was told
//! failed (issue `supervisor-confirm-readiness-pipe`, review finding F4b).
//!
//! The fix is the classic UNIX readiness pipe threaded through the double-fork:
//!
//! 1. The parent ([`ReadinessPipe::new`]) creates a pipe with `FD_CLOEXEC` on
//!    **both** ends (so a concurrent `exec` on another thread cannot leak the
//!    write end and suppress EOF). CLOEXEC on the write end is cleared only
//!    inside the forked child's `pre_exec`, right before `exec`, so exactly the
//!    intended grandchild inherits it. The parent passes the write-end fd number
//!    to the child via `TASKFLEET_READINESS_FD` in the child's environment.
//! 2. The grandchild (the real supervisor) takes ownership of that inherited fd
//!    ([`ReadinessReporter::from_env`], validating it is an open pipe above
//!    stdio) and, **after** `claim_pid_atomic` + boot init, writes a one-line
//!    readiness message and closes the fd ([`ReadinessReporter::ready`] /
//!    [`ReadinessReporter::error`]).
//! 3. The parent closes its own copy of the write end
//!    ([`ReadinessPipe::close_write`]) — so it is not itself a writer — and reads
//!    one framed line in [`ReadinessPipe::await_ready`], bounded by a generous
//!    backstop deadline:
//!    - a `ready` line → the supervisor confirmed boot (carries its pid);
//!    - EOF with no message → every write-end copy closed without a signal, so
//!      the supervisor **died during init** (fate-sharing);
//!    - an `error` line → the supervisor articulated a real boot failure;
//!    - a truncated/garbled message → treated as a boot failure, never a hang;
//!    - the deadline elapses → the supervisor is **wedged** (alive, not
//!      progressing) → [`Readiness::Timeout`].
//!
//! Fate-sharing is exact: EOF arrives only once **all** write-end copies
//! (parent, double-fork intermediate, grandchild) are closed. The parent must
//! therefore [`close_write`](ReadinessPipe::close_write) before reading, and the
//! grandchild closes its copy as soon as it has written (or when it exits
//! without writing). Reading one newline-framed line — rather than to EOF — means
//! a complete `ready` is honored even if some stray duplicate of the write end
//! lingers open; confirmation never waits on every descriptor closing.
//!
//! # Why a generous backstop deadline (not the old 15s poll)
//!
//! EOF detects *death*, not a *wedge*. `claim_pid_atomic` takes the run flock
//! with a **blocking** `flock` (taskfleet-core `lock.rs`), so a supervisor stuck
//! behind a dead lock-holder — or wedged on NFS/a failed mount during init —
//! never writes AND never closes the write end, and a purely unbounded read
//! would hang `run create` forever. [`await_ready`](ReadinessPipe::await_ready)
//! therefore polls the read end with a **generous** deadline (default 120s,
//! `TASKFLEET_READY_WAIT_MS`) that acts as a wedge circuit-breaker, ~8× the old
//! bounded poll so it never false-fails a merely slow-but-healthy boot. The
//! confirmation itself is still edge-triggered (a byte or EOF, whichever comes
//! first) — the deadline only bounds a genuine hang.
//!
//! # Wire format
//!
//! One newline-terminated line, tag byte first. The frame is **strict**: a
//! ready signal is accepted only as `R<digits>\n` in full, so a write truncated
//! by a crash (missing the terminator, or trailing garbage) is classified
//! `Malformed` — never a false `Ready` carrying a truncated pid.
//!   - ready:  `R<pid>\n`               (`pid` decimal, the supervisor's own pid)
//!   - error:  `E<code>\t<message>\n`   (a structured boot-failure reason)

// Native pipe transport remains exercised on Linux in unit tests; production
// Linux readiness crosses the manager boundary via sd_notify instead.
#![cfg_attr(target_os = "linux", allow(dead_code))]

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::{Duration, Instant};

/// Environment variable carrying the inherited write-end fd number from the
/// parent ([`ReadinessPipe`]) to the grandchild supervisor
/// ([`ReadinessReporter`]). Set by `run create`'s confirmation path only; the
/// lenient spawn paths (`run reattach`, child-spawn) clear it in
/// [`crate::run::supervisor_spawn::detached_supervise_command`] so a supervisor
/// they launch never tries to write to a fd it did not inherit.
pub const ENV_READINESS_FD: &str = "TASKFLEET_READINESS_FD";

/// Upper bound on the readiness message the parent will read, so a misbehaving
/// or corrupt writer cannot make `run create` read unboundedly. The real
/// messages are a few dozen bytes; this is generous headroom.
const MAX_MSG: usize = 4096;

const TAG_READY: u8 = b'R';
const TAG_ERROR: u8 = b'E';

/// The parent's decode of what the grandchild reported down the pipe.
#[derive(Debug, PartialEq, Eq)]
pub enum Readiness {
    /// The supervisor confirmed boot after claiming the pid file. Carries the
    /// supervisor's own pid (identical to the value it wrote into
    /// `supervisor.pid`).
    Ready { pid: u32 },
    /// EOF with no message: every write-end copy closed without a readiness
    /// signal, so the supervisor died during init (fate-sharing).
    Died,
    /// The supervisor articulated a specific boot failure before exiting.
    Error { code: String, message: String },
    /// A non-empty but undecodable message — a truncated write (no terminator),
    /// trailing garbage, or a bogus tag. Surfaced as a boot failure, carrying the
    /// raw bytes (lossily) for diagnostics.
    Malformed(String),
    /// The backstop deadline elapsed with neither a complete frame nor EOF: the
    /// supervisor is wedged during init (e.g. blocked on the run flock) — alive
    /// but not making progress. Distinct from `Died` so the caller can say so.
    Timeout,
}

/// Parse a **complete** readiness frame (up to and including its terminating
/// newline). Pure and total so the named cases (readiness success, init-failure
/// EOF, truncated/partial write) are unit-testable without real pipes. Strict by
/// construction: a `Ready` is accepted only for an exact `R<digits>\n` in a
/// valid pid range, so a truncated or garbled ready never becomes a false
/// success.
pub fn parse_readiness(bytes: &[u8]) -> Readiness {
    if bytes.is_empty() {
        return Readiness::Died;
    }
    // A complete frame is newline-terminated. Its absence means the write was
    // truncated (crash mid-syscall) — never accept it as a ready signal.
    let Some(body) = bytes.strip_suffix(b"\n") else {
        return Readiness::Malformed(String::from_utf8_lossy(bytes).into_owned());
    };
    match body.first().copied() {
        Some(TAG_READY) => {
            let digits = &body[1..];
            let pid = if !digits.is_empty() && digits.iter().all(u8::is_ascii_digit) {
                std::str::from_utf8(digits)
                    .ok()
                    .and_then(|s| s.parse::<u32>().ok())
            } else {
                // Empty pid, or trailing non-digit garbage before the newline.
                None
            };
            match pid {
                Some(pid) if pid != 0 && pid <= libc::pid_t::MAX as u32 => Readiness::Ready { pid },
                _ => Readiness::Malformed(String::from_utf8_lossy(bytes).into_owned()),
            }
        }
        Some(TAG_ERROR) => {
            let rest = &body[1..];
            let (code, message) = match rest.iter().position(|&b| b == b'\t') {
                Some(i) => (
                    String::from_utf8_lossy(&rest[..i]).into_owned(),
                    String::from_utf8_lossy(&rest[i + 1..]).into_owned(),
                ),
                None => (String::from_utf8_lossy(rest).into_owned(), String::new()),
            };
            Readiness::Error { code, message }
        }
        _ => Readiness::Malformed(String::from_utf8_lossy(bytes).into_owned()),
    }
}

/// True iff `fd` refers to an open pipe/FIFO — the shape of a readiness write
/// end. Used to reject a stale or hand-set [`ENV_READINESS_FD`] pointing at an
/// unrelated descriptor (a log file, socket, or stdio) before adopting it.
fn is_writable_pipe(fd: RawFd) -> bool {
    // SAFETY: `fstat` only reads metadata for `fd` into the zeroed struct.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, std::ptr::from_mut(&mut st)) } != 0 {
        return false;
    }
    (st.st_mode & libc::S_IFMT) == libc::S_IFIFO
}

/// Set or clear `FD_CLOEXEC` on `fd`.
fn set_cloexec(fd: RawFd, on: bool) -> std::io::Result<()> {
    // SAFETY: `fcntl` with `F_GETFD`/`F_SETFD` only reads/writes the fd's
    // close-on-exec flag; it does not consume or alias the fd.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let new = if on {
        flags | libc::FD_CLOEXEC
    } else {
        flags & !libc::FD_CLOEXEC
    };
    if unsafe { libc::fcntl(fd, libc::F_SETFD, new) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Parent side of the readiness pipe. Owns both ends; the read end is used to
/// [`await_ready`](Self::await_ready) and the write end is handed (by fd number)
/// to the child then dropped via [`close_write`](Self::close_write).
pub struct ReadinessPipe {
    read: OwnedFd,
    write: Option<OwnedFd>,
}

impl ReadinessPipe {
    /// Create the pipe with `FD_CLOEXEC` on **both** ends. The write end stays
    /// CLOEXEC in the parent — a concurrent `exec` on another thread (the CLI
    /// runs a `tracing_appender` worker) must not leak it, or the parent would
    /// never see EOF. The write end's CLOEXEC is cleared only inside the forked
    /// child's `pre_exec`, immediately before `exec`, so exactly the intended
    /// grandchild inherits it. macOS lacks `pipe2`, so `pipe()` + `fcntl` leaves
    /// a microscopic non-atomic window; the multi-millisecond creation→spawn
    /// window that actually matters is closed.
    pub fn new() -> std::io::Result<Self> {
        let mut fds = [0 as RawFd; 2];
        // SAFETY: `pipe` writes exactly two fds into the provided array and
        // returns 0 on success; we check the return before wrapping the fds.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `pipe` just handed us two fresh, owned fds.
        let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        set_cloexec(read.as_raw_fd(), true)?;
        set_cloexec(write.as_raw_fd(), true)?;
        Ok(Self {
            read,
            write: Some(write),
        })
    }

    /// The write-end fd number: advertised to the child via [`ENV_READINESS_FD`]
    /// and handed to `pre_exec` (which clears its CLOEXEC before exec). Valid
    /// until [`close_write`](Self::close_write).
    pub fn write_fd(&self) -> RawFd {
        self.write
            .as_ref()
            .expect("write end still held")
            .as_raw_fd()
    }

    /// Drop the parent's copy of the write end. The parent must do this after
    /// the spawn so it is not itself a writer keeping the pipe open — otherwise
    /// [`await_ready`](Self::await_ready) would never observe EOF on the
    /// grandchild's death.
    pub fn close_write(&mut self) {
        self.write = None;
    }

    /// Poll the read end for one complete frame, bounded by `deadline`. Consumes
    /// the pipe (the read end closes on return). Returns as soon as a
    /// newline-terminated frame arrives, or on EOF (all write ends closed), or
    /// when `deadline` elapses ([`Readiness::Timeout`]) — the wedge backstop.
    ///
    /// Reading one framed line (rather than to EOF) means a complete `R<pid>\n`
    /// is honored even if some stray duplicate of the write end lingers open, so
    /// confirmation never depends on every descriptor closing.
    pub fn await_ready(self, deadline: Duration) -> Readiness {
        // The parent must have already closed its own write end.
        debug_assert!(self.write.is_none(), "close_write() before await_ready()");
        let fd = self.read.as_raw_fd();
        let start = Instant::now();
        let mut buf: Vec<u8> = Vec::with_capacity(64);
        loop {
            let elapsed = start.elapsed();
            if elapsed >= deadline {
                return Readiness::Timeout;
            }
            let remaining_ms =
                i32::try_from(deadline.saturating_sub(elapsed).as_millis()).unwrap_or(i32::MAX);
            let mut pfd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: single valid pollfd; `poll` only reads `events` and writes
            // `revents`.
            let rc = unsafe { libc::poll(std::ptr::from_mut(&mut pfd), 1, remaining_ms) };
            if rc < 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Readiness::Malformed(format!("readiness poll failed: {err}"));
            }
            if rc == 0 {
                return Readiness::Timeout;
            }
            let mut chunk = [0u8; 128];
            // SAFETY: `read` writes at most `chunk.len()` bytes into `chunk`.
            let n =
                unsafe { libc::read(fd, chunk.as_mut_ptr().cast::<libc::c_void>(), chunk.len()) };
            if n < 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Readiness::Malformed(format!("readiness read failed: {err}"));
            }
            if n == 0 {
                // EOF: every writer closed. Classify whatever partial bytes (if
                // any) arrived — empty ⇒ Died, non-terminated ⇒ Malformed.
                return parse_readiness(&buf);
            }
            buf.extend_from_slice(&chunk[..n as usize]);
            if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                return parse_readiness(&buf[..=pos]);
            }
            if buf.len() > MAX_MSG {
                return Readiness::Malformed(
                    "readiness frame exceeded size limit before a newline".to_string(),
                );
            }
        }
    }
}

/// Grandchild (supervisor) side. Holds the inherited write-end fd until it
/// reports readiness or a boot error, then closes it. If dropped without
/// reporting, the fd closes anyway and the parent sees EOF (`Died`).
pub struct ReadinessReporter {
    fd: Option<OwnedFd>,
}

impl ReadinessReporter {
    /// Take ownership of the inherited write-end fd named by
    /// [`ENV_READINESS_FD`]. Returns a no-op reporter when the variable is unset
    /// (the lenient spawn paths) or fails validation.
    ///
    /// Validated, not trusted: the fd must be above stdio (never adopt/close
    /// stdin/out/err — `TASKFLEET_READINESS_FD=1` must not steal stdout) and must be
    /// an open pipe (`fstat` `S_IFIFO`), guarding against a stale/hand-set env
    /// value pointing at an unrelated descriptor. On adoption we re-set
    /// `FD_CLOEXEC` (the parent cleared it only for our one `exec`) so a
    /// subprocess spawned during boot cannot inherit the readiness writer.
    pub fn from_env() -> Self {
        let fd = std::env::var(ENV_READINESS_FD)
            .ok()
            .and_then(|v| v.trim().parse::<RawFd>().ok())
            .filter(|&fd| fd > libc::STDERR_FILENO)
            .filter(|&fd| is_writable_pipe(fd))
            .filter(|&fd| set_cloexec(fd, true).is_ok())
            // SAFETY: validated above as an open pipe fd the parent passed us. We
            // take sole ownership so it is closed exactly once, on report or drop.
            .map(|raw| unsafe { OwnedFd::from_raw_fd(raw) });
        Self { fd }
    }

    /// Report a confirmed boot carrying the supervisor's own pid, then close the
    /// write end so the parent's `read()` returns. Idempotent-ish: only the
    /// first `ready`/`error` writes; later calls are no-ops.
    pub fn ready(&mut self, pid: u32) {
        #[cfg(target_os = "linux")]
        crate::linux_lifetime::notify_ready(pid);
        self.emit(format!("{}{pid}\n", TAG_READY as char));
    }

    /// Report a structured boot failure the parent will surface as the real
    /// reason, then close the write end. Newlines/tabs in `message` are
    /// flattened so the single-line wire format holds.
    pub fn error(&mut self, code: &str, message: &str) {
        let flat = message.replace(['\n', '\t'], " ");
        let frame = format!("{}{code}\t{flat}\n", TAG_ERROR as char);
        #[cfg(target_os = "linux")]
        crate::linux_lifetime::write_receipt(&frame);
        self.emit(frame);
    }

    fn emit(&mut self, msg: String) {
        if let Some(fd) = self.fd.take() {
            write_all(fd.as_raw_fd(), msg.as_bytes());
            // `fd` drops here → the write end closes → the parent unblocks.
        }
    }
}

/// Write the whole buffer, retrying short writes and `EINTR`. Best-effort: if
/// the parent has already gone away (`EPIPE`) or the fd is otherwise unwritable
/// there is nothing useful to do — the parent will see EOF and treat the boot as
/// failed. Kept as a raw `write` loop (not `File::write_all`) so the partial-
/// write retry is explicit and self-contained.
fn write_all(fd: RawFd, mut buf: &[u8]) {
    while !buf.is_empty() {
        // SAFETY: `write` reads `buf.len()` bytes from `buf` and does not retain
        // the pointer; `fd` is owned by the caller for the duration.
        let n = unsafe { libc::write(fd, buf.as_ptr().cast::<libc::c_void>(), buf.len()) };
        if n < 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return;
        }
        if n == 0 {
            return;
        }
        buf = &buf[n as usize..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deadline for round-trip tests: long enough that a same-process write is
    /// never a false timeout, short enough to keep the suite fast.
    const TEST_DEADLINE: Duration = Duration::from_secs(5);

    #[test]
    fn parse_ready_line() {
        assert_eq!(
            parse_readiness(b"R12345\n"),
            Readiness::Ready { pid: 12345 }
        );
    }

    #[test]
    fn parse_ready_without_trailing_newline_is_malformed() {
        // A truncated write (missing the terminator) must NOT be accepted as a
        // ready signal — `R7777` truncated to `R777` would confirm a wrong pid.
        match parse_readiness(b"R777") {
            Readiness::Malformed(_) => {}
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn parse_ready_trailing_garbage_is_malformed() {
        // Digits followed by junk before the newline is a malformed frame, not
        // `Ready { pid: 123 }` with the junk silently dropped.
        match parse_readiness(b"R123xyz\n") {
            Readiness::Malformed(_) => {}
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn parse_ready_overflow_pid_is_malformed() {
        // Above pid_t::MAX (and/or u32) → rejected, never truncated into range.
        match parse_readiness(b"R99999999999\n") {
            Readiness::Malformed(_) => {}
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn parse_empty_is_died() {
        assert_eq!(parse_readiness(b""), Readiness::Died);
    }

    #[test]
    fn parse_error_line() {
        assert_eq!(
            parse_readiness(b"Esupervisor_already_running\tpid 42 is alive\n"),
            Readiness::Error {
                code: "supervisor_already_running".into(),
                message: "pid 42 is alive".into(),
            }
        );
    }

    #[test]
    fn parse_error_without_message() {
        assert_eq!(
            parse_readiness(b"Elock_error\n"),
            Readiness::Error {
                code: "lock_error".into(),
                message: String::new(),
            }
        );
    }

    #[test]
    fn parse_partial_ready_tag_only_is_malformed() {
        // Writer crashed after the tag byte but before any pid digit.
        match parse_readiness(b"R\n") {
            Readiness::Malformed(_) => {}
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn parse_ready_zero_pid_is_malformed() {
        match parse_readiness(b"R0\n") {
            Readiness::Malformed(_) => {}
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn parse_garbage_is_malformed() {
        match parse_readiness(b"xyzzy\n") {
            Readiness::Malformed(_) => {}
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    /// Round-trip over a real pipe: reporter writes ready, parent reads it.
    #[test]
    fn pipe_roundtrip_ready() {
        let mut pipe = ReadinessPipe::new().unwrap();
        let mut reporter = reporter_for(&pipe);
        pipe.close_write();
        reporter.ready(4242);
        assert_eq!(
            pipe.await_ready(TEST_DEADLINE),
            Readiness::Ready { pid: 4242 }
        );
    }

    /// Round-trip: the reporter is dropped without reporting → the parent sees
    /// EOF and classifies it as a death during init.
    #[test]
    fn pipe_roundtrip_died_on_drop() {
        let mut pipe = ReadinessPipe::new().unwrap();
        let reporter = reporter_for(&pipe);
        pipe.close_write();
        drop(reporter); // dies without signalling
        assert_eq!(pipe.await_ready(TEST_DEADLINE), Readiness::Died);
    }

    /// Round-trip: the reporter articulates a structured error.
    #[test]
    fn pipe_roundtrip_error() {
        let mut pipe = ReadinessPipe::new().unwrap();
        let mut reporter = reporter_for(&pipe);
        pipe.close_write();
        reporter.error("supervisor_already_running", "pid 9 is alive");
        assert_eq!(
            pipe.await_ready(TEST_DEADLINE),
            Readiness::Error {
                code: "supervisor_already_running".into(),
                message: "pid 9 is alive".into(),
            }
        );
    }

    /// The reporter closes the fd after `ready`, so the parent returns as soon
    /// as the framed line arrives — it never blocks waiting for more.
    #[test]
    fn reporter_closes_after_ready() {
        let mut pipe = ReadinessPipe::new().unwrap();
        let mut reporter = reporter_for(&pipe);
        pipe.close_write();
        reporter.ready(5);
        // Second call is a no-op (fd already taken); no panic, no double close.
        reporter.ready(6);
        assert_eq!(pipe.await_ready(TEST_DEADLINE), Readiness::Ready { pid: 5 });
    }

    /// A wedged supervisor that holds the write end open without ever writing is
    /// caught by the backstop deadline, not an infinite hang.
    #[test]
    fn pipe_roundtrip_timeout_on_wedge() {
        let mut pipe = ReadinessPipe::new().unwrap();
        // Keep the reporter (and thus the write end) alive for the whole read.
        let _reporter = reporter_for(&pipe);
        pipe.close_write();
        assert_eq!(
            pipe.await_ready(Duration::from_millis(150)),
            Readiness::Timeout
        );
    }

    #[test]
    #[serial_test::serial(taskfleet_readiness_env)]
    fn from_env_absent_is_noop() {
        // With the variable unset, the reporter holds no fd and reporting does
        // nothing (the lenient spawn paths rely on this).
        std::env::remove_var(ENV_READINESS_FD);
        let mut reporter = ReadinessReporter::from_env();
        assert!(reporter.fd.is_none());
        reporter.ready(1); // must not panic
        reporter.error("x", "y"); // must not panic
    }

    #[test]
    #[serial_test::serial(taskfleet_readiness_env)]
    fn from_env_rejects_stdio_fd() {
        // A stale/hand-set fd pointing at stdout must never be adopted (adopting
        // it would close stdout when the reporter drops).
        std::env::set_var(ENV_READINESS_FD, "1");
        let reporter = ReadinessReporter::from_env();
        std::env::remove_var(ENV_READINESS_FD);
        assert!(reporter.fd.is_none(), "must not adopt fd 1 (stdout)");
    }

    #[test]
    #[serial_test::serial(taskfleet_readiness_env)]
    fn from_env_rejects_non_pipe_fd() {
        // A high fd that is a regular file (a temp file here) is not a pipe and
        // must be rejected by the fstat guard.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let raw = tmp.as_file().as_raw_fd();
        std::env::set_var(ENV_READINESS_FD, raw.to_string());
        let reporter = ReadinessReporter::from_env();
        std::env::remove_var(ENV_READINESS_FD);
        assert!(reporter.fd.is_none(), "must not adopt a non-pipe fd");
    }

    /// Build a reporter owning its own dup'd copy of the pipe's write end, so a
    /// test can drive the grandchild side without disturbing the pipe's own
    /// bookkeeping. `dup(2)` clears CLOEXEC on the new fd, matching what the
    /// grandchild sees post-`exec`.
    fn reporter_for(pipe: &ReadinessPipe) -> ReadinessReporter {
        let d = unsafe { libc::dup(pipe.write_fd()) };
        assert!(d >= 0, "dup failed");
        // SAFETY: `d` is a fresh owned fd from `dup`.
        ReadinessReporter {
            fd: Some(unsafe { OwnedFd::from_raw_fd(d) }),
        }
    }
}
