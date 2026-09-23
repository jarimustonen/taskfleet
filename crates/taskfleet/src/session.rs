//! Persistent autonomous-worker tmux sessions and bounded inert displays.
//!
//! Retention is opt-in and recorded on each run. Durable Pi evidence is always
//! archived first; the original worker pane is then stopped/removed before a
//! dead `remain-on-exit` display is created outside the deleted worktree.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use chrono::Utc;
use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::json;
use taskfleet_core::{
    append_and_apply_event, append_and_apply_unlocked, EvidenceStatus, Node, NodeId,
    RetainedDisplay, RunId, RunLock, RunPaths,
};

use crate::error::CliError;
use crate::output::{self, OutputFormat, OutputSpec};
use crate::proc::{run_with_timeout, TimedOutcome};

const TMUX_TIMEOUT: Duration = Duration::from_secs(3);
const OUTPUT_CAP: usize = 256 * 1024;
const OWNER_OPTION: &str = "@taskfleet_retained_id";
const ORIGINAL_SHELL_OPTION: &str = "@taskfleet_original_shell";
const ORIGINAL_SCREEN_OPTION: &str = "@taskfleet_original_screen";

#[derive(Subcommand, Debug)]
pub enum SessionAction {
    /// Expire exact Taskfleet-owned inert displays according to recorded TTL/count policy.
    Maintain(MaintainArgs),
}

#[derive(Args, Debug)]
pub struct MaintainArgs {
    /// Bound the complete scan and cleanup invocation. Range 1–300 seconds.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=300))]
    timeout_secs: u64,
}

pub fn dispatch(
    action: SessionAction,
    spec: &OutputSpec,
    warnings: &[String],
) -> Result<(), CliError> {
    match action {
        SessionAction::Maintain(args) => maintain(args, spec, warnings),
    }
}

#[derive(Debug, Serialize)]
struct MaintainPayload {
    examined: usize,
    eligible: usize,
    expired: usize,
    missing: usize,
    preserved: usize,
    timed_out: bool,
    results: Vec<MaintainResult>,
}

#[derive(Debug, Serialize)]
struct MaintainResult {
    run_id: String,
    node_id: String,
    action: &'static str,
    reason: String,
}

#[derive(Clone)]
struct Candidate {
    paths: RunPaths,
    node_id: NodeId,
    display: RetainedDisplay,
    max: u32,
    count_expired: bool,
}

fn maintain(args: MaintainArgs, spec: &OutputSpec, warnings: &[String]) -> Result<(), CliError> {
    let root = crate::home::root_dir()?;
    let deadline = Instant::now() + Duration::from_secs(args.timeout_secs);
    let mut payload = MaintainPayload {
        examined: 0,
        eligible: 0,
        expired: 0,
        missing: 0,
        preserved: 0,
        timed_out: false,
        results: Vec::new(),
    };
    let mut candidates = scan_candidates(&root, deadline, &mut payload)?;
    mark_count_expiry(&mut candidates);
    let now = Utc::now();
    for candidate in candidates {
        if Instant::now() >= deadline {
            payload.timed_out = true;
            payload.preserved += 1;
            payload.results.push(result(
                &candidate,
                "preserved",
                "maintenance deadline reached",
            ));
            continue;
        }
        if candidate.display.expires_at > now && !candidate.count_expired {
            continue;
        }
        payload.eligible += 1;
        match expire_candidate(&candidate, deadline) {
            Expire::Expired => {
                payload.expired += 1;
                payload.results.push(result(
                    &candidate,
                    "expired",
                    if candidate.count_expired {
                        "count limit"
                    } else {
                        "ttl"
                    },
                ));
            }
            Expire::Missing => {
                payload.missing += 1;
                payload.results.push(result(
                    &candidate,
                    "missing",
                    "recorded tmux server/window is absent; no server started",
                ));
            }
            Expire::Preserved(reason) => {
                payload.preserved += 1;
                payload
                    .results
                    .push(result(&candidate, "preserved", &reason));
            }
        }
    }
    emit_maintenance(payload, spec, warnings)
}

fn result(candidate: &Candidate, action: &'static str, reason: &str) -> MaintainResult {
    MaintainResult {
        run_id: candidate.paths.run_id.to_string(),
        node_id: candidate.node_id.to_string(),
        action,
        reason: reason.into(),
    }
}

fn scan_candidates(
    root: &Path,
    deadline: Instant,
    payload: &mut MaintainPayload,
) -> Result<Vec<Candidate>, CliError> {
    let runs = root.join("runs");
    let entries = match std::fs::read_dir(&runs) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CliError::system(
                "state_unreadable",
                format!("read {}: {error}", runs.display()),
            ))
        }
    };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    dirs.sort();
    let mut out = Vec::new();
    for dir in dirs {
        if Instant::now() >= deadline {
            payload.timed_out = true;
            break;
        }
        let Some(raw) = dir.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let Ok(run_id) = RunId::parse_str(raw) else {
            continue;
        };
        let Ok(paths) = RunPaths::from_validated(&dir, run_id) else {
            continue;
        };
        let Ok(Some(guard)) = RunLock::try_acquire_existing(&paths.lock()) else {
            payload.preserved += 1;
            continue;
        };
        let snapshot = (|| {
            let Some(manifest) = taskfleet_core::read_manifest_opt(&paths)? else {
                return Ok::<_, taskfleet_core::Error>(None);
            };
            let Some(policy) = manifest.tmux_retention else {
                return Ok(None);
            };
            Ok(Some((policy, read_nodes(&paths)?)))
        })();
        drop(guard);
        let Ok(Some((policy, nodes))) = snapshot else {
            payload.preserved += 1;
            continue;
        };
        for node in nodes {
            payload.examined += 1;
            if let Some(display) = node
                .retained_display
                .filter(|display| display.expired_at.is_none())
            {
                out.push(Candidate {
                    paths: paths.clone(),
                    node_id: node.node_id,
                    display: *display,
                    max: policy.completed_window_max,
                    count_expired: false,
                });
            }
        }
    }
    Ok(out)
}

fn read_nodes(paths: &RunPaths) -> taskfleet_core::Result<Vec<Node>> {
    let mut nodes = Vec::new();
    let entries = match std::fs::read_dir(paths.nodes_dir()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(nodes),
        Err(error) => {
            return Err(taskfleet_core::Error::Io {
                path: paths.nodes_dir(),
                source: error,
            })
        }
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect();
    files.sort();
    for file in files {
        let Some(stem) = file.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let Ok(node_id) = NodeId::parse_str(stem) else {
            continue;
        };
        if let Some(node) = taskfleet_core::read_node_opt(paths, &node_id)? {
            nodes.push(node);
        }
    }
    Ok(nodes)
}

fn mark_count_expiry(candidates: &mut [Candidate]) {
    let mut groups: BTreeMap<(String, String, u32, u64, String), Vec<usize>> = BTreeMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        groups
            .entry((
                candidate.display.socket.clone(),
                candidate.display.session.clone(),
                candidate.display.server_pid,
                candidate.display.server_pid_start_secs,
                candidate.display.server_marker.clone(),
            ))
            .or_default()
            .push(index);
    }
    for indices in groups.values_mut() {
        indices.sort_by_key(|index| std::cmp::Reverse(candidates[*index].display.retained_at));
        let keep = indices
            .iter()
            .map(|index| candidates[*index].max)
            .min()
            .unwrap_or(0) as usize;
        for index in indices.iter().skip(keep) {
            candidates[*index].count_expired = true;
        }
    }
}

#[derive(Debug)]
enum Expire {
    Expired,
    Missing,
    Preserved(String),
}

