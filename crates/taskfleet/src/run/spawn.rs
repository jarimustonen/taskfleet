//! Native Taskfleet worktree/tmux/worker materialization.
//!
//! Taskfleet owns the transaction and invokes `workmux` only as an explicit
//! external CLI. A generated launcher publishes a private, attempt-bound PID
//! handshake immediately before `exec` of the exact recorded candidate. No
//! process name or descendant-tree inference is used.

use std::io::Write as _;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use taskfleet_core::schema::TmuxIdentity;

use crate::error::CliError;
use crate::worker_handshake::{WorkerHandshake, TOKEN_ENV};

#[derive(Debug)]
pub struct SpawnOutcome {
    pub branch: String,
    pub worktree_path: String,
    pub tmux_window: String,
    pub agent_pid_hint: i64,
    pub agent_start_time: u64,
    pub agent_start_identity: String,
    pub tmux_socket: Option<String>,
    pub tmux_session: Option<String>,
    pub tmux_window_id: Option<String>,
    pub tmux_pane_id: Option<String>,
    pub pi_session_id: Option<String>,
    pub pi_session_path: Option<String>,
    pub pi_session_cwd: Option<String>,
    rollback: Option<Rollback>,
}

impl SpawnOutcome {
    pub fn commit(&mut self) {
        if let Some(mut rollback) = self.rollback.take() {
            rollback.commit();
        }
    }
}

pub struct SpawnRequest<'a> {
    // Retained for the unit-only legacy materializer fixture, which still
    // receives the run kind as `--type`; production workmux naming does not.
    #[allow(dead_code)]
    pub kind: &'a str,
    /// Absolute generated launcher passed as one opaque workmux agent command.
    pub agent: Option<&'a str>,
    pub branch: &'a str,
    pub prompt_file: &'a Path,
    pub layout: Option<&'a str>,
    pub no_hooks: bool,
    pub keep_tmux_on_error: bool,
    pub parent_session: Option<&'a str>,
    pub agent_startup_timeout: u32,
    pub source_branch: Option<&'a str>,
    pub cwd: Option<&'a Path>,
    /// Expected private handshake for `agent`. Native materialization refuses
    /// an unbound launcher.
    pub launcher: Option<&'a AgentLauncher>,
}

#[derive(Debug, Clone)]
pub struct AgentLauncher {
    path: PathBuf,
    handshake_path: PathBuf,
    token: String,
    run_id: String,
    node_id: String,
    attempt: u32,
    pi_session_source_path: Option<PathBuf>,
}

