---
created: 2026-10-16
updated: 2026-10-16
type: feature
reporter: jari
status: untriaged
priority: normal
provenance: chat
source_ref: taskfleet:01m51nrfvbqwsybz0gdggz6q4b/request:user-unit
originating_run: 01m51nrfvbqwsybz0gdggz6q4b
originating_run_kind: spinoff
related: ['@caller-owned-agent-runs']
---

# Launch Linux supervisors and workers in independent user units

## Description

## Request

Requested by Jari for Open Habitat on 2026-10-16. A browser parent's Pi runs
inside a daemon systemd service with `KillMode=mixed` and `PrivateTmp=yes`.
Taskfleet supervisors/workers launched as its descendants would die on daemon
restart; a tmux socket in private `/tmp` is also inaccessible over SSH.

On Linux launch the supervisor and worker outside that service's cgroup in
their own user unit via `systemd-run --user`, and put the Taskfleet tmux socket
under `$XDG_RUNTIME_DIR`. Detachment (`setsid`, double-fork, `nohup`) does not
escape the service cgroup. Keep native ownership and cleanup semantics.

The daemon intentionally starts Pi with only PATH, USER and HOME. Taskfleet
must therefore derive `XDG_RUNTIME_DIR` when unset from `/run/user/<uid>` only
when it exists and is owned by the caller, and supply the environment its
user-manager launch needs. Do not require widening the daemon's environment.
Homebase provides a running user manager through lingering; missing manager
or invalid runtime-directory ownership needs an actionable failure.

## Acceptance Criteria

- [ ] From the daemon's minimal environment, supervisor and worker survive daemon service restart under their own user unit.
- [ ] The same user's SSH session can reach the recorded Taskfleet tmux socket; other Unix users gain no access.
- [ ] Missing user manager, unset/invalid runtime directory, restart and teardown are tested without disturbing unrelated sessions.
- [ ] Non-Linux launch behavior remains native and documented.

## Context

The lifetime requirement already appears in @caller-owned-agent-runs; extract
its Taskfleet-owned-worker application here, without waiting for caller-owned
creation/binding. Open Habitat defers caller-owned browser agents and observes
ordinary Taskfleet spinoffs instead. Consumer architecture:
native-agent-host `issues/manage-agent-worktrees/architecture.md`.
