# Implementation plan — caller-owned agent runs

## Problem and agreed direction

Omega Habitat needs worktree agents that a person can create and control in
the browser. Taskfleet currently starts a terminal worker; the browser cannot
control that worker through Pi RPC.

Jari approved option B on 2026-10-04: Taskfleet creates and settles the run and
worktree, while the host's per-user daemon starts and controls a separate Pi
process in the verified checkout. Implementation may proceed on that basis.
The approval is recorded in native-agent-host's
`issues/decide-taskfleet-browser-integration/item.md` and
`docs/decisions/adr-0010-taskfleet-worktree-agents.md`; this issue's
[item.md](item.md) describes the Taskfleet outcome.

The division of responsibility determines what must survive a failure.
Stopping Pi or restarting the daemon leaves the Taskfleet run and unmerged
work available for explicit settlement. Retrying an interrupted creation must
recover the original run or explain why recovery needs attention, without
silently creating a duplicate. The supervisor needs a lifetime independent
of the daemon, and the run needs a durable link to its Pi session so the
conversation remains findable after worktree teardown.

## Proposed implementation

Start with one caller-owned spinoff and extend the existing creation and
settlement paths. This is a proposed first slice; the exact CLI, event fields
and session-binding representation remain implementation choices. Record the
consumer-facing contract in a design document under this issue as it takes
shape, coordinating with native-agent-host's `manage-agent-worktrees` work.
Resolve each contract when its implementation needs it.

### 1. Create a worktree and settle its run

Build a complete path in a disposable repository: create a run and node,
return their identities and the actual checkout path, then explicitly merge
through Taskfleet's existing transaction and cleanup.

Creation needs an explicit way to select caller ownership and supervision
needs a durable way to recognize it. A missing worker PID is expected for
these runs; it must not trigger worker-death or retry handling. The existing
`--interactive` flag changes terminalization, but does not provide creation
without a worker. The test-only `--skip-materialize` path is not sufficient
either.

Caller-owned creation must use a Taskfleet-owned Git worktree path and never
invoke workmux, even for retry, merge or cleanup. Agree a collision-safe
checkout location and return its verified absolute path so the host can check
it against the run and node. Replace the embedded `merge.sh` driver with
Rust-owned merge orchestration before caller-owned settlement: preserve the
existing serialization, source-OID guard, transaction recovery and supervisor
cleanup while invoking Git for its own operations. Ordinary worker creation
may continue to use workmux; remove the script only once both modes pass the
same merge-safety tests. Placement and discovery under `~/src` remain
coordinated host implementation choices.

### 2. Make interruption recoverable

Extend keyed creation to cover partial worktree creation and a lost response.
A retry should return the original run, node and checkout, finish a safely
recoverable creation, or report an actionable uncertain state. Preserve work
whose ownership or safety cannot be established.

Exercise interruptions around external side effects and publication, including
concurrent retries. Decide the required durable recovery records from those
cases. Also cover supervisor startup failure after publication: the caller
needs the recoverable run identity and a supported reattach route.

Arrange supervisor launch outside the daemon's service cgroup with the host.
Taskfleet's double-fork does not provide that separation. Verify supervisor
survival and run reconciliation across daemon stop and restart in a disposable
service setup.

### 3. Connect the Pi session and safe settlement

Add a durable run/node-to-Pi-session binding through Taskfleet's locked state
updates. Agree its registration, replacement and read behavior with the host,
including how history is read after the checkout disappears. Pi owns its
conversation history; whether Taskfleet also archives a copy is an open
implementation choice.

The daemon owns the Pi writer, so Taskfleet's worker PID cannot establish
that writing has stopped. Coordinate merge, cancel and discard with that
owner so settlement cannot destroy work still being written. A dedicated
quiescence handshake is one possible mechanism, not a settled requirement.
Keep Taskfleet's existing distinctions between successful merge, cancellation
with retained work, and explicit discard.

## Completion

Demonstrate the host journey with disposable data: creation, Pi startup,
session binding, interrupted creation, daemon restart, explicit settlement
and readable history after teardown. Include ownership mismatches and a Pi
process still writing during settlement. Verify existing runs and normal
Taskfleet-launched workers continue to behave correctly.

Use the repository's validation gate, `scripts/validate-local-release.sh`,
with focused integration fixtures and JSON snapshots for the new contract.
Which settlement actions appear in the browser remains the host's decision;
Taskfleet supplies the supported operations and their outcomes.
