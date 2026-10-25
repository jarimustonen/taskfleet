---
created: 2026-10-25
updated: 2026-10-25
type: bug
status: untriaged
priority: normal
provenance: ai-review
source_ref: taskfleet:01m5t7sh2k42k77drb89tcsgx0/review-finding:tmux-server-identity-seconds
review_source: ai-review
originating_run: 01m5t7sh2k42k77drb89tcsgx0
originating_run_kind: spinoff
labels:
- ai-review-model:gpt-6-astra
- ai-review-model:gpt-6.1-sol
assessment_classification: CONFIRMED
assessment_outcome: SPIN_OFF
review_confidence: high
review_severity: should-fix
review_target: 7026d09f
---

# Retention reads a live tmux server as gone after a wall-clock step

## Description

## Problem

`crates/taskfleet/src/session.rs` `server_identity` (around line 1086) decides whether the recorded tmux server is still the same process by comparing `watchdog::pid_start_time(pid) != Some(start)` exactly against `tmux_server_pid_start_secs` recorded at run create. On Linux that value is sysinfo's `btime + starttime/HZ`, and `/proc/stat` `btime` shifts whenever NTP steps the wall clock. On Trude (2026-10-25) btime fell 403 → 398 over about three hours, with timesyncd pinned at +500 ppm.

## Effect

Once the clock has drifted by 1 s, a still-running tmux server reads as `IdentityCheck::Gone`:

- `retain_completed_display` records retention unavailable ("recorded tmux server generation is gone"), and cleanup continues without the retained display the run opted into (`supervise/cleanup.rs` treats `Retention::Unavailable` like `Retained`).
- `session maintain` maps it to `Inspect::ServerGone` and calls `record_expired_locked` for a retained display that still exists, leaving an orphaned display that no longer counts against the bounds.

No branch or worktree is lost (window close still requires `IdentityCheck::Exact`). The cost is lost retention and untracked displays on any host whose clock gets stepped.

## Fix direction

`supervisor-false-pid-recycled` moved agent, supervisor-pid-file, and merge-driver identity to the kernel start identity (`watchdog::pid_start_identity`, Linux `/proc/<pid>/stat` starttime ticks) via `watchdog::RecordedStart`. Do the same for `TmuxIdentity` / `RetainedDisplay` (`server_pid_start_identity`), keep seconds only as the legacy fallback, and treat an unreadable identity as `Unsafe` rather than `Gone`.
