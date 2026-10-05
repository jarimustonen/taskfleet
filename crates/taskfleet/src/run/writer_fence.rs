//! Durable lock identity for caller-owned runs. This is NOT launch admission:
//! the host must hold the gate through Pi Start and pass the shared lease FD.
//! Intent recording and held exclusive leases guard merge, salvage and cancellation.
use std::ffi::CString;
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use serde::{Deserialize, Serialize};
use taskfleet_core::RunPaths;

use crate::error::CliError;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Identity {
    pub dev: u64,
    pub ino: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Fence {
    pub writer: Identity,
    pub launch_gate: Identity,
}
fn unavailable(why: impl std::fmt::Display) -> CliError {
    CliError::user(
        "writer_fence_unavailable",
        format!("caller writer fence unavailable: {why}; preserve the run and checkout"),
    )
}
fn open_at(dir: &File, name: &str, flags: i32, mode: u32) -> Result<File, CliError> {
    let name = CString::new(name).expect("constant lock filename");
    // SAFETY: the directory fd and NUL terminated name are valid for openat.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            mode,
        )
    };
    if fd < 0 {
        return Err(unavailable(std::io::Error::last_os_error()));
    }
    // SAFETY: successful openat returns an owned fd.
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn directory(path: &Path) -> Result<File, CliError> {
    let mut current = File::open("/").map_err(unavailable)?;
    if !path.is_absolute() {
        return Err(unavailable("state root is not absolute"));
    }
    let parts: Vec<_> = path.components().collect();
    for (index, part) in parts.iter().enumerate() {
        if let std::path::Component::Normal(name) = part {
            let c = CString::new(name.as_bytes()).map_err(unavailable)?;
            // SAFETY: held parent directory fd and NUL terminated component.
            let fd = unsafe {
                libc::openat(
                    current.as_raw_fd(),
                    c.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(unavailable(std::io::Error::last_os_error()));
            }
            // SAFETY: successful openat returns an owned fd.
            current = unsafe { File::from_raw_fd(fd) };
            // The state root, publication/staging parent, and run itself must
            // all be private. Earlier ancestors may be root-owned (/var/tmp).
            if index + 3 >= parts.len() {
                let m = current.metadata().map_err(unavailable)?;
                if !m.is_dir()
                    || m.uid() != unsafe { libc::geteuid() }
                    || m.permissions().mode() & 0o7777 != 0o700
                {
                    return Err(unavailable(
                        "state root, run parent and run must be same-user 0700",
                    ));
                }
            }
        } else if !matches!(part, std::path::Component::RootDir) {
            return Err(unavailable("state path contains traversal"));
        }
    }
    Ok(current)
}
fn checked(dir: &File, name: &str) -> Result<Identity, CliError> {
    let f = open_at(dir, name, libc::O_RDWR | libc::O_NONBLOCK, 0)?;
    let m = f.metadata().map_err(unavailable)?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o7777 != 0o600
        || m.nlink() != 1
        || m.len() != 0
    {
        return Err(unavailable(format!(
            "{name} is not an empty same-user regular 0600 single-link file"
        )));
    }
    Ok(Identity {
        dev: m.dev(),
        ino: m.ino(),
    })
}
fn inspect_dir(dir: &File, expected: &Fence) -> Result<(), CliError> {
    if checked(dir, "writer.lock")? != expected.writer
        || checked(dir, "launch-gate.lock")? != expected.launch_gate
    {
        return Err(unavailable("recorded lock inode changed"));
    }
    Ok(())
}
/// Creates only inside a fresh private staging directory, before run.created.
/// A pre-existing directory without a durable record is never enrolled.
pub(super) fn create(paths: &RunPaths) -> Result<Fence, CliError> {
    let dir = directory(&paths.root)?;
    for name in ["writer.lock", "launch-gate.lock"] {
        let file = open_at(
            &dir,
            name,
            libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
            0o600,
        )?;
        file.sync_all().map_err(unavailable)?;
    }
    dir.sync_all().map_err(unavailable)?;
    let fence = Fence {
        writer: checked(&dir, "writer.lock")?,
        launch_gate: checked(&dir, "launch-gate.lock")?,
    };
    let mut ledger = open_at(
        &dir,
        "writer-fence.json",
        libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY,
        0o600,
    )?;
    ledger
        .write_all(&serde_json::to_vec(&fence).map_err(unavailable)?)
        .map_err(unavailable)?;
    ledger.sync_all().map_err(unavailable)?;
    dir.sync_all().map_err(unavailable)?;
    Ok(fence)
}
/// Read the immutable identity through a verified run dir; never create missing state.
/// Caller must hold the run's shared or exclusive .lock while checking event identity.
pub(super) fn inspect(paths: &RunPaths) -> Result<Fence, CliError> {
    let dir = directory(&paths.root)?;
    let ledger = open_at(
        &dir,
        "writer-fence.json",
        libc::O_RDONLY | libc::O_NONBLOCK,
        0,
    )?;
    let m = ledger.metadata().map_err(unavailable)?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o7777 != 0o600
        || m.nlink() != 1
        || m.len() > 256
    {
        return Err(unavailable("invalid identity ledger"));
    }
    let fence: Fence = serde_json::from_reader(ledger).map_err(unavailable)?;
    inspect_dir(&dir, &fence)?;
    // Re-traverse the path and verify both parent and child inodes against
    // the held descriptors: rename/replacement during inspection fails closed.
    let fresh = directory(&paths.root)?;
    let a = dir.metadata().map_err(unavailable)?;
    let b = fresh.metadata().map_err(unavailable)?;
    if (a.dev(), a.ino()) != (b.dev(), b.ino()) {
        return Err(unavailable("run directory changed"));
    }
    inspect_dir(&fresh, &fence)?;
    Ok(fence)
}

/// Validate a daemon-supplied descriptor pair against the durable ledger and
/// observe conflicting locks through independent open-file descriptions. This
/// cannot attest which process acquired the locks: the host must retain both
/// descriptors and the gate through Start. No CLI reply is launch permission.
pub(super) fn check_launch_fds(
    paths: &RunPaths,
    gate_fd: i32,
    writer_fd: i32,
) -> Result<Fence, CliError> {
    use std::os::fd::BorrowedFd;
    if gate_fd < 3 || writer_fd < 3 || gate_fd == writer_fd {
        return Err(unavailable(
            "distinct inherited gate and writer FDs >= 3 required",
        ));
    }
    let fence = inspect(paths)?;
    let dir = directory(&paths.root)?;
    for (fd, name, expected) in [
        (gate_fd, "launch-gate.lock", &fence.launch_gate),
        (writer_fd, "writer.lock", &fence.writer),
    ] {
        // SAFETY: borrow only for this synchronous operation; fstat reports EBADF
        // for a closed caller-supplied descriptor without assuming ownership.
        let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
        let m = File::from(borrowed.try_clone_to_owned().map_err(unavailable)?)
            .metadata()
            .map_err(unavailable)?;
        if !m.is_file()
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o7777 != 0o600
            || m.nlink() != 1
            || m.len() != 0
            || (m.dev(), m.ino()) != (expected.dev, expected.ino)
        {
            return Err(unavailable(format!(
                "inherited {name} FD differs from ledger"
            )));
        }
        let probe = open_at(&dir, name, libc::O_RDWR | libc::O_NONBLOCK, 0)?;
        // A different open-file description must NOT be able to take EX.
        if unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Err(unavailable(format!(
                "{name} is not locked by a separate holder"
            )));
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(unavailable(format!("cannot probe {name} flock")));
        }
    }
    if inspect(paths)? != fence {
        return Err(unavailable("fence identity changed during FD check"));
    }
    Ok(fence)
}

