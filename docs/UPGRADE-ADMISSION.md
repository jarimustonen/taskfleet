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

## Operator overrides

By default activation requires a quiescent state root. Two separate flags let
an operator accept exactly one kind of non-quiescence each. New admissions stay
excluded throughout in every case: the gate is held exclusively and the
inhibit marker is persisted before anything is moved or activated. A refused
override changes nothing, not even the marker.

**`--allow-live-runs --target-state-schema <N>`** activates while runs are live,
awaiting input, or mid-merge. Their supervisors and workers keep running the
old executable image (Linux and macOS keep a replaced binary's image mapped),
so the only thing the old processes and the new CLI share is the on-disk run
state, versioned by the state schema (`taskfleet version --json` reports it as
`data.state_schema_version`). The rule: `<N>` is the target release's state
schema, read from the *staged* binary, and it must equal the gate binary's own
schema and every manifest and node schema of every accepted run. Otherwise the
gate refuses with `upgrade_state_schema_incompatible`; settle those runs and
upgrade quiescently. A release that changes the on-disk format so an older
reducer or a newer CLI would misread it must bump the state schema. A run whose
supervisor pid record is unreadable cannot be classified and refuses
`upgrade_state_unverifiable` even with the flag.

Accepted runs keep calling the executable path that launched them: their
worker launchers and Claude hooks embed it. The activation must therefore
leave the predecessor executable in place or replace it in place until those
runs settle (for Homebrew, avoid the automatic cleanup of the old keg, for
example with `HOMEBREW_NO_INSTALL_CLEANUP=1`). The success envelope reports
`data.predecessor_executable.present_after_activation` and warns when it
vanished.

**`--abandon-stale-creations`** handles an unpublished `.creating/runs/<id>`
left by a creator that was killed before publication. Such a creation counts
as abandoned only when nothing that could still write it is alive: the
exclusive admission gate is held (every participating creator holds it shared
until publication), its staging `.lock` is free, its keyed materializer lease,
if any, is released, no recorded supervisor, agent, or handshaked worker pid is
live, and no `.creating/message-<id>-<attempt>.sock` still accepts connections.
Any probe that cannot decide refuses `upgrade_state_unverifiable`; a probe that
finds a live writer refuses `upgrade_creation_live`. Nothing is deleted: the
staging tree moves to `abandoned-creations/<id>/run` with its
`.creating/message-<id>*` files beside it and an `abandoned.json` that records
the evidence and the original paths; restore it by renaming back while no
Taskfleet process uses the state root. The creation's git worktree and branch,
if the materializer got that far, are left untouched. A keyless creation's
materializer child holds no lease, so one that outlived its killed creator is
not visible to these probes; it would then write into a moved directory, which
loses nothing. An unrecognized `.creating` entry is never treated as a
creation and still refuses.

Every override that accepted something writes
`upgrade-receipts/<receipt-id>.json` before the activation starts and rewrites
its `outcome` afterwards (`completed`, `failed`, `timeout`, ...). The JSON
envelope carries `data.quiescent` (false when anything was overridden),
`data.accepted_live_runs[]` (`run_id`, `reasons`, `state_schemas`),
`data.abandoned_creations[]` (`creation_id`, `moved_to`, `moved`, `evidence`),
`data.receipt`, and `data.recovered`. A failed or timed-out activation keeps
the marker, and its error carries `error.details.receipt`,
`accepted_live_runs`, and `abandoned_creations`. The overrides also apply
under `--recover`.

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
