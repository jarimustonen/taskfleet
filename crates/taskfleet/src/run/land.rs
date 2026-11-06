//! `run land` — land a resident worker's committed rounds without ending the
//! run (ADR 0003).
//!
//! The mechanics are exactly `run merge`'s: the worker branch is rebased onto
//! the source branch tip and the source is fast-forwarded under the shared Git
//! merge lock, guarded by a compare-and-swap on the recorded source OID. The
//! transaction is recorded first (`merge.started` with `landing: true`), so a
//! crashed land is resolved by the same OID-based recovery — except that its
//! completion is the non-terminal `node.round_landed`, never an
//! `explicit-merge` report. The worktree, branch and native session stay.
//!
//! A land runs only while no worker process exists (the current attempt has a
//! recorded `worker.exited`), so nobody writes to the worktree during the
//! rebase, and under the resident operation lock, so a concurrent resume or
//! idle expiry cannot act on the same worktree. A rebase conflict is aborted,
//! leaving the worktree exactly as the worker left it; the next `run message`
//! rebases again and hands the conflict to the worker as its first task.
//! The supervisor never lands: landing is the parent's explicit act.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use taskfleet_core::{append_and_apply_event, NodeId};

use crate::error::CliError;
use crate::output::{self, OutputFormat, OutputSpec};
use crate::run::merge::{abort_merge_start, merge_failed, record_merge_start, run_git_merge};
use crate::run::resident::{self, ResidentLock};
use crate::run::{merge_recovery, parse_node_id};
use crate::supervise::cleanup::git_bin;

pub struct Args<'a> {
    pub run: String,
    pub node: String,
    pub wait_exit: Duration,
    pub dry_run: bool,
    pub spec: &'a OutputSpec,
    pub warnings: &'a [String],
}

#[derive(Serialize)]
struct Payload<'a> {
    run_id: &'a str,
    node_id: &'a str,
    resident_name: &'a str,
    branch: &'a str,
    source: &'a str,
    /// Whether this call moved the source branch.
    landed: bool,
    /// Why nothing was landed, when `landed` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
    /// Commits on the worker branch not yet on the source (dry run / landed).
    commits: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    integrated_oid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_seq: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dry_run: Option<bool>,
}