fn expire_candidate(candidate: &Candidate, deadline: Instant) -> Expire {
    let guard = match RunLock::try_acquire_existing(&candidate.paths.lock()) {
        Ok(Some(guard)) => guard,
        Ok(None) => return Expire::Preserved("run lock busy".into()),
        Err(error) => return Expire::Preserved(format!("run lock unavailable: {error}")),
    };
    let lock = guard.witness();
    let operation = (|| -> taskfleet_core::Result<Expire> {
        let Some(node) = taskfleet_core::read_node_opt(&candidate.paths, &candidate.node_id)?
        else {
            return Ok(Expire::Preserved(
                "run state disappeared during scan".into(),
            ));
        };
        if node.retained_display.as_deref() != Some(&candidate.display) {
            return Ok(Expire::Preserved("record changed during scan".into()));
        }
        match inspect_exact_display(&candidate.display, deadline) {
            Inspect::ServerGone => {
                record_expired_locked(&lock, candidate)?;
                Ok(Expire::Missing)
            }
            Inspect::Unsafe(reason) => Ok(Expire::Preserved(reason)),
            Inspect::Owned => {
                match window_ids_until(
                    &crate::multiplexer::tmux::tmux_bin(),
                    &candidate.display.socket,
                    &candidate.display.session,
                    deadline,
                ) {
                    Ok(windows) if windows.len() <= 1 => {
                        return Ok(Expire::Preserved(
                            "last retained pane keeps persistent session alive".into(),
                        ));
                    }
                    Ok(_) => {}
                    Err(reason) => return Ok(Expire::Preserved(reason)),
                }
                if !tmux_ok_until(
                    Some(&candidate.display.socket),
                    &["kill-pane", "-t", &candidate.display.pane_id],
                    deadline,
                ) {
                    return Ok(Expire::Preserved("tmux refused exact pane removal".into()));
                }
                record_expired_locked(&lock, candidate)?;
                Ok(Expire::Expired)
            }
            Inspect::WindowGone => {
                record_expired_locked(&lock, candidate)?;
                Ok(Expire::Expired)
            }
        }
    })();
    drop(guard);
    operation.unwrap_or_else(|error| {
        Expire::Preserved(format!("maintenance state update failed: {error}"))
    })
}

fn record_expired_locked(
    lock: &taskfleet_core::LockedRun<'_>,
    candidate: &Candidate,
) -> taskfleet_core::Result<()> {
    let key = format!(
        "worker.display.expired:{}:{}:{}",
        candidate.paths.run_id, candidate.node_id, candidate.display.attempt
    );
    append_and_apply_unlocked(
        lock,
        &candidate.paths,
        "worker.display.expired",
        Some(&candidate.node_id),
        Some(&key),
        json!({"attempt":candidate.display.attempt,"ownership_marker":candidate.display.ownership_marker}),
    )?;
    Ok(())
}

enum Inspect {
    Owned,
    WindowGone,
    ServerGone,
    Unsafe(String),
}

fn inspect_exact_display(display: &RetainedDisplay, deadline: Instant) -> Inspect {
    match server_identity(
        &crate::multiplexer::tmux::tmux_bin(),
        &display.socket,
        &display.session,
        display.server_pid,
        display.server_pid_start_secs,
        &display.server_marker,
        Some(deadline),
    ) {
        IdentityCheck::Gone => return Inspect::ServerGone,
        IdentityCheck::Unsafe(reason) => return Inspect::Unsafe(reason),
        IdentityCheck::Exact => {}
    }
    let windows = match window_ids_until(
        &crate::multiplexer::tmux::tmux_bin(),
        &display.socket,
        &display.session,
        deadline,
    ) {
        Ok(windows) => windows,
        Err(reason) => return Inspect::Unsafe(reason),
    };
    if !windows.iter().any(|window| window == &display.window_id) {
        return Inspect::WindowGone;
    }
    let format = format!(
        "#{{socket_path}}\t#{{session_name}}\t#{{window_id}}\t#{{pane_id}}\t#{{pane_dead}}\t#{{{OWNER_OPTION}}}\t#{{pid}}\t#{{HOMEBASE_TMUX_OWNER}}"
    );
    let text = match tmux_exec_until(
        &crate::multiplexer::tmux::tmux_bin(),
        Some(&display.socket),
        &["list-panes", "-t", &display.window_id, "-F", &format],
        deadline,
    ) {
        TmuxExec::Success(text) => text,
        failure => return Inspect::Unsafe(failure.reason("inspect retained panes")),
    };
    let rows: Vec<&str> = text.lines().collect();
    if rows.len() != 1 {
        return Inspect::Unsafe("retained window no longer has exactly one pane".into());
    }
    let fields: Vec<&str> = rows[0].split('\t').collect();
    if fields.len() != 8 {
        return Inspect::Unsafe("malformed tmux identity response".into());
    }
    if fields[0] != display.socket
        || fields[1] != display.session
        || fields[2] != display.window_id
        || fields[3] != display.pane_id
        || fields[4] != "1"
        || fields[5] != display.ownership_marker
        || fields[6].parse::<u32>().ok() != Some(display.server_pid)
        || fields[7] != display.server_marker
    {
        return Inspect::Unsafe("socket/server/marker/window/pane ownership drift".into());
    }
    Inspect::Owned
}

#[derive(Debug)]
pub(crate) enum Retention {
    Retained,
    NotApplicable,
    Unavailable,
    Retry,
}

/// Replace a completed Pi worker pane with one inert archived display. Every
/// archive/script/new-window prerequisite is complete before the old pane is
/// touched. `Retry` vetoes later worktree teardown; non-Pi workers remain on
/// the historical immediate-cleanup path.
pub(crate) fn retain_completed_display(paths: &RunPaths, node: &Node, tmux: &str) -> Retention {
    let Ok(Some(manifest)) = taskfleet_core::read_manifest_opt(paths) else {
        return Retention::Retry;
    };
    let Some(policy) = manifest.tmux_retention else {
        return Retention::NotApplicable;
    };
    let Some(evidence) = node.evidence.as_ref() else {
        return Retention::NotApplicable;
    };
    if evidence.status != EvidenceStatus::Complete {
        return Retention::Retry;
    }
    if node.retained_display.is_some() {
        return Retention::Retained;
    }
    if node.retention_unavailable.is_some() {
        return Retention::Unavailable;
    }
    let Some(identity) = node.tmux_identity.as_ref() else {
        return Retention::Retry;
    };
    let (Some(socket), Some(pane_id), Some(server_pid), Some(server_start)) = (
        identity.socket.as_deref(),
        identity.pane_id.as_deref(),
        identity.server_pid,
        identity.server_pid_start_secs,
    ) else {
        return Retention::Retry;
    };
    let expected_server_marker = identity.server_marker.as_deref().unwrap_or("");
    match server_identity(
        tmux,
        socket,
        &identity.session,
        server_pid,
        server_start,
        expected_server_marker,
        None,
    ) {
        IdentityCheck::Exact => {}
        IdentityCheck::Unsafe(_) => return Retention::Retry,
        IdentityCheck::Gone => {
            return record_unavailable(paths, node, "recorded tmux server generation is gone")
        }
    }

    // Validate every filesystem prerequisite before changing either tmux
    // window. The archive was produced and hash-checked by evidence cleanup.
    let Some(source_repo) = manifest
        .source_repo
        .as_deref()
        .filter(|path| Path::new(path).is_dir())
    else {
        return Retention::Retry;
    };
    let Some(pane_path) = evidence.pane_path.as_deref() else {
        return Retention::Retry;
    };
    let archive = paths.root.join(pane_path);
    if !archive.is_file() {
        return Retention::Retry;
    }
    let marker = ownership_marker(paths, node);
    // Bind the Workmux-created companion while the worker still proves the
    // original window's ownership. The marker survives removal/crash retries.
    mark_original_shell(
        tmux,
        identity,
        pane_id,
        node.worktree_path.as_deref(),
        &marker,
    );
    let retained_at = node.updated_at;
    let Some(expires_at) = expiry_at(retained_at, policy.completed_window_ttl_secs) else {
        return Retention::Retry;
    };
    if let Some(mut existing) = find_owned_display(tmux, identity, &marker, retained_at) {
        existing.expires_at = expires_at;
        if !dispose_original_pane(tmux, identity, pane_id) {
            return Retention::Retry;
        }
        return if record_retained(paths, node, existing) {
            Retention::Retained
        } else {
            Retention::Retry
        };
    }
    match owned_marker_count(tmux, identity, &marker) {
        Ok(0) => {}
        Ok(1) => {
            let Some(mut existing) = wait_owned_display(tmux, identity, &marker, retained_at)
            else {
                return Retention::Retry;
            };
            existing.expires_at = expires_at;
            if !dispose_original_pane(tmux, identity, pane_id) {
                return Retention::Retry;
            }
            return if record_retained(paths, node, existing) {
                Retention::Retained
            } else {
                Retention::Retry
            };
        }
        Ok(_) | Err(_) => return Retention::Retry,
    }

    let script = paths
        .root
        .join("evidence")
        .join(node.node_id.as_str())
        .join("retained-display.sh");
    // The replacement marks its own exact window before producing output.
    // Thus a supervisor crash cannot interrupt marker installation, and a
    // failure before remain-on-exit causes tmux to remove the unretained pane.
    let body = format!(
        "#!/bin/sh\nset -eu\n{} -S {} set-window-option -t \"$TMUX_PANE\" {} {}\n{} -S {} set-window-option -t \"$TMUX_PANE\" remain-on-exit on\ncat -- {}\n",
        shell_quote(tmux),
        shell_quote(socket),
        OWNER_OPTION,
        shell_quote(&marker),
        shell_quote(tmux),
        shell_quote(socket),
        shell_quote(&archive.to_string_lossy())
    );
    if std::fs::write(&script, body).is_err() {
        return Retention::Retry;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).is_err() {
            return Retention::Retry;
        }
    }

    let repo = Path::new(source_repo)
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("repo");
    let short = &paths.run_id.as_str()[paths.run_id.as_str().len().saturating_sub(10)..];
    let name = format!("{}-{}-complete", sanitize_name(repo), short);
    let target = exact_session_target(&identity.session);
    let Some(created) = tmux_text_bin(
        tmux,
        Some(socket),
        &[
            "new-window",
            "-d",
            "-t",
            &target,
            "-c",
            source_repo,
            "-n",
            &name,
            "-P",
            "-F",
            "#{window_id}\t#{pane_id}",
            script.to_string_lossy().as_ref(),
        ],
    ) else {
        return Retention::Retry;
    };
    let values: Vec<&str> = created.trim().split('\t').collect();
    let (Some(window_id), Some(new_pane)) = (
        values
            .first()
            .copied()
            .filter(|value| value.starts_with('@')),
        values
            .get(1)
            .copied()
            .filter(|value| value.starts_with('%')),
    ) else {
        return Retention::Retry;
    };

    let Some(mut display) = wait_owned_display(tmux, identity, &marker, retained_at) else {
        return Retention::Retry;
    };
    if display.window_id != window_id || display.pane_id != new_pane {
        return Retention::Retry;
    }
    display.expires_at = expires_at;
    if !dispose_original_pane(tmux, identity, pane_id) {
        return Retention::Retry;
    }
    if record_retained(paths, node, display) {
        Retention::Retained
    } else {
        // The script-owned tmux marker makes the next supervisor tick recover
        // this exact window with the original terminal timestamp and TTL.
        Retention::Retry
    }
}

