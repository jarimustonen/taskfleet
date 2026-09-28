---
name: taskfleet-run-overview
description: Read the output of `taskfleet run list`, `taskfleet run show`, and `taskfleet run wait` to learn the true state of orchestrated agent runs (spinoffs, research, technical decisions, fan-outs) and what each state calls for. Use when asked about run status, when triaging an in-flight or stuck run, or before deciding whether to spawn, wait, salvage, cancel, or discard work.
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# taskfleet-run-overview

`taskfleet` owns the state of agent runs. Every run lives under
`~/.taskfleet/runs/<run-id>/` as an append-only event log plus projections,
and the read verbs expose that state so you never have to reconstruct it from
git, tmux, or a process table. The point of reading it is to answer honestly
what a run is doing, whether its work has landed in the source branch, and
what, if anything, somebody has to do next.

Three verbs cover it:

- `taskfleet run list` — every run, newest first by `created_at`. `--status`
  and `--kind` filter on exact values. `--repo <path>` (including `--repo .`
  from a subdirectory or a linked worktree) keeps the runs whose recorded
  source repository is the same git repository: main and linked worktrees
  match, an independent repository nested inside a checkout does not, and a
  run with no recorded repository never matches.
- `taskfleet run show <run-id>` — one run in full. Inside a worker worktree,
  `run show --current` resolves the owning run from recorded ownership; the
  run-id fragment in a branch name is not authoritative and can be ambiguous.
- `taskfleet run wait <run-id>…` — block until the listed runs settle, then
  emit one summary. Use it instead of a hand-rolled `run show` poll loop: the
  loop in the binary already handles backoff, the stuck shapes described
  below, and a default `6h` ceiling so a wedged run cannot block you forever.

Use `--output json` for one envelope or `--output jsonl` for one envelope per
line. `--output text` is for a human at a terminal; its wording is not a
contract.

## The envelope

```json
{ "schema_version": 1, "data": { ... }, "warnings": ["..."] }
```

`schema_version` is the envelope version this skill was written for. If you
see another number, the payload shapes below may no longer hold; tell the user
the skill and binary disagree rather than reading fields on faith. `warnings`
carries advisory conditions, such as a telemetry scan that could not complete;
pass them on, they never alter the canonical fields.

Failures print the same envelope shape with an `error` object to stderr and
exit non-zero. Branch on `error.code`, never on the prose message:
`run_not_found` (`run wait` alone says `unknown_run` for a well-formed id that
names no run), `node_not_found`, `invalid_run_id` for a malformed run id,
`invalid_value` for another bad argument,
`corrupt_state` for a state file the binary cannot read (including a state
schema newer than it supports; `expected.supported_schema_versions` lists what
it can read), and `corrupt_run` for a run directory that has been tampered
with.

## What a run row says

`run list` returns `data.runs[]`, and `run show` places the same row flat at
the top of `data`, so `data.status`, `data.supervisor`, and the rest read
identically from both verbs.

```json
{
  "run_id": "01HZ...",
  "kind": "spinoff | research | technical-decision | fan-out",
  "lifecycle": "autonomous | interactive",
  "status": "pending | running | done | failed | cancelled",
  "title": "...",
  "created_at": "2026-06-12T10:30:00Z",
  "source_repo": "/path/recorded-at-create | null",
  "source_branch": "main | null",
  "worktree_path": "/path/to/worker/worktree | null",
  "harness": "pi | claude | null",
  "node_count": 1,
  "supervisor": { "pid": 65745, "state": "alive", "alive": true },
  "stalled": false,
  "stillborn": false,
  "attention_required": false,
  "awaiting_input": false,
  "open_discussion_count": 0,
  "telemetry_available": true,
  "telemetry_counts": { ... }
}
```

**`status` is the only progress field.** `done`, `failed`, and `cancelled` are
terminal and frozen; `pending` and `running` are live. `lifecycle` looks like
a progress field and is not. It is recorded once, at `run create`, and says
whether the run is `autonomous` (the supervisor adjudicates the worker's exit
and tears down) or `interactive` (created with `--interactive`; the supervisor
never terminalizes it on its own and waits for an explicit `run merge` or
`run cancel`, so it can sit non-terminal indefinitely by design). An agent
once polled `lifecycle` for `completed` and hung forever. Read `lifecycle` to
know how a run is driven and `status` to know whether it is finished.

