//! Per-run advisory `flock` primitive (design.md §4).
//!
//! The lock is also the source of a **compile-time witness**, [`LockedRun`],
//! that the run's exclusive lock is held. The unlocked event-append entry points
//! ([`crate::append_and_apply_unlocked`] and friends) take a `&LockedRun`
//! parameter, so the type system — not a "the caller must hold the lock" doc
//! comment — proves a writer holds the `flock` before it appends. Only the
//! **exclusive** guard mints a witness ([`RunLock::witness`]); a shared
//! (`LOCK_SH`) reader has no write capability and cannot produce one, because
//! [`RunLock`] is a typestate generic ([`Exclusive`] vs [`Shared`]) and
//! `witness` exists only on `RunLock<Exclusive>`.

use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::marker::PhantomData;
use std::path::Path;

use fs4::FileExt;

use crate::error::{Error, Result};
use crate::paths::{reject_symlink, RunPaths};

/// Typestate marker: the **exclusive** (`LOCK_EX`) lock. A `RunLock<Exclusive>`
/// grants write access — it alone can mint a [`LockedRun`] witness via
/// [`RunLock::witness`]. Uninhabited: it exists only as a type tag.
pub enum Exclusive {}

/// Typestate marker: the **shared** (`LOCK_SH`) lock. A `RunLock<Shared>` is
/// read-only and cannot produce a [`LockedRun`] witness, so it can never be
/// used to reach a write-side entry point. Uninhabited: only a type tag.
pub enum Shared {}

/// Compile-time proof that the holder is inside a critical section guarded by
/// the run's **exclusive** `flock`.
///
/// The unlocked append primitives ([`append_and_apply_unlocked`],
/// [`append_event_with_seq`], [`quarantine_corrupt_lines_unlocked`]) take a
/// `&LockedRun` so they cannot be called without proof the lock is held —
/// replacing the old "caller must already hold the `RunLock`" contract that was
/// enforced only by a doc comment. Obtain one from
/// [`RunLock::with_lock`] (which threads it into the closure) or
/// [`RunLock::witness`] (for a manually-held [`RunLock::acquire`] guard).
///
/// The witness is a zero-sized borrow of the guard: its lifetime `'a` ties it to
/// the [`RunLock`] it was minted from, so it cannot outlive the lock. It is
/// deliberately **non-`Send` and non-`Sync`** (the `PhantomData<*const …>`) — a
/// proof that *this thread* holds the `flock` must not cross a task or thread
/// boundary, where the lock would no longer apply.
///
/// [`append_and_apply_unlocked`]: crate::append_and_apply_unlocked
/// [`append_event_with_seq`]: crate::events
/// [`quarantine_corrupt_lines_unlocked`]: crate::quarantine_corrupt_lines_unlocked
pub struct LockedRun<'a> {
    // `*const` makes the witness `!Send + !Sync`; the `&'a ()` ties it to the
    // borrowed guard's lifetime. Zero-sized — it carries no data, only proof.
    _lock: PhantomData<*const &'a ()>,
}

/// RAII guard holding the run's `flock`.
///
/// Released on drop. Acquired exclusively ([`RunLock::acquire`]) and held across
/// all writes for a single logical mutation, or shared ([`RunLock::acquire_shared`])
/// for the duration of a reader's multi-file scan so a concurrent reducer cannot
/// leave the reader with a half-updated projection set. A shared guard may hold
/// no lock at all when the run has no `.lock` file yet — see
/// [`RunLock::acquire_shared`].
///
/// The `Mode` typestate ([`Exclusive`] / [`Shared`]) records which kind of lock
/// is held: only `RunLock<Exclusive>` exposes [`RunLock::witness`], so a shared
/// reader can never forge the write capability a [`LockedRun`] represents.
pub struct RunLock<Mode = Exclusive> {
    file: Option<File>,
    _mode: PhantomData<Mode>,
}