/// Workmux's configured second pane is a default shell (empty start command).
/// The marker is installed only while both the recorded worker and exactly one
/// companion are present. Never adopt an unmarked lone pane by name or cwd.
fn mark_original_shell(
    tmux: &str,
    identity: &taskfleet_core::TmuxIdentity,
    worker: &str,
    worktree: Option<&str>,
    marker: &str,
) {
    let (Some(socket), Some(worktree)) = (identity.socket.as_deref(), worktree) else {
        return;
    };
    let (Some(pid), Some(start)) = (identity.server_pid, identity.server_pid_start_secs) else {
        return;
    };
    if !matches!(
        server_identity(
            tmux,
            socket,
            &identity.session,
            pid,
            start,
            identity.server_marker.as_deref().unwrap_or(""),
            None
        ),
        IdentityCheck::Exact
    ) {
        return;
    }
    if !window_ids(tmux, socket, &identity.session)
        .is_ok_and(|ids| ids.contains(&identity.window_id))
    {
        return;
    }
    let Some(rows) = original_rows(tmux, socket, &identity.window_id) else {
        return;
    };
    if rows.len() != 2 || !rows.iter().any(|r| r.pane == worker) {
        return;
    }
    let Some(shell) = rows.iter().find(|r| r.pane != worker) else {
        return;
    };
    if !idle_original_shell(shell, worktree) || (!shell.marker.is_empty() && shell.marker != marker)
    {
        return;
    }
    if shell.marker.is_empty() {
        let Some(raw) = tmux_text_bin(
            tmux,
            Some(socket),
            &["capture-pane", "-p", "-t", &shell.pane],
        ) else {
            return;
        };
        // A shell process alone does not prove it is at its prompt: typed
        // input (including a pending builtin) can still show `bash` as the
        // current command. Bind only an unambiguous ordinary prompt.
        if !raw
            .lines()
            .rfind(|line| !line.trim().is_empty())
            .is_some_and(|line| {
                matches!(line.trim_end().chars().last(), Some('$' | '#' | '%' | '>'))
            })
        {
            return;
        }
        let Some(screen) = shell_screen(tmux, socket, &shell.pane) else {
            return;
        };
        // Hash the complete visible pane at binding time: pending input and
        // shell output after binding invalidate the idle-shell proof.
        if !tmux_ok_bin(
            tmux,
            Some(socket),
            &[
                "set-option",
                "-p",
                "-t",
                &shell.pane,
                ORIGINAL_SCREEN_OPTION,
                &screen,
            ],
        ) {
            return;
        }
        let _ = tmux_ok_bin(
            tmux,
            Some(socket),
            &[
                "set-option",
                "-p",
                "-t",
                &shell.pane,
                ORIGINAL_SHELL_OPTION,
                marker,
            ],
        );
    }
}

struct OriginalRow {
    pane: String,
    path: String,
    start: String,
    current: String,
    marker: String,
    history: String,
    mode: String,
    dead: String,
    screen: String,
}

fn original_rows(tmux: &str, socket: &str, window: &str) -> Option<Vec<OriginalRow>> {
    let format = format!("#{{pane_id}}\t#{{pane_current_path}}\t#{{pane_start_command}}\t#{{pane_current_command}}\t#{{{ORIGINAL_SHELL_OPTION}}}\t#{{history_size}}\t#{{pane_in_mode}}\t#{{pane_dead}}\t#{{{ORIGINAL_SCREEN_OPTION}}}");
    let text = tmux_text_bin(
        tmux,
        Some(socket),
        &["list-panes", "-t", window, "-F", &format],
    )?;
    text.lines()
        .map(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            (fields.len() == 9).then(|| OriginalRow {
                pane: fields[0].into(),
                path: fields[1].into(),
                start: fields[2].into(),
                current: fields[3].into(),
                marker: fields[4].into(),
                history: fields[5].into(),
                mode: fields[6].into(),
                dead: fields[7].into(),
                screen: fields[8].into(),
            })
        })
        .collect()
}

fn shell_screen(tmux: &str, socket: &str, pane: &str) -> Option<String> {
    use sha2::Digest as _;
    let text = tmux_text_bin(tmux, Some(socket), &["capture-pane", "-p", "-t", pane])?;
    // A split collapsing changes blank rows and right-padding, not shell
    // content. Ignore that layout-only noise while retaining every nonblank
    // character (including pending input).
    let visible = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!("{:x}", sha2::Sha256::digest(visible.as_bytes())))
}

fn idle_original_shell(row: &OriginalRow, worktree: &str) -> bool {
    row.path == worktree
        && row.start.is_empty()
        && row.dead == "0"
        && row.mode == "0"
        && row.history == "0"
        && matches!(row.current.as_str(), "bash" | "zsh" | "sh" | "fish")
}

