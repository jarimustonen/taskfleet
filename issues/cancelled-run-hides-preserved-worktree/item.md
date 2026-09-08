---
created: 2026-09-06
updated: 2026-09-08
type: bug
reporter: jari
status: in-progress
priority: normal
provenance: other
provenance_detail: Recovered from a pre-convergence Haapa intake checkout during the final Taskfleet filesystem inventory
source_ref: haapa:intake-recovery:cancelled-run-preserved-worktree:2026-09-06
lane: terminal-work-disposition
lane_seq: 10
---

# Cancelled run hides preserved dirty worktree

## Description

## Description

Cancelled run hides preserved dirty worktree.

## Observed

Two cancelled single-worker Taskfleet runs remained registered as Git worktrees with uncommitted content after cancellation:

- `01m1vdtqn42hzadxhga0xc35ma`
- `01m1dyaw2xhz66xj9wwnkcfs2q`

For both runs, `taskfleet run show <run-id> --output json` reported:

```json
{
  "status": "cancelled",
  "landed": false,
  "landed_method": "unverified",
  "recoverable_work": null,
  "supervisor": {"state": "not-recorded", "alive": false}
}
```

The corresponding paths still appeared in `git worktree list`. `git status --short` inside each showed uncommitted or untracked content. Repeating `taskfleet run cancel <run-id> --json` returned `already_cancelled: true` and did not expose or clean up the preserved worktree.

One worktree retained modified/staged Taskfleet launcher files; the other retained untracked Nextcloud infrastructure and an encrypted secret scaffold. They were discovered only because Homebase's terminal handoff independently compared Taskfleet state with `git worktree list`.

The owner reviewed the remnants, confirmed both were superseded, and explicitly approved force-removing the worktrees and deleting their local branches. No data loss incident occurred.

## Expected

A terminal cancelled run that still owns a dirty or otherwise preserved worktree must report that state explicitly. `recoverable_work` must not be null when the registered worktree contains changes. The run surface should provide an actionable, safe disposition path for salvage versus explicit discard, and terminal handoff tooling should not need raw Git archaeology to discover the hold.

If cancellation intentionally preserves dirty work, document and expose that as first-class state. If it should tear down automatically, cancellation must either complete teardown or report why it refused. An idempotent repeated cancel must not look fully settled while silently retaining owned mutable state.

## Reproduction

1. Start a single-worker run and create uncommitted changes in its worktree.
2. Cancel the run before it reports or merges.
3. Run `taskfleet run show <id> --output json` and `git worktree list`.
4. Observe whether the worktree remains while `recoverable_work` is null.
5. Repeat `taskfleet run cancel <id> --output json` and observe that it reports already cancelled without resolving or surfacing the retained state.

## Decisions

### 2026-09-07T18:15:48Z · @jari

Accepted the joint recommendation in `analysis.md`: expose retained terminal resources through `run show.data.preserved_work`, then add an explicit, audited, idempotent `run discard` operation. Keep `run salvage` as the merge path, keep statuses/history unchanged, require explicit force for verified dirty work, and fail closed for unverifiable state. Implement visibility before disposition.

## Agent Runs

### 2026-09-08T10:45:09Z · @codex

Implementation uses one live retained-resource observer for both run show and discard; no persisted Git observation or lifecycle state. Destructive review depth: focused independent review because discard crosses process-identity and irreversible Git boundaries. Review findings were incorporated: NUL-safe exact worktree registration, common-dir binding including stale-path replacement refusal, detached-HEAD commit observation, projection catch-up/read-only dry-run, authorization payload conflict checks, and current-vs-historical multi-node selection.
