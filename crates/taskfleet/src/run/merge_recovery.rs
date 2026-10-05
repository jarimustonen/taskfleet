//! Deterministic, OID-based recovery for a crashed `run merge` transaction (A2).
//!
//! `run merge` spans two durability domains — git refs and the event log — and
//! is not atomic across them (design.md §2.1b, issue `merge-transaction-recovery`).
//! Before mutating git it records a [`taskfleet_core::MergeTxn`] via a
//! `merge.started` event ([`Node::pending_merge`](taskfleet_core::Node)); on a clean
//! run the terminal `explicit-merge` `node.report` completes the transaction and
//! clears the record. A crash in between leaves a *pending* transaction, which
//! this module resolves — **once, against IMMUTABLE OIDs, for the ONE recorded
//! transaction** (never the mutable worker/source branch tips, never a general
//! branch scan):
//!
//! 1. **Driver-liveness gate.** If the `run merge` process that recorded the
//!    transaction is still alive (PID + start-time identity; bounded by a
//!    staleness window when no start-time was recorded), the merge is in-flight —
//!    leave it. Recovery only fires for a *crashed* driver, so it never races a
//!    live merge about to append its own report. (merge.sh also re-checks driver
//!    liveness right before the ref mutation, bounding the orphan-child window.)
//! 2. **Compare (the CAS "compare" half).** Read the recorded `source_branch`'s
//!    current OID (`source_now`). If it is still exactly `expected_source_oid`, the
//!    git mutation never landed → **reject** (`merge.aborted`), preserving the
//!    worker's branch and work. This is the merge-start-recorded / no-git-mutation
//!    crash window.
//! 3. **Confirm (source moved).** If `source_now` moved off `expected_source_oid`,
//!    confirm the move integrated *this* transaction's recorded work — using the
//!    rebase-robust, git-verified landing check ([`landing_signal`]) against the
//!    immutable `worker_oid`:
//!    - `worker_oid` git-verified integrated into `source_now` AND NOT already
//!      integrated into `expected_source_oid` → **complete**: append the
//!      `explicit-merge` `node.report` the crash prevented (the git-mutated /
//!      event-not-yet-appended window). Same idempotency key `run merge` uses, so a
//!      racing retry dedupes.
//!    - provably not integrated, or already present before the move → **reject**
//!      (fail closed, work preserved).
//!    - any git read undecidable → **`CannotVerify`**: leave the transaction
//!      pending; never abort or complete on a guess.
//!
//! The git shell-outs run OUTSIDE the run lock (invariant 3); the resolving event
//! is appended under the exclusive lock after re-verifying the transaction is
//! still the one we classified AND the node is still non-terminal (a racing cancel
//! is resolved by aborting the dangling transaction, not by a dead completion).
//!
//! Known residuals (deferred to the 0.2.1 operation lease, design §2.7): the CAS
//! is check-then-FF under the merge lock, not a single atomic ref update, so a
//! non-cooperating writer between the check and the FF — or a force-push between
//! this classify and the append — is not defended; and the orphan-child window is
//! bounded, not closed.

use chrono::{Duration, Utc};
use serde_json::{json, Value};

use taskfleet_core::report::validate_report_payload;
use taskfleet_core::{
    append_and_apply_idempotent, read_manifest_opt, read_node_opt, AppendOutcome, LockedRun,
    MergeTxn, NodeId, RunLock, RunPaths, VIA_EXPLICIT_MERGE,
};

use crate::run::landed::{landing_signal, LandedMethod, LandingInputs};
use crate::supervise::pid_file::pid_live_with_identity;

/// A transaction whose driver PID is still live but carries NO start-time
/// identity is trusted as an in-flight merge only until this age — past it, a
/// recycled PID must not strand the transaction forever (a real merge never runs
/// this long; the merge-lock timeout default is 10 min). With a recorded
/// start-time the identity check is authoritative and this bound does not apply.
const DRIVER_STALE_AFTER_MINS: i64 = 30;

/// What recovery did with one pending merge transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Recovery {
    /// No transaction was pending on the node.
    NothingPending,
    /// The driving `run merge` process is still alive — the merge is in-flight,
    /// so recovery left it untouched.
    DriverAlive,
    /// Git could not be consulted (missing repo/ref, git error): the transaction
    /// was left pending for a later attempt rather than resolved on a guess.
    CannotVerify,
    /// The transaction was resolved by completing it — the `explicit-merge`
    /// report the crash prevented was appended (or already present).
    Completed,
    /// The transaction was resolved by rejecting it — `merge.aborted` was
    /// appended; the worker's branch and work are preserved.
    Rejected { reason: String },
    /// The transaction we classified was superseded (resolved concurrently, or a
    /// newer attempt replaced it) before we could append — nothing to do.
    Superseded,
}

/// The git verdict for a pending transaction, computed outside the run lock.
enum Verdict {
    Complete,
    Reject { reason: String },
    CannotVerify,
}

