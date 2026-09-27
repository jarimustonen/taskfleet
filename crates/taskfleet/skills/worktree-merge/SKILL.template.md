---
name: worktree-merge
description: Close a taskfleet worktree run with one `taskfleet run merge` call — rebase and merge the branch into its source branch, submit the terminal `node report` stamped `via: "explicit-merge"`, and let the supervisor remove the tmux window, worktree, and branch. Use when an autonomous worktree (spinoff, research, technical-decision, bug-analysis, or a fan-out unit) reaches its merge-and-report step, or when a human has finished reviewing an `--interactive` run and wants it merged. Replaces the old two-step `/worktree-merge` + `taskfleet node report` sequence. When the branch and its source have diverged too far for an ordinary rebase, recover with `/complex-rebase` and re-run.
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# worktree-merge

A worktree run ends in one of two ways: its work is merged into the source
branch, or it is not. `taskfleet run merge` is the merged way, and it is the only
thing taskfleet accepts as success. It rebases the run's branch onto the source
branch and merges it, appends the terminal `node.report` stamped
`via: "explicit-merge"` in the same call, and makes sure a supervisor is alive
to read that report. The supervisor then closes the tmux window, removes the
worktree, and deletes the branch. Nothing is left for you to clean up, and there
is no separate `node report` step on this path.

You are reading this either as the worker closing its own run (the sibling
skills copy this recipe into every brief), or as a human's session finishing an
`--interactive` run after review, where the supervisor deliberately waits for
exactly this call or `run cancel` and never tears down on its own. If the run /
supervisor / node vocabulary is new to you, read `taskfleet-overview` first.

This skill is only for a worktree that taskfleet created; a branch with no run
under `~/.taskfleet/runs/` is ordinary git, and `/git-rebase` plus a normal merge
is the right tool. A blocked outcome, work you could not finish, is not a merge
either: it goes through a direct `taskfleet node report` with `success: false`,
which preserves the branch and worktree for a human. `run merge` refuses a report
that says `success: false`, because a report contradicting the merge that just
landed would either mis-terminalize the node or strand its teardown.

## What is at stake

**The source branch.** The merge target is the branch the run was spawned from,
recorded as the run's `source_branch` (usually `main`; an integration branch for
a fan-out unit). That branch is modified in place, in the worktree where it is
checked out, so it has to be checked out somewhere and clean; a colleague's
uncommitted edit there blocks the merge rather than being swept into it.

**Your work.** Every merge failure leaves the node live and submits no report,
so a failed `run merge` costs nothing but a retry. The supervisor's teardown
runs only after a confirmed merge, and on every other path it checks for
unmerged commits and uncommitted files before removing anything. Doing the
teardown by hand (`tmux kill-window`, `git worktree remove`, `git branch -d`)
bypasses those checks and races the supervisor, which is why the supervisor is
the sole teardown actor and you leave the window to close under you.

**Concurrent merges.** Fan-out units merging into one branch are serialized by a
lock directory in the shared git dir. Waiting on it is normal; the default wait
is 600 seconds (`MERGE_LOCK_TIMEOUT` overrides), and only a crashed merge leaves
the lock stale, in which case the error message names the directory to remove.

**Attention.** A worker has nobody to ask; if it is genuinely stuck it reports
blocked rather than sitting at a prompt that looks like a hang. A session acting
for a human resolves an ordinary conflict itself by reading both sides, and
reaches for `/complex-rebase` when the two branches overlap in intent. Only a
conflict whose two resolutions differ in a way the human would care about is
worth a question.

## Before you merge

The merge refuses an uncommitted worker tree, including untracked files, so
commit first (`/git-commit`). It also refuses when the checked-out branch is
`main` or `master` itself, so confirm you are on the run's branch with
`git rev-parse --abbrev-ref HEAD`. Check the binary once per session as
described under "Install or upgrade" below.

Resolve the owning run from the durable ownership record, not from the branch
name. The short fragment in `wt/<fragment>-<slug>` is display metadata that can
repeat, and `run merge` treats its argument as an id (or unambiguous prefix),
not as a branch:

