//! Supervisor PID file: atomic write/read + liveness check.
//!
//! `<run-dir>/supervisor.pid` is the single source of truth for "which
//! supervisor owns this run". A startup check refuses to launch if the
//! file exists AND the recorded PID is still alive. On clean shutdown
//! the supervisor removes the file. After a crash the file is stale and
//! `run reattach` (or a manual `kill`) is required.

use std::io::{ErrorKind, Read, Write};
use std::path::Path;

use taskfleet_core::{RunLock, RunPaths};

use crate::error::CliError;
use crate::supervise::watchdog::RecordedStart;

/// Reject a `supervisor.pid` whose final component is a symlink before any
/// open follows it, mapping the refusal to a clean `pid_file_symlink_rejected`
/// envelope. `supervisor.pid` is CLI-owned (it does not route through
/// `taskfleet_core`'s run-state guards), so this mirrors `taskfleet_core`'s best-effort
/// `symlink_metadata` containment here. Paired with `O_NOFOLLOW` on the actual
/// open (see [`taskfleet_core::nofollow`]) so a leaf swapped *after* this check but
/// *before* the open is still refused at the file level.
///
/// An absent file is accepted (`Ok`): a not-yet-written pid file is normal. Any
/// other `symlink_metadata` failure surfaces as `io_error`.
fn reject_pid_symlink(path: &Path) -> Result<(), CliError> {
    match std::fs::symlink_metadata(path) {
        Ok(md) if md.file_type().is_symlink() => Err(CliError::system(
            "pid_file_symlink_rejected",
            format!(
                "supervisor pid file {} is a symlink (refusing to follow it)",
                path.display()
            ),
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CliError::system(
            "io_error",
            format!("stat {}: {}", path.display(), e),
        )),
    }
}

/// Read `path` to a string with `O_NOFOLLOW`, returning `None` on any failure
/// (absent, unreadable, or — the security-relevant case — a symlink, which
/// fails the open with `ELOOP` instead of being followed). Callers treat a
/// `None` here as "no valid owner record", which is the safe default: a forged
/// symlink never redirects the read, and the recorded owner is read as absent
/// rather than from an attacker-chosen target.
fn read_pid_string(path: &Path) -> Option<String> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    taskfleet_core::nofollow(&mut opts);
    let mut f = opts.open(path).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s)
}

/// Atomically write `pid` (plus its process start identity, when readable)
/// to `path` via tempfile + rename. The on-disk format is one line:
/// `"<pid>"`, `"<pid> <start_time_secs>"`, or
/// `"<pid> <start_time_secs|-> <start_identity>"`. The start identity is the
/// §7.5 PID-identity defense (§7.6): a later stale-check can tell a recycled
/// PID from the original supervisor. The kernel identity token decides when
/// present (see [`watchdog::RecordedStart`](crate::supervise::watchdog::RecordedStart));
/// the seconds token stays second so a reader that only knows the two-token
/// format still parses it. Mirrors `taskfleet_core::atomic` without pulling in
/// the JSON layer.
pub fn write_pid(path: &Path, pid: u32) -> Result<(), CliError> {
    let parent = path.parent().ok_or_else(|| {
        CliError::system(
            "io_error",
            format!("pid path {} has no parent", path.display()),
        )
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|e| CliError::system("io_error", format!("mkdir {}: {}", parent.display(), e)))?;
    // Refuse a symlinked destination before we build the temp + rename over it
    // (rename does not follow the leaf, but the reject keeps the failure mode a
    // clean `pid_file_symlink_rejected` rather than silently clobbering the link).
    reject_pid_symlink(path)?;
    let start = crate::supervise::watchdog::RecordedStart::capture(pid);
    let contents = if start.is_recorded() {
        let secs = start
            .secs
            .map_or_else(|| "-".to_string(), |st| st.to_string());
        match start.identity {
            Some(identity) => format!("{pid} {secs} {identity}"),
            None => format!("{pid} {secs}"),
        }
    } else {
        // Degrade to the legacy bare-integer format (a later
        // stale-check falls back to plain liveness). This loses the
        // §7.6 PID-reuse defense, so make it visible rather than
        // silent — we can normally always read our own start-time.
        tracing::warn!(
            target: "taskfleet::supervise",
            pid,
            "could not read own start_time; writing legacy pid file (no recycle defense)"
        );
        pid.to_string()
    };
    let tmp = parent.join(format!(".supervisor.pid.tmp.{}", std::process::id()));
    write_tmp_exclusive(&tmp, contents.as_bytes())?;
    std::fs::rename(&tmp, path)
        .map_err(|e| CliError::system("io_error", format!("rename {}: {}", path.display(), e)))?;
    Ok(())
}

/// Write `bytes` to `tmp` via an exclusive create (`O_CREAT|O_EXCL` +
/// `O_NOFOLLOW`), mirroring `taskfleet_core::atomic`: `create_new` refuses an
/// existing temp (and any symlink planted there), so a stale or forged temp
/// cannot be followed or appended to. A leftover temp from a crashed write
/// (same pid, same run) is removed and the create retried once.
fn write_tmp_exclusive(tmp: &Path, bytes: &[u8]) -> Result<(), CliError> {
    let open = || {
        let mut opts = std::fs::OpenOptions::new();
        opts.create_new(true).write(true);
        taskfleet_core::nofollow(&mut opts);
        opts.open(tmp)
    };
    let mut f = match open() {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            std::fs::remove_file(tmp).map_err(|e| {
                CliError::system("io_error", format!("rm stale {}: {}", tmp.display(), e))
            })?;
            open().map_err(|e| {
                CliError::system("io_error", format!("write {}: {}", tmp.display(), e))
            })?
        }
        Err(e) => {
            return Err(CliError::system(
                "io_error",
                format!("write {}: {}", tmp.display(), e),
            ))
        }
    };
    f.write_all(bytes)
        .map_err(|e| CliError::system("io_error", format!("write {}: {}", tmp.display(), e)))?;
    Ok(())
}

