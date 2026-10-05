//! `--idempotency-key` storage for `run create`.
//!
//! One file per `(source_repo, source_branch, key)` triple under
//! `<root>/idempotency/<hash>`. File contents are the resolved run-id
//! that the first call returned; a second call with the same key
//! returns that run-id and short-circuits.
//!
//! The hash is a 128-bit FNV-1a digest rendered as 32 hex chars.
//! FNV-1a is chosen over `std::hash::DefaultHasher` because the latter
//! is explicitly documented as not stable across Rust versions — a
//! rustc upgrade would silently invalidate every persisted idempotency
//! key. FNV-1a is a stable, dependency-free algorithm.
//!
//! Collisions across distinct `(repo, branch, key)` triples would
//! mis-route a retry to the wrong run-id, but the 128-bit space makes
//! that essentially impossible on a single workstation. The components
//! are length-prefixed before hashing to avoid the canonical FNV
//! ambiguity (`a||bc` vs. `ab||c`).
//!
//! ## Reservation lifecycle
//!
//! [`reserve`] records both the proposed run id and a durable creator identity
//! (PID, process start time, and lease start). A per-key `flock` serializes the
//! reservation's read/replace/remove operations. If the creator is killed before
//! publication, a retry first proves that exact owner dead, then uses
//! [`reclaim`] to compare-and-replace the stale record. A live or unverifiable
//! owner is never raced. Legacy run-id-only records remain replayable after
//! publication, but cannot be reclaimed because they carry no owner identity.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use fs4::{FileExt, TryLockError};
use serde::{Deserialize, Serialize};

use crate::error::CliError;
use crate::home;

const FNV_OFFSET_LOW: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_OFFSET_HIGH: u64 = 0x8422_2325_cbf2_9ce4;
const FNV_PRIME: u64 = 0x100_0000_01b3;

fn fnv1a64(seed: u64, bytes: &[u8]) -> u64 {
    let mut h = seed;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

fn key_hash(repo: Option<&str>, branch: Option<&str>, key: &str) -> String {
    // Two independent 64-bit FNV-1a passes with different seeds give
    // 128 bits of output without pulling in `sha2` and friends.
    let mut low = FNV_OFFSET_LOW;
    let mut high = FNV_OFFSET_HIGH;
    for part in [repo.unwrap_or(""), branch.unwrap_or(""), key] {
        let len = (part.len() as u64).to_le_bytes();
        low = fnv1a64(low, &len);
        high = fnv1a64(high, &len);
        low = fnv1a64(low, part.as_bytes());
        high = fnv1a64(high, part.as_bytes());
    }
    format!("{high:016x}{low:016x}")
}

/// Path to a key's reservation file under `<root>/idempotency/<hash>`.
///
/// The `_in` functions take the Taskfleet root explicitly so tests can
/// point at a scratch dir without mutating the process-global
/// `TASKFLEET_HOME` (which would race with other unit tests). The public
/// wrappers (`lookup` / `reserve` / `release`) resolve the root via `home` and
/// delegate.
fn file_path_in(root: &Path, repo: Option<&str>, branch: Option<&str>, key: &str) -> PathBuf {
    root.join("idempotency").join(key_hash(repo, branch, key))
}

fn lock_path_in(root: &Path, repo: Option<&str>, branch: Option<&str>, key: &str) -> PathBuf {
    root.join("idempotency")
        .join(format!("{}.lock", key_hash(repo, branch, key)))
}

/// Durable identity of the process materializing a reserved run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreatorLease {
    pub pid: u32,
    pub pid_start_secs: Option<u64>,
    pub started_at: DateTime<Utc>,
    /// Absolute path of the inherited materializer flock. New creators hold
    /// this lock in both the CLI and native materializer process tree until publication.
    #[serde(default)]
    pub materializer_lease_path: Option<String>,
}

