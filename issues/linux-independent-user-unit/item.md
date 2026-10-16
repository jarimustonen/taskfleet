---
created: 2026-10-16
updated: 2026-10-16
type: feature
reporter: jari
status: done
priority: normal
provenance: chat
source_ref: taskfleet:01m51nrfvbqwsybz0gdggz6q4b/request:user-unit
originating_run: 01m51nrfvbqwsybz0gdggz6q4b
originating_run_kind: spinoff
related: ['@caller-owned-agent-runs']
lane: open-habitat
lane_seq: 10
collision: [supervise-cleanup, run-public-contract]
closed: 2026-10-16
closed_by: pi
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

- [x] From the daemon's minimal environment, supervisor and worker survive daemon service restart under their own user unit.
- [x] The same user's SSH session can reach the recorded Taskfleet tmux socket; other Unix users gain no access.
- [x] Missing user manager, unset/invalid runtime directory, restart and teardown are tested without disturbing unrelated sessions.
- [x] Non-Linux launch behavior remains native and documented.

## Context

The lifetime requirement already appears in @caller-owned-agent-runs; extract
its Taskfleet-owned-worker application here, without waiting for caller-owned
creation/binding. Open Habitat defers caller-owned browser agents and observes
ordinary Taskfleet spinoffs instead. Consumer architecture:
native-agent-host `issues/manage-agent-worktrees/architecture.md`.

## Comments

### 2026-10-16T08:17:55Z · @pi

Validation complete: TMPDIR=/var/tmp/taskfleet-linux-build scripts/validate-local-release.sh passed on clean 661885a582e6fcacd8aa083bfc4757e8921655cc (1220 release nextest tests passed, 1 skipped; doctests, rustdoc, packaging/distribution and all release fixtures passed). Real systemd 259/tmux 3.6 headless and foreground smokes both passed: two parent-service restarts each, minimal environment, socket discovery, idempotent create, reattach, delayed notification delivery and explicit-merge worktree/session/server/service teardown. One claude-fable-5-1 independent review; all 3 confirmed findings fixed, none deferred. Runtime ownership/missing manager use isolated unit/stub evidence, not disruption of the live manager. Non-Linux runtime was not executed here. Full reproducible evidence, corrections and limitations: validation.md. No installed binary/skills or unrelated services changed; no release/tag/publication.
