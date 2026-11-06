# Result: named resident workers

Shipped as designed in open-habitat `issues/design-named-workers/` and recorded
in ADR 0003 (`docs/decisions/0003-resident-workers.md`). The surface:

- `run create --resident --name <name> [--idle-expiry 24h]`
- `node round-report <run-id> --summary-file <file>` (`node.round_reported`)
- `run wait <name>` settles at once on a round report (`round_reported: true`)
- `run message <name> <text>`: rebase onto the source tip, then resume the same
  Pi (`--session`) or Claude (`--resume`) session in the same worktree
- `run land <name>`: recorded, recoverable, non-terminal merge of a round
- idle expiry: landed → dormant (worktree removed, revived by the next
  message); unlanded → attention, never merged
- `run show` / `run list` / `run wait` expose `resident` (name, `round_state`,
  latest report summary, landing, expiry)
- bundled skills: "Named support workers" in `worktree-spinoff`, pointers in
  `worktree`, `stint-start`, `taskfleet-overview`, `taskfleet-run-overview`

Evidence: `tests/resident.rs` (six production-path scenarios with real Git and
the native materializer), `taskfleet-core/tests/resident_rounds.rs`, a landing
recovery unit test, and `scripts/resident-smoke.sh pi|claude`, which passed
against real Pi and real Claude (Haiku 4.5) with the release binary: the resumed
session recalled a code word given only in round 1 and saw the source commit
landed while it slept.

## Decisions for later review

Settled by judgement during implementation; each is cheap to change.

1. **Landing is the parent's explicit act, not automatic.** Jari's decision is
   that every round reaches `main`. The supervisor never initiates a merge, so
   the parent runs `run land` after each round report; the skill text says so.
   Alternative: `run message` lands first by default.
2. **Names start with a lowercase letter** (`[a-z][a-z0-9-]*`, ≤ 48 bytes), so
   they can never be read as run-id prefixes. Names resolve in `run show`,
   `wait`, `message`, `land`, `cancel`, `merge`, only among this repository's
   live runs; a name-shaped argument that matches nothing keeps the original
   `invalid_run_id` error.
3. **A failing exit after a round report ends the round**, not the run, and the
   crash backstop does not fire after a report; a recorded worker proven gone
   after its report (host restart) also counts as the round's end.
4. **Residents are excluded from the empty-handed auto-retry** (a retry mints a
   fresh worktree and session).
5. **Pi stops itself after a round**: `node round-report` writes a per-attempt
   marker and the bundled adapter calls `ctx.shutdown()` at the next
   `agent_settled`. Claude's `--print` exits with its turn.
6. **A Pi resident mid-round keeps the live adapter path**; a Pi or Claude
   resident with an open round report is always resumed (never steered), and a
   Claude resident mid-round is refused with retryable `worker_running`.
7. **Rebase conflicts on message are left in progress** for the worker, named in
   its first input. `run land` instead aborts a conflicting rebase and lands
   nothing (`land_conflict`), leaving the worktree unchanged.
8. **Dormancy keeps the branch** (wholly reachable from the source) and saves
   the final pane history first, so a dormant resident can still be cancelled
   or merged with complete evidence.
9. **Resumed rounds reuse the worker's pane** (`respawn-pane -k`) only when its
   exact recorded identity holds (socket, session, window, server pid and
   start time); otherwise a new window. Resumed panes keep `remain-on-exit` so
   evidence can capture them after the worker exits.
10. **The round summary is in the event log; the relayed message text is not**
    (it is only the new attempt's input).
11. **`--notify` fires on each round report** with `TASKFLEET_STATUS=round-reported`
    and the summary, deduped per report.
12. **`run merge` and `run cancel` take the resident operation lock**, so they
    never interleave with a resume, a land, or idle expiry.
