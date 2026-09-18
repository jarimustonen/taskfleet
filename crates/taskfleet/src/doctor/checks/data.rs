//! `data.orphan-supervisor.<id>` — no run carries a `supervisor.pid` that
//! points at a dead process (AGENTS-AI-FIRST-CLI §18 "data integrity").
//!
//! A live supervisor is healthy. A dead PID is a stale marker left by a
//! crashed supervisor: the run looks supervised but nothing is driving
//! it. We `WARN` and suggest `run reattach`. When the marker is clearly
//! abandoned — its file is more than 24h old — `--fix` may remove it
//! (the §18 safe subset); a fresher dead PID is left alone because the
//! supervisor may be mid-restart.

use std::path::Path;
use std::time::Duration;

use crate::doctor::check::{CheckResult, FixAction};
use crate::supervise::pid_file;

use super::Ctx;

/// A dead PID file older than this is considered safe to auto-remove.
const STALE_AGE: Duration = Duration::from_hours(24);

pub fn check(ctx: &Ctx) -> Vec<CheckResult> {
    let Some(root) = ctx.root.as_deref() else {
        return Vec::new();
    };
    let runs_dir = root.join("runs");

    let entries = match std::fs::read_dir(&runs_dir) {
        Ok(e) => e,
        // Missing runs dir is the empty state; schema.runs already says so.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        // Any other read failure (permissions, IO) is a real data-integrity
        // blind spot — surface it rather than silently passing the category.
        Err(e) => {
            return vec![CheckResult::fail(
                "data.orphan-supervisor",
                format!("cannot read {}: {e}", runs_dir.display()),
                format!("check permissions on {}", runs_dir.display()),
            )];
        }
    };

    let mut run_ids: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect();
    run_ids.sort();

    let mut out = Vec::new();
    for run_id in &run_ids {
        // The orphan-supervisor check is pure path + PID logic, so it runs for
        // every directory regardless of whether its name is a valid run id — a
        // stale pid in an oddly-named dir must still be caught. (Invalid dir
        // names are surfaced by the schema check.)
        let pid_path = runs_dir.join(run_id).join("supervisor.pid");
        let id = format!("data.orphan-supervisor.{run_id}");

        // Distinguish "no PID file" (nothing to orphan-check) from "PID
        // file present but unparseable" (data corruption worth a WARN).
        if !pid_path.exists() {
            continue;
        }
        let Some(pid) = pid_file::read_pid(&pid_path) else {
            out.push(CheckResult::warn(
                id,
                format!(
                    "supervisor pid file {} is unreadable/unparseable",
                    pid_path.display()
                ),
                format!("inspect or rm {}", pid_path.display()),
            ));
            continue;
        };
        if pid_file::pid_alive(pid) {
            out.push(CheckResult::ok(id, format!("supervisor pid {pid} alive")));
            continue;
        }

        // Dead PID. Suggest reattach; offer auto-removal only when the
        // marker is clearly abandoned (>24h old). The applier re-validates
        // the PID just before removal, so the staleness here is a gate on
        // *offering* the fix, not the final safety check.
        let suggestion = format!(
            "taskfleet run reattach {run_id} (or rm {})",
            pid_path.display()
        );
        let mut result = CheckResult::warn(
            id,
            format!(
                "supervisor pid {pid} is dead (stale {})",
                pid_path.display()
            ),
            suggestion,
        );
        if pid_file_is_stale(&pid_path) {
            result = result.with_safe_fix(FixAction::RemoveStaleSupervisorPid {
                path: pid_path,
                observed_pid: pid,
            });
        }
        out.push(result);
    }
    out
}

/// True when the PID file's mtime is older than [`STALE_AGE`]. A clock
/// error or unreadable mtime conservatively returns false (do not
/// auto-remove).
fn pid_file_is_stale(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|mtime| mtime.elapsed().ok())
        .is_some_and(|age| age > STALE_AGE)
}
