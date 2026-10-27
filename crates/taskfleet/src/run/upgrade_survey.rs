//! State survey and operator overrides behind `run upgrade-lock`.
//!
//! Runs only while the exclusive admission gate is held. Every participating
//! `run create` and `run reattach` holds admission SH from before staging until
//! publication, so holding EX already proves that no participating creator
//! process is in flight. The survey classifies what it finds and never
//! mutates; [`super::admission::upgrade`] decides what an explicit override
//! accepts, and moves stale creations aside only after every refusal has been
//! ruled out.
//!
//! **Live runs (`--allow-live-runs`).** A running supervisor or worker keeps
//! its old executable image when the installation is replaced (Linux and
//! macOS keep a mapped image alive after unlink or rename). What crosses the
//! version boundary is the on-disk run state: the new CLI will read and
//! append to runs the old processes are still reducing, and the old processes
//! will read what the new CLI writes. That format is versioned by
//! [`taskfleet_core::STATE_SCHEMA_VERSION`], so the rule is: live runs are
//! accepted only when the operator declares the target release's state
//! schema, it equals this binary's, and every manifest and node of every
//! accepted run is written at that schema. Anything else refuses with
//! `upgrade_state_schema_incompatible`, flag or not. A run whose supervisor
//! pid record is unreadable cannot be classified and still refuses.
//!
//! **Stale creations (`--abandon-stale-creations`).** An unpublished
//! `.creating/runs/<id>` counts as abandoned only when nothing that could still
//! write it is alive: admission EX (no creator), its staging `.lock` free, its
//! keyed materializer lease (if any) released, no live recorded supervisor,
//! agent or handshake pid, and no Pi message socket that still accepts
//! connections. Any probe that cannot decide refuses. An abandoned creation is
//! renamed, not deleted, to `<root>/abandoned-creations/<id>/` together with
//! its `.creating/message-<id>*` control files and an `abandoned.json` record.
//! A launcher that `workmux add` started outlives a killed creator and holds no
//! lease; its handshake fails closed once the staging path is gone, and
//! [`move_aside`] re-probes the moved tree so a handshake that landed between
//! the probes and the rename is caught and the move undone.
//!
//! Schema equality is a release discipline, not a field-level proof: typed
//! projections are rewritten whole, so an older same-schema writer drops
//! optional fields a newer release added (0.17.3 added PID start identities
//! within schema 1). `docs/UPGRADE-ADMISSION.md` states the consequence.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fs4::{FileExt, TryLockError};
use serde::Serialize;
use taskfleet_core::{read_manifest_opt, read_node_opt, NodeId, RunId, RunLock, RunPaths, Shared};

use crate::error::CliError;
use crate::run::from_core;

/// Directory under the state root that receives abandoned creations.
pub(super) const ABANDONED_DIR: &str = "abandoned-creations";

/// Directory under the state root that receives override receipts.
pub(super) const RECEIPT_DIR: &str = "upgrade-receipts";

/// What the exclusive survey found.
pub(super) struct Survey {
    pub checked_runs: usize,
    /// Published runs that a live upgrade would run alongside.
    pub live: Vec<LiveRun>,
    /// Published runs whose liveness cannot be classified at all.
    pub unverifiable_runs: Vec<String>,
    /// Unpublished creations keyed by run id.
    pub creations: BTreeMap<String, Creation>,
}

#[derive(Clone, Serialize)]
pub(super) struct LiveRun {
    pub run_id: String,
    /// Why the run is not quiescent: `not_terminal`, `agent_live`,
    /// `merge_pending`, `supervisor_live`.
    pub reasons: Vec<&'static str>,
    /// Every `schema_version` read from the run's manifest and nodes.
    pub state_schemas: BTreeSet<u32>,
    /// Taskfleet executables the run's generated launchers call (worker
    /// handshake, `run-worker`, Claude hooks): the old release's path, which
    /// must survive the activation while the run lives.
    pub executables: BTreeSet<String>,
}

#[derive(Default)]
pub(super) struct Creation {
    pub staging_dir: Option<PathBuf>,
    pub control_files: Vec<PathBuf>,
}

/// One creation an operator override moved aside.
#[derive(Clone, Serialize)]
pub(super) struct Abandoned {
    pub creation_id: String,
    pub moved_to: String,
    pub moved: Vec<String>,
    pub evidence: Vec<String>,
}

