//! Supervisor duties for a named resident run (ADR 0003).
//!
//! Two non-terminal duties, both keyed on told facts:
//!
//! - **Round notification.** When the worker records `node.round_reported`,
//!   fire the run's `--notify` hook once for that report
//!   (`TASKFLEET_STATUS=round-reported`).
//! - **Idle expiry.** Once the open round report is older than the run's
//!   `idle_expiry_secs` and the reporting worker has exited, take the resident
//!   operation lock (without waiting; a busy run is left for the next tick)
//!   and apply invariant 5's fail-closed rule. A wholly landed, clean worktree
//!   is removed (`node.resident_dormant`): the branch, the Pi transcript, and
//!   the Claude session store stay, and the next `run message` recreates the
//!   worktree at the same path. Anything else is preserved and raises
//!   attention once (`node.resident_attention`). Expiry never merges and never
//!   terminalizes.
//!
//! The deadline is a timer on the told round report, not an activity clock:
//! nothing here looks at panes, output, or CPU.

use chrono::Utc;
use serde_json::json;
use tracing::{info, warn};

use taskfleet_core::{
    append_and_apply_event, read_manifest_opt, read_node_opt, NodeId, RunLock, RunPaths,
};

use super::cleanup;

/// One resident tick. `round_notified` is the in-process fast path for the
/// round notification; the durable marker is the restart truth.
pub(crate) fn tick(paths: &RunPaths, run_id: &str, round_notified: &mut Option<u64>) {
    let nid = NodeId::parse_str("n-0001").expect("valid default node id");
    let snapshot = RunLock::with_shared_lock(&paths.lock(), || {
        Ok((read_manifest_opt(paths)?, read_node_opt(paths, &nid)?))
    });
    let Ok((Some(manifest), Some(node))) = snapshot else {
        return;
    };
    let Some(policy) = manifest.resident.as_ref() else {
        return;
    };
    if manifest.status.is_terminal() || node.status.is_terminal() {
        return;
    }
    let Some(open) = node
        .resident
        .as_deref()
        .and_then(taskfleet_core::ResidentState::open_round)
        .cloned()
    else {
        return;
    };

    if *round_notified != Some(open.event_seq)
        && super::notify::maybe_fire_round_reported(
            paths,
            run_id,
            manifest.notify_cmd.as_deref(),
            open.event_seq,
            crate::run::kind_kebab(manifest.kind),
            &manifest.title,
        )
    {
        *round_notified = Some(open.event_seq);
    }

    let state = node.resident.as_deref().expect("open round implies state");
    if state.dormant_since.is_some() || state.expiry_attention_at.is_some() {
        return;
    }
    let Some(deadline) = crate::run::resident::idle_deadline(policy, &node) else {
        return;
    };
    if Utc::now() < deadline {
        return;
    }
    let Ok(Some(_lock)) = crate::run::resident::ResidentLock::try_acquire(paths) else {
        return;
    };
    // Re-read under the resident lock: a message may have resumed the worker,
    // or a land changed what is unlanded, since the snapshot.
    let Ok(Some(node)) = RunLock::with_shared_lock(&paths.lock(), || read_node_opt(paths, &nid))
    else {
        return;
    };
    let still_due = !node.status.is_terminal()
        && node.resident.as_deref().is_some_and(|s| {
            s.dormant_since.is_none()
                && s.expiry_attention_at.is_none()
                && s.open_round()
                    .is_some_and(|r| r.event_seq == open.event_seq)
        })
        && crate::run::resident::idle_deadline(policy, &node).is_some_and(|d| Utc::now() >= d);
    if !still_due {
        return;
    }
    let (kind, data) = match cleanup::remove_landed_resident_worktree(
        paths,
        &node,
        &cleanup::tmux_bin(),
        &cleanup::git_bin(),
    ) {
        Ok(()) => {
            info!(target: "taskfleet::supervise", run_id, "resident idle expiry: landed worktree removed; session kept");
            (
                "node.resident_dormant",
                json!({"reason": "idle-expired", "round": open.round,
                    "worktree_path": node.worktree_path}),
            )
        }
        Err(reason) => {
            info!(target: "taskfleet::supervise", run_id, reason, "resident idle expiry: work preserved; attention raised");
            (
                "node.resident_attention",
                json!({"reason": reason, "round": open.round}),
            )
        }
    };
    let key = format!("resident-expiry:{run_id}:{}", open.event_seq);
    if let Err(e) = append_and_apply_event(paths, kind, Some(&nid), Some(&key), data) {
        warn!(target: "taskfleet::supervise", run_id, error = %e, "could not record resident idle expiry");
    }
}
