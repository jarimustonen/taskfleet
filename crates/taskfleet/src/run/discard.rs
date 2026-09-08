//! Explicit, audited disposal of retained terminal worker resources.

use serde::{Deserialize, Serialize};
use serde_json::json;
use taskfleet_core::{
    append_and_apply_unlocked, find_prior_with_key, read_all_events, read_manifest_opt,
    replay_unapplied_unlocked, NodeId, RunLock, Status,
};

use crate::error::CliError;
use crate::git::repo::Git;
use crate::output::{self, OutputFormat, OutputSpec};
use crate::run::dto::{SupervisorState, SupervisorView};
use crate::run::retained::{observe, RetainedObservation};
use crate::run::worker::{classify_worker, WorkerState};
use crate::run::{from_core, run_paths_from_cli_arg};

const EVENT_KIND: &str = "cleanup.discard_authorized";

pub struct Args<'a> {
    pub run_id: String,
    pub node: Option<String>,
    pub reason: String,
    pub force: bool,
    pub dry_run: bool,
    pub spec: &'a OutputSpec,
    pub warnings: &'a [String],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Authorization {
    actor: String,
    reason: String,
    force: bool,
    worktree_path: Option<String>,
    branch: Option<String>,
    cleanliness: String,
    unmerged_commits: Option<u64>,
}

#[derive(Serialize)]
struct Removed {
    worktree: bool,
    branch: bool,
}

#[derive(Serialize)]
struct AuditView {
    kind: &'static str,
    seq: u64,
    actor: String,
    reason: String,
}

// These independent booleans are the accepted wire contract (result,
// idempotency, force preview, and dry-run), not combinatorial domain state.
#[allow(clippy::struct_excessive_bools)]
#[derive(Serialize)]
struct Payload {
    run_id: String,
    node_id: String,
    discarded: bool,
    already_discarded: bool,
    removed: Removed,
    force_required: bool,
    worktree_path: Option<String>,
    branch: Option<String>,
    cleanliness: String,
    unmerged_commits: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audit_event: Option<AuditView>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    dry_run: bool,
}