/// A caller transaction is authoritative only when both immutable event records
/// agree with the original sticky intent. Projections and report text alone are
/// never cleanup authority. Called outside the run lock under the EX writer lease.
pub(crate) fn verified_caller_transaction(
    paths: &RunPaths,
    node_id: &NodeId,
    intent: &taskfleet_core::schema::CallerSettlementIntent,
    report_required: bool,
) -> Option<MergeTxn> {
    use taskfleet_core::schema::{AgentOwner, SettlementOperation};
    let (manifest, node, events) = RunLock::with_shared_lock(&paths.lock(), || {
        Ok((
            read_manifest_opt(paths)?,
            read_node_opt(paths, node_id)?,
            taskfleet_core::read_all_events(&paths.events())?,
        ))
    })
    .ok()?;
    let manifest = manifest?;
    let node = node?;
    if manifest.agent_owner != AgentOwner::Caller
        || manifest.caller_settlement_intent.as_ref() != Some(intent)
        || intent.operation != SettlementOperation::Merge
        || intent.run_id != paths.run_id
        || intent.node_id != *node_id
        || intent.seq == 0
        || node.run_id != paths.run_id
        || node.node_id != *node_id
    {
        return None;
    }
    let intent_event = events.iter().find(|e| {
        e.seq == intent.seq
            && e.kind == "caller.settlement_intent"
            && e.run_id == paths.run_id
            && e.node_id.as_ref() == Some(node_id)
    })?;
    let mut original: taskfleet_core::schema::CallerSettlementIntent =
        serde_json::from_value(intent_event.data.clone()).ok()?;
    original.seq = intent.seq;
    if original != *intent {
        return None;
    }
    let txns: Vec<_> = events
        .iter()
        .filter(|e| {
            e.seq > intent.seq
                && e.kind == taskfleet_core::KIND_MERGE_STARTED
                && e.run_id == paths.run_id
                && e.node_id.as_ref() == Some(node_id)
        })
        .filter_map(|e| serde_json::from_value::<MergeTxn>(e.data.clone()).ok())
        .filter(|t| {
            t.caller_authority.as_ref().is_some_and(|link| {
                link.intent_seq == intent.seq
                    && link.intent_key == intent.key
                    && (link.writer_dev, link.writer_ino) == (intent.writer_dev, intent.writer_ino)
            }) && manifest.source_branch.as_deref() == Some(&t.source_branch)
                && node.branch.as_deref() == Some(&t.worker_branch)
        })
        .collect();
    let txn = if report_required {
        let report = node.last_report.as_ref()?;
        if node.pending_merge.is_some()
            || !node.status.is_terminal()
            || report.get("via").and_then(Value::as_str) != Some(VIA_EXPLICIT_MERGE)
            || report.get("success").and_then(Value::as_bool) != Some(true)
        {
            return None;
        }
        let (op, oid) = match taskfleet_core::ReportOrigin::from_report(report)? {
            taskfleet_core::ReportOrigin::RunMerge {
                op_id: Some(op),
                worker_oid: Some(oid),
            } => (op, oid),
            _ => return None,
        };
        let reports: Vec<_> = events
            .iter()
            .filter(|e| {
                e.kind == "node.report"
                    && e.run_id == paths.run_id
                    && e.node_id.as_ref() == Some(node_id)
                    && e.data == *report
            })
            .collect();
        if reports.len() != 1 {
            return None;
        }
        let report_seq = reports[0].seq;
        let matching: Vec<_> = txns
            .into_iter()
            .filter(|t| {
                t.op_id == op
                    && t.worker_oid == oid
                    && events.iter().any(|e| {
                        e.kind == taskfleet_core::KIND_MERGE_STARTED
                            && e.seq > intent.seq
                            && e.seq < report_seq
                            && e.node_id.as_ref() == Some(node_id)
                            && serde_json::from_value::<MergeTxn>(e.data.clone())
                                .ok()
                                .as_ref()
                                == Some(t)
                    })
            })
            .collect();
        if matching.len() != 1 {
            return None;
        }
        matching.into_iter().next()?
    } else {
        let pending = node.pending_merge.as_ref()?;
        let matching: Vec<_> = txns.into_iter().filter(|t| t == pending.as_ref()).collect();
        if matching.len() != 1 {
            return None;
        }
        matching.into_iter().next()?
    };
    Some(txn)
}

/// Caller completion is tied to the driver's durable post-rebase tip, not to
/// patch equivalence of its pre-rebase tip. A source ref alone is not authority.
fn caller_verdict(
    paths: &RunPaths,
    node_id: &NodeId,
    txn: &MergeTxn,
    repo: &str,
    git: &str,
) -> Verdict {
    let Ok(events) = taskfleet_core::read_all_events(&paths.events()) else {
        return Verdict::CannotVerify;
    };
    let Some(link) = txn.caller_authority.as_ref() else {
        return Verdict::CannotVerify;
    };
    let starts: Vec<_> = events
        .iter()
        .filter(|e| {
            e.kind == taskfleet_core::KIND_MERGE_STARTED
                && e.node_id.as_ref() == Some(node_id)
                && e.run_id == paths.run_id
                && serde_json::from_value::<MergeTxn>(e.data.clone())
                    .ok()
                    .as_ref()
                    == Some(txn)
        })
        .collect();
    if starts.len() != 1 {
        return Verdict::CannotVerify;
    }
    let start_seq = starts[0].seq;
    let proofs: Vec<_> = events
        .iter()
        .filter(|e| {
            e.kind == "merge.rebased"
                && e.seq > start_seq
                && e.run_id == paths.run_id
                && e.node_id.as_ref() == Some(node_id)
                && e.data["op_id"] == txn.op_id
                && e.data["intent_seq"] == link.intent_seq
                && e.data["worker_oid"] == txn.worker_oid
                && e.data["expected_source_oid"] == txn.expected_source_oid
        })
        .collect();
    if proofs.is_empty() {
        // A dead driver may have stopped before publishing the post-rebase tip.
        // Unchanged source under the EX lease proves it did not move the ref;
        // never infer completion from an unlinked worker ancestor.
        return match read_oid(git, repo, &txn.source_branch) {
            Some(now) if now == txn.expected_source_oid => Verdict::Reject {
                reason: "source ref unchanged — merge never landed".into(),
            },
            _ => Verdict::CannotVerify,
        };
    }
    if proofs.len() != 1 {
        return Verdict::CannotVerify;
    }
    let Some(tip) = proofs[0].data["integrated_oid"].as_str() else {
        return Verdict::CannotVerify;
    };
    if !matches!(tip.len(), 40 | 64) || !tip.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Verdict::CannotVerify;
    }
    let Some(now) = read_oid(git, repo, &txn.source_branch) else {
        return Verdict::CannotVerify;
    };
    let Some(expected) = read_oid(git, repo, &txn.expected_source_oid) else {
        return Verdict::CannotVerify;
    };
    let Some(integrated) = read_oid(git, repo, tip) else {
        return Verdict::CannotVerify;
    };
    if expected != txn.expected_source_oid || integrated != tip {
        return Verdict::CannotVerify;
    }
    // A no-op merge is valid only when the worker and source were already the
    // same commit at start. A pre-ref-move proof on a nonempty rebase is not a landing.
    if now == expected {
        return if tip == expected && txn.worker_oid == expected {
            Verdict::Complete
        } else {
            Verdict::Reject {
                reason: "source ref unchanged — merge never landed".into(),
            }
        };
    }
    match (
        ancestor(git, repo, &expected, tip),
        ancestor(git, repo, tip, &now),
    ) {
        (Some(true), Some(true)) if tip != expected => Verdict::Complete,
        (Some(_), Some(false)) | (Some(false), Some(_)) => Verdict::Reject {
            reason: "source moved without the recorded rebased tip".into(),
        },
        _ => Verdict::CannotVerify,
    }
}

