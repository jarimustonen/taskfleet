---
name: taskfleet-spawn-spinoff
description: Create one autonomous spinoff run directly with `taskfleet run create --kind spinoff`, the minimal spawn primitive for an agent that already has a self-contained brief and wants a focused task done in its own git worktree and merged back with no interactive review. Use when spawning a fire-and-forget sub-task from plain task text. For issue-driven spawns, profile routing, or a driver spawning children, use `worktree-spinoff` instead.
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# taskfleet-spawn-spinoff

A spinoff is one autonomous agent in its own git worktree, doing one
well-scoped task and merging itself back into the branch it was spawned from
with `taskfleet run merge`. Nobody reviews it while it runs. This skill is the
smallest way to launch one: you already have the task in words, and
`taskfleet run create --kind spinoff` turns it into a supervised run. The brief
you write is the only conversation the worker will ever have with you, and its
terminal report is the only thing you hear back, so most of the work here is
writing that brief well and telling the caller honestly how they will learn
the outcome.

The workflow built on top of this primitive, with issue lookup, the
capability-profile matrix, driver mode for `/fan-out`, and the issue-closing
contract, is `worktree-spinoff`. Reading run state afterwards is
`taskfleet-run-overview`, and the closing call in full detail is
`worktree-merge`. If the run / supervisor / node vocabulary is new to you,
read `taskfleet-overview` first. The binary's `--help` describes every flag;
this skill covers what the help text cannot.

## What is at stake

**The source branch.** `run merge` rebases the worker's branch onto the run's
recorded source branch and merges it there. With `--source-branch` the
worktree forks from that branch and it becomes the recorded merge target.
Without it the worktree forks from whatever is checked out where you run
`run create`, and that branch is recorded from git's own record of the branch
creation; a detached HEAD leaves the source unrecorded, and `run merge` then
falls back to whichever of `main` / `master` it finds. Spawn from a real
branch or name one. `run merge` is also the only success truth taskfleet
accepts: a worker that finishes and skips it has delivered nothing as far as
the caller can tell, and the run sits non-terminal with
`attention_required: true` until an operator salvages or cancels it.

**The worker's work.** Preserving work is the supervisor's job, not the
worker's: a blocked report, a failure, or a cancel leaves the branch and
worktree in place for a human to harvest. The real loss is a run that never
terminalizes at all. It stays alive, the worktree dangles, and anything
waiting on it waits forever. That is why the brief ends with exactly one
terminal path.

**A worktree made by hand.** A branch created with `workmux` or `git worktree`
directly has no supervisor and no run: nothing tears it down, nothing reports
it, and `run wait` / `run show` never see it. Spinoffs go through `taskfleet`
for that reason.

**The user's attention.** The worker cannot ask follow-up questions, and a
brief that misreads the task costs a worktree and a merge cycle. If the goal,
the context, or what "done" means is genuinely missing from the request, one
question before spawning is cheaper than that. Everything else, the title,
the placement, the profile, is a routine call: make it, say what you chose,
and continue.

## The brief

`run create` prepends generated run context to every worker prompt, whether
it arrived as `--task` or `--prompt-file`: the exact run id, the
`run show --current` ownership resolver, and the issue-filing boundary
(worker-filed issues go through `issuectl intake file` and are born unlaned).
That generated policy is authoritative over later brief text, so do not
restate or weaken it. Everything else the worker needs is yours to supply:

- **Goal.** One sentence on what to deliver.
- **Context.** Files, modules, constraints, with relative paths quoted. The
  worker starts from a fresh worktree of the source branch and knows nothing
  of your conversation.
- **Done criteria**, concrete and verifiable. Copy the repository's exact
  green-gate commands rather than paraphrasing them: a looser build or warning
  policy passes in the worktree and turns the source branch red. A missing
  prerequisite is something the worker reports, not something it installs
  globally.
- **Quality bar.** Unless the request mandates one, the
  worker chooses and explains review depth after the final diff. Primary
  evidence comes first: sources, deterministic tests, the repository gates.
  One focused review of the final diff is the default ceiling for bounded
  work, and a mechanical, well-tested change may need none. A panel, repeated
  reviews, or a broader independent review need a concrete reason recorded in
  the brief or the report: security or privacy, destructive or
  concurrency-sensitive behaviour, architectural breadth, hard rollback, weak
  tests. Adequate existing evidence is reused unless the risk surface changed.
