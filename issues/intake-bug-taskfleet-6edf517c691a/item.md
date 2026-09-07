---
created: 2026-09-02
updated: 2026-09-07
type: bug
reporter: jari
status: open
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:stint-handoff-unrelated-global-runs-2026-09-02
lane: workflow-skills
lane_seq: 10
---

# stint-handoff blocks on unrelated global runs

## Description

stint-handoff blocks on unrelated global runs

## Observed

The `stint-handoff` skill preflight says: "Every live, awaiting-input, recoverable, or otherwise resumable worker must have landed or relinquished ownership." It tells the executor to inspect the global output of `taskfleet run list --output json` without defining ownership scope.

During a short Saleshub session in `3dbear-monorepo`, the current agent had launched no taskfleet runs. Another agent window had an unrelated pending run, `01m1g1y6yta952m75ze59x25hg` (`mail-triage-cheap-union`). Applying the skill literally made this session stop before `/wrap-up`, write the unrelated run into `TODO.md`, and report an incomplete handoff.

The unrelated run was healthy, owned by another window, and had no connection to this session. A repository filter would still be wrong because parallel windows can run work in the same repository.

## Expected

Terminal handoff must block only on workers that this stint or agent session launched or explicitly took ownership of. Runs owned by other windows must not block wrap-up and must not be copied into this session's handoff narrative.

## Proposed correction

1. Replace the global wording with: every live or resumable worker launched by this stint/session must have landed or relinquished ownership.
2. Require the stint orchestrator to retain the run IDs it launched and inspect only those IDs during preflight.
3. If no runs were launched by this session, treat worker ownership as clear without using unrelated rows from the global run list.
4. Add a regression scenario with two simultaneous agent windows in the same repository. Window A must be able to wrap up while Window B's run remains pending.
5. Keep the existing fail-closed behavior for session-owned runs.

## Affected artifact

The taskfleet-owned `stint-handoff` skill installed at `~/.pi/agent/skills/stint-handoff/SKILL.md`, preflight step 0.

## Decisions

### 2026-09-07T18:15:48Z · @jari

Accepted the minimal skill-owned correction: stint-start retains the run IDs it launched, and stint-handoff checks only those session-owned IDs. Unrelated global runs and known foreign changes must not block or enter this session's handoff. A durable session-ownership field is not required for this slice; that belongs to any later checkpoint design.
