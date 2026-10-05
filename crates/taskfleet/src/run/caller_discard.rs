//! Destructive caller settlement. The intent closes launch admission; the held
//! writer EX lease, durable audit receipt, and fresh Git/history checks authorize
//! each mutation. No PID, tmux or workmux observation is settlement authority.
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::json;
use taskfleet_core::schema::{CallerSettlementIntent, SettlementOperation};
use taskfleet_core::{
    append_and_apply_event, read_all_events, read_manifest_opt, read_node_opt, Node, NodeId,
    RunLock, RunPaths, Status,
};

use super::discard::Args;
use super::merge::CallerMergeAuthority;
use super::retained::observe;
use super::{from_core, writer_fence};
use crate::error::CliError;
use crate::git::repo::Git;
use crate::output;
use crate::supervise::cleanup::{caller_history_retained, git_bin};

const KIND: &str = "cleanup.discard_authorized";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Receipt {
    run_id: taskfleet_core::RunId,
    node_id: NodeId,
    intent_seq: u64,
    settlement_key: String,
    writer_dev: u64,
    writer_ino: u64,
    generation: u64,
    actor: String,
    reason: String,
    force: bool,
    source_repo: String,
    source_branch: String,
    worktree_path: String,
    branch: String,
    branch_oid: String,
    source_git_dev: u64,
    source_git_ino: u64,
}

fn denied(message: impl Into<String>) -> CliError {
    CliError::user("checkout_mismatch", message)
}
fn recovery(paths: &RunPaths, key: &str, reason: &str) -> String {
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    format!(
        "taskfleet run discard {} --force --settlement-key {} --reason {}",
        paths.run_id,
        quote(key),
        quote(reason)
    )
}
fn details(
    e: CliError,
    paths: &RunPaths,
    key: &str,
    reason: &str,
    intent: &CallerSettlementIntent,
) -> CliError {
    e.with_details(json!({"run_id":paths.run_id,"phase":"intent-recorded",
        "intent_seq":intent.seq,"recovery_command":recovery(paths,key,reason)}))
}
fn state(paths: &RunPaths) -> Result<(taskfleet_core::Manifest, Node), CliError> {
    RunLock::with_shared_lock(&paths.lock(), || {
        Ok((
            read_manifest_opt(paths)?,
            read_node_opt(paths, &NodeId::parse_str("n-0001").expect("constant"))?,
        ))
    })
    .map_err(from_core)
    .and_then(|(m, n)| {
        Ok((
            m.ok_or_else(|| denied("run projection missing"))?,
            n.ok_or_else(|| denied("node projection missing"))?,
        ))
    })
}
fn terminal(m: &taskfleet_core::Manifest, n: &Node, paths: &RunPaths) -> Result<(), CliError> {
    if m.agent_owner != taskfleet_core::AgentOwner::Caller
        || m.run_id != paths.run_id
        || n.run_id != paths.run_id
        || n.node_id.as_str() != "n-0001"
        || m.node_count != 1
        || !matches!(m.status, Status::Failed | Status::Cancelled)
        || n.status != m.status
        || n.pending_merge.is_some()
    {
        return Err(CliError::user(
            "settlement_conflict",
            "caller discard requires one failed/cancelled node without a pending merge transaction",
        ));
    }
    let facts = RunLock::with_shared_lock(&paths.lock(), || {
        taskfleet_core::read_node_status_facts(paths, None)
    })
    .map_err(from_core)?;
    if !matches!(facts.as_slice(), [fact] if fact.node_id == n.node_id && fact.status == n.status) {
        return Err(CliError::user(
            "settlement_conflict",
            "event log does not confirm the single terminal node",
        ));
    }
    Ok(())
}
fn identities<'a>(
    m: &'a taskfleet_core::Manifest,
    n: &'a Node,
) -> Result<(&'a str, &'a str, &'a str, &'a str), CliError> {
    let repo = m
        .source_repo
        .as_deref()
        .ok_or_else(|| denied("source repository missing"))?;
    let source = m
        .source_branch
        .as_deref()
        .ok_or_else(|| denied("source branch missing"))?;
    let checkout = n
        .worktree_path
        .as_deref()
        .ok_or_else(|| denied("checkout missing"))?;
    let branch = n
        .branch
        .as_deref()
        .ok_or_else(|| denied("worker branch missing"))?;
    if repo == checkout
        || branch == source
        || !Path::new(repo).is_absolute()
        || !Path::new(checkout).is_absolute()
        || !Path::new(repo).is_dir()
        || Path::new(repo).canonicalize().ok().as_deref() != Some(Path::new(repo))
    {
        return Err(denied("source/worker checkout identity is unsafe"));
    }
    Ok((repo, source, checkout, branch))
}
/// A missing lifecycle projection is not proof that Pi never reserved a
/// generation: the log must corroborate that absence. With a reservation,
/// match the last logged generation and immutable binding to the projection
/// before trusting the native JSONL path outside the checkout.
fn history_checked(paths: &RunPaths, node: &Node, checkout: &str) -> Result<(), CliError> {
    let events = RunLock::with_shared_lock(&paths.lock(), || read_all_events(&paths.events()))
        .map_err(from_core)?;
    let mut lifecycle: Option<taskfleet_core::schema::CallerPiLifecycle> = None;
    let mut binding: Option<taskfleet_core::schema::CallerPiSession> = None;
    for e in events
        .iter()
        .filter(|e| e.node_id.as_ref() == Some(&node.node_id))
    {
        if e.run_id != paths.run_id {
            return Err(CliError::user(
                "history_unavailable",
                "foreign run identity in Pi event log",
            ));
        }
        match e.kind.as_str() {
            "caller.pi.lifecycle" => {
                lifecycle = Some(
                    serde_json::from_value::<taskfleet_core::schema::CallerPiLifecycle>(
                        e.data.clone(),
                    )
                    .map_err(|_| {
                        CliError::user("history_unavailable", "invalid Pi generation in event log")
                    })?,
                );
            }
            "caller.pi.session_bound" => {
                let bound: taskfleet_core::schema::CallerPiSession =
                    serde_json::from_value(e.data.clone()).map_err(|_| {
                        CliError::user("history_unavailable", "invalid Pi binding in event log")
                    })?;
                if binding.as_ref().is_some_and(|old| old != &bound)
                    || bound.original_cwd != checkout
                    || bound.original_cwd.is_empty()
                    || bound.pi_session_id.is_empty()
                    || bound.session_path.is_empty()
                    || lifecycle.as_ref().is_some_and(|reserved| {
                        reserved.pi_session_id != bound.pi_session_id
                            || Some(reserved.generation) != bound.generation
                            || reserved
                                .session_path
                                .as_deref()
                                .is_some_and(|p| p != bound.session_path)
                            || !matches!(
                                reserved.state,
                                taskfleet_core::schema::CallerPiState::Reserved
                                    | taskfleet_core::schema::CallerPiState::Started
                            )
                    })
                    || (bound.generation.is_some() && lifecycle.is_none())
                {
                    return Err(CliError::user(
                        "history_unavailable",
                        "Pi binding does not match reservation",
                    ));
                }
                if let Some(current) = &mut lifecycle {
                    current.session_path = Some(bound.session_path.clone());
                }
                binding = Some(bound);
            }
            _ => {}
        }
    }
    if lifecycle != node.caller_pi_lifecycle
        || binding != node.caller_pi_session
        || !caller_history_retained(node, checkout)
    {
        return Err(CliError::user(
            "history_unavailable",
            "native Pi history, generation or no-reservation proof unavailable; preserve checkout",
        ));
    }
    Ok(())
}
fn source_git_identity(repo: &str) -> Result<(u64, u64), CliError> {
    let common = super::list::repository_identity(Path::new(repo), false)?
        .ok_or_else(|| denied("source Git common directory unavailable"))?;
    let m = common
        .metadata()
        .map_err(|_| denied("source Git directory unavailable"))?;
    if !m.is_dir() {
        return Err(denied("source Git identity is not a directory"));
    }
    Ok((m.dev(), m.ino()))
}
fn path_absent(path: &str) -> bool {
    std::fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}
