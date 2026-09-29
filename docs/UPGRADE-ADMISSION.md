# Fleet activation and worker admission

`taskfleet run upgrade-lock -- <trusted executable> [args...]` takes the exclusive
worker-admission lock, checks existing runs, persists
`$TASKFLEET_HOME/.worker-admission-inhibited` (including directory fsync), and
only then starts the activation. A successful synchronous exit clears the
marker; an error, timeout, or hard-killed Taskfleet process leaves it in place.
New `run create` and `run reattach` refuse admission while the marker exists,
even after the flock is released. Do not delete or rename the marker manually.
Use the same state root and a Taskfleet release that understands the marker for
**every** creator: older binaries cannot enforce this fence.

The activation executable is trusted: exit zero must mean *all* writes to the
installation have completed, including any background subprocesses, and no
writer can start later. Taskfleet cannot prove this from a PID: a descendant
can call `setsid()`, close its inherited flock FD, and continue writing after
its parent exits. The lock is not process-tree containment. A nonzero exit
also leaves admission inhibited, even if the installation looks unchanged.
Keep activation synchronous; no detached installers or post-exit writes.

## Recovery after timeout, crash, or failure

1. Inspect the activation's logs and installation, identify **all** writers
   from the prior invocation, and ensure they have stopped or completed.
   Do not infer this solely from the immediate child PID, a process group, or
   the absence of a lock holder. If this cannot be established, leave
   admission inhibited and escalate rather than clearing the marker.
2. Choose a trusted synchronous reconciliation executable that verifies and
   repairs/rolls back the installation. Its exit zero must assert that the
   installation is consistent and no prior or new activation writer can
   mutate it after exit. A failed or timed-out reconciliation keeps the marker.
3. Run `taskfleet run upgrade-lock --recover --confirm-quiescent -- <executable> [args...]`.
   This holds the exclusive lock throughout and checks run quiescence before
   executing. `--confirm-quiescent` is the operator's attestation for step 1;
   Taskfleet cannot verify detached descendants automatically. Recovery does
   not run without an existing valid marker. If the marker is a symlink or
   has unexpected contents, stop and investigate the state-root integrity.

On a clean upgrade there is no manual approval or recovery step. A recovery
that exits zero prematurely would reopen admission unsafely; use a checker
specific to the activation, not `/bin/true` in production.
