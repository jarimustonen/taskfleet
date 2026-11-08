# Result: interrupt a resident worker's round

`taskfleet run interrupt <run-id|name> [--wait-exit 30s] [--dry-run]` stops a
resident worker's current round and keeps the run.

- **Told first, then signal.** Under the resident operation lock it appends
  `node.round_reported` for the current attempt with `interrupted: true` and a
  Taskfleet summary, then signals the recorded agent process. The signalled
  exit therefore falls under ADR 0003's existing "round report, then any exit
  → waiting" row: no new outcome, supervisor rule, or inference.
- **Fenced like `run salvage`.** The recorded start identity is re-verified
  immediately before every `kill`; a recycled pid is never signalled, and a
  live pid whose identity cannot be read is refused (`worker_unverifiable`)
  before anything is recorded. Only the recorded pid is signalled, never a
  process group.
- **Per harness.** Claude gets `SIGINT` (its `--print` turn aborts and the
  process exits), escalated once to `SIGTERM` after 10 s. Pi gets `SIGTERM`,
  which Pi handles as its graceful shutdown (`session_shutdown`, so the bundled
  adapter removes its socket; Pi kills its tool processes). The Pi round marker
  is written as for a round report. A repeated interrupt sends `SIGTERM`.
- **Waits for the told exit** (`worker.exited`) up to `--wait-exit`; a worker
  that is provably gone without one counts after a 3 s grace. A worker still
  alive after the wait is `worker_still_running` with `round_recorded: true`;
  re-running signals again.
- **Leaves alone** a worker that already reported its round
  (`outcome: round-reported`) or already exited (`between-rounds`).
- **Keeps** the worktree, branch, uncommitted changes, and native session. The
  next `run message` resumes the same Pi `--session` / Claude `--resume`
  session; its preamble tells the worker the round was interrupted and that
  uncommitted work is still in the tree.
- **Read surfaces.** `run wait` settles with `round_reported: true` and
  `round_interrupted: true` (text: `round_interrupted=true`); `run show` /
  `run list` / `run wait` carry `resident.last_round.interrupted`; `run show`
  text gains a `resident:` line.
- **Skills.** `worktree-spinoff` "Named support workers" says when to interrupt
  (the round became wrong while it runs; not merely slow), how, what to send
  next, and not to land an interrupted round's half-finished commits blindly;
  short pointers in `stint-start`, `taskfleet-run-overview`,
  `taskfleet-overview`. ADR 0003 amended; CHANGELOG updated.

Evidence: `tests/resident.rs::pi_resident_interrupt_stops_the_round_and_message_resumes_it`
and `claude_resident_interrupt_uses_sigint_and_resumes_the_session` through the
production materializer, launcher, handshake, shim and supervisor (dry run,
signal, told exit, waiting not failed or attention, dirty file kept, wait
flags, repeat is a no-op, resume with the same session and the interrupted
preamble, a self-reported round left alone);
`taskfleet-core/tests/resident_rounds.rs::an_interrupt_opens_a_sticky_round_and_never_amends_a_reported_one`;
`not_resident` refusal in `resident_flags_and_round_reports_are_validated`.
No live Pi or Claude smoke was run in this worktree.

## Decisions for later review

1. **Signals, not the Pi adapter's `ctx.abort()`.** The issue sketched the
   acknowledged adapter for Pi. A resident's process ends at every round
   boundary anyway, and Pi's `SIGTERM` handler is its graceful shutdown, so a
   verified signal gives the same result for Pi and Claude, also for Pi runs
   that predate the adapter, with one fence to review. `SIGINT` would kill a
   Node TUI without `session_shutdown`, so Pi gets `SIGTERM`.
2. **An interrupted round is a round report with a flag**, not a new event or
   round state. `round_state` stays `waiting`; the distinction is
   `last_round.interrupted` and `round_interrupted` on `run wait`. Idle expiry,
   the round-reported notify hook (`TASKFLEET_STATUS=round-reported`) and
   `run land` treat it like any open round. The hook carries no interrupted
   flag yet; the parent issued the interrupt, so it already knows.
3. **The flag is sticky.** A worker report that races the interrupt amends
   the summary but the round stays interrupted, because the parent did stop
   it. An interrupt after the worker's own report is a reducer no-op.
4. **A provably gone worker without a told exit** (its shim lost too) is
   interrupted without a signal, which turns a run that the crash backstop
   would otherwise fail into a waiting round. This is the parent's told act,
   not an inference. The review noted the narrow case of a worker that crashed on its own
   milliseconds before its shim recorded the exit: an interrupt in that
   window reads the crash as a round end. Accepted (the parent wanted the
   round stopped; the next message resumes cleanly).
5. **Uncommitted changes are not committed or stashed** by Taskfleet; the
   worker is told they are there. Landing remains the parent's choice; the
   skill advises messaging before landing an interrupted round.
6. **Process-group residual.** Like salvage, only the recorded pid is
   signalled; a Claude tool subprocess can outlive the turn.