fn receipt(
    paths: &RunPaths,
    intent: &CallerSettlementIntent,
    reason: &str,
    target: (&str, &str, &str, &str),
    oid: &str,
    source_git: (u64, u64),
) -> Receipt {
    let (repo, source, checkout, branch) = target;
    Receipt {
        run_id: paths.run_id.clone(),
        node_id: intent.node_id.clone(),
        intent_seq: intent.seq,
        settlement_key: intent.key.clone(),
        writer_dev: intent.writer_dev,
        writer_ino: intent.writer_ino,
        generation: intent.generation,
        actor: format!("uid:{}", unsafe { libc::geteuid() }),
        reason: reason.into(),
        force: true,
        source_repo: repo.into(),
        source_branch: source.into(),
        worktree_path: checkout.into(),
        branch: branch.into(),
        branch_oid: oid.into(),
        source_git_dev: source_git.0,
        source_git_ino: source_git.1,
    }
}
fn audit(
    paths: &RunPaths,
    intent: &CallerSettlementIntent,
    expected: &Receipt,
) -> Result<Option<u64>, CliError> {
    let events = RunLock::with_shared_lock(&paths.lock(), || read_all_events(&paths.events()))
        .map_err(from_core)?;
    let mut found = None;
    for e in events
        .iter()
        .filter(|e| e.kind == KIND && e.run_id == paths.run_id)
    {
        let parsed: Receipt = serde_json::from_value(e.data.clone())
            .map_err(|_| denied("malformed discard authorization"))?;
        if e.node_id.as_ref() != Some(&intent.node_id)
            || e.idempotency_key.as_deref() != Some(expected.settlement_key.as_str())
            || parsed != *expected
            || found.is_some()
        {
            return Err(CliError::user(
                "idempotency_conflict",
                "different or malformed caller discard authorization already recorded",
            ));
        }
        found = Some(e.seq);
    }
    Ok(found)
}
fn observe_checked(
    m: &taskfleet_core::Manifest,
    n: &Node,
    git: &Git,
) -> Result<Option<super::retained::RetainedObservation>, CliError> {
    let o = observe(m, n, git);
    if o.as_ref().is_some_and(|o| !o.verified_for_discard()) {
        return Err(denied(
            "retained Git work or source-relative commit check unverifiable",
        ));
    }
    Ok(o)
}
fn source_checked(
    git: &Git,
    repo: &str,
    source: &str,
    checkout: &str,
    branch: &str,
) -> Result<(), CliError> {
    let rows = git
        .worktree_registrations(repo)
        .ok_or_else(|| denied("worktree registrations unavailable"))?;
    if rows.iter().any(|r| r.path == checkout)
        && Path::new(checkout).exists()
        && std::fs::symlink_metadata(Path::new(checkout).join(".git"))
            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    {
        return Err(CliError::user(
            "checkout_damaged",
            "registered caller checkout lacks .git metadata; preserve the run, branch and remaining files; inspect the receipt and registrations, then manually repair the recorded checkout identity before retrying the same key/reason (see issues/caller-owned-agent-runs/writer-fence.md)",
        ));
    }
    if git.head_branch(repo).as_deref() != Some(source)
        || !rows
            .iter()
            .any(|r| r.path == repo && r.branch.as_deref() == Some(source))
        || rows
            .iter()
            .any(|r| r.path == checkout && r.branch.as_deref() != Some(branch))
    {
        return Err(denied("source checkout or worker registration changed"));
    }
    Ok(())
}

