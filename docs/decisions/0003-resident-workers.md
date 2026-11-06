# ADR 0003 — Named resident workers work in told rounds

- **Status:** Accepted
- **Date:** 2026-11-06
- **Decider:** Jari Mustonen
- **Issue:** `named-resident-workers` (design and decisions:
  `open-habitat` `issues/design-named-workers/`)
- **Builds on:** ADR 0001 (thin supervisor, typed outcomes)

## Context

A spinoff worker today does one unit of work and ends with `run merge`. Some
work is a conversation instead: prototype rounds while Jari answers, a bug
analysis whose fix comes next, a reviewer who later checks the fix against its
own findings, a researcher who gets follow-up questions. A fresh worker for
every round re-reads the same files (measured at roughly a quarter of a
prototype round) and, worse, loses what was never written down: rejected
paths, intermediate hypotheses, a reviewer's own findings.

Two partial mechanisms existed. A live Pi worker can be steered with
`run message` through the bundled adapter, and can park with
`node.awaiting_input`. But that keeps a 0.2–0.3 GB process alive between
rounds, does not exist for Claude, makes `run wait` sit out the 180 s human
grace, and leaves the round's work off `main` until the very end.

Jari chose (2026-11-06) that Taskfleet owns named workers, Habitat only shows
them and relays the person's words through the parent agent, each round's work
reaches `main` at the end of the round, and `main` is rebased into the worker's
tree before it continues.

## Decision

### A resident run is an ordinary single-node run with one extra told fact

`run create --resident --name <name> [--idle-expiry <duration>]` records a
`resident` policy on `run.created`. It is accepted only for autonomous,
single-node, Taskfleet-owned kinds (`spinoff`, `research`,
`technical-decision`); fan-out, `--interactive` and caller-owned runs refuse
it. A name starts with a lowercase letter, so it can never be read as a run-id
prefix (which starts with `0`–`7`). A name resolves only among the
repository's non-terminal runs and refuses ambiguity; `run create` refuses a
name that is already live in the same repository.

### A round ends with a typed report, and the exit that follows is expected

The worker ends a round with `taskfleet node round-report <run-id>
--summary-file <file>`. That appends `node.round_reported` (round number,
worker attempt, bounded summary) and opens the node's round state. Then the
worker stops: Claude's `--print` process ends with its turn, and the bundled
Pi adapter calls `ctx.shutdown()` at the next `agent_settled` boundary after
the report. The launcher shim records `worker.exited` as always.

This is the "told, not guessing" model of ADR 0001 extended by one row. The
supervisor never decides that a worker is idle or finished from activity, a
pane, or a quiet period. It reads two told facts — a round report for the
current attempt and the shim's exit — and the outcome table gains:

| Told facts (resident run) | Outcome | Teardown |
|---|---|---|
| round report for this attempt, then any exit | `waiting` (non-terminal, not attention) | none |
| clean exit, no round report, no merge | `attention-required` (unchanged) | none |
| non-zero exit or signal, no round report | `failed` (unchanged) | preserve |
| idle expiry, everything landed | `dormant` (non-terminal) | worktree and integrated branch removed; native session kept |
| idle expiry, unlanded work | `attention-required` (`resident-idle-unlanded`) | none |
| `run merge` | `done` (unchanged) | yes |
| `run cancel` | `cancelled` (unchanged) | preserve |

A failing exit *after* the round report does not fail the node: the report is
the higher-fidelity fact about the round, the same way a merge wins over a
later non-zero exit. Resident runs are excluded from the empty-handed
auto-retry, because a retry mints a fresh worktree and session and would throw
away the continuity the run exists for.

### The next message starts a new attempt from the same native session

`run message <name|id> <text>` to a resident whose current attempt has exited
(a round report, or the attention case) resumes it:

1. Serialize on the run's resident operation lock, so a message, a land and
   the supervisor's expiry never act on the worktree together.
2. Recreate the worktree at its recorded path from the source branch if idle
   expiry removed it (Claude resumes only in the same directory).
3. Rebase the worker branch onto the source branch tip, so the worker sees
   decisions and other landings. A conflicted rebase is left in progress and
   becomes the round's first task, named in the message preamble. A dirty
   tree skips the rebase and says so.
