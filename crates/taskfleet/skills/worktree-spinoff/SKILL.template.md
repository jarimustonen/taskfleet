---
name: worktree-spinoff
description: Spawn an autonomous spinoff worktree agent via `taskfleet run create --kind spinoff` — one fire-and-forget agent that takes a focused task, executes it in its own git worktree, and merges itself back to the source branch. Use when the user says `/worktree-spinoff <task>`, when a parallel sub-task can be handled without interactive review, or when a driver (`/fan-out`) needs to spawn one autonomous unit. NOT for hands-on interactive review (add `--interactive` to `run create` so the supervisor waits for an explicit `run merge`/`run cancel`), N identical units (`/fan-out`), or dependency-ordered features (stint waves).
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# worktree-spinoff

A spinoff is one autonomous agent in its own git worktree, doing one well-scoped
task and merging itself back to the source branch with `taskfleet run merge`.
Nobody reviews it interactively. The brief you write is the only conversation it
will ever have with you, and the terminal report it submits is the only thing you
will hear back. Your job is therefore mostly writing: decide that the task is
spinoff-shaped, write a brief the worker can finish alone, create the run through
`taskfleet`, and tell the caller honestly how they will learn the outcome.

`taskfleet` owns the run state under `~/.taskfleet/runs/<run-id>/`. A worktree
made by hand, or by calling `workmux` / `create.sh` directly, has no supervisor:
nothing tears it down, nothing reports it, and `run wait` / `run show` never see
it. If the run / supervisor / node vocabulary is new to you, read
`taskfleet-overview` first.

## Is this a spinoff?

It is when the user says `/worktree-spinoff <task>`, asks for a "background" or
"fire-and-forget" worktree for a focused task, or a driver such as `/fan-out`
needs one autonomous unit with `--parent-run-id` + `--parent-node-id`. A bug fix
is a spinoff driven by its existing issue slug; there is no separate bugfix kind.

It is not a spinoff when the user wants to review the work hands-on: create the
run with `--interactive` instead, and the supervisor then never auto-terminalizes
but waits for an explicit `run merge` or `run cancel`. Five or more similar
independent units are `/fan-out`. Several features that depend on each other are
scheduled as bounded waves through `/stint-start` and the issuectl DAG; taskfleet
has no integration orchestrator. Research and architectural decisions have their
own kinds (`/worktree-research`, `/worktree-technical-decision`).

## What is at stake

**The source branch.** `run merge` rebases the worker's branch onto the run's
recorded source branch and merges it there. Whatever branch you spawn from is the
branch that gets modified; the default is the branch checked out when you run
`run create`. `run merge` is also the only success truth: a worker that finishes
and skips it has delivered nothing as far as the caller can tell.

**The worker's work.** Preserving work is the supervisor's job, not the worker's:
a blocked report, a failure, or a cancel leaves the branch and worktree in place
so a human can harvest them. The real loss is a run that never terminalizes at
all. It stays alive, the worktree dangles, and a driver waiting on it waits
forever. That is why the brief has to end with exactly one terminal path.

**The user's installed taskfleet.** Inside the taskfleet repository a worker may
`cargo build --release` and run `./target/release/taskfleet …` from its worktree,
but it never installs, replaces, or removes the user's installed binary or
bundled skills, by `cargo install`, Homebrew, a manual copy, or any
`skill install` variant. The installed release is the user's production tool; it
legitimately differs from source `HEAD`, and upgrading it is a separate
distribution-channel step, never part of a worker's task.

**The user's attention.** The worker cannot ask follow-up questions, so a brief
that misreads the task costs a worktree and a merge cycle. If the goal, the
context, or what "done" means is genuinely missing from the request, one question
before spawning is cheaper than that. Everything else, the title, the profile,
the placement, is a routine call: make it, say what you chose, and continue.

## Before you spawn

You need a git repository (the spinoff needs a source branch) and a compatible
binary: run `taskfleet version --output json` once per session and compare
`.data.version` with `{{CLI_VERSION}}` as described under "Install or upgrade"
below. Capture the current branch with `git rev-parse --abbrev-ref HEAD`; it is
the default source and merge target, and the caller should hear which branch that
was.

