//! Private worker-PID publication used by generated spawn launchers.
//!
//! The launcher invokes this hidden command in the process that is about to
//! `exec` the recorded candidate. The resulting PID therefore remains the
//! candidate PID across `exec`; no executable-name or process-tree inference is
//! involved.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use clap::Args as ClapArgs;
use serde::{Deserialize, Serialize};

use crate::error::CliError;

pub const TOKEN_ENV: &str = "TASKFLEET_INTERNAL_WORKER_HANDSHAKE_TOKEN";

#[derive(ClapArgs, Debug)]
pub struct WorkerHandshakeArgs {
    #[arg(long)]
    pub path: PathBuf,
    #[arg(long)]
    pub run_id: String,
    #[arg(long)]
    pub node_id: String,
    #[arg(long)]
    pub attempt: u32,
    #[arg(long)]
    pub pid: u32,
    #[arg(long)]
    pub state_root: PathBuf,
    #[arg(long)]
    pub pi_session_id: Option<String>,
    #[arg(long)]
    pub pi_session_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WorkerHandshake {
    pub schema_version: u32,
    pub run_id: String,
    pub node_id: String,
    pub attempt: u32,
    pub token: String,
    pub pid: u32,
    pub start_time: u64,
    /// Platform-native birth identity: microseconds on macOS and kernel start
    /// ticks on Linux. This closes same-second PID reuse in the create path.
    pub start_identity: String,
    pub tmux_pane_id: String,
    #[serde(default)]
    pub pi_session_id: Option<String>,
    #[serde(default)]
    pub pi_session_path: Option<String>,
    #[serde(default)]
    pub pi_session_cwd: Option<String>,
}

pub fn dispatch(args: WorkerHandshakeArgs) -> Result<(), CliError> {
    taskfleet_core::RunId::parse_str(&args.run_id).map_err(|e| {
        CliError::system("worker_handshake_invalid", format!("invalid run id: {e}"))
    })?;
    taskfleet_core::NodeId::parse_str(&args.node_id).map_err(|e| {
        CliError::system("worker_handshake_invalid", format!("invalid node id: {e}"))
    })?;
    let token = std::env::var(TOKEN_ENV).map_err(|_| {
        CliError::system(
            "worker_handshake_unauthorized",
            "private handshake token is absent",
        )
    })?;
    if token.len() < 20 || !token.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(CliError::system(
            "worker_handshake_unauthorized",
            "private handshake token is malformed",
        ));
    }
    validate_handshake_path(
        &args.path,
        &args.state_root,
        &args.run_id,
        &args.node_id,
        args.attempt,
    )?;
    let parent_pid = unsafe { libc::getppid() };
    if crate::supervise::pid_file::to_pid_t(args.pid) != Some(parent_pid) {
        return Err(CliError::system(
            "worker_handshake_pid_invalid",
            format!(
                "launcher supplied PID {} but helper parent PID is {parent_pid}",
                args.pid
            ),
        ));
    }
    let start_time = crate::supervise::watchdog::pid_start_time(args.pid).ok_or_else(|| {
        CliError::system(
            "worker_handshake_identity_unavailable",
            format!("cannot read start time for worker PID {}", args.pid),
        )
    })?;
    let start_identity = process_start_identity(args.pid).ok_or_else(|| {
        CliError::system(
            "worker_handshake_identity_unavailable",
            format!(
                "cannot read precise start identity for worker PID {}",
                args.pid
            ),
        )
    })?;
    let parent = args.path.parent().ok_or_else(|| {
        CliError::system(
            "worker_handshake_path_invalid",
            "handshake path has no parent",
        )
    })?;
    let parent_meta = std::fs::symlink_metadata(parent).map_err(|e| {
        CliError::system(
            "worker_handshake_io",
            format!("stat {}: {e}", parent.display()),
        )
    })?;
    if !parent_meta.is_dir() || parent_meta.file_type().is_symlink() {
        return Err(CliError::system(
            "worker_handshake_path_invalid",
            format!(
                "handshake parent {} is not a real directory",
                parent.display()
            ),
        ));
    }
    let tmux_pane_id = std::env::var("TMUX_PANE").map_err(|_| {
        CliError::system(
            "worker_handshake_pane_invalid",
            "TMUX_PANE is absent from the worker launcher",
        )
    })?;
    if !tmux_pane_id.starts_with('%')
        || tmux_pane_id.len() < 2
        || !tmux_pane_id[1..].bytes().all(|b| b.is_ascii_digit())
    {
        return Err(CliError::system(
            "worker_handshake_pane_invalid",
            format!("TMUX_PANE has invalid form: {tmux_pane_id:?}"),
        ));
    }
    let pi_session = prepare_pi_session(
        args.pi_session_id.as_deref(),
        args.pi_session_path.as_deref(),
        parent,
        &args.state_root,
        &args.run_id,
    )?;
    let record = WorkerHandshake {
        schema_version: 1,
        run_id: args.run_id,
        node_id: args.node_id,
        attempt: args.attempt,
        token,
        pid: args.pid,
        start_time,
        start_identity,
        tmux_pane_id,
        pi_session_id: pi_session.as_ref().map(|v| v.0.clone()),
        pi_session_path: pi_session.as_ref().map(|v| v.1.clone()),
        pi_session_cwd: pi_session.map(|v| v.2),
    };
    write_durable_json(&args.path, &record)
}

