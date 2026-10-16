---
created: 2026-10-16
updated: 2026-10-16
type: feature
reporter: jari
status: open
priority: normal
provenance: chat
source_ref: taskfleet:01m51nrfvbqwsybz0gdggz6q4b/request:created-by-session
originating_run: 01m51nrfvbqwsybz0gdggz6q4b
originating_run_kind: spinoff
related: ['@intake-feature-taskfleet-7e8ef9b4b70c']
lane: open-habitat
lane_seq: 20
collision: [supervise-cleanup, run-public-contract]
---

# Record the native session that creates a run

## Description

## Request

Requested by Jari for Open Habitat on 2026-10-16. A browser conversation runs
in a main checkout, not a Taskfleet-owned worktree, so `created_by_run_id`
cannot associate its spinoffs with that conversation.

Record optional `created_by_session: { harness, session_id }` at run creation
from inherited `TASKFLEET_CREATOR_SESSION=pi:<session-uuid>`. Preserve it on
idempotent replay and expose it in `run show` and `run list --json` beside
`created_by_run_id`. The host sets it when launching Pi; agents need not supply it.
Clear the variable in worker environments so nested spawns do not inherit a
stale browser-session association; their precise creator is `created_by_run_id`.
This is same-user display attribution, never authorization or fan-out ownership.

## Acceptance Criteria

- [ ] Main-checkout creation records and exposes the session association; absent or malformed input has a documented, tested outcome.
- [ ] Replay preserves original attribution and nested workers cannot inherit the parent's stale session claim.
- [ ] Existing creator-run and fan-out semantics remain unchanged.

## Context

Extends @intake-feature-taskfleet-7e8ef9b4b70c (creator-run attribution), not
@caller-owned-agent-runs. Open Habitat now observes Taskfleet-owned spinoffs
read-only; caller-owned browser worktree agents are deferred there.
Consumer: native-agent-host `issues/manage-agent-worktrees/architecture.md`
and `issues/implement-spinoff-agents/item.md`.
