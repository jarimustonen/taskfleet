---
name: fan-out
description: Fan out five or more similar, fully independent units as parallel autonomous worktrees — one `taskfleet run create --kind fan-out` driver run whose agent spawns one parent-pointed child run per unit, bounds concurrency, waits with `run wait`, and merges the batch back. Use when the user says `/fan-out <batch>` or asks to apply the same operation to every item of an enumerated set, each writing a disjoint output. NOT for fewer than five units (`/worktree-spinoff` each), units that edit the same file, units that depend on each other (issuectl-scheduled `/stint-start` waves), generic parallel shell commands, or per-unit human review (`--interactive` spinoffs).
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# fan-out

A fan-out is one driver run plus one child run per unit. The driver is an
ordinary autonomous agent in its own worktree, created with
`taskfleet run create --kind fan-out`; what makes it a driver is its brief,
which tells it to enumerate the units, spawn a child for each with
`--parent-run-id` / `--parent-node-id`, keep only so many alive at once, wait
for them, and close the batch. Each child is an autonomous unit that reads one
input, writes one disjoint output, commits, and merges itself back. The
driver's supervisor forks a supervisor for every child and does not let the
driver run finish until every child has settled.

Two agents read this skill. The session that receives `/fan-out` decides that
the request is a fan-out, writes the driver brief and the unit brief template,
and creates the driver run. The driver agent then executes the brief; it does
not have this skill in front of it, so everything it needs to know about
spawning, waiting, and closing goes into the brief. If the run / supervisor /
node vocabulary is new to you, read `taskfleet-overview` first; the shared
worker contract is in `worktree-spinoff`.

## Is this a fan-out?

It is when five or more units undergo the same operation on different inputs
("convert each receipt PDF to JSON", "regenerate each NFO under `media/`",
"apply this codemod to each of 47 packages") and each unit's output is its own
file or path, so siblings never touch the same file. Fewer than five is just
that many spinoffs. Units that edit a shared file are serialized spinoffs, or
the work is restructured until the outputs are disjoint. Units that depend on
each other belong to issuectl's DAG and bounded `/stint-start` waves; taskfleet
has no dependency scheduler. Per-unit human review means each unit is a
spinoff created with `--interactive`. A batch of plain shell commands with no
agent judgment in them is a shell loop, not a fan-out.

## What is at stake

**The branch the units merge into.** Every child's `run merge` modifies the
branch recorded as its `source_branch`, in the worktree where that branch is
checked out, and refuses if that worktree has uncommitted or untracked
changes. If the children merge straight into the user's `main`, every one of N
merges depends on the user's own worktree being clean at that moment, and the
source moves N times. The default here is therefore an integration branch: the
driver's own branch. Children are created from inside the driver's worktree
with `--source-branch <driver-branch>`, merge into it, and the driver merges
the whole batch into the real source once. The cost is that the driver's
worktree has to stay clean for the entire batch: the driver keeps its scratch
(unit lists, logs) under `/tmp` or committed, never as untracked files in its
tree, and it closes only after the last child has settled, because its own
teardown would remove the worktree the children merge into. Merging children
straight into the source is a legitimate choice when partial results should
land as they come and the source worktree is quiet; the driver then has
nothing to merge and closes with a direct report.

**The units' work.** Preserving work is the supervisor's job: a child that
fails or reports blocked keeps its branch and worktree for a human, and a
cancelled child keeps them whenever they hold commits or changes that are not
in the source. The loss that nobody recovers from is a unit that never
terminalizes, because a child that neither merges nor reports stays alive
forever, and the driver run cannot complete while any child is live. Every
unit brief therefore ends with exactly one terminal path.

**The machine.** Each child is an agent process, a tmux window, and a git
worktree. Ten at once is a reasonable default; more only when the units are
cheap and the host has headroom, fewer for heavy model or IO work. On macOS
five or six foreground spawns exhaust pseudo-terminals and the batch fails
mid-way with `workmux_add_failed`, so children are created `--headless` or in
one named `--tmux-session <batch-slug>`; that also keeps N windows out of the
user's foreground session.

**The user's attention.** Successful units are silent: a receipt that
converted cleanly is not a discussion item, and N of them drown the user. What
the user needs to hear, once, at the end, is which units did not complete and
why, with enough detail to retry or accept. The driver's closing report is
that channel.

