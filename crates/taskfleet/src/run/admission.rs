//! Global worker-admission gate. Lock order: admission SH -> idempotency ->
//! per-run locks; upgrade EX -> per-run SH reads. Never acquire admission while
//! holding a per-run lock. The lock file is permanent (never unlink it), and
//! all versions participating in this protocol must use the same state root.
//! Before launching activation we persist an inhibit marker under EX. A dead
//! parent, timed-out child, or detached setsid descendant can never reopen
//! admissions by merely releasing the flock. Only a trusted synchronous
//! activation success, or explicit operator recovery, removes the marker.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use fs4::{FileExt, TryLockError};
use serde::Serialize;
use taskfleet_core::{read_manifest_opt, read_node_opt, NodeId, RunLock, RunPaths, Shared};

use crate::error::CliError;
use crate::output::{self, OutputFormat, OutputSpec};
use crate::run::from_core;

const FILE: &str = ".worker-admission.lock";
const DEFAULT_WAIT: u64 = 10;
const INHIBIT: &str = ".worker-admission-inhibited";

pub(super) struct Gate(File);

fn acquire(root: &Path, exclusive: bool, wait: u64) -> Result<Gate, CliError> {
    std::fs::create_dir_all(root).map_err(|e| io_error(root, e))?;
    let path = root.join(FILE);
    // Refuse a redirected lock path. O_NOFOLLOW backs up the lstat check.
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(CliError::system(
            "admission_lock_invalid",
            format!("symlink lock: {}", path.display()),
        ));
    }
    let mut opts = OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = opts.open(&path).map_err(|e| io_error(&path, e))?;
    let deadline = Instant::now() + Duration::from_secs(wait);
    loop {
        let result = if exclusive {
            FileExt::try_lock(&file)
        } else {
            FileExt::try_lock_shared(&file)
        };
        match result {
            Ok(()) => return Ok(Gate(file)),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(TryLockError::WouldBlock) => {
                return Err(CliError::system(
                    "admission_busy",
                    format!(
                        "worker admission lock {} busy after {wait}s; retry later",
                        path.display()
                    ),
                ))
            }
            Err(TryLockError::Error(e)) => return Err(io_error(&path, e)),
        }
    }
}

fn io_error(path: &Path, e: io::Error) -> CliError {
    CliError::system("admission_lock_io", format!("{}: {e}", path.display()))
}

/// Acquired before any create-side idempotency or per-run lock and held until
/// the create call returns, including child publication and replay repair.
pub(super) fn admit(root: &Path) -> Result<Gate, CliError> {
    let gate = acquire(root, false, DEFAULT_WAIT)?;
    ensure_not_inhibited(root)?;
    Ok(gate)
}

fn ensure_not_inhibited(root: &Path) -> Result<(), CliError> {
    let path = root.join(INHIBIT);
    match path.symlink_metadata() {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(CliError::system("upgrade_inhibited", format!(
            "worker admission inhibited at {}; inspect activation and descendants, then use run upgrade-lock --recover --confirm-quiescent with a trusted synchronous reconciliation command", path.display()
        ))),
        Err(e) => Err(io_error(&path, e)),
    }
}

fn sync_root(root: &Path) -> Result<(), CliError> {
    File::open(root)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| io_error(root, e))
}

fn inhibit(root: &Path) -> Result<(), CliError> {
    let path = root.join(INHIBIT);
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = opts.open(&path).map_err(|e| io_error(&path, e))?;
    file.sync_all().map_err(|e| io_error(&path, e))?;
    sync_root(root)
}

fn clear_inhibit(root: &Path) -> Result<(), CliError> {
    let path = root.join(INHIBIT);
    // Only remove our own regular file. A replaced marker is never authority
    // to admit workers, even after a successful activation.
    let meta = path.symlink_metadata().map_err(|e| io_error(&path, e))?;
    if !meta.file_type().is_file() || meta.len() != 0 {
        return Err(CliError::system(
            "upgrade_state_unverifiable",
            format!(
                "inhibit marker changed at {}; admission remains blocked",
                path.display()
            ),
        ));
    }
    std::fs::remove_file(&path).map_err(|e| io_error(&path, e))?;
    sync_root(root)
}

