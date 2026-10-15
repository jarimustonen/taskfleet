---
created: 2026-10-12
updated: 2026-10-15
type: feature
reporter: claude
status: duplicate
priority: normal
provenance: agent
provenance_detail: native-agent-host wrap-up 2026-10-12
closed: 2026-10-15
duplicate_of: intake-feature-taskfleet-7e8ef9b4b70c
---

# Record the parent of runs a worker spawns

## Description

Workers in native-agent-host stints spawn their own Homebase helper runs with `taskfleet run create`. Those child runs carry no `parent_run_id` (the parent flags are only used by fan-out drivers), so `run show` cannot say which run started them.

Reconstructing the session hierarchy afterwards required grepping every `~/.taskfleet/runs/*/evidence/n-0001/pi-session.original.jsonl` for the child's run id and taking the earliest match; see native-agent-host `issues/night-stint-2026-10-05/interruptions.md`.

Expected: a run created from inside another run's worktree records the creating run (for example from `run show --current`) as its parent or `created_by`, visible in `run show` and `run list`, without the fan-out driver semantics.

## Comments

### 2026-10-15T17:32:34Z · @intake

Duplicate of intake-feature-taskfleet-7e8ef9b4b70c