```bash
run_id="$(taskfleet run show --current --output json | jq -er '.data.run_id')" || {
  echo "failed to resolve exact owning run id" >&2
  exit 1
}
```

`--current` requires exactly one node whose recorded worktree path and branch
match this checkout, and it fails closed on missing, duplicate, stale, or
malformed evidence and on a detached HEAD (`run_owner_not_found`,
`run_owner_ambiguous`, `run_owner_stale`, `run_owner_malformed`). If it fails,
report that rather than guess: a merge against the wrong run lands git state
the run state can never reflect.

## The report

Without `--report-file`, `run merge` submits a minimal
`{"success": true, "summary": "merged <branch> into <source> via run merge"}`,
which is enough for a unit with nothing structured to say. Anything a human or
the calling driver should see, a decision that needed a human call, follow-up
work worth spawning, wrap-up advice, or a disclosed optional-tool failure, goes
in a report file passed with `--report-file`. `run merge` validates it before
touching git and stamps `via: "explicit-merge"` itself.

These field names are an interface: the supervisor and the caller read exactly
these, and an unknown key passes validation and is never read.

```bash
cat > /tmp/node-report-${run_id}.json <<'JSON'
{
  "success": true,
  "summary": "<one-line outcome>",
  "discussion_items": [],
  "spinoff_proposals": [],
  "wrap_up_recommendations": []
}
JSON
```

- `success` is the one required field, and for `run merge` it has to be `true`.
- `summary` is a one-line human-readable result.
- `discussion_items[]` holds decisions that needed a human: `{"topic":
  "<non-empty>", "severity": "discuss|critical|info", "options": ["…"]}`.
- `spinoff_proposals[]` holds follow-up work worth spawning: `{"proposed_title":
  "<non-empty>", "proposed_kind": "spinoff|research|technical-decision|fan-out",
  "rationale": "<why>"}`; only those four kinds exist.
- `wrap_up_recommendations[]` is an array of strings for the caller.

The advisory sections are validated leniently: a malformed element or a
non-array field is dropped with a warning (`dropped spinoff_proposals[1]: …` in
`warnings`, structured in `data.report_advisory_warnings`) so a typo never
blocks a merge of already-committed work; `success` and the outcome fields stay
strict. The per-run path matters because two concurrent units writing a shared
`/tmp/node-report.json` clobber each other.

## Merging

```bash
taskfleet run merge "$run_id" \
  [--source <branch>] \
  [--report-file /tmp/node-report-${run_id}.json]
```

`--source` overrides the recorded `source_branch`; with neither, the backend
auto-detects the `main`/`master` worktree. `--node-id` defaults to `n-0001`,
the only node a single-worker run has. `--dry-run` reads the report file,
resolves the target worktree, and checks its cleanliness without taking the
lock, running the merge, or appending any event, so it doubles as a preflight
for a doubtful `--source` or report file. Output defaults to `--output jsonl`.

Inside the call, in order: the report file is validated; a `merge.started`
transaction records the source tip and worker commit; the bundled backend takes
the lock, checks both trees, verifies the source tip is still the recorded one,
and runs the rebase-and-merge; the report is appended; a supervisor is confirmed
or restarted. The recorded transaction is what makes a retry safe: if the driver
dies after git moved but before the report was written, the next `run merge`
recognises the landed work by commit id and completes the record instead of
merging again.

A clean merge returns:

```json
{
  "schema_version": 1,
  "data": {
    "run_id": "01HZ...",
    "node_id": "n-0001",
    "branch": "wt/<fragment>-<slug>",
    "source": "main",
    "merged": true,
    "report_seq": 7,
    "supervisor": {"state": "alive"}
  }
}
```

`merged: true` means the branch landed. `report_seq` is the terminal report's
event sequence; it is absent only when a crashed earlier merge was completed
from its transaction record. `supervisor.state` says who consumes the report:
`alive` (the normal case), `reattached` (the supervisor had died and was
restarted; teardown is running), `terminal` or `not-supervised` (nothing left to
tear down), or `deferred`, where auto-restart failed and the envelope carries a
`recovery_command` (`taskfleet run reattach <run-id>`) that you run so the
window, worktree, and branch actually go away. Teardown follows within a second
or two; a worker's session ends as its window closes, and `run show` reading
`pending` for that moment is not a reason to resubmit.