**The user's installed taskfleet.** Inside the taskfleet repository a unit may
`cargo build --release` and run `./target/release/taskfleet …` from its
worktree, but it never installs, replaces, or removes the installed binary or
bundled skills by any mechanism (`cargo install`, Homebrew, a manual copy, any
`skill install` variant). The installed release is the user's production tool
and is upgraded through a distribution channel, never by a worker.

**Issues.** The driver owns any issue interaction, typically one epic closed
when the batch is done. Children never touch `issuectl`: N children updating
the same issue race each other. Both parent flags on a child put the spawning
skill in driver mode, which skips issue detection and the issue-closing
contract for exactly this reason.

## How the pieces fit

Facts about the binary that the briefs rely on:

- **The driver run is a worker.** `run create --kind fan-out` materializes a
  worktree, a branch, a tmux window, node `n-0001`, and a supervisor, exactly
  like a spinoff. The kind is a label; it carries no special merge policy and
  no unit ledger. The one thing the supervisor withholds from it, and from
  every parent-pointed child, is the bounded auto-retry a spinoff gets when
  its agent dies: a child whose agent dies settles as `failed` and the driver's
  retry budget re-spawns it. Enumeration, concurrency, and unit state are the
  driver agent's job, and the durable record of what exists is the source
  branch itself: the outputs that are committed are the units that are done.
- **Children are runs with a parent pointer.** A `run create` with
  `--parent-run-id <driver-run-id> --parent-node-id n-0001` writes a
  `child.spawned` event on the driver's log (with `child_run_id`,
  `child_title`) and a `children[]` entry on the driver's node. The child's
  create envelope reports `supervisor: "delegated-to-parent-supervisor"`: the
  driver's supervisor forks the child's supervisor when it sees the event.
  If the driver's supervisor is dead, no child gets a supervisor until
  `taskfleet run reattach <driver-run-id>` revives it. There is no
  `child.report` event; a child's terminal report lives on the child run
  (`node show <child-run-id> n-0001`), and the driver's log records
  `supervisor.cursor_advanced` when the parent has consumed it.
- **Roll-up.** The driver run reaches a terminal status only when its own
  node is terminal and every child run is terminal. Its status is derived from
  its own node's report, not from the children's outcomes, so the driver's
  report is what tells the truth about the batch. `run wait <driver-run-id>`
  therefore settles once for the whole batch. Cancelling the driver does not
  cancel children; each child run is cancelled by its own id.
- **Concurrent merges serialize, and the loser gets `merge_source_moved`.**
  Every `run merge` records the target's commit id before it takes the merge
  lock and refuses to fast-forward if the target moved meanwhile. When several
  children finish together, the first through the lock lands and each one
  queued behind it exits with `merge_source_moved`. Nothing was merged and no
  report was submitted; re-running the same `run merge` records a fresh
  expectation and lands. `merge_in_progress` is either the lock timeout (600 s
  by default) or a second `run merge` already driving the same node, and is
  retried the same way.
- **Idempotency keys return the existing run, whatever its state.** A key
  makes a spawn safe to repeat after a transient error (`idempotent_replay:
  true` in the envelope). It also means a retry of a failed unit needs a new
  key, such as an attempt suffix, or it silently gets the failed run back.
- **`--dry-run` is refused on a child create** (`dry_run_unsupported`),
  because parent publication cannot be previewed. Preflight a profile once
  with the parent flags omitted, then add them back.
- **The title becomes the branch and window name**, slugified and truncated,
  so the unit id goes at the front of a child's title where truncation cannot
  eat it.
- **`run create` inside tmux is required**, or `--headless` /
  `--tmux-session`; the driver's own window satisfies this for the children it
  spawns.

## The driver brief

The driver brief is the whole operating manual of the batch. It contains:

- **The enumeration**, stated so that the same list in the same order can be
  re-derived from the source branch (a glob, a committed list, an input
  file). That is what lets a second driver, or a human, finish an interrupted
  batch: a unit whose output is committed is done, everything else is
  pending. Zero units means the batch stops before spawning anything and says
  why.