/// Close only a marked, still-idle Workmux companion after the worker pane is
/// gone. A shared/modified window is never killed; no path/name fallback.
pub(crate) fn close_original_workmux_window(paths: &RunPaths, node: &Node, tmux: &str) {
    let Some(identity) = node.tmux_identity.as_ref() else {
        return;
    };
    let (Some(socket), Some(worktree), Some(pid), Some(start), Some(worker)) = (
        identity.socket.as_deref(),
        node.worktree_path.as_deref(),
        identity.server_pid,
        identity.server_pid_start_secs,
        identity.pane_id.as_deref(),
    ) else {
        return;
    };
    mark_original_shell(
        tmux,
        identity,
        worker,
        Some(worktree),
        &ownership_marker(paths, node),
    );
    if !matches!(
        server_identity(
            tmux,
            socket,
            &identity.session,
            pid,
            start,
            identity.server_marker.as_deref().unwrap_or(""),
            None
        ),
        IdentityCheck::Exact
    ) {
        return;
    }
    if !window_ids(tmux, socket, &identity.session)
        .is_ok_and(|ids| ids.contains(&identity.window_id))
    {
        return;
    }
    let Some(rows) = original_rows(tmux, socket, &identity.window_id) else {
        return;
    };
    if rows.len() != 1 {
        return;
    }
    let shell = &rows[0];
    if shell.pane == worker
        || shell.marker != ownership_marker(paths, node)
        || !idle_original_shell(shell, worktree)
        || shell.screen.is_empty()
        || shell_screen(tmux, socket, &shell.pane).as_deref() != Some(&shell.screen)
    {
        return;
    }
    // Recheck immediately before the exact window kill; tmux does not offer a
    // conditional kill, so a concurrent pane mutation remains a small race.
    if !original_rows(tmux, socket, &identity.window_id).is_some_and(|again| {
        again.len() == 1
            && again[0].pane == shell.pane
            && again[0].marker == shell.marker
            && again[0].screen == shell.screen
            && idle_original_shell(&again[0], worktree)
            && shell_screen(tmux, socket, &again[0].pane).as_deref() == Some(&shell.screen)
    }) {
        return;
    }
    if !matches!(
        server_identity(
            tmux,
            socket,
            &identity.session,
            pid,
            start,
            identity.server_marker.as_deref().unwrap_or(""),
            None
        ),
        IdentityCheck::Exact
    ) {
        return;
    }
    let _ = tmux_ok_bin(
        tmux,
        Some(socket),
        &["kill-window", "-t", &identity.window_id],
    );
}

/// Non-Pi persistent runs have no retained display. Remove the exact worker
/// pane, then close only a proven Workmux companion; never kill shared windows.
pub(crate) fn close_persistent_non_pi(paths: &RunPaths, node: &Node, tmux: &str) {
    let Some(identity) = node.tmux_identity.as_ref() else {
        return;
    };
    let (Some(socket), Some(worker)) = (identity.socket.as_deref(), identity.pane_id.as_deref())
    else {
        return;
    };
    let marker = ownership_marker(paths, node);
    mark_original_shell(
        tmux,
        identity,
        worker,
        node.worktree_path.as_deref(),
        &marker,
    );
    let Some(rows) = original_rows(tmux, socket, &identity.window_id) else {
        return;
    };
    // An unrecognized extra pane is user-owned: leave the entire window alone.
    if rows.len() > 2
        || (rows.len() == 2
            && !rows.iter().any(|r| {
                r.pane != worker
                    && r.marker == marker
                    && node
                        .worktree_path
                        .as_deref()
                        .is_some_and(|path| idle_original_shell(r, path))
            }))
    {
        return;
    }
    if rows.iter().any(|r| r.pane == worker) && !dispose_original_pane(tmux, identity, worker) {
        return;
    }
    close_original_workmux_window(paths, node, tmux);
}

fn expiry_at(retained_at: chrono::DateTime<Utc>, ttl_secs: u64) -> Option<chrono::DateTime<Utc>> {
    let seconds = i64::try_from(ttl_secs).ok()?;
    retained_at.checked_add_signed(chrono::Duration::seconds(seconds))
}

fn dispose_original_pane(
    tmux: &str,
    identity: &taskfleet_core::TmuxIdentity,
    pane_id: &str,
) -> bool {
    match inspect_original_pane(tmux, identity, pane_id) {
        OriginalPane::Gone => true,
        OriginalPane::Exact => tmux_ok_bin(
            tmux,
            identity.socket.as_deref(),
            &["kill-pane", "-t", pane_id],
        ),
        OriginalPane::Drift | OriginalPane::Unsafe => false,
    }
}

fn record_unavailable(paths: &RunPaths, node: &Node, reason: &str) -> Retention {
    let key = format!(
        "worker.display.unavailable:{}:{}:{}",
        paths.run_id, node.node_id, node.retry_attempts
    );
    if append_and_apply_event(
        paths,
        "worker.display.unavailable",
        Some(&node.node_id),
        Some(&key),
        json!({"attempt":node.retry_attempts,"reason":reason}),
    )
    .is_ok()
    {
        Retention::Unavailable
    } else {
        Retention::Retry
    }
}

fn record_retained(paths: &RunPaths, node: &Node, display: RetainedDisplay) -> bool {
    let key = format!(
        "worker.display.retained:{}:{}:{}",
        paths.run_id, node.node_id, display.attempt
    );
    append_and_apply_event(
        paths,
        "worker.display.retained",
        Some(&node.node_id),
        Some(&key),
        serde_json::to_value(display).expect("display serializes"),
    )
    .is_ok()
}

#[derive(Debug)]
enum OriginalPane {
    Exact,
    Gone,
    Drift,
    Unsafe,
}

fn inspect_original_pane(
    tmux: &str,
    identity: &taskfleet_core::TmuxIdentity,
    pane: &str,
) -> OriginalPane {
    let (Some(socket), Some(server_pid), Some(server_start)) = (
        identity.socket.as_deref(),
        identity.server_pid,
        identity.server_pid_start_secs,
    ) else {
        return OriginalPane::Unsafe;
    };
    match server_identity(
        tmux,
        socket,
        &identity.session,
        server_pid,
        server_start,
        identity.server_marker.as_deref().unwrap_or(""),
        None,
    ) {
        IdentityCheck::Gone => return OriginalPane::Gone,
        IdentityCheck::Unsafe(_) => return OriginalPane::Unsafe,
        IdentityCheck::Exact => {}
    }
    let windows = match window_ids(tmux, socket, &identity.session) {
        Ok(windows) => windows,
        Err(_) => return OriginalPane::Unsafe,
    };
    if !windows.iter().any(|window| window == &identity.window_id) {
        return OriginalPane::Gone;
    }
    let format =
        "#{socket_path}\t#{session_name}\t#{window_id}\t#{pane_id}\t#{pid}\t#{HOMEBASE_TMUX_OWNER}";
    let text = match tmux_exec(
        tmux,
        Some(socket),
        &["list-panes", "-t", &identity.window_id, "-F", format],
    ) {
        TmuxExec::Success(text) => text,
        _ => return OriginalPane::Unsafe,
    };
    let Some(row) = text
        .lines()
        .find(|row| row.split('\t').nth(3) == Some(pane))
    else {
        return OriginalPane::Gone;
    };
    let fields: Vec<&str> = row.split('\t').collect();
    if fields.len() == 6
        && fields[0] == socket
        && fields[1] == identity.session
        && fields[2] == identity.window_id
        && fields[3] == pane
        && fields[4].parse::<u32>().ok() == Some(server_pid)
        && fields[5] == identity.server_marker.as_deref().unwrap_or("")
    {
        OriginalPane::Exact
    } else {
        OriginalPane::Drift
    }
}

fn ownership_marker(paths: &RunPaths, node: &Node) -> String {
    format!(
        "taskfleet:{}:{}:{}",
        paths.run_id, node.node_id, node.retry_attempts
    )
}

enum IdentityCheck {
    Exact,
    Gone,
    Unsafe(String),
}

