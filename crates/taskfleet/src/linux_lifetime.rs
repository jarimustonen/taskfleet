//! Linux lifetime belongs to the user manager, never the caller's cgroup.
//! No fork/setsid fallback is safe here. Runtime sockets cross `PrivateTmp`
//! boundaries without exposing tmux to other users. See docs/LINUX-LIFETIME.md.

use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::error::CliError;
use crate::proc::{run_with_timeout, TimedOutcome};

pub(crate) fn runtime_dir() -> Result<PathBuf, CliError> {
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let path = std::env::var_os("XDG_RUNTIME_DIR")
        .map_or_else(|| PathBuf::from(format!("/run/user/{uid}")), PathBuf::from);
    validate_dir(&path, uid)?;
    Ok(path)
}

fn validate_dir(path: &Path, uid: u32) -> Result<(), CliError> {
    let valid = path.is_absolute()
        && std::fs::symlink_metadata(path).is_ok_and(|m| {
            m.is_dir() && !m.file_type().is_symlink() && m.uid() == uid && m.mode() & 0o777 == 0o700
        });
    if !valid {
        return Err(CliError::user("linux_runtime_dir_invalid", format!(
            "runtime directory {} must exist, be an absolute non-symlink directory owned by uid {uid}, and have mode 0700; provision the user's login/lingering runtime directory or correct XDG_RUNTIME_DIR", path.display()
        )));
    }
    Ok(())
}

pub(crate) fn private_dir() -> Result<PathBuf, CliError> {
    let path = runtime_dir()?.join("taskfleet");
    private_subdir(&path)?;
    Ok(path)
}

fn private_subdir(path: &Path) -> Result<(), CliError> {
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(CliError::system("linux_runtime_dir_invalid", e.to_string())),
    }
    validate_dir(path, unsafe { libc::geteuid() })
}

fn home_key() -> Result<String, CliError> {
    use std::os::unix::ffi::OsStrExt;
    let root = crate::home::root_dir()?;
    let hash = Sha256::digest(root.as_os_str().as_bytes());
    Ok(format!("{hash:x}")[..20].to_string())
}

pub(crate) fn tmux_socket() -> Result<PathBuf, CliError> {
    let socket = private_dir()?.join(format!("tmux-{}.sock", home_key()?));
    use std::os::unix::ffi::OsStrExt;
    if socket.as_os_str().as_bytes().len() >= 108 {
        return Err(CliError::user("linux_runtime_dir_invalid", "XDG_RUNTIME_DIR is too long for Taskfleet's Unix socket; use the user's /run/user/<uid> runtime directory"));
    }
    Ok(socket)
}

/// Construct a manager-owned service. --setenv values are not shell text or
/// `ExecStart` argument expansion. Never import values into the shared manager.
pub(crate) fn service(unit: &str, kind: &str) -> Result<Command, CliError> {
    let runtime = runtime_dir()?;
    let mut cmd =
        Command::new(std::env::var_os("SYSTEMD_RUN_BIN").unwrap_or_else(|| "systemd-run".into()));
    cmd.args([
        "--user",
        "--collect",
        "--quiet",
        "--service-type",
        kind,
        "--unit",
        unit,
        "--property=TimeoutStartSec=120",
        "--property=TimeoutStopSec=10",
        // Also remove a stale value from the shared manager's environment, not
        // merely the creating process. Workers have their own launcher guard.
        "--property=UnsetEnvironment=TASKFLEET_CREATOR_SESSION",
    ]);
    for (key, value) in std::env::vars_os() {
        // The new service has its own identity and readiness endpoint.
        if matches!(
            key.to_str(),
            Some(
                "NOTIFY_SOCKET"
                    | "INVOCATION_ID"
                    | "JOURNAL_STREAM"
                    | "SYSTEMD_EXEC_PID"
                    | "TASKFLEET_READINESS_FD"
                    | "TASKFLEET_READINESS_FILE"
                    | "TASKFLEET_CREATOR_SESSION"
            )
        ) {
            continue;
        }
        let mut arg = std::ffi::OsString::from("--setenv=");
        arg.push(key);
        arg.push("=");
        arg.push(value);
        cmd.arg(arg);
    }
    let cwd = std::env::current_dir().map_err(|e| CliError::system("io_error", e.to_string()))?;
    cmd.arg(format!("--working-directory={}", cwd.display()));
    cmd.arg(format!("--setenv=XDG_RUNTIME_DIR={}", runtime.display()));
    cmd.arg(format!(
        "--setenv=TASKFLEET_HOME={}",
        crate::home::root_dir()?.display()
    ));
    // Never reuse an address pointing into a calling service's namespace.
    let bus = format!("unix:path={}/bus", runtime.display());
    cmd.env("XDG_RUNTIME_DIR", &runtime)
        .env("DBUS_SESSION_BUS_ADDRESS", &bus)
        .arg(format!("--setenv=DBUS_SESSION_BUS_ADDRESS={bus}"));
    Ok(cmd)
}

pub(crate) fn run_service(cmd: &mut Command) -> Result<(), CliError> {
    let command = std::mem::replace(cmd, Command::new("systemd-run"));
    match run_with_timeout(command, Duration::from_secs(130), 64 * 1024) {
        TimedOutcome::Exited { status, .. } if status.success() => Ok(()),
        TimedOutcome::Exited { stderr, .. } => Err(manager_error(
            String::from_utf8_lossy(&stderr.bytes).trim().to_string(),
        )),
        TimedOutcome::SpawnErr(e) => Err(manager_error(format!(
            "cannot start systemd-run --user: {e}"
        ))),
        TimedOutcome::TimedOut => Err(manager_error("user-service startup timed out".into())),
    }
}