`run create` must run inside tmux or be given `--headless` / `--tmux-session`;
otherwise it refuses with `no_tmux_session`. `--headless` puts the worker's window
in a detached `headless` session (attach with `tmux attach -t headless`), which
also matters on macOS: the system runs out of pseudo-terminals around five or six
foreground spawns, and a batch then fails mid-way with `workmux_add_failed`. Use
headless placement for any parallel batch larger than three. A user `[tmux]`
config section may name a default session for autonomous workers; explicit
placement flags still win.

Drivers (`/stint-start`, `/fan-out`, `/worktree-bug-analysis`) may prefix the
request with `--headless`, `--tmux-session <name>`, or `--profile <name>`. Strip
them from the task text, forward placement flags verbatim, and treat the profile
as the caller's explicit selection. A leading `--review` token is not a
`run create` flag (it would be rejected); it expresses the quality bar, so fold it
into the brief.

The task comes from one of two places. An issue reference (`#NN`, `issuectl:slug`,
or a bare slug that `issuectl --json show` recognises) means the issue's title and
body are the brief; prefer the bare slug or `issuectl:<slug>` for hyphenated
slugs, which are not guaranteed to parse behind `#`. Otherwise the user's prompt
is the brief, and you distil a 2–4 word `--title` from it. When both parent flags
are set you are in driver mode: skip issue detection and the issue-closing
contract below, because a driver fanning out N children from one issue would
otherwise have that issue updated and closed N times.

## The brief

`run create` prepends generated run context to every worker prompt, including a
custom `--prompt-file`: the exact run id, the `run show --current` ownership
resolver, and the issue-filing boundary (worker-filed issues go through
`issuectl intake file`, are born unlaned, and review findings carry
machine-visible `ai-review` provenance). That generated policy is authoritative
over later brief text, so do not restate it, weaken it, or tell the worker to
execute an `/assess-findings`-staged `issuectl create` command verbatim.
Everything else the worker needs is yours to supply:

- **Goal.** One sentence on what to deliver.
- **Context.** Files, modules, constraints, relative paths.
- **Done criteria**, concrete and verifiable. Copy the repository's exact
  green-gate commands from its `AGENTS.md` rather than paraphrasing: a debug build
  or a looser warning policy passes in the worktree and turns `main` red. A
  missing prerequisite such as `cargo-nextest` is something the worker reports,
  not something it installs globally. A developer machine is not a bare CI
  runner, so tool-sensitive tests are exercised with a stripped `PATH`, not
  trusted because the fully equipped host passed them.
- **Repository-local build safety**, the installed-taskfleet rule above, when the
  target repository is taskfleet.
- **Quality bar.** Primary evidence comes first: sources, source-grounded
  scenarios, deterministic tests, the repository gates, and where relevant the
  behaviour of a local bundle. For bounded implementation from an accepted
  design, one focused final-diff review covering every relevant concern is the
  default ceiling; mechanical, well-tested refactors and documentation usually
  need no model review at all. Panels, repeated reviews, or a broader
  independent review are justified by a concrete reason recorded in the brief or
  the report: security or privacy, destructive or concurrency-sensitive
  behaviour, architectural breadth, hard rollback, weak tests, or an unresolved
  trade-off. Adequate existing evidence is reused unless the risk surface
  changed, and model review never substitutes for deterministic validation.
- **The failure-disclosure contract and the closing recipe** from the two
  sections below, copied in. The report shape is an interface the supervisor and
  the caller parse, so it has to be exact.

A brief longer than about 2 KB, or one with awkward shell quoting, goes in a
temp file (`mktemp -t spinoff-prompt-XXXXXX.md`) passed as `--prompt-file`;
remove the file once `run create` returns, since the CLI copies it into the run
directory.

### How the worker closes

The worker takes exactly one terminal path. Completed, mergeable work goes
through `run merge`, which rebases and merges the branch and submits the terminal
report stamped `via: "explicit-merge"` in the same call. Work blocked by a
required failure does not merge; it submits a direct `success: false` report.
Taking neither leaves the run alive and the worktree dangling; taking both
confuses the record.