fn server_identity(
    tmux: &str,
    socket: &str,
    session: &str,
    pid: u32,
    start: u64,
    marker: &str,
    deadline: Option<Instant>,
) -> IdentityCheck {
    if crate::supervise::watchdog::pid_start_time(pid) != Some(start) {
        return IdentityCheck::Gone;
    }
    let format = "#{socket_path}\t#{session_name}\t#{pid}\t#{HOMEBASE_TMUX_OWNER}";
    let result = deadline.map_or_else(
        || tmux_exec(tmux, Some(socket), &["list-sessions", "-F", format]),
        |until| tmux_exec_until(tmux, Some(socket), &["list-sessions", "-F", format], until),
    );
    let text = match result {
        TmuxExec::Success(text) => text,
        failure => return IdentityCheck::Unsafe(failure.reason("inspect tmux server")),
    };
    let mut saw_session = false;
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 4 {
            return IdentityCheck::Unsafe("malformed tmux server identity response".into());
        }
        if fields[1] == session {
            saw_session = true;
            if fields[0] != socket
                || fields[2].parse::<u32>().ok() != Some(pid)
                || fields[3] != marker
            {
                return IdentityCheck::Unsafe("tmux server/session identity drift".into());
            }
        }
    }
    if saw_session {
        IdentityCheck::Exact
    } else {
        IdentityCheck::Gone
    }
}

fn exact_session_target(session: &str) -> String {
    format!("={session}")
}

fn window_ids(tmux: &str, socket: &str, session: &str) -> Result<Vec<String>, String> {
    window_ids_with(tmux, socket, session, None)
}

fn window_ids_until(
    tmux: &str,
    socket: &str,
    session: &str,
    deadline: Instant,
) -> Result<Vec<String>, String> {
    window_ids_with(tmux, socket, session, Some(deadline))
}

fn window_ids_with(
    tmux: &str,
    socket: &str,
    session: &str,
    deadline: Option<Instant>,
) -> Result<Vec<String>, String> {
    let target = exact_session_target(session);
    let args = ["list-windows", "-t", &target, "-F", "#{window_id}"];
    let result = deadline.map_or_else(
        || tmux_exec(tmux, Some(socket), &args),
        |until| tmux_exec_until(tmux, Some(socket), &args, until),
    );
    match result {
        TmuxExec::Success(text) => Ok(text.lines().map(str::to_string).collect()),
        failure => Err(failure.reason("inspect tmux windows")),
    }
}

fn owned_marker_count(
    tmux: &str,
    identity: &taskfleet_core::TmuxIdentity,
    marker: &str,
) -> Result<usize, String> {
    let socket = identity
        .socket
        .as_deref()
        .ok_or_else(|| "recorded socket absent".to_string())?;
    let target = exact_session_target(&identity.session);
    let format = format!("#{{{OWNER_OPTION}}}");
    match tmux_exec(
        tmux,
        Some(socket),
        &["list-panes", "-s", "-t", &target, "-F", &format],
    ) {
        TmuxExec::Success(text) => Ok(text.lines().filter(|line| *line == marker).count()),
        failure => Err(failure.reason("scan retained ownership markers")),
    }
}