- **The closing recipe and the disclosure contract.** Copy the closing
  commands and copy the disclosure contract below into the brief. The report
  shape is an interface the supervisor and the caller parse, so it has to be
  exact.

A brief longer than about 2 KB, or one with awkward shell quoting, goes in a
temp file passed as `--prompt-file`. The CLI copies it into the run directory,
so the temp file can go once `run create` returns.

### How the worker closes

The worker takes exactly one terminal path. Completed, mergeable work goes
through `run merge`, which rebases and merges the branch and submits the
terminal report stamped `via: "explicit-merge"` in one call; the supervisor
then removes the worktree, tmux window, and branch within a second or two,
and the worker's session ends as its window closes. Work blocked by a
required failure does not merge; it submits a direct `success: false` report
through `node report`, which preserves the branch and worktree for a human.
Taking neither leaves the run alive and the worktree dangling; taking both
confuses the record.

The run id is in the generated run context at the top of the worker's
prompt. If a generic recipe has to recover it, the durable ownership record
is the source, never the branch name, whose short fragment is display
metadata that can repeat:

```bash
run_id="$(taskfleet run show --current --output json | jq -er '.data.run_id')" || {
  echo "failed to resolve exact owning run id" >&2
  exit 1
}
```

Once the work is committed, the worker writes the report and merges:

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
taskfleet run merge "$run_id" --report-file /tmp/node-report-${run_id}.json
```

`success` is the one required field. `discussion_items[]` carries decisions
that genuinely needed a human (`{"topic": "<non-empty>", "severity":
"discuss|critical|info", "options": ["…"]}`), `spinoff_proposals[]` follow-up
work worth spawning (`{"proposed_title": "<non-empty>", "proposed_kind":
"spinoff|research|technical-decision|fan-out", "rationale": "<why>"}`), and
`wrap_up_recommendations[]` strings for the caller. An unknown key passes
validation and is never read. The per-run path matters because two concurrent
spinoffs writing a shared `/tmp/node-report.json` clobber each other, and the
quoted heredoc keeps a summary containing `$` or backticks literal. A worker
with nothing structured to say may run `taskfleet run merge "$run_id"` alone
and get a minimal `{success, summary}` report. `run merge` defaults to node
`n-0001`, the only node a spinoff has.

On `error.code: "merge_failed"` no report was submitted and the node is still
live: resolve the conflict, commit, and re-run the same `run merge` command.
The worker does not run `git worktree remove`, `git branch -d`, or
`tmux kill-window` itself, and does not resubmit because `run show` still
reads `pending` for a moment after the merge.

### Decision forks

Nobody is watching an autonomous worker, so a worker that stops at an
interactive prompt looks exactly like a hung run. A genuine human decision is
recorded as durable run state instead. The worker writes report-shaped
discussion items (`topic`, non-empty string `options`, `recommended_default`)
to a file shaped `{"discussion_items":[…]}` and opens the signal:

```bash
taskfleet event create "$run_id" --kind node.awaiting_input --node-id n-0001 \
  --from-file /tmp/awaiting-input-${run_id}.json \
  --idempotency-key "awaiting-input:${run_id}:<short-topic>"
request_seq="$(taskfleet run show "$run_id" --output json | jq -r '.data.awaiting_input_detail.event_seq')"
```

`run show` and `run list` show it at once; after three minutes
(`TASKFLEET_AWAITING_INPUT_GRACE_SECS` overrides) `run wait` settles and a
registered `--notify` hook fires with `TASKFLEET_STATUS=awaiting-input`. The
worker then waits at most five minutes, a bound that keeps an unattended run
from stalling, and proceeds on its recommended default, closing the signal
with the generation it opened:

```bash
printf '{"event_seq":%s}\n' "$request_seq" > /tmp/input-resolved-${run_id}.json
taskfleet event create "$run_id" --kind node.input_resolved --node-id n-0001 \
  --from-file /tmp/input-resolved-${run_id}.json