/// Lifetime lease inherited by the native materializer subprocess tree.
/// Clearing `FD_CLOEXEC` is load-bearing: if the Rust creator is `SIGKILLed`, a
/// still-running materializer keeps the flock held, so a retry cannot reclaim
/// its staging state. Lease files intentionally remain after unlock: preserved
/// cleanup reservations need an unlocked inode to classify as `Dead` on retry.
pub struct MaterializerLease {
    file: File,
    path: PathBuf,
}

impl MaterializerLease {
    pub fn acquire(root: &Path, run_id: &str) -> Result<Self, CliError> {
        let dir = root.join("idempotency").join("materializers");
        std::fs::create_dir_all(&dir)
            .map_err(|e| CliError::system("io_error", format!("mkdir {}: {e}", dir.display())))?;
        let path = dir.join(format!("{run_id}.lease"));
        let mut opts = OpenOptions::new();
        opts.create(true).read(true).write(true).truncate(false);
        taskfleet_core::nofollow(&mut opts);
        let file = opts
            .open(&path)
            .map_err(|e| CliError::system("io_error", format!("open {}: {e}", path.display())))?;
        FileExt::lock(&file)
            .map_err(|e| CliError::system("lock_error", format!("lock {}: {e}", path.display())))?;
        // SAFETY: F_GETFD/F_SETFD only inspect/update this valid owned fd.
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
        if flags < 0
            || unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, flags & !libc::FD_CLOEXEC) }
                < 0
        {
            return Err(CliError::system(
                "io_error",
                format!(
                    "make {} inheritable: {}",
                    path.display(),
                    std::io::Error::last_os_error()
                ),
            ));
        }
        Ok(Self { file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Keep the creator's flock until publication/boot, but prevent a detached
    /// supervisor from inheriting it across exec and blocking keyed retries.
    pub fn close_on_exec(&self) -> Result<(), CliError> {
        // SAFETY: F_GETFD/F_SETFD operate on this owned, open descriptor.
        let flags = unsafe { libc::fcntl(self.file.as_raw_fd(), libc::F_GETFD) };
        if flags < 0
            || unsafe {
                libc::fcntl(
                    self.file.as_raw_fd(),
                    libc::F_SETFD,
                    flags | libc::FD_CLOEXEC,
                )
            } < 0
        {
            return Err(CliError::system(
                "io_error",
                format!("protect lease fd: {}", std::io::Error::last_os_error()),
            ));
        }
        Ok(())
    }
}

impl Drop for MaterializerLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseLiveness {
    Live,
    Dead,
    Unverifiable,
}

pub fn materializer_liveness(path: &str) -> LeaseLiveness {
    let path = Path::new(path);
    let mut opts = OpenOptions::new();
    opts.read(true).write(true);
    taskfleet_core::nofollow(&mut opts);
    let file = match opts.open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LeaseLiveness::Unverifiable,
        Err(_) => return LeaseLiveness::Unverifiable,
    };
    match FileExt::try_lock(&file) {
        Ok(()) => {
            let _ = FileExt::unlock(&file);
            LeaseLiveness::Dead
        }
        Err(TryLockError::WouldBlock) => LeaseLiveness::Live,
        Err(TryLockError::Error(_)) => LeaseLiveness::Unverifiable,
    }
}

/// Durable value stored in a keyed reservation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReservationRecord {
    pub schema_version: u32,
    pub run_id: String,
    pub creator: Option<CreatorLease>,
    /// Staging run ids inherited from creators reclaimed before they could
    /// clean up. Kept in the compare-and-replaced record so another crash
    /// between reclaim and cleanup does not lose the cleanup obligation.
    #[serde(default)]
    pub stale_run_ids: Vec<String>,
    /// Immutable Git plan, persisted before the external effect.
    #[serde(default)]
    pub caller_plan: Option<Box<CallerPlan>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CallerPlan {
    pub request: String,
    pub branch: String,
    pub checkout: String,
    pub base_sha: String,
}

