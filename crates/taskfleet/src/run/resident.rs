//! Named resident runs (ADR 0003, `docs/decisions/0003-resident-workers.md`).
//!
//! A resident run is an ordinary single-node run whose `run.created` carries a
//! [`ResidentPolicy`]. Its worker works in told rounds: `node round-report`
//! records the end of a round, the worker process exits, and the next
//! `run message` resumes the same native session in the same worktree
//! (`run/resume.rs`). This module holds what the resident verbs share:
//!
//! - **Name resolution.** A name resolves only among the *current
//!   repository's* non-terminal runs, and within those first among the runs
//!   of the caller's own creator session (`TASKFLEET_CREATOR_SESSION`, the
//!   same create-time attribution `run.created` records; unattributed callers
//!   form one shared scope). Names are unique per (repository, creator
//!   session), so two conversations may each have a `reviewer`. An attributed
//!   caller never reaches another session's run by name; an unattributed
//!   caller falls back to all sessions as before. Ambiguity is refused with
//!   the full run ids. The scope is a namespace, not access control: any
//!   caller can still address any run by id. Names start with a lowercase
//!   letter and run-id prefixes with `0`-`7`, so the two selector forms never
//!   overlap and an id keeps resolving exactly as before.
//! - **The resident operation lock.** `run message` (resume), `run land` and the
//!   supervisor's idle expiry each act on the worktree while no worker runs.
//!   They serialize on an exclusive `flock` of `<run-dir>/resident.lock`, which
//!   the kernel releases if the holder dies; the run lock itself is only ever
//!   held for the short projection reads and appends.
//! - **The round marker** that tells the bundled Pi adapter to shut Pi down at
//!   the next settled boundary after a round report. Claude's `--print` process
//!   ends with its turn and needs no marker.
//! - **The read view** (`resident` on `run show` / `run list`).

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use fs4::{FileExt, TryLockError};
use serde::Serialize;
use taskfleet_core::schema::CreatorSession;
use taskfleet_core::{
    read_manifest_opt, Manifest, Node, ResidentPolicy, ResidentState, RunId, RunPaths,
};

use crate::error::CliError;
use crate::run::{run_paths_exact, runs_root, RunSelector};

/// Default idle expiry after an open round report: 24 hours (Jari, 2026-11-06).
pub const DEFAULT_IDLE_EXPIRY_SECS: u64 = 24 * 60 * 60;

/// Resolve a CLI run reference that may be a run id, an unambiguous run-id
/// prefix, or a resident name. Names resolve against the git repository that
/// contains the current directory.
pub(crate) fn resolve_run_ref(root: &Path, arg: &str) -> Result<RunPaths, CliError> {
    match RunSelector::parse(arg) {
        Ok(selector) => run_paths_exact(root, &selector.resolve(root)?),
        Err(id_error) => run_paths_exact(root, &resolve_name_or(root, arg, id_error)?),
    }
}

/// Resolve `arg` as a resident name. When it is not a name, or no live run
/// of this repository carries it, the original run-id error stands (a typo
/// keeps its `invalid_run_id` class); an ambiguous name, a name held only by
/// other sessions, and an invalid caller attribution are their own errors.
fn resolve_name_or(root: &Path, arg: &str, id_error: CliError) -> Result<RunId, CliError> {
    if ResidentPolicy::validate_name(arg).is_err() {
        return Err(id_error);
    }
    let cwd = std::env::current_dir()
        .map_err(|e| CliError::system("io_error", format!("read current directory: {e}")))?;
    let caller = crate::home::creator_session()?;
    match resolve_name(root, arg, &cwd, caller.as_ref()) {
        Ok(rid) => Ok(rid),
        Err(e) if e.code == "ambiguous_run_name" || e.code == "resident_name_other_session" => {
            Err(e)
        }
        Err(_) => {
            let mut error = id_error;
            error
                .message
                .push_str("; it is also not the name of a live resident run in this repository");
            Err(error)
        }
    }
}