fn wait_owned_display(
    tmux: &str,
    identity: &taskfleet_core::TmuxIdentity,
    marker: &str,
    retained_at: chrono::DateTime<Utc>,
) -> Option<RetainedDisplay> {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Some(display) = find_owned_display(tmux, identity, marker, retained_at) {
            return Some(display);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

fn find_owned_display(
    tmux: &str,
    identity: &taskfleet_core::TmuxIdentity,
    marker: &str,
    retained_at: chrono::DateTime<Utc>,
) -> Option<RetainedDisplay> {
    let socket = identity.socket.as_deref()?;
    let server_pid = identity.server_pid?;
    let server_start = identity.server_pid_start_secs?;
    let format = format!("#{{window_id}}\t#{{pane_id}}\t#{{pane_dead}}\t#{{{OWNER_OPTION}}}");
    let target = exact_session_target(&identity.session);
    let text = tmux_text_bin(
        tmux,
        Some(socket),
        &["list-panes", "-s", "-t", &target, "-F", &format],
    )?;
    let matches: Vec<Vec<&str>> = text
        .lines()
        .map(|line| line.split('\t').collect())
        .filter(|fields: &Vec<&str>| fields.len() == 4 && fields[3] == marker)
        .collect();
    if matches.len() != 1 || matches[0][2] != "1" {
        return None;
    }
    // Window options appear on every pane. Require the marked window itself to
    // contain exactly this one dead pane before adopting it.
    let panes = tmux_text_bin(
        tmux,
        Some(socket),
        &["list-panes", "-t", matches[0][0], "-F", "#{pane_id}"],
    )?;
    if panes.lines().count() != 1 {
        return None;
    }
    Some(RetainedDisplay {
        attempt: marker.rsplit(':').next()?.parse().ok()?,
        socket: socket.into(),
        session: identity.session.clone(),
        window_id: matches[0][0].into(),
        pane_id: matches[0][1].into(),
        server_pid,
        server_pid_start_secs: server_start,
        server_marker: identity.server_marker.clone().unwrap_or_default(),
        ownership_marker: marker.into(),
        retained_at,
        expires_at: retained_at,
        expired_at: None,
    })
}

fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .take(24)
        .collect()
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn tmux_ok_bin(bin: &str, socket: Option<&str>, args: &[&str]) -> bool {
    matches!(tmux_exec(bin, socket, args), TmuxExec::Success(_))
}
fn tmux_ok_until(socket: Option<&str>, args: &[&str], deadline: Instant) -> bool {
    matches!(
        tmux_exec_until(
            &crate::multiplexer::tmux::tmux_bin(),
            socket,
            args,
            deadline
        ),
        TmuxExec::Success(_)
    )
}
#[cfg(test)]
fn tmux_text(socket: Option<&str>, args: &[&str]) -> Option<String> {
    tmux_text_bin(&crate::multiplexer::tmux::tmux_bin(), socket, args)
}
fn tmux_text_bin(bin: &str, socket: Option<&str>, args: &[&str]) -> Option<String> {
    match tmux_exec(bin, socket, args) {
        TmuxExec::Success(text) => Some(text),
        _ => None,
    }
}

enum TmuxExec {
    Success(String),
    Failed(String),
    TimedOut,
    SpawnFailed(String),
}

impl TmuxExec {
    fn reason(&self, operation: &str) -> String {
        match self {
            Self::Success(_) => format!("{operation}: unexpected successful result"),
            Self::Failed(stderr) => format!("{operation} failed: {stderr}"),
            Self::TimedOut => format!("{operation} timed out"),
            Self::SpawnFailed(error) => format!("{operation} could not start tmux: {error}"),
        }
    }
}

fn tmux_exec(bin: &str, socket: Option<&str>, args: &[&str]) -> TmuxExec {
    tmux_exec_for(bin, socket, args, TMUX_TIMEOUT)
}

fn tmux_exec_until(bin: &str, socket: Option<&str>, args: &[&str], deadline: Instant) -> TmuxExec {
    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
        return TmuxExec::TimedOut;
    };
    tmux_exec_for(bin, socket, args, remaining.min(TMUX_TIMEOUT))
}

fn tmux_exec_for(bin: &str, socket: Option<&str>, args: &[&str], timeout: Duration) -> TmuxExec {
    if timeout.is_zero() {
        return TmuxExec::TimedOut;
    }
    let mut command = Command::new(bin);
    if let Some(socket) = socket {
        command.args(["-S", socket]);
    }
    command.args(args);
    match run_with_timeout(command, timeout, OUTPUT_CAP) {
        TimedOutcome::Exited {
            status,
            stdout,
            stderr,
        } => {
            if status.success() {
                TmuxExec::Success(String::from_utf8_lossy(&stdout.bytes).into_owned())
            } else {
                TmuxExec::Failed(String::from_utf8_lossy(&stderr.bytes).trim().to_string())
            }
        }
        TimedOutcome::TimedOut => TmuxExec::TimedOut,
        TimedOutcome::SpawnErr(error) => TmuxExec::SpawnFailed(error.to_string()),
    }
}

fn emit_maintenance(
    payload: MaintainPayload,
    spec: &OutputSpec,
    warnings: &[String],
) -> Result<(), CliError> {
    match spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => output::emit_envelope(&payload, spec, warnings),
        OutputFormat::Text => {
            println!("examined: {}", payload.examined);
            println!("eligible: {}", payload.eligible);
            println!("expired: {}", payload.expired);
            println!("missing: {}", payload.missing);
            println!("preserved: {}", payload.preserved);
            println!("timed_out: {}", payload.timed_out);
            for row in payload.results {
                println!(
                    "{} {} {}: {}",
                    row.run_id, row.node_id, row.action, row.reason
                );
            }
            output::emit_text_warnings(warnings);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    fn tmux_available() -> bool {
        Command::new("tmux")
            .arg("-V")
            .output()
            .is_ok_and(|out| out.status.success())
    }

    struct PrivateTmuxServer(std::path::PathBuf);

    impl Drop for PrivateTmuxServer {
        fn drop(&mut self) {
            let _ = Command::new("tmux")
                .arg("-S")
                .arg(&self.0)
                .arg("kill-server")
                .status();
        }
    }

    /// Owns the deliberately stopped fixture child across assertions. SIGKILL is
    /// effective even while stopped, unlike a pending HUP/TERM from tmux teardown.
    struct StoppedChildGuard(Option<u32>);

    impl Drop for StoppedChildGuard {
        fn drop(&mut self) {
            if let Some(pid) = self.0 {
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGKILL);
                }
            }
        }
    }

    fn process_is_stopped(pid: u32) -> bool {
        Command::new("ps")
            .args(["-o", "state=", "-p", &pid.to_string()])
            .output()
            .is_ok_and(|out| {
                out.status.success()
                    && String::from_utf8_lossy(&out.stdout)
                        .trim_start()
                        .starts_with('T')
            })
    }

    #[test]
    fn real_private_tmux_expiry_preserves_unrelated_split_and_is_idempotent() {
        let _env_lock = crate::harness::support::test_env::lock();
        if !tmux_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("tmux.sock");
        // Arm cleanup before the first command that can start the private server.
        let _server = PrivateTmuxServer(socket.clone());
        let socket_s = socket.to_str().unwrap();
        let run_id = RunId::parse_str("01jxsnap000000000000000000").unwrap();
        let run_dir = temp.path().join("state/runs").join(run_id.as_str());
        std::fs::create_dir_all(&run_dir).unwrap();
        let paths = RunPaths::from_validated(&run_dir, run_id).unwrap();
        append_and_apply_event(&paths, "run.created", None, None, json!({
            "kind":"spinoff", "lifecycle":"autonomous", "title":"retention-test",
            "source_repo": temp.path(), "managed_tmux_session":"agents",
            "tmux_retention":{"persistent":true,"completed_window_ttl_secs":1,"completed_window_max":20}
        })).unwrap();
        let node_id = NodeId::parse_str("n-0001").unwrap();
        append_and_apply_event(
            &paths,
            "node.created",
            Some(&node_id),
            None,
            json!({"kind":"spinoff","attempt":0}),
        )
        .unwrap();

        assert!(Command::new("tmux")
            .env_remove("HOMEBASE_TMUX_OWNER")
            .args([
                "-f",
                "/dev/null",
                "-S",
                socket_s,
                "new-session",
                "-d",
                "-s",
                "agents",
                "-n",
                "sentinel"
            ])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("tmux")
            .args([
                "-S",
                socket_s,
                "split-window",
                "-d",
                "-t",
                "agents:sentinel",
                "sleep 60"
            ])
            .status()
            .unwrap()
            .success());
        let script = temp.path().join("dead.sh");
        let stopped_pid_path = temp.path().join("dead.pid");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n/bin/sh -c 'kill -STOP $$; echo archived' &\nchild=$!\nprintf '%s\\n' \"$child\" > '{pid}.tmp'\nmv '{pid}.tmp' '{pid}'\nwait \"$child\"\n",
                pid = stopped_pid_path.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let made = Command::new("tmux")
            .args([
                "-S",
                socket_s,
                "new-window",
                "-d",
                "-t",
                "agents",
                "-n",
                "done",
                "-P",
                "-F",
                "#{window_id}\t#{pane_id}\t#{pane_pid}",
                script.to_str().unwrap(),
            ])
            .output()
            .unwrap();

        // `new-window` returning does not prove the fixture child reached
        // SIGSTOP. The pane script stays alive while its child is stopped,
        // atomically publishes that child's pid, and waits for it. Acquire exact
        // child ownership before parsing tmux output or making later assertions.
        let stopped_deadline = Instant::now() + Duration::from_secs(5);
        let mut stopped_child = StoppedChildGuard(None);
        let stopped_pid = loop {
            let pid = std::fs::read_to_string(&stopped_pid_path)
                .ok()
                .and_then(|raw| raw.trim().parse::<u32>().ok());
            if let Some(pid) = pid {
                stopped_child.0 = Some(pid);
                if process_is_stopped(pid) {
                    break pid;
                }
            }
            assert!(
                Instant::now() < stopped_deadline,
                "retained fixture script never published a stopped pid"
            );
            std::thread::sleep(Duration::from_millis(20));
        };

        let made = String::from_utf8(made.stdout).unwrap();
        let fields: Vec<&str> = made.trim().split('\t').collect();
        let (window, pane) = (fields[0], fields[1]);
        let marker = format!("taskfleet:{}:{}:0", paths.run_id, node_id);
        assert!(Command::new("tmux")
            .args([
                "-S",
                socket_s,
                "set-window-option",
                "-t",
                window,
                "remain-on-exit",
                "on"
            ])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("tmux")
            .args([
                "-S",
                socket_s,
                "set-window-option",
                "-t",
                window,
                OWNER_OPTION,
                &marker
            ])
            .status()
            .unwrap()
            .success());
        let continued = unsafe { libc::kill(stopped_pid as libc::pid_t, libc::SIGCONT) };
        assert_eq!(continued, 0, "failed to continue retained fixture child");
        let wait_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if tmux_text(
                Some(socket_s),
                &["display-message", "-p", "-t", pane, "#{pane_dead}"],
            )
            .is_some_and(|value| value.trim() == "1")
            {
                break;
            }
            assert!(
                Instant::now() < wait_deadline,
                "retained test pane did not exit"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        // pane_dead proves the wrapper's `wait "$child"` completed.
        stopped_child.0 = None;
        let server = tmux_text(
            Some(socket_s),
            &["display-message", "-p", "-t", "agents", "#{pid}"],
        )
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
        let display = RetainedDisplay {
            attempt: 0,
            socket: socket_s.into(),
            session: "agents".into(),
            window_id: window.into(),
            pane_id: pane.into(),
            server_pid: server,
            server_pid_start_secs: crate::supervise::watchdog::pid_start_time(server).unwrap(),
            server_marker: String::new(),
            ownership_marker: marker,
            retained_at: Utc::now() - chrono::Duration::hours(2),
            expires_at: Utc::now() - chrono::Duration::hours(1),
            expired_at: None,
        };
        append_and_apply_event(
            &paths,
            "worker.display.retained",
            Some(&node_id),
            None,
            serde_json::to_value(&display).unwrap(),
        )
        .unwrap();
        let candidate = Candidate {
            paths: paths.clone(),
            node_id: node_id.clone(),
            display,
            max: 20,
            count_expired: false,
        };
        let expired = expire_candidate(&candidate, Instant::now() + Duration::from_secs(10));
        assert!(matches!(expired, Expire::Expired), "{expired:?}");
        assert!(matches!(
            expire_candidate(&candidate, Instant::now() + Duration::from_secs(10)),
            Expire::Preserved(_)
        ));
        let panes = tmux_text(
            Some(socket_s),
            &["list-panes", "-t", "agents:sentinel", "-F", "#{pane_id}"],
        )
        .unwrap();
        assert_eq!(panes.lines().count(), 2, "unrelated split survives");
        assert!(
            tmux_text(Some(socket_s), &["has-session", "-t", "agents"]).is_some(),
            "persistent session survives"
        );
        let archived = std::fs::read(paths.root.join("events.jsonl")).unwrap();
        assert!(!archived.is_empty(), "durable run archive is untouched");
    }

    #[test]
    fn real_retention_archives_before_killing_only_owned_pane_and_keeps_active_worker() {
        let _env_lock = crate::harness::support::test_env::lock();
        if !tmux_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("tmux.sock");
        let socket_s = socket.to_str().unwrap();
        assert!(Command::new("tmux")
            .env_remove("HOMEBASE_TMUX_OWNER")
            .args([
                "-f",
                "/dev/null",
                "-S",
                socket_s,
                "new-session",
                "-d",
                "-s",
                "agents",
                "-n",
                "worker",
                "sleep 60",
            ])
            .status()
            .unwrap()
            .success());
        let original = tmux_text(
            Some(socket_s),
            &[
                "display-message",
                "-p",
                "-t",
                "agents:worker",
                "#{window_id}\t#{pane_id}\t#{pane_pid}\t#{pid}",
            ],
        )
        .unwrap();
        let fields: Vec<&str> = original.trim().split('\t').collect();
        let (original_window, original_pane) = (fields[0], fields[1]);
        let original_pid = fields[2].parse::<u32>().unwrap();
        let server_pid = fields[3].parse::<u32>().unwrap();
        assert!(Command::new("tmux")
            .args([
                "-S",
                socket_s,
                "split-window",
                "-d",
                "-t",
                original_window,
                "sleep 60",
            ])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("tmux")
            .args([
                "-S",
                socket_s,
                "new-window",
                "-d",
                "-t",
                "agents",
                "-n",
                "active",
                "sleep 60",
            ])
            .status()
            .unwrap()
            .success());

        let run_id = RunId::parse_str("01jxsnap111111111111111111").unwrap();
        let run_dir = temp.path().join("state/runs").join(run_id.as_str());
        std::fs::create_dir_all(&run_dir).unwrap();
        let paths = RunPaths::from_validated(&run_dir, run_id).unwrap();
        append_and_apply_event(
            &paths,
            "run.created",
            None,
            None,
            json!({
                "kind":"spinoff", "lifecycle":"autonomous", "title":"retention-create-test",
                "source_repo":temp.path(), "managed_tmux_session":"agents",
                "tmux_retention":{"persistent":true,"completed_window_ttl_secs":3600,"completed_window_max":20}
            }),
        )
        .unwrap();
        let node_id = NodeId::parse_str("n-0001").unwrap();
        let session_id = "11111111-1111-4111-8111-111111111111";
        append_and_apply_event(
            &paths,
            "node.created",
            Some(&node_id),
            None,
            json!({
                "kind":"spinoff", "attempt":0,
                "tmux_socket":socket_s, "tmux_session":"agents",
                "tmux_window_id":original_window, "tmux_pane_id":original_pane,
                "tmux_server_pid":server_pid,
                "tmux_server_pid_start_secs":crate::supervise::watchdog::pid_start_time(server_pid).unwrap(),
                "pi_session_id":session_id,
                "pi_session_path":format!(".creating/pi-sessions/{}/pi-session-{session_id}.jsonl", paths.run_id),
                "pi_session_cwd":temp.path(),
            }),
        )
        .unwrap();
        let evidence_dir = paths.root.join("evidence").join(node_id.as_str());
        std::fs::create_dir_all(&evidence_dir).unwrap();
        for (name, body) in [
            ("pi-session.original.jsonl", "original\n"),
            ("pi-session.resume.jsonl", "resume\n"),
            ("final-pane.log", "full archived pane\n"),
            ("terminal-report.json", "{}\n"),
        ] {
            std::fs::write(evidence_dir.join(name), body).unwrap();
        }
        append_and_apply_event(
            &paths,
            "worker.evidence.archived",
            Some(&node_id),
            None,
            json!({
                "attempt":0, "session_id":session_id,
                "transcript_path":format!("evidence/{}/pi-session.original.jsonl", node_id),
                "resume_path":format!("evidence/{}/pi-session.resume.jsonl", node_id),
                "pane_path":format!("evidence/{}/final-pane.log", node_id),
                "report_path":format!("evidence/{}/terminal-report.json", node_id),
                "transcript_sha256":"0".repeat(64),
            }),
        )
        .unwrap();
        let node = taskfleet_core::read_node_opt(&paths, &node_id)
            .unwrap()
            .unwrap();
        let retention = retain_completed_display(&paths, &node, "tmux");
        assert!(matches!(retention, Retention::Retained), "{retention:?}");
        let deadline = Instant::now() + Duration::from_secs(2);
        while crate::supervise::watchdog::pid_start_time(original_pid).is_some()
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(crate::supervise::watchdog::pid_start_time(original_pid).is_none());
        let original_panes = tmux_text(
            Some(socket_s),
            &["list-panes", "-t", original_window, "-F", "#{pane_id}"],
        )
        .unwrap();
        assert_eq!(original_panes.lines().count(), 1, "split sentinel survives");
        assert!(tmux_text(Some(socket_s), &["has-session", "-t", "agents:active"]).is_some());
        let node = taskfleet_core::read_node_opt(&paths, &node_id)
            .unwrap()
            .unwrap();
        let retained = node.retained_display.unwrap();
        assert!(matches!(
            inspect_exact_display(&retained, Instant::now() + Duration::from_secs(10)),
            Inspect::Owned
        ));
        assert_eq!(
            std::fs::read_to_string(evidence_dir.join("final-pane.log")).unwrap(),
            "full archived pane\n"
        );
        let _ = Command::new("tmux")
            .args(["-S", socket_s, "kill-server"])
            .status();
    }

    #[test]
    fn private_workmux_shell_closes_only_when_exact_and_idle() {
        let _env_lock = crate::harness::support::test_env::lock();
        if !tmux_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("socket");
        let _server = PrivateTmuxServer(socket.clone());
        let socket = socket.to_str().unwrap();
        let wt = temp.path().join("worktree");
        std::fs::create_dir(&wt).unwrap();
        let run_id = RunId::parse_str("01jxsnap333333333333333333").unwrap();
        let dir = temp.path().join("runs").join(run_id.as_str());
        std::fs::create_dir_all(&dir).unwrap();
        let paths = RunPaths::from_validated(&dir, run_id).unwrap();
        append_and_apply_event(
            &paths,
            "run.created",
            None,
            None,
            json!({
                "kind":"spinoff", "lifecycle":"autonomous", "title":"shell test"
            }),
        )
        .unwrap();
        let id = NodeId::parse_str("n-0001").unwrap();
        let made = Command::new("tmux")
            .env_remove("HOMEBASE_TMUX_OWNER")
            .args([
                "-f",
                "/dev/null",
                "-S",
                socket,
                "new-session",
                "-d",
                "-s",
                "agents",
                "-c",
                wt.to_str().unwrap(),
                "-n",
                "worker",
                "-P",
                "-F",
                "#{window_id}\t#{pane_id}\t#{pid}",
                "sleep 60",
            ])
            .output()
            .unwrap();
        assert!(made.status.success());
        let made = String::from_utf8(made.stdout).unwrap();
        let fields: Vec<_> = made.trim().split('\t').collect();
        let (window, worker, pid) = (fields[0], fields[1], fields[2].parse::<u32>().unwrap());
        assert!(Command::new("tmux")
            .args(["-S", socket, "set-option", "-g", "default-shell", "/bin/sh"])
            .status()
            .unwrap()
            .success());
        let split = Command::new("tmux")
            .args([
                "-S",
                socket,
                "split-window",
                "-d",
                "-c",
                wt.to_str().unwrap(),
                "-t",
                window,
            ])
            .output()
            .unwrap();
        assert!(
            split.status.success(),
            "{}",
            String::from_utf8_lossy(&split.stderr)
        );
        let identity = taskfleet_core::TmuxIdentity {
            socket: Some(socket.into()),
            session: "agents".into(),
            window_id: window.into(),
            pane_id: Some(worker.into()),
            server_pid: Some(pid),
            server_pid_start_secs: crate::supervise::watchdog::pid_start_time(pid),
            server_marker: Some(String::new()),
        };
        append_and_apply_event(
            &paths,
            "node.created",
            Some(&id),
            None,
            json!({
                "kind":"spinoff", "attempt":0, "worktree_path":wt,
                "tmux_socket":socket, "tmux_session":"agents", "tmux_window_id":window,
                "tmux_pane_id":worker, "tmux_server_pid":pid,
                "tmux_server_pid_start_secs":identity.server_pid_start_secs,
            }),
        )
        .unwrap();
        let node = taskfleet_core::read_node_opt(&paths, &id).unwrap().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !original_rows("tmux", socket, window)
            .is_some_and(|rows| rows.iter().any(|r| r.pane != worker && r.current == "sh"))
        {
            assert!(Instant::now() < deadline, "shell never became idle");
            std::thread::sleep(Duration::from_millis(20));
        }
        mark_original_shell(
            "tmux",
            &identity,
            worker,
            node.worktree_path.as_deref(),
            &ownership_marker(&paths, &node),
        );
        let rows = original_rows("tmux", socket, window).unwrap();
        assert!(
            rows.iter()
                .any(|r| r.pane != worker && r.marker == ownership_marker(&paths, &node)),
            "shell {:?}",
            rows.iter()
                .map(|r| (&r.pane, &r.start, &r.current, &r.history, &r.path, &r.screen))
                .collect::<Vec<_>>()
        );
        // Two-pane window is never killed while the worker is still there.
        close_original_workmux_window(&paths, &node, "tmux");
        assert!(window_ids("tmux", socket, "agents")
            .unwrap()
            .contains(&window.to_string()));
        assert!(tmux_text(
            Some(socket),
            &[
                "new-window",
                "-d",
                "-t",
                "agents",
                "-n",
                "sentinel",
                "sleep 60"
            ]
        )
        .is_some());
        assert!(
            dispose_original_pane("tmux", &identity, worker),
            "{:?} {:?}",
            inspect_original_pane("tmux", &identity, worker),
            original_rows("tmux", socket, window).map(|rows| rows
                .iter()
                .map(|r| (r.pane.clone(), r.path.clone(), r.marker.clone()))
                .collect::<Vec<_>>())
        );
        let rows = original_rows("tmux", socket, window).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].screen,
            shell_screen("tmux", socket, &rows[0].pane).unwrap(),
            "pane screen changed across split collapse"
        );
        close_original_workmux_window(&paths, &node, "tmux");
        close_original_workmux_window(&paths, &node, "tmux"); // crash retry
        assert!(!window_ids("tmux", socket, "agents")
            .unwrap_or_default()
            .contains(&window.to_string()));

        for changed in ["input", "extra-pane", "drift"] {
            let made = tmux_text(
                Some(socket),
                &[
                    "new-window",
                    "-d",
                    "-t",
                    "agents",
                    "-c",
                    wt.to_str().unwrap(),
                    "-P",
                    "-F",
                    "#{window_id}\t#{pane_id}",
                    "sleep 60",
                ],
            )
            .unwrap();
            let ids: Vec<_> = made.trim().split('\t').collect();
            let mut other = identity.clone();
            other.window_id = ids[0].into();
            other.pane_id = Some(ids[1].into());
            assert!(tmux_text(
                Some(socket),
                &[
                    "split-window",
                    "-d",
                    "-c",
                    wt.to_str().unwrap(),
                    "-t",
                    ids[0]
                ]
            )
            .is_some());
            let deadline = Instant::now() + Duration::from_secs(3);
            while !original_rows("tmux", socket, ids[0])
                .is_some_and(|rows| rows.iter().any(|r| r.pane != ids[1] && r.current == "sh"))
            {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(20));
            }
            mark_original_shell(
                "tmux",
                &other,
                ids[1],
                node.worktree_path.as_deref(),
                &ownership_marker(&paths, &node),
            );
            let shell = original_rows("tmux", socket, ids[0])
                .unwrap()
                .into_iter()
                .find(|r| r.pane != ids[1])
                .unwrap();
            assert_eq!(shell.marker, ownership_marker(&paths, &node));
            match changed {
                "input" => {
                    assert!(tmux_text(
                        Some(socket),
                        &["send-keys", "-t", &shell.pane, "echo user-input"]
                    )
                    .is_some());
                }
                "extra-pane" => {
                    assert!(tmux_text(
                        Some(socket),
                        &["split-window", "-d", "-t", ids[0], "sleep 60"]
                    )
                    .is_some());
                }
                _ => {
                    assert!(tmux_text(
                        Some(socket),
                        &[
                            "set-option",
                            "-p",
                            "-t",
                            &shell.pane,
                            ORIGINAL_SHELL_OPTION,
                            "someone-else"
                        ]
                    )
                    .is_some());
                }
            }
            assert!(dispose_original_pane("tmux", &other, ids[1]));
            close_original_workmux_window(&paths, &node, "tmux"); // recorded window differs
            let mut changed_node = node.clone();
            changed_node.tmux_identity = Some(Box::new(other));
            close_original_workmux_window(&paths, &changed_node, "tmux");
            assert!(
                window_ids("tmux", socket, "agents")
                    .unwrap()
                    .contains(&ids[0].to_string()),
                "{changed} pane was not preserved"
            );
        }
        let made = tmux_text(
            Some(socket),
            &[
                "new-window",
                "-d",
                "-t",
                "agents",
                "-c",
                wt.to_str().unwrap(),
                "-P",
                "-F",
                "#{window_id}\t#{pane_id}",
                "sleep 60",
            ],
        )
        .unwrap();
        let ids: Vec<_> = made.trim().split('\t').collect();
        let mut non_pi = node.clone();
        let mut non_pi_identity = identity.clone();
        non_pi_identity.window_id = ids[0].into();
        non_pi_identity.pane_id = Some(ids[1].into());
        non_pi.tmux_identity = Some(Box::new(non_pi_identity));
        assert!(tmux_text(
            Some(socket),
            &[
                "split-window",
                "-d",
                "-c",
                wt.to_str().unwrap(),
                "-t",
                ids[0]
            ]
        )
        .is_some());
        let deadline = Instant::now() + Duration::from_secs(3);
        while !original_rows("tmux", socket, ids[0])
            .is_some_and(|rows| rows.iter().any(|r| r.pane != ids[1] && r.current == "sh"))
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        close_persistent_non_pi(&paths, &non_pi, "tmux");
        close_persistent_non_pi(&paths, &non_pi, "tmux");
        assert!(!window_ids("tmux", socket, "agents")
            .unwrap()
            .contains(&ids[0].to_string()));
    }

    #[test]
    fn count_bound_marks_oldest_and_keeps_equal_time_order_deterministic() {
        let temp = tempfile::tempdir().unwrap();
        let retained_at = Utc::now();
        let mut candidates = (0..3)
            .map(|index| {
                let run_id =
                    RunId::parse_str(&format!("01jxsnap22222222222222222{index}")).unwrap();
                let run_dir = temp.path().join(run_id.as_str());
                std::fs::create_dir_all(&run_dir).unwrap();
                Candidate {
                    paths: RunPaths::from_validated(&run_dir, run_id).unwrap(),
                    node_id: NodeId::parse_str("n-0001").unwrap(),
                    display: RetainedDisplay {
                        attempt: 0,
                        socket: "/tmp/private.sock".into(),
                        session: "agents".into(),
                        window_id: format!("@{index}"),
                        pane_id: format!("%{index}"),
                        server_pid: 42,
                        server_pid_start_secs: 7,
                        server_marker: "owner".into(),
                        ownership_marker: format!("owned-{index}"),
                        retained_at,
                        expires_at: retained_at + chrono::Duration::hours(1),
                        expired_at: None,
                    },
                    max: 2,
                    count_expired: false,
                }
            })
            .collect::<Vec<_>>();
        mark_count_expiry(&mut candidates);
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.count_expired)
                .collect::<Vec<_>>(),
            [false, false, true]
        );
        mark_count_expiry(&mut candidates);
        assert_eq!(
            candidates
                .iter()
                .filter(|candidate| candidate.count_expired)
                .count(),
            1,
            "a second maintenance classification is idempotent"
        );
    }

    #[test]
    fn wrong_server_identity_and_split_retained_window_fail_closed() {
        let display = RetainedDisplay {
            attempt: 0,
            socket: "/missing/socket".into(),
            session: "agents".into(),
            window_id: "@1".into(),
            pane_id: "%1".into(),
            server_pid: u32::MAX,
            server_pid_start_secs: 1,
            server_marker: String::new(),
            ownership_marker: "owned".into(),
            retained_at: Utc::now(),
            expires_at: Utc::now(),
            expired_at: None,
        };
        assert!(matches!(
            inspect_exact_display(&display, Instant::now() + Duration::from_secs(10)),
            Inspect::ServerGone
        ));
    }
}