fn ancestor(git: &str, repo: &str, older: &str, newer: &str) -> Option<bool> {
    let status = std::process::Command::new(git)
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--is-ancestor", older, newer])
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    match status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

pub(crate) fn caller_landed(
    paths: &RunPaths,
    node_id: &NodeId,
    txn: &MergeTxn,
    source_repo: &str,
    git: &str,
) -> bool {
    matches!(
        caller_verdict(paths, node_id, txn, source_repo, git),
        Verdict::Complete
    )
}

/// Resolve every pending merge transaction on this run's nodes. Called from the
/// supervisor tick (the canonical recovery actor) and safe to call repeatedly —
/// each resolution is idempotent and a node with no pending transaction is a
/// cheap no-op.
pub(crate) fn recover_run(paths: &RunPaths, git: &str) {
    // Enumerate node ids under the shared lock (invariant 3), then resolve each
    // outside the scan so the git shell-out never runs under the flock.
    let node_ids = RunLock::with_shared_lock(&paths.lock(), || {
        let mut ids = Vec::new();
        let entries = match std::fs::read_dir(paths.nodes_dir()) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ids),
            Err(e) => return Err(taskfleet_core::Error::io(paths.nodes_dir(), e)),
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(nid) = NodeId::parse_str(stem) else {
                continue;
            };
            // Only nodes with a pending transaction are candidates.
            if read_node_opt(paths, &nid)?.is_some_and(|n| n.pending_merge.is_some()) {
                ids.push(nid);
            }
        }
        Ok(ids)
    });
    let node_ids = match node_ids {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                target: "taskfleet::supervise",
                error = %e,
                "merge-recovery: could not enumerate nodes; will retry next tick"
            );
            return;
        }
    };
    for nid in node_ids {
        match recover_node(paths, &nid, git) {
            Recovery::Completed => tracing::info!(
                target: "taskfleet::supervise",
                node_id = %nid,
                "merge-recovery: completed a crashed merge transaction (git-verified landed)"
            ),
            Recovery::Rejected { reason } => tracing::warn!(
                target: "taskfleet::supervise",
                node_id = %nid, reason = %reason,
                "merge-recovery: rejected a crashed merge transaction; work preserved"
            ),
            _ => {}
        }
    }
}

