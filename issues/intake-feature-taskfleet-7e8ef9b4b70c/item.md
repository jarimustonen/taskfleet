---
created: 2026-10-12
updated: 2026-10-12
type: feature
reporter: jari
status: untriaged
priority: normal
provenance: agent:native-agent-host-wrapup
source_ref: agent:native-agent-host-wrapup/reporter:jari/id:nah-wrapup-2026-10-12-parent
---

# Record the parent run of runs a worker spawns

## Description

Record the parent run of runs a worker spawns

Workers in native-agent-host stints spawn their own Homebase helper runs with `taskfleet run create`. Those child runs carry no `parent_run_id` (the parent flags are only used by fan-out drivers), so `run show` cannot say which run started them.

Reconstructing the session hierarchy afterwards required grepping every `~/.taskfleet/runs/*/evidence/n-0001/pi-session.original.jsonl` for the child's run id and taking the earliest match; see native-agent-host `issues/night-stint-2026-10-05/interruptions.md`.

Expected: a run created from inside another run's worktree records the creating run (for example from `run show --current`) as its parent or `created_by`, visible in `run show` and `run list`, without the fan-out driver semantics.