For the completed path, once the work is committed:

1. Resolve the owning run id from the durable ownership record, never from the
   branch name, whose short fragment is display metadata that can repeat:

   ```bash
   run_id="$(taskfleet run show --current --output json | jq -er '.data.run_id')" || {
     echo "failed to resolve exact owning run id" >&2
     exit 1
   }
   ```

   This fails closed on missing, duplicate, stale, or malformed evidence. If it
   fails, report the error rather than guess.

2. Write the report. These field names are what the supervisor and the caller
   read; an unknown key such as `discuss` or `wrap_up` passes validation and is
   silently dropped, and a malformed element drops that whole element:

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

   `success` is the one required field. `discussion_items[]` carries decisions
   that genuinely needed a human (`{"topic": "<non-empty>", "severity":
   "discuss|critical|info", "options": ["…"]}`); `spinoff_proposals[]` carries
   follow-up work worth spawning (`{"proposed_title": "<non-empty>",
   "proposed_kind": "spinoff|research|technical-decision|fan-out", "rationale":
   "<why>"}`, and only those four kinds exist); `wrap_up_recommendations[]` is
   an array of strings for the caller. The per-run path matters: a shared
   `/tmp/node-report.json` lets two concurrent spinoffs clobber each other.

3. Merge and report in one call:

   ```bash
   taskfleet run merge "$run_id" --report-file /tmp/node-report-${run_id}.json
   ```

   The file is validated before the merge runs. `run merge` defaults to node
   `n-0001`, the only node a spinoff has. A worker with nothing structured to say
   may skip the file and run `taskfleet run merge "$run_id"`, which submits a
   minimal `{success, summary}` report; a worker disclosing an optional failure
   uses the full report.

   The supervisor then tears down the worktree, tmux window, and branch within a
   second or two and the worker's session ends as the window closes. The worker
   does not run `tmux kill-window`, `git worktree remove`, or `git branch -d`
   itself, and does not resubmit because `run show` still reads `pending` for a
   moment. On `error.code: "merge_failed"` no report was submitted and the node
   stays live: resolve the conflict (or `/complex-rebase` for a deeply diverged
   branch) and re-run the same `run merge` command.

A worker that hits a genuine human decision does not stop at an interactive
prompt; nobody is watching, and a blocked prompt looks exactly like a hung run.
It records the fork as durable run state instead. Write report-shaped discussion
items (`topic`, non-empty string `options`, `recommended_default`) to a file
shaped `{"discussion_items":[…]}` and open the signal:

```bash
taskfleet event create "$run_id" --kind node.awaiting_input --node-id n-0001 \
  --from-file /tmp/awaiting-input-${run_id}.json \
  --idempotency-key "awaiting-input:${run_id}:<short-topic>"
request_seq="$(taskfleet run show "$run_id" --output json | jq -r '.data.awaiting_input_detail.event_seq')"
```

`run show` and `run list` show it immediately; after three minutes
(`TASKFLEET_AWAITING_INPUT_GRACE_SECS` overrides) `run wait` settles and a
registered `--notify` hook fires with `TASKFLEET_STATUS=awaiting-input`. The
worker then either waits at most five minutes and proceeds on its recommended
default, closing the signal with the generation it opened:

```bash
printf '{"event_seq":%s}\n' "$request_seq" > /tmp/input-resolved-${run_id}.json
taskfleet event create "$run_id" --kind node.input_resolved --node-id n-0001 \
  --from-file /tmp/input-resolved-${run_id}.json
```

or submits a terminal blocked report (`success: false`, the same
`discussion_items`) through `node report`, which preserves the branch and
worktree for the human. A fork resolved by other evidence is closed the same way
before continuing.

### Tool and sub-workflow failure disclosure

Before closing, the worker inventories every failed or detectably incomplete
tool, command, external service, review, panel, or delegated workflow.

