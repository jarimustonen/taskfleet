---
created: 2026-09-02
updated: 2026-09-07
type: bug
reporter: jari
status: duplicate
priority: normal
provenance: agent:3dbear-stint-handoff
source_ref: agent:3dbear-stint-handoff/reporter:jari/id:stint-handoff-unrelated-concurrent-agents-20260902
related: ['@intake-bug-taskfleet-6edf517c691a']
closed: 2026-09-07
closed_by: jari
---

# stint-handoff blocks on unrelated concurrent agents

## Description

stint-handoff blocks on unrelated concurrent agents

## Observed

During `/stint-handoff` in 3dbear-monorepo, another legitimate agent was actively editing unrelated issue files in the shared main worktree (`mail-triage-cheap-union` and related mail-triage files). The handoff instructions were interpreted as requiring a globally clean worktree and no other repository activity, so `/wrap-up` was skipped even though the current stint's own work was committed and pushed.

The preflight also treated three old terminal `failed` taskfleet runs as unresolved ownership solely because their worktree paths still existed. Their substantive work had already been superseded by later landed runs. None had a live supervisor or awaited input.

## Expected

`stint-handoff` should distinguish:

1. current-stint or otherwise live/recoverable ownership that can be damaged by wrap-up;
2. a separate known agent legitimately editing unrelated files;
3. stale terminal failed runs whose cleanup is an taskfleet maintenance concern.

An unrelated active agent must not block `/wrap-up`. The skill should preserve foreign changes, commit only its own exact paths, report the concurrent activity, and continue. It should stop only when the current wrap could overwrite, commit, reset, or otherwise interfere with the other agent, or when ownership of the current stint's work is genuinely unresolved.

Please add explicit scope and examples for a shared main worktree, including how to proceed when `git status` contains known foreign paths.

## Resolution

### 2026-09-07T18:15:49Z · @jari

Duplicate of the scheduled session-owned handoff correction; retain this report as the foreign-change regression scenario.