impl ReservationRecord {
    pub fn new(run_id: &str, creator: CreatorLease) -> Self {
        Self {
            schema_version: 1,
            run_id: run_id.to_string(),
            creator: Some(creator),
            stale_run_ids: Vec::new(),
            caller_plan: None,
        }
    }
}

pub fn lookup(
    repo: Option<&str>,
    branch: Option<&str>,
    key: &str,
) -> Result<Option<ReservationRecord>, CliError> {
    let root = home::root_dir()?;
    lookup_in(&root, repo, branch, key)
}

fn lookup_in(
    root: &Path,
    repo: Option<&str>,
    branch: Option<&str>,
    key: &str,
) -> Result<Option<ReservationRecord>, CliError> {
    with_key_lock(root, repo, branch, key, || {
        read_record(&file_path_in(root, repo, branch, key))
    })
}

/// Outcome of an atomic [`reserve`].
#[must_use = "a Reservation must be inspected: ignoring AlreadyReserved would duplicate the run"]
pub enum Reservation {
    /// This caller won the key: it now owns `record.run_id` and must materialize it.
    Reserved,
    /// The key was already reserved by an earlier (possibly still in-flight)
    /// call; the record identifies the run and its creator lease.
    AlreadyReserved(ReservationRecord),
}

/// Outcome of a compare-and-replace stale-owner reclamation.
#[must_use]
pub enum Reclaim {
    Reclaimed,
    Published,
    Changed(ReservationRecord),
}

/// Drop-removes a temp file on every exit path from an atomic write.
struct TempFileGuard(PathBuf);
impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// fsync a directory so a newly-created (or removed) entry within it survives a
/// crash. Best-effort: platforms that reject `fsync` on a directory handle
/// (some non-Unix filesystems) simply skip it — the reservation stays correct
/// for the in-process concurrency case, which needs visibility, not durability.
fn fsync_dir(dir: &Path) {
    if let Ok(f) = std::fs::File::open(dir) {
        let _ = f.sync_all();
    }
}

/// Atomically reserve `key` for `record`, closing the duplicate-create race.
/// The per-key flock covers the read-then-write decision and the destination is
/// published by fsynced rename, so readers observe either the old complete
/// record or the new complete record.
pub fn reserve(
    repo: Option<&str>,
    branch: Option<&str>,
    key: &str,
    record: &ReservationRecord,
) -> Result<Reservation, CliError> {
    let root = home::root_dir()?;
    reserve_in(&root, repo, branch, key, record)
}

fn reserve_in(
    root: &Path,
    repo: Option<&str>,
    branch: Option<&str>,
    key: &str,
    record: &ReservationRecord,
) -> Result<Reservation, CliError> {
    with_key_lock(root, repo, branch, key, || {
        let p = file_path_in(root, repo, branch, key);
        if let Some(existing) = read_record(&p)? {
            return Ok(Reservation::AlreadyReserved(existing));
        }
        if write_record_exclusive(&p, record)? {
            Ok(Reservation::Reserved)
        } else {
            read_record(&p)?
                .map(Reservation::AlreadyReserved)
                .ok_or_else(|| {
                    CliError::system(
                        "io_error",
                        format!(
                            "reservation {} vanished after claim contention",
                            p.display()
                        ),
                    )
                })
        }
    })
}

/// Compare-and-replace an observed reservation after the caller has proved its
/// exact creator dead. If another caller changed the record first, return that
/// record rather than overwriting it.
pub fn reclaim(
    repo: Option<&str>,
    branch: Option<&str>,
    key: &str,
    observed: &ReservationRecord,
    replacement: &ReservationRecord,
    published_manifest: &Path,
) -> Result<Reclaim, CliError> {
    let root = home::root_dir()?;
    reclaim_in(
        &root,
        repo,
        branch,
        key,
        observed,
        replacement,
        published_manifest,
    )
}