/// For verbs that resolve ids themselves (`run show`, `run wait`, `run cancel`,
/// `run merge`): turn a resident name into its full run id and leave every
/// other argument untouched, so id and prefix handling (and their errors) are
/// exactly as before.
pub(crate) fn canonical_run_arg(arg: String) -> Result<String, CliError> {
    let id_error = match RunSelector::parse(&arg) {
        Ok(_) => return Ok(arg),
        Err(e) => e,
    };
    if ResidentPolicy::validate_name(&arg).is_err() {
        return Ok(arg);
    }
    let root = crate::home::root_dir()?;
    Ok(resolve_name_or(&root, &arg, id_error)?.to_string())
}

/// The git common directory identifying the repository that contains `path`.
fn repository_of(path: &Path) -> Result<Option<PathBuf>, CliError> {
    crate::run::list::repository_identity(path, false)
}

/// One live resident run carrying a name, with the creator session that
/// scopes it (`None` for an unattributed creator).
struct NamedRun {
    run_id: RunId,
    creator: Option<CreatorSession>,
}

/// Every non-terminal resident run named `name` whose recorded source
/// repository is the repository identified by `repo_identity`.
fn live_runs_named(
    root: &Path,
    name: &str,
    repo_identity: &Path,
) -> Result<Vec<NamedRun>, CliError> {
    let dir = runs_root(root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(CliError::system(
                "io_error",
                format!("read_dir {}: {e}", dir.display()),
            ))
        }
    };
    let mut matches = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| {
            CliError::system("io_error", format!("read_dir {}: {e}", dir.display()))
        })?;
        let Some(Ok(rid)) = entry.file_name().to_str().map(RunId::parse_str) else {
            continue;
        };
        let Ok(paths) = run_paths_exact(root, &rid) else {
            continue;
        };
        // One projection file: an atomic-rename read needs no shared lock.
        let Ok(Some(manifest)) = read_manifest_opt(&paths) else {
            continue;
        };
        if manifest.status.is_terminal()
            || manifest.resident.as_ref().map(|r| r.name.as_str()) != Some(name)
        {
            continue;
        }
        let Some(source) = manifest.source_repo.as_deref() else {
            continue;
        };
        if repository_of(Path::new(source))?.as_deref() == Some(repo_identity) {
            matches.push(NamedRun {
                run_id: rid,
                creator: manifest.created_by_session,
            });
        }
    }
    matches.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    Ok(matches)
}

/// `pi:<uuid>` for messages, or `unattributed`.
fn scope_label(scope: Option<&CreatorSession>) -> String {
    scope.map_or_else(
        || "unattributed".to_string(),
        |s| format!("{}:{}", s.harness, s.session_id),
    )
}

