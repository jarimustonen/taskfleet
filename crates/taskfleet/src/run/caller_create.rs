//! Git-only caller-owned creation. The key reservation is the resource ledger:
//! it records the immutable run, branch, checkout and fork OID before Git runs.
//! No error path releases this reservation or deletes the checkout. A retry
//! serializes on its run's lease and can only adopt an exactly registered tree.
//! This mode is NOT safe for an external Pi writer until settlement fencing ships.
use std::path::Path;
use std::process::{Command, Stdio};

use chrono::Utc;
use serde_json::json;
use taskfleet_core::{AgentOwner, Kind, Lifecycle, RunId, RunLock};

use super::{create::Args, from_core, parse_node_id, run_paths_exact, supervisor_spawn};
use crate::error::CliError;
use crate::idempotency::{
    self, CallerPlan, CreatorLease, MaterializerLease, Reservation, ReservationRecord,
};
use crate::output::{self, OutputFormat};

fn git(repo: &Path, args: &[&str]) -> Result<String, CliError> {
    let output = Command::new(crate::supervise::cleanup::git_bin())
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| CliError::system("git_failed", format!("git in {}: {e}", repo.display())))?;
    if !output.status.success() {
        return Err(CliError::system(
            "git_failed",
            format!(
                "git in {} {:?}: {}",
                repo.display(),
                args,
                String::from_utf8_lossy(&output.stderr)
            ),
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|e| CliError::system("git_failed", format!("git output is not UTF-8: {e}")))
}
fn crash_at(boundary: &str) {
    // Test-only hard-kill seam. No production caller should set this variable.
    if cfg!(debug_assertions)
        && std::env::var("TASKFLEET_TEST_CALLER_CRASH_AT")
            .ok()
            .as_deref()
            == Some(boundary)
    {
        std::process::exit(71);
    }
}
fn uncertain(id: &str, plan: &CallerPlan, why: &str) -> CliError {
    CliError::system("creation_uncertain", format!(
        "run {id}: {why}; preserve {} and branch {}. Inspect `git worktree list --porcelain` and the reservation before retrying; no resources were removed",
        plan.checkout, plan.branch
    )).with_invalid_value(id)
}
fn verify(repo: &Path, source: &str, plan: &CallerPlan) -> Result<(), CliError> {
    if git(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{source}"),
        ],
    )
    .is_err()
    {
        return Err(uncertain(
            "reserved",
            plan,
            "recorded source branch is missing",
        ));
    }
    let path = Path::new(&plan.checkout);
    let canonical = path.canonicalize().map_err(|e| {
        uncertain(
            "reserved",
            plan,
            &format!("checkout cannot be resolved: {e}"),
        )
    })?;
    if canonical != path {
        return Err(uncertain(
            "reserved",
            plan,
            "checkout path is not canonical",
        ));
    }
    let expected_common = git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let actual_common = git(
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    if expected_common.trim() != actual_common.trim() {
        return Err(uncertain("reserved", plan, "Git common directory differs"));
    }
    let branch = git(path, &["symbolic-ref", "--quiet", "HEAD"])?;
    if branch.trim() != format!("refs/heads/{}", plan.branch) {
        return Err(uncertain(
            "reserved",
            plan,
            "HEAD is not the planned branch",
        ));
    }
    let listing = git(repo, &["worktree", "list", "--porcelain"])?;
    let matches: Vec<_> = listing
        .split("\n\n")
        .filter(|entry| {
            entry
                .lines()
                .any(|s| s == format!("worktree {}", plan.checkout))
        })
        .collect();
    if matches.len() != 1
        || !matches[0]
            .lines()
            .any(|s| s == format!("branch refs/heads/{}", plan.branch))
    {
        return Err(uncertain(
            "reserved",
            plan,
            "checkout is not uniquely registered with planned branch",
        ));
    }
    Ok(())
}

pub(super) fn run(args: &Args<'_>) -> Result<(), CliError> {
    if args.kind != Kind::Spinoff
        || args.source_repo.is_none()
        || args.source_branch.is_none()
        || args.task.is_none()
        || args.idempotency_key.is_none()
        || args.prompt_file.is_some()
        || args.profile.is_some()
        || args.harness.is_some()
        || args.layout.is_some()
        || args.no_hooks
        || args.headless
        || args.tmux_session.is_some()
        || args.parent_run_id.is_some()
        || args.parent_node_id.is_some()
        || args.interactive
        || args.dry_run
        || args.skip_materialize
        || args.notify.is_some()
        || args.agent_startup_timeout != 90
    {
        return Err(CliError::user("invalid_arguments", "caller-owned spinoff requires --source-repo, --source-branch, --title, --task, --idempotency-key; worker/profile/tmux/parent/dry-run flags are unsupported"));
    }
    let title = super::require_nonempty(&args.title, "title")?;
    let task = super::require_nonempty(args.task.as_deref().unwrap(), "task")?;
    let key = super::require_nonempty(args.idempotency_key.as_deref().unwrap(), "idempotency-key")?;
    let raw = Path::new(args.source_repo.as_deref().unwrap());
    if !raw.is_absolute() {
        return Err(CliError::user(
            "invalid_source_repo",
            "--source-repo must be absolute",
        ));
    }
    let repo = raw
        .canonicalize()
        .map_err(|e| CliError::user("invalid_source_repo", format!("source checkout: {e}")))?;
    let top = git(&repo, &["rev-parse", "--show-toplevel"])?;
    if Path::new(top.trim()) != repo {
        return Err(CliError::user(
            "invalid_source_repo",
            "--source-repo must be the exact Git checkout root",
        ));
    }
    let source = super::require_nonempty(args.source_branch.as_deref().unwrap(), "source-branch")?;
    if source.starts_with('-')
        || source.starts_with('/')
        || source.starts_with("refs/")
        || source.contains("..")
        || git(&repo, &["check-ref-format", "--branch", &source]).is_err()
    {
        return Err(CliError::user(
            "invalid_source_branch",
            "--source-branch must name a valid local branch",
        ));
    }
    let branch_ref = format!("refs/heads/{source}");
    let base = git(
        &repo,
        &["rev-parse", "--verify", &format!("{branch_ref}^{{commit}}")],
    )?
    .trim()
    .to_owned();
    let root = crate::home::root_dir()?;
    let _admission = super::admission::admit(&root)?;
    let repo_string = repo.to_string_lossy().into_owned();
    // JSON encoding is unambiguous even if a title or task contains separators.
    let request = json!({"repo":repo_string,"source":source,"title":title,"task":task}).to_string();
    let proposed_id = taskfleet_core::new_run_id();
    let lease = MaterializerLease::acquire(&root, &proposed_id)?;
    let checkout = repo
        .parent()
        .ok_or_else(|| CliError::user("invalid_source_repo", "source has no parent"))?
        .join(".taskfleet-worktrees")
        .join(&proposed_id);
    let plan = CallerPlan {
        request: request.clone(),
        branch: format!("wt/{proposed_id}"),
        checkout: checkout.display().to_string(),
        base_sha: base,
    };
    let mut proposed = ReservationRecord::new(
        &proposed_id,
        CreatorLease {
            pid: std::process::id(),
            pid_start_secs: crate::supervise::watchdog::pid_start_time(std::process::id()),
            started_at: Utc::now(),
            materializer_lease_path: Some(lease.path().display().to_string()),
        },
    );
    proposed.caller_plan = Some(Box::new(plan));
    let (record, _lease, replay) =
        match idempotency::reserve(Some(&repo_string), Some(&source), &key, &proposed)? {
            Reservation::Reserved => (proposed, lease, false),
            Reservation::AlreadyReserved(existing) => {
                drop(lease);
                let Some(ref old_plan) = existing.caller_plan else {
                    return Err(CliError::user(
                        "idempotency_key_conflict",
                        format!("key already belongs to non-caller run {}", existing.run_id),
                    ));
                };
                if old_plan.request != request {
                    return Err(CliError::user(
                        "idempotency_key_conflict",
                        format!(
                            "key belongs to run {} with different inputs",
                            existing.run_id
                        ),
                    )
                    .with_invalid_value(existing.run_id));
                }
                // A dead creator's lease is reused, never reclaimed into another ID.
                // Blocking here serializes concurrent same-key retries, including
                // after the first creator died between Git and publication.
                let guard = MaterializerLease::acquire(&root, &existing.run_id)?;
                (existing, guard, true)
            }
        };
    let id = &record.run_id;
    crash_at("reservation");
    let plan = record.caller_plan.as_ref().expect("caller ledger");
    let run_id =
        RunId::parse_str(id).map_err(|e| CliError::system("corrupt_state", e.to_string()))?;
    let public = run_paths_exact(&root, &run_id)?;
    let stage_root = root.join(".creating");
    let staged_dir = taskfleet_core::run_dir(&stage_root, &run_id);
    let staging =
        taskfleet_core::RunPaths::from_validated(&staged_dir, run_id.clone()).map_err(from_core)?;
    if !public.manifest().exists() {
        let path = Path::new(&plan.checkout);
        if !path.exists() {
            // A branch without its checkout may be a partial Git operation; it
            // is not proof of an empty worktree. Fail closed, without pruning.
            if git(
                &repo,
                &[
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{}", plan.branch),
                ],
            )
            .is_ok()
            {
                return Err(uncertain(
                    id,
                    plan,
                    "planned branch exists but checkout is absent",
                ));
            }
            std::fs::create_dir_all(path.parent().unwrap())
                .map_err(|e| uncertain(id, plan, &format!("create checkout parent: {e}")))?;
            git(
                &repo,
                &[
                    "worktree",
                    "add",
                    "-b",
                    &plan.branch,
                    &plan.checkout,
                    &plan.base_sha,
                ],
            )
            .map_err(|e| uncertain(id, plan, &format!("git worktree add failed: {}", e.message)))?;
        }
        crash_at("git");
        verify(&repo, &source, plan).map_err(|e| uncertain(id, plan, &e.message))?;
        if staging.manifest().exists() {
            let (m, n) = RunLock::with_shared_lock(&staging.lock(), || {
                Ok((
                    taskfleet_core::read_manifest_opt(&staging)?,
                    taskfleet_core::read_node_opt(&staging, &parse_node_id("n-0001").unwrap())?,
                ))
            })
            .map_err(from_core)?;
            if m.as_ref().is_none_or(|m| {
                m.agent_owner != AgentOwner::Caller
                    || m.title != title
                    || m.source_repo.as_deref() != Some(&repo_string)
            }) || n.as_ref().is_none_or(|n| {
                n.branch.as_deref() != Some(&plan.branch)
                    || n.worktree_path.as_deref() != Some(&plan.checkout)
            }) {
                return Err(uncertain(
                    id,
                    plan,
                    "staged run is incomplete or inconsistent",
                ));
            }
        } else {
            if staged_dir.exists() {
                return Err(uncertain(id, plan, "staging directory lacks manifest"));
            }
            std::fs::create_dir_all(&staged_dir)
                .map_err(|e| uncertain(id, plan, &format!("create staging: {e}")))?;
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&staged_dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| uncertain(id, plan, &format!("protect staging: {e}")))?;
            taskfleet_core::append_and_apply_event(&staging, "run.created", None, None,
                json!({"kind":"spinoff","lifecycle":"interactive","agent_owner":"caller","title":title,"source_repo":repo_string,"source_branch":source,"task":task})).map_err(from_core)?;
            taskfleet_core::append_and_apply_event(&staging, "node.created", Some(&parse_node_id("n-0001")?), None,
                json!({"kind":"spinoff","branch":plan.branch,"worktree_path":plan.checkout,"source_branch":source,"base_sha":plan.base_sha,"task":task,"attempt":0})).map_err(from_core)?;
        }
        crash_at("staged");
        verify(&repo, &source, plan).map_err(|e| uncertain(id, plan, &e.message))?;
        std::fs::create_dir_all(root.join("runs"))
            .map_err(|e| uncertain(id, plan, &format!("create runs: {e}")))?;
        std::fs::rename(&staged_dir, &public.root)
            .map_err(|e| uncertain(id, plan, &format!("publish run: {e}")))?;
    }
    crash_at("published");
    let (m, n) = RunLock::with_shared_lock(&public.lock(), || {
        Ok((
            taskfleet_core::read_manifest_opt(&public)?,
            taskfleet_core::read_node_opt(&public, &parse_node_id("n-0001").unwrap())?,
        ))
    })
    .map_err(from_core)?;
    if m.as_ref().is_none_or(|m| {
        m.agent_owner != AgentOwner::Caller
            || m.lifecycle != Lifecycle::Interactive
            || m.source_repo.as_deref() != Some(&repo_string)
            || m.source_branch.as_deref() != Some(&source)
            || m.title != title
    }) || n.as_ref().is_none_or(|n| {
        n.branch.as_deref() != Some(&plan.branch)
            || n.worktree_path.as_deref() != Some(&plan.checkout)
            || n.agent_pid.is_some()
    }) {
        return Err(uncertain(
            id,
            plan,
            "published run/node does not match the reservation",
        ));
    }
    verify(&repo, &source, plan).map_err(|e| uncertain(id, plan, &e.message))?;
    let pid = if let Some(pid) = supervisor_spawn::read_live_recorded_pid(&public) {
        pid
    } else {
        match supervisor_spawn::spawn_for_run(&public, id)? {
            supervisor_spawn::SupervisorSpawn::Confirmed { pid } => pid,
            supervisor_spawn::SupervisorSpawn::Unconfirmed { reason } => return Err(CliError::system("supervisor_spawn_failed", format!("run {id} is published with a verified checkout but supervisor did not confirm: {reason}; retry the same key or use `taskfleet run reattach {id}`")).with_invalid_value(id)),
        }
    };
    let payload = json!({"run_id":id,"node_id":"n-0001","dir":public.root,"kind":"spinoff","agent_owner":"caller","lifecycle":"interactive","status":super::status_kebab(m.unwrap().status),"source_repo":repo_string,"source_branch":source,"branch":plan.branch,"worktree_path":plan.checkout,"checkout_verified":true,"supervisor":{"state":"confirmed","pid":pid},"idempotent_replay":replay,"writer_fence":"unavailable"});
    match args.spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => {
            output::emit_envelope(&payload, args.spec, args.warnings)?;
        }
        OutputFormat::Text => {
            println!("caller-owned run {id} node n-0001: {} (supervisor {pid}); writer fence unavailable", plan.checkout);
            output::emit_text_warnings(args.warnings);
        }
    }
    Ok(())
}
