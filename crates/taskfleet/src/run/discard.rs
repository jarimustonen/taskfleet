//! Explicit, audited disposal of retained terminal worker resources.

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::UNIX_EPOCH;

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
const FORCE_UPGRADE_KIND: &str = "cleanup.discard_force_authorized";

pub struct Args<'a> {
    pub run_id: String,
    pub settlement_key: Option<String>,
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
    #[serde(default)]
    identity: Option<TargetIdentity>,
    #[serde(default)]
    branch_oid: Option<String>,
}

// Captured while the checkout is still registered and Git-bound. Git may
// deregister it and remove .git before failing to unlink read-only contents.
// The directory inode then supplies the missing ownership proof on retry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TargetIdentity {
    directory_dev: u64,
    directory_ino: u64,
    directory_created_ns: u128,
    source_git_dev: u64,
    source_git_ino: u64,
    branch_oid: Option<String>,
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
    let caller = RunLock::with_shared_lock(&paths.lock(), || {
        Ok(read_manifest_opt(&paths)?
            .is_some_and(|m| m.agent_owner == taskfleet_core::AgentOwner::Caller))
    })
    .map_err(from_core)?;
    if caller {
        return super::caller_discard::run(&args, &paths);
    }
    if args.settlement_key.is_some() {
        return Err(CliError::user(
            "invalid_settlement_key",
            "--settlement-key is only for caller-owned runs",
        ));
    }
    let git = Git::with_bin(crate::supervise::cleanup::git_bin());

    let lock = RunLock::acquire_existing(&paths.lock()).map_err(from_core)?;
    let witness = lock.witness();
    // Legacy runs are read-only. Reject from the first locked manifest read,
    // before catch-up can truncate a torn tail or rewrite projections.
    let initial_manifest = read_manifest_opt(&paths)
        .map_err(from_core)?
        .ok_or_else(|| CliError::user("run_not_found", format!("no run with id {run_id}")))?;
    crate::run::reject_legacy_kind(initial_manifest.kind, &run_id)?;
    crate::run::require_writer_fence(&initial_manifest)?;
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
    let populated_submodules = if observation
        .as_ref()
        .is_some_and(|o| o.view.worktree_present && o.verified_for_discard())
    {
        git.worktree_has_populated_submodules(
            observation
                .as_ref()
                .and_then(|o| o.view.worktree_path.as_deref())
                .expect("present path"),
        )
        .ok_or_else(|| {
            CliError::system(
                "preserved_work_unverifiable",
                "could not inspect worktree submodules; refusing discard",
            )
        })?
    } else {
        false
    };
    let orphan = if let (Some(prior), Some(recorded)) = (prior.as_ref(), prior_auth.as_ref()) {
        // Inputs remain pinned even if the recorded path no longer looks like a
        // worktree. An old receipt without inode proof cannot authorize orphan
        // deletion; keep that directory for manual recovery.
        let upgraded = !recorded.force
            && args.force
            && force_upgrade_recorded(
                &witness,
                &paths,
                &node.node_id,
                prior.seq,
                &recorded.reason,
            )?;
        if recorded.reason != args.reason
            || (recorded.force != args.force
                && !upgraded
                && !force_retry_allowed(
                    recorded,
                    observation.as_ref(),
                    args.force,
                    populated_submodules,
                ))
        {
            return Err(CliError::user(
                "idempotency_conflict",
                format!("node {node_id} has an incomplete discard authorization with different reason/force inputs"),
            ));
        }
        let orphan = observation
            .as_ref()
            .is_some_and(|o| o.view.worktree_present && !o.verified_for_discard());
        if orphan {
            verify_orphan(recorded, &manifest, &node, &git)?;
        }
        if let Some(observation) = observation.as_ref() {
            if observation.has_resources()
                && (recorded.worktree_path != observation.view.worktree_path
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
        orphan
    } else {
        false
    };

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
    if !orphan && !observation.verified_for_discard() {
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

    // A force upgrade is only valid for the same inode/repository/ref pins,
    // including previews: never advertise recovery for a replacement target.
    if !orphan && observation.view.worktree_present {
        if let Some(auth) = prior_auth.as_ref().filter(|a| a.identity.is_some()) {
            verify_registered(auth, &manifest, &node, &git)?;
        }
    }
    let force_required = populated_submodules
        || observation.view.cleanliness == "dirty"
        || (orphan && !prior_auth.as_ref().is_some_and(|auth| auth.force));
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
                "node {node_id}'s worktree is dirty, has populated submodules, or is partially removed; --force explicitly authorizes deletion of remaining files (retry with the same reason)"
            ),
        )
        .with_expected(json!({"force": true})));
    }

    let auth = if let Some(auth) = prior_auth {
        auth
    } else {
        authorization(
            args.reason.clone(),
            args.force,
            &observation,
            &manifest,
            &git,
        )?
    };
    if !orphan && observation.view.worktree_present && auth.identity.is_some() {
        verify_registered(&auth, &manifest, &node, &git)?;
    }
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

    if args.force && !auth.force {
        // Keep the original receipt immutable. The separate upgrade references
        // its exact target/OID pins and records the operator's stronger choice.
        let key = format!("discard-force:{run_id}:{node_id}");
        let expected = json!({"authorization_seq":seq,"reason":auth.reason,"actor":format!("uid:{}", unsafe { libc::geteuid() })});
        if let Some(old) =
            find_prior_with_key(&witness, &paths, FORCE_UPGRADE_KIND, &key).map_err(from_core)?
        {
            if old.data != expected || old.node_id.as_deref() != Some(node.node_id.as_str()) {
                return Err(CliError::user(
                    "idempotency_conflict",
                    "force upgrade does not match recorded discard",
                ));
            }
        } else {
            append_and_apply_unlocked(
                &witness,
                &paths,
                FORCE_UPGRADE_KIND,
                Some(&node.node_id),
                Some(&key),
                expected,
            )
            .map_err(from_core)?;
        }
    }

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
        let failure = if orphan {
            // The inode and source Git identity were just checked; refuse any
            // registered checkout and never follow a replacement/symlink.
            (!remove_orphan(&auth, &manifest, &node, &git))
                .then(|| "orphaned directory removal failed".to_string())
        } else {
            if auth.identity.is_some() {
                verify_registered(&auth, &manifest, &node, &git)?;
            }
            git.worktree_remove_detail(repo, path, args.force)
        };
        if let Some(detail) = failure {
            return Err(CliError::system(
                "discard_partial_cleanup",
                format!(
                    "discard authorization event {seq} is durable, but worktree {path:?} was not removed: {detail}; retry with the same reason and force inputs; if Git requires force, explicitly authorize --force after a dry-run"
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
        // Never delete a ref while the recorded directory remains, even if
        // Git already deregistered it. A moved ref is not the audited target.
        if !path_absent(auth.worktree_path.as_deref())
            || git.worktree_registrations(repo).is_none_or(|rows| {
                rows.iter().any(|r| {
                    auth.worktree_path
                        .as_deref()
                        .is_some_and(|path| paths_equivalent(&r.path, path))
                        || r.branch.as_deref() == Some(branch)
                })
            })
        {
            return Err(CliError::system(
                "discard_partial_cleanup",
                "checkout still present or registered; branch preserved",
            ));
        }
        if prior_auth_missing_oid(&auth) {
            return Err(CliError::system(
                "preserved_work_unverifiable",
                "older discard receipt has no branch OID; preserving branch for manual recovery",
            ));
        }
        let detail = auth
            .branch_oid
            .as_ref()
            .or_else(|| auth.identity.as_ref().and_then(|id| id.branch_oid.as_ref()))
            .map_or_else(
                || git.branch_delete(repo, branch, true),
                |oid| git.branch_delete_oid(repo, &format!("refs/heads/{branch}"), oid),
            );
        if let Some(detail) = detail {
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

fn authorization(
    reason: String,
    force: bool,
    o: &RetainedObservation,
    manifest: &taskfleet_core::Manifest,
    git: &Git,
) -> Result<Authorization, CliError> {
    let branch_oid = o
        .view
        .branch
        .as_deref()
        .filter(|_| o.view.branch_present)
        .and_then(|branch| {
            git.ref_oid(
                manifest.source_repo.as_deref().expect("verified repo"),
                branch,
            )
        });
    if o.view.branch_present && branch_oid.is_none() {
        return Err(CliError::system(
            "preserved_work_unverifiable",
            "branch tip unavailable",
        ));
    }
    let identity = if o.view.worktree_present {
        let path = o.view.worktree_path.as_deref().expect("verified path");
        let dir = std::fs::symlink_metadata(path)
            .map_err(|e| CliError::system("preserved_work_unverifiable", e.to_string()))?;
        let common = git
            .common_dir(manifest.source_repo.as_deref().expect("verified repo"))
            .ok_or_else(|| {
                CliError::system(
                    "preserved_work_unverifiable",
                    "source Git directory unavailable",
                )
            })?;
        let source = common
            .metadata()
            .map_err(|e| CliError::system("preserved_work_unverifiable", e.to_string()))?;
        if !dir.is_dir() || !source.is_dir() {
            return Err(CliError::system(
                "preserved_work_unverifiable",
                "checkout or source Git identity is not a directory",
            ));
        }
        Some(TargetIdentity {
            directory_dev: dir.dev(),
            directory_ino: dir.ino(),
            directory_created_ns: created_ns(&dir)?,
            source_git_dev: source.dev(),
            source_git_ino: source.ino(),
            branch_oid: branch_oid.clone(),
        })
    } else {
        None
    };
    Ok(Authorization {
        actor: format!("uid:{}", unsafe { libc::geteuid() }),
        reason,
        force,
        worktree_path: o.view.worktree_path.clone(),
        branch: o.view.branch.clone(),
        cleanliness: o.view.cleanliness.to_string(),
        unmerged_commits: o.view.unmerged_commits,
        identity,
        branch_oid,
    })
}

fn path_absent(path: Option<&str>) -> bool {
    path.is_none_or(|p| {
        std::fs::symlink_metadata(p).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    })
}

fn force_upgrade_recorded(
    witness: &taskfleet_core::LockedRun<'_>,
    paths: &taskfleet_core::RunPaths,
    node: &NodeId,
    seq: u64,
    reason: &str,
) -> Result<bool, CliError> {
    let key = format!("discard-force:{}:{node}", paths.run_id);
    let prior = find_prior_with_key(witness, paths, FORCE_UPGRADE_KIND, &key).map_err(from_core)?;
    if let Some(event) = prior {
        let expected = json!({"authorization_seq":seq,"reason":reason,"actor":format!("uid:{}", unsafe { libc::geteuid() })});
        if event.node_id.as_deref() != Some(node.as_str()) || event.data != expected {
            return Err(CliError::user(
                "idempotency_conflict",
                "force upgrade does not match recorded discard",
            ));
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

fn force_retry_allowed(
    auth: &Authorization,
    observation: Option<&RetainedObservation>,
    force: bool,
    populated_submodules: bool,
) -> bool {
    force
        && !auth.force
        && auth.identity.is_some()
        && observation.is_some_and(|o| {
            o.view.worktree_present
                && (!o.verified_for_discard()
                    || (populated_submodules && o.view.cleanliness == "clean"))
        })
}

fn prior_auth_missing_oid(auth: &Authorization) -> bool {
    auth.branch_oid.is_none()
        && auth
            .identity
            .as_ref()
            .and_then(|id| id.branch_oid.as_ref())
            .is_none()
}

fn created_ns(meta: &std::fs::Metadata) -> Result<u128, CliError> {
    meta.created()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .ok_or_else(|| {
            CliError::system(
                "preserved_work_unverifiable",
                "checkout creation time unavailable; cannot prove orphan identity",
            )
        })
}

fn paths_equivalent(left: &str, right: &str) -> bool {
    left == right
        || Path::new(left)
            .canonicalize()
            .ok()
            .zip(Path::new(right).canonicalize().ok())
            .is_some_and(|(a, b)| a == b)
}

fn stale_own_marker(path: &Path, common: &Path) -> bool {
    let marker = path.join(".git");
    let meta = match std::fs::symlink_metadata(&marker) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => meta,
        _ => return false,
    };
    if meta.len() > 4096 {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(marker) else {
        return false;
    };
    let Some(gitdir) = text.trim_end_matches('\n').strip_prefix("gitdir: ") else {
        return false;
    };
    let gitdir = Path::new(gitdir);
    gitdir.is_absolute()
        && !gitdir.exists()
        && gitdir.parent().is_some_and(|parent| {
            parent.file_name().is_some_and(|name| name == "worktrees")
                && parent
                    .parent()
                    .and_then(|p| p.canonicalize().ok())
                    .as_deref()
                    == Some(common)
        })
}

fn verify_registered(
    auth: &Authorization,
    manifest: &taskfleet_core::Manifest,
    node: &taskfleet_core::Node,
    git: &Git,
) -> Result<(), CliError> {
    let refuse = || {
        CliError::system(
            "preserved_work_unverifiable",
            "registered checkout or branch no longer matches discard authorization",
        )
    };
    let id = auth.identity.as_ref().ok_or_else(refuse)?;
    let path = auth.worktree_path.as_deref().ok_or_else(refuse)?;
    let repo = manifest.source_repo.as_deref().ok_or_else(refuse)?;
    let dir = std::fs::symlink_metadata(path).map_err(|_| refuse())?;
    let source = git
        .common_dir(repo)
        .ok_or_else(refuse)?
        .metadata()
        .map_err(|_| refuse())?;
    let rows = git.worktree_registrations(repo).ok_or_else(refuse)?;
    if node.worktree_path.as_deref() != Some(path)
        || node.branch != auth.branch
        || !dir.is_dir()
        || dir.file_type().is_symlink()
        || (dir.dev(), dir.ino()) != (id.directory_dev, id.directory_ino)
        || created_ns(&dir).ok() != Some(id.directory_created_ns)
        || (source.dev(), source.ino()) != (id.source_git_dev, id.source_git_ino)
        || !rows.iter().any(|r| {
            paths_equivalent(&r.path, path)
                && r.branch
                    .as_deref()
                    .is_none_or(|b| Some(b) == auth.branch.as_deref())
        })
        || auth.branch.as_deref().is_some_and(|branch| {
            git.branch_exists(repo, branch) != Some(id.branch_oid.is_some())
                || id
                    .branch_oid
                    .as_ref()
                    .is_some_and(|oid| git.ref_oid(repo, branch).as_deref() != Some(oid))
        })
    {
        return Err(refuse());
    }
    Ok(())
}

fn verify_orphan(
    auth: &Authorization,
    manifest: &taskfleet_core::Manifest,
    node: &taskfleet_core::Node,
    git: &Git,
) -> Result<(), CliError> {
    let refuse = || {
        CliError::system("preserved_work_unverifiable", "orphaned directory identity, source repository, or branch changed; preserve for manual recovery")
    };
    let id = auth.identity.as_ref().ok_or_else(refuse)?;
    let path = auth.worktree_path.as_deref().ok_or_else(refuse)?;
    let repo = manifest.source_repo.as_deref().ok_or_else(refuse)?;
    if node.worktree_path.as_deref() != Some(path) || node.branch != auth.branch {
        return Err(refuse());
    }
    let dir = std::fs::symlink_metadata(path).map_err(|_| refuse())?;
    let common = git.common_dir(repo).ok_or_else(refuse)?;
    let source = common.metadata().map_err(|_| refuse())?;
    let rows = git.worktree_registrations(repo).ok_or_else(refuse)?;
    if !dir.is_dir()
        || dir.file_type().is_symlink()
        || (dir.dev(), dir.ino()) != (id.directory_dev, id.directory_ino)
        || created_ns(&dir).ok() != Some(id.directory_created_ns)
        || (source.dev(), source.ino()) != (id.source_git_dev, id.source_git_ino)
        || rows.iter().any(|r| {
            paths_equivalent(&r.path, path)
                || auth
                    .branch
                    .as_deref()
                    .is_some_and(|branch| r.branch.as_deref() == Some(branch))
        })
        || !stale_own_marker(Path::new(path), &common)
        || auth.branch.as_deref().is_some_and(|branch| {
            git.branch_exists(repo, branch) != Some(id.branch_oid.is_some())
                || id
                    .branch_oid
                    .as_ref()
                    .is_some_and(|oid| git.ref_oid(repo, branch).as_deref() != Some(oid))
        })
    {
        return Err(refuse());
    }
    Ok(())
}

fn remove_orphan(
    auth: &Authorization,
    manifest: &taskfleet_core::Manifest,
    node: &taskfleet_core::Node,
    git: &Git,
) -> bool {
    if verify_orphan(auth, manifest, node, git).is_err() {
        return false;
    }
    auth.worktree_path
        .as_deref()
        .is_some_and(|path| std::fs::remove_dir_all(path).is_ok())
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