A step the brief or the done criteria required, still failed or incomplete,
blocks this attempt: no `run merge`, but a `success: false` report written to
`/tmp/node-report-${run_id}.json` and submitted with `taskfleet node report
"$run_id" n-0001 --from-file /tmp/node-report-${run_id}.json`. An optional or
advisory failure may continue only when the deliverable is independently
complete and safe, and is then disclosed in the full `success: true` report
passed to `run merge`, never hidden behind the minimal auto-report.

Requested completeness is a contract: a panel with a missing model section, a
truncation marker, malformed output, or a missing artifact is incomplete, not
consensus. The worker retries only within a bound an existing workflow policy
grants, records each attempt, and takes the required or optional path at
exhaustion.

Every distinct failure goes into one aggregate `discussion_items[]` entry whose
`topic` starts `Tool/sub-workflow failure —`, coalescing repeated attempts of the
same one: tool and purpose, expected completeness, observed error, attempts,
affected step, whether work continued and why that was safe, suggested bug
surface, and a stable artifact or log path when there is one. Actionable
retry/recover/accept/file steps go in the item's `options`. The whole entry stays
under 2 KiB with only a short redacted excerpt, never secrets, credentials,
personal data, environment dumps, or unbounded logs. Top-level `summary` and
`success` say whether the run is blocked or completed; they do not move into the
item, and no new schema or terminal state is invented.

## Choosing a profile and creating the run

A profile is a capability tier, not a model name: `implementation` for bounded
work from an accepted design (the ordinary feature or bug fix), `lightweight` for
a mechanical, strongly tested refactor or documentation, `capable` for broad or
high-risk design, uncertain or mixed scope, security or privacy, destructive or
concurrency-sensitive behaviour, hard rollback, or weak tests. The full matrix and
the reasoning live in `taskfleet-overview`. An explicit caller profile wins and
may escalate; do not silently downgrade it. Concrete model names and commands stay
out of the issue brief, because the user's `config.toml` owns which harness and
argv a name means.

Profile names are user-owned, so an installation may not define the one you
recommend. Find that out before creating state: run the complete intended command
with `--dry-run`. If a workflow-recommended profile returns `unknown_profile`, try
`capable`; if that is also unknown and the error's `expected` list is empty, the
installation predates profiles and the real call omits `--profile`. If profiles
exist but neither is defined, stop and surface the structured error rather than
pick an arbitrary one. An unknown profile the caller chose explicitly is theirs
to fix, not yours to substitute. Any other dry-run error also stops. The real
call uses exactly the selector whose dry-run passed. Child creates refuse
`--dry-run` (`dry_run_unsupported`) because parent publication cannot be
previewed truthfully, so in driver mode preflight with both parent flags omitted,
add them back for the real call, and give that call an `--idempotency-key` so a
retry is safe.

```
# First issue this command with --dry-run; remove only --dry-run after it passes.
# skill-example-ci: skip
taskfleet run create \
  --kind spinoff \
  --title "<2–4 word title>" \
  --task "<self-contained brief>" \
  [--profile <resolved-profile>] \
  [--source-branch <branch>] \
  [--headless | --tmux-session <name>] \
  [--notify <cmd>] \
  [--parent-run-id <id> --parent-node-id <id>] \
  [--idempotency-key <key>] \
  [--dry-run]
```

What the flags do that `--help` does not tell you:

- Exactly one of `--task` and `--prompt-file`; empty or whitespace-only text is
  rejected, so do not strip it silently. `--parent-run-id` and
  `--parent-node-id` come together or not at all.
- `--idempotency-key` makes the call safe to repeat after a transient error: the
  same key returns the original run, with `idempotent_replay: true` in the
  envelope, instead of spawning twice.