pub fn run(args: Args<'_>) -> Result<(), CliError> {
    if args.reason.is_empty() || args.reason.chars().all(char::is_whitespace) {
        return Err(CliError::user(
            "invalid_reason",
            "--reason must contain non-whitespace text",
        )
        .with_invalid_value(&args.reason));
    }
    let selected_node = args
        .node
        .as_deref()
        .map(|raw| {
            NodeId::parse_str(raw).map_err(|_| {
                CliError::user("invalid_node_id", format!("invalid node id {raw:?}"))
                    .with_invalid_value(raw)
            })
        })
        .transpose()?;
    let root = crate::home::root_dir()?;
    let paths = run_paths_from_cli_arg(&root, &args.run_id)?;
    let run_id = paths.run_id.as_str().to_string();
    let git = Git::with_bin(crate::supervise::cleanup::git_bin());

    let lock = RunLock::acquire_existing(&paths.lock()).map_err(from_core)?;
    let witness = lock.witness();
    // Legacy runs are read-only. Reject from the first locked manifest read,
    // before catch-up can truncate a torn tail or rewrite projections.
    let initial_manifest = read_manifest_opt(&paths)
        .map_err(from_core)?
        .ok_or_else(|| CliError::user("run_not_found", format!("no run with id {run_id}")))?;
    crate::run::reject_legacy_kind(initial_manifest.kind, &run_id)?;
    if args.dry_run {
        let manifest = &initial_manifest;
        let last_seq = read_all_events(&paths.events())
            .map_err(from_core)?
            .last()
            .map_or(0, |event| event.seq);
        if manifest.applied_seq < last_seq {
            return Err(CliError::system(
                "projection_stale",
                format!(
                    "run {run_id} projections are behind the event log (applied {}, last {last_seq}); dry-run refuses rather than mutate replay state",
                    manifest.applied_seq
                ),
            ));
        }
    } else {
        replay_unapplied_unlocked(&witness, &paths).map_err(from_core)?;
    }
    let manifest = if args.dry_run {
        initial_manifest
    } else {
        read_manifest_opt(&paths)
            .map_err(from_core)?
            .ok_or_else(|| CliError::user("run_not_found", format!("no run with id {run_id}")))?
    };
    if !matches!(manifest.status, Status::Failed | Status::Cancelled) {
        return Err(CliError::user(
            "run_not_discardable",
            format!(
                "run {run_id} is {}; discard requires a failed or cancelled run",
                crate::run::status_kebab(manifest.status)
            ),
        ));
    }

    let nodes =
        crate::run::retained::read_nodes(&paths, crate::run::retained::MissingNodesDir::Error)
            .map_err(from_core)?;
    let mut candidates = Vec::new();
    for node in nodes {
        if !matches!(node.status, Status::Failed | Status::Cancelled) {
            continue;
        }
        let key = authorization_key(&run_id, node.node_id.as_str());
        let prior = find_prior_with_key(&witness, &paths, EVENT_KIND, &key).map_err(from_core)?;
        let observation = observe(&manifest, &node, &git);
        if observation.is_some() || prior.is_some() {
            candidates.push((node, observation, prior));
        }
    }

    let selected = if let Some(wanted) = selected_node.as_ref() {
        candidates
            .into_iter()
            .find(|(node, _, _)| &node.node_id == wanted)
            .ok_or_else(|| {
                CliError::user(
                    "preserved_work_not_found",
                    format!("node {wanted} has no retained or previously-discarded work"),
                )
                .with_invalid_value(wanted.as_str())
            })?
    } else {
        let current_count = candidates
            .iter()
            .filter(|(_, observation, _)| {
                observation
                    .as_ref()
                    .is_some_and(RetainedObservation::has_resources)
            })
            .count();
        if current_count == 1 {
            let index = candidates
                .iter()
                .position(|(_, observation, _)| {
                    observation
                        .as_ref()
                        .is_some_and(RetainedObservation::has_resources)
                })
                .expect("one current candidate");
            candidates.swap_remove(index)
        } else {
            match (current_count, candidates.len()) {
                (count, _) if count > 1 => {
                    return Err(CliError::user(
                        "ambiguous_preserved_work",
                        format!("run {run_id} has {count} retained nodes; pass --node <id>"),
                    ))
                }
                (0, 0) => {
                    return Err(CliError::user(
                        "preserved_work_not_found",
                        format!("run {run_id} has no retained work and no discard authorization"),
                    ))
                }
                (0, 1) => candidates.pop().expect("one authorized candidate"),
                // Every authorized target is now verifiably absent. A default
                // retry converges to the latest completed decision rather than
                // becoming ambiguous merely because history contains older
                // completed nodes. Any unverifiable observation remains Some
                // above and therefore cannot enter this no-op path.
                (0, _) => candidates
                    .into_iter()
                    .max_by_key(|(_, _, prior)| prior.as_ref().map_or(0, |event| event.seq))
                    .expect("at least one completed authorization"),
                _ => unreachable!(),
            }
        }
    };
    let (node, observation, prior) = selected;
    let node_id = node.node_id.as_str().to_string();

    let prior_auth = prior
        .as_ref()
        .map(|prior| {
            serde_json::from_value::<Authorization>(prior.data.clone()).map_err(|e| {
                CliError::system(
                    "discard_authorization_invalid",
                    format!(
                        "discard authorization event {} is malformed: {e}",
                        prior.seq
                    ),
                )
            })
        })
        .transpose()?;
    if let (Some(prior), Some(recorded)) = (prior.as_ref(), prior_auth.as_ref()) {
        if let Some(observation) = observation.as_ref() {
            if observation.has_resources()
                && (recorded.reason != args.reason
                    || recorded.force != args.force
                    || recorded.worktree_path != observation.view.worktree_path
                    || recorded.branch != observation.view.branch)
            {
                return Err(CliError::user(
                    "idempotency_conflict",
                    format!(
                        "node {node_id} has an incomplete discard authorization with different reason/force inputs"
                    ),
                ));
            }
        } else {
            return emit(
                Payload {
                    run_id,
                    node_id,
                    discarded: false,
                    already_discarded: true,
                    removed: Removed {
                        worktree: false,
                        branch: false,
                    },
                    force_required: false,
                    worktree_path: recorded.worktree_path.clone(),
                    branch: recorded.branch.clone(),
                    cleanliness: recorded.cleanliness.clone(),
                    unmerged_commits: recorded.unmerged_commits,
                    audit_event: Some(audit_view(prior.seq, recorded)),
                    dry_run: args.dry_run,
                },
                args.spec,
                args.warnings,
            );
        }
    }

    let observation = observation.ok_or_else(|| {
        CliError::user(
            "preserved_work_not_found",
            format!("node {node_id} has no current retained resources"),
        )
    })?;
    if !observation.has_resources() {
        return Err(CliError::system(
            "preserved_work_unverifiable",
            format!("node {node_id}'s retained resource existence is unverifiable"),
        ));
    }
    if !observation.verified_for_discard() {
        return Err(CliError::system(
            "preserved_work_unverifiable",
            format!(
                "node {node_id}'s repository, worktree registration, branch binding, cleanliness, or commit state could not be verified; refusing discard"
            ),
        ));
    }

    match SupervisorView::probe(&paths).state {
        SupervisorState::Alive => {
            return Err(CliError::user(
                "supervisor_live",
                format!("run {run_id}'s supervisor is still alive; refusing discard"),
            ))
        }
        SupervisorState::Unreadable | SupervisorState::Unknown => {
            return Err(CliError::system(
                "supervisor_unverifiable",
                format!("run {run_id}'s supervisor identity is unverifiable; refusing discard"),
            ))
        }
        SupervisorState::Dead | SupervisorState::NotRecorded => {}
    }
    match classify_worker(&node) {
        WorkerState::Exited | WorkerState::NoPid | WorkerState::Gone => {}
        WorkerState::Live { pid, .. } => {
            return Err(CliError::user(
                "worker_live",
                format!("node {node_id}'s original worker pid {pid} is still alive; refusing discard"),
            ))
        }
        WorkerState::Unverifiable { pid } => {
            return Err(CliError::system(
                "worker_unverifiable",
                format!("node {node_id}'s worker pid {pid} is alive but its identity is unverifiable; refusing discard"),
            ))
        }
    }

    let force_required = observation.view.cleanliness == "dirty";
    if args.dry_run {
        return emit(
            Payload {
                run_id,
                node_id,
                discarded: false,
                already_discarded: false,
                removed: Removed {
                    worktree: false,
                    branch: false,
                },
                force_required,
                worktree_path: observation.view.worktree_path.clone(),
                branch: observation.view.branch.clone(),
                cleanliness: observation.view.cleanliness.to_string(),
                unmerged_commits: observation.view.unmerged_commits,
                audit_event: None,
                dry_run: true,
            },
            args.spec,
            args.warnings,
        );
    }
    if force_required && !args.force {
        return Err(CliError::user(
            "force_required",
            format!(
                "node {node_id}'s worktree is dirty; --force explicitly authorizes deletion of staged, modified, and untracked files"
            ),
        )
        .with_expected(json!({"force": true})));
    }

    let auth =
        prior_auth.unwrap_or_else(|| authorization(args.reason.clone(), args.force, &observation));
    let key = authorization_key(&run_id, &node_id);
    let seq = if let Some(prior) = prior {
        prior.seq
    } else {
        append_and_apply_unlocked(
            &witness,
            &paths,
            EVENT_KIND,
            Some(&node.node_id),
            Some(&key),
            serde_json::to_value(&auth).expect("authorization serializes"),
        )
        .map_err(from_core)?
    };

    let repo = observation
        .source_repo
        .as_deref()
        .expect("verified source repo");
    let mut removed_worktree = false;
    if observation.view.worktree_present {
        let path = observation
            .view
            .worktree_path
            .as_deref()
            .expect("present worktree has path");
        if !git.worktree_remove(repo, path, force_required) {
            return Err(CliError::system(
                "discard_partial_cleanup",
                format!(
                    "discard authorization event {seq} is durable, but worktree {path:?} was not removed; retry with the same reason and force inputs"
                ),
            ));
        }
        removed_worktree = true;
    }

    let mut removed_branch = false;
    if observation.view.branch_present {
        let branch = observation
            .view
            .branch
            .as_deref()
            .expect("present branch recorded");
        if let Some(detail) = git.branch_delete(repo, branch, true) {
            return Err(CliError::system(
                "discard_partial_cleanup",
                format!(
                    "discard authorization event {seq} is durable and worktree removal completed, but branch {branch:?} survives: {detail}; retry with the same reason and force inputs"
                ),
            ));
        }
        removed_branch = true;
    }

    emit(
        Payload {
            run_id,
            node_id,
            discarded: true,
            already_discarded: false,
            removed: Removed {
                worktree: removed_worktree,
                branch: removed_branch,
            },
            force_required,
            worktree_path: auth.worktree_path.clone(),
            branch: auth.branch.clone(),
            cleanliness: auth.cleanliness.clone(),
            unmerged_commits: auth.unmerged_commits,
            audit_event: Some(audit_view(seq, &auth)),
            dry_run: false,
        },
        args.spec,
        args.warnings,
    )
}