pub(super) fn run(args: &Args<'_>, paths: &RunPaths) -> Result<(), CliError> {
    if args.run_id != paths.run_id.as_str() {
        return Err(CliError::user(
            "invalid_run_id",
            "caller discard requires the exact full run ID",
        ));
    }
    if args.node.as_deref().is_some_and(|n| n != "n-0001") {
        return Err(CliError::user(
            "invalid_node_id",
            "caller discard requires node n-0001",
        ));
    }
    let key = args
        .settlement_key
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            CliError::user(
                "invalid_settlement_key",
                "caller discard requires --settlement-key",
            )
        })?;
    if !args.force && !args.dry_run {
        return Err(CliError::user(
            "force_required",
            "caller discard always requires --force",
        ));
    }
    let (m, n) = state(paths)?;
    terminal(&m, &n, paths)?;
    identities(&m, &n)?;
    if args.dry_run {
        let _lock = RunLock::acquire_existing(&paths.lock()).map_err(from_core)?;
        let fence = writer_fence::inspect(paths)?;
        super::session::check_creation_fence(paths, &fence)?;
    }
    let git = Git::with_bin(git_bin());
    let observation = observe_checked(&m, &n, &git)?;
    if args.dry_run {
        // This preview neither fences admission nor claims the writer is idle.
        return output::emit_envelope(
            &json!({"run_id":paths.run_id,"node_id":n.node_id,
            "dry_run":true,"writer_quiesced":false,"force_required":true,
            "preserved_work":observation.map(|o| o.view)}),
            args.spec,
            args.warnings,
        );
    }
    let intent = writer_fence::record_intent(
        paths,
        &n.node_id,
        key,
        SettlementOperation::Discard,
        "caller-cli",
        Some(&args.reason),
        true,
    )?;
    let result = execute(args, paths, &intent);
    result.map_err(|e| details(e, paths, key, &args.reason, &intent))
}
/// The supervisor never executes the operator's destructive choice. It may
/// acknowledge completion only after the same fenced history and absence proof.
pub(crate) fn completed(
    paths: &RunPaths,
    intent: &CallerSettlementIntent,
    authority: &CallerMergeAuthority<'_>,
) -> bool {
    let result = (|| -> Result<(), CliError> {
        authority.revalidate()?;
        let (m, n) = state(paths)?;
        terminal(&m, &n, paths)?;
        if m.caller_settlement_intent.as_ref() != Some(intent)
            || !matches!(
                (m.status, intent.operation),
                (Status::Failed, SettlementOperation::Discard)
                    | (
                        Status::Cancelled | Status::Failed,
                        SettlementOperation::Cancel
                    )
            )
        {
            return Err(denied("intent changed"));
        }
        let (repo, source, checkout, branch) = identities(&m, &n)?;
        let git = Git::with_bin(git_bin());
        source_checked(&git, repo, source, checkout, branch)?;
        let events = RunLock::with_shared_lock(&paths.lock(), || read_all_events(&paths.events()))
            .map_err(from_core)?;
        let receipts: Vec<_> = events
            .iter()
            .filter(|e| e.kind == KIND && e.run_id == paths.run_id)
            .collect();
        if receipts.len() != 1 {
            return Err(denied("discard receipt absent or ambiguous"));
        }
        let event = receipts[0];
        let r: Receipt =
            serde_json::from_value(event.data.clone()).map_err(|_| denied("invalid receipt"))?;
        if !r.force
            || r.actor != format!("uid:{}", unsafe { libc::geteuid() })
            || r.reason.trim().is_empty()
            || r.intent_seq != intent.seq
            || r.settlement_key != intent.key
            || r.writer_dev != intent.writer_dev
            || r.writer_ino != intent.writer_ino
            || r.generation != intent.generation
            || r.run_id != paths.run_id
            || r.node_id != n.node_id
            || (
                r.source_repo.as_str(),
                r.source_branch.as_str(),
                r.worktree_path.as_str(),
                r.branch.as_str(),
            ) != (repo, source, checkout, branch)
            || event.node_id.as_ref() != Some(&n.node_id)
            || event.idempotency_key.as_deref() != Some(intent.key.as_str())
            || source_git_identity(repo)? != (r.source_git_dev, r.source_git_ino)
            || history_checked(paths, &n, checkout).is_err()
            || observe_checked(&m, &n, &git)?.is_some()
            || !path_absent(checkout)
            || git.branch_exists(repo, branch) != Some(false)
            || git
                .worktree_registrations(repo)
                .is_none_or(|rows| rows.iter().any(|row| row.path == checkout))
        {
            return Err(denied("discard completion not verified"));
        }
        authority.revalidate()?;
        Ok(())
    })();
    result.is_ok()
}