/// Held exclusive writer lease. The FD remains locked for the whole guarded
/// action; dropping this guard releases it, never unlinks the inode.
pub(super) struct ExclusiveWriter(File);

impl ExclusiveWriter {
    /// Recheck the pathname and durable intent immediately before an irreversible
    /// action. This is not a Git identity/history check; callers must add those.
    pub(super) fn revalidate(
        &self,
        paths: &RunPaths,
        intent: &taskfleet_core::schema::CallerSettlementIntent,
    ) -> Result<(), CliError> {
        use taskfleet_core::RunLock;
        let _lock = RunLock::acquire_existing(&paths.lock()).map_err(super::from_core)?;
        self.revalidate_unlocked(paths, intent)
    }

    /// Caller holds the run lock; do not recursively acquire it.
    pub(super) fn revalidate_unlocked(
        &self,
        paths: &RunPaths,
        intent: &taskfleet_core::schema::CallerSettlementIntent,
    ) -> Result<(), CliError> {
        let fence = inspect(paths)?;
        super::session::check_creation_fence(paths, &fence)?;
        let meta = self.0.metadata().map_err(unavailable)?;
        if (meta.dev(), meta.ino()) != (fence.writer.dev, fence.writer.ino)
            || (intent.writer_dev, intent.writer_ino) != (fence.writer.dev, fence.writer.ino)
            || (intent.gate_dev, intent.gate_ino) != (fence.launch_gate.dev, fence.launch_gate.ino)
            || taskfleet_core::read_manifest_opt(paths)
                .map_err(super::from_core)?
                .and_then(|m| m.caller_settlement_intent)
                != Some(intent.clone())
        {
            return Err(unavailable("writer lease or persisted intent changed"));
        }
        Ok(())
    }
}