/// Resolve the pending merge transaction (if any) on one node, by exact OID.
///
/// Reads the transaction + repo under the shared lock, gates on driver liveness,
/// computes the git verdict outside the lock, then appends the resolving event
/// under the exclusive lock after re-verifying the transaction is unchanged.
pub(crate) fn recover_node(paths: &RunPaths, node_id: &NodeId, git: &str) -> Recovery {
    let owner = RunLock::with_shared_lock(&paths.lock(), || {
        Ok(read_manifest_opt(paths)?.map(|m| m.agent_owner))
    });
    let caller = match owner {
        Ok(Some(taskfleet_core::AgentOwner::Caller)) => true,
        Ok(Some(taskfleet_core::AgentOwner::Taskfleet)) => false,
        _ => return Recovery::CannotVerify,
    };
    if caller {
        return recover_caller_node(paths, node_id, git);
    }
    // 1. Read the transaction + the repo to probe, under the shared lock.
    let probed = RunLock::with_shared_lock(&paths.lock(), || {
        let node = read_node_opt(paths, node_id)?;
        let manifest = read_manifest_opt(paths)?;
        let source_repo = manifest.as_ref().and_then(|m| m.source_repo.clone());
        let txn = node.as_ref().and_then(|n| n.pending_merge.clone());
        let worktree = node.as_ref().and_then(|n| n.worktree_path.clone());
        let terminal = node.as_ref().is_some_and(|n| n.status.is_terminal());
        Ok((txn, source_repo, worktree, terminal))
    });
    let (txn, source_repo, worktree_path, terminal) = match probed {
        Ok(v) => v,
        Err(_) => return Recovery::CannotVerify,
    };
    let Some(txn) = txn else {
        return Recovery::NothingPending;
    };
    // A terminal node's transaction is already resolved by the terminal report's
    // reducer (which clears `pending_merge`); nothing to recover.
    if terminal {
        return Recovery::NothingPending;
    }

    // 2. Driver-liveness gate: a live `run merge` still owns the transaction.
    if driver_is_alive(&txn) {
        return Recovery::DriverAlive;
    }

    // 3. Classify by OID, outside the lock (git must not run under the flock).
    let verdict = classify(&txn, source_repo.as_deref(), worktree_path.as_deref(), git);
    let verdict = match verdict {
        Verdict::CannotVerify => return Recovery::CannotVerify,
        v => v,
    };

    // 4. Append the resolving event under the exclusive lock, re-verifying the
    //    transaction is still the exact one we classified (a concurrent retry may
    //    have superseded or resolved it in the git-probe window).
    let run_id = paths.run_id.as_str().to_string();
    let outcome = RunLock::with_lock(paths, |lock| {
        let Some(node) = read_node_opt(paths, node_id)? else {
            return Ok(Recovery::Superseded);
        };
        let still_pending = node
            .pending_merge
            .as_ref()
            .is_some_and(|t| t.op_id == txn.op_id);
        if !still_pending {
            return Ok(Recovery::Superseded);
        }
        // The node may have terminalized (e.g. a racing `run cancel` → Cancelled)
        // between the shared-lock classification and here. Appending an
        // explicit-merge report would be a dead event for a Cancelled node (the
        // reducer's adoption whitelist excludes Cancelled), leaving `pending_merge`
        // set and causing infinite re-classification every tick. Resolve the
        // transaction by ABORTING it instead — that clears `pending_merge` even on a
        // terminal node (/llm-review finding).
        if node.status.is_terminal() {
            abort_txn(
                paths,
                lock,
                node_id,
                &run_id,
                &txn.op_id,
                "node terminalized before recovery could complete",
            )?;
            return Ok(Recovery::Superseded);
        }
        match &verdict {
            Verdict::Complete => {
                // Append the terminal report the crashed driver would have — same
                // idempotency key as `run merge`, so a racing retry dedupes.
                let report = completion_report(&txn);
                let key = format!("explicit-merge:{run_id}:{node_id}");
                match append_and_apply_idempotent(
                    paths,
                    lock,
                    "node.report",
                    Some(node_id),
                    &key,
                    |_seq| Ok(report.clone()),
                )? {
                    // A prior event already carries this key with a DIFFERENT payload
                    // — a real `run merge` report already completed the node. Don't
                    // claim WE completed it; the node is already terminal.
                    AppendOutcome::Conflict { .. } => Ok(Recovery::Superseded),
                    _ => Ok(Recovery::Completed),
                }
            }
            Verdict::Reject { reason } => {
                match abort_txn(paths, lock, node_id, &run_id, &txn.op_id, reason)? {
                    AppendOutcome::Conflict { .. } => Ok(Recovery::Superseded),
                    _ => Ok(Recovery::Rejected {
                        reason: reason.clone(),
                    }),
                }
            }
            Verdict::CannotVerify => Ok(Recovery::CannotVerify),
        }
    });
    outcome.unwrap_or(Recovery::CannotVerify)
}

fn recover_caller_node(paths: &RunPaths, node_id: &NodeId, git: &str) -> Recovery {
    let snapshot = RunLock::with_shared_lock(&paths.lock(), || {
        Ok((read_manifest_opt(paths)?, read_node_opt(paths, node_id)?))
    });
    let Ok((Some(manifest), Some(node))) = snapshot else {
        return Recovery::CannotVerify;
    };
    let Some(txn) = node.pending_merge.as_ref() else {
        return Recovery::NothingPending;
    };
    let Some(intent) = manifest.caller_settlement_intent else {
        return Recovery::CannotVerify;
    };
    let Ok(authority) = crate::run::merge::CallerMergeAuthority::acquire(paths, intent.clone())
    else {
        return Recovery::CannotVerify;
    };
    let Some(recorded) = verified_caller_transaction(paths, node_id, &intent, false) else {
        return Recovery::CannotVerify;
    };
    if &recorded != txn.as_ref() {
        return Recovery::CannotVerify;
    }
    if driver_is_alive(txn) {
        return Recovery::DriverAlive;
    }
    let (Some(repo), Some(source)) = (
        manifest.source_repo.as_deref(),
        manifest.source_branch.as_deref(),
    ) else {
        return Recovery::CannotVerify;
    };
    if authority.verify(&node, repo, source).is_err() {
        return Recovery::CannotVerify;
    }
    let verdict = caller_verdict(paths, node_id, &recorded, repo, git);
    if matches!(verdict, Verdict::CannotVerify) {
        return Recovery::CannotVerify;
    }
    if authority.verify(&node, repo, source).is_err() {
        return Recovery::CannotVerify;
    }
    // The run lock is held only for event replay/append. The writer lease stays
    // held across Git classification and the report, including every error path.
    let result = RunLock::with_lock(paths, |lock| {
        if authority.revalidate_unlocked().is_err() {
            return Ok(Recovery::CannotVerify);
        }
        let fresh = read_node_opt(paths, node_id)?;
        let current = read_manifest_opt(paths)?;
        if fresh.as_ref().and_then(|n| n.pending_merge.as_deref()) != Some(&recorded)
            || current
                .as_ref()
                .and_then(|m| m.caller_settlement_intent.as_ref())
                != Some(&intent)
            || fresh.as_ref().is_none_or(|n| {
                n.status.is_terminal() && n.status != taskfleet_core::Status::Failed
            })
        {
            return Ok(Recovery::Superseded);
        }
        let run_id = paths.run_id.as_str();
        match &verdict {
            Verdict::Complete => {
                let report = completion_report(&recorded);
                let key = format!("explicit-merge:{run_id}:{node_id}");
                match append_and_apply_idempotent(
                    paths,
                    lock,
                    "node.report",
                    Some(node_id),
                    &key,
                    |_seq| Ok(report.clone()),
                )? {
                    AppendOutcome::Conflict { .. } => Ok(Recovery::CannotVerify),
                    _ => Ok(Recovery::Completed),
                }
            }
            Verdict::Reject { reason } => {
                match abort_txn(paths, lock, node_id, run_id, &recorded.op_id, reason)? {
                    AppendOutcome::Conflict { .. } => Ok(Recovery::CannotVerify),
                    _ => Ok(Recovery::Rejected {
                        reason: reason.clone(),
                    }),
                }
            }
            Verdict::CannotVerify => Ok(Recovery::CannotVerify),
        }
    });
    result.unwrap_or(Recovery::CannotVerify)
}

