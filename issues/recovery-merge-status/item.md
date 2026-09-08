---
created: 2026-09-08
updated: 2026-09-08
type: bug
status: fixed
priority: normal
provenance: agent:homebase-wrapup
source_ref: homebase:2026-09-07/taskfleet-recovery-status
originating_run: 01m1zhvsh452b1dphmpnwzdh0c
originating_run_kind: spinoff
lane: terminal-work-disposition
lane_seq: 30
blocked_by: ['@intake-feature-taskfleet-41343c4dd3e4']
collision: [crates/taskfleet-core/src/reducer.rs, crates/taskfleet/src/supervise/cleanup.rs]
closed: 2026-09-08
closed_by: codex
---

# Recovery merge leaves landed run failed

## Description

## Observed
Taskfleet run `01m1xd8tvgqvvf0281btqn04gt` first submitted a required `success:false` node report after `taskfleet run merge` was blocked by an unrelated dirty target worktree. After preserving and committing the unrelated file, the conductor reran `taskfleet run merge 01m1xd8tvgqvvf0281btqn04gt --report-file <success-report> --output json`. The command returned `merged: true`, restarted the supervisor, removed the worktree and branch, persisted a new `success:true` explicit-merge report, and `run show` reports `landed: true` with `landed_method: report-marker`. However `.data.manifest.status` remains `failed`.

## Expected
A supported recovery merge that lands and records a successful terminal report should reconcile the public run status to `done`, or the CLI should reject recovery from a failed terminal state before merging. It must not expose the contradictory durable state `status: failed`, `landed: true`, `report.success: true` after successful cleanup.

## Impact
Status consumers and handoff preflight can classify successfully recovered work as a failed run even though no recoverable worktree, branch, or ownership remains.

## Triage analysis

Manually reproduced and confirmed against production run `01m1xd8tvgqvvf0281btqn04gt`. The final public state is contradictory: run `failed`, node `done`, `landed:true`, and a confirmed successful `run-merge` report after cleanup. Source inspection confirms that late merge adoption intentionally repairs the node, while the terminal guard in run roll-up intentionally leaves an already failed manifest unchanged. The focused existing regression test permits this recovery merge but does not assert its final run status. See `analysis.md` for the event sequence, code-path distinction, test evidence, and recommended narrow `Failed` → `Done` recovery contract.

## Resolution

### 2026-09-08T12:08:41Z · @codex

Implemented and verified narrow Failed -> Done recovery after an adopted authoritative run-merge report and wholly successful own/linked-child topology. Cancelled runs, forged/malformed/false reports, failed/live siblings, and non-successful children remain immutable/blocking. Five required gates, canonical identity check, local release build/skill print, explicit native dependency fixtures, and iterative independent concurrency/authority review all passed.