/// Atomically claim supervisor ownership of `paths`' run under the run
/// `flock`, closing the §7.6 TOCTOU race.
///
/// The original startup check read a stale `supervisor.pid`, then wrote its
/// own with *no lock between* — so two concurrent `supervise` / `run
/// reattach`-spawned supervisors could both read "stale", both write, and
/// both enter the main loop, violating one-supervisor-per-run. Holding the
/// run flock across the read-then-write serializes the claim: exactly one
/// caller observes the slot free and writes; the loser sees the winner's
/// live pid and is rejected.
///
/// Sequence (all under the lock):
///  1. Read the existing `supervisor.pid` record (if any).
///  2. If it records a supervisor still alive (start-time identity check,
///     §7.6) → return `supervisor_already_running` *without* touching the
///     file.
///  3. Otherwise (no file, dead pid, recycled pid, or a legacy plain-integer
///     file) → write our pid + start-time atomically (tempfile + rename) and
///     return `Ok`. A legacy bare-integer file is upgraded to the
///     `"<pid> <start_time>"` format here on first claim — a non-destructive
///     migration that restores the recycle defense for the new owner.
///
/// The lock is released when the returned guard drops at end of scope; the
/// pid file itself remains as the durable ownership marker.
pub fn claim_pid_atomic(paths: &RunPaths, our_pid: u32) -> Result<(), CliError> {
    let _guard = RunLock::acquire(&paths.lock())
        .map_err(|e| CliError::system("lock_error", format!("acquire run lock: {e}")))?;
    let pid_path = paths.supervisor_pid();
    if let Some((existing, start)) = read_pid_record(&pid_path) {
        if pid_live_with_identity(existing, &start) {
            // A legacy (no recorded start-time) file cannot distinguish the
            // real owner from a recycled pid — say so explicitly so an
            // operator hit by an upgrade-then-pid-reuse lockout knows to
            // remove the file, rather than reading "is alive" and assuming a
            // healthy supervisor.
            let hint = if !start.is_recorded() {
                "; this is a legacy pid file with no identity, so a recycled \
                 pid cannot be ruled out — remove supervisor.pid if it is stale"
            } else {
                " (kill it or use `run reattach`)"
            };
            return Err(CliError::system(
                "supervisor_already_running",
                format!(
                    "supervisor pid {existing} for run {} is alive{hint}",
                    paths.run_id.as_str(),
                ),
            ));
        }
        // Stale (dead, or a recycled PID per the start-time check) or a legacy
        // bare-integer file: log and overwrite under the lock.
        tracing::warn!(
            target: "taskfleet::supervise",
            stale_pid = existing,
            "claiming run: removing stale supervisor.pid"
        );
    }
    write_pid(&pid_path, our_pid)
}

/// Narrow a stored `u32` pid into a `libc::pid_t` (signed `i32` on every
/// supported Unix), rejecting `0` and any value above `i32::MAX`. The latter
/// is the security-critical guard: a corrupt or tampered `supervisor.pid`
/// holding e.g. `4294967295` would otherwise cast to `-1`, and
/// `kill(-1, SIGTERM)` signals *every* process the user may signal. No
/// legitimate pid exceeds `i32::MAX`, so out-of-range is treated as "not a
/// real process".
pub(crate) fn to_pid_t(pid: u32) -> Option<libc::pid_t> {
    if pid == 0 || pid > libc::pid_t::MAX as u32 {
        return None;
    }
    Some(pid as libc::pid_t)
}

