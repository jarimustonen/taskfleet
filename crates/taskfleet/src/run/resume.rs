//! Resume a resident worker for its next round (ADR 0003).
//!
//! `run message` lands here when the resident's current attempt has a told
//! exit (a round report followed by the exit, or the clean-exit attention
//! case). Under the resident operation lock it:
//!
//! 1. recreates the worktree at its recorded path from the source branch when
//!    idle expiry left the resident dormant (Claude resumes only in the same
//!    directory, and the Pi transcript names it too);
//! 2. rebases the worker branch onto the source branch tip so the worker sees
//!    decisions and other landings. A conflicted rebase is left in progress and
//!    named in the message preamble as the round's first task; a dirty tree or
//!    a checkout off its branch skips the rebase and says so;
//! 3. generates the next attempt's launcher from the recorded selection with
//!    the recorded native session (Pi `--session`, Claude `--resume`) and
//!    starts it in the worker's recorded pane or a new window of its session;
//! 4. appends `node.resumed` under the run lock after re-checking that the
//!    node is still exactly the exited attempt it read, which closes the open
//!    round with the message id.
//!
//! The message text never enters the event log: it is the new attempt's first
//! input. The audit is the same metadata pair `run message` always writes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use taskfleet_core::{
    append_and_apply_unlocked, read_node_opt, Manifest, Node, NodeId, RunLock, RunPaths,
};

use crate::error::CliError;
use crate::run::resident::{self, ResidentLock};
use crate::run::spawn::{self, LaunchOptions, ResumeSession};
use crate::run::{from_core, reattach};

/// What the pre-resume rebase did, reported to the caller and the worker.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum RebaseOutcome {
    /// The branch already contains the source tip.
    UpToDate { source_oid: String },
    /// The branch was rebased onto the source tip.
    Rebased { source_oid: String },
    /// The rebase stopped on conflicts; it is left in progress for the worker.
    Conflicts { files: Vec<String> },
    /// The rebase was not attempted.
    Skipped { reason: &'static str },
}