fn reclaim_in(
    root: &Path,
    repo: Option<&str>,
    branch: Option<&str>,
    key: &str,
    observed: &ReservationRecord,
    replacement: &ReservationRecord,
    published_manifest: &Path,
) -> Result<Reclaim, CliError> {
    with_key_lock(root, repo, branch, key, || {
        if published_manifest.try_exists().map_err(|e| {
            CliError::system(
                "io_error",
                format!("check {}: {e}", published_manifest.display()),
            )
        })? {
            return Ok(Reclaim::Published);
        }
        let p = file_path_in(root, repo, branch, key);
        match read_record(&p)? {
            Some(current) if current == *observed => {
                write_record_atomic(&p, replacement)?;
                Ok(Reclaim::Reclaimed)
            }
            Some(current) => Ok(Reclaim::Changed(current)),
            None => {
                if write_record_exclusive(&p, replacement)? {
                    Ok(Reclaim::Reclaimed)
                } else {
                    read_record(&p)?.map(Reclaim::Changed).ok_or_else(|| {
                        CliError::system(
                            "io_error",
                            format!(
                                "reservation {} vanished after reclaim contention",
                                p.display()
                            ),
                        )
                    })
                }
            }
        }
    })
}

fn with_key_lock<T>(
    root: &Path,
    repo: Option<&str>,
    branch: Option<&str>,
    key: &str,
    f: impl FnOnce() -> Result<T, CliError>,
) -> Result<T, CliError> {
    let lock_path = lock_path_in(root, repo, branch, key);
    let _lock = taskfleet_core::RunLock::acquire(&lock_path).map_err(|e| {
        CliError::system(
            "lock_error",
            format!("acquire {}: {e}", lock_path.display()),
        )
    })?;
    f()
}

fn read_record(path: &Path) -> Result<Option<ReservationRecord>, CliError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(CliError::system(
                "io_error",
                format!("read {}: {e}", path.display()),
            ))
        }
    };
    if raw.trim_start().starts_with('{') {
        let record: ReservationRecord = serde_json::from_str(&raw).map_err(|e| {
            CliError::system(
                "idempotency_record_invalid",
                format!("parse {}: {e}", path.display()),
            )
        })?;
        if record.schema_version != 1 {
            return Err(CliError::system(
                "idempotency_record_unsupported",
                format!(
                    "reservation {} has unsupported schema_version {} (supported: 1)",
                    path.display(),
                    record.schema_version
                ),
            ));
        }
        Ok(Some(record))
    } else {
        // Backward compatibility: old records contained only the run id. They
        // can replay a published run, but `creator: None` makes an unpublished
        // one deliberately unreclaimable (fail closed).
        Ok(Some(ReservationRecord {
            schema_version: 0,
            run_id: raw.trim().to_string(),
            creator: None,
            stale_run_ids: Vec::new(),
            caller_plan: None,
        }))
    }
}

/// Publish an absent reservation with a hard-link CAS. The key flock is the
/// normal serialization mechanism; the link is an independent backstop on a
/// filesystem/mount that fails to honour advisory locking.
fn write_record_exclusive(path: &Path, record: &ReservationRecord) -> Result<bool, CliError> {
    let parent = path
        .parent()
        .ok_or_else(|| CliError::system("io_error", format!("{} has no parent", path.display())))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| CliError::system("io_error", format!("mkdir {}: {e}", parent.display())))?;
    let tmp = parent.join(format!(
        ".tmp-claim-{}",
        key_hash(None, None, &record.run_id)
    ));
    let _guard = TempFileGuard(tmp.clone());
    let bytes = serde_json::to_vec(record)
        .map_err(|e| CliError::system("io_error", format!("serialize reservation: {e}")))?;
    let mut opts = OpenOptions::new();
    opts.create(true).truncate(true).write(true);
    taskfleet_core::nofollow(&mut opts);
    let mut file = opts
        .open(&tmp)
        .map_err(|e| CliError::system("io_error", format!("create {}: {e}", tmp.display())))?;
    file.write_all(&bytes)
        .map_err(|e| CliError::system("io_error", format!("write {}: {e}", tmp.display())))?;
    file.sync_all()
        .map_err(|e| CliError::system("io_error", format!("fsync {}: {e}", tmp.display())))?;
    match std::fs::hard_link(&tmp, path) {
        Ok(()) => {
            fsync_dir(parent);
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(CliError::system(
            "io_error",
            format!("link {} to {}: {e}", tmp.display(), path.display()),
        )),
    }
}