impl RunLock<Exclusive> {
    /// Acquire the exclusive lock on `<run-dir>/.lock`, creating the file if
    /// needed. Blocks until the lock is available.
    ///
    /// Best-effort symlink containment: a `.lock` that is a symlink is refused
    /// ([`Error::SymlinkStateFile`]) so `flock` cannot be taken on a file
    /// outside the run tree, which would silently break mutual exclusion. This
    /// guards the lock file's own final component; a symlinked *run root* is
    /// caught downstream when the held critical section opens `events.jsonl` /
    /// the projections (both re-guard the root before writing). See
    /// [`reject_symlink`](crate::paths) for the check-then-open TOCTOU caveat.
    pub fn acquire(lock_path: &Path) -> Result<Self> {
        // Test-only spy: count this acquisition so a test can assert a
        // multi-write transaction (e.g. `cancel_run`) takes the lock exactly
        // once, not once per appended event.
        #[cfg(test)]
        ACQUIRE_COUNT.with(|c| c.set(c.get() + 1));
        if let Some(p) = lock_path.parent() {
            std::fs::create_dir_all(p).map_err(|e| Error::io(p, e))?;
        }
        reject_symlink(lock_path, || Error::SymlinkStateFile {
            name: "lock",
            path: lock_path.to_path_buf(),
        })?;
        let mut opts = OpenOptions::new();
        opts.create(true).read(true).write(true).truncate(false);
        // `O_NOFOLLOW`: refuse to take `flock` through a symlinked `.lock`, the
        // file-level backstop to the `reject_symlink` check above.
        crate::paths::nofollow(&mut opts);
        let file = opts.open(lock_path).map_err(|e| Error::io(lock_path, e))?;
        // Fully-qualified to call fs4's trait method, not `std::fs::File::lock`
        // (an inherent method stable since 1.89 that would otherwise shadow it
        // on newer toolchains). fs4 renamed `fs2`'s `lock_exclusive` to `lock`
        // to mirror std.
        <File as FileExt>::lock(&file).map_err(|e| Error::io(lock_path, e))?;
        Ok(Self {
            file: Some(file),
            _mode: PhantomData,
        })
    }

    /// Acquire the exclusive lock on an existing lock file without creating
    /// either the lock file or its parent run directory.
    ///
    /// This is for mutation endpoints that must reject a missing/deleted run
    /// without resurrecting it as an empty directory. A canonical run has a
    /// `.lock` because its event creation path acquired the ordinary exclusive
    /// lock before publishing projections. Like [`RunLock::acquire`], this
    /// rejects symlinks and blocks until the exclusive lock is available.
    pub fn acquire_existing(lock_path: &Path) -> Result<Self> {
        let file = open_existing_lock(lock_path)?;
        <File as FileExt>::lock(&file).map_err(|e| Error::io(lock_path, e))?;
        Ok(Self {
            file: Some(file),
            _mode: PhantomData,
        })
    }

    /// Try to acquire the exclusive lock on an existing lock file without
    /// creating state or waiting. Returns `Ok(None)` when another process holds
    /// the lock. Migration preflight uses this bounded probe so a busy run is a
    /// prompt refusal rather than an unbounded wait.
    pub fn try_acquire_existing(lock_path: &Path) -> Result<Option<Self>> {
        let file = open_existing_lock(lock_path)?;
        match <File as FileExt>::try_lock(&file) {
            Ok(()) => Ok(Some(Self {
                file: Some(file),
                _mode: PhantomData,
            })),
            Err(fs4::TryLockError::WouldBlock) => Ok(None),
            Err(fs4::TryLockError::Error(error)) => Err(Error::io(lock_path, error)),
        }
    }

    /// Mint a [`LockedRun`] witness proving this exclusive guard holds the
    /// run's `flock`. The witness borrows `self`, so the borrow checker forbids
    /// it from outliving the guard (and thus the lock). Use this for the
    /// manually-held [`RunLock::acquire`] pattern — when the locked body needs
    /// control flow ([`with_lock`](RunLock::with_lock)'s closure cannot express)
    /// — then pass `&witness` to the unlocked append entry points.
    // `&self` is load-bearing despite the body not reading it: it borrows the
    // guard so the returned `LockedRun<'_>` is lifetime-tied to the held lock
    // and cannot outlive it. That is the whole point — not an accidental unused
    // receiver, so this method must stay a method, never an associated fn.
    #[allow(clippy::unused_self)]
    pub fn witness(&self) -> LockedRun<'_> {
        LockedRun { _lock: PhantomData }
    }

    /// Convenience: run `f` with the exclusive lock held, passing it a
    /// [`LockedRun`] witness it can thread into the unlocked append entry
    /// points, releasing the lock afterwards.
    pub fn with_lock<R>(paths: &RunPaths, f: impl FnOnce(&LockedRun) -> Result<R>) -> Result<R> {
        let guard = Self::acquire(&paths.lock())?;
        let r = f(&guard.witness());
        drop(guard);
        r
    }
}

fn open_existing_lock(lock_path: &Path) -> Result<File> {
    reject_symlink(lock_path, || Error::SymlinkStateFile {
        name: "lock",
        path: lock_path.to_path_buf(),
    })?;
    let mut opts = OpenOptions::new();
    opts.read(true).write(true).truncate(false);
    crate::paths::nofollow(&mut opts);
    opts.open(lock_path).map_err(|e| Error::io(lock_path, e))
}