## When it fails

Failures print `{"schema_version": 1, "error": {"code": "<code>", "message":
"..."}}` on stderr with a non-zero exit; branch on the code. On every failure
below no report was submitted and the node is still live, so the same command
can be re-run once the cause is fixed.

- `merge_failed` — the backend refused or the rebase conflicted; the message
  carries its stderr. Uncommitted changes in the worker tree or in the target
  worktree, the target branch not checked out in any worktree (a parent
  worktree was removed; recreate it or correct `--source`), or conflicts to
  resolve, with `/complex-rebase` for a deeply diverged pair.
- `merge_in_progress` — another merge into the same target held the lock past
  the timeout, or another `run merge` is driving this node. Retry; if nothing is
  running, the message names the stale lock directory.
- `merge_source_moved` — the source tip moved between recording the transaction
  and taking the lock. Rebase onto the new tip and re-run.
- `run_already_terminal` — the run is already done or cancelled and its
  worktree is gone; a cancelled run is refused even under `--dry-run`. If
  teardown looks incomplete, `taskfleet run reattach <run-id>`.
- `worktree_missing` — a live run whose worktree no longer exists; if the run
  actually finished, `run reattach` completes the roll-up.
- `no_worktree` / `no_branch` — a driver node, not a worker; drivers are not
  merged.
- `invalid_merge_report`, `schema_violation`, `report_not_object`,
  `report_file_invalid_json`, `report_file_unreadable`,
  `report_file_too_large` (1 MiB) — the report file, caught before any git
  mutation.
- `merge_recovery_unverifiable` — a prior transaction is pending and git could
  not be consulted; resolve the worktree or source repo and retry rather than
  overwrite it.

## Afterwards

The run directory outlives the teardown. `taskfleet run show <run-id>` reads
`status: done` with a `landed` flag, and `taskfleet node show <run-id> n-0001`
shows the report you submitted with `via: "explicit-merge"`. When a human is
watching, tell them what merged into which branch and that the worktree and
window are being removed automatically. In autonomous mode there is nothing to
say: the report is the handoff the caller reads.

A run that a worker left unmerged (a clean exit that skipped `run merge`, or a
failure with a preserved branch) is finished from outside by
`taskfleet run salvage`, which fences the old worker and drives this same merge
from the preserved worktree; that is an operator's move, not a worker's.

## Install or upgrade `taskfleet`

This skill was installed for `taskfleet {{CLI_VERSION}}`. On the first
invocation in a session, run `taskfleet version --output json`, parse
the JSON, and read `.data.version`. Compare it to `{{CLI_VERSION}}`:

- **Missing**: tell the user to install through a published distribution channel
  outside this repository workflow, then stop.

- **Older than `{{CLI_VERSION}}`**: tell the user the skill expects
  `{{CLI_VERSION}}` and suggest upgrading via the same channel they
  originally used (`brew upgrade jarimustonen/taskfleet/taskfleet` or
  re-run the shell installer).
  Stop and wait — the `run merge` flag surface may have changed.
- **Newer than `{{CLI_VERSION}}`**: tell the user the installed skill is
  stale and stop. Refreshing installed bundled instructions is published-tool
  maintenance outside repository work; never run `skill install` as part of
  this workflow.
- **Equal**: proceed normally.

## Examples

```
# Minimal: a spinoff worktree merges back to its recorded source with an
# auto-generated report.
run_id="$(taskfleet run show --current --output json | jq -er '.data.run_id')" || exit 1
taskfleet run merge "$run_id"

# Structured: a research worktree merges and delivers a full report.
taskfleet run merge "$run_id" --report-file /tmp/node-report-${run_id}.json

# Preflight a doubtful target or report file without merging.
taskfleet run merge "$run_id" --source <branch> --report-file /tmp/node-report-${run_id}.json --dry-run
```