fn unverifiable(message: String) -> CliError {
    CliError::system("upgrade_state_unverifiable", message)
}

fn io_error(path: &Path, e: io::Error) -> CliError {
    CliError::system("admission_lock_io", format!("{}: {e}", path.display()))
}

/// `message-<id>.ts` or `message-<id>-<attempt>.sock`: Pi adapter files a
/// launcher writes outside the staging tree so they survive publication.
fn control_file_run_id(name: &str) -> Option<String> {
    let rest = name.strip_prefix("message-")?;
    let id = if let Some(id) = rest.strip_suffix(".ts") {
        id
    } else {
        let stem = rest.strip_suffix(".sock")?;
        let (id, attempt) = stem.rsplit_once('-')?;
        if attempt.is_empty() || !attempt.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        id
    };
    RunId::parse_str(id).ok().map(|_| id.to_string())
}

pub(super) fn survey(root: &Path, wait_secs: u64) -> Result<Survey, CliError> {
    let deadline = Instant::now() + Duration::from_secs(wait_secs);
    let (checked_runs, live, unverifiable_runs, published) = survey_runs(root, deadline)?;
    let creations = survey_staging(root, &published)?;
    Ok(Survey {
        checked_runs,
        live,
        unverifiable_runs,
        creations,
    })
}

type RunSurvey = (usize, Vec<LiveRun>, Vec<String>, BTreeSet<String>);

fn survey_runs(root: &Path, deadline: Instant) -> Result<RunSurvey, CliError> {
    let runs = root.join("runs");
    let entries = match std::fs::read_dir(&runs) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok((0, Vec::new(), Vec::new(), BTreeSet::new()))
        }
        Err(e) => return Err(io_error(&runs, e)),
    };
    let mut checked = 0;
    let mut live = Vec::new();
    let mut unverifiable_runs = Vec::new();
    let mut published = BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(|e| io_error(&runs, e))?;
        if !entry
            .file_type()
            .map_err(|e| io_error(&entry.path(), e))?
            .is_dir()
        {
            return Err(unverifiable(format!(
                "unexpected entry in runs: {}",
                entry.path().display()
            )));
        }
        let id = entry.file_name().to_string_lossy().to_string();
        let paths = RunPaths::new(entry.path(), id.clone())
            .map_err(|e| unverifiable(format!("invalid run entry {id}: {e}")))?;
        published.insert(id.clone());
        // Never block on a per-run writer while holding EX, but a live
        // supervisor holds its run lock briefly and often: retry until the
        // caller's admission wait budget runs out.
        let lock = loop {
            if let Some(lock) =
                RunLock::<Shared>::try_acquire_shared_existing(&paths.lock()).map_err(from_core)?
            {
                break lock;
            }
            if Instant::now() >= deadline {
                return Err(unverifiable(format!("busy run {id}; retry")));
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        let manifest = read_manifest_opt(&paths)
            .map_err(from_core)?
            .ok_or_else(|| unverifiable(format!("missing manifest for run {id}")))?;
        checked += 1;
        let mut state_schemas = BTreeSet::from([manifest.schema_version]);
        // Terminal status alone is insufficient: merge recovery, live worker
        // or supervisor teardown may still depend on this binary. Conversely,
        // settled failed/cancelled work is preserved on disk across upgrades.
        let mut agent_live = false;
        let mut merge_pending = false;
        for index in 1..=manifest.node_count {
            let node_id = NodeId::parse_str(&format!("n-{index:04}"))
                .map_err(|e| unverifiable(format!("run {id} node index {index}: {e}")))?;
            let node = read_node_opt(&paths, &node_id)
                .map_err(from_core)?
                .ok_or_else(|| unverifiable(format!("missing {node_id} in run {id}")))?;
            state_schemas.insert(node.schema_version);
            agent_live |= node
                .agent_pid
                .and_then(|pid| u32::try_from(pid).ok())
                .is_some_and(|pid| {
                    crate::supervise::pid_file::pid_live_with_identity(
                        pid,
                        &crate::supervise::watchdog::RecordedStart::of_agent(&node),
                    )
                });
            merge_pending |= node.pending_merge.is_some();
        }
        let pid_path = paths.supervisor_pid();
        let record = crate::supervise::pid_file::read_pid_record(&pid_path);
        let supervisor_live = record.as_ref().is_some_and(|(pid, start)| {
            crate::supervise::pid_file::pid_live_with_identity(*pid, start)
        });
        drop(lock);
        if pid_path.exists() && record.is_none() {
            unverifiable_runs.push(id);
            continue;
        }
        let reasons: Vec<&'static str> = [
            (!manifest.status.is_terminal(), "not_terminal"),
            (agent_live, "agent_live"),
            (merge_pending, "merge_pending"),
            (supervisor_live, "supervisor_live"),
        ]
        .into_iter()
        .filter_map(|(hit, reason)| hit.then_some(reason))
        .collect();
        if !reasons.is_empty() {
            let executables = launcher_executables(&entry.path())?;
            live.push(LiveRun {
                run_id: id,
                reasons,
                state_schemas,
                executables,
            });
        }
    }
    Ok((checked, live, unverifiable_runs, published))
}