impl RunLock<Shared> {
    /// Acquire a **shared** (`LOCK_SH`) lock on an existing `<run-dir>/.lock`,
    /// blocking until no writer holds the exclusive lock. The mirror of
    /// [`RunLock::acquire`] for the read side: many readers may hold the shared
    /// lock at once, but the exclusive lock a reducer takes excludes them all,
    /// so a multi-file read taken under this lock never observes the torn state
    /// a mid-flight reducer would otherwise expose (design.md §4).
    ///
    /// Unlike [`RunLock::acquire`], this **never creates** the run directory or
    /// the lock file — a reader must not bring run-tree state into existence. A
    /// missing `.lock` means no writer has ever locked this run, so a lock-free
    /// read is already coherent: the returned guard then holds nothing and drops
    /// to a no-op. The same symlink containment as [`RunLock::acquire`] applies;
    /// the file is opened read-only with `O_NOFOLLOW`.
    ///
    /// Nesting is safe: a shared lock is compatible with other shared locks, so
    /// a read path that calls another read helper (each on its own descriptor)
    /// cannot deadlock against itself.
    pub fn acquire_shared(lock_path: &Path) -> Result<Self> {
        reject_symlink(lock_path, || Error::SymlinkStateFile {
            name: "lock",
            path: lock_path.to_path_buf(),
        })?;
        let mut opts = OpenOptions::new();
        // Read-only, no `create`: a reader never authors the lock file or its
        // parent run directory.
        opts.read(true);
        // `O_NOFOLLOW`: refuse to take `flock` through a symlinked `.lock`, the
        // file-level backstop to the `reject_symlink` check above.
        crate::paths::nofollow(&mut opts);
        let file = match opts.open(lock_path) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                // No `.lock` (or no run dir): nothing to serialize against.
                return Ok(Self {
                    file: None,
                    _mode: PhantomData,
                });
            }
            Err(e) => return Err(Error::io(lock_path, e)),
        };
        // Fully-qualified to call fs4's trait method rather than the inherent
        // `std::fs::File::lock_shared` (stable 1.89) that would shadow it on
        // newer toolchains — same reasoning as the exclusive `lock` above.
        <File as FileExt>::lock_shared(&file).map_err(|e| Error::io(lock_path, e))?;
        Ok(Self {
            file: Some(file),
            _mode: PhantomData,
        })
    }

    /// Convenience: run `f` with the shared lock held, releasing afterwards.
    /// The read-side counterpart to [`RunLock::with_lock`] — but the closure
    /// gets **no** [`LockedRun`] witness: a shared reader has no write
    /// capability, so it can never reach a write-side entry point. (It also
    /// takes the lock path directly rather than a [`RunPaths`], since a reader
    /// may run before the run dir is fully materialized.)
    pub fn with_shared_lock<T>(lock_path: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let guard = Self::acquire_shared(lock_path)?;
        let r = f();
        drop(guard);
        r
    }
}