- `--notify <cmd>` is the push signal: the supervisor runs it via `sh -c` on the
  terminal transition, before teardown, with `TASKFLEET_RUN_ID`,
  `TASKFLEET_STATUS`, `TASKFLEET_SUMMARY`, `TASKFLEET_RUN_KIND`, and
  `TASKFLEET_RUN_TITLE` in the environment, and again for an unresolved
  `node.awaiting_input` after the grace window with
  `TASKFLEET_STATUS=awaiting-input`, `TASKFLEET_AWAITING_INPUT=1`, and the
  discussion array in `TASKFLEET_AWAITING_INPUT_JSON`. Delivery is at least
  once: a supervisor crash between firing and recording its marker re-fires on
  restart, because a missed completion is worse than a duplicate, so the command
  should tolerate running twice (an appended line, a toast; nothing that
  double-counts). It runs in the supervisor's environment, a long-lived detached
  process, not your login shell, so a desktop toast may lack `DISPLAY` or
  `DBUS_SESSION_BUS_ADDRESS`; a file or FIFO append the harness watches is the
  robust sink. Pass it only when such a sink exists.
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
    "tmux_window": "<workmux-reported-window-name>",
    "worktree_path": "$HOME/repos/<repo>/worktrees/<title>",
    "branch": "wt/<id>-<title>"
  }
}
```

`data.run_id` is the handle for everything that follows. `data.supervisor` is
the supervisor's pid when one was spawned; a string instead
(`not-spawned-dry-run`, `recorded-on-prior-run`, `delegated-to-parent-supervisor`)
explains why not, and outside those expected cases means nothing is driving the
worker, which the user needs to hear.

Tell the user the run id, the source branch, and the tmux window name (they can
select it in the reported session; do not guess a session name); that the spinoff
merges and reports itself, so no `/worktree-merge` is needed from them; and how
to follow it (`taskfleet run show <run-id>`). Be precise about how completion
reaches them: a spinoff runs out of band and nothing re-invokes this session by
itself, so promise "I'll tell you when it's done" only if you wired one of the
mechanisms in the next section. Otherwise say plainly that they check
`run show <run-id>` or ask you to wait on it. A driver gets the structured
payload (run id, node id, branch, tmux window) instead of a human summary; it
needs the ids to poll.

## Following the run and reading the outcome

To wait, use `taskfleet run wait "$run_id"` rather than a hand-rolled poll loop:
the backoff, the terminal set (`done | failed | cancelled`), and the rule to
branch on `manifest.status` rather than `lifecycle` (which is a fixed category
and never matches a terminal value) all live inside it. It exits `0` when the
run settles, `2` on `--timeout`, and `3` under `--fail-on-error` when the settled
run failed or was cancelled. Several ids block until all settle, `--any` until
the first. Its envelope is multi-run, so read `data.runs[]` (for example
`jq '.data.runs[] | {run_id, status, summary}'`), not `data.status`.

Two mechanisms deliver completion to this session without polling: `--notify`
at spawn time, or a background `taskfleet run wait "$run_id"` started at spawn
time through a harness facility that re-invokes you when a background task exits.
A run you never waited on has no watcher. Everything persists after teardown
(the run directory, terminal `manifest.status`, the node's report), so a late
`run show` still answers.

Settled is not landed. `run wait` and `run show` both carry a `landed` boolean
with a `landed_method` (`git-verified` | `report-marker` | `unverified`), and
that flag is the landing signal. Do not verify with
`git merge-base --is-ancestor <worker-branch> <target>`: if you rebased local
`main` meanwhile, routine on a busy repo, the merge was replayed under a new hash
while the branch ref stayed put and the ancestry check reports a false "not
landed". The CLI's check is patch-id based against the current target tip and
survives that; when git cannot run because the branch is gone it falls back to
the durable `run merge` marker. `landed: false` with method `unverified` means
"could not confirm", so verify by content on the actual target before concluding
the work is missing. `run show` also lists `preserved_work` (retained worktrees or
branches, including on a `done` run) and, on a failed run, `recoverable_work`
from the report.

The terminal report persists on the node as `last_report`. For a single-worker
spinoff `run show` exposes it as `data.report`; `node show` exposes both names:

```bash
# skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
taskfleet run show "$run_id" --output json | jq '.data.report'
# Node-level projection-compatible probe:
# skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
taskfleet node show "$run_id" n-0001 --output json |
  jq '.data.report // .data.last_report'