fn authorization(reason: String, force: bool, o: &RetainedObservation) -> Authorization {
    Authorization {
        actor: format!("uid:{}", unsafe { libc::geteuid() }),
        reason,
        force,
        worktree_path: o.view.worktree_path.clone(),
        branch: o.view.branch.clone(),
        cleanliness: o.view.cleanliness.to_string(),
        unmerged_commits: o.view.unmerged_commits,
    }
}

fn authorization_key(run_id: &str, node_id: &str) -> String {
    format!("discard-authorized:{run_id}:{node_id}")
}

fn audit_view(seq: u64, auth: &Authorization) -> AuditView {
    AuditView {
        kind: EVENT_KIND,
        seq,
        actor: auth.actor.clone(),
        reason: auth.reason.clone(),
    }
}

fn emit(payload: Payload, spec: &OutputSpec, warnings: &[String]) -> Result<(), CliError> {
    match spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => {
            output::emit_envelope(&payload, spec, warnings)?;
        }
        OutputFormat::Text => {
            println!("run-id:             {}", payload.run_id);
            println!("node-id:            {}", payload.node_id);
            println!("discarded:          {}", payload.discarded);
            println!("already-discarded:  {}", payload.already_discarded);
            println!("removed-worktree:   {}", payload.removed.worktree);
            println!("removed-branch:     {}", payload.removed.branch);
            if payload.dry_run {
                println!("note:               --dry-run (no audit event or deletion)");
            }
            output::emit_text_warnings(warnings);
        }
    }
    Ok(())
}