impl<Mode> Drop for RunLock<Mode> {
    fn drop(&mut self) {
        if let Some(f) = self.file.take() {
            // Best-effort unlock — kernel releases on file close anyway.
            // Use the fs4 trait method explicitly to avoid clashing with
            // `std::fs::File::unlock` (available on supported stable toolchains).
            let _ = <File as FileExt>::unlock(&f);
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Test-only spy counter for [`RunLock::acquire`] calls on the current
    /// thread. `cargo test` runs each test on its own thread and `cancel_run`
    /// does all its work synchronously on the calling thread, so a test can
    /// reset this and assert the exact number of lock acquisitions a
    /// transaction performed without cross-test interference.
    pub(crate) static ACQUIRE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn acquire_succeeds_on_a_regular_lock_file() {
        let tmp = TempDir::new().unwrap();
        let lock = tmp.path().join(".lock");
        // First acquire creates the file; a second acquire after drop succeeds.
        drop(RunLock::acquire(&lock).unwrap());
        assert!(RunLock::acquire(&lock).is_ok());
    }

    /// Build a valid `RunPaths` rooted at a fresh temp dir for the witness tests.
    fn fresh_paths(tmp: &TempDir) -> RunPaths {
        let run_id = "01jxsnap000000000000000000";
        let dir = tmp.path().join(run_id);
        std::fs::create_dir_all(&dir).unwrap();
        RunPaths::new(dir, run_id).unwrap()
    }

    /// `with_lock` runs the closure under the exclusive lock, hands it a
    /// [`LockedRun`] witness, and returns the closure's value.
    #[test]
    fn with_lock_passes_a_witness_and_returns_the_closure_value() {
        let tmp = TempDir::new().unwrap();
        let paths = fresh_paths(&tmp);
        let got = RunLock::with_lock(&paths, |_witness: &LockedRun| Ok(7u8)).unwrap();
        assert_eq!(got, 7);
    }

    #[test]
    fn acquire_existing_never_creates_missing_state() {
        let tmp = TempDir::new().unwrap();
        let run = tmp.path().join("missing-run");
        let lock = run.join(".lock");
        assert!(RunLock::acquire_existing(&lock).is_err());
        assert!(!run.exists(), "no-create acquire must not author a run dir");

        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(&lock, []).unwrap();
        assert!(RunLock::acquire_existing(&lock).is_ok());
    }

    /// A manually-held exclusive guard ([`RunLock::acquire`]) can mint a witness
    /// — the escape hatch for lock-held bodies that `with_lock`'s closure shape
    /// cannot express.
    #[test]
    fn manually_acquired_exclusive_guard_mints_a_witness() {
        let tmp = TempDir::new().unwrap();
        let lock = tmp.path().join(".lock");
        let guard = RunLock::acquire(&lock).unwrap();
        // The witness borrows the guard, so it cannot outlive the held lock.
        let _witness: LockedRun<'_> = guard.witness();
    }

    /// A shared lock on a run with no `.lock` file is a no-op guard, not an
    /// error: a reader must not create run-tree state, and a missing lock file
    /// means no writer can race the read anyway.
    #[test]
    fn acquire_shared_on_missing_lock_file_is_a_noop_guard() {
        let tmp = TempDir::new().unwrap();
        let lock = tmp.path().join(".lock");
        let guard = RunLock::acquire_shared(&lock).expect("missing lock file is fine");
        assert!(guard.file.is_none(), "no lock file ⇒ guard holds nothing");
        // The read must not have created the lock file (or its parent run dir).
        assert!(!lock.exists(), "a reader must never author the lock file");
    }

    /// A writer holding the exclusive lock blocks a shared reader until release.
    #[test]
    fn exclusive_writer_blocks_shared_reader_until_release() {
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        let tmp = TempDir::new().unwrap();
        let lock = tmp.path().join(".lock");
        // The writer's exclusive acquire creates the lock file the reader opens.
        let writer = RunLock::acquire(&lock).unwrap();

        let (tx, rx) = mpsc::channel();
        let lock2 = lock.clone();
        let reader = thread::spawn(move || {
            // Blocks until the exclusive lock is released.
            let _g = RunLock::acquire_shared(&lock2).unwrap();
            tx.send(()).unwrap();
        });

        // While the exclusive lock is held, the reader cannot proceed.
        assert!(
            rx.recv_timeout(Duration::from_millis(250)).is_err(),
            "shared reader must block while the exclusive lock is held"
        );
        drop(writer);
        // Once released, the reader acquires the shared lock and reports.
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "shared reader must proceed after the exclusive lock is released"
        );
        reader.join().unwrap();
    }

    /// Two shared readers hold the lock concurrently — neither blocks the other.
    #[test]
    fn two_shared_readers_proceed_concurrently() {
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        let tmp = TempDir::new().unwrap();
        let lock = tmp.path().join(".lock");
        // Create the lock file (writer makes it, then releases).
        drop(RunLock::acquire(&lock).unwrap());

        // First reader takes and holds the shared lock.
        let r1 = RunLock::acquire_shared(&lock).unwrap();
        assert!(
            r1.file.is_some(),
            "lock file exists ⇒ real shared lock held"
        );

        // A second reader must acquire it without blocking on the first.
        let (tx, rx) = mpsc::channel();
        let lock2 = lock.clone();
        let r2 = thread::spawn(move || {
            let _g = RunLock::acquire_shared(&lock2).unwrap();
            tx.send(()).unwrap();
        });
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "a second shared reader must not block on the first"
        );
        r2.join().unwrap();
    }

    /// Nested `with_shared_lock` calls (each on its own descriptor) must not
    /// deadlock — shared locks are compatible with one another.
    #[test]
    fn nested_shared_locks_do_not_deadlock() {
        let tmp = TempDir::new().unwrap();
        let lock = tmp.path().join(".lock");
        drop(RunLock::acquire(&lock).unwrap());
        let r = RunLock::with_shared_lock(&lock, || RunLock::with_shared_lock(&lock, || Ok(42)))
            .unwrap();
        assert_eq!(r, 42);
    }

    #[test]
    fn nonblocking_existing_probe_reports_contention_without_waiting() {
        let tmp = TempDir::new().unwrap();
        let lock = tmp.path().join(".lock");
        let held = RunLock::acquire(&lock).unwrap();
        assert!(RunLock::try_acquire_existing(&lock).unwrap().is_none());
        drop(held);
        assert!(RunLock::try_acquire_existing(&lock).unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn acquire_rejects_a_symlinked_lock_file() {
        // A symlinked `.lock` would take `flock` on a file outside the run,
        // silently breaking mutual exclusion — refuse it.
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("outside.lock");
        let lock = tmp.path().join(".lock");
        symlink(&target, &lock).unwrap();
        assert!(matches!(
            RunLock::acquire(&lock),
            Err(Error::SymlinkStateFile { name: "lock", .. })
        ));
        // The forged lock never touched the symlink target.
        assert!(!target.exists());
    }
}