**`supervisor.state`** is the liveness of the per-run supervisor process.
`alive` is running. `dead` means a pid was recorded and is no longer that
process: the run is orphaned and `run reattach` revives it. `not-recorded`
means the supervisor never launched or was cleanly torn down after the run
finished; on a `pending` run with no nodes it is the stillborn signature, not a
finished run. `unreadable` means the pid file exists but cannot be parsed, so
nothing is proven either way. The boolean `alive` is kept for old consumers
and collapses the last three states, which is exactly the distinction a
recovery decision needs, so read `state`.

**Three ways a live run can be stuck.** Each is a typed verdict the binary
computes so you do not have to infer it from pids, panes, or branches, and
each has a different remedy:

- `stalled` — the supervisor is confirmed dead and nothing can roll the run
  up. `stillborn` additionally means it died before creating any worker node.
  `run show` and `run wait` flag both shapes; `run list` flags only the
  stillborn one, and only once the run is older than the create window, so a
  supervisor that died mid-run does not show as stalled in the listing.
  Remedy: `run reattach <run-id>` to revive the supervisor, or `run cancel`.
- `attention_required` — the worker exited cleanly but skipped `run merge`,
  so its node never went terminal. The supervisor may be perfectly healthy,
  which is why this is not a stall and `run reattach` does nothing for it.
  The `attention` object carries the worktree path, worker pid, pending age,
  and a `resume_hint`. Remedy: `run salvage <run-id>`, which verifies the old
  worker is gone (or fences it when you pass `--fence`) and drives `run merge`
  from the preserved worktree; or `run cancel`. When a run matches both this
  and a stall shape, attention wins and the stall flags are cleared, because
  the manual finish is the right fix.
- `awaiting_input` — the worker is alive and has asked for a human decision.
  `awaiting_input_detail.discussion_items[]` holds the questions with their
  options and recommended defaults. This is the one stuck shape whose answer
  belongs to a person, not to a command.

## What `run show` adds

Under the flat row, `data.manifest` carries the recorded facts
(`schema_version`, `updated_at`, `source_repo`, `source_branch`,
`worktree_root`, `harness`, `selection`, `parent_run_id`, `parent_node_id`,
and the same `status` as the flat row), `data.counts.nodes` the node count,
and `data.telemetry[]` per-node advisory activity that never changes status.
The computed fields are the ones decisions turn on:

- `landed` / `landed_method` — whether the worker's committed content is in
  the source branch. `git-verified` means patch-id equivalence against the
  current source tip, which survives a rebase on the caller's side;
  `report-marker` means only the recorded merge says so; `unverified` means
  neither could be established. Trust this over your own `git merge-base`
  reasoning.
- `report` — the terminal worker report, present only for a single-node run
  and only once its worker has reported. A fan-out has no single report; read
  each worker with `node show <run-id> <node-id>`, whose `data.report` and
  `data.last_report` carry the same value. `run wait` folds only a report's
  `summary` in; the full `discussion_items`, `spinoff_proposals`, and
  `wrap_up_recommendations` arrays are on `run show` and `node show`.
- `recoverable_work` — on a `failed` run, the block the supervisor stamps when
  the dead worker's branch has commits ahead of source, so salvageable work is
  visible without running `git log` yourself.
- `false_failed` — present only when the run is `failed`, git verifies its
  content is in source, and no `run merge` is on record: the worker merged
  with raw git and then died. It is a hint, never an auto-success; the run
  stays `failed` until `run salvage <run-id>` records the merge through the
  real merge machinery (idempotent against content already integrated) and
  terminalizes it to `done`. Finishing a run with raw `git merge` is what
  creates this state: it bypasses the recorded merge transaction, so the
  run's own history cannot tell that the work landed.
- `preserved_work[]` — always present; the current inventory of worktrees and
  branches still on disk for terminal nodes, with `cleanliness`,
  `unmerged_commits`, `verification`, and a `reason`. They exist because
  teardown deliberately keeps unmerged, uncommitted, or unverifiable work
  instead of deleting it, so a cancel or a crash never silently loses an
  agent's edits. An empty array means nothing is retained. A non-empty row is
  not a success signal and is not disposable just because the run was
  cancelled.