/// The Taskfleet executable each generated `candidate-launch-*.sh` invokes
/// for its worker handshake. The launcher writes that line with the creating
/// process's exact `current_exe()` as a single-quoted shell word.
fn launcher_executables(run_dir: &Path) -> Result<BTreeSet<String>, CliError> {
    let mut found = BTreeSet::new();
    for entry in std::fs::read_dir(run_dir).map_err(|e| io_error(run_dir, e))? {
        let entry = entry.map_err(|e| io_error(run_dir, e))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !(name.starts_with("candidate-launch-")
            && Path::new(&name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("sh")))
        {
            continue;
        }
        let path = entry.path();
        let body = std::fs::read_to_string(&path).map_err(|e| io_error(&path, e))?;
        let exe = body.lines().find_map(handshake_executable).ok_or_else(|| {
            unverifiable(format!(
                "launcher {} names no worker-handshake executable",
                path.display()
            ))
        })?;
        found.insert(exe);
    }
    Ok(found)
}

/// Parse `'<exe>' 'worker-handshake' ...`, undoing the `'\''` quote escape.
fn handshake_executable(line: &str) -> Option<String> {
    let mut rest = line.strip_prefix('\'')?;
    let mut word = String::new();
    loop {
        let end = rest.find('\'')?;
        word.push_str(&rest[..end]);
        rest = &rest[end + 1..];
        match rest.strip_prefix("\\''") {
            Some(next) => {
                word.push('\'');
                rest = next;
            }
            None => break,
        }
    }
    rest.starts_with(" 'worker-handshake' ").then_some(word)
}

fn survey_staging(
    root: &Path,
    published: &BTreeSet<String>,
) -> Result<BTreeMap<String, Creation>, CliError> {
    let staging = root.join(".creating");
    let entries = match std::fs::read_dir(&staging) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(io_error(&staging, e)),
    };
    let mut creations: BTreeMap<String, Creation> = BTreeMap::new();
    let mut control = Vec::new();
    let unpublished = |path: &Path| {
        unverifiable(format!(
            "unpublished creation state at {}; inspect/recover before activation",
            path.display()
        ))
    };
    for entry in entries {
        let entry = entry.map_err(|e| io_error(&staging, e))?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| io_error(&path, e))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            return Err(unpublished(&path));
        };
        match name.as_str() {
            // The private Pi session store outlives publication by design.
            "pi-sessions" => {}
            // ensure_root(.creating) makes an empty logs/ directory.
            "logs"
                if file_type.is_dir()
                    && std::fs::read_dir(&path).is_ok_and(|mut d| d.next().is_none()) => {}
            "runs" if file_type.is_dir() => {
                for run in std::fs::read_dir(&path).map_err(|e| io_error(&path, e))? {
                    let run = run.map_err(|e| io_error(&path, e))?;
                    let run_path = run.path();
                    let id = run.file_name().to_str().map(str::to_owned);
                    let is_dir = run.file_type().is_ok_and(|t| t.is_dir());
                    match id.filter(|id| is_dir && RunId::parse_str(id).is_ok()) {
                        Some(id) => creations.entry(id).or_default().staging_dir = Some(run_path),
                        None => return Err(unpublished(&run_path)),
                    }
                }
            }
            _ => match control_file_run_id(&name) {
                Some(id) if !file_type.is_dir() => control.push((id, path)),
                _ => return Err(unpublished(&path)),
            },
        }
    }
    // A published run's adapter files belong to that run; its liveness is
    // judged by the run survey and its supervisor removes them at teardown.
    for (id, path) in control {
        if !published.contains(&id) {
            creations.entry(id).or_default().control_files.push(path);
        }
    }
    Ok(creations)
}