fn validate_handshake_path(
    path: &Path,
    root: &Path,
    run_id: &str,
    node_id: &str,
    attempt: u32,
) -> Result<(), CliError> {
    let canonical_root = root.canonicalize().map_err(|e| {
        CliError::system(
            "worker_handshake_path_invalid",
            format!("resolve state root {}: {e}", root.display()),
        )
    })?;
    let staging_parent = canonical_root.join(".creating").join("runs").join(run_id);
    let published_parent = canonical_root.join("runs").join(run_id);
    let parent = path.parent().ok_or_else(|| {
        CliError::system(
            "worker_handshake_path_invalid",
            "handshake path has no parent",
        )
    })?;
    let canonical_parent = parent.canonicalize().map_err(|e| {
        CliError::system(
            "worker_handshake_path_invalid",
            format!("resolve handshake parent {}: {e}", parent.display()),
        )
    })?;
    let expected_name = format!("worker-handshake-{node_id}-attempt-{attempt}.json");
    let expected_parent = if canonical_parent == staging_parent {
        staging_parent.clone()
    } else if canonical_parent == published_parent {
        published_parent
    } else {
        return Err(CliError::system(
            "worker_handshake_path_invalid",
            "handshake path is outside the bound staging or published run",
        ));
    };
    if path.file_name().and_then(|v| v.to_str()) != Some(&expected_name) {
        return Err(CliError::system(
            "worker_handshake_path_invalid",
            "handshake filename is not bound to this node attempt",
        ));
    }
    let mut components = vec![canonical_root.join("runs"), expected_parent.clone()];
    if expected_parent == staging_parent {
        components.insert(0, canonical_root.join(".creating"));
        components.insert(1, canonical_root.join(".creating/runs"));
    }
    for component in components {
        let meta = std::fs::symlink_metadata(&component).map_err(|e| {
            CliError::system(
                "worker_handshake_path_invalid",
                format!("stat {}: {e}", component.display()),
            )
        })?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(CliError::system(
                "worker_handshake_path_invalid",
                format!("{} is not a real directory", component.display()),
            ));
        }
    }
    Ok(())
}