/// Whether the `run merge` process that recorded `txn` is still alive and thus
/// still owns the transaction. Uses the recorded PID + start-time identity; when
/// no start-time was recorded (platform could not read it), a bare-PID "alive"
/// verdict is only trusted while the transaction is younger than
/// [`DRIVER_STALE_AFTER_MINS`], so a recycled PID cannot strand it forever
/// (/llm-review finding).
fn driver_is_alive(txn: &MergeTxn) -> bool {
    let Some(pid) = txn.driver_pid else {
        return false;
    };
    if pid <= 0 || !pid_live_with_identity(pid as u32, txn.driver_pid_start_secs) {
        return false;
    }
    if txn.driver_pid_start_secs.is_some() {
        // Authoritative identity: a recycled PID would have a different start-time
        // and already failed `pid_live_with_identity`.
        return true;
    }
    // No identity to rule out PID reuse — trust the bare-PID liveness only within a
    // bounded window anchored to the transaction's own start.
    Utc::now().signed_duration_since(txn.started_at) < Duration::minutes(DRIVER_STALE_AFTER_MINS)
}

/// Append the `merge.aborted` event that resolves (clears) transaction `op_id`,
/// keyed idempotently. Shared by the reject path and the terminal-node fallback.
fn abort_txn(
    paths: &RunPaths,
    lock: &LockedRun<'_>,
    node_id: &NodeId,
    run_id: &str,
    op_id: &str,
    reason: &str,
) -> taskfleet_core::Result<AppendOutcome> {
    let key = format!("merge-aborted:{run_id}:{node_id}:{op_id}");
    let data = json!({ "op_id": op_id, "reason": reason });
    append_and_apply_idempotent(
        paths,
        lock,
        taskfleet_core::KIND_MERGE_ABORTED,
        Some(node_id),
        &key,
        |_seq| Ok(data.clone()),
    )
}

/// Tri-state result of the git-verified landing check for one immutable source
/// OID: the worker's recorded content is integrated, provably absent, or git
/// could not decide (transient error / branch gone).
#[derive(PartialEq, Eq)]
enum Landing {
    Landed,
    NotLanded,
    Unverifiable,
}

/// The deterministic, OID-based verdict for a pending transaction. All git
/// evidence is read against IMMUTABLE OIDs — the recorded `expected_source_oid`
/// and `worker_oid`, and a single pinned `source_now` — never a mutable branch
/// tip, so the verdict cannot be steered by the worker branch moving after the
/// transaction was recorded (/llm-review finding).
///
/// - source ref still at `expected_source_oid` → `Reject` (git mutation never
///   landed — the no-git-mutation crash window),
/// - source ref moved AND `worker_oid`'s content is git-verified integrated into
///   it AND was NOT already integrated into `expected_source_oid` → `Complete`
///   (the move integrated OUR recorded work — the git-mutated / event-not-appended
///   window),
/// - source ref moved but `worker_oid`'s content is provably not integrated, or
///   was already present before the move → `Reject` (fail closed: unrelated move),
/// - any git read undecidable → `CannotVerify` (leave pending; never guess).
fn classify(
    txn: &MergeTxn,
    source_repo: Option<&str>,
    worktree_path: Option<&str>,
    git: &str,
) -> Verdict {
    // The repo to run git in: the durable `source_repo` (survives teardown), else
    // the worker's worktree (a linked worktree shares the common git dir). Pick the
    // first that can actually resolve the source ref, so a stale `source_repo` does
    // not permanently block recovery when the worktree still works (/llm-review).
    let (repo, source_now) =
        match resolve_source(source_repo, worktree_path, &txn.source_branch, git) {
            Some(v) => v,
            None => return Verdict::CannotVerify,
        };
    if source_now == txn.expected_source_oid {
        // Compare failed the CAS: the source ref is still exactly where we left
        // it, so the git mutation never applied. Reject, preserving the work.
        return Verdict::Reject {
            reason: "source ref unchanged — merge never landed".to_string(),
        };
    }
    // Source moved. Did OUR recorded worker content land BECAUSE of the move?
    match worker_landed(
        &repo,
        &source_now,
        &txn.worker_oid,
        txn.base_sha.as_deref(),
        git,
    ) {
        Landing::Unverifiable => Verdict::CannotVerify,
        Landing::NotLanded => Verdict::Reject {
            reason: "source ref moved but the worker's recorded work is not integrated".to_string(),
        },
        Landing::Landed => {
            // The content is in `source_now`. Confirm the MOVE integrated it — i.e.
            // it was NOT already present in `expected_source_oid` — otherwise an
            // unrelated commit over an already-integrated branch would falsely
            // complete an empty merge (/llm-review finding).
            match worker_landed(
                &repo,
                &txn.expected_source_oid,
                &txn.worker_oid,
                txn.base_sha.as_deref(),
                git,
            ) {
                Landing::Landed => Verdict::Reject {
                    reason: "worker content was already integrated before the merge; the source \
                             move was unrelated"
                        .to_string(),
                },
                Landing::NotLanded => Verdict::Complete,
                Landing::Unverifiable => Verdict::CannotVerify,
            }
        }
    }
}

/// Resolve the source ref against the first repo that can read it: prefer the
/// durable `source_repo`, fall back to the worker's worktree. Returns
/// `(repo, source_oid)` or `None` if neither can resolve the ref.
fn resolve_source(
    source_repo: Option<&str>,
    worktree_path: Option<&str>,
    source_branch: &str,
    git: &str,
) -> Option<(String, String)> {
    [source_repo, worktree_path]
        .into_iter()
        .flatten()
        .find_map(|repo| read_oid(git, repo, source_branch).map(|oid| (repo.to_string(), oid)))
}