pub fn run(args: Args<'_>) -> Result<(), CliError> {
    let root = crate::home::root_dir()?;
    let paths = resident::resolve_run_ref(&root, &args.run)?;
    let node_id = parse_node_id(&args.node)?;
    let (manifest, _) = resident::await_worker_exit(&paths, &node_id, args.wait_exit)?;
    let policy = manifest.resident.clone().ok_or_else(|| {
        CliError::user(
            "not_resident",
            format!(
                "run {} is not a resident run; use `run merge` to merge and finish it",
                paths.run_id
            ),
        )
        .with_invalid_value(paths.run_id.to_string())
    })?;
    crate::run::require_writer_fence(&manifest)?;
    let source = manifest.source_branch.clone().ok_or_else(|| {
        CliError::user(
            "no_source_branch",
            format!(
                "run {} has no recorded source branch to land into",
                paths.run_id
            ),
        )
    })?;

    let _lock = ResidentLock::acquire(&paths, Duration::from_secs(30))?;
    // Re-read under the resident lock: a resume that finished before we took it
    // has started a new attempt, and the worker is running again.
    let (_, node) = resident::await_worker_exit(&paths, &node_id, Duration::ZERO)?;
    let run_id = paths.run_id.to_string();
    let branch = node
        .branch
        .clone()
        .filter(|b| !b.is_empty())
        .ok_or_else(|| CliError::user("no_branch", format!("node {node_id} has no branch")))?;
    let emit = |landed: bool,
                reason: Option<&'static str>,
                commits: usize,
                integrated_oid: Option<String>,
                event_seq: Option<u64>| {
        let payload = Payload {
            run_id: &run_id,
            node_id: node_id.as_str(),
            resident_name: &policy.name,
            branch: &branch,
            source: &source,
            landed,
            reason,
            commits,
            integrated_oid,
            event_seq,
            dry_run: args.dry_run.then_some(true),
        };
        match args.spec.format {
            OutputFormat::Text => {
                if landed {
                    println!(
                        "landed {commits} commit(s) of {} onto {source} ({})",
                        policy.name,
                        payload.integrated_oid.as_deref().unwrap_or("?")
                    );
                } else {
                    println!(
                        "nothing landed for {} ({})",
                        policy.name,
                        reason.unwrap_or(if args.dry_run { "dry run" } else { "no change" })
                    );
                }
                output::emit_text_warnings(args.warnings);
                Ok(())
            }
            _ => output::emit_envelope(&payload, args.spec, args.warnings),
        }
    };

    if node
        .resident
        .as_deref()
        .is_some_and(|s| s.dormant_since.is_some())
    {
        // Idle expiry removed the worktree only after proving everything landed.
        return emit(false, Some("dormant"), 0, None, None);
    }
    let worktree = node
        .worktree_path
        .clone()
        .ok_or_else(|| CliError::user("no_worktree", format!("node {node_id} has no worktree")))?;
    let wt = Path::new(&worktree);
    if !wt.is_dir() {
        return Err(CliError::user(
            "worktree_missing",
            format!("worktree {worktree} does not exist; nothing can be landed from it"),
        ));
    }
    if resident::rebase_in_progress(wt) {
        return Err(CliError::user(
            "rebase_in_progress",
            format!(
                "worktree {worktree} has a rebase in progress; message the worker so it resolves \
                 it as the first task of its next round, then land"
            ),
        ));
    }
    let dirty = resident::git(wt, &["status", "--porcelain", "--untracked-files=all"])
        .map_err(|e| merge_failed(&branch, e))?;
    if !dirty.is_empty() {
        return Err(CliError::user(
            "worktree_dirty",
            format!(
                "worktree {worktree} has uncommitted or untracked files; nothing was landed. \
                 Message the worker to commit (or remove) them, then land"
            ),
        )
        .with_details(json!({"status": dirty.lines().take(20).collect::<Vec<_>>()})));
    }
    let commits = resident::git(wt, &["rev-list", "--count", &format!("{source}..{branch}")])
        .map_err(|e| merge_failed(&branch, e))?
        .parse::<usize>()
        .unwrap_or(0);
    if commits == 0 {
        return emit(false, Some("nothing-to-land"), 0, None, None);
    }
    if args.dry_run {
        return emit(false, None, commits, None, None);
    }

    let git = git_bin();
    match merge_recovery::recover_node(&paths, &node_id, &git) {
        merge_recovery::Recovery::DriverAlive => {
            return Err(CliError::user(
                "merge_in_progress",
                format!(
                    "another merge or land is in flight for node {node_id}; retry when it finishes"
                ),
            ))
        }
        merge_recovery::Recovery::CannotVerify => {
            return Err(CliError::system(
                "merge_recovery_unverifiable",
                format!(
                    "a prior merge transaction for node {node_id} could not be verified via git"
                ),
            ))
        }
        // A completed crashed land leaves any remaining commits to the fresh
        // transaction below (re-counted by the merge driver itself).
        merge_recovery::Recovery::Completed
        | merge_recovery::Recovery::Rejected { .. }
        | merge_recovery::Recovery::Superseded
        | merge_recovery::Recovery::NothingPending => {}
    }
    let start = record_merge_start(
        &paths,
        &node_id,
        &worktree,
        &branch,
        &node,
        Some(&source),
        &git,
        None,
        true,
    )?
    .ok_or_else(|| merge_failed(&branch, "landing transaction was not recorded"))?;
    let integrated = std::cell::OnceCell::new();
    if let Err(error) = run_git_merge(
        wt,
        &branch,
        Some(&source),
        Some(&start.expected_source_oid),
        || Ok(()),
        |oid| {
            let _ = integrated.set(oid.to_string());
            Ok(())
        },
    ) {
        abort_merge_start(&paths, &node_id, &start.op_id, "land did not complete");
        if resident::rebase_in_progress(wt) {
            let _ = resident::git(wt, &["rebase", "--abort"]);
            return Err(CliError::user(
                "land_conflict",
                format!(
                    "rebasing {branch} onto {source} conflicts; nothing was landed and the worktree \
                     is unchanged. Message the worker: Taskfleet rebases before resuming and the \
                     conflict becomes the first task of its next round"
                ),
            )
            .with_details(json!({"branch": branch, "source": source})));
        }
        return Err(error);
    }
    let integrated_oid = integrated
        .into_inner()
        .or_else(|| merge_recovery::read_oid(&git, &worktree, &source))
        .ok_or_else(|| merge_failed(&branch, "could not read the landed source tip"))?;
    let outcome = append_and_apply_event(
        &paths,
        "node.round_landed",
        Some(&node_id),
        Some(&landed_key(&run_id, &node_id, &start.op_id)),
        json!({
            "op_id": start.op_id,
            "worker_oid": start.worker_oid,
            "integrated_oid": integrated_oid,
            "source_branch": source,
        }),
    )
    .map_err(crate::run::from_core)?;
    emit(true, None, commits, Some(integrated_oid), Some(outcome.seq))
}

/// Idempotency key of the `node.round_landed` that completes landing `op_id`.
pub(crate) fn landed_key(run_id: &str, node_id: &NodeId, op_id: &str) -> String {
    format!("round-landed:{run_id}:{node_id}:{op_id}")
}