/// Read the recorded supervisor PID from `path`. Returns `None` if the file
/// is absent, its first token does not parse as an integer, or the value is
/// out of the valid pid range (see [`to_pid_t`]).
pub fn read_pid(path: &Path) -> Option<u32> {
    let s = read_pid_string(path)?;
    let pid = s.split_whitespace().next()?.parse::<u32>().ok()?;
    to_pid_t(pid).map(|_| pid)
}

/// Read `(pid, start)` from `path`. `start` records nothing for a legacy
/// single-integer file (written before §7.6 identity landed) or when the start
/// could not be captured at write time, and only seconds for a file written
/// before the kernel identity token.
pub fn read_pid_record(path: &Path) -> Option<(u32, RecordedStart)> {
    let s = read_pid_string(path)?;
    let mut it = s.split_whitespace();
    let pid = it.next()?.parse::<u32>().ok()?;
    // Reject an out-of-range pid (see `to_pid_t`) before it can reach any
    // `kill()` cast downstream.
    to_pid_t(pid)?;
    Some((pid, parse_start(it)))
}

/// The start tokens after the pid: seconds (`-` when unreadable), then the
/// optional kernel identity.
fn parse_start<'a>(mut tokens: impl Iterator<Item = &'a str>) -> RecordedStart {
    let secs = tokens.next().and_then(|t| t.parse::<u64>().ok());
    RecordedStart::new(secs, tokens.next())
}

/// The three ways reading a supervisor pid file can resolve, classified from a
/// **single** `open()` so the absent-vs-unreadable distinction comes from the
/// actual error the open produced — not from a second `stat` that can race the
/// file being created or removed in between (and that would otherwise fold
/// `EACCES`/`EIO`/`ELOOP` into "absent"). Consumed by
/// [`SupervisorView::probe`](crate::run::dto::SupervisorView::probe).
pub enum PidRecord {
    /// No file at the path (`open` returned `ENOENT`): no supervisor recorded —
    /// never launched, or cleanly torn down (the pid file is removed on a clean
    /// exit). These two are indistinguishable at this layer.
    Absent,
    /// A file is there but could not be turned into a valid record: `open`
    /// failed for a non-`ENOENT` reason (`ELOOP` from a rejected symlink,
    /// `EACCES`, `EIO`, …), the read failed, or the contents did not parse to a
    /// valid in-range pid. Distinct from `Absent`: something is present, we
    /// just cannot trust it.
    Unreadable,
    /// A valid record: `(pid, start)`, with nothing recorded in `start` for a
    /// legacy single-integer file. Same parse rules as [`read_pid_record`].
    Present { pid: u32, start: RecordedStart },
}

/// Classify `<run-dir>/supervisor.pid` from one `open()`. See [`PidRecord`] for
/// why this must not be a read-then-`stat` pair. Mirrors [`read_pid_string`]'s
/// `O_NOFOLLOW` open (a planted symlink fails with `ELOOP` → `Unreadable`,
/// never followed) and [`read_pid_record`]'s parse (first token is the pid, an
/// optional second token the start-time).
pub fn classify_pid_record(path: &Path) -> PidRecord {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    taskfleet_core::nofollow(&mut opts);
    let mut f = match opts.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return PidRecord::Absent,
        // ELOOP (symlink), EACCES, EIO, ENOTDIR, … — present but untrustworthy.
        Err(_) => return PidRecord::Unreadable,
    };
    let mut s = String::new();
    if f.read_to_string(&mut s).is_err() {
        return PidRecord::Unreadable;
    }
    let mut it = s.split_whitespace();
    let Some(pid) = it.next().and_then(|t| t.parse::<u32>().ok()) else {
        return PidRecord::Unreadable;
    };
    // Reject an out-of-range pid (see `to_pid_t`) before it can reach any
    // `kill()` cast downstream.
    if to_pid_t(pid).is_none() {
        return PidRecord::Unreadable;
    }
    PidRecord::Present {
        pid,
        start: parse_start(it),
    }
}

/// Liveness check for a recorded PID that additionally defends against PID
/// reuse via the §7.5 start identity check (§7.6). A recycled PID (alive, but
/// its start identity disagrees) is reported `false` (stale) so reattach is
/// not blocked forever. A legacy record with no recorded start, or a platform
/// that cannot read it, falls back to plain liveness.
pub fn pid_live_with_identity(pid: u32, recorded: &RecordedStart) -> bool {
    pid_alive(pid) && recorded.matches(pid) != Some(false)
}

