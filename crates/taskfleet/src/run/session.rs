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

fn verify(path: &str, id: &str, checkout: &str, pi_style: bool) -> Result<(u64, u64), CliError> {
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
    if pi_style {
        let timestamp = header
            .get("timestamp")
            .and_then(|v| v.as_str())
            .ok_or_else(|| invalid("native session timestamp missing"))?;
        let expected = format!("{}_{}.jsonl", timestamp.replace([':', '.'], "-"), id);
        if requested
            .file_name()
            .is_none_or(|name| name != expected.as_str())
        {
            return Err(invalid(
                "native session filename does not match header timestamp and UUID",
            ));
        }
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
    generation: Option<u64>,
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
        || manifest.status.is_terminal()
        || node.caller_pi_lifecycle.as_ref().is_some_and(|v| {
            !matches!(
                v.state,
                taskfleet_core::schema::CallerPiState::Reserved
                    | taskfleet_core::schema::CallerPiState::Started
            )
        })
        || node.caller_pi_lifecycle.as_ref().is_some_and(|v| {
            v.pi_session_id != pi_id
                || v.session_path.as_deref().is_some_and(|p| p != path)
                || (generation != Some(v.generation)
                    && !(generation.is_none()
                        && node
                            .caller_pi_session
                            .as_ref()
                            .is_some_and(|b| b.generation.is_none())))
        })
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
    if generation.is_some() && node.caller_pi_lifecycle.is_none() {
        return Err(conflict("generation requires an existing reservation"));
    }
    let (file_dev, file_ino) = verify(path, pi_id, checkout, generation.is_some())?;
    let binding = CallerPiSession {
        file_dev,
        file_ino,
        pi_session_id: pi_id.into(),
        session_path: path.into(),
        original_cwd: checkout.into(),
        generation,
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

/// Same-UID attestations are not host ownership proof. The caller must verify
/// its Pi child, native history and inherited writer lease before asserting an
/// exit. In particular control-uncertain is never permission to restart Pi.
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
#[clap(rename_all = "kebab-case")]
pub enum PiStateArg {
    Started,
    LaunchFailed,
    Exited,
    ControlUncertain,
}

impl From<PiStateArg> for taskfleet_core::schema::CallerPiState {
    fn from(value: PiStateArg) -> Self {
        use taskfleet_core::schema::CallerPiState as S;
        match value {
            PiStateArg::Started => S::Started,
            PiStateArg::LaunchFailed => S::LaunchFailed,
            PiStateArg::Exited => S::Exited,
            PiStateArg::ControlUncertain => S::ControlUncertain,
        }
    }
}

pub struct UpdateArgs<'a> {
    pub run_id: &'a str,
    pub node_id: &'a str,
    pub generation: u64,
    pub pi_id: &'a str,
    pub path: Option<&'a str>,
    pub checkout: &'a str,
    pub state: PiStateArg,
    pub reason: Option<&'a str>,
    pub dry_run: bool,
    pub spec: &'a OutputSpec,
    pub warnings: &'a [String],
}

pub fn update(args: UpdateArgs<'_>) -> Result<(), CliError> {
    use taskfleet_core::schema::{CallerPiLifecycle, CallerPiState};
    if args.dry_run {
        return Err(CliError::user(
            "dry_run_unsupported",
            "a lifecycle attestation cannot reserve a Pi generation",
        ));
    }
    let run_id = parse_run_id(args.run_id)?;
    let node_id = parse_node_id(args.node_id)?;
    let state: CallerPiState = args.state.into();
    if args.generation == 0
        || args
            .reason
            .is_some_and(|r| r.trim().is_empty() || r.len() > 1024)
        || (state == CallerPiState::Started) != args.reason.is_none()
    {
        return Err(invalid("started forbids --reason; other states require a nonblank reason (at most 1024 bytes); generation must be positive"));
    }
    let uuid = uuid::Uuid::parse_str(args.pi_id)
        .map_err(|_| invalid("Pi session ID must be a canonical lowercase UUID"))?;
    if uuid.to_string() != args.pi_id {
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
    let binding = node.caller_pi_session.as_ref();
    if manifest.agent_owner != AgentOwner::Caller
        || manifest.status.is_terminal()
        || node.run_id != run_id
        || node.worktree_path.as_deref() != Some(args.checkout)
        || binding.is_some_and(|b| {
            b.original_cwd != args.checkout
                || b.pi_session_id != args.pi_id
                || Some(b.session_path.as_str()) != args.path
        })
        || (binding.is_none() && args.path.is_some())
        || (binding.is_none()
            && !node
                .caller_pi_lifecycle
                .as_ref()
                .is_some_and(|old| old.state == CallerPiState::Reserved))
    {
        return Err(conflict(
            "live caller-owned run/node/checkout/bound Pi identity mismatch",
        ));
    }
    let fact = CallerPiLifecycle {
        generation: args.generation,
        pi_session_id: args.pi_id.into(),
        session_path: args.path.map(str::to_string),
        state,
        reason: args.reason.map(str::to_string),
    };
    // Reconciliation after a lost reply: identical current projection means
    // success with no duplicate event. Older generation retries are refused.
    let replay = node.caller_pi_lifecycle.as_ref() == Some(&fact);
    if !replay {
        append_and_apply_unlocked(
            &witness,
            &paths,
            "caller.pi.lifecycle",
            Some(&node_id),
            None,
            serde_json::to_value(&fact)
                .map_err(|e| CliError::system("internal_serialize", e.to_string()))?,
        )
        .map_err(|e| conflict(format!("caller Pi generation transition refused: {e}")))?;
    }
    drop(guard);
    #[derive(Serialize)]
    struct Response<'a> {
        run_id: &'a str,
        node_id: &'a str,
        caller_agent: &'a CallerPiLifecycle,
        idempotent_replay: bool,
    }
    match args.spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => output::emit_envelope(
            &Response {
                run_id: run_id.as_str(),
                node_id: node_id.as_str(),
                caller_agent: &fact,
                idempotent_replay: replay,
            },
            args.spec,
            args.warnings,
        )?,
        OutputFormat::Text => {
            println!(
                "caller Pi generation {} {:?} on {run_id}/{node_id} (idempotent replay: {replay})",
                fact.generation, fact.state
            );
            output::emit_text_warnings(args.warnings);
        }
    }
    Ok(())
}

pub struct ReserveArgs<'a> {
    pub run_id: &'a str,
    pub node_id: &'a str,
    pub generation: u64,
    pub pi_id: &'a str,
    pub checkout: &'a str,
    pub gate_fd: i32,
    pub writer_fd: i32,
    pub spec: &'a OutputSpec,
    pub warnings: &'a [String],
}