fn manager_error(reason: String) -> CliError {
    CliError::system("linux_user_unit_failed", format!("{reason}. Taskfleet requires a running systemd user manager on Linux (no cgroup-unsafe fallback). Check `systemctl --user status` and ask the host administrator to enable lingering with `loginctl enable-linger <user>`; inspect the run's supervisor.stderr.log or `journalctl --user -u 'taskfleet-*'` for service failures"))
}

/// The existing confirmed-boot point sends `sd_notify`. A private per-launch
/// receipt keeps the told PID even if a fast supervisor exits before `StartUnit`
/// returns and removes its normal PID file. No pid liveness guess is needed.
pub(crate) fn notify_ready(pid: u32) {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};
    write_receipt(&format!("R{pid}\n"));
    let Some(socket) = std::env::var_os("NOTIFY_SOCKET") else {
        return;
    };
    let bytes = socket.as_bytes();
    let address = if bytes.first() == Some(&b'@') {
        SocketAddr::from_abstract_name(&bytes[1..])
    } else {
        SocketAddr::from_pathname(&socket)
    };
    if let (Ok(socket), Ok(address)) = (UnixDatagram::unbound(), address) {
        let _ = socket.send_to_addr(b"READY=1", &address);
    }
}

/// Preserve typed boot failures as well as readiness across the manager boundary.
pub(crate) fn write_receipt(frame: &str) {
    use std::io::Write;
    if let Some(path) = std::env::var_os("TASKFLEET_READINESS_FILE") {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
        {
            let _ = file.write_all(frame.as_bytes());
        }
    }
}

/// Serialize bootstrap and first-session creation for one state home.
pub(crate) fn tmux_lock() -> Result<std::fs::File, CliError> {
    let path = private_dir()?.join(format!("tmux-{}.lock", home_key()?));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| CliError::system("linux_runtime_dir_invalid", e.to_string()))?;
    fs4::FileExt::lock(&file)
        .map_err(|e| CliError::system("linux_runtime_dir_invalid", e.to_string()))?;
    Ok(file)
}

pub(crate) struct TmuxBootstrap {
    unit: String,
    runtime: PathBuf,
    armed: bool,
}

impl TmuxBootstrap {
    pub(crate) fn commit(&mut self) {
        self.armed = false;
    }
}

impl Drop for TmuxBootstrap {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Called only for the server we just started, while the bootstrap
        // lock is still held. No existing/shared server can be stopped here.
        let mut cmd = Command::new("systemctl");
        cmd.args(["--user", "stop", &self.unit])
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}/bus", self.runtime.display()),
            );
        let _ = run_with_timeout(cmd, Duration::from_secs(15), 4096);
    }
}

pub(crate) fn start_tmux(cwd: &Path) -> Result<TmuxBootstrap, CliError> {
    let socket = tmux_socket()?;
    let bin = resolve_tmux()?;
    let unit = format!("taskfleet-tmux-{}-{}", home_key()?, ulid::Ulid::new());
    let mut cmd = service(&unit, "exec")?;
    let cwd = cwd
        .canonicalize()
        .map_err(|e| CliError::system("io_error", e.to_string()))?;
    cmd.arg(format!("--working-directory={}", cwd.display()))
        .arg("--")
        .arg(bin)
        .args(["-D", "-S"])
        .arg(socket);
    run_service(&mut cmd)?;
    Ok(TmuxBootstrap {
        unit,
        runtime: runtime_dir()?,
        armed: true,
    })
}

fn resolve_tmux() -> Result<PathBuf, CliError> {
    let bin = std::env::var_os("TMUX_BIN").unwrap_or_else(|| "tmux".into());
    let path = PathBuf::from(&bin);
    let found = if path.components().count() > 1 {
        std::path::absolute(path).ok()
    } else {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(&bin))
                .find(|path| path.is_file())
        })
    };
    found.ok_or_else(|| {
        CliError::user(
            "tmux_unavailable",
            "tmux must be installed on PATH (or selected with TMUX_BIN)",
        )
    })
}

/// Workmux's tmux subprocesses must not auto-start a replacement server in the
/// caller's cgroup if the manager-owned server disappears during placement.
/// A private PATH wrapper pins both the socket and tmux's no-start (-N) option.
pub(crate) fn tmux_client_dir() -> Result<tempfile::TempDir, CliError> {
    use std::io::Write;
    let dir = tempfile::Builder::new()
        .prefix("client-")
        .tempdir_in(private_dir()?)
        .map_err(|e| CliError::system("io_error", e.to_string()))?;
    let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    let body = format!(
        "#!/bin/sh\nexec {} -N -S {} \"$@\"\n",
        quote(&resolve_tmux()?),
        quote(&tmux_socket()?)
    );
    let mut file = tempfile::NamedTempFile::new_in(dir.path())
        .map_err(|e| CliError::system("io_error", e.to_string()))?;
    file.as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o700))
        .map_err(|e| CliError::system("io_error", e.to_string()))?;
    file.write_all(body.as_bytes())
        .map_err(|e| CliError::system("io_error", e.to_string()))?;
    file.persist(dir.path().join("tmux"))
        .map_err(|e| CliError::system("io_error", e.to_string()))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_validation_rejects_wrong_owner_symlink_missing_and_open_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let uid = unsafe { libc::geteuid() };
        assert!(validate_dir(dir.path(), uid).is_ok());
        assert!(validate_dir(dir.path(), uid.wrapping_add(1)).is_err());
        assert!(validate_dir(&dir.path().join("missing"), uid).is_err());
        assert!(validate_dir(Path::new(""), uid).is_err());
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(dir.path(), &link).unwrap();
        assert!(validate_dir(&link, uid).is_err());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(validate_dir(dir.path(), uid).is_err());
    }
}