/// Nonblocking bounded acquisition, outside both gate and run locks. An EX
/// lease alone grants no authority without persisted intent and Git/history proof.
pub(super) fn acquire_exclusive(
    paths: &RunPaths,
    intent: &taskfleet_core::schema::CallerSettlementIntent,
) -> Result<ExclusiveWriter, CliError> {
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + Duration::from_millis(300);
    loop {
        let fence = inspect(paths)?;
        let dir = directory(&paths.root)?;
        let file = open_at(&dir, "writer.lock", libc::O_RDWR | libc::O_NONBLOCK, 0)?;
        match unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } {
            0 => {
                let held = ExclusiveWriter(file);
                held.revalidate(paths, intent)?;
                if inspect(paths)? != fence {
                    return Err(unavailable("writer pathname changed"));
                }
                return Ok(held);
            }
            _ if std::io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK) => {
                if Instant::now() >= deadline {
                    return Err(CliError::user("writer_active", format!(
                        "intent recorded for {}; writer lease busy (phase intent-recorded); retry the same key after writer exits; no Git action taken",
                        intent.run_id
                    )));
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            _ => return Err(unavailable(std::io::Error::last_os_error())),
        }
    }
}

/// Persist sticky settlement intent under launch gate and run lock.
pub(super) fn record_intent(
    paths: &RunPaths,
    node_id: &taskfleet_core::NodeId,
    key: &str,
    operation: taskfleet_core::schema::SettlementOperation,
    actor: &str,
    reason: Option<&str>,
    allow_failed: bool,
) -> Result<taskfleet_core::schema::CallerSettlementIntent, CliError> {
    use taskfleet_core::schema::{AgentOwner, CallerSettlementIntent};
    use taskfleet_core::{append_and_apply_unlocked, read_manifest_opt, read_node_opt, RunLock};
    if key.trim().is_empty()
        || key.len() > 256
        || actor.trim().is_empty()
        || actor.len() > 256
        || reason.is_some_and(|r| r.trim().is_empty() || r.len() > 1024)
        || (matches!(
            operation,
            taskfleet_core::schema::SettlementOperation::Discard
                | taskfleet_core::schema::SettlementOperation::Cancel
        ) && reason.is_none())
    {
        return Err(CliError::user(
            "invalid_settlement_intent",
            "nonblank bounded key/actor and reason for cancel/discard required",
        ));
    }
    // No run lock is held while waiting for gate. Never create a missing gate.
    let fence = inspect(paths)?;
    let dir = directory(&paths.root)?;
    let gate = open_at(&dir, "launch-gate.lock", libc::O_RDWR | libc::O_NONBLOCK, 0)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
    loop {
        if unsafe { libc::flock(gate.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            break;
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(unavailable(err));
        }
        if std::time::Instant::now() >= deadline {
            return Err(CliError::user(
                "gate_busy",
                "caller launch gate busy; retry settlement with the same key; no intent recorded",
            )
            .with_details(serde_json::json!({"retryable":true,"run_id":paths.run_id})));
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    if inspect(paths)? != fence {
        return Err(unavailable("gate identity changed"));
    }
    let guard = RunLock::acquire_existing(&paths.lock()).map_err(super::from_core)?;
    // A reply can be lost after the event sync but before any projection. Fold
    // that tail first, before checking for an existing sticky intent.
    taskfleet_core::replay_unapplied_unlocked(&guard.witness(), paths).map_err(super::from_core)?;
    let fence = inspect(paths)?;
    let held_gate = gate.metadata().map_err(unavailable)?;
    if (held_gate.dev(), held_gate.ino()) != (fence.launch_gate.dev, fence.launch_gate.ino) {
        return Err(unavailable(
            "held gate inode differs from recorded pathname",
        ));
    }
    super::session::check_creation_fence(paths, &fence)?;
    let manifest = read_manifest_opt(paths)
        .map_err(super::from_core)?
        .ok_or_else(|| unavailable("missing run"))?;
    let node = read_node_opt(paths, node_id)
        .map_err(super::from_core)?
        .ok_or_else(|| unavailable("missing node"))?;
    if manifest.agent_owner != AgentOwner::Caller
        || node.run_id != paths.run_id
        || node.worktree_path.is_none()
        || node.branch.is_none()
        || (manifest.caller_settlement_intent.is_none()
            && node.worktree_path.as_ref().is_none_or(|checkout| {
                !Path::new(checkout)
                    .canonicalize()
                    .is_ok_and(|canonical| canonical == Path::new(checkout))
            }))
    {
        return Err(unavailable("not a verified caller run/node"));
    }
    let mut intent = CallerSettlementIntent {
        run_id: paths.run_id.clone(),
        node_id: node_id.clone(),
        key: key.into(),
        operation,
        actor: actor.into(),
        reason: reason.map(str::to_owned),
        writer_dev: fence.writer.dev,
        writer_ino: fence.writer.ino,
        gate_dev: fence.launch_gate.dev,
        gate_ino: fence.launch_gate.ino,
        generation: node
            .caller_pi_lifecycle
            .as_ref()
            .map_or(0, |v| v.generation),
        seq: 0,
    };
    if let Some(old) = manifest.caller_settlement_intent {
        // Cancellation is already a sticky admission fence. Discard is a
        // separate authorization, not a replacement intent; it must reuse
        // that exact fence and the caller's original settlement key.
        if operation == taskfleet_core::schema::SettlementOperation::Discard
            && old.operation == taskfleet_core::schema::SettlementOperation::Cancel
            && matches!(
                manifest.status,
                taskfleet_core::Status::Cancelled | taskfleet_core::Status::Failed
            )
            && node.status == manifest.status
            && old.key == key
            && old.run_id == paths.run_id
            && old.node_id == *node_id
            && old.writer_dev == intent.writer_dev
            && old.writer_ino == intent.writer_ino
            && old.gate_dev == intent.gate_dev
            && old.gate_ino == intent.gate_ino
            && old.generation == intent.generation
            && node.pending_merge.is_none()
            && manifest.node_count == 1
            && node_id.as_str() == "n-0001"
        {
            return Ok(old);
        }
        if old.key == key
            && old.operation == operation
            && old.actor == actor
            && old.reason.as_deref() == reason
            && old.writer_dev == intent.writer_dev
            && old.writer_ino == intent.writer_ino
            && old.gate_dev == intent.gate_dev
            && old.gate_ino == intent.gate_ino
            && old.node_id == *node_id
            && old.generation == intent.generation
        {
            return Ok(old);
        }
        return Err(CliError::user(
            "settlement_conflict",
            "different settlement intent already persisted; inspect the run",
        ));
    }
    if operation == taskfleet_core::schema::SettlementOperation::Discard
        && (node.pending_merge.is_some()
            || manifest.node_count != 1
            || node_id.as_str() != "n-0001"
            || !matches!(
                manifest.status,
                taskfleet_core::Status::Failed | taskfleet_core::Status::Cancelled
            )
            || node.status != manifest.status
            || !matches!(taskfleet_core::read_node_status_facts(paths, None)
                .map_err(super::from_core)?.as_slice(), [fact] if fact.node_id == *node_id && fact.status == manifest.status))
    {
        return Err(CliError::user(
            "settlement_conflict",
            "discard requires one matching terminal node and no pending merge transaction",
        ));
    }
    if operation == taskfleet_core::schema::SettlementOperation::Cancel
        && manifest.caller_settlement_intent.is_none()
        && (manifest.node_count != 1
            || node_id.as_str() != "n-0001"
            || !matches!(taskfleet_core::read_node_status_facts(paths, None)
                .map_err(super::from_core)?.as_slice(), [fact] if fact.node_id == *node_id && !fact.status.is_terminal()))
    {
        return Err(CliError::user(
            "settlement_conflict",
            "caller node already terminal or run is not single-node",
        ));
    }
    if manifest.status.is_terminal()
        && !(allow_failed
            && matches!(
                operation,
                taskfleet_core::schema::SettlementOperation::Merge
                    | taskfleet_core::schema::SettlementOperation::Discard
            )
            && manifest.status == taskfleet_core::Status::Failed
            && node.status == taskfleet_core::Status::Failed
            && manifest.node_count == 1
            && node_id.as_str() == "n-0001")
    {
        return Err(CliError::user(
            "settlement_conflict",
            "terminal caller run has no settlement intent",
        ));
    }
    if inspect(paths)? != fence {
        return Err(unavailable("fence identity changed before intent append"));
    }
    let seq = append_and_apply_unlocked(
        &guard.witness(),
        paths,
        "caller.settlement_intent",
        Some(node_id),
        Some(key),
        serde_json::to_value(&intent).map_err(unavailable)?,
    )
    .map_err(super::from_core)?;
    intent.seq = seq;
    if std::env::var_os("TASKFLEET_TEST_CALLER_INTENT_CRASH_AFTER_APPEND").is_some() {
        std::process::exit(72);
    }
    Ok(intent)
}

fn quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', "'\\''"))
}

fn recovery(intent: &taskfleet_core::schema::CallerSettlementIntent) -> String {
    use taskfleet_core::schema::SettlementOperation as Op;
    format!(
        "taskfleet run settlement-intent {} --node {} --operation {} --key {} --actor {}{}",
        intent.run_id,
        intent.node_id,
        match intent.operation {
            Op::Merge => "merge",
            Op::Cancel => "cancel",
            Op::Discard => "discard",
        },
        quote(&intent.key),
        quote(&intent.actor),
        intent
            .reason
            .as_ref()
            .map_or(String::new(), |r| format!(" --reason {}", quote(r)))
    )
}

#[allow(clippy::too_many_arguments)] // One CLI verb: audit inputs plus standard output contract.
pub(super) fn settlement_intent(
    run: &str,
    node: &str,
    operation: taskfleet_core::schema::SettlementOperation,
    key: &str,
    actor: &str,
    reason: Option<&str>,
    spec: &crate::output::OutputSpec,
    warnings: &[String],
) -> Result<(), CliError> {
    let run_id = super::parse_run_id(run)?;
    let node_id = super::parse_node_id(node)?;
    let paths = super::run_paths_exact(&crate::home::root_dir()?, &run_id)?;
    let intent = record_intent(&paths, &node_id, key, operation, actor, reason, false)?;
    let command = recovery(&intent);
    match acquire_exclusive(&paths, &intent) {
        Ok(lease) => {
            lease.revalidate(&paths, &intent)?;
            crate::output::emit_envelope(
                &serde_json::json!({
                    "run_id":run_id,"node_id":node_id,"intent":intent,
                    "phase":"quiesced","destructive_permission":false,
                    "recovery_command":command,
                }),
                spec,
                warnings,
            )
        }
        Err(e) if e.code == "writer_active" => Err(e.with_details(serde_json::json!({
            "run_id":run_id,"node_id":node_id,"intent":intent,
            "phase":"intent-recorded","retryable":true,
            "recovery_command":command,
        }))),
        Err(e) => Err(e.with_details(serde_json::json!({
            "run_id":run_id,"node_id":node_id,"intent":intent,
            "phase":"intent-recorded","recovery_command":command,
        }))),
    }
}