```

`run wait` folds in only the `summary`; the full `discussion_items`,
`spinoff_proposals`, and `wrap_up_recommendations` come from `run show` or
`node show`.

## Issue-driven runs: closing the issue

For an issue-driven run outside driver mode the worker owns the issue lifecycle;
you resolve the closing contract while building the brief and write the concrete
slug, status, and agent identity into it rather than metavariables. A bug closes
as `fixed`, a feature, task, improvement, or chore as `done`; if the repository
customises types or delivery statuses and no single valid status follows from its
schema, the brief says to stop before implementation rather than guess.

The worker marks work begun with `issuectl update <slug> --status in-progress
--json`, commits only that metadata path, and starts implementation from a clean
tree. After the final validated implementation commit and before `run merge`, it
runs `issuectl close <slug> --status <fixed-or-done> --stamp --as <agent> --json`.
The stamp rewrites the implementation commit's message with a
`Fixes-Issue: @<slug>` trailer, which is what the trailer-driven changelog reads,
without touching its tree; `.data.stamp.status` has to be `stamped` or
`already_present`, since `skipped` (detached HEAD, merge commit, signed, or mid
rebase) or a missing stamp means the landing commit would be invisible to the
changelog, and that blocks the merge. The closure metadata path issuectl returns
is committed separately, and the tree is clean before `run merge`. A freeform run
adds no trailer and closes no issue; in driver mode the driver owns the issue and
the spawning skill never races the worker on it.

## Errors

Failures print a JSON envelope to stderr with a non-zero exit:

```json
{"schema_version": 1, "error": {"code": "<code>", "message": "..."}}
```

Branch on `error.code`; the message is prose. Codes you are likely to meet:

- `invalid_arguments` — missing or empty `--title` / `--task`, both `--task` and
  `--prompt-file`, or mismatched parent flags.
- `prompt_file_not_found` / `prompt_file_not_readable`.
- `no_tmux_session` — not inside tmux and no `--headless` / `--tmux-session`.
- `base_ref_not_found` — `--source-branch` does not resolve locally. Fetch or
  correct the name; do not create it.
- `workmux_add_failed` — workmux refused (uncommitted changes on the source
  branch, a conflicting worktree path, or macOS PTY exhaustion mid-batch, which
  headless placement avoids). Materialization is rolled back.
- `unknown_profile` / `profile_required` — see the preflight above.
- `dry_run_unsupported` — `--dry-run` on a child create.
- `parent_not_found` — the driver's run id does not name a run.
- `supervisor_spawn_failed` — the run directory exists but no one drives the
  worker. Inspect `<dir>/supervisor.stderr.log` and consider
  `taskfleet run reattach <run-id>`.

`--dry-run` validates inputs and emits a `dry_run: true` envelope without
materializing anything.

## Install or upgrade `taskfleet`

This skill was installed for `taskfleet {{CLI_VERSION}}`. On the
first invocation in a session, run
`taskfleet version --output json`, parse the JSON, and read
`.data.version`. Compare it to `{{CLI_VERSION}}`:

- **Missing**: tell the user to install through a published distribution channel
  outside this repository workflow, then stop.

- **Older than `{{CLI_VERSION}}`**: tell the user the skill expects
  `{{CLI_VERSION}}` and suggest upgrading via the same channel they
  originally used (`brew upgrade jarimustonen/taskfleet/taskfleet` or
  re-run the shell installer). Stop and wait — the `run create --kind spinoff` flag
  surface may have changed.
- **Newer than `{{CLI_VERSION}}`**: tell the user the installed skill is
  stale and stop. Refreshing installed bundled instructions is published-tool
  maintenance outside repository work; never run `skill install` as part of
  this workflow.
- **Equal**: proceed normally.

## Examples

```
# Freeform spinoff
/worktree-spinoff Process receipts batch 2026-05 with vision OCR

# Issue-driven (skill reads the issue, builds the task brief from it)
/worktree-spinoff extremely-quiet-otter

# Driver mode — /fan-out passes these
taskfleet run create --kind spinoff \
  --title "u-003-receipts" \
  --task "..." \
  --source-branch fan-out/2026-05 \
  --parent-run-id 01HZ... \
  --parent-node-id n-0001
```