impl AgentLauncher {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn cleanup_private_session(&self) {
        if let Some(path) = self.pi_session_source_path.as_deref() {
            let _ = std::fs::remove_file(path);
            if let Some(parent) = path.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }
}

pub fn write_prompt_file(run_dir: &Path, task: &str) -> Result<PathBuf, CliError> {
    let path = run_dir.join("prompt.md");
    std::fs::create_dir_all(run_dir).map_err(|e| io_error("mkdir", run_dir, e))?;
    std::fs::write(&path, task).map_err(|e| io_error("write", &path, e))?;
    Ok(path)
}

/// Write the outer Taskfleet launcher and its inner candidate launcher.
///
/// The inner launcher calls the hidden durable handshake helper with its own
/// shell PID and then immediately `exec`s the recorded argv. POSIX preserves
/// the PID across exec. Autonomous candidates remain wrapped by `run-worker`;
/// that wrapper starts the inner launcher and records its true exit status.
pub fn builtin_agent_selection(harness: &str, autonomous: bool) -> taskfleet_core::AgentSelection {
    taskfleet_core::AgentSelection {
        schema_version: 1,
        profile: "builtin".into(),
        selection_source: "builtin-harness".into(),
        interaction: if autonomous {
            "autonomous"
        } else {
            "explicit-interactive"
        }
        .into(),
        capability: "capable".into(),
        residency: "local".into(),
        requested_harness: Some(harness.into()),
        selected: taskfleet_core::SelectedAgentCandidate {
            candidate_index: 0,
            harness: harness.into(),
            command: vec![harness.into()],
            telemetry: (harness == "pi").then(|| "worker-v1".into()),
        },
        fallback: Vec::new(),
    }
}

pub fn write_agent_launcher(
    run_dir: &Path,
    state_root: &Path,
    selection: &taskfleet_core::AgentSelection,
    run_id: &str,
    node_id: &str,
    attempt: u32,
) -> Result<AgentLauncher, CliError> {
    let selected = &selection.selected;
    if selected.command.is_empty() {
        return Err(CliError::system(
            "recorded_agent_command_invalid",
            "recorded selected candidate has an empty argv",
        ));
    }
    taskfleet_core::RunId::parse_str(run_id)
        .map_err(|e| CliError::system("recorded_run_id_invalid", e.to_string()))?;
    let node = taskfleet_core::NodeId::parse_str(node_id)
        .map_err(|e| CliError::system("recorded_node_id_invalid", e.to_string()))?;
    let self_exe = crate::self_exec::executable().map_err(|e| {
        CliError::system("io_error", format!("current_exe for worker launcher: {e}"))
    })?;
    let state_root = absolute_path(state_root).map_err(|e| io_error("resolve", state_root, e))?;
    let token = ulid::Ulid::new().to_string();
    let handshake_path = run_dir.join(format!(
        "worker-handshake-{}-attempt-{attempt}.json",
        node.as_str()
    ));
    let pi_session = if selected.harness == "pi" {
        for arg in &selected.command[1..] {
            if matches!(
                arg.as_str(),
                "--session"
                    | "--session-id"
                    | "--session-dir"
                    | "--continue"
                    | "-c"
                    | "--resume"
                    | "-r"
                    | "--fork"
                    | "--no-session"
            ) || arg == "--"
                || arg.starts_with("--session=")
                || arg.starts_with("--session-id=")
                || arg.starts_with("--session-dir=")
                || arg.starts_with("--continue=")
                || arg.starts_with("--resume=")
                || arg.starts_with("--fork=")
                || (arg.starts_with("-r") && arg.len() > 2)
            {
                return Err(CliError::user(
                    "recorded_agent_session_conflict",
                    format!("Pi candidate argv contains Taskfleet-owned session flag {arg:?}"),
                ));
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        // Keep the live writer in a run-bound private area outside the staging
        // directory that publication renames. This lets Pi start before
        // publication (preserving the live-worker transaction) while never
        // touching Pi's global/default session tree.
        let creating = state_root.join(".creating");
        ensure_private_directory(&creating)?;
        let sessions = creating.join("pi-sessions");
        ensure_private_directory(&sessions)?;
        let dir = sessions.join(run_id);
        ensure_private_directory(&dir)?;
        let path = dir.join(format!("pi-session-{id}.jsonl"));
        Some((id, path))
    } else {
        None
    };
    let mut private_session_guard =
        PrivateSessionGuard(pi_session.as_ref().map(|(_, path)| path.clone()));
    match std::fs::symlink_metadata(&handshake_path) {
        Ok(meta) if meta.file_type().is_file() || meta.file_type().is_symlink() => {
            std::fs::remove_file(&handshake_path)
                .map_err(|e| io_error("remove stale handshake", &handshake_path, e))?;
        }
        Ok(_) => {
            return Err(CliError::system(
                "worker_handshake_path_invalid",
                format!(
                    "stale handshake path is not a file: {}",
                    handshake_path.display()
                ),
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_error("stat", &handshake_path, e)),
    }
    let inner = run_dir.join(format!(
        "candidate-launch-{}-attempt-{attempt}.sh",
        node.as_str()
    ));
    let outer = run_dir.join(format!(
        "agent-launch-{}-attempt-{attempt}.sh",
        node.as_str()
    ));
    let mut inner_body = b"#!/bin/sh\nset -eu\n".to_vec();
    inner_body.extend_from_slice(b"export ");
    inner_body.extend_from_slice(TOKEN_ENV.as_bytes());
    inner_body.push(b'=');
    inner_body.extend_from_slice(shell_literal(&token).as_bytes());
    inner_body.extend_from_slice(b"\n");
    inner_body.extend_from_slice(&shell_command(
        &self_exe,
        &[
            "worker-handshake".into(),
            "--path".into(),
            os_string(&handshake_path)?,
            "--run-id".into(),
            run_id.into(),
            "--node-id".into(),
            node_id.into(),
            "--attempt".into(),
            attempt.to_string(),
            "--pid".into(),
            "$${RAW}".into(),
            "--state-root".into(),
            os_string(&state_root)?,
        ]
        .into_iter()
        .chain(pi_session.as_ref().into_iter().flat_map(|(id, path)| {
            [
                "--pi-session-id".to_string(),
                id.clone(),
                "--pi-session-path".to_string(),
                path.to_string_lossy().into_owned(),
            ]
        }))
        .collect::<Vec<_>>(),
        true,
    ));
    inner_body.extend_from_slice(b"unset ");
    inner_body.extend_from_slice(TOKEN_ENV.as_bytes());
    inner_body.extend_from_slice(b"\nexec");
    for arg in &selected.command {
        inner_body.push(b' ');
        inner_body.extend_from_slice(shell_literal(arg).as_bytes());
    }
    if let Some((_, path)) = pi_session.as_ref() {
        inner_body.extend_from_slice(b" --session ");
        inner_body.extend_from_slice(&shell_literal_path(path));
    }
    inner_body.extend_from_slice(b" \"$@\"\n");
    write_executable(&inner, &inner_body)?;

    let mut body =
        b"#!/bin/sh\nset -eu\nunset TASKFLEET_RUN_ID TASKFLEET_NODE_ID TASKFLEET_ATTEMPT\n"
            .to_vec();
    if selected.supports_worker_telemetry_v1() {
        writeln!(body, "export TASKFLEET_RUN_ID={}", shell_literal(run_id)).unwrap();
        writeln!(body, "export TASKFLEET_NODE_ID={}", shell_literal(node_id)).unwrap();
        writeln!(
            body,
            "export TASKFLEET_ATTEMPT={}",
            shell_literal(&attempt.to_string())
        )
        .unwrap();
    }
    if selection.interaction == "autonomous" {
        body.extend_from_slice(b"export TASKFLEET_INTERNAL_WORKER_AWAIT_PUBLICATION=1\nexport TASKFLEET_INTERNAL_WORKER_STATE_ROOT=");
        body.extend_from_slice(&shell_literal_path(&state_root));
        body.extend_from_slice(b"\nexec ");
        body.extend_from_slice(&shell_literal_path(&self_exe));
        body.extend_from_slice(b" run-worker ");
        body.extend_from_slice(shell_literal(run_id).as_bytes());
        body.push(b' ');
        body.extend_from_slice(shell_literal(node_id).as_bytes());
        body.extend_from_slice(b" --attempt ");
        body.extend_from_slice(shell_literal(&attempt.to_string()).as_bytes());
        body.extend_from_slice(b" -- ");
    } else {
        body.extend_from_slice(b"exec ");
    }
    body.extend_from_slice(&shell_literal_path(&inner));
    body.extend_from_slice(b" \"$@\"\n");
    write_executable(&outer, &body)?;

    let launcher = AgentLauncher {
        path: outer
            .canonicalize()
            .map_err(|e| io_error("canonicalize", &outer, e))?,
        handshake_path,
        token,
        run_id: run_id.into(),
        node_id: node_id.into(),
        attempt,
        pi_session_source_path: pi_session.as_ref().map(|(_, path)| path.clone()),
    };
    private_session_guard.0 = None;
    Ok(launcher)
}

fn ensure_private_directory(path: &Path) -> Result<(), CliError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(CliError::system(
                    "private_session_path_invalid",
                    format!("{} is not a real directory", path.display()),
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt as _;
                builder.mode(0o700);
            }
            builder
                .create(path)
                .map_err(|e| io_error("mkdir", path, e))?;
        }
        Err(error) => return Err(io_error("stat", path, error)),
    }
    Ok(())
}

fn shell_command(exe: &Path, args: &[String], raw_pid: bool) -> Vec<u8> {
    let mut out = shell_literal_path(exe);
    for arg in args {
        out.push(b' ');
        if raw_pid && arg == "$${RAW}" {
            out.extend_from_slice(b"\"$$\"");
        } else {
            out.extend_from_slice(shell_literal(arg).as_bytes());
        }
    }
    out.push(b'\n');
    out
}

fn os_string(path: &Path) -> Result<String, CliError> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        CliError::system(
            "agent_launcher_path_invalid",
            format!("path is not UTF-8: {}", path.display()),
        )
    })
}