fn execute(
    args: &Args<'_>,
    paths: &RunPaths,
    intent: &CallerSettlementIntent,
) -> Result<(), CliError> {
    // The EX lease is taken outside the run lock and held across both Git operations.
    let authority = CallerMergeAuthority::acquire(paths, intent.clone())?;
    let (m, n) = state(paths)?;
    terminal(&m, &n, paths)?;
    if m.caller_settlement_intent.as_ref() != Some(intent)
        || !matches!(
            intent.operation,
            SettlementOperation::Discard | SettlementOperation::Cancel
        )
        || (intent.operation == SettlementOperation::Cancel
            && !matches!(m.status, Status::Cancelled | Status::Failed))
        || (intent.operation == SettlementOperation::Discard && m.status != Status::Failed)
        || intent.key != args.settlement_key.as_deref().unwrap_or_default()
        || intent.run_id != paths.run_id
        || intent.node_id != n.node_id
    {
        return Err(CliError::user(
            "settlement_conflict",
            "caller discard intent changed",
        ));
    }
    authority.revalidate()?;
    let (repo, source, checkout, branch) = identities(&m, &n)?;
    history_checked(paths, &n, checkout)?;
    let git = Git::with_bin(git_bin());
    source_checked(&git, repo, source, checkout, branch)?;
    let observation = observe_checked(&m, &n, &git)?;
    let branch_ref = format!("refs/heads/{branch}");
    let branch_oid = || super::merge_recovery::read_oid(&git_bin(), repo, &branch_ref);
    let oid = branch_oid();
    // An absent branch is only valid after a matching receipt. For a retry,
    // compare to the recorded immutable OID rather than mint a new target.
    let events = RunLock::with_shared_lock(&paths.lock(), || read_all_events(&paths.events()))
        .map_err(from_core)?;
    let old: Option<Receipt> = events
        .iter()
        .filter(|e| e.kind == KIND && e.run_id == paths.run_id)
        .map(|e| {
            serde_json::from_value(e.data.clone()).map_err(|_| denied("malformed discard receipt"))
        })
        .next()
        .transpose()?;
    let target_oid = oid
        .as_deref()
        .or(old.as_ref().map(|r| r.branch_oid.as_str()))
        .ok_or_else(|| denied("branch absent without a discard receipt"))?;
    let expected = receipt(
        paths,
        intent,
        &args.reason,
        (repo, source, checkout, branch),
        target_oid,
        source_git_identity(repo)?,
    );
    let prior = audit(paths, intent, &expected)?;
    if prior.is_none() && observation.as_ref().is_none_or(|o| !o.has_resources()) {
        return Err(denied("no retained work to authorize"));
    }
    if prior.is_none()
        || observation
            .as_ref()
            .is_some_and(|o| o.view.worktree_present)
    {
        authority.verify(&n, repo, source)?;
    } else {
        history_checked(paths, &n, checkout)?;
    }
    let seq = if let Some(seq) = prior {
        seq
    } else {
        authority.revalidate()?;
        append_and_apply_event(
            paths,
            KIND,
            Some(&n.node_id),
            Some(&intent.key),
            serde_json::to_value(&expected).expect("receipt serializes"),
        )
        .map_err(from_core)?
        .seq
    };
    #[cfg(test)]
    if std::env::var_os("TASKFLEET_TEST_CALLER_DISCARD_CRASH_AFTER_AUDIT").is_some() {
        std::process::exit(76);
    }
    // Re-read the event and all resources immediately before each destructive step.
    let check = || -> Result<(), CliError> {
        authority.revalidate()?;
        if audit(paths, intent, &expected)? != Some(seq) {
            return Err(denied("discard receipt changed"));
        }
        let (current, node) = state(paths)?;
        terminal(&current, &node, paths)?;
        if current.caller_settlement_intent.as_ref() != Some(intent)
            || identities(&current, &node)? != (repo, source, checkout, branch)
        {
            return Err(denied("recorded target changed"));
        }
        history_checked(paths, &node, checkout)?;
        source_checked(&git, repo, source, checkout, branch)?;
        let _ = observe_checked(&current, &node, &git)?;
        if source_git_identity(repo)? != (expected.source_git_dev, expected.source_git_ino) {
            return Err(denied("source repository identity changed"));
        }
        if let Some(actual) = branch_oid() {
            if actual != expected.branch_oid {
                return Err(denied("worker branch tip changed"));
            }
        } else if !path_absent(checkout) {
            return Err(denied("branch disappeared while checkout remains"));
        }
        Ok(())
    };
    check()?;
    let present = observation
        .as_ref()
        .is_some_and(|o| o.view.worktree_present);
    if present {
        // EX and explicit force allow dirty/unmerged work; neither bypasses identity.
        authority.verify(&n, repo, source)?;
        check()?;
        if !git.worktree_remove(repo, checkout, true) {
            return Err(CliError::user(
                "discard_partial_cleanup",
                "Git refused caller worktree removal; retry the same key/reason",
            ));
        }
        #[cfg(test)]
        if std::env::var_os("TASKFLEET_TEST_CALLER_DISCARD_CRASH_AFTER_WORKTREE").is_some() {
            std::process::exit(77);
        }
    }
    check()?;
    if git.branch_exists(repo, branch) != Some(false) {
        if branch_oid().as_deref() != Some(&expected.branch_oid) {
            return Err(denied("worker branch moved before deletion"));
        }
        // update-ref has no branch -D checked-out guard: independently refuse
        // deletion if ANY registered worktree still has this branch checked out.
        let rows = git
            .worktree_registrations(repo)
            .ok_or_else(|| denied("worktree registrations unavailable before branch deletion"))?;
        if rows.iter().any(|r| r.branch.as_deref() == Some(branch))
            || rows.iter().any(|r| r.path == checkout)
            || !path_absent(checkout)
        {
            return Err(denied("worker branch or checkout still registered"));
        }
        #[cfg(test)]
        if let Some(moved_oid) = std::env::var_os("TASKFLEET_TEST_DISCARD_MOVE_REF_BEFORE_CAS") {
            let moved_oid = moved_oid.to_str().expect("test OID");
            let status = std::process::Command::new(git_bin())
                .arg("-C")
                .arg(repo)
                .args(["update-ref", &branch_ref, moved_oid, &expected.branch_oid])
                .status()
                .expect("test ref move");
            assert!(status.success());
        }
        if git
            .branch_delete_oid(repo, &branch_ref, &expected.branch_oid)
            .is_some()
        {
            return Err(CliError::user(
                "discard_partial_cleanup",
                "Git refused caller branch deletion; retry the same key/reason",
            ));
        }
    }
    check()?;
    let (current, node) = state(paths)?;
    if observe_checked(&current, &node, &git)?.is_some()
        || !path_absent(checkout)
        || git
            .worktree_registrations(repo)
            .is_none_or(|rows| rows.iter().any(|r| r.path == checkout))
        || git.branch_exists(repo, branch) != Some(false)
    {
        return Err(CliError::user(
            "discard_partial_cleanup",
            "cannot verify complete caller resource removal",
        ));
    }
    output::emit_envelope(
        &json!({"run_id":paths.run_id,"node_id":n.node_id,
        "discarded":true,"already_discarded":!present && oid.is_none(),
        "removed":{"worktree":present,"branch":oid.is_some()},
        "audit_event":{"kind":KIND,"seq":seq,"actor":expected.actor,"reason":expected.reason},
        "cleanup":"complete"}),
        args.spec,
        args.warnings,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::fd::AsRawFd;
    use std::process::Command;
    use taskfleet_core::append_and_apply_event;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().into()
    }
    struct Fixture {
        tmp: tempfile::TempDir,
        paths: RunPaths,
        repo: std::path::PathBuf,
        checkout: std::path::PathBuf,
        node: NodeId,
        spec: output::OutputSpec,
        held_writer: Option<fs::File>,
    }
    impl Fixture {
        fn new(cancelled: bool) -> Self {
            Self::new_internal(cancelled, false, false)
        }
        fn new_internal(cancelled: bool, with_pi: bool, cancel_failed: bool) -> Self {
            let tmp = tempfile::tempdir_in("/var/tmp").unwrap();
            let repo = tmp.path().join("repo");
            fs::create_dir(&repo).unwrap();
            git(&repo, &["init", "-q", "-b", "main"]);
            git(&repo, &["config", "user.email", "test@example.invalid"]);
            git(&repo, &["config", "user.name", "Test"]);
            fs::write(repo.join("base"), "base").unwrap();
            git(&repo, &["add", "."]);
            git(&repo, &["commit", "-qm", "base"]);
            let checkout = tmp.path().join("worker");
            git(
                &repo,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    "wt/worker",
                    checkout.to_str().unwrap(),
                ],
            );
            fs::write(checkout.join("scratch"), "unmerged and dirty").unwrap();
            let home = tmp.path().join("state");
            let runs = home.join("runs");
            let dir = runs.join("01jxsnap000000000000000000");
            for p in [&home, &runs, &dir] {
                fs::create_dir(p).unwrap();
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
            }
            let paths = RunPaths::new(dir, "01jxsnap000000000000000000").unwrap();
            let fence = writer_fence::create(&paths).unwrap();
            append_and_apply_event(
                &paths,
                "run.created",
                None,
                None,
                json!({
                    "kind":"spinoff","lifecycle":"interactive","agent_owner":"caller",
                    "title":"test","source_repo":repo,"source_branch":"main","writer_fence":fence
                }),
            )
            .unwrap();
            let node = NodeId::parse_str("n-0001").unwrap();
            append_and_apply_event(
                &paths,
                "node.created",
                Some(&node),
                None,
                json!({
                    "kind":"spinoff","branch":"wt/worker","worktree_path":checkout
                }),
            )
            .unwrap();
            if with_pi {
                let store =
                    Path::new(&std::env::var_os("HOME").unwrap()).join(".pi/agent/sessions");
                fs::create_dir_all(&store).unwrap();
                let id = "b30d3508-a8d4-4aa7-bafa-7f5dfef72014";
                let timestamp = "2026-08-01T10:20:30.000Z";
                let history = store.join(format!(
                    "{}_{}.jsonl",
                    timestamp.replace([':', '.'], "-"),
                    id
                ));
                fs::write(&history, format!("{{\"type\":\"session\",\"version\":3,\"id\":\"{id}\",\"timestamp\":\"{timestamp}\",\"cwd\":\"{}\"}}\n",checkout.display())).unwrap();
                append_and_apply_event(
                    &paths,
                    "caller.pi.lifecycle",
                    Some(&node),
                    None,
                    json!({"generation":1,"pi_session_id":id,"state":"reserved","reason":null}),
                )
                .unwrap();
                let meta = fs::metadata(&history).unwrap();
                append_and_apply_event(
                    &paths,
                    "caller.pi.session_bound",
                    Some(&node),
                    None,
                    json!({"pi_session_id":id,"session_path":history,"original_cwd":checkout,
                        "file_dev":meta.dev(),"file_ino":meta.ino(),"generation":1}),
                )
                .unwrap();
                if std::env::var_os("TASKFLEET_TEST_RESERVED_BOUND_ONLY").is_none() {
                    append_and_apply_event(
                        &paths,
                        "caller.pi.lifecycle",
                        Some(&node),
                        None,
                        json!({"generation":1,"pi_session_id":id,"session_path":history,
                            "state":"started","reason":null}),
                    )
                    .unwrap();
                }
            }
            if cancelled {
                writer_fence::record_intent(
                    &paths,
                    &node,
                    "key-1",
                    SettlementOperation::Cancel,
                    "caller-cli",
                    Some("stop"),
                    false,
                )
                .unwrap();
            }
            let held_writer = if cancel_failed {
                let fd = fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(paths.root.join("writer.lock"))
                    .unwrap();
                assert_eq!(
                    unsafe { libc::flock(fd.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) },
                    0
                );
                Some(fd)
            } else {
                None
            };
            append_and_apply_event(
                &paths,
                "node.report",
                Some(&node),
                None,
                if cancelled && !cancel_failed {
                    json!({"success":false,"cancelled":true,"reason":"stop"})
                } else {
                    json!({"success":false,"reason":"failure"})
                },
            )
            .unwrap();
            append_and_apply_event(
                &paths,
                "run.status",
                None,
                None,
                json!({"status":if cancelled && !cancel_failed {"cancelled"} else {"failed"}}),
            )
            .unwrap();
            Self {
                tmp,
                paths,
                repo,
                checkout,
                node,
                spec: output::OutputSpec::default(),
                held_writer,
            }
        }
        fn args(&self, reason: &str) -> Args<'_> {
            Args {
                run_id: self.paths.run_id.to_string(),
                settlement_key: Some("key-1".into()),
                node: None,
                reason: reason.into(),
                force: true,
                dry_run: false,
                spec: &self.spec,
                warnings: &[],
            }
        }
        fn intent(&self, operation: SettlementOperation, reason: &str) -> CallerSettlementIntent {
            writer_fence::record_intent(
                &self.paths,
                &self.node,
                "key-1",
                operation,
                "caller-cli",
                Some(reason),
                true,
            )
            .unwrap()
        }
        fn events(&self) -> Vec<taskfleet_core::Event> {
            read_all_events(&self.paths.events()).unwrap()
        }
    }
    #[test]
    fn failed_discard_fences_active_descendant_and_retries_without_losing_state() {
        let f = Fixture::new(false);
        let fd = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(f.paths.root.join("writer.lock"))
            .unwrap();
        assert_eq!(
            unsafe { libc::flock(fd.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) },
            0
        );
        assert_eq!(
            run(&f.args("operator decision"), &f.paths)
                .unwrap_err()
                .code,
            "writer_active"
        );
        assert!(f.checkout.join("scratch").exists());
        assert_eq!(f.events().iter().filter(|e| e.kind == KIND).count(), 0);
        let intent = read_manifest_opt(&f.paths)
            .unwrap()
            .unwrap()
            .caller_settlement_intent
            .unwrap();
        assert_eq!(intent.operation, SettlementOperation::Discard);
        drop(fd);
        run(&f.args("operator decision"), &f.paths).unwrap();
        assert!(!f.checkout.exists());
        assert_eq!(
            Git::with_bin("git").branch_exists(f.repo.to_str().unwrap(), "wt/worker"),
            Some(false)
        );
        assert_eq!(f.events().iter().filter(|e| e.kind == KIND).count(), 1);
        run(&f.args("operator decision"), &f.paths).unwrap();
        assert_eq!(f.events().iter().filter(|e| e.kind == KIND).count(), 1);
        assert_eq!(
            read_manifest_opt(&f.paths).unwrap().unwrap().status,
            Status::Failed
        );
        assert!(f.paths.root.join("writer.lock").exists());
        assert!(f.paths.events().exists());
        assert!(crate::supervise::cleanup::cleanup_terminal_nodes(&f.paths));
    }
    #[test]
    fn cancel_intent_then_unrelated_failure_can_discard_with_original_key() {
        let mut f = Fixture::new_internal(true, false, true);
        let before = read_manifest_opt(&f.paths)
            .unwrap()
            .unwrap()
            .caller_settlement_intent
            .unwrap();
        assert_eq!(before.operation, SettlementOperation::Cancel);
        assert_eq!(
            read_node_opt(&f.paths, &f.node).unwrap().unwrap().status,
            Status::Failed
        );
        assert_eq!(
            run(&f.args("discard after failure"), &f.paths)
                .unwrap_err()
                .code,
            "writer_active"
        );
        drop(f.held_writer.take());
        let mut wrong = f.args("discard after failure");
        wrong.settlement_key = Some("different".into());
        assert_eq!(
            run(&wrong, &f.paths).unwrap_err().code,
            "settlement_conflict"
        );
        run(&f.args("discard after failure"), &f.paths).unwrap();
        assert!(!f.checkout.exists());
        assert_eq!(
            read_manifest_opt(&f.paths)
                .unwrap()
                .unwrap()
                .caller_settlement_intent,
            Some(before)
        );
        assert!(crate::supervise::cleanup::cleanup_terminal_nodes(&f.paths));
    }
    #[test]
    fn dry_run_is_read_only_even_with_a_busy_writer() {
        let f = Fixture::new(false);
        let fd = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(f.paths.root.join("writer.lock"))
            .unwrap();
        assert_eq!(
            unsafe { libc::flock(fd.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) },
            0
        );
        let mut args = f.args("preview");
        args.dry_run = true;
        args.force = false;
        let before = fs::read(f.paths.events()).unwrap();
        run(&args, &f.paths).unwrap();
        assert_eq!(before, fs::read(f.paths.events()).unwrap());
        assert!(f.checkout.exists());
        drop(fd);
    }
    #[test]
    fn cancelled_discard_keeps_cancel_intent_and_requires_original_key() {
        let f = Fixture::new(true);
        let before = read_manifest_opt(&f.paths)
            .unwrap()
            .unwrap()
            .caller_settlement_intent
            .unwrap();
        assert_eq!(before.operation, SettlementOperation::Cancel);
        let mut wrong = f.args("later decision");
        wrong.settlement_key = Some("other-key".into());
        assert_eq!(
            run(&wrong, &f.paths).unwrap_err().code,
            "settlement_conflict"
        );
        assert!(f.checkout.exists());
        run(&f.args("later decision"), &f.paths).unwrap();
        assert!(!f.checkout.exists());
        assert_eq!(
            read_manifest_opt(&f.paths)
                .unwrap()
                .unwrap()
                .caller_settlement_intent,
            Some(before)
        );
        assert_eq!(
            f.events()
                .iter()
                .filter(|e| e.kind == "caller.settlement_intent")
                .count(),
            1
        );
        assert_eq!(
            run(&f.args("different reason"), &f.paths).unwrap_err().code,
            "idempotency_conflict"
        );
    }
    #[test]
    fn lost_reply_after_audit_retries_same_target() {
        let f = Fixture::new(false);
        let child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("run::caller_discard::tests::crash_child")
            .env("TASKFLEET_DISCARD_TEST_ROOT", &f.paths.root)
            .env("TASKFLEET_TEST_CALLER_DISCARD_CRASH_AFTER_AUDIT", "1")
            .status()
            .unwrap();
        assert_eq!(child.code(), Some(76));
        assert_eq!(f.events().iter().filter(|e| e.kind == KIND).count(), 1);
        assert!(f.checkout.join("scratch").exists());
        run(&f.args("delete"), &f.paths).unwrap();
        assert_eq!(f.events().iter().filter(|e| e.kind == KIND).count(), 1);
        assert!(!f.checkout.exists());
    }
    #[test]
    fn retry_after_worktree_removed_finishes_only_recorded_branch() {
        let f = Fixture::new(false);
        let child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("run::caller_discard::tests::crash_child")
            .env("TASKFLEET_DISCARD_TEST_ROOT", &f.paths.root)
            .env("TASKFLEET_TEST_CALLER_DISCARD_CRASH_AFTER_WORKTREE", "1")
            .status()
            .unwrap();
        assert_eq!(child.code(), Some(77));
        assert!(!f.checkout.exists());
        assert_eq!(
            Git::with_bin("git").branch_exists(f.repo.to_str().unwrap(), "wt/worker"),
            Some(true)
        );
        let wrong = f.args("different target");
        assert_eq!(
            run(&wrong, &f.paths).unwrap_err().code,
            "settlement_conflict"
        );
        run(&f.args("delete"), &f.paths).unwrap();
        assert_eq!(
            Git::with_bin("git").branch_exists(f.repo.to_str().unwrap(), "wt/worker"),
            Some(false)
        );
    }
    #[test]
    fn retry_with_same_name_tag_and_changed_branch_preserves_new_tip() {
        let f = Fixture::new(false);
        let child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("run::caller_discard::tests::crash_child")
            .env("TASKFLEET_DISCARD_TEST_ROOT", &f.paths.root)
            .env("TASKFLEET_TEST_CALLER_DISCARD_CRASH_AFTER_WORKTREE", "1")
            .status()
            .unwrap();
        assert_eq!(child.code(), Some(77));
        let old = git(&f.repo, &["rev-parse", "refs/heads/wt/worker"]);
        git(&f.repo, &["tag", "wt/worker", &old]);
        let moved = git(
            &f.repo,
            &[
                "commit-tree",
                &format!("{old}^{{tree}}"),
                "-p",
                &old,
                "-m",
                "moved",
            ],
        );
        git(
            &f.repo,
            &["update-ref", "refs/heads/wt/worker", &moved, &old],
        );
        assert!(matches!(
            run(&f.args("delete"), &f.paths).unwrap_err().code.as_str(),
            "idempotency_conflict" | "checkout_mismatch"
        ));
        assert_eq!(git(&f.repo, &["rev-parse", "refs/heads/wt/worker"]), moved);
    }
    #[test]
    fn branch_move_between_check_and_cas_preserves_new_tip() {
        let f = Fixture::new(false);
        let old = git(&f.repo, &["rev-parse", "refs/heads/wt/worker"]);
        let moved = git(
            &f.repo,
            &[
                "commit-tree",
                &format!("{old}^{{tree}}"),
                "-p",
                &old,
                "-m",
                "moved",
            ],
        );
        let child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("run::caller_discard::tests::crash_child")
            .env("TASKFLEET_DISCARD_TEST_ROOT", &f.paths.root)
            .env("TASKFLEET_TEST_DISCARD_MOVE_REF_BEFORE_CAS", &moved)
            .status()
            .unwrap();
        assert!(child.success());
        assert_eq!(git(&f.repo, &["rev-parse", "refs/heads/wt/worker"]), moved);
        assert!(!f.checkout.exists());
        assert!(run(&f.args("delete"), &f.paths).is_err());
    }
    #[test]
    fn damaged_registered_checkout_requires_manual_repair() {
        let f = Fixture::new(false);
        fs::remove_file(f.checkout.join(".git")).unwrap();
        assert_eq!(
            run(&f.args("delete"), &f.paths).unwrap_err().code,
            "checkout_damaged"
        );
        assert!(f.checkout.join("scratch").exists());
        assert_eq!(
            Git::with_bin("git").branch_exists(f.repo.to_str().unwrap(), "wt/worker"),
            Some(true)
        );
    }
    #[test]
    fn git_only_discard_under_stripped_path() {
        let f = Fixture::new(false);
        let bin = f.tmp.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let git_path = Command::new("which").arg("git").output().unwrap();
        assert!(git_path.status.success());
        let git_path = String::from_utf8(git_path.stdout).unwrap();
        std::os::unix::fs::symlink(git_path.trim(), bin.join("git")).unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("run::caller_discard::tests::crash_child")
            .env("TASKFLEET_DISCARD_TEST_ROOT", &f.paths.root)
            .env("PATH", &bin)
            .status()
            .unwrap();
        assert!(child.success());
        assert!(!f.checkout.exists());
    }
    #[test]
    fn crash_child() {
        let Ok(root) = std::env::var("TASKFLEET_DISCARD_TEST_ROOT") else {
            return;
        };
        let paths = RunPaths::new(root, "01jxsnap000000000000000000").unwrap();
        let spec = output::OutputSpec::default();
        let args = Args {
            run_id: paths.run_id.to_string(),
            settlement_key: Some("key-1".into()),
            node: None,
            reason: "delete".into(),
            force: true,
            dry_run: false,
            spec: &spec,
            warnings: &[],
        };
        if std::env::var_os("TASKFLEET_TEST_DISCARD_MOVE_REF_BEFORE_CAS").is_some() {
            assert_eq!(
                run(&args, &paths).unwrap_err().code,
                "discard_partial_cleanup"
            );
            return;
        }
        run(&args, &paths).unwrap();
        assert!(
            std::env::var_os("TASKFLEET_TEST_CALLER_DISCARD_CRASH_AFTER_AUDIT").is_none()
                && std::env::var_os("TASKFLEET_TEST_CALLER_DISCARD_CRASH_AFTER_WORKTREE").is_none(),
            "crash injection did not fire"
        );
    }
    #[test]
    fn missing_or_replaced_native_history_preserves_checkout() {
        // HOME is process-global. Keep native history under an isolated child HOME.
        let home = tempfile::tempdir_in("/var/tmp").unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("run::caller_discard::tests::history_child")
            .env("TASKFLEET_DISCARD_HISTORY_CHILD", "1")
            .env("HOME", home.path())
            .status()
            .unwrap();
        assert!(child.success());
    }
    #[test]
    fn history_child() {
        if std::env::var_os("TASKFLEET_DISCARD_HISTORY_CHILD").is_none() {
            return;
        }
        let f = Fixture::new_internal(false, true, false);
        let binding = read_node_opt(&f.paths, &f.node)
            .unwrap()
            .unwrap()
            .caller_pi_session
            .unwrap();
        let history = Path::new(&binding.session_path);
        let saved = history.with_extension("held");
        fs::rename(history, &saved).unwrap();
        assert!(matches!(
            run(&f.args("delete"), &f.paths).unwrap_err().code.as_str(),
            "history_unavailable" | "invalid_pi_session"
        ));
        assert!(f.checkout.exists());
        fs::copy(&saved, history).unwrap();
        assert!(matches!(
            run(&f.args("delete"), &f.paths).unwrap_err().code.as_str(),
            "history_unavailable" | "invalid_pi_session"
        ));
        assert!(f.checkout.exists());
        fs::remove_file(history).unwrap();
        fs::rename(&saved, history).unwrap();
        run(&f.args("delete"), &f.paths).unwrap();
        assert!(!f.checkout.exists());
        assert!(history.exists());
    }
    #[test]
    fn reserved_bound_history_child() {
        if std::env::var_os("TASKFLEET_TEST_RESERVED_BOUND_ONLY").is_none() {
            return;
        }
        let f = Fixture::new_internal(false, true, false);
        run(&f.args("delete"), &f.paths).unwrap();
        assert!(!f.checkout.exists());
        let mismatched = Fixture::new_internal(false, true, false);
        let path = mismatched.paths.node(&mismatched.node);
        let mut projection: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        projection["caller_pi_lifecycle"]["session_path"] = json!("/var/tmp/unrelated.jsonl");
        fs::write(&path, serde_json::to_vec(&projection).unwrap()).unwrap();
        assert_eq!(
            run(&mismatched.args("delete"), &mismatched.paths)
                .unwrap_err()
                .code,
            "history_unavailable"
        );
        assert!(mismatched.checkout.exists());
    }
    #[test]
    fn reserved_bound_history_survives_without_started() {
        let home = tempfile::tempdir_in("/var/tmp").unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("run::caller_discard::tests::reserved_bound_history_child")
            .env("TASKFLEET_TEST_RESERVED_BOUND_ONLY", "1")
            .env("HOME", home.path())
            .status()
            .unwrap();
        assert!(child.success());
    }
    #[test]
    fn conflicting_merge_intent_and_replaced_lock_preserve_work() {
        let f = Fixture::new(false);
        f.intent(SettlementOperation::Merge, "other");
        assert_eq!(
            run(&f.args("delete"), &f.paths).unwrap_err().code,
            "settlement_conflict"
        );
        assert!(f.checkout.exists());
        let f = Fixture::new(false);
        let old = f.paths.root.join("writer.lock");
        fs::rename(&old, f.paths.root.join("held.lock")).unwrap();
        fs::write(&old, "").unwrap();
        assert_eq!(
            run(&f.args("delete"), &f.paths).unwrap_err().code,
            "writer_fence_unavailable"
        );
        assert!(f.checkout.exists());
        let f = Fixture::new(false);
        fs::remove_file(f.paths.root.join("writer.lock")).unwrap();
        assert_eq!(
            run(&f.args("delete"), &f.paths).unwrap_err().code,
            "writer_fence_unavailable"
        );
        assert!(f.checkout.exists());
    }
    #[test]
    fn forged_authorization_never_makes_supervisor_delete_work() {
        let f = Fixture::new(false);
        let intent = f.intent(SettlementOperation::Discard, "delete");
        append_and_apply_event(
            &f.paths,
            KIND,
            Some(&f.node),
            Some("key-1"),
            json!({"forged":true}),
        )
        .unwrap();
        let authority = CallerMergeAuthority::acquire(&f.paths, intent.clone()).unwrap();
        assert!(!completed(&f.paths, &intent, &authority));
        drop(authority);
        assert!(!crate::supervise::cleanup::cleanup_terminal_nodes(&f.paths));
        assert_eq!(
            run(&f.args("delete"), &f.paths).unwrap_err().code,
            "checkout_mismatch"
        );
        assert!(f.checkout.exists());
    }
}