- **The concurrency bound** and the reason for it.
- **The unit brief template** (next section) with the interpolation points.
- **The spawn command.** Children are created from the driver's worktree:

  ```
  taskfleet run create \
    --kind spinoff \
    --title "<unit-id> <batch-slug>" \
    --task "<unit brief with this unit interpolated>" \
    --source-branch <integration-branch> \
    --headless \
    --parent-run-id <driver-run-id> \
    --parent-node-id n-0001 \
    --idempotency-key <batch-slug>-<unit-id>-a1
  ```

  The driver's run id is in the generated run context at the top of its
  prompt, and its branch is `git rev-parse --abbrev-ref HEAD` in its
  worktree. Passing `--source-branch` explicitly beats relying on the
  captured default. A long unit brief goes through `--prompt-file`.
- **The wait loop.** Keep up to the bound alive; wait on the live set with
  `run wait`, which has the terminal set and the backoff built in and exits
  `2` on its timeout (default 6 h; pass `--timeout` for a long campaign):

  ```
  taskfleet run wait <child-run-id> <child-run-id> --any --timeout 2h --output json
  ```

  Read `data.outcome` first (`condition-met` or `timed-out`), then
  `data.runs[]` for each run's `status` and `summary`; several may have
  settled at once. Spawn the next unit for every freed slot. A settled child
  with `status: failed`, or `done` with a report disclosing a failure, is a
  failed attempt.
- **The retry budget**: a finite whole-unit count, stated as total attempts
  or as retries after the first, each attempt with a fresh idempotency key.
  A child's own bounded tool retries are separate. At exhaustion the unit is
  recorded failed and the independent units continue.
- **What to retain**: every attempt's disclosure, including attempts later
  superseded by success, identified by unit and attempt. The closing report
  never reduces them to a count.
- **The closing recipe** for the driver (below), including that it waits
  for the last child before closing.

## The unit brief

Each child gets a self-contained brief with one unit interpolated:

- **Objective** for unit `<id>`, the **input path** it reads, and the
  **output path** it writes, derived from the unit id or a hash so two units
  cannot collide.
- **Done criteria**: the output exists and is committed. If a unit changes
  code, the repository's exact green gate from its `AGENTS.md` is copied in,
  release mode, lockfile, and warnings-as-errors included, rather than
  paraphrased into a debug-mode `cargo test`; a missing prerequisite such as
  `cargo-nextest` is reported, not installed globally; tool-sensitive tests
  run under a stripped `PATH` because a developer machine is not a bare CI
  runner.
- **Repository-local build safety**, the installed-taskfleet rule above, when
  the repository is taskfleet.
- **Silence.** A successful unit surfaces nothing: no discussion items, no
  spin-off proposals. A failed or incomplete tool is the one exception and
  follows the disclosure contract below.
- **The disclosure contract and the closing recipe.** For each unit, copy the disclosure contract below
  and the closing recipe into the spawning brief verbatim; the report shape is
  an interface the supervisor and the driver parse.

## How a child closes

A child takes exactly one terminal path. Completed work goes through
`run merge`, which rebases and merges the branch into the recorded source and
submits the terminal report stamped `via: "explicit-merge"` in one call. Work
blocked by a required failure does not merge and submits a direct
`success: false` report. Either releases the slot; neither leaves the batch
stuck.

Resolve the owning run id from the durable ownership record, never from the
branch name, whose short fragment is display metadata that can repeat:

```bash
run_id="$(taskfleet run show --current --output json | jq -er '.data.run_id')" || {
  echo "failed to resolve exact owning run id" >&2
  exit 1
}
```

This fails closed on missing, duplicate, stale, or malformed evidence; if it
fails, report the error rather than guess.

A unit with nothing structured to say merges with the minimal auto-report:

```bash
taskfleet run merge "$run_id"
```

No `--source` is needed; the integration branch is the child's recorded
`source_branch`. On `merge_source_moved` or `merge_in_progress` nothing was
merged and the node is still live: re-run the same command. On
`merge_failed` the message carries the backend's stderr (a dirty target, a
missing target worktree, or a real conflict); resolve it (or `/complex-rebase`
for a deeply diverged branch) and re-run. After a clean merge the supervisor
tears down the window, worktree, and branch within a second or two; the
worker does not touch tmux or git itself and does not resubmit because
`run show` still reads a non-terminal status for a moment.

A unit that must disclose an optional failure uses the full report instead.
These field names are read by the supervisor and the driver; an unknown key
passes validation and is never read, and the `via` and `origin` provenance
keys are stamped by the binary, never taken from the file:

```bash
cat > /tmp/node-report-${run_id}.json <<'JSON'
{
  "success": true,
  "summary": "<unit-id>: <one-line outcome>",
  "discussion_items": [],
  "spinoff_proposals": [],
  "wrap_up_recommendations": []
}
JSON

taskfleet run merge "$run_id" --report-file /tmp/node-report-${run_id}.json
```

`success` is the one required field; the summary starts with the unit id so
the driver's log stays legible; the per-run temp path matters because siblings
sharing `/tmp/node-report.json` clobber each other. The file is validated
before the merge runs. The per-field shapes of the advisory arrays are in
`worktree-spinoff`.

### Tool and sub-workflow failure disclosure

Before closing, the unit inventories every failed or detectably incomplete
tool, command, external service, review, or delegated workflow.

A step the brief or the done criteria **required**, still failed or
incomplete, blocks this attempt. Do not call `run merge`; write a
`success: false` report to `/tmp/node-report-${run_id}.json` and submit it
with

```bash
taskfleet node report "$run_id" n-0001 --from-file /tmp/node-report-${run_id}.json
```

(`n-0001` is the only node a child has). An **optional/advisory** failure
may continue only when the unit's output is independently complete and safe,
and is then disclosed in the full `success: true` report passed to
`taskfleet run merge "$run_id" --report-file /tmp/node-report-${run_id}.json`,
never hidden behind the minimal auto-report.

Requested completeness is a contract: a missing command result, source, or
artifact is incomplete, not done. Retry only within a finite bound the brief
grants, record each attempt, and take the required or optional path at
exhaustion; whole-unit retries belong to the driver.

Every distinct failure goes into one aggregate `discussion_items[]` entry
whose `topic` starts `Tool/sub-workflow failure —`, coalescing repeated
attempts of the same one: tool and purpose, expected completeness, observed
error, attempts, affected step, whether work continued and why that was safe,
suggested bug surface, and a stable artifact/log path when there is one.
Actionable retry/recover/accept/file steps go in the item's `options`. The
entry stays under 2 KiB with only a short redacted excerpt, never
secrets, credentials, personal data, environment dumps, or unbounded logs.
Top-level `summary` and `success` say whether the attempt is blocked or
completed; do not put them inside the item, and do not add a schema or
supervisor state.

## How the driver closes

Once the last child has settled, the driver writes one aggregate report:
counts per outcome in `summary`, and one `discussion_items[]` entry per unit
that did not complete (or completed with a disclosed failure), naming the
unit and attempt with the child's disclosure carried through, so the user can
retry or accept each one without opening N child runs.

If every required unit completed, the driver merges the integration branch
into its recorded source with `run merge` and that report file, resolving its
own run id with `run show --current` exactly as a child does. If any required
unit remains failed, the batch is not complete: the driver submits the report
with `success: false` through `node report` and does not merge. That
preserves the integration branch and worktree with every landed unit on it,
and the driver's closing summary tells the user which units are missing;
`taskfleet run salvage <driver-run-id>` or a manual merge lands the partial
batch if the user decides that is what they want. Where children merged
directly into the source there is nothing to merge either way, and the driver
closes with a direct report whose `success` says whether the batch is whole.

## What the caller hears

`run create --kind fan-out` returns the usual envelope: `data.run_id`,
`data.branch` (the integration branch), `data.tmux_window`
(`<workmux-reported-window-name>`), `data.worktree_path`, and
`data.supervisor` (a pid; a string there outside `--dry-run` and an
idempotent replay means nothing is driving the batch and the user needs to
know). Tell the user the driver run id, the unit count and concurrency, the
branch the units merge into and the branch the batch finally lands on, the
tmux session you placed it in, and an estimate if the per-unit cost is known.

Be precise about how completion reaches them: the batch runs out of band and
nothing re-invokes this session by itself. Promise "I'll tell you when it's
done" only if you wired `--notify <cmd>` on the driver (fired by its
supervisor on the terminal transition, at least once, in the supervisor's
environment rather than your login shell, so a file append the harness
watches is the robust sink) or started a background
`taskfleet run wait <driver-run-id>` through a harness facility that
re-invokes you. Otherwise say plainly that they check `run show` or ask you
to wait.

## Following a batch

