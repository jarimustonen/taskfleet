---
created: 2026-10-04
updated: 2026-10-05
type: feature
reporter: jari
status: untriaged
priority: normal
provenance: chat
source_ref: native-agent-host:decide-taskfleet-browser-integration
---

# Support worktree runs with caller-owned agents

## Description

Enable Omega Habitat (native-agent-host) to offer browser-controlled worktree
agents. Jari approved the ownership boundary on 2026-10-04: Taskfleet creates
and settles the run and worktree; the host's per-user daemon launches and
controls a separate Pi process in the verified checkout. Provide a supported
contract for this division without launching Taskfleet's normal terminal worker.

The caller needs to identify the run, node and checkout reliably and explicitly
merge, cancel or discard through Taskfleet. Stopping Pi or losing the daemon
is not a decision to merge or discard work. Taskfleet's supervisor needs a
lifetime independent of the daemon's systemd service: detaching a process does
not remove it from the service cgroup. Interrupted creation needs recoverable
idempotency semantics so a retry does not create duplicate work.

A durable run/node-to-Pi-session binding lets the host keep conversations
findable after worktree teardown. Worktree placement also needs a supported,
predictable contract: the host currently discovers immediate workspaces under
`~/src`, while workmux's default sibling layout nests checkouts one level deeper.
Its `worktree_dir` configuration is a possible solution to verify.

## Design context and outcome

In native-agent-host, `issues/decide-taskfleet-browser-integration/item.md`
records the approval of option B in the adjacent `design.md`;
`issues/manage-agent-worktrees` owns the browser implementation. Placement and
which settlement actions appear in the browser remain choices for that work.

Define and implement the Taskfleet contract in coordination with that consumer.
Demonstrate creation without a terminal worker, recovery after interrupted
creation or daemon restart, explicit settlement that preserves unmerged work,
and a session binding that survives teardown. The existing run-state and merge
invariants in `AGENTS.md` remain the basis for this lifecycle.

## Decisions

### 2026-10-05T12:29:56Z · @jari

Caller-owned runs must not invoke workmux at all, including create, retry, merge and teardown. Taskfleet owns Git worktree provisioning and Git-only settlement. Move merge orchestration from the embedded merge.sh into Rust while preserving the existing transaction, CAS, locking and cleanup invariants; ordinary terminal-worker creation may continue to use workmux. See design.md for the updated proposal.