fn write_record_atomic(path: &Path, record: &ReservationRecord) -> Result<(), CliError> {
    let parent = path
        .parent()
        .ok_or_else(|| CliError::system("io_error", format!("{} has no parent", path.display())))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| CliError::system("io_error", format!("mkdir {}: {e}", parent.display())))?;
    let name = path.file_name().and_then(|v| v.to_str()).ok_or_else(|| {
        CliError::system("io_error", format!("{} has no file name", path.display()))
    })?;
    // Deterministic per-key temp: the key flock excludes concurrent writers, and
    // truncate safely reuses a partial temp abandoned by SIGKILL/power loss.
    let tmp = parent.join(format!(".tmp-write-{name}"));
    let _guard = TempFileGuard(tmp.clone());
    let bytes = serde_json::to_vec(record)
        .map_err(|e| CliError::system("io_error", format!("serialize reservation: {e}")))?;
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).truncate(true).write(true);
    taskfleet_core::nofollow(&mut opts);
    let mut file = opts
        .open(&tmp)
        .map_err(|e| CliError::system("io_error", format!("create {}: {e}", tmp.display())))?;
    file.write_all(&bytes)
        .map_err(|e| CliError::system("io_error", format!("write {}: {e}", tmp.display())))?;
    file.sync_all()
        .map_err(|e| CliError::system("io_error", format!("fsync {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        CliError::system(
            "io_error",
            format!("rename {} to {}: {e}", tmp.display(), path.display()),
        )
    })?;
    fsync_dir(parent);
    Ok(())
}

/// Clear inherited cleanup obligations after every listed staging directory
/// has been removed. Ownership-checked so a stale creator cannot rewrite a
/// reservation already reclaimed by another process.
pub fn finish_stale_cleanup(
    repo: Option<&str>,
    branch: Option<&str>,
    key: &str,
    run_id: &str,
) -> Result<(), CliError> {
    let root = home::root_dir()?;
    with_key_lock(&root, repo, branch, key, || {
        let path = file_path_in(&root, repo, branch, key);
        let Some(mut record) = read_record(&path)? else {
            return Ok(());
        };
        if record.run_id != run_id {
            return Err(CliError::system(
                "idempotency_owner_changed",
                format!("reservation changed before stale cleanup completed for {run_id}"),
            ));
        }
        if !record.stale_run_ids.is_empty() {
            record.stale_run_ids.clear();
            write_record_atomic(&path, &record)?;
        }
        Ok(())
    })
}

/// Release a reservation made by [`reserve`], but ONLY if it still points at
/// `run_id`. Used when a reserved run fails to materialize and its on-disk run
/// dir is discarded (a child-spawn failure, or any error before the run is
/// durable), so a later keyed retry re-spawns cleanly rather than replaying a
/// run that no longer exists.
///
/// The ownership check makes release race-safe: it never clobbers a DIFFERENT
/// run's reservation. Once a key holds some other run's id, only that run can
/// have written it (re-reservation requires the file to be absent first, and a
/// run only ever releases its OWN id), so a stale/late release is a no-op
/// rather than an ABA that would re-open the duplicate-create window. A missing
/// or already-reassigned file is not an error.
pub fn release(
    repo: Option<&str>,
    branch: Option<&str>,
    key: &str,
    run_id: &str,
) -> Result<(), CliError> {
    let root = home::root_dir()?;
    release_in(&root, repo, branch, key, run_id)
}