4. Generate the next attempt's launcher from the recorded selection with the
   recorded session: Pi `--session <recorded path>` (the handshake verifies
   the existing transcript instead of creating one), Claude
   `--resume <recorded session id>` with the same session-bound hooks.
5. Start it in the worker's recorded tmux pane (`respawn-pane`) or, if that
   pane is gone, a new window in the recorded session; wait for the same
   PID handshake as a fresh spawn.
6. Under the run lock append `node.resumed`, which rewires the node to the
   new attempt and closes the open round with the message id.

The message text is never written to the event log; it reaches the worker as
the new attempt's first input. A Pi worker that is mid-round (no open round
report) keeps today's acknowledged adapter path. A Claude worker mid-round has
no input channel and is refused with a retryable error. When a round report
is open but the exit has not yet been recorded, `run message` and `run land`
wait a bounded time for the told exit rather than guessing.

### Landing a round is an explicit, recorded, non-terminal merge

`run land <name|id>` rebases the worker branch onto the source tip and
fast-forwards the source, with the same machinery and transaction record as
`run merge` (`merge.started` with `landing: true`, the source-ref
compare-and-swap, and OID-based recovery). Success appends
`node.round_landed`, never a terminal report: worktree, branch and session
stay. It is allowed only while the worker's current attempt has a recorded
exit, so nobody writes to the worktree during the rebase. Recovery of a
crashed land completes it with `node.round_landed`, never with an
`explicit-merge` report. The parent lands after every round report by default
(Jari's decision that each round reaches `main`); the supervisor never
initiates a merge or a land.

### Idle expiry keeps the session and never merges

After `idle_expiry` (default 24 h) from the open round report with the worker
exited, the supervisor takes the resident lock and applies invariant 5's
fail-closed rule. If the branch is wholly reachable from the source branch and
the worktree is clean, it removes the worktree and the integrated branch and
records `node.resident_dormant`; the Pi transcript and the Claude session
store are untouched, and the next message recreates the worktree at the same
path. Anything else records `node.resident_attention` once and preserves
everything. A dormant resident is still revivable at any time; it ends only
with `run merge` or `run cancel`.

The deadline is anchored on the told round report, not on observed activity.
It is a timer on a fact, the same kind of thing as the post-death grace in
A4, not one of the activity clocks ADR 0001 deleted.

### Read surfaces

`run show` and `run list` expose `resident` (name, `round_state` of
`working | waiting | dormant`, rounds reported, the latest report summary, its
landed and expiry state). `run wait` settles immediately on an open round
report (`round_reported: true`) with no awaiting-input grace, so the parent
and Habitat wake when the round ends.

## Consequences

The supervisor's adjudication stays a lookup over told facts; the new state is
two events and one projection field, and every automatic step either preserves
work or removes only work Git proves integrated. A waiting worker costs a
worktree and its 10 MB supervisor, and survives daemon restarts and memory
pressure because no agent process exists between rounds.

Accepted costs: each round pays an agent start and a first request over the
whole context (a cold cache after an hour costs about as much as a fresh
re-read); Claude resume is bound to the original directory, which the revival
path preserves but which forbids moving a session; and a resident is bound to
its profile, so a step that needs another model is a fresh worker. The worker's
context remains a cache, not a record: the bundled skills require each round's
result in files and commits as if the next round started fresh.

## Rejected alternatives

- **Keep the process alive between rounds (today's Pi awaiting-input
  contract).** Pi-only, holds memory, and dies with daemon restarts.
- **Sub-sessions inside the parent's Pi (extension).** Pi-only, no worktree
  isolation, dies with the parent, and duplicates the run platform.
- **Habitat conversations in worktrees.** Pi-only and would give the Habitat
  daemon merge authority it must not have.
- **Supervisor lands each round automatically.** It would make the supervisor
  initiate source-branch mutation, which only `run merge` (and now
  `run land`) may do; the parent lands explicitly instead.
- **Infer the round end from an idle prompt.** That is the inference engine
  ADR 0001 removed.