- `evidence` — archived Pi transcript and pane artifacts for a single-node
  run's worker, with an explicit `pending | failed | complete` status.

```bash
# skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
taskfleet run show "$run_id" --output json | jq '.data.report'
# Node-level projection-compatible probe:
# skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
taskfleet node show "$run_id" n-0001 --output json |
  jq '.data.report // .data.last_report'
```

## What `run wait` returns

Waiting can cover several ids, so its shape differs from `run show`:
`data.outcome` is `condition-met` or `timed-out`, and per-run results are in
`data.runs[]` (`run_id`, `status`, `merged`, `report_only`, `landed`,
`landed_method`, `stalled`, `attention_required`, `awaiting_input`,
`preserved_work`, plus `summary`, `error`, `attention`,
`awaiting_input_detail`, and `recoverable_work` when present). There
is no `data.status`; read the runs array. Read `outcome` first: `timed-out` is
not completion, and the field is authoritative even when a pipeline swallowed
the exit code. Exit codes are `0` condition met, `1` usage or unknown run,
`2` timeout, and `3` under `--fail-on-error` when a settled run did not finish
`done`, or finished `done` without a recorded `run merge` (`report_only`)
while still holding `preserved_work`.

A run *settles* the wait when it goes terminal or when it can no longer
progress on its own: stalled, attention-required, or awaiting input past a
short grace. A settled run can therefore still be `pending` or `running`; that
is the wait telling you it needs a hand, not a defect. `--any` returns on the
first settled run; `--all` (the default) waits for every one.

```bash
taskfleet run wait "$run_id" --output json |
  jq '.data | {outcome, runs: [.runs[] | {run_id, status, summary}]}'
```

## Acting on what you read

Reading is free. The follow-up verbs differ in what they put at risk, and that,
rather than their category, decides how much care each deserves.

- `run cancel <run-id>` stops a run and preserves both committed and
  uncommitted work. With `--node <id>` it cancels one live fan-out child,
  preserving its branch, while the run stays live until every sibling settles.
  Re-cancelling a `cancelled` run, or a node that is already terminal, is a
  no-op success; a `done` or `failed` run is refused with
  `run_already_terminal`. Cancelling costs nothing but the worker's remaining
  time.
- `run reattach` only restarts a supervisor. `run salvage` changes the source
  branch, which is the sanctioned way to land work; `--dry-run` shows what it
  would do first. Never finish a run with raw git in its place.
- `run discard <run-id> --reason "..."` permanently deletes one failed or
  cancelled node's retained worktree and branch after recording who
  authorized it and why. It is the only follow-up here that destroys work:
  verified dirty work needs `--force`, more than one retained node needs
  `--node`, and unverifiable ownership or git state is refused outright. Read
  the `preserved_work` row, run `--dry-run --output json`, and decide whether
  anything should be harvested by hand before you commit to it. Discard leaves
  the run's status and `landed` truth unchanged, and a repeated `run cancel`
  is not a discard.

```bash
taskfleet run discard <run-id> --reason "superseded" --dry-run
# For a selected dirty row:
taskfleet run discard <run-id> --node n-0002 --reason "superseded" --force
```

Before spawning more work, list what is already running: a second fan-out on
the same scope competes with the first for the same files, so resume or wait
instead. When a run is `failed`, its cause is in the event log
(`taskfleet event tail <run-id>`); quote it rather than guessing.

## Install or upgrade `taskfleet`

This skill was installed for `taskfleet {{CLI_VERSION}}`. On the
first invocation in a session, run
`taskfleet version --output json`, compare `.data.version` to
`{{CLI_VERSION}}`, and:

- **Missing**: tell the user to install through a published distribution channel
  outside this repository workflow, then stop.
- **Older**: tell the user the skill expects `{{CLI_VERSION}}` and suggest
  upgrading via the channel they originally used
  (`brew upgrade jarimustonen/taskfleet/taskfleet` or the shell installer),
  then stop; the `run list` / `run show` / `run wait` payload shapes may have
  changed.
- **Newer**: tell the user the installed skill is stale and stop. Refreshing
  installed bundled instructions is published-tool maintenance outside
  repository work; never run `skill install` as part of this workflow.
- **Equal**: proceed.