fn write_executable(path: &Path, body: &[u8]) -> Result<(), CliError> {
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o700);
        let mut file = options
            .open(&temp)
            .map_err(|e| io_error("create", &temp, e))?;
        file.write_all(body)
            .map_err(|e| io_error("write", &temp, e))?;
        file.sync_all().map_err(|e| io_error("sync", &temp, e))?;
        std::fs::rename(&temp, path).map_err(|e| io_error("rename", path, e))?;
        let parent = path.parent().ok_or_else(|| {
            CliError::system("agent_launcher_path_invalid", "launcher path has no parent")
        })?;
        std::fs::File::open(parent)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| io_error("sync", parent, e))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn await_handshake(
    launcher: &AgentLauncher,
    timeout: Duration,
) -> Result<WorkerHandshake, CliError> {
    let deadline = Instant::now() + timeout;
    loop {
        match std::fs::read(&launcher.handshake_path) {
            Ok(bytes) => {
                let h: WorkerHandshake = serde_json::from_slice(&bytes).map_err(|e| {
                    CliError::system(
                        "worker_handshake_invalid",
                        format!("parse {}: {e}", launcher.handshake_path.display()),
                    )
                })?;
                if h.schema_version != 1
                    || h.run_id != launcher.run_id
                    || h.node_id != launcher.node_id
                    || h.attempt != launcher.attempt
                    || h.token != launcher.token
                {
                    return Err(CliError::system(
                        "worker_handshake_binding_mismatch",
                        "worker handshake did not match this run/node/attempt/token",
                    ));
                }
                verify_identity(h.pid, h.start_time, &h.start_identity)?;
                return Ok(h);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error("read", &launcher.handshake_path, e)),
        }
        if Instant::now() >= deadline {
            return Err(CliError::system(
                "worker_handshake_timeout",
                format!("worker did not publish a PID handshake within {timeout:?}"),
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn verify_identity(pid: u32, start_time: u64, start_identity: &str) -> Result<(), CliError> {
    if crate::supervise::pid_file::to_pid_t(pid).is_none() {
        return Err(CliError::system(
            "agent_pid_invalid",
            format!("worker handshake contained invalid PID {pid}"),
        ));
    }
    let observed = crate::supervise::watchdog::pid_start_time(pid);
    let observed_identity = crate::worker_handshake::process_start_identity(pid);
    if observed != Some(start_time)
        || observed_identity.as_deref() != Some(start_identity)
        || !crate::supervise::pid_file::pid_alive(pid)
    {
        return Err(CliError::system(
            "worker_handshake_identity_mismatch",
            format!("worker PID {pid} exited or its start identity changed before publication"),
        ));
    }
    Ok(())
}

/// Native materialization. The launcher handshake supplies the stable pane ID
/// from inside the live pane, eliminating the old name-lookup settle retry.
pub fn materialize_native(req: &SpawnRequest<'_>) -> Result<SpawnOutcome, CliError> {
    #[cfg(test)]
    if let Some(result) = test_script_materialize(req) {
        return result;
    }
    materialize(req)
}

#[cfg(test)]
fn test_script_materialize(req: &SpawnRequest<'_>) -> Option<Result<SpawnOutcome, CliError>> {
    #[derive(serde::Deserialize)]
    struct FixtureOutcome {
        branch: String,
        worktree_path: String,
        tmux_window: String,
        agent_pid_hint: i64,
        #[serde(default)]
        tmux_socket: Option<String>,
        #[serde(default)]
        tmux_session: Option<String>,
        #[serde(default)]
        tmux_window_id: Option<String>,
        #[serde(default)]
        tmux_pane_id: Option<String>,
    }
    let script = std::env::var_os("TASKFLEET_CREATE_SH")?;
    let mut command = Command::new(script);
    if let Some(cwd) = req.cwd {
        command.current_dir(cwd);
    }
    command.args(["--type", req.kind]);
    // Legacy supervisor fixtures launch their own sleeping worker. The one
    // launcher-boundary test opts in with an explicit test self executable.
    if std::env::var_os("TASKFLEET_TEST_SELF_EXE").is_some() {
        if let Some(agent) = req.agent {
            command.args(["--agent", agent]);
        }
    }
    if let Some(session) = req.parent_session {
        command.args(["--parent-session", session]);
    }
    if let Some(base) = req.source_branch {
        command.args(["--base", base]);
    }
    command.arg(req.branch).arg(req.prompt_file);
    let output = match command.output() {
        Ok(output) => output,
        Err(e) => return Some(Err(CliError::system("spawn_failed", e.to_string()))),
    };
    if !output.status.success() {
        return Some(Err(CliError::system(
            "create_sh_error_workmux-add-failed",
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )));
    }
    Some(
        serde_json::from_slice::<FixtureOutcome>(&output.stdout)
            .map(|o| SpawnOutcome {
                branch: o.branch,
                worktree_path: o.worktree_path,
                tmux_window: o.tmux_window,
                agent_pid_hint: o.agent_pid_hint,
                agent_start_time: crate::supervise::watchdog::pid_start_time(
                    u32::try_from(o.agent_pid_hint).unwrap_or_default(),
                )
                .unwrap_or_default(),
                agent_start_identity: crate::worker_handshake::process_start_identity(
                    u32::try_from(o.agent_pid_hint).unwrap_or_default(),
                )
                .unwrap_or_default(),
                tmux_socket: o.tmux_socket,
                tmux_session: o.tmux_session,
                tmux_window_id: o.tmux_window_id,
                tmux_pane_id: o.tmux_pane_id,
                pi_session_id: None,
                pi_session_path: None,
                pi_session_cwd: None,
                rollback: None,
            })
            .map_err(|e| CliError::system("create_sh_unparseable_stdout", e.to_string())),
    )
}

fn materialize(req: &SpawnRequest<'_>) -> Result<SpawnOutcome, CliError> {
    validate_request(req)?;
    let launcher = req.launcher.ok_or_else(|| {
        CliError::system(
            "worker_launcher_required",
            "native materialization requires an attempt-bound generated launcher",
        )
    })?;
    let agent = req.agent.ok_or_else(|| {
        CliError::system(
            "worker_launcher_required",
            "native materialization requires the generated launcher path",
        )
    })?;
    if Path::new(agent) != launcher.path() {
        return Err(CliError::system(
            "worker_launcher_binding_mismatch",
            "workmux agent path is not the expected generated launcher",
        ));
    }
    let cwd = req.cwd.unwrap_or_else(|| Path::new("."));
    let session = ensure_session(req.parent_session, cwd)?;
    let mut rollback = Rollback::new(req, cwd);

    let mut args = vec![
        "add".into(),
        req.branch.into(),
        "-b".into(),
        "-P".into(),
        os_string(req.prompt_file)?,
        "-a".into(),
        agent.into(),
    ];
    if let Some(s) = req.parent_session {
        args.extend(["--parent-session".into(), s.into()]);
    }
    if let Some(base) = req.source_branch {
        args.extend(["--base".into(), base.into()]);
    }
    if let Some(layout) = req.layout {
        args.extend(["-l".into(), layout.into()]);
    }
    if req.no_hooks {
        args.push("--no-hooks".into());
    }
    // `workmux add` may fail after creating a branch/worktree/window. Arm the
    // rollback before spawning it so a non-zero exit receives the same complete
    // cleanup as every later failure.
    rollback.workmux_added = true;
    let add = command_output("workmux", &args, cwd, "workmux_add_failed")?;
    if !add.status.success() {
        return Err(CliError::user(
            "workmux_add_failed",
            format!(
                "workmux add exited {:?}: {}",
                add.status.code(),
                String::from_utf8_lossy(&add.stderr).trim()
            ),
        ));
    }

    let path_out = command_output(
        "workmux",
        &["path".into(), req.branch.into()],
        cwd,
        "worktree_path_unresolved",
    )?;
    let worktree = text_stdout(&path_out, "worktree_path_unresolved")?
        .trim()
        .to_string();
    if worktree.is_empty() || !Path::new(&worktree).is_dir() {
        return Err(CliError::system(
            "worktree_path_unresolved",
            "workmux path did not return an existing directory",
        ));
    }
    rollback.worktree_path = Some(worktree.clone());

    let handshake = await_handshake(
        launcher,
        Duration::from_secs(u64::from(req.agent_startup_timeout)),
    )?;
    // The helper returns immediately and the shell execs the candidate with the
    // same PID. A short settle window makes missing and immediate-exit commands
    // fail privately instead of publishing a stillborn node.
    std::thread::sleep(Duration::from_millis(100));
    verify_identity(
        handshake.pid,
        handshake.start_time,
        &handshake.start_identity,
    )?;
    let pane = handshake.tmux_pane_id.clone();
    // Workmux is the sole owner of display naming, including its layered
    // `window_prefix` configuration. Record what it actually created; the
    // stable ids below, not a Taskfleet-generated name, own later operations.
    let window = query_tmux_window(&pane, &session, cwd)?;

    let dest = Path::new(&worktree)
        .join("history/.worktree")
        .join(format!("{}.md", req.branch));
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_error("mkdir", parent, e))?;
    }
    std::fs::copy(req.prompt_file, &dest).map_err(|e| io_error("copy", &dest, e))?;
    verify_identity(
        handshake.pid,
        handshake.start_time,
        &handshake.start_identity,
    )?;
    Ok(SpawnOutcome {
        branch: req.branch.into(),
        worktree_path: worktree,
        tmux_window: window.name,
        agent_pid_hint: i64::from(handshake.pid),
        agent_start_time: handshake.start_time,
        agent_start_identity: handshake.start_identity,
        tmux_socket: window.identity.socket,
        tmux_session: Some(session),
        tmux_window_id: Some(window.identity.window_id),
        tmux_pane_id: Some(pane),
        pi_session_id: handshake.pi_session_id,
        pi_session_path: handshake.pi_session_path,
        pi_session_cwd: handshake.pi_session_cwd,
        rollback: Some(rollback),
    })
}

fn validate_request(req: &SpawnRequest<'_>) -> Result<(), CliError> {
    if !req.prompt_file.is_file() {
        return Err(CliError::user(
            "prompt_file_not_readable",
            format!("prompt file is not readable: {}", req.prompt_file.display()),
        ));
    }
    let cwd = req.cwd.unwrap_or_else(|| Path::new("."));
    command_ok(
        "git",
        &[
            "check-ref-format".into(),
            "--branch".into(),
            req.branch.into(),
        ],
        cwd,
        "invalid_branch_name",
    )?;
    let exists = command_output(
        "git",
        &[
            "show-ref".into(),
            "--verify".into(),
            "--quiet".into(),
            format!("refs/heads/{}", req.branch),
        ],
        cwd,
        "git_preflight_failed",
    )?;
    if exists.status.success() {
        return Err(CliError::user(
            "branch_exists",
            format!("branch already exists: {}", req.branch),
        ));
    }
    if let Some(base) = req.source_branch {
        command_ok(
            "git",
            &[
                "rev-parse".into(),
                "--verify".into(),
                "--quiet".into(),
                format!("{base}^{{commit}}"),
            ],
            cwd,
            "base_ref_not_found",
        )?;
    }
    Ok(())
}

fn ensure_session(parent: Option<&str>, cwd: &Path) -> Result<String, CliError> {
    if let Some(session) = parent {
        let probe = command_output(
            "tmux",
            &["has-session".into(), "-t".into(), session.into()],
            cwd,
            "parent_session_uncreatable",
        )?;
        if !probe.status.success() {
            let made = command_output(
                "tmux",
                &[
                    "new-session".into(),
                    "-d".into(),
                    "-s".into(),
                    session.into(),
                ],
                cwd,
                "parent_session_uncreatable",
            )?;
            if !made.status.success() {
                // A concurrent creator may have won the probe/create race.
                command_ok(
                    "tmux",
                    &["has-session".into(), "-t".into(), session.into()],
                    cwd,
                    "parent_session_uncreatable",
                )?;
            }
        }
        command_ok(
            "tmux",
            &["has-session".into(), "-t".into(), session.into()],
            cwd,
            "parent_session_uncreatable",
        )?;
        Ok(session.into())
    } else {
        if std::env::var_os("TMUX").is_none() {
            return Err(CliError::user(
                "no_tmux_session",
                "run create must be inside tmux or use --headless/--tmux-session",
            ));
        }
        let out = command_output(
            "tmux",
            &[
                "display-message".into(),
                "-p".into(),
                "#{session_name}".into(),
            ],
            cwd,
            "no_tmux_session",
        )?;
        let s = text_stdout(&out, "no_tmux_session")?.trim().to_string();
        if s.is_empty() {
            Err(CliError::user(
                "no_tmux_session",
                "run create must be inside tmux or use --headless/--tmux-session",
            ))
        } else {
            Ok(s)
        }
    }
}

struct TmuxWindow {
    identity: TmuxIdentity,
    name: String,
}

fn query_tmux_window(
    pane: &str,
    expected_session: &str,
    cwd: &Path,
) -> Result<TmuxWindow, CliError> {
    let out = command_output(
        "tmux",
        &[
            "display-message".into(),
            "-p".into(),
            "-t".into(),
            pane.into(),
            "#{socket_path}\t#{session_name}\t#{window_id}\t#{window_name}".into(),
        ],
        cwd,
        "tmux_identity_unavailable",
    )?;
    let text = text_stdout(&out, "tmux_identity_unavailable")?.trim_end_matches(['\r', '\n']);
    let mut fields = text.splitn(4, '\t');
    let socket = fields.next().unwrap_or("");
    let session = fields.next().unwrap_or("");
    let window_id = fields.next().unwrap_or("");
    let name = fields.next().unwrap_or("");
    if session != expected_session || !window_id.starts_with('@') || name.is_empty() {
        return Err(CliError::system(
            "tmux_identity_unavailable",
            "tmux returned a wrong-session or malformed worker identity/name",
        ));
    }
    Ok(TmuxWindow {
        identity: TmuxIdentity {
            socket: (!socket.is_empty()).then(|| socket.into()),
            session: session.into(),
            window_id: window_id.into(),
            pane_id: Some(pane.into()),
        },
        name: name.into(),
    })
}

struct PrivateSessionGuard(Option<PathBuf>);

impl Drop for PrivateSessionGuard {
    fn drop(&mut self) {
        if let Some(path) = self.0.as_deref() {
            let _ = std::fs::remove_file(path);
            if let Some(parent) = path.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }
}

#[derive(Debug)]
struct Rollback {
    branch: String,
    keep_tmux_on_error: bool,
    cwd: PathBuf,
    workmux_added: bool,
    worktree_path: Option<String>,
    committed: bool,
}
impl Rollback {
    fn new(req: &SpawnRequest<'_>, cwd: &Path) -> Self {
        Self {
            branch: req.branch.into(),
            keep_tmux_on_error: req.keep_tmux_on_error,
            cwd: cwd.into(),
            workmux_added: false,
            worktree_path: None,
            committed: false,
        }
    }
    fn commit(&mut self) {
        self.committed = true;
    }
}
impl Drop for Rollback {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if self.workmux_added && self.keep_tmux_on_error {
            // Preserve the old debug contract: leave the pane, but remove git
            // resources without asking workmux to kill that pane.
            if let Some(path) = &self.worktree_path {
                let _ = command_output(
                    "git",
                    &[
                        "worktree".into(),
                        "remove".into(),
                        "--force".into(),
                        path.clone(),
                    ],
                    &self.cwd,
                    "rollback",
                );
            }
            let _ = command_output(
                "git",
                &[
                    "branch".into(),
                    "-D".into(),
                    "--".into(),
                    self.branch.clone(),
                ],
                &self.cwd,
                "rollback",
            );
        } else if self.workmux_added {
            let _ = command_output(
                "workmux",
                &["remove".into(), "--force".into(), self.branch.clone()],
                &self.cwd,
                "rollback",
            );
            if let Some(path) = &self.worktree_path {
                let _ = command_output(
                    "git",
                    &[
                        "worktree".into(),
                        "remove".into(),
                        "--force".into(),
                        path.clone(),
                    ],
                    &self.cwd,
                    "rollback",
                );
            }
            let _ = command_output(
                "git",
                &[
                    "branch".into(),
                    "-D".into(),
                    "--".into(),
                    self.branch.clone(),
                ],
                &self.cwd,
                "rollback",
            );
        }
    }
}

fn command_output(
    bin: &str,
    args: &[String],
    cwd: &Path,
    code: &'static str,
) -> Result<Output, CliError> {
    let actual = match bin {
        "tmux" => std::env::var("TMUX_BIN").unwrap_or_else(|_| bin.into()),
        "git" => std::env::var("GIT_BIN").unwrap_or_else(|_| bin.into()),
        "workmux" => std::env::var("WORKMUX_BIN").unwrap_or_else(|_| bin.into()),
        _ => bin.into(),
    };
    Command::new(&actual)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| CliError::system(code, format!("spawn {actual}: {e}")))
}
fn command_ok(bin: &str, args: &[String], cwd: &Path, code: &'static str) -> Result<(), CliError> {
    let out = command_output(bin, args, cwd, code)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(CliError::system(
            code,
            format!(
                "{bin} exited {:?}: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ))
    }
}
fn text_stdout<'a>(out: &'a Output, code: &'static str) -> Result<&'a str, CliError> {
    std::str::from_utf8(&out.stdout)
        .map_err(|e| CliError::system(code, format!("command stdout was not UTF-8: {e}")))
}
fn io_error(op: &str, path: &Path, e: std::io::Error) -> CliError {
    CliError::system("io_error", format!("{op} {}: {e}", path.display()))
}
fn absolute_path(path: &Path) -> std::io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.into())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}
fn shell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
#[cfg(unix)]
fn shell_literal_path(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt as _;
    let mut q = vec![b'\''];
    for b in path.as_os_str().as_bytes() {
        if *b == b'\'' {
            q.extend_from_slice(b"'\\''");
        } else {
            q.push(*b);
        }
    }
    q.push(b'\'');
    q
}

pub fn verify_agent_pid(pid: i64, start_time: u64, start_identity: &str) -> Result<(), CliError> {
    let pid = u32::try_from(pid)
        .map_err(|_| CliError::system("agent_pid_invalid", format!("invalid agent PID {pid}")))?;
    verify_identity(pid, start_time, start_identity)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn launcher(dir: &Path) -> AgentLauncher {
        AgentLauncher {
            path: dir.join("launcher.sh"),
            handshake_path: dir.join("handshake.json"),
            token: "0123456789ABCDEFGHJKMNPQRS".into(),
            run_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
            node_id: "n-0001".into(),
            attempt: 3,
            pi_session_source_path: None,
        }
    }

    fn record(launcher: &AgentLauncher) -> WorkerHandshake {
        let pid = std::process::id();
        WorkerHandshake {
            schema_version: 1,
            run_id: launcher.run_id.clone(),
            node_id: launcher.node_id.clone(),
            attempt: launcher.attempt,
            token: launcher.token.clone(),
            pid,
            start_time: crate::supervise::watchdog::pid_start_time(pid).unwrap(),
            start_identity: crate::worker_handshake::process_start_identity(pid).unwrap(),
            tmux_pane_id: "%7".into(),
            pi_session_id: None,
            pi_session_path: None,
            pi_session_cwd: None,
        }
    }

    #[test]
    fn launcher_cleanup_removes_only_its_exact_session_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join(".creating/pi-sessions/run");
        std::fs::create_dir_all(&dir).unwrap();
        let owned = dir.join("pi-session-owned.jsonl");
        let sibling = dir.join("pi-session-sibling.jsonl");
        std::fs::write(&owned, "owned").unwrap();
        std::fs::write(&sibling, "sibling").unwrap();
        let mut launcher = launcher(tmp.path());
        launcher.pi_session_source_path = Some(owned.clone());

        launcher.cleanup_private_session();

        assert!(!owned.exists());
        assert!(sibling.exists(), "sibling attempt transcript survives");
        assert!(
            dir.exists(),
            "non-empty shared parent is never recursive-deleted"
        );
    }

    #[test]
    fn pi_launcher_assigns_one_exact_native_session_before_exec() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let run_dir = tmp.path().join("runs/01jxsnap000000000000000000");
        std::fs::create_dir_all(&run_dir).unwrap();
        let fake_self = tmp.path().join("taskfleet-test");
        std::fs::write(&fake_self, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = std::fs::metadata(&fake_self).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt as _;
        permissions.set_mode(0o700);
        std::fs::set_permissions(&fake_self, permissions).unwrap();
        let prior = std::env::var_os("TASKFLEET_TEST_SELF_EXE");
        std::env::set_var("TASKFLEET_TEST_SELF_EXE", &fake_self);
        let selection = builtin_agent_selection("pi", true);
        let launcher = write_agent_launcher(
            &run_dir,
            tmp.path(),
            &selection,
            "01jxsnap000000000000000000",
            "n-0001",
            0,
        )
        .unwrap();
        match prior {
            Some(value) => std::env::set_var("TASKFLEET_TEST_SELF_EXE", value),
            None => std::env::remove_var("TASKFLEET_TEST_SELF_EXE"),
        }
        let inner =
            std::fs::read_to_string(run_dir.join("candidate-launch-n-0001-attempt-0.sh")).unwrap();
        assert!(inner.contains("worker-handshake"));
        assert!(inner.contains("--pi-session-id"));
        assert!(inner.contains("--pi-session-path"));
        assert!(inner.contains("exec 'pi' --session"));
        let marker = "/pi-session-";
        let id_start = inner.find(marker).unwrap() + marker.len();
        let id = &inner[id_start..id_start + 36];
        assert!(uuid::Uuid::parse_str(id).is_ok());
        assert!(
            inner.matches(id).count() >= 2,
            "same id binds helper and Pi path"
        );
        assert!(launcher.path().is_absolute());
    }

    #[test]
    fn pi_launcher_rejects_caller_owned_session_selection() {
        let tmp = tempfile::TempDir::new().unwrap();
        let run_dir = tmp.path().join("runs/01jxsnap000000000000000000");
        std::fs::create_dir_all(&run_dir).unwrap();
        let mut selection = builtin_agent_selection("pi", true);
        selection.selected.command.push("--continue".into());
        let error = write_agent_launcher(
            &run_dir,
            tmp.path(),
            &selection,
            "01jxsnap000000000000000000",
            "n-0001",
            0,
        )
        .unwrap_err();
        assert_eq!(error.code, "recorded_agent_session_conflict");
    }

    #[test]
    fn handshake_rejects_wrong_attempt_and_forged_token() {
        let dir = tempfile::TempDir::new().unwrap();
        let launcher = launcher(dir.path());
        for mut bad in [record(&launcher), record(&launcher)] {
            if bad.attempt == launcher.attempt {
                bad.attempt += 1;
            } else {
                bad.token.push('X');
            }
            std::fs::write(&launcher.handshake_path, serde_json::to_vec(&bad).unwrap()).unwrap();
            assert_eq!(
                await_handshake(&launcher, Duration::from_millis(20))
                    .unwrap_err()
                    .code,
                "worker_handshake_binding_mismatch"
            );
            // Exercise the other binding on the second iteration.
            std::fs::remove_file(&launcher.handshake_path).unwrap();
        }
        let mut forged = record(&launcher);
        forged.token.push('X');
        std::fs::write(
            &launcher.handshake_path,
            serde_json::to_vec(&forged).unwrap(),
        )
        .unwrap();
        assert_eq!(
            await_handshake(&launcher, Duration::from_millis(20))
                .unwrap_err()
                .code,
            "worker_handshake_binding_mismatch"
        );
    }

    #[test]
    fn handshake_rejects_pid_start_identity_mismatch_and_timeout() {
        let dir = tempfile::TempDir::new().unwrap();
        let launcher = launcher(dir.path());
        let mut stale = record(&launcher);
        stale.start_time = stale.start_time.saturating_add(1);
        std::fs::write(
            &launcher.handshake_path,
            serde_json::to_vec(&stale).unwrap(),
        )
        .unwrap();
        assert_eq!(
            await_handshake(&launcher, Duration::from_millis(20))
                .unwrap_err()
                .code,
            "worker_handshake_identity_mismatch"
        );
        std::fs::remove_file(&launcher.handshake_path).unwrap();
        assert_eq!(
            await_handshake(&launcher, Duration::from_millis(20))
                .unwrap_err()
                .code,
            "worker_handshake_timeout"
        );
    }
}
