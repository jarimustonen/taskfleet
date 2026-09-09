---
created: 2026-09-08
updated: 2026-09-09
type: feature
status: testing
priority: high
lane: supervisor
collision: [taskfleet-supervisor]
blocked_by: ['@durable-worker-evidence']
---

# Bound completed windows in persistent agent sessions

## Goal

Support a deliberate persistent agents session with bounded completed-window retention, while preserving active/unrelated work and durable archived evidence. Accepted by Jari in the 2026-09-08 Haapa agent-environment programme.

## Current behavior

Taskfleet --tmux-session agents already places workers in a named shared session. Completion immediately removes managed windows; cleanup_managed_session may remove the detached session when only synthetic shells remain. No completed-window TTL/count policy or long-lived reaper exists. Reuse Taskfleet ownership and stable pane/socket identities, not an Intakectl/Homebase session manager.

## Scope

Add explicit opt-in shared-session persistence and age/count retention through existing configuration. Retained terminal windows are inert evidence displays, never running agents. Implement a bounded idempotent maintenance command suitable for Homebase's systemd timer after per-run supervisors exit. Default compatibility and fail-closed ownership checks remain. Retention failure must not lose archived evidence or delete unowned panes.

## Acceptance criteria

- [ ] Two concurrent workers in a named session remain individually identifiable by repository, run and purpose.
- [ ] Completion of one preserves the other and persistent session.
- [ ] Retained completed windows are inert and bounded by explicit age/count policy.
- [ ] Repeated maintenance removes only owned terminal windows and preserves archives, active and unrelated windows.
- [ ] Supervisor/maintenance concurrency and restart tests prove idempotent lifecycle.
- [ ] Configuration and maintenance CLI are documented for Homebase systemd integration.

## Validation

scripts/validate-local-release.sh; private-tmux mixed-ownership and expiry tests; required /llm-review and /assess-findings. No worker release/install or host changes. Depends on durable-worker-evidence so cleanup has a proven archive contract.
