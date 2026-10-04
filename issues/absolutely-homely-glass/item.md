---
created: 2026-10-04
updated: 2026-10-04
type: feature
reporter: jari
status: untriaged
priority: normal
provenance: chat
source_ref: native-agent-host:decide-taskfleet-browser-integration
---

# Support workspace-only runs with a caller-owned agent for browser worktree agents

## Description

## Need

Omega Habitat (native-agent-host) will show Taskfleet worktree agents as first-class
browser agents. Jari approved the division on 2026-10-04: Taskfleet owns run and
worktree creation and settlement, while the host's per-user daemon launches and
controls a separate, browser-controllable Pi process in the verified worktree.
The design is in native-agent-host `issues/decide-taskfleet-browser-integration/design.md`
(option B); implementation there is `issues/manage-agent-worktrees`.

## What Taskfleet likely needs

- A supported run mode that reserves and supervises a worktree run without
  launching a terminal worker, with an explicit settlement path (merge, cancel,
  discard) driven by the caller. Today `run create` expects a worker PID handshake.
- Supervisor lifetime that does not depend on the caller's process or service
  cgroup (the daemon is a systemd user service).
- A stable binding from run/node to the Pi session that worked in it, so the
  conversation stays findable after worktree teardown.
- Verified idempotency-key recovery for these runs.
- Predictable worktree placement (workmux `worktree_dir`) so the host can open
  the checkout safely under the user's `~/src`.

The host's design document lists the open questions; the exact contract is for
the Taskfleet design to settle together with that work.