impl RebaseOutcome {
    fn preamble(&self, source: &str) -> String {
        match self {
            Self::UpToDate { .. } => format!("Your branch already contains `{source}`; no rebase was needed."),
            Self::Rebased { source_oid } => format!(
                "Taskfleet rebased your branch onto `{source}` ({}) before this round: decisions and \
                 other work landed meanwhile are now in your tree.",
                &source_oid[..source_oid.len().min(12)]
            ),
            Self::Conflicts { files } => format!(
                "Taskfleet's rebase of your branch onto `{source}` stopped on conflicts and is still \
                 in progress. Resolve it first, before anything else: fix {}, `git add` them, and run \
                 `git rebase --continue` (or `git rebase --abort` and explain why in your round \
                 summary).",
                if files.is_empty() {
                    "the conflicted files (`git status`)".to_string()
                } else {
                    files
                        .iter()
                        .map(|f| format!("`{f}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ),
            Self::Skipped { reason } => format!(
                "Taskfleet did not rebase your branch onto `{source}` ({reason}). Commit or resolve \
                 that first, then `git rebase {source}` yourself so you work on the current tip."
            ),
        }
    }
}

/// The result of a resume, folded into the `run message` receipt.
#[derive(Serialize, Debug, Clone)]
pub struct Resumed {
    pub attempt: u32,
    pub round: u32,
    pub rebase: RebaseOutcome,
    pub worktree_recreated: bool,
    pub agent_pid: i64,
    pub started_at: DateTime<Utc>,
}

fn git_err(code: &'static str) -> impl Fn(String) -> CliError {
    move |detail| CliError::system(code, detail)
}

/// Recreate a dormant resident's worktree at the recorded path.
fn recreate_worktree(
    repo: &Path,
    worktree: &Path,
    branch: &str,
    source: &str,
) -> Result<(), CliError> {
    let wt = worktree.to_str().ok_or_else(|| {
        CliError::system(
            "worktree_path_invalid",
            "recorded worktree path is not UTF-8",
        )
    })?;
    let _ = resident::git(repo, &["worktree", "prune"]);
    let branch_exists = resident::git(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok();
    if branch_exists {
        resident::git(repo, &["worktree", "add", "--", wt, branch])
    } else {
        resident::git(repo, &["worktree", "add", "-b", branch, "--", wt, source])
    }
    .map(|_| ())
    .map_err(git_err("worktree_recreate_failed"))
}

/// Rebase the worker branch onto the source tip, leaving conflicts in place.
fn rebase_onto_source(
    worktree: &Path,
    branch: &str,
    source: &str,
) -> Result<RebaseOutcome, CliError> {
    if resident::rebase_in_progress(worktree) {
        return Ok(RebaseOutcome::Conflicts {
            files: conflicted_files(worktree),
        });
    }
    if resident::git(worktree, &["symbolic-ref", "--short", "-q", "HEAD"]).as_deref() != Ok(branch)
    {
        return Ok(RebaseOutcome::Skipped {
            reason: "the worktree is not checked out on its branch",
        });
    }
    let dirty = resident::git(worktree, &["status", "--porcelain", "--untracked-files=no"])
        .map_err(git_err("git_status_failed"))?;
    if !dirty.is_empty() {
        return Ok(RebaseOutcome::Skipped {
            reason: "uncommitted changes in the worktree",
        });
    }
    let source_oid = resident::git(
        worktree,
        &["rev-parse", "--verify", &format!("{source}^{{commit}}")],
    )
    .map_err(git_err("base_ref_not_found"))?;
    if resident::git(
        worktree,
        &["merge-base", "--is-ancestor", &source_oid, "HEAD"],
    )
    .is_ok()
    {
        return Ok(RebaseOutcome::UpToDate { source_oid });
    }
    match resident::git(worktree, &["rebase", &source_oid]) {
        Ok(_) => Ok(RebaseOutcome::Rebased { source_oid }),
        Err(detail) if resident::rebase_in_progress(worktree) => {
            tracing::info!(target: "taskfleet::resume", %detail, "pre-resume rebase stopped on conflicts");
            Ok(RebaseOutcome::Conflicts {
                files: conflicted_files(worktree),
            })
        }
        Err(detail) => Err(CliError::system("rebase_failed", detail)),
    }
}

fn conflicted_files(worktree: &Path) -> Vec<String> {
    resident::git(worktree, &["diff", "--name-only", "--diff-filter=U"])
        .map(|out| out.lines().map(str::to_string).take(50).collect())
        .unwrap_or_default()
}

fn message_input(run_id: &str, round: u32, rebase: &str, text: &str) -> String {
    format!(
        "# Taskfleet round {round}: message relayed by the parent agent\n\n\
         {rebase}\n\n\
         End this round as before: commit the result, run \
         `taskfleet node round-report {run_id} --summary-file <file>`, and stop.\n\n\
         ---\n\n{text}\n"
    )
}

/// The recorded native session of the exited attempt.
fn recorded_session(
    harness: &str,
    state_root: &Path,
    node: &Node,
) -> Result<ResumeSession, CliError> {
    match harness {
        "pi" => {
            let evidence = node.evidence.as_ref().ok_or_else(|| {
                CliError::user(
                    "resume_session_unknown",
                    "this Pi worker has no recorded native session to resume",
                )
            })?;
            let path = state_root.join(&evidence.live_session_path);
            if !path.is_file() {
                return Err(CliError::user(
                    "resume_session_unknown",
                    format!("recorded Pi transcript {} is missing", path.display()),
                ));
            }
            Ok(ResumeSession::Pi {
                session_id: evidence.session_id.clone(),
                path,
            })
        }
        "claude" => node
            .claude_session_id
            .clone()
            .map(|session_id| ResumeSession::Claude { session_id })
            .ok_or_else(|| {
                CliError::user(
                    "resume_session_unknown",
                    "this Claude worker has no recorded session id (created before resident support)",
                )
            }),
        other => Err(CliError::user(
            "message_unsupported",
            format!("harness {other:?} cannot resume a resident round"),
        )),
    }
}

/// Resume the exited resident worker of `node_id` with `text` as its first
/// input. Returns the message id and what happened.
pub(crate) fn resume(
    root: &Path,
    paths: &RunPaths,
    node_id: &NodeId,
    text: &str,
    wait_exit: Duration,
) -> Result<(String, u64, Resumed), CliError> {
    resident::await_worker_exit(paths, node_id, wait_exit)?;
    let _lock = ResidentLock::acquire(paths, Duration::from_secs(30))?;
    let (manifest, node) = resident::await_worker_exit(paths, node_id, Duration::ZERO)?;
    crate::run::require_writer_fence(&manifest)?;
    let reported = node
        .resident
        .as_deref()
        .and_then(taskfleet_core::ResidentState::open_round)
        .is_some_and(|r| r.attempt == node.retry_attempts);
    if !reported
        && node
            .worker_exit
            .is_some_and(taskfleet_core::WorkerExit::is_failure)
    {
        // A failing exit without a round report is the typed `failed` outcome
        // (ADR 0001); the supervisor settles it and the branch is preserved.
        return Err(CliError::user(
            "worker_failed",
            format!(
                "the worker of run {} exited with a failure before reporting its round; it is                  settled as failed with its work preserved (see `taskfleet run show {}`)",
                paths.run_id, paths.run_id
            ),
        )
        .with_invalid_value(paths.run_id.to_string()));
    }
    let state_root = root;
    let selection = recorded_selection(&manifest);
    let harness = selection.selected.harness.clone();
    let session = recorded_session(&harness, state_root, &node)?;

    let repo = PathBuf::from(manifest.source_repo.clone().ok_or_else(|| {
        CliError::user(
            "no_source_repo",
            "resident run has no recorded source repository",
        )
    })?);
    let source = manifest.source_branch.clone().ok_or_else(|| {
        CliError::user(
            "no_source_branch",
            "resident run has no recorded source branch",
        )
    })?;
    let branch = node
        .branch
        .clone()
        .filter(|b| !b.is_empty())
        .ok_or_else(|| CliError::user("no_branch", format!("node {node_id} has no branch")))?;
    let worktree = PathBuf::from(node.worktree_path.clone().ok_or_else(|| {
        CliError::user(
            "no_worktree",
            format!("node {node_id} has no worktree path"),
        )
    })?);
    let tmux_session = manifest
        .managed_tmux_session
        .clone()
        .or_else(|| node.tmux_identity.as_ref().map(|i| i.session.clone()))
        .ok_or_else(|| {
            CliError::user(
                "tmux_session_unknown",
                "resident run has no recorded tmux session to resume into",
            )
        })?;

    let worktree_recreated = !worktree.exists();
    if worktree_recreated {
        recreate_worktree(&repo, &worktree, &branch, &source)?;
    }
    let rebase = rebase_onto_source(&worktree, &branch, &source)?;
    let base_sha = match &rebase {
        RebaseOutcome::UpToDate { source_oid } | RebaseOutcome::Rebased { source_oid } => {
            Some(source_oid.clone())
        }
        _ => None,
    };

    let round = node
        .resident
        .as_deref()
        .map_or(0, |s| s.rounds_reported)
        .saturating_add(1);
    let input = message_input(
        paths.run_id.as_str(),
        round,
        &rebase.preamble(&source),
        text,
    );
    let attempt = node.retry_attempts.saturating_add(1);
    let marker = (harness == "pi")
        .then(|| resident::round_marker_path(state_root, paths.run_id.as_str(), attempt));
    if let Some(marker) = &marker {
        let _ = std::fs::remove_file(marker);
    }
    let launcher = spawn::write_agent_launcher_with(
        &paths.root,
        state_root,
        &selection,
        paths.run_id.as_str(),
        node_id.as_str(),
        attempt,
        &LaunchOptions {
            resume: Some(session),
            round_marker: marker,
        },
    )?;

    let message_id = uuid::Uuid::new_v4().to_string();
    let attempt_seq = {
        let guard = RunLock::acquire_existing(&paths.lock()).map_err(from_core)?;
        append_and_apply_unlocked(
            &guard.witness(),
            paths,
            "worker.message.attempted",
            Some(node_id),
            Some(&message_id),
            json!({
                "message_id": message_id,
                "sender": std::env::var("USER").unwrap_or_else(|_| "unknown".into()),
                "sender_uid": unsafe { libc::geteuid() },
                "adapter": "resident-resume-v1",
                "bytes": text.len(),
                "attempt": attempt,
            }),
        )
        .map_err(from_core)?
    };
    let record_result = |delivery: &str| {
        if let Ok(guard) = RunLock::acquire_existing(&paths.lock()) {
            let _ = append_and_apply_unlocked(
                &guard.witness(),
                paths,
                "worker.message.result",
                Some(node_id),
                Some(&message_id),
                json!({"message_id": message_id, "attempt_seq": attempt_seq,
                    "delivery": delivery, "read_or_acted": "unconfirmed"}),
            );
        }
    };

    let window_name = node
        .tmux_window
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| branch.clone());
    let outcome = spawn::materialize_resume(&spawn::ResumeRequest {
        launcher: &launcher,
        worktree: &worktree,
        message: &input,
        recorded: node.tmux_identity.as_deref(),
        window_name: &window_name,
        session: &tmux_session,
        cwd: &repo,
        agent_startup_timeout: 90,
        require_server_identity: manifest.tmux_retention.is_some(),
    });
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            record_result("failed");
            return Err(error.with_details(json!({
                "message_id": message_id, "delivery": "failed", "attempt_seq": attempt_seq,
            })));
        }
    };

    let started_at = Utc::now();
    let data = json!({
        "attempt": attempt,
        "message_id": message_id,
        "base_sha": base_sha,
        "worktree_recreated": worktree_recreated,
        "rebase": rebase,
        "tmux_window": outcome.tmux_window,
        "tmux_socket": outcome.tmux_socket,
        "tmux_session": outcome.tmux_session,
        "tmux_window_id": outcome.tmux_window_id,
        "tmux_pane_id": outcome.tmux_pane_id,
        "tmux_server_pid": outcome.tmux_server_pid,
        "tmux_server_pid_start_secs": outcome.tmux_server_pid_start_secs,
        "tmux_server_marker": outcome.tmux_server_marker,
        "pi_session_id": outcome.pi_session_id,
        "pi_session_path": outcome.pi_session_path,
        "pi_session_cwd": outcome.pi_session_cwd,
        "claude_session_id": launcher.claude_session_id(),
        "agent_pid": outcome.agent_pid_hint,
        "agent_pid_start_time": DateTime::<Utc>::from_timestamp(
            i64::try_from(outcome.agent_start_time).unwrap_or(i64::MAX),
            0,
        ),
        "agent_pid_start_identity": outcome.agent_start_identity,
    });
    let appended = (|| -> Result<(), CliError> {
        let guard = RunLock::acquire_existing(&paths.lock()).map_err(from_core)?;
        // The resident lock excludes other resumes; re-check that nothing else
        // (a cancel or a merge) settled the node while the worker started.
        let current = read_node_opt(paths, node_id).map_err(from_core)?;
        if !current.is_some_and(|n| !n.status.is_terminal() && n.retry_attempts + 1 == attempt) {
            return Err(CliError::user(
                "worker_not_active",
                format!("node {node_id} changed while the resumed worker was starting"),
            ));
        }
        append_and_apply_unlocked(
            &guard.witness(),
            paths,
            "node.resumed",
            Some(node_id),
            None,
            data,
        )
        .map(|_| ())
        .map_err(from_core)
    })();
    if let Err(error) = appended {
        // An unrecorded worker must not keep running: stop exactly the
        // handshaken process (identity re-verified) and report the failure.
        if spawn::verify_agent_pid(
            outcome.agent_pid_hint,
            outcome.agent_start_time,
            &outcome.agent_start_identity,
        )
        .is_ok()
        {
            unsafe { libc::kill(outcome.agent_pid_hint as libc::pid_t, libc::SIGTERM) };
        }
        record_result("failed");
        return Err(error);
    }
    record_result("resumed");
    if harness == "pi" {
        let _ = std::fs::remove_file(resident::round_marker_path(
            state_root,
            paths.run_id.as_str(),
            node.retry_attempts,
        ));
    }
    ensure_supervisor(paths);
    Ok((
        message_id,
        attempt_seq,
        Resumed {
            attempt,
            round,
            rebase,
            worktree_recreated,
            agent_pid: outcome.agent_pid_hint,
            started_at,
        },
    ))
}

/// The recorded launch selection, or the historical built-in for old runs.
fn recorded_selection(manifest: &Manifest) -> taskfleet_core::AgentSelection {
    manifest.agent_selection.clone().unwrap_or_else(|| {
        spawn::builtin_agent_selection(
            manifest
                .harness
                .as_deref()
                .unwrap_or(crate::harness::DEFAULT_HARNESS),
            true,
        )
    })
}

/// A resumed worker needs a live supervisor to adjudicate its exit. A host
/// restart can have ended the supervisor of a waiting resident; restart it.
fn ensure_supervisor(paths: &RunPaths) {
    if crate::run::dto::SupervisorView::probe(paths).alive {
        return;
    }
    if let Err(error) = reattach::spawn_supervisor(paths, paths.run_id.as_str(), false, None) {
        if error.code != "supervisor_already_running" {
            tracing::warn!(
                target: "taskfleet::resume",
                error = %error.message,
                "resumed resident worker has no supervisor; run `taskfleet run reattach`"
            );
        }
    }
}