fn launch_identity(
    paths: &taskfleet_core::RunPaths,
    node_id: &taskfleet_core::NodeId,
    checkout: &str,
) -> Result<(taskfleet_core::Manifest, taskfleet_core::Node), CliError> {
    let manifest = read_manifest_opt(paths)
        .map_err(from_core)?
        .ok_or_else(|| conflict("missing run"))?;
    let node = read_node_opt(paths, node_id)
        .map_err(from_core)?
        .ok_or_else(|| conflict("missing node"))?;
    if manifest.agent_owner != AgentOwner::Caller
        || manifest.status.is_terminal()
        || node.run_id != paths.run_id
        || node.worktree_path.as_deref() != Some(checkout)
        || Path::new(checkout)
            .canonicalize()
            .map_err(|_| conflict("checkout unavailable"))?
            .to_string_lossy()
            != checkout
    {
        return Err(conflict("live caller run/node/checkout mismatch"));
    }
    Ok((manifest, node))
}

pub fn fence(
    run: &str,
    node: &str,
    checkout: &str,
    spec: &OutputSpec,
    warnings: &[String],
) -> Result<(), CliError> {
    let id = parse_run_id(run)?;
    let nid = parse_node_id(node)?;
    let paths = super::run_paths_exact(&crate::home::root_dir()?, &id)?;
    let _lock = RunLock::acquire_shared(&paths.lock()).map_err(from_core)?;
    let _ = launch_identity(&paths, &nid, checkout)?;
    let identity = super::writer_fence::inspect(&paths)?;
    check_creation_fence(&paths, &identity)?;
    output::emit_envelope(
        &serde_json::json!({"run_id":run,"node_id":node,"checkout":checkout,
        "launch_gate":{"path":paths.root.join("launch-gate.lock"),"identity":identity.launch_gate},
        "writer":{"path":paths.root.join("writer.lock"),"identity":identity.writer},
        "state":"foundation-only","launch_permission":false}),
        spec,
        warnings,
    )
}

pub fn reserve(args: ReserveArgs<'_>) -> Result<(), CliError> {
    use taskfleet_core::schema::{CallerPiLifecycle, CallerPiState};
    let id = parse_run_id(args.run_id)?;
    let nid = parse_node_id(args.node_id)?;
    let uuid = uuid::Uuid::parse_str(args.pi_id).map_err(|_| invalid("Pi ID must be UUID"))?;
    if uuid.to_string() != args.pi_id || args.generation == 0 {
        return Err(invalid(
            "canonical lowercase UUID and positive generation required",
        ));
    }
    let paths = super::run_paths_exact(&crate::home::root_dir()?, &id)?;
    let guard = RunLock::acquire_existing(&paths.lock()).map_err(from_core)?;
    let (_, node) = launch_identity(&paths, &nid, args.checkout)?;
    let identity = super::writer_fence::check_launch_fds(&paths, args.gate_fd, args.writer_fd)?;
    check_creation_fence(&paths, &identity)?;
    let fact = CallerPiLifecycle {
        generation: args.generation,
        pi_session_id: args.pi_id.into(),
        session_path: None,
        state: CallerPiState::Reserved,
        reason: None,
    };
    let replay = node.caller_pi_lifecycle.as_ref() == Some(&fact);
    if node.caller_pi_session.is_some() || (!replay && node.caller_pi_lifecycle.is_some()) {
        return Err(conflict(
            "reservation already exists; reconcile current generation; no second launch",
        ));
    }
    if !replay {
        append_and_apply_unlocked(
            &guard.witness(),
            &paths,
            "caller.pi.lifecycle",
            Some(&nid),
            None,
            serde_json::to_value(&fact)
                .map_err(|e| CliError::system("internal_serialize", e.to_string()))?,
        )
        .map_err(|e| conflict(format!("reservation refused: {e}")))?;
        if std::env::var_os("TASKFLEET_TEST_CALLER_RESERVE_CRASH_AFTER_APPEND").is_some() {
            std::process::exit(71);
        }
    }
    drop(guard);
    output::emit_envelope(
        &serde_json::json!({"run_id":args.run_id,"node_id":args.node_id,
        "caller_agent":fact,"writer_fence":identity,"idempotent_replay":replay,
        "launch_permission":false}),
        args.spec,
        args.warnings,
    )
}

fn check_creation_fence(
    paths: &taskfleet_core::RunPaths,
    identity: &super::writer_fence::Fence,
) -> Result<(), CliError> {
    let events = taskfleet_core::read_all_events(&paths.events()).map_err(from_core)?;
    if events
        .iter()
        .find(|e| e.kind == "run.created")
        .and_then(|e| e.data.get("writer_fence"))
        != Some(&serde_json::json!(identity))
    {
        return Err(conflict("writer fence differs from run creation event"));
    }
    Ok(())
}