/// Git-verified landing of the IMMUTABLE `worker_oid` into `source_rev` (also an
/// immutable OID), bounded by `base_sha`. Rebase-robust (patch-id) via
/// [`landing_signal`]. `report: None`, so the only verdicts are the authoritative
/// `GitVerified` (→ `Landed`/`NotLanded`) or `Unverified` (→ `Unverifiable`); a
/// report-marker can never fabricate a landing here.
fn worker_landed(
    repo: &str,
    source_rev: &str,
    worker_oid: &str,
    base_sha: Option<&str>,
    git: &str,
) -> Landing {
    let inputs = LandingInputs {
        source_repo: Some(repo),
        source_branch: Some(source_rev),
        // Force git through `source_repo`; never fall back to a mutable worktree.
        worktree_path: None,
        branch: Some(worker_oid),
        base_sha,
        report: None,
    };
    let signal = landing_signal(&inputs, git);
    match signal.method {
        LandedMethod::GitVerified if signal.landed => Landing::Landed,
        LandedMethod::GitVerified => Landing::NotLanded,
        // `ReportMarker` (impossible with `report: None`) or `Unverified`.
        _ => Landing::Unverifiable,
    }
}

/// The terminal `explicit-merge` report recovery appends to complete a
/// transaction. Mirrors the minimal report `run merge` synthesizes (a clean
/// merge is a success), plus a note that it was recovered.
fn completion_report(txn: &MergeTxn) -> Value {
    let mut report = json!({
        "success": true,
        "summary": format!(
            "recovered crashed merge of {} into {} (op {})",
            txn.worker_branch, txn.source_branch, txn.op_id
        ),
        "via": VIA_EXPLICIT_MERGE,
    });
    // Recovery completes the SAME run-merge transaction, so it stamps the same
    // typed `RunMerge` origin the live merge path would have (issue
    // `typed-report-origin`), carrying the transaction's immutable OIDs.
    taskfleet_core::ReportOrigin::RunMerge {
        op_id: Some(txn.op_id.clone()),
        worker_oid: Some(txn.worker_oid.clone()),
    }
    .stamp(&mut report);
    // Validate the §7.3 shape UNCONDITIONALLY (not `debug_assert!`) so a future
    // edit to this constant-shaped payload cannot silently append an invalid report
    // in a release build (/llm-review finding). It never fails at runtime today.
    validate_report_payload(&report)
        .expect("recovery completion report is a constant, valid §7.3 payload");
    report
}