fn prepare_pi_session(
    session_id: Option<&str>,
    session_path: Option<&Path>,
    _staging_run_dir: &Path,
    state_root: &Path,
    run_id: &str,
) -> Result<Option<(String, String, String)>, CliError> {
    let (Some(session_id), Some(session_path)) = (session_id, session_path) else {
        if session_id.is_some() || session_path.is_some() {
            return Err(CliError::system(
                "worker_handshake_session_invalid",
                "Pi session id and path must be supplied together",
            ));
        }
        return Ok(None);
    };
    if uuid::Uuid::parse_str(session_id).is_err() {
        return Err(CliError::system(
            "worker_handshake_session_invalid",
            format!("Pi session id is not a UUID: {session_id:?}"),
        ));
    }
    let expected_name = format!("pi-session-{session_id}.jsonl");
    let expected_parent = state_root
        .join(".creating")
        .join("pi-sessions")
        .join(run_id);
    if session_path.parent() != Some(expected_parent.as_path())
        || session_path.file_name().and_then(|v| v.to_str()) != Some(&expected_name)
    {
        return Err(CliError::system(
            "worker_handshake_session_invalid",
            "Pi session path is not the attempt-bound file in the run directory",
        ));
    }
    let cwd = std::env::current_dir().map_err(|e| {
        CliError::system(
            "worker_handshake_session_io",
            format!("read worker cwd: {e}"),
        )
    })?;
    let cwd = cwd.canonicalize().map_err(|e| {
        CliError::system(
            "worker_handshake_session_io",
            format!("resolve worker cwd {}: {e}", cwd.display()),
        )
    })?;
    let header = serde_json::json!({
        "type": "session",
        "version": 3,
        "id": session_id,
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "cwd": cwd,
    });
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
        taskfleet_core::nofollow(&mut options);
    }
    let mut file = options.open(session_path).map_err(|e| {
        CliError::system(
            "worker_handshake_session_io",
            format!("create native Pi session {}: {e}", session_path.display()),
        )
    })?;
    serde_json::to_writer(&mut file, &header).map_err(|e| {
        CliError::system(
            "worker_handshake_session_io",
            format!("write native Pi session {}: {e}", session_path.display()),
        )
    })?;
    file.write_all(b"\n")
        .and_then(|()| file.sync_all())
        .map_err(|e| {
            CliError::system(
                "worker_handshake_session_io",
                format!("sync native Pi session {}: {e}", session_path.display()),
            )
        })?;
    std::fs::File::open(&expected_parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| {
            CliError::system(
                "worker_handshake_session_io",
                format!(
                    "sync Pi session directory {}: {e}",
                    expected_parent.display()
                ),
            )
        })?;
    let relative_path = session_path.strip_prefix(state_root).map_err(|_| {
        CliError::system(
            "worker_handshake_session_invalid",
            "Pi session path is outside the state root",
        )
    })?;
    Ok(Some((
        session_id.to_string(),
        relative_path
            .to_str()
            .ok_or_else(|| {
                CliError::system(
                    "worker_handshake_session_invalid",
                    "Pi session path is not UTF-8",
                )
            })?
            .to_string(),
        cwd.to_str()
            .ok_or_else(|| {
                CliError::system(
                    "worker_handshake_session_invalid",
                    "worker cwd is not UTF-8",
                )
            })?
            .to_string(),
    )))
}

pub fn process_start_identity(pid: u32) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let read = unsafe {
            libc::proc_pidinfo(
                pid as i32,
                libc::PROC_PIDTBSDINFO,
                0,
                (&raw mut info).cast(),
                size,
            )
        };
        return (read == size)
            .then(|| format!("macos:{}:{}", info.pbi_start_tvsec, info.pbi_start_tvusec));
    }
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let rest = stat.rsplit_once(") ")?.1;
        let ticks = rest.split_whitespace().nth(19)?;
        return Some(format!("linux:{ticks}"));
    }
    #[allow(unreachable_code)]
    crate::supervise::watchdog::pid_start_time(pid).map(|v| format!("seconds:{v}"))
}

fn write_durable_json(path: &Path, value: &WorkerHandshake) -> Result<(), CliError> {
    let parent = path.parent().expect("validated parent");
    let temp = parent.join(format!(".worker-handshake-{}.tmp", ulid::Ulid::new()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
            taskfleet_core::nofollow(&mut options);
        }
        let mut file = options.open(&temp).map_err(|e| {
            CliError::system(
                "worker_handshake_io",
                format!("create {}: {e}", temp.display()),
            )
        })?;
        serde_json::to_writer(&mut file, value).map_err(|e| {
            CliError::system(
                "worker_handshake_io",
                format!("write {}: {e}", temp.display()),
            )
        })?;
        file.write_all(b"\n").map_err(|e| {
            CliError::system(
                "worker_handshake_io",
                format!("write {}: {e}", temp.display()),
            )
        })?;
        file.sync_all().map_err(|e| {
            CliError::system(
                "worker_handshake_io",
                format!("sync {}: {e}", temp.display()),
            )
        })?;
        std::fs::rename(&temp, path).map_err(|e| {
            CliError::system(
                "worker_handshake_io",
                format!("rename {}: {e}", path.display()),
            )
        })?;
        std::fs::File::open(parent)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| {
                CliError::system(
                    "worker_handshake_io",
                    format!("sync {}: {e}", parent.display()),
                )
            })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}
