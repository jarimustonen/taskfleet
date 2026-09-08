---
created: 2026-09-02
updated: 2026-09-08
type: feature
reporter: jari
status: done
priority: normal
provenance: agent:3dbear-stint-handoff
source_ref: agent:3dbear-stint-handoff/reporter:jari/id:failed-run-preserved-worktree-teardown-20260902
lane: terminal-work-disposition
lane_seq: 20
blocked_by: ['@cancelled-run-hides-preserved-worktree']
closed: 2026-09-08
closed_by: codex
---

# Add teardown for terminal failed runs with preserved worktrees

## Description

Add teardown for terminal failed runs with preserved worktrees

## Observed

Three superseded runs were terminal `failed`, had no live supervisor, and retained clean worktrees and branches. `taskfleet run cancel` could not relinquish or remove them:

```text
taskfleet run cancel 01m1em8me7kq1pbapjfrsxnfx1 --output json
{"error":{"code":"run_already_terminal","message":"run is failed, cannot cancel",...}}
```

`run salvage` was inappropriate because the branches were intentionally superseded and must not be merged. The only available cleanup was manual `git worktree remove` plus `git branch -D`, which leaves taskfleet's manifest unable to record the explicit abandonment/cleanup decision.

## Expected

Provide a safe command such as `taskfleet run abandon <run-id>` or `run cleanup <run-id>` for terminal failed runs. It should:

- require a terminal failed/cancelled state and no live worker;
- refuse dirty worktrees unless an explicit reviewed override is supplied;
- remove the preserved worktree and branch without merging;
- retain the run manifest, report, and event history;
- record who abandoned it, when, and why;
- be idempotent and return structured JSON;
- clearly distinguish cleanup from cancellation and salvage.

This is needed by terminal handoff workflows so superseded failures can be closed without direct git surgery.

## Decisions

### 2026-09-07T18:15:48Z · @jari

Accepted the joint recommendation in `analysis.md`: expose retained terminal resources through `run show.data.preserved_work`, then add an explicit, audited, idempotent `run discard` operation. Keep `run salvage` as the merge path, keep statuses/history unchanged, require explicit force for verified dirty work, and fail closed for unverifiable state. Implement visibility before disposition.

## Agent Runs

### 2026-09-08T10:45:09Z · @codex

Cohesive disposition implementation keeps salvage, cancel, status, landed, and TerminalOutcome semantics unchanged. run discard is a projection-neutral pre-delete authorization plus convergent resource removal; normal clean removal remains non-force, while verified dirty removal alone requires explicit --force. Real-Git tests use disposable repositories and cover failed/cancelled, dirty/untracked, detached HEAD, branch-only retry, exact path/repository binding, live worker refusal, stale projection replay, dry-run byte preservation, and multi-node selection.

## Resolution

### 2026-09-08T11:23:58Z · @codex

Implemented as the same terminal-work disposition path; statuses and landing truth remain unchanged. Full destructive-boundary review and required validation passed.
