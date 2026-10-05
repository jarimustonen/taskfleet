//! Durable lock identity for caller-owned runs. This is NOT launch admission:
//! the host must hold the gate through Pi Start and pass the shared lease FD.
//! No settlement endpoint consumes this foundation yet.
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