- `taskfleet run show <driver-run-id>` — the driver's status, its supervisor
  state, and `preserved_work` for its own node. It counts the driver's own
  node only; there is no aggregate of the children here.
- `taskfleet event tail <driver-run-id> --follow` — `child.spawned` per unit
  (with the child's run id and title), `supervisor.cursor_advanced` when a
  child's terminal report has been consumed, and the terminal `run.status`.
- The children of a driver:

  ```bash
  # skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
  taskfleet node show <driver-run-id> n-0001 --output json | jq '.data.children'
  ```

  Each entry is `{run_id, node_id}`; `run show <child-run-id>` gives that
  unit's status, `landed` flag, and terminal report at `data.report`, and
  `node show <child-run-id> n-0001` gives the node's status and the same
  report.
- `taskfleet run wait <driver-run-id>` — blocks until the whole batch has
  settled, exits `2` on timeout and `3` under `--fail-on-error` for a failed
  driver. Read `data.runs[]`, never a top-level `data.status`, and branch on
  `status`, not `lifecycle`, which is a fixed category that never matches a
  terminal value.
- `taskfleet run cancel <child-run-id>` — unblocks one stuck unit without
  killing the batch; the child's branch and worktree are preserved when they
  hold unmerged commits or uncommitted changes. The driver sees it settle as
  `cancelled` and treats it as a failed attempt.
- `taskfleet run reattach <driver-run-id>` — restarts the driver's
  supervisor, which adopts the children's surviving supervisors and re-forks
  the dead ones. It does not restart the driver agent: a driver whose agent
  died rolls up as `failed` once the children finish, and the batch is
  completed by a new driver from the same enumeration, since committed
  outputs are the resume state.

## Errors

Failures print `{"schema_version": 1, "error": {"code": "<code>", "message":
"..."}}` on stderr with a non-zero exit; branch on the code. Beyond the
spinoff codes in `worktree-spinoff`, a fan-out is likely to meet:

- `parent_not_found` — the driver run id on a child create names no run.
- `dry_run_unsupported` — `--dry-run` on a child create.
- `base_ref_not_found` — the `--source-branch` does not resolve locally.
- `workmux_add_failed` — workmux refused the worktree: uncommitted changes on
  the source branch, a path collision, or macOS PTY exhaustion mid-batch
  (headless placement avoids the last). The driver records the unit as a
  failed attempt and continues; one git refusal does not abort the batch.
- `merge_source_moved` / `merge_in_progress` — a sibling landed first;
  re-run `run merge`.
- `invalid_merge_report` — a `--report-file` handed to `run merge` set
  `success: false` or `cancelled: true`; a blocked unit reports through
  `node report` instead.
- `supervisor_spawn_failed` on the driver — the run exists but nothing
  drives it; inspect `<dir>/supervisor.stderr.log` and `run reattach`.

## Install or upgrade `taskfleet`

This skill was installed for `taskfleet {{CLI_VERSION}}`. On the first
invocation in a session, run `taskfleet version --output json`, parse the
JSON, and read `.data.version`. Compare it to `{{CLI_VERSION}}`:

- **Missing**: tell the user to install through a published distribution channel
  outside this repository workflow, then stop.
- **Older than `{{CLI_VERSION}}`**: tell the user the skill expects
  `{{CLI_VERSION}}` and suggest upgrading via the same channel they
  originally used (`brew upgrade jarimustonen/taskfleet/taskfleet` or
  re-run the shell installer). Stop and wait — the child-spawn and `run wait`
  surfaces may have changed.
- **Newer than `{{CLI_VERSION}}`**: tell the user the installed skill is
  stale and stop. Refreshing installed bundled instructions is published-tool
  maintenance outside repository work; never run `skill install` as part of
  this workflow.
- **Equal**: proceed normally.

## Example

```
/fan-out Convert every PDF in corporate/receipts/2026-05/ to JSON via vision OCR — one output file per input under corporate/receipts/2026-05/json/
```

The session enumerates the PDFs, writes the driver brief with the unit
template and a concurrency of 10, and creates the driver:

```
taskfleet run create \
  --kind fan-out \
  --title "receipts 2026-05" \
  --prompt-file /tmp/fan-out-driver-brief.md \
  --headless \
  --idempotency-key receipts-2026-05-driver
```