```

When no default is safe, it submits a terminal blocked report instead
(`success: false`, the same `discussion_items`) through `node report`. A fork
resolved by other evidence is closed the same way before continuing.

## Tool and sub-workflow failure disclosure

Before closing, the worker inventories every failed or detectably incomplete
tool, command, external service, review, panel, or delegated workflow, so the
caller learns of a gap from the report rather than from a later surprise.

A step **required** by the brief or the done criteria that remains failed or
incomplete blocks this attempt. Do not call `run merge`; write the report
payload above with top-level `success: false` to
`/tmp/node-report-${run_id}.json` and submit it with `taskfleet node report
"$run_id" n-0001 --from-file /tmp/node-report-${run_id}.json` (`n-0001` is
the sole node in this single-worker run). An **optional/advisory** failure
may continue only when the deliverable is independently complete and safe;
it is then disclosed in the full `success: true` report passed to
`taskfleet run merge "$run_id" --report-file /tmp/node-report-${run_id}.json`,
never hidden behind the minimal auto-report.

Requested completeness is a contract. A requested panel with a missing model
section, a truncation marker, malformed output, or a missing expected
artifact is incomplete, not representative consensus. Retry only within a
finite bound that an existing workflow policy grants; if none does, do not
retry. Record each attempt and its outcome, then take the required or
optional path at exhaustion.

Every distinct failure goes into one aggregate `discussion_items[]` entry
whose `topic` starts `Tool/sub-workflow failure —`, coalescing repeated
attempts of the same one: tool or workflow and its purpose, expected
completeness, observed exit, error, or incompleteness, attempts, affected
step, whether work continued and why that was safe, suggested bug surface,
and a stable artifact/log path when there is one. Actionable
retry/recover/accept/file steps go in the item's `options`. Keep the complete
entry, options included, under 2 KiB, with only a short redacted excerpt and
never secrets, credentials, personal data, environment dumps, or unbounded
logs. Top-level `summary` and `success` say whether the run is blocked or
completed; do not put them inside the discussion item. The existing prose
fields suffice, so do not add a schema or supervisor state.

## Creating the run

```
taskfleet run create \
  --kind spinoff \
  --title "<2–4 word title>" \
  --task "<self-contained brief>" \
  [--source-branch <branch>] \
  [--profile <name>] \
  [--headless] \
  [--notify <cmd>] \
  [--idempotency-key <key>] \
  [--dry-run]