fn release_in(
    root: &Path,
    repo: Option<&str>,
    branch: Option<&str>,
    key: &str,
    run_id: &str,
) -> Result<(), CliError> {
    with_key_lock(root, repo, branch, key, || {
        let p = file_path_in(root, repo, branch, key);
        let Some(record) = read_record(&p)? else {
            return Ok(());
        };
        // Only OUR reservation may be removed. A non-match means the key was
        // already reclaimed by another run, so a stale Drop guard is a no-op.
        if record.run_id != run_id {
            return Ok(());
        }
        match std::fs::remove_file(&p) {
            Ok(()) => {
                fsync_dir(p.parent().unwrap_or(root));
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(CliError::system(
                "io_error",
                format!("remove {}: {e}", p.display()),
            )),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(run_id: &str, pid: u32) -> ReservationRecord {
        ReservationRecord::new(
            run_id,
            CreatorLease {
                pid,
                pid_start_secs: Some(u64::from(pid)),
                started_at: Utc::now(),
                materializer_lease_path: None,
            },
        )
    }

    /// Golden test pinning the `(repo, branch, key) → hash` derivation.
    ///
    /// `key_hash` is the integrity boundary for `--idempotency-key`: a
    /// silent change to the FNV constants, the length-prefix shape, or
    /// the empty-component handling would invalidate every persisted
    /// key on disk and either double-execute a retried call or silently
    /// dedup against the wrong run. Any future drift from these literals
    /// is a wire-incompatible change and needs a migration plan, not a
    /// test update (issue: idempotency-hash-golden-test).
    #[test]
    fn key_hash_is_stable() {
        assert_eq!(
            key_hash(Some("repo"), Some("main"), "key-1"),
            "374d54a7713c5c1529a2efe850ddaf06"
        );
        assert_eq!(
            key_hash(None, None, "key-1"),
            "63e136949fd4014a7e5f1d5d18d98cc5"
        );
        assert_eq!(
            key_hash(Some("repo"), Some("main"), ""),
            "3b1adbcd4680d8f508f812409f6e4e60"
        );
        assert_eq!(
            key_hash(Some("räpo"), Some("main"), "key-1"),
            "f3b96680018fdf90bf5f9357e0ff987d"
        );
        // Length-prefix anti-canonical-FNV-ambiguity guard: differing
        // splits of the same byte sequence must hash differently.
        assert_ne!(
            key_hash(Some("a"), Some("bc"), "key"),
            key_hash(Some("ab"), Some("c"), "key")
        );
    }

    /// Concurrency contract for `reserve`: N threads racing the same key must
    /// yield exactly ONE `Reserved`; every loser observes the winner's run-id.
    /// This is the invariant that stops two near-simultaneous `run create`
    /// calls from both materializing (issue
    /// `idempotency-key-allowed-duplicate-run`).
    #[test]
    fn reserve_is_atomic_under_concurrency() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let key = "race-key";
        const N: usize = 16;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(N));
        let winners = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let handles: Vec<_> = (0..N)
            .map(|i| {
                let barrier = barrier.clone();
                let winners = winners.clone();
                let observed = observed.clone();
                let root = root.clone();
                std::thread::spawn(move || {
                    // Each thread proposes a distinct run-id so we can tell the
                    // single winner's id apart from the losers' proposals.
                    let my_run_id = format!("01run{i:026}");
                    barrier.wait();
                    match reserve_in(&root, None, None, key, &record(&my_run_id, i as u32 + 1))
                        .unwrap()
                    {
                        Reservation::Reserved => {
                            winners.lock().unwrap().push(my_run_id.clone());
                            observed.lock().unwrap().push(my_run_id);
                        }
                        Reservation::AlreadyReserved(existing) => {
                            observed.lock().unwrap().push(existing.run_id);
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let winners = winners.lock().unwrap();
        assert_eq!(winners.len(), 1, "exactly one thread may win the key");
        let winning_id = &winners[0];
        let observed = observed.lock().unwrap();
        assert_eq!(observed.len(), N);
        assert!(
            observed.iter().all(|id| id == winning_id),
            "every caller must resolve to the single winner's run-id; got {observed:?}"
        );

        // The persisted key holds the winner's run-id, and a later lookup sees
        // it — the fast path a fully-registered retry uses.
        assert_eq!(
            lookup_in(&root, None, None, key).unwrap().map(|r| r.run_id),
            Some(winning_id.clone())
        );
    }

    /// `release` removes a reservation so a subsequent `reserve` wins afresh;
    /// releasing a missing key is a no-op.
    #[test]
    fn release_frees_the_key() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let key = "release-key";
        let a = "01runaaaaaaaaaaaaaaaaaaaaaaaaa";
        let b = "01runbbbbbbbbbbbbbbbbbbbbbbbbb";
        let c = "01runccccccccccccccccccccccccc";

        // Releasing an unreserved key is fine.
        release_in(root, None, None, key, a).unwrap();

        assert!(matches!(
            reserve_in(root, None, None, key, &record(a, 1)).unwrap(),
            Reservation::Reserved
        ));
        // Now reserved: a second reserve loses.
        assert!(matches!(
            reserve_in(root, None, None, key, &record(b, 2)).unwrap(),
            Reservation::AlreadyReserved(_)
        ));
        // Release by the owner, then a fresh reserve wins again.
        release_in(root, None, None, key, a).unwrap();
        assert!(matches!(
            reserve_in(root, None, None, key, &record(c, 3)).unwrap(),
            Reservation::Reserved
        ));
    }

    /// Ownership-checked release: a stale release from a run that no longer
    /// owns the key must NOT clobber the current owner's reservation (else the
    /// duplicate-create window re-opens).
    #[test]
    fn reclaim_compare_and_replaces_only_the_observed_owner() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let key = "reclaim-key";
        let stale = record("01runaaaaaaaaaaaaaaaaaaaaaaaaa", 1);
        let replacement = record("01runbbbbbbbbbbbbbbbbbbbbbbbbb", 2);
        assert!(matches!(
            reserve_in(root, None, None, key, &stale).unwrap(),
            Reservation::Reserved
        ));

        let unpublished = root.join("unpublished-manifest");
        assert!(matches!(
            reclaim_in(root, None, None, key, &stale, &replacement, &unpublished,).unwrap(),
            Reclaim::Reclaimed
        ));
        assert_eq!(
            lookup_in(root, None, None, key).unwrap(),
            Some(replacement.clone())
        );

        let third = record("01runccccccccccccccccccccccccc", 3);
        assert!(matches!(
            reclaim_in(root, None, None, key, &stale, &third, &unpublished).unwrap(),
            Reclaim::Changed(current) if current == replacement
        ));
    }

    #[test]
    fn release_is_ownership_checked() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let key = "cas-key";
        let owner = "01runaaaaaaaaaaaaaaaaaaaaaaaaa";
        let stale = "01runbbbbbbbbbbbbbbbbbbbbbbbbb";

        assert!(matches!(
            reserve_in(root, None, None, key, &record(owner, 1)).unwrap(),
            Reservation::Reserved
        ));
        // A release naming a DIFFERENT run is a no-op — the owner's key survives.
        release_in(root, None, None, key, stale).unwrap();
        assert_eq!(
            lookup_in(root, None, None, key).unwrap().map(|r| r.run_id),
            Some(owner.to_string())
        );
        // A concurrent reserve still loses (key intact).
        assert!(matches!(
            reserve_in(root, None, None, key, &record(stale, 2)).unwrap(),
            Reservation::AlreadyReserved(_)
        ));
    }
}
