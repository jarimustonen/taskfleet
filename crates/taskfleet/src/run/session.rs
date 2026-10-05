//! Caller-owned native Pi binding. No Pi launch, history read or settlement authority.
//! The native file is opened component-by-component relative to a private user
//! session store; no caller-chosen path outside it is ever opened.

use std::ffi::CString;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};

use serde::Serialize;
use taskfleet_core::schema::{AgentOwner, CallerPiSession};
use taskfleet_core::{append_and_apply_unlocked, read_manifest_opt, read_node_opt, RunLock};

use super::{from_core, parse_node_id, parse_run_id};
use crate::error::CliError;
use crate::output::{self, OutputFormat, OutputSpec};

fn invalid(message: impl Into<String>) -> CliError {
    CliError::user("invalid_pi_session", message)
}
fn conflict(message: impl Into<String>) -> CliError {
    CliError::user("pi_session_conflict", message)
}

#[derive(Serialize)]
struct Bound<'a> {
    run_id: &'a str,
    node_id: &'a str,
    caller_pi_session: &'a CallerPiSession,
    idempotent_replay: bool,
}

/// Open a directory or regular file without following *any* component, even
/// if an attacker swaps a parent between pathname checks. Keep parent FDs live.
fn child(parent: &File, name: &std::ffi::OsStr, directory: bool) -> Result<File, CliError> {
    let name = CString::new(name.as_bytes()).map_err(|_| invalid("NUL in session path"))?;
    let flags = libc::O_RDONLY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if directory { libc::O_DIRECTORY } else { 0 };
    // SAFETY: parent is a live directory FD; name is a NUL-terminated CString.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(invalid(format!(
            "native session path unavailable: {}",
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: successful openat returns an owned fd.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn verify(path: &str, id: &str, checkout: &str) -> Result<(u64, u64), CliError> {
    let home = std::env::var_os("HOME").ok_or_else(|| invalid("HOME is unavailable"))?;
    let root = Path::new(&home).join(".pi/agent/sessions");
    let requested = Path::new(path);
    if !requested.is_absolute()
        || requested
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        || requested.extension().is_none_or(|e| e != "jsonl")
    {
        return Err(invalid(
            "session path must be an absolute .jsonl path without traversal",
        ));
    }
    let relative = requested
        .strip_prefix(&root)
        .map_err(|_| invalid("session path is outside HOME/.pi/agent/sessions"))?;
    let parts: Vec<_> = relative.components().collect();
    if parts.is_empty()
        || parts.len() > 3
        || parts.iter().any(|p| !matches!(p, Component::Normal(_)))
    {
        return Err(invalid("session path exceeds private session-store depth"));
    }
    // Open from /, not from an unchecked HOME pathname. No symlink at any level.
    let mut dir = File::open("/").map_err(|e| invalid(format!("open filesystem root: {e}")))?;
    for component in root
        .components()
        .chain(parts[..parts.len() - 1].iter().copied())
    {
        if let Component::Normal(name) = component {
            dir = child(&dir, name, true)?;
            let meta = dir
                .metadata()
                .map_err(|e| invalid(format!("stat session directory: {e}")))?;
            if !meta.is_dir()
                || (meta.uid() != unsafe { libc::geteuid() } && meta.uid() != 0)
                || (meta.mode() & 0o022 != 0 && !(meta.uid() == 0 && meta.mode() & 0o1000 != 0))
            {
                return Err(invalid(
                    "session directory is foreign or group/other writable",
                ));
            }
            if component.as_os_str() == "sessions" && meta.mode() & 0o077 != 0 {
                return Err(invalid("Pi session store must be private (0700)"));
            }
        }
    }
    let Component::Normal(name) = parts[parts.len() - 1] else {
        unreachable!()
    };
    let file = child(&dir, name, false)?;
    let before = file
        .metadata()
        .map_err(|e| invalid(format!("stat session: {e}")))?;
    if !before.is_file()
        || before.uid() != unsafe { libc::geteuid() }
        || before.len() > 512 << 20
        || before.len() == 0
        || before.mode() & 0o022 != 0
    {
        return Err(invalid(
            "session must be a same-user, regular, bounded, non-writable-by-others file",
        ));
    }
    let mut line = Vec::new();
    BufReader::new(
        file.try_clone()
            .map_err(|e| invalid(format!("clone session fd: {e}")))?,
    )
    .take(16385)
    .read_until(b'\n', &mut line)
    .map_err(|e| invalid(format!("read session header: {e}")))?;
    if line.len() > 16384 || line.last() != Some(&b'\n') {
        return Err(invalid("native session header is missing or oversized"));
    }
    let header: serde_json::Value =
        serde_json::from_slice(&line).map_err(|_| invalid("malformed native session header"))?;
    if header.get("type").and_then(|v| v.as_str()) != Some("session")
        || header.get("version").and_then(serde_json::Value::as_u64) != Some(3)
        || header.get("id").and_then(|v| v.as_str()) != Some(id)
        || header.get("cwd").and_then(|v| v.as_str()) != Some(checkout)
    {
        return Err(invalid(
            "native session header does not match Pi ID and recorded checkout",
        ));
    }
    // Re-traverse from / to detect a parent renamed/replaced while the
    // header was read. The held descriptors remain the authority for the
    // content; the pathname must still resolve to that same inode.
    let mut fresh = File::open("/").map_err(|e| invalid(format!("reopen root: {e}")))?;
    for component in root
        .components()
        .chain(parts[..parts.len() - 1].iter().copied())
    {
        if let Component::Normal(name) = component {
            fresh = child(&fresh, name, true)?;
        }
    }
    let stable_parent = dir
        .metadata()
        .map_err(|e| invalid(format!("stat parent: {e}")))?;
    let fresh_parent = fresh
        .metadata()
        .map_err(|e| invalid(format!("stat current parent: {e}")))?;
    if (stable_parent.dev(), stable_parent.ino()) != (fresh_parent.dev(), fresh_parent.ino()) {
        return Err(invalid("native session parent changed during verification"));
    }
    let after = file
        .metadata()
        .map_err(|e| invalid(format!("restat session: {e}")))?;
    let current = child(&fresh, name, false)?;
    let now = current
        .metadata()
        .map_err(|e| invalid(format!("restat session path: {e}")))?;
    if (
        before.dev(),
        before.ino(),
        before.len(),
        before.mtime(),
        before.mtime_nsec(),
        before.ctime(),
        before.ctime_nsec(),
    ) != (
        after.dev(),
        after.ino(),
        after.len(),
        after.mtime(),
        after.mtime_nsec(),
        after.ctime(),
        after.ctime_nsec(),
    ) || (
        before.dev(),
        before.ino(),
        before.len(),
        before.mtime(),
        before.mtime_nsec(),
    ) != (
        now.dev(),
        now.ino(),
        now.len(),
        now.mtime(),
        now.mtime_nsec(),
    ) {
        return Err(invalid("native session changed during verification"));
    }
    Ok((before.dev(), before.ino()))
}

use std::io::Read;

#[allow(clippy::too_many_arguments)] // clap flags plus output envelope are passed exactly once.
pub fn bind(
    dry_run: bool,
    run_id: &str,
    node_id: &str,
    pi_id: &str,
    path: &str,
    checkout: &str,
    spec: &OutputSpec,
    warnings: &[String],
) -> Result<(), CliError> {
    if dry_run {
        return Err(CliError::user(
            "dry_run_unsupported",
            "binding a live native file cannot be reserved by a dry run",
        ));
    }
    let run_id = parse_run_id(run_id)?; // full ID; never resolve a prefix here
    let node_id = parse_node_id(node_id)?;
    let uuid = uuid::Uuid::parse_str(pi_id).map_err(|_| invalid("Pi session ID must be a UUID"))?;
    if uuid.to_string() != pi_id {
        return Err(invalid("Pi session ID must be a canonical lowercase UUID"));
    }
    let root = crate::home::root_dir()?;
    let paths = super::run_paths_exact(&root, &run_id)?;
    if !paths.root.exists() {
        return Err(CliError::user("run_not_found", format!("no run {run_id}")));
    }
    let guard = RunLock::acquire_existing(&paths.lock()).map_err(from_core)?;
    let witness = guard.witness();
    let manifest = read_manifest_opt(&paths)
        .map_err(from_core)?
        .ok_or_else(|| CliError::user("run_not_found", format!("no run {run_id}")))?;
    let node = read_node_opt(&paths, &node_id)
        .map_err(from_core)?
        .ok_or_else(|| {
            CliError::user("node_not_found", format!("no node {node_id} in {run_id}"))
        })?;
    if manifest.agent_owner != AgentOwner::Caller
        || node.run_id != run_id
        || node.worktree_path.as_deref() != Some(checkout)
        || !Path::new(checkout).is_absolute()
        || Path::new(checkout)
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return Err(conflict(
            "run/node/recorded checkout identity does not match a caller-owned run",
        ));
    }
    if Path::new(checkout)
        .canonicalize()
        .map_err(|_| conflict("recorded checkout unavailable"))?
        .to_string_lossy()
        != checkout
    {
        return Err(conflict(
            "recorded checkout is no longer an exact canonical directory",
        ));
    }
    let (file_dev, file_ino) = verify(path, pi_id, checkout)?;
    let binding = CallerPiSession {
        file_dev,
        file_ino,
        pi_session_id: pi_id.into(),
        session_path: path.into(),
        original_cwd: checkout.into(),
    };
    if let Some(existing) = &node.caller_pi_session {
        if existing != &binding {
            return Err(conflict("node is already bound to a different Pi session"));
        }
    }
    // Even retries must check the current native source. A replaced file is not
    // an identical retry merely because its path string matches.
    let replay = node.caller_pi_session.is_some();
    if !replay {
        append_and_apply_unlocked(
            &witness,
            &paths,
            "caller.pi.session_bound",
            Some(&node_id),
            None,
            serde_json::to_value(&binding)
                .map_err(|e| CliError::system("internal_serialize", e.to_string()))?,
        )
        .map_err(from_core)?;
    }
    drop(guard);
    match spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => output::emit_envelope(
            &Bound {
                run_id: run_id.as_str(),
                node_id: node_id.as_str(),
                caller_pi_session: &binding,
                idempotent_replay: replay,
            },
            spec,
            warnings,
        )?,
        OutputFormat::Text => {
            println!(
                "bound Pi session {pi_id} to {run_id}/{node_id} (idempotent replay: {replay})"
            );
            output::emit_text_warnings(warnings);
        }
    }
    Ok(())
}