```

What the flags do that `--help` does not tell you:

- `--title` names the run, the branch (`wt/<id>-<title>`), and the tmux
  window; keep it short, it is normalized into a slug.
- Exactly one of `--task` and `--prompt-file`; empty or whitespace-only text
  is rejected, so do not strip it silently.
- `run create` has to run inside tmux or be given `--headless` /
  `--tmux-session <name>`; otherwise, unless the user's `[tmux]` config names
  a default session, it refuses with `no_tmux_session`. `--headless` puts the
  window in a detached `headless` session (attach with
  `tmux attach -t headless`). This also matters on macOS, where about five or
  six foreground spawns exhaust pseudo-terminals and a batch then fails
  mid-way with `workmux_add_failed`; use headless placement for any parallel
  batch larger than three.
- `--profile <name>` picks a user-owned executable profile from the taskfleet
  home's `config.toml`; the names an installation defines are its own, so an
  installation may lack the one you would pick. Find out before creating
  state: run the complete intended command with `--dry-run`, which validates
  everything and materializes nothing. `unknown_profile` with an empty
  `expected` list means the installation predates profiles and the real call
  omits the flag. The capability matrix behind profile choice is in
  `taskfleet-overview`; omitting the flag uses the configured default.
- `--idempotency-key` makes the call safe to repeat after a transient error:
  the same key returns the original run with `idempotent_replay: true`
  instead of spawning twice.
- `--notify <cmd>` runs in the supervisor's environment, a long-lived
  detached process, not your login shell, so a desktop toast may lack
  `DISPLAY` or `DBUS_SESSION_BUS_ADDRESS`; a file or FIFO append that the
  harness watches is the robust sink. Delivery is at least once by design, so
  the command should tolerate running twice. Pass it only when such a sink
  exists.
- `--interactive` is not this skill's path: it makes the supervisor wait for
  an explicit `run merge` or `run cancel` so a human can review hands-on.
- Output defaults to `--output jsonl`, one compact envelope per line.

## What comes back

```json
{
  "schema_version": 1,
  "data": {
    "run_id": "01HZ...",
    "dir": "$HOME/.taskfleet/runs/01HZ...",
    "supervisor": 12345,
    "kind": "spinoff",
    "lifecycle": "autonomous",
    "node_id": "n-0001",
    "tmux_window": "<window-name>",
    "worktree_path": "<repo>/worktrees/<title>",
    "branch": "wt/<id>-<title>",
    "selection": { "profile": "...", "selected": { "harness": "pi", "command": ["..."] } }
  }
}
```

`data.run_id` is the handle for everything that follows. `data.supervisor` is
the supervisor's pid when one was spawned; a string instead
(`not-spawned-dry-run`, `recorded-on-prior-run`,
`delegated-to-parent-supervisor`) explains why not, and outside those
expected cases means nothing is driving the worker, which the user needs to
hear. `lifecycle` is a fixed category (`autonomous` here) and never changes;
progress lives in `status` on `run show` and `run wait`, and an agent that
once polled `lifecycle` for a terminal value hung forever.

Tell the user the run id, the source branch, the tmux window, and the session
it landed in; that the spinoff merges and reports itself, so nothing is needed
from them at the end; and how to follow it (`taskfleet run show <run-id>`). Be
precise about how completion reaches them: the run is out of band and nothing
re-invokes this session by itself, so promise to report back only if you
wired `--notify` or a background `taskfleet run wait "$run_id"` through a
harness facility that wakes you when it exits. Otherwise say plainly that they
check `run show` or ask you to wait on it.

## Following the run

`taskfleet run wait "$run_id"` blocks until the run settles and already
contains the backoff, the terminal set (`done | failed | cancelled`), and the
stuck shapes (stalled, attention-required, awaiting input), so it replaces
any hand-rolled poll loop. Its envelope is multi-run: read `data.runs[]`, not
`data.status`. Settled is not landed: `run wait` and `run show` carry a
`landed` boolean with a `landed_method`, computed by patch id against the
current source tip, so it survives a rebase on your side that a
`git merge-base --is-ancestor` check would misreport as "not landed";
`unverified` means "could not confirm", not "missing". The terminal report is
`data.report` on `run show`, and everything persists after teardown, so a
late `run show` still answers. `taskfleet-run-overview` covers the rest of
the read surface and what each stuck shape calls for.

## Errors

Failures print `{"schema_version": 1, "error": {"code": "<code>", "message":
"..."}}` on stderr with a non-zero exit. Branch on `error.code`; the message is
prose. Codes you are likely to meet from `run create`:

- `invalid_arguments` — missing or empty `--title` / `--task`, both or neither
  of `--task` and `--prompt-file`.
- `prompt_file_not_found` / `prompt_file_not_readable`.
- `no_tmux_session` — not inside tmux and no `--headless` /
  `--tmux-session`.
- `base_ref_not_found` — `--source-branch` does not resolve locally. Fetch or
  correct the name; do not create it.
- `workmux_add_failed` — the materializer refused: uncommitted changes on the
  source branch, a conflicting worktree path, or macOS PTY exhaustion
  mid-batch. Materialization is rolled back.
- `unknown_profile` / `profile_required` — see the `--dry-run` preflight
  above.
- `supervisor_spawn_failed` — the run directory exists but nothing drives the
  worker. Inspect `<dir>/supervisor.stderr.log` and consider
  `taskfleet run reattach <run-id>`.

## Install or upgrade `taskfleet`

This skill was rendered for `taskfleet {{CLI_VERSION}}`, and CI validates
every command in it against exactly that binary, so a matching version means
the invocations here parse. Check once per session: read `.data.version` from
`taskfleet version --output json` and compare it to `{{CLI_VERSION}}`. When
they differ, the `run create --kind spinoff` flag surface or the envelope
fields may have moved, and the installed binary's `--help` outranks this
document; tell the user about the mismatch and stop rather than guess. The
remedy is the user's tool maintenance, not something to do silently inside
another task: an older binary is upgraded through the channel it was installed
from (`brew upgrade jarimustonen/taskfleet/taskfleet` or the shell installer),
a newer binary means the installed skills are stale and need refreshing, and a
missing binary is installed through a published distribution channel. Inside
the taskfleet repository itself, never touch the installed binary or skills;
maintained `HEAD` and the installed release are allowed to differ.
