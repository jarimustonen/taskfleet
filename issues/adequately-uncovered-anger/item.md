---
created: 2026-09-08
updated: 2026-09-08
type: bug
status: untriaged
priority: normal
provenance: agent:homebase-wrapup
source_ref: homebase:2026-09-07/taskfleet-recovery-status
originating_run: 01m1zhvsh452b1dphmpnwzdh0c
originating_run_kind: spinoff
---

# Recovery merge leaves landed run failed

## Description

## Observed
Taskfleet run `01m1xd8tvgqvvf0281btqn04gt` first submitted a required `success:false` node report after `taskfleet run merge` was blocked by an unrelated dirty target worktree. After preserving and committing the unrelated file, the conductor reran `taskfleet run merge 01m1xd8tvgqvvf0281btqn04gt --report-file <success-report> --output json`. The command returned `merged: true`, restarted the supervisor, removed the worktree and branch, persisted a new `success:true` explicit-merge report, and `run show` reports `landed: true` with `landed_method: report-marker`. However `.data.manifest.status` remains `failed`.

## Expected
A supported recovery merge that lands and records a successful terminal report should reconcile the public run status to `done`, or the CLI should reject recovery from a failed terminal state before merging. It must not expose the contradictory durable state `status: failed`, `landed: true`, `report.success: true` after successful cleanup.

## Impact
Status consumers and handoff preflight can classify successfully recovered work as a failed run even though no recoverable worktree, branch, or ownership remains.