/// `git -C <repo> rev-parse <rev>` → the full object id, or `None` on any git
/// error / non-SHA output. Rejects an option-injection-shaped ref.
pub(crate) fn read_oid(git: &str, repo: &str, rev: &str) -> Option<String> {
    if repo.is_empty() || rev.is_empty() || rev.starts_with('-') {
        return None;
    }
    let out = std::process::Command::new(git)
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--end-of-options", rev])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let ok = matches!(sha.len(), 40 | 64) && sha.chars().all(|c| c.is_ascii_hexdigit());
    ok.then_some(sha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    use serde_json::json;
    use taskfleet_core::{append_and_apply_event, read_node_opt, Status};

    use crate::supervise::cleanup::git_bin;

    fn git(repo: &Path, args: &[&str]) -> String {
        let out = Command::new(git_bin())
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A git repo with `main` at a base commit and a `wt/worker` branch carrying
    /// one commit of real work. Returns `(dir, base_sha, worker_tip)`.
    fn repo_with_worker() -> (tempfile::TempDir, String, String) {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.email", "t@t"]);
        git(repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("f"), "base\n").unwrap();
        git(repo, &["add", "f"]);
        git(repo, &["commit", "-qm", "base"]);
        let base = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "-q", "-b", "wt/worker"]);
        std::fs::write(repo.join("f"), "base\nwork\n").unwrap();
        git(repo, &["commit", "-qam", "worker work"]);
        let worker_tip = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "-q", "main"]);
        (dir, base, worker_tip)
    }

    fn fresh_run(tmp: &tempfile::TempDir, repo: &Path) -> RunPaths {
        let run_id = "01jxsnap000000000000000000";
        let dir = tmp.path().join(run_id);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = RunPaths::new(dir, run_id).unwrap();
        append_and_apply_event(
            &paths,
            "run.created",
            None,
            None,
            json!({
                "kind": "spinoff",
                "lifecycle": "autonomous",
                "title": "t",
                "source_branch": "main",
                "source_repo": repo.to_str().unwrap(),
            }),
        )
        .unwrap();
        append_and_apply_event(
            &paths,
            "node.created",
            Some(&NodeId::parse_str("n-0001").unwrap()),
            None,
            json!({
                "kind": "spinoff",
                "worktree_path": repo.to_str().unwrap(),
                "branch": "wt/worker",
            }),
        )
        .unwrap();
        paths
    }

    fn record_txn(paths: &RunPaths, expected_source_oid: &str, worker_oid: &str, base: &str) {
        let txn = json!({
            "op_id": "01jxop00000000000000000000",
            "source_branch": "main",
            "worker_branch": "wt/worker",
            "expected_source_oid": expected_source_oid,
            "worker_oid": worker_oid,
            "base_sha": base,
            // A definitely-dead driver pid so recovery is not gated on liveness.
            "driver_pid": 2_000_000_000,
            "driver_pid_start_secs": null,
            "started_at": "2026-08-15T00:00:00Z",
        });
        append_and_apply_event(
            paths,
            taskfleet_core::KIND_MERGE_STARTED,
            Some(&NodeId::parse_str("n-0001").unwrap()),
            None,
            txn,
        )
        .unwrap();
    }

    #[test]
    fn caller_transaction_without_sticky_intent_cannot_be_recovered() {
        let (repo_dir, base, worker_tip) = repo_with_worker();
        let repo = repo_dir.path();
        git(repo, &["merge", "-q", "--ff-only", "wt/worker"]);
        let home = tempfile::TempDir::new().unwrap();
        let dir = home.path().join("01jxsnap000000000000000000");
        std::fs::create_dir_all(&dir).unwrap();
        let paths = RunPaths::new(dir, "01jxsnap000000000000000000").unwrap();
        append_and_apply_event(
            &paths,
            "run.created",
            None,
            None,
            json!({
                "kind":"spinoff", "lifecycle":"interactive", "agent_owner":"caller",
                "title":"t", "source_branch":"main", "source_repo":repo.to_str().unwrap()
            }),
        )
        .unwrap();
        let nid = NodeId::parse_str("n-0001").unwrap();
        append_and_apply_event(
            &paths,
            "node.created",
            Some(&nid),
            None,
            json!({
                "kind":"spinoff", "worktree_path":repo.to_str().unwrap(), "branch":"wt/worker"
            }),
        )
        .unwrap();
        // The reducer now refuses an unlinked caller transaction at append
        // time, even when Git already moved. Recovery still refuses this run.
        let bad = json!({
            "op_id":"01jxop00000000000000000000", "source_branch":"main",
            "worker_branch":"wt/worker", "expected_source_oid":base,
            "worker_oid":worker_tip, "started_at":"2026-08-15T00:00:00Z"
        });
        assert!(append_and_apply_event(
            &paths,
            taskfleet_core::KIND_MERGE_STARTED,
            Some(&nid),
            None,
            bad
        )
        .is_err());
        assert_eq!(
            recover_node(&paths, &nid, &git_bin()),
            Recovery::NothingPending
        );
        let node = read_node_opt(&paths, &nid).unwrap().unwrap();
        assert!(node.pending_merge.is_none());
        assert!(!node.status.is_terminal());
    }

    /// Crash window 1: git was mutated (worker's work fast-forwarded into `main`)
    /// but the terminal `explicit-merge` report was never appended. Recovery must
    /// COMPLETE the transaction — terminalize the node `Done` with the merge marker.
    #[test]
    fn recovers_git_mutated_event_not_appended_by_completing() {
        let (repo_dir, base, worker_tip) = repo_with_worker();
        let repo = repo_dir.path();
        // The merge landed: main fast-forwards to the worker's tip. `expected` is
        // the pre-merge source OID (base), so source has moved off it.
        git(repo, &["merge", "-q", "--ff-only", "wt/worker"]);
        assert_ne!(git(repo, &["rev-parse", "main"]), base);

        let home = tempfile::TempDir::new().unwrap();
        let paths = fresh_run(&home, repo);
        record_txn(&paths, &base, &worker_tip, &base);
        let nid = NodeId::parse_str("n-0001").unwrap();
        // Precondition: the transaction is pending and the node is live.
        let n = read_node_opt(&paths, &nid).unwrap().unwrap();
        assert!(n.pending_merge.is_some());
        assert_eq!(n.status, Status::Pending);

        let outcome = recover_node(&paths, &nid, &git_bin());
        assert_eq!(outcome, Recovery::Completed);

        let n = read_node_opt(&paths, &nid).unwrap().unwrap();
        assert_eq!(n.status, Status::Done, "completed merge terminalizes Done");
        assert!(
            n.pending_merge.is_none(),
            "transaction cleared on completion"
        );
        let via = n
            .last_report
            .as_ref()
            .and_then(|r| r.get("via"))
            .and_then(|v| v.as_str());
        assert_eq!(
            via,
            Some(VIA_EXPLICIT_MERGE),
            "explicit-merge report adopted"
        );
    }

    /// Crash window 2: `merge.started` was recorded but git was never mutated (the
    /// source ref is still exactly `expected_source_oid`). Recovery must REJECT the
    /// transaction — fail closed, clear it, and leave the node live with its work
    /// preserved (never fabricate a completion).
    #[test]
    fn rejects_merge_started_with_no_git_mutation() {
        let (repo_dir, base, worker_tip) = repo_with_worker();
        let repo = repo_dir.path();
        // No merge happened: main is still at base == expected_source_oid.
        assert_eq!(git(repo, &["rev-parse", "main"]), base);

        let home = tempfile::TempDir::new().unwrap();
        let paths = fresh_run(&home, repo);
        record_txn(&paths, &base, &worker_tip, &base);
        let nid = NodeId::parse_str("n-0001").unwrap();

        let outcome = recover_node(&paths, &nid, &git_bin());
        assert!(
            matches!(outcome, Recovery::Rejected { .. }),
            "no git mutation → reject, got {outcome:?}"
        );

        let n = read_node_opt(&paths, &nid).unwrap().unwrap();
        assert_eq!(n.status, Status::Pending, "rejected merge leaves node live");
        assert!(
            n.pending_merge.is_none(),
            "transaction cleared on rejection"
        );
        // The worker branch is untouched — its work is preserved.
        assert_eq!(git(repo, &["rev-parse", "wt/worker"]), worker_tip);
    }

    /// A live driver still owns the transaction — recovery must not race it.
    #[test]
    fn leaves_transaction_when_driver_alive() {
        let (repo_dir, base, worker_tip) = repo_with_worker();
        let repo = repo_dir.path();
        let home = tempfile::TempDir::new().unwrap();
        let paths = fresh_run(&home, repo);
        // Record with OUR pid as the driver — very much alive.
        let pid = std::process::id();
        let txn = json!({
            "op_id": "01jxop00000000000000000000",
            "source_branch": "main",
            "worker_branch": "wt/worker",
            "expected_source_oid": base,
            "worker_oid": worker_tip,
            "base_sha": base,
            "driver_pid": pid as i32,
            "driver_pid_start_secs": crate::supervise::watchdog::pid_start_time(pid),
            "started_at": "2026-08-15T00:00:00Z",
        });
        append_and_apply_event(
            &paths,
            taskfleet_core::KIND_MERGE_STARTED,
            Some(&NodeId::parse_str("n-0001").unwrap()),
            None,
            txn,
        )
        .unwrap();
        let nid = NodeId::parse_str("n-0001").unwrap();
        assert_eq!(
            recover_node(&paths, &nid, &git_bin()),
            Recovery::DriverAlive
        );
        assert!(read_node_opt(&paths, &nid)
            .unwrap()
            .unwrap()
            .pending_merge
            .is_some());
    }

    /// Source moved, but NOT to the worker's content (an unrelated commit landed on
    /// main). Recovery must fail closed — REJECT, never fabricate a completion.
    #[test]
    fn rejects_when_source_moved_without_worker_content() {
        let (repo_dir, base, worker_tip) = repo_with_worker();
        let repo = repo_dir.path();
        // An unrelated commit advances main; the worker's work is NOT integrated.
        std::fs::write(repo.join("g"), "unrelated\n").unwrap();
        git(repo, &["add", "g"]);
        git(repo, &["commit", "-qm", "unrelated"]);
        assert_ne!(git(repo, &["rev-parse", "main"]), base);

        let home = tempfile::TempDir::new().unwrap();
        let paths = fresh_run(&home, repo);
        record_txn(&paths, &base, &worker_tip, &base);
        let nid = NodeId::parse_str("n-0001").unwrap();

        let outcome = recover_node(&paths, &nid, &git_bin());
        assert!(
            matches!(outcome, Recovery::Rejected { .. }),
            "unrelated move → fail closed, got {outcome:?}"
        );
        let n = read_node_opt(&paths, &nid).unwrap().unwrap();
        assert_eq!(n.status, Status::Pending);
        assert!(n.pending_merge.is_none());
    }

    /// Completion is verified against the IMMUTABLE recorded `worker_oid`, not the
    /// current `worker_branch` tip: even after the worker branch is moved to
    /// unrelated content, a genuinely-landed transaction still completes.
    #[test]
    fn completion_uses_recorded_worker_oid_not_mutable_branch() {
        let (repo_dir, base, worker_tip) = repo_with_worker();
        let repo = repo_dir.path();
        // The merge landed: main FF's to the worker's tip.
        git(repo, &["merge", "-q", "--ff-only", "wt/worker"]);
        // Now move wt/worker to a NEW unrelated commit — the recorded worker_oid
        // must remain the source of truth.
        git(repo, &["checkout", "-q", "wt/worker"]);
        std::fs::write(repo.join("h"), "moved\n").unwrap();
        git(repo, &["add", "h"]);
        git(repo, &["commit", "-qm", "worker moved on"]);
        git(repo, &["checkout", "-q", "main"]);
        assert_ne!(git(repo, &["rev-parse", "wt/worker"]), worker_tip);

        let home = tempfile::TempDir::new().unwrap();
        let paths = fresh_run(&home, repo);
        record_txn(&paths, &base, &worker_tip, &base);
        let nid = NodeId::parse_str("n-0001").unwrap();

        assert_eq!(recover_node(&paths, &nid, &git_bin()), Recovery::Completed);
        assert_eq!(
            read_node_opt(&paths, &nid).unwrap().unwrap().status,
            Status::Done
        );
    }

    /// Fail-closed guard: the worker's content was ALREADY integrated into the
    /// expected source before the transaction (an empty merge). An unrelated source
    /// move must NOT be read as "our merge landed" — recovery rejects.
    #[test]
    fn rejects_when_worker_content_already_in_expected_source() {
        let (repo_dir, base, worker_tip) = repo_with_worker();
        let repo = repo_dir.path();
        // The worker's content is already in main (merged earlier); `expected` is
        // main AT that point.
        git(repo, &["merge", "-q", "--ff-only", "wt/worker"]);
        let expected = git(repo, &["rev-parse", "main"]);
        assert_eq!(expected, worker_tip);
        // An unrelated commit then advances main.
        std::fs::write(repo.join("g"), "unrelated\n").unwrap();
        git(repo, &["add", "g"]);
        git(repo, &["commit", "-qm", "unrelated"]);

        let home = tempfile::TempDir::new().unwrap();
        let paths = fresh_run(&home, repo);
        // base_sha is the fork point (base) so the delta is bounded to the worker's
        // own commit; the worker content is already present in `expected`.
        record_txn(&paths, &expected, &worker_tip, &base);
        let nid = NodeId::parse_str("n-0001").unwrap();

        let outcome = recover_node(&paths, &nid, &git_bin());
        assert!(
            matches!(outcome, Recovery::Rejected { .. }),
            "content already in expected → no fabricated completion, got {outcome:?}"
        );
        assert_eq!(
            read_node_opt(&paths, &nid).unwrap().unwrap().status,
            Status::Pending
        );
    }

    /// When git cannot be consulted (repo path gone), recovery is `CannotVerify` and
    /// leaves the transaction pending — it never rejects or completes on a guess.
    #[test]
    fn cannot_verify_leaves_transaction_pending() {
        let (repo_dir, base, worker_tip) = repo_with_worker();
        let repo = repo_dir.path().to_path_buf();
        let home = tempfile::TempDir::new().unwrap();
        let paths = fresh_run(&home, &repo);
        record_txn(&paths, &base, &worker_tip, &base);
        let nid = NodeId::parse_str("n-0001").unwrap();
        // Remove the repo so no git read can resolve the source ref.
        drop(repo_dir);

        assert_eq!(
            recover_node(&paths, &nid, &git_bin()),
            Recovery::CannotVerify
        );
        assert!(read_node_opt(&paths, &nid)
            .unwrap()
            .unwrap()
            .pending_merge
            .is_some());
    }
}