/// Remove the PID file if it still records `expected_pid`. Mismatch is
/// silent: another supervisor may have taken over.
pub fn remove_if_owner(path: &Path, expected_pid: u32) {
    if read_pid(path) == Some(expected_pid) {
        let _ = std::fs::remove_file(path);
    }
}

/// `kill(pid, 0)` — returns true iff the process exists and we have
/// permission to signal it. `ESRCH` → dead, `EPERM` → alive but
/// foreign-owned (still counts as alive for liveness purposes).
pub fn pid_alive(pid: u32) -> bool {
    let Some(pid_t) = to_pid_t(pid) else {
        // 0 or out-of-range (would cast to a negative `pid_t` → process
        // group / broadcast target): never treat as a live process.
        return false;
    };
    // SAFETY: `kill(pid, 0)` is signal-free and side-effect-free on
    // POSIX; it only probes existence/permission. `pid_t` is range-checked.
    let rc = unsafe { libc::kill(pid_t, 0) };
    if rc == 0 {
        return true;
    }
    // Errno EPERM (1 on macOS/Linux) → process exists, foreign owner.
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    errno == libc::EPERM
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn round_trip_write_read() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("supervisor.pid");
        write_pid(&p, 12345).unwrap();
        assert_eq!(read_pid(&p), Some(12345));
    }

    #[test]
    fn missing_file_reads_none() {
        let dir = TempDir::new().unwrap();
        assert_eq!(read_pid(&dir.path().join("missing")), None);
    }

    #[test]
    fn remove_if_owner_matches() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("supervisor.pid");
        write_pid(&p, 999).unwrap();
        remove_if_owner(&p, 1234); // wrong pid: file stays
        assert!(p.exists());
        remove_if_owner(&p, 999);
        assert!(!p.exists());
    }

    #[test]
    fn own_pid_is_alive() {
        assert!(pid_alive(std::process::id()));
    }

    #[test]
    fn pid_zero_is_dead() {
        assert!(!pid_alive(0));
    }

    /// Security guard: a corrupt/tampered pid value above `i32::MAX` must
    /// never reach `kill()` (it would cast to a negative `pid_t` and target a
    /// process group / broadcast). It is reported dead and never read back.
    #[test]
    fn out_of_range_pid_is_dead_and_unreadable() {
        assert!(!pid_alive(u32::MAX), "u32::MAX would cast to -1");
        assert!(!pid_alive((i32::MAX as u32) + 1));
        assert!(to_pid_t(u32::MAX).is_none());
        assert!(to_pid_t(0).is_none());
        assert!(to_pid_t(i32::MAX as u32).is_some());

        let dir = TempDir::new().unwrap();
        let p = dir.path().join("supervisor.pid");
        std::fs::write(&p, "4294967295").unwrap();
        assert_eq!(read_pid(&p), None, "out-of-range pid must read as absent");
        assert_eq!(read_pid_record(&p), None);
    }

    /// A `supervisor.pid` replaced by a symlink must not be followed on write:
    /// `write_pid` refuses it with a clean `pid_file_symlink_rejected` envelope
    /// rather than renaming over (or, pre-rename, opening through) the link.
    #[cfg(unix)]
    #[test]
    fn write_pid_rejects_a_symlinked_pid_file() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("outside.pid");
        let p = dir.path().join("supervisor.pid");
        symlink(&target, &p).unwrap();

        let err = write_pid(&p, 4321).expect_err("must refuse a symlinked pid file");
        assert_eq!(err.code, "pid_file_symlink_rejected");
        // The forged link's target was never written through.
        assert!(!target.exists(), "write must not follow the symlink");
    }

    /// A symlinked `supervisor.pid` reads as absent (`None`) rather than being
    /// followed: `O_NOFOLLOW` fails the open with `ELOOP`, so a forged link
    /// never redirects the owner-record read to an attacker-chosen file.
    #[cfg(unix)]
    #[test]
    fn read_pid_does_not_follow_a_symlink() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("outside.pid");
        std::fs::write(&target, "4321 100").unwrap();
        let p = dir.path().join("supervisor.pid");
        symlink(&target, &p).unwrap();

        assert_eq!(read_pid(&p), None, "symlinked pid file must read as absent");
        assert_eq!(read_pid_record(&p), None);
    }

    /// The core invariant of the §7.6 fix: many concurrent `claim_pid_atomic`
    /// calls racing on one run yield EXACTLY ONE winner; every loser gets
    /// `supervisor_already_running`. All threads claim our own (alive,
    /// identity-matching) pid, so the loser always observes the winner's pid
    /// as live and is rejected.
    #[test]
    fn concurrent_claim_exactly_one_wins() {
        use std::sync::{Arc, Barrier};

        let dir = TempDir::new().unwrap();
        let run_dir = dir.path().join("01jxsnap000000000000000000");
        std::fs::create_dir_all(&run_dir).unwrap();
        let run_id = "01jxsnap000000000000000000";
        let our_pid = std::process::id();

        const N: usize = 8;
        let barrier = Arc::new(Barrier::new(N));
        let handles: Vec<_> = (0..N)
            .map(|_| {
                let rd = run_dir.clone();
                let b = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let paths = RunPaths::new(rd, run_id).unwrap();
                    // Line every thread up so they genuinely contend on the flock.
                    b.wait();
                    claim_pid_atomic(&paths, our_pid)
                })
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let wins = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(wins, 1, "exactly one concurrent claim must win, got {wins}");
        for r in &results {
            if let Err(e) = r {
                assert_eq!(
                    e.code, "supervisor_already_running",
                    "loser must report supervisor_already_running, got {}",
                    e.code
                );
            }
        }

        // The winner's pid is on disk.
        let p = run_dir.join("supervisor.pid");
        assert_eq!(read_pid(&p), Some(our_pid));
    }

    /// A legacy bare-integer pid file (written before the §7.6 start-time
    /// identity format) is non-destructively upgraded to `"<pid> <start>"`
    /// on the next claim, provided the recorded pid is not alive.
    #[test]
    fn claim_migrates_legacy_plain_integer_pid_file() {
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path().join("01jxsnap000000000000000000");
        std::fs::create_dir_all(&run_dir).unwrap();
        let paths = RunPaths::new(run_dir.clone(), "01jxsnap000000000000000000").unwrap();
        let pid_path = run_dir.join("supervisor.pid");

        // Legacy format: a single bare integer, no start-time token, for a
        // guaranteed-dead pid (so the claim is not blocked by a live owner).
        std::fs::write(&pid_path, "2147483646").unwrap();
        assert_eq!(
            read_pid_record(&pid_path),
            Some((2_147_483_646, RecordedStart::default()))
        );

        let our_pid = std::process::id();
        claim_pid_atomic(&paths, our_pid).expect("claim over a dead legacy pid succeeds");

        // Rewritten in the modern format: pid + start-time (when readable).
        let (pid, start) = read_pid_record(&pid_path).unwrap();
        assert_eq!(pid, our_pid);
        assert!(
            start.secs.is_some() && start.identity.is_some(),
            "claim must upgrade the legacy file to carry a start identity"
        );
    }

    /// Regression for `supervisor-false-pid-recycled`: a live supervisor whose
    /// recorded seconds drifted with a wall-clock step is still live, because
    /// the kernel identity token decides. A two-token file from an older
    /// writer keeps the seconds comparison, and a different identity is stale.
    #[test]
    fn identity_token_decides_supervisor_liveness() {
        let dir = TempDir::new().unwrap();
        let pid_path = dir.path().join("supervisor.pid");
        let our_pid = std::process::id();
        write_pid(&pid_path, our_pid).unwrap();
        let (pid, start) = read_pid_record(&pid_path).unwrap();
        assert_eq!(pid, our_pid);
        let secs = start.secs.unwrap();
        let identity = start.identity.clone().unwrap();
        assert!(pid_live_with_identity(pid, &start));

        std::fs::write(&pid_path, format!("{our_pid} {} {identity}", secs - 4)).unwrap();
        let (_, drifted) = read_pid_record(&pid_path).unwrap();
        assert!(pid_live_with_identity(our_pid, &drifted));
        assert!(matches!(
            classify_pid_record(&pid_path),
            PidRecord::Present { start, .. } if start == drifted
        ));

        std::fs::write(&pid_path, format!("{our_pid} - {identity}")).unwrap();
        let (_, no_secs) = read_pid_record(&pid_path).unwrap();
        assert_eq!(no_secs.secs, None);
        assert!(pid_live_with_identity(our_pid, &no_secs));

        std::fs::write(&pid_path, format!("{our_pid} {secs} linux:1")).unwrap();
        let (_, other) = read_pid_record(&pid_path).unwrap();
        assert!(!pid_live_with_identity(our_pid, &other));

        std::fs::write(&pid_path, format!("{our_pid} {}", secs - 4)).unwrap();
        let (_, legacy) = read_pid_record(&pid_path).unwrap();
        assert_eq!(legacy.identity, None);
        assert!(!pid_live_with_identity(our_pid, &legacy));
    }
}