fn id_list(runs: &[NamedRun]) -> String {
    runs.iter()
        .map(|r| r.run_id.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn ids_value(runs: &[NamedRun]) -> serde_json::Value {
    serde_json::Value::Array(
        runs.iter()
            .map(|r| serde_json::Value::String(r.run_id.to_string()))
            .collect(),
    )
}

/// Resolve a resident name among the live runs of the repository containing
/// `cwd`, for a caller attributed to `caller` (`None`: unattributed).
///
/// The caller's own scope is searched first: one match resolves, several are
/// `ambiguous_run_name`. With no match there, an attributed caller is refused
/// with `resident_name_other_session` listing the other sessions' run ids (a
/// name must not reach another conversation's worker by accident), while an
/// unattributed caller (a terminal user) keeps the repository-wide lookup.
/// Zero matches anywhere is `run_not_found`. Every refusal names the full run
/// ids so the caller can address the intended run by id.
pub(crate) fn resolve_name(
    root: &Path,
    name: &str,
    cwd: &Path,
    caller: Option<&CreatorSession>,
) -> Result<RunId, CliError> {
    let repo = repository_of(cwd)?.ok_or_else(|| {
        CliError::user(
            "resident_name_needs_repository",
            format!(
                "resident name {name:?} resolves only inside a git repository; run the command \
                 from the run's repository or pass the full run id"
            ),
        )
        .with_invalid_value(name)
    })?;
    let all = live_runs_named(root, name, &repo)?;
    let (mut own, others): (Vec<NamedRun>, Vec<NamedRun>) =
        all.into_iter().partition(|r| r.creator.as_ref() == caller);
    let scope = scope_label(caller);
    let candidates = match (own.len(), caller) {
        (1, _) => return Ok(own.pop().expect("one match").run_id),
        (0, Some(_)) if !others.is_empty() => {
            return Err(CliError::user(
                "resident_name_other_session",
                format!(
                    "no live resident run named {name:?} was created by this session ({scope}) in \
                     this repository; {} run(s) of other sessions carry that name ({}). Address \
                     another session's run by its full run id",
                    others.len(),
                    id_list(&others)
                ),
            )
            .with_invalid_value(name)
            .with_expected(ids_value(&others)));
        }
        (0, Some(_)) => Vec::new(),
        (0, None) => others,
        _ => own,
    };
    match candidates.len() {
        0 => Err(CliError::user(
            "run_not_found",
            format!("no live resident run named {name:?} in this repository"),
        )
        .with_invalid_value(name)),
        1 => Ok(candidates.into_iter().next().expect("one match").run_id),
        n => Err(CliError::user(
            "ambiguous_run_name",
            format!(
                "{n} live runs in this repository are named {name:?} ({}); use a full run id",
                candidates
                    .iter()
                    .map(|r| format!("{} by {}", r.run_id, scope_label(r.creator.as_ref())))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
        .with_invalid_value(name)
        .with_expected(ids_value(&candidates))),
    }
}

/// Refuse a resident name that is already live in the repository `repo` for
/// the same creator session `creator` (both unattributed counts as the same
/// scope). Best-effort: a concurrent create can still race, and resolution
/// then refuses the ambiguity instead of guessing.
pub(crate) fn ensure_name_free(
    root: &Path,
    name: &str,
    repo: &Path,
    creator: Option<&CreatorSession>,
) -> Result<(), CliError> {
    let Some(identity) = repository_of(repo)? else {
        return Err(CliError::user(
            "resident_name_needs_repository",
            format!(
                "--resident requires a git source repository; {} is not one",
                repo.display()
            ),
        ));
    };
    let live = live_runs_named(root, name, &identity)?;
    if let Some(existing) = live.iter().find(|r| r.creator.as_ref() == creator) {
        return Err(CliError::user(
            "resident_name_in_use",
            format!(
                "a live resident run named {name:?} created by this session ({}) already exists \
                 in this repository ({}); message it, or finish it with `run merge` / \
                 `run cancel` first",
                scope_label(creator),
                existing.run_id
            ),
        )
        .with_invalid_value(name));
    }
    Ok(())
}

/// Exclusive resident operation lock (`<run-dir>/resident.lock`).
pub(crate) struct ResidentLock {
    _file: File,
}

impl ResidentLock {
    fn open(paths: &RunPaths) -> Result<File, CliError> {
        let path = paths.root.join("resident.lock");
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| CliError::system("io_error", format!("open {}: {e}", path.display())))
    }

    /// Take the lock without waiting; `None` when another operation holds it.
    pub(crate) fn try_acquire(paths: &RunPaths) -> Result<Option<Self>, CliError> {
        let file = Self::open(paths)?;
        match FileExt::try_lock(&file) {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => Err(CliError::system(
                "io_error",
                format!("lock resident operation: {e}"),
            )),
        }
    }

    /// Take the lock, waiting at most `wait`; `resident_busy` after that.
    pub(crate) fn acquire(paths: &RunPaths, wait: Duration) -> Result<Self, CliError> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(lock) = Self::try_acquire(paths)? {
                return Ok(lock);
            }
            if Instant::now() >= deadline {
                return Err(CliError::user(
                    "resident_busy",
                    format!(
                        "another resident operation (message, land, or idle expiry) is running on \
                         run {}; retry shortly",
                        paths.run_id
                    ),
                )
                .with_invalid_value(paths.run_id.to_string()));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Marker whose existence tells the bundled Pi adapter of `attempt` to shut
/// Pi down at its next settled boundary (the attempt reported its round).
pub(crate) fn round_marker_path(state_root: &Path, run_id: &str, attempt: u32) -> PathBuf {
    state_root
        .join(".creating")
        .join(format!("round-{run_id}-{attempt}.reported"))
}

/// Resident round state as the read surfaces name it.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RoundState {
    /// A round is in progress (no open round report).
    Working,
    /// The worker reported its round and waits for the next message.
    Waiting,
    /// Idle expiry removed the worktree of a wholly landed round; the next
    /// message recreates it and resumes the same session.
    Dormant,
}

#[must_use]
pub fn round_state(state: Option<&ResidentState>) -> RoundState {
    match state {
        Some(s) if s.dormant_since.is_some() => RoundState::Dormant,
        Some(s) if s.open_round().is_some() => RoundState::Waiting,
        _ => RoundState::Working,
    }
}

/// The latest round report as surfaced on `run show` / `run list`.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct RoundView {
    pub round: u32,
    pub attempt: u32,
    pub reported_at: DateTime<Utc>,
    pub summary: String,
    pub open: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<DateTime<Utc>>,
}

/// The latest landing as surfaced on `run show` / `run list`.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct LandingView {
    pub round: u32,
    pub at: DateTime<Utc>,
    pub source_branch: String,
    pub integrated_oid: String,
}

/// `resident` on `run show` / `run list` (ADR 0003). Present only for
/// resident runs.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct ResidentView {
    pub name: String,
    pub round_state: RoundState,
    pub rounds_reported: u32,
    /// Whether the current worker attempt has no recorded exit yet.
    pub worker_running: bool,
    pub idle_expiry_secs: u64,
    /// When idle expiry will act on the open round (worker exited only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_expires_at: Option<DateTime<Utc>>,
    /// Set when idle expiry found unlanded work and preserved it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiry_attention_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dormant_since: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_round: Option<RoundView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_landing: Option<LandingView>,
}

/// When idle expiry may act: the open round's report time plus the policy,
/// only once the reporting worker has exited (a running worker is never idle).
#[must_use]
pub fn idle_deadline(policy: &ResidentPolicy, node: &Node) -> Option<DateTime<Utc>> {
    let state = node.resident.as_deref()?;
    if state.dormant_since.is_some() || node.worker_exit.is_none() {
        return None;
    }
    let open = state.open_round()?;
    let secs = i64::try_from(policy.idle_expiry_secs).unwrap_or(i64::MAX);
    open.reported_at
        .checked_add_signed(chrono::Duration::seconds(secs))
}

/// Build the resident read view for a run, or `None` for an ordinary run.
#[must_use]
pub fn view(manifest: &Manifest, node: Option<&Node>) -> Option<ResidentView> {
    let policy = manifest.resident.as_ref()?;
    let state = node.and_then(|n| n.resident.as_deref());
    Some(ResidentView {
        name: policy.name.clone(),
        round_state: round_state(state),
        rounds_reported: state.map_or(0, |s| s.rounds_reported),
        worker_running: node.is_some_and(|n| n.worker_exit.is_none() && !n.status.is_terminal()),
        idle_expiry_secs: policy.idle_expiry_secs,
        idle_expires_at: node.and_then(|n| idle_deadline(policy, n)),
        expiry_attention_at: state.and_then(|s| s.expiry_attention_at),
        dormant_since: state.and_then(|s| s.dormant_since),
        last_round: state
            .and_then(|s| s.last_round.as_ref())
            .map(|r| RoundView {
                round: r.round,
                attempt: r.attempt,
                reported_at: r.reported_at,
                summary: r.summary.clone(),
                open: r.closed_at.is_none(),
                closed_at: r.closed_at,
            }),
        last_landing: state
            .and_then(|s| s.last_landing.as_ref())
            .map(|l| LandingView {
                round: l.round,
                at: l.at,
                source_branch: l.source_branch.clone(),
                integrated_oid: l.integrated_oid.clone(),
            }),
    })
}

/// Wait (bounded) for the told exit of a resident worker that reported its
/// round, and return the node once its current attempt has a recorded exit.
///
/// Refusals: a terminal node (`worker_not_active`), a round still in progress
/// with no open report (`worker_running`), and an open report whose exit did
/// not arrive within `wait` (`worker_still_running`, retryable). Never infers
/// the exit from a pid or a pane: only `worker.exited` counts.
pub(crate) fn await_worker_exit(
    paths: &RunPaths,
    node_id: &taskfleet_core::NodeId,
    wait: Duration,
) -> Result<(Manifest, Node), CliError> {
    let deadline = Instant::now() + wait;
    loop {
        let (manifest, node) = taskfleet_core::RunLock::with_shared_lock(&paths.lock(), || {
            Ok((
                read_manifest_opt(paths)?,
                taskfleet_core::read_node_opt(paths, node_id)?,
            ))
        })
        .map_err(crate::run::from_core)?;
        let manifest = manifest.ok_or_else(|| {
            CliError::user("run_not_found", format!("run {} not found", paths.run_id))
        })?;
        let node = node.ok_or_else(|| {
            CliError::user("node_not_found", format!("node {node_id} not found"))
                .with_invalid_value(node_id.as_str())
        })?;
        if manifest.status.is_terminal() || node.status.is_terminal() {
            return Err(CliError::user(
                "worker_not_active",
                format!("run {} is terminal", paths.run_id),
            )
            .with_invalid_value(paths.run_id.to_string()));
        }
        let reported = node
            .resident
            .as_deref()
            .and_then(ResidentState::open_round)
            .is_some_and(|r| r.attempt == node.retry_attempts);
        // The told exit is authoritative unless the recorded process is
        // positively live. After a told round report, a recorded worker that is
        // provably gone (its shim lost to a hard kill or a host restart) has
        // ended the round too; an unverifiable one is waited for.
        match crate::run::worker::classify_worker(&node) {
            crate::run::worker::WorkerState::Exited => return Ok((manifest, node)),
            crate::run::worker::WorkerState::Gone | crate::run::worker::WorkerState::NoPid
                if reported =>
            {
                return Ok((manifest, node));
            }
            _ => {}
        }
        if !reported {
            return Err(CliError::user(
                "worker_running",
                format!(
                    "run {} is in the middle of a round (no round report yet); wait for its round \
                     report with `taskfleet run wait {}`",
                    paths.run_id, paths.run_id
                ),
            )
            .with_invalid_value(paths.run_id.to_string()));
        }
        if Instant::now() >= deadline {
            return Err(CliError::user(
                "worker_still_running",
                format!(
                    "run {} reported its round but its process has not exited after {}s; retry \
                     shortly (inspect it with `taskfleet run attach {}`)",
                    paths.run_id,
                    wait.as_secs(),
                    paths.run_id
                ),
            )
            .with_invalid_value(paths.run_id.to_string()));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// `git` in `dir`, returning trimmed stdout on success.
pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new(crate::supervise::cleanup::git_bin())
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("git {args:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Whether a rebase is in progress in the worktree at `dir`.
pub(crate) fn rebase_in_progress(dir: &Path) -> bool {
    ["rebase-merge", "rebase-apply"].iter().any(|name| {
        git(
            dir,
            &["rev-parse", "--path-format=absolute", "--git-path", name],
        )
        .is_ok_and(|p| Path::new(&p).exists())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_never_collide_with_run_id_prefixes() {
        for name in ["proto-m365", "review-sharepoint", "a", "r2d2"] {
            assert!(ResidentPolicy::validate_name(name).is_ok(), "{name}");
            assert!(!taskfleet_core::is_run_id_prefix(name), "{name}");
        }
        for bad in [
            "",
            "7up",
            "-x",
            "x-",
            "a--b",
            "Proto",
            "a_b",
            &"a".repeat(49),
        ] {
            assert!(ResidentPolicy::validate_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn round_state_follows_told_facts_only() {
        assert_eq!(round_state(None), RoundState::Working);
        let mut s = ResidentState::default();
        assert_eq!(round_state(Some(&s)), RoundState::Working);
        s.last_round = Some(taskfleet_core::RoundReport {
            round: 1,
            attempt: 0,
            reported_at: Utc::now(),
            event_seq: 9,
            summary: "done".into(),
            closed_at: None,
            closed_by: None,
        });
        assert_eq!(round_state(Some(&s)), RoundState::Waiting);
        s.dormant_since = Some(Utc::now());
        assert_eq!(round_state(Some(&s)), RoundState::Dormant);
        s.dormant_since = None;
        s.last_round.as_mut().unwrap().closed_at = Some(Utc::now());
        assert_eq!(round_state(Some(&s)), RoundState::Working);
    }
}