fn creation_live(id: &str, why: String) -> CliError {
    CliError::system(
        "upgrade_creation_live",
        format!("creation {id} is not provably abandoned: {why}; admission was not changed"),
    )
    .with_details(serde_json::json!({ "creation_ids": [id] }))
}

/// Probe everything recorded inside one staging tree that could still write
/// it: its run lock, supervisor pid, recorded agents, and handshaked workers.
fn probe_staging(dir: &Path, id: &str, evidence: &mut Vec<String>) -> Result<(), CliError> {
    let meta = dir.symlink_metadata().map_err(|e| io_error(dir, e))?;
    if !meta.is_dir() {
        return Err(unverifiable(format!(
            "staging entry {} is not a directory",
            dir.display()
        )));
    }
    let lock = dir.join(".lock");
    if lock.symlink_metadata().is_ok() {
        let mut opts = OpenOptions::new();
        opts.read(true).write(true);
        taskfleet_core::nofollow(&mut opts);
        let file: File = opts.open(&lock).map_err(|e| io_error(&lock, e))?;
        match FileExt::try_lock(&file) {
            Ok(()) => {
                let _ = FileExt::unlock(&file);
                evidence.push("staging_lock_free".into());
            }
            Err(TryLockError::WouldBlock) => {
                return Err(creation_live(id, "its staging lock is held".into()))
            }
            Err(TryLockError::Error(e)) => return Err(io_error(&lock, e)),
        }
    }
    let paths = RunPaths::new(dir, id.to_string())
        .map_err(|e| unverifiable(format!("invalid staging run {id}: {e}")))?;
    let pid_path = paths.supervisor_pid();
    if pid_path.symlink_metadata().is_ok() {
        match crate::supervise::pid_file::read_pid_record(&pid_path) {
            Some((pid, start))
                if crate::supervise::pid_file::pid_live_with_identity(pid, &start) =>
            {
                return Err(creation_live(id, format!("supervisor pid {pid} is live")))
            }
            Some(_) => evidence.push("supervisor_dead".into()),
            None => {
                return Err(unverifiable(format!(
                    "unreadable supervisor pid record {}",
                    pid_path.display()
                )))
            }
        }
    }
    let nodes = paths.nodes_dir();
    if nodes.symlink_metadata().is_ok() {
        for entry in std::fs::read_dir(&nodes).map_err(|e| io_error(&nodes, e))? {
            let entry = entry.map_err(|e| io_error(&nodes, e))?;
            let name = entry.file_name().to_string_lossy().to_string();
            let node_id = name
                .strip_suffix(".json")
                .and_then(|stem| NodeId::parse_str(stem).ok())
                .ok_or_else(|| {
                    unverifiable(format!("unexpected node file {}", entry.path().display()))
                })?;
            let node = read_node_opt(&paths, &node_id)
                .map_err(from_core)?
                .ok_or_else(|| unverifiable(format!("vanished node {node_id} in {id}")))?;
            let live = node
                .agent_pid
                .and_then(|pid| u32::try_from(pid).ok())
                .is_some_and(|pid| {
                    crate::supervise::pid_file::pid_live_with_identity(
                        pid,
                        &crate::supervise::watchdog::RecordedStart::of_agent(&node),
                    )
                });
            if live {
                return Err(creation_live(id, format!("agent of {node_id} is live")));
            }
        }
        evidence.push("recorded_agents_dead".into());
    }
    for entry in std::fs::read_dir(dir).map_err(|e| io_error(dir, e))? {
        let entry = entry.map_err(|e| io_error(dir, e))?;
        let name = entry.file_name().to_string_lossy().to_string();
        let is_json = Path::new(&name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
        if !(name.starts_with("worker-handshake-") && is_json) {
            continue;
        }
        let path = entry.path();
        let handshake: crate::worker_handshake::WorkerHandshake = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or_else(|| {
                unverifiable(format!("unreadable worker handshake {}", path.display()))
            })?;
        let start = crate::supervise::watchdog::RecordedStart::new(
            Some(handshake.start_time),
            Some(&handshake.start_identity),
        );
        if crate::supervise::pid_file::pid_live_with_identity(handshake.pid, &start) {
            return Err(creation_live(
                id,
                format!("handshaked worker pid {} is live", handshake.pid),
            ));
        }
        evidence.push(format!("handshake_worker_dead:{name}"));
    }
    Ok(())
}

/// Whether a Unix socket has a listener. Nonblocking, so a listener with a
/// full accept queue cannot wedge the gate while it holds admission EX; such a
/// listener counts as live.
fn socket_has_listener(path: &Path) -> io::Result<bool> {
    use std::os::fd::{FromRawFd as _, OwnedFd};
    use std::os::unix::ffi::OsStrExt as _;
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: sockaddr_un is plain old data; all-zero is a valid value.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.len() >= addr.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path too long",
        ));
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (dst, src) in addr.sun_path.iter_mut().zip(bytes) {
        *dst = *src as libc::c_char;
    }
    // SAFETY: plain socket(2); the result is checked before use.
    let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` is a freshly created descriptor owned by nobody else.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let raw = std::os::fd::AsRawFd::as_raw_fd(&fd);
    // SAFETY: F_GETFL/F_SETFL on the descriptor owned above.
    let set = unsafe {
        let flags = libc::fcntl(raw, libc::F_GETFL);
        flags >= 0 && libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) >= 0
    };
    if !set {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `addr` is an initialized sockaddr_un and the length matches it.
    let rc = unsafe {
        libc::connect(
            raw,
            (&raw const addr).cast::<libc::sockaddr>(),
            libc::socklen_t::try_from(std::mem::size_of::<libc::sockaddr_un>())
                .expect("sockaddr_un fits socklen_t"),
        )
    };
    if rc == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ECONNREFUSED) => Ok(false),
        Some(code) if code == libc::EAGAIN || code == libc::EINPROGRESS => Ok(true),
        _ => Err(error),
    }
}

/// Collect positive evidence that nothing can still write creation `id`.
/// Called only under admission EX (which already excludes the creator).
pub(super) fn prove_abandoned(
    root: &Path,
    id: &str,
    creation: &Creation,
) -> Result<Vec<String>, CliError> {
    let mut evidence = vec!["admission_exclusive_held".to_string()];
    if let Some(dir) = &creation.staging_dir {
        probe_staging(dir, id, &mut evidence)?;
    }
    let lease = root
        .join("idempotency")
        .join("materializers")
        .join(format!("{id}.lease"));
    if lease.symlink_metadata().is_ok() {
        match crate::idempotency::materializer_liveness(&lease.to_string_lossy()) {
            crate::idempotency::LeaseLiveness::Dead => {
                evidence.push("materializer_lease_released".into());
            }
            crate::idempotency::LeaseLiveness::Live => {
                return Err(creation_live(id, "its materializer lease is held".into()))
            }
            crate::idempotency::LeaseLiveness::Unverifiable => {
                return Err(unverifiable(format!(
                    "materializer lease {} cannot be probed",
                    lease.display()
                )))
            }
        }
    }
    for file in &creation.control_files {
        if !file.to_string_lossy().ends_with(".sock") {
            continue;
        }
        match socket_has_listener(file) {
            Ok(true) => {
                return Err(creation_live(
                    id,
                    format!("Pi message socket {} accepts connections", file.display()),
                ))
            }
            Ok(false) => {
                evidence.push(format!(
                    "message_socket_refused:{}",
                    file.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
            Err(e) => {
                return Err(unverifiable(format!(
                    "probe Pi message socket {}: {e}",
                    file.display()
                )))
            }
        }
    }
    Ok(evidence)
}

/// Destination for an abandoned creation. Refuses to reuse one, so an earlier
/// abandonment is never overwritten.
pub(super) fn abandon_destination(root: &Path, id: &str) -> Result<PathBuf, CliError> {
    let dest = root.join(ABANDONED_DIR).join(id);
    if dest.symlink_metadata().is_ok() {
        return Err(unverifiable(format!(
            "{} already exists; inspect the earlier abandonment of {id}",
            dest.display()
        )));
    }
    Ok(dest)
}

pub(super) fn private_dir(path: &Path) -> Result<(), CliError> {
    match path.symlink_metadata() {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(unverifiable(format!(
            "{} is not a real directory",
            path.display()
        ))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt as _;
                builder.mode(0o700);
            }
            builder.create(path).map_err(|e| io_error(path, e))?;
            sync_dir(path.parent().unwrap_or(path))
        }
        Err(e) => Err(io_error(path, e)),
    }
}

pub(super) fn sync_dir(dir: &Path) -> Result<(), CliError> {
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| io_error(dir, e))
}

/// Rename a proven-abandoned creation aside, recording why first.
pub(super) fn move_aside(
    root: &Path,
    id: &str,
    creation: &Creation,
    evidence: Vec<String>,
    receipt_id: &str,
) -> Result<Abandoned, CliError> {
    private_dir(&root.join(ABANDONED_DIR))?;
    let dest = abandon_destination(root, id)?;
    private_dir(&dest)?;
    let mut renames = Vec::new();
    if let Some(dir) = &creation.staging_dir {
        renames.push((dir.clone(), dest.join("run")));
    }
    for file in &creation.control_files {
        renames.push((
            file.clone(),
            dest.join(file.file_name().unwrap_or_default()),
        ));
    }
    let record = serde_json::json!({
        "creation_id": id,
        "abandoned_at": chrono::Utc::now(),
        "receipt_id": receipt_id,
        "taskfleet_version": env!("CARGO_PKG_VERSION"),
        "evidence": evidence,
        "moves": renames.iter().map(|(from, to)| serde_json::json!({
            "from": from.display().to_string(),
            "to": to.display().to_string(),
        })).collect::<Vec<_>>(),
        "restore": "rename each `to` back to its `from` while no taskfleet process uses this state root",
    });
    taskfleet_core::atomic::write_json_atomic(&dest.join("abandoned.json"), &record)
        .map_err(from_core)?;
    let mut moved = Vec::new();
    for (index, (from, to)) in renames.iter().enumerate() {
        std::fs::rename(from, to).map_err(|e| io_error(from, e))?;
        if index == 0 && creation.staging_dir.is_some() {
            // A surviving launcher's handshake that landed between the probes
            // and this rename is now inside the moved tree; any later one
            // fails closed because its bound staging path no longer exists.
            // Re-probe so neither window hides a live worker.
            if let Err(error) = probe_staging(to, id, &mut Vec::new()) {
                let restored = std::fs::rename(to, from).is_ok();
                return Err(error.with_details(serde_json::json!({
                    "creation_ids": [id],
                    "restored": restored,
                    "moved_to": to.display().to_string(),
                })));
            }
        }
        moved.push(from.display().to_string());
        if let Some(parent) = from.parent() {
            sync_dir(parent)?;
        }
    }
    sync_dir(&dest)?;
    Ok(Abandoned {
        creation_id: id.to_string(),
        moved_to: dest.display().to_string(),
        moved,
        evidence,
    })
}

#[cfg(test)]
mod tests {
    use super::control_file_run_id;

    #[test]
    fn handshake_line_names_the_launcher_executable() {
        use super::handshake_executable;
        assert_eq!(
            handshake_executable("'/opt/v0.17.1/taskfleet' 'worker-handshake' '--path' 'x'")
                .as_deref(),
            Some("/opt/v0.17.1/taskfleet")
        );
        assert_eq!(
            handshake_executable("'/o'\\''d/taskfleet' 'worker-handshake' '--pid' \"$$\"")
                .as_deref(),
            Some("/o'd/taskfleet")
        );
        assert_eq!(handshake_executable("'/bin/pi' '--print'"), None);
        assert_eq!(handshake_executable("exec '/x' run-worker"), None);
    }

    #[test]
    fn control_files_name_their_run() {
        let id = "01m5sx7xmqpmmp5dv2dm4hes7t";
        assert_eq!(
            control_file_run_id(&format!("message-{id}.ts")).as_deref(),
            Some(id)
        );
        assert_eq!(
            control_file_run_id(&format!("message-{id}-12.sock")).as_deref(),
            Some(id)
        );
        assert_eq!(control_file_run_id(&format!("message-{id}-.sock")), None);
        assert_eq!(control_file_run_id(&format!("message-{id}.tsx")), None);
        assert_eq!(control_file_run_id("message-not-a-run.ts"), None);
        assert_eq!(control_file_run_id("orphan"), None);
    }
}