#[derive(Serialize)]
struct UpgradeResult {
    quiescent: bool,
    checked_runs: usize,
    command_exit: i32,
}

/// Hold an exclusive gate over a trusted fleet activation argv. No shell is
/// involved. The lock fd is inherited by the immediate child, so killing the
/// CLI parent cannot silently unlock while its activation process still runs.
pub(super) fn upgrade(
    wait_secs: u64,
    command_timeout_secs: u64,
    argv: Vec<String>,
    recover: bool,
    confirm_quiescent: bool,
    spec: &OutputSpec,
    warnings: &[String],
) -> Result<(), CliError> {
    let root = crate::home::root_dir()?;
    let gate = acquire(&root, true, wait_secs)?;
    if recover {
        if !confirm_quiescent {
            return Err(CliError::system("upgrade_recovery_unconfirmed", "recovery requires --confirm-quiescent after verifying all previous activation writers have stopped"));
        }
        if root
            .join(INHIBIT)
            .symlink_metadata()
            .is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
        {
            return Err(CliError::system(
                "upgrade_recovery_unneeded",
                "no inhibited upgrade to recover",
            ));
        }
        // A malformed or symlink marker still blocks admission; do not run
        // arbitrary code or clear it until its provenance is inspected.
        let marker = root.join(INHIBIT);
        let meta = marker
            .symlink_metadata()
            .map_err(|e| io_error(&marker, e))?;
        if !meta.file_type().is_file() || meta.len() != 0 {
            return Err(CliError::system(
                "upgrade_state_unverifiable",
                format!("invalid inhibit marker at {}", marker.display()),
            ));
        }
    } else {
        ensure_not_inhibited(&root)?;
    }
    let checked = quiescent(&root)?;
    if !recover {
        inhibit(&root)?;
    }
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    // Activation output belongs on stderr; stdout is reserved for the JSON
    // result. A trusted program may emit diagnostics without corrupting it.
    cmd.stdout(std::process::Stdio::from(std::io::stderr()));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let fd = gate.0.as_raw_fd();
        // SAFETY: only async-signal-safe fcntl in the forked child, before exec.
        unsafe {
            cmd.pre_exec(move || {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = cmd.spawn().map_err(|e| {
        CliError::system(
            "upgrade_command_failed",
            format!("start {:?}: {e}", argv[0]),
        )
    })?;
    let deadline = Instant::now() + Duration::from_secs(command_timeout_secs);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| {
            CliError::system(
                "upgrade_command_failed",
                format!("wait for activation: {e}"),
            )
        })? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CliError::system("upgrade_command_timeout", format!(
                "activation exceeded {command_timeout_secs}s; admission remains inhibited; inspect detached descendants and reconcile before recovery"
            )));
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    if !status.success() {
        return Err(CliError::system(
            "upgrade_command_failed",
            format!(
                "activation exited with {status}; inspect and retry or roll back under the gate"
            ),
        ));
    }
    clear_inhibit(&root)?;
    let result = UpgradeResult {
        quiescent: true,
        checked_runs: checked,
        command_exit: 0,
    };
    match spec.format {
        OutputFormat::Text => println!("upgrade completed; {checked} runs checked"),
        _ => output::emit_envelope(&result, spec, warnings)?,
    }
    drop(gate);
    Ok(())
}

fn quiescent(root: &Path) -> Result<usize, CliError> {
    // Unpublished native work may survive a hard creator kill. It is not in
    // runs/ and must not disappear from the upgrade decision. pi-sessions is
    // the private session store; all other entries need operator inspection.
    let staging = root.join(".creating");
    let staging_entries = match std::fs::read_dir(&staging) {
        Ok(entries) => Some(entries),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(io_error(&staging, e)),
    };
    if let Some(entries) = staging_entries {
        for entry in entries {
            let entry = entry.map_err(|e| io_error(&staging, e))?;
            if entry.file_name() == "pi-sessions" {
                continue;
            }
            // ensure_root(.creating) makes empty runs/ and logs/ directories.
            // An unrecognized or nonempty directory is a potential hard-killed
            // materialization, not evidence of quiescence.
            if (entry.file_name() == "runs" || entry.file_name() == "logs")
                && entry.file_type().is_ok_and(|t| t.is_dir())
                && std::fs::read_dir(entry.path()).is_ok_and(|mut d| d.next().is_none())
            {
                continue;
            }
            return Err(CliError::system(
                "upgrade_state_unverifiable",
                format!(
                    "unpublished creation state at {}; inspect/recover before activation",
                    entry.path().display()
                ),
            ));
        }
    }
    let runs = root.join("runs");
    let entries = match std::fs::read_dir(&runs) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(io_error(&runs, e)),
    };
    let mut checked = 0;
    let mut blockers = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| io_error(&runs, e))?;
        if !entry
            .file_type()
            .map_err(|e| io_error(&entry.path(), e))?
            .is_dir()
        {
            return Err(CliError::system(
                "upgrade_state_unverifiable",
                format!("unexpected entry in runs: {}", entry.path().display()),
            ));
        }
        let id = entry.file_name().to_string_lossy().to_string();
        let paths = RunPaths::new(entry.path(), id.clone()).map_err(|e| {
            CliError::system(
                "upgrade_state_unverifiable",
                format!("invalid run entry {id}: {e}"),
            )
        })?;
        // Do not wait indefinitely on a busy per-run writer while holding EX.
        let lock = RunLock::<Shared>::try_acquire_shared_existing(&paths.lock())
            .map_err(from_core)?
            .ok_or_else(|| {
                CliError::system(
                    "upgrade_state_unverifiable",
                    format!("busy run {id}; retry"),
                )
            })?;
        let manifest = read_manifest_opt(&paths)
            .map_err(from_core)?
            .ok_or_else(|| {
                CliError::system(
                    "upgrade_state_unverifiable",
                    format!("missing manifest for run {id}"),
                )
            })?;
        checked += 1;
        // Terminal status alone is insufficient: merge recovery, live worker
        // or supervisor teardown may still depend on this binary. Conversely,
        // settled failed/cancelled work is preserved on disk across upgrades.
        let mut node_busy = false;
        for index in 1..=manifest.node_count {
            let node_id = NodeId::parse_str(&format!("n-{index:04}")).map_err(|e| {
                CliError::system(
                    "upgrade_state_unverifiable",
                    format!("run {id} node index {index}: {e}"),
                )
            })?;
            let node = read_node_opt(&paths, &node_id)
                .map_err(from_core)?
                .ok_or_else(|| {
                    CliError::system(
                        "upgrade_state_unverifiable",
                        format!("missing {node_id} in run {id}"),
                    )
                })?;
            let agent_live = node
                .agent_pid
                .and_then(|pid| u32::try_from(pid).ok())
                .is_some_and(|pid| {
                    crate::supervise::pid_file::pid_live_with_identity(
                        pid,
                        node.agent_pid_start_time
                            .and_then(|at| u64::try_from(at.timestamp()).ok()),
                    )
                });
            node_busy |= agent_live || node.pending_merge.is_some();
        }
        let pid_path = paths.supervisor_pid();
        let supervisor_live =
            crate::supervise::pid_file::read_pid_record(&pid_path).is_some_and(|(pid, start)| {
                crate::supervise::pid_file::pid_live_with_identity(pid, start)
            });
        let pid_unverifiable =
            pid_path.exists() && crate::supervise::pid_file::read_pid_record(&pid_path).is_none();
        if !manifest.status.is_terminal() || node_busy || supervisor_live || pid_unverifiable {
            blockers.push(id);
        }
        drop(lock);
    }
    if !blockers.is_empty() {
        return Err(CliError::system(
            "upgrade_not_quiescent",
            format!(
                "{} live, awaiting-input, or recoverable runs block activation: {}",
                blockers.len(),
                blockers.join(", ")
            ),
        )
        .with_details(serde_json::json!({"run_ids": blockers, "checked_runs": checked})));
    }
    Ok(checked)
}
