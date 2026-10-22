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
Nobody reviews it interactively. Write the brief so it can finish without a live
reply; attention signals can surface decisions, but not every harness can receive
an answer. Your job is therefore mostly writing: decide that the task is
spinoff-shaped, write a brief the worker can finish alone, create the run through
`taskfleet`, and tell the caller honestly how they will learn the outcome.

`taskfleet` owns the run state under `~/.taskfleet/runs/<run-id>/`. A worktree
made by hand, or by calling `workmux` / `create.sh` directly, has no supervisor:
nothing tears it down, nothing reports it, and `run wait` / `run show` never see
it. If the run / supervisor / node vocabulary is new to you, read
`taskfleet-overview` first.

## Repository tracker (optional)

Use issuectl only when **both** repository instructions or an issuectl schema
establish that this repository uses it **and** its CLI is available. A directory named `issues/` alone may belong to another tracker;
check the repository policy before treating it as issuectl storage. This probe
checks the conventional schema-backed case from the repository root:

```sh
# issuectl-capability-probe
if [ ! -f issues/.schema.yaml ]; then
  printf 'tracker-not-used\n'
elif command -v issuectl >/dev/null 2>&1; then
  printf 'issuectl-ready\n'
else
  printf 'issuectl-cli-missing\n'
fi
```

For a documented issuectl tree without that schema path, verify configuration
by reading repository instructions or tracker files, not by running issuectl
against an unidentified `issues/` tree. Check CLI availability separately.
Do not install or initialize a tracker. Without both capabilities, use the free-text task and committed
repository artifacts; skip issue lookup, mutation, closure, trailers, and DAG
commands. If an explicit issue-driven request requires unavailable tracker data
or a required mutation, report that prerequisite and stop before spawning;
do not silently reinterpret a slug or hand-edit tracker metadata. A CLI command
failure in an enabled tracker is an error, not permission to switch modes.
`issuectl-cli-missing` is a disclosed missing capability, not proof that the
repository is tracker-free. If repository policy requires tracker scheduling or
writes for this task/stint, stop; only policy-permitted free-text work may use
the no-issuectl path. Record follow-up findings in the native report's
`spinoff_proposals` or `discussion_items` instead of creating tracker files.
State this capability result as a fact in every worker brief; it does not relax
the generated policy on how an issue, if filed, must be created.

## Parent wait and delivery

Use a plain blocking process:

```bash
taskfleet run wait <run-id> --output json
```

Start it through the parent harness's existing background-command/wake facility
when one exists, requesting a wake on exit, then continue independent work or end
the turn. No notification hook, desktop, or attached terminal is required. Do not
shell-background it, invent a callback transport, or poll `run show` for wakeups.
Without a wake facility, await the same blocking command with `--timeout`
shorter than the harness's foreground-command limit. On `timed-out`, establish
another bounded wait if continued waiting is appropriate; do not treat the
timeout as completion. If the harness kills the process without an envelope,
inspect `run show` once and re-establish the wait. Never promise an automatic
later message or interpret a killed observer as a failed worker. After a parent restart, inspect the
known run and establish a fresh wait if needed, rather than create a duplicate.

For unattended waits, add `--check-in 30m` (or a shorter interval within the
host's command budget). `outcome: check-in`, exit `4`, means the condition is
still unmet: inspect `data.runs[].progress_evidence` and establish a new wait.
Absent telemetry or prompt evidence is unknown, not health. This remains a plain
blocking command and requires no desktop or host-specific wake extension.

Read `data.outcome` and `data.runs[]`: timeout is not completion, and attention,
stall, or awaiting-input can settle a wait without a terminal success. Resolve
that condition; `--terminal` waits for the eventual terminal result but answers
no decision. Then read `taskfleet run show <run-id> --output json` for the full
`data.report`, `landed`, `landed_method`, and `preserved_work`. A settled run,
clean worker exit, visible UI entry, or disappearing window is not merge proof.
Taskfleet's CLI and supervisor retain merge, reporting, and teardown authority.

## Runtime capabilities

Read sibling instructions through this harness's skill loader or file-reading
facility; no particular invocation tool is required. Bundled text is also
available with `taskfleet skill print <name>`. External review, Git-helper,
and session-saving skills mentioned here are optional unless the user or
repository mandates them. If absent, use ordinary Git, evidence-based self-review,
and repository-local handoff notes instead. A missing **required** review or
capability blocks completion and is disclosed, never claimed as run. Do not
install tools, change another user's configuration, or publish artifacts merely
to satisfy a skill reference.

## Is this a spinoff?

It is when the user says `/worktree-spinoff <task>`, asks for a "background" or
"fire-and-forget" worktree for a focused task, or a driver such as `/fan-out`
needs one autonomous unit with `--parent-run-id` + `--parent-node-id`. A bug fix
is a spinoff driven by its existing issue slug; there is no separate bugfix kind.

It is not a spinoff when the user wants to review the work hands-on: create the
run with `--interactive` instead, and the supervisor then never auto-terminalizes
but waits for an explicit `run merge` or `run cancel`. Five or more similar
independent units are `/fan-out`. Several features that depend on each other are
scheduled as bounded waves through `/stint-start` (using the issuectl DAG only
when the repository tracker is enabled); taskfleet
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

**The user's attention.** A brief that misreads the task costs a worktree and
a merge cycle; live follow-up replies are not guaranteed. If the goal, the
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

Taskfleet uses native tmux internally, not as a required human interface.
Use `--headless` for an unattended parent without an attached terminal; it still
uses native tmux. An explicit `--tmux-session <name>` or configured default may
also supply placement. Report the run id and source branch, with `run show` for
inspection; window/session details are optional diagnostics, not a requirement
to attach. Missing native runtime prerequisites are setup failures to report,
not permission to start unrelated services or implement another supervisor.

Drivers (`/stint-start`, `/fan-out`, `/worktree-bug-analysis`) may prefix the
request with `--headless`, `--tmux-session <name>`, or `--profile <name>`. Strip
them from the task text, forward placement flags verbatim, and treat the profile
as the caller's explicit selection. A leading `--review` token is not a
`run create` flag (it would be rejected); it expresses the quality bar, so fold it
into the brief.

The task comes from one of two places. When the optional repository tracker is enabled, an issue reference (`#NN`, `issuectl:slug`,
or a bare slug that `issuectl show --json` recognises) means the issue's title and
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
machine-visible `ai-review` provenance). This AI-review provenance includes
the stable review source field even when the repository rejects `ai-review` as
a provenance value. That generated policy is authoritative over later brief text, so do not restate it, weaken it, or tell the worker to
execute an `/assess-findings`-staged `issuectl create` command verbatim.
Everything else the worker needs is yours to supply:

- **Goal.** One sentence on what to deliver.
- **Context.** Files, modules, constraints, relative paths.
- **Decision authority.** Pre-authorize safe defaults, especially for non-Pi or
  headless work. Name decisions that require preserved-work escalation instead
  of guessing. Claude remains supported for autonomous work; it has no live
  `run message` reply channel.
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
  design, allow at most one focused final-diff review by default, covering every
  relevant concern; mechanical, well-tested refactors and documentation usually
  need no model review at all. Panels, repeated reviews, or a broader
  independent review are justified by a concrete reason recorded in the brief or
  the report: security or privacy, destructive or concurrency-sensitive
  behaviour, architectural breadth, hard rollback, weak tests, or an unresolved
  trade-off. Adequate existing evidence is reused unless the risk surface
  changed, and model review never substitutes for deterministic validation.
- **The failure-disclosure contract and the closing recipe** from the two
  sections below: copy the disclosure contract below into the spawning brief
  together with the closing recipe. The supervisor and caller parse the report
  shape as an exact interface.

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
   never read, and `run merge` drops a malformed element with a warning while
   `node report` rejects the whole file:

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

A worker that hits a genuine human decision uses a safe default only if already
authorized by the brief or repository policy. Otherwise it preserves work and
submits a terminal `success: false` report with the decision in `discussion_items`
through `taskfleet node report`, without merging. The caller can decide and
respawn with that answer and the preserved branch in the new brief. Do not wait
indefinitely for a reply the harness cannot receive.

`run message` delivers only to live Taskfleet-launched Pi workers with the
acknowledged message adapter. Claude, caller-owned workers, and older Pi runs
without that adapter are unsupported, including in headless placement. An
accepted message is not proof it was read or acted on.

When useful, record the fork as durable attention state before taking the
authorized-default or terminal-report path. Write report-shaped discussion
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
worker may proceed only on a safe default already authorized by the brief or
repository policy, never merely because time elapsed, closing the signal with the generation it opened:

```bash
printf '{"event_seq":%s}\n' "$request_seq" > /tmp/input-resolved-${run_id}.json
taskfleet event create "$run_id" --kind node.input_resolved --node-id n-0001 \
  --from-file /tmp/input-resolved-${run_id}.json
```

A fork resolved by other evidence is closed the same way before continuing.
`node.input_resolved` only clears the matching attention generation; it neither
delivers an answer nor grants approval. A Pi worker may continue on an actually
received, authorized decision, not on an attention clear or adapter receipt alone.

### Tool and sub-workflow failure disclosure

Before closing, the worker inventories every failed or detectably incomplete
tool, command, external service, review, panel, or delegated workflow.

A **required** step in the brief or done criteria that remains failed or
incomplete blocks this attempt. Do not call `run merge`; write a `success: false`
report to `/tmp/node-report-${run_id}.json` and submit it with
`taskfleet node report "$run_id" n-0001 --from-file /tmp/node-report-${run_id}.json`.
An
**optional/advisory** failure may continue only when the deliverable is
independently complete and safe. Disclose it in the full `success: true` report
passed to `run merge` with `--report-file`, never the minimal auto-report.

Requested completeness is a contract: a panel with a missing model section, a
truncation marker, malformed output, or a missing artifact is incomplete, not representative consensus.
Retry only within a finite bound granted by existing workflow policy;
record each attempt and take the required or optional path at exhaustion.

Every distinct failure goes into one aggregate `discussion_items[]` entry whose
`topic` starts `Tool/sub-workflow failure —`, coalescing repeated attempts of the
same one: tool and purpose, expected completeness, observed error, attempts,
affected step, whether work continued and why that was safe, suggested bug
surface, and a stable artifact/log path when there is one. Actionable
retry/recover/accept/file steps go in the item's `options`. The whole entry stays
under 2 KiB with only a short redacted excerpt, never secrets, credentials,
personal data, environment dumps, or unbounded logs. Top-level `summary` and
`success` say whether the run is blocked or completed; do not put them inside the
item, and do not add a schema or supervisor state.

## Choosing a profile and creating the run

Profile names are installation-owned, not universal capability tiers. Use the
configured default by omitting `--profile`, unless the caller or repository
explicitly selects an available name. Describe the required capability and risk
in the brief, not a presumed local model or launcher. Preflight the complete
intended create with `--dry-run`; an unknown explicit profile or any other
preflight error stops creation, without silently substituting a profile. The
real call uses the selector whose preflight passed. For child creates, preflight
without both parent flags (child dry-run is unsupported), then restore them on
the real call with an idempotency key. On `profile_required`, surface the
configured choices and ask for a selection; do not invent or silently choose
one. Profile selection is not review evidence.

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
  should tolerate running twice. This is an optional operator-supplied hook,
  not a required parent wake mechanism; use the plain wait process above.
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

Tell the caller the full run id, source branch, and deliverable path or scope.
The worker merges and reports itself; follow the Parent wait and delivery
section above. Return structured run/node ids and branch to a driver caller.
The full report remains readable after teardown. Inspect the committed result
on the target before presenting it as delivered. Retained work is not disposable.

## Following the run and reading the outcome

To wait, use `taskfleet run wait "$run_id"` rather than a hand-rolled poll loop:
the backoff, the terminal set (`done | failed | cancelled`), and the rule to
branch on `manifest.status` rather than `lifecycle` (which is a fixed category
and never matches a terminal value) all live inside it. It exits `0` when the
run settles, `2` on `--timeout`, and `3` under `--fail-on-error` when the settled
run failed, was cancelled, stalled, or needs attention. If it has already
returned for awaiting input and you need the eventual terminal result, call it
again with `--terminal` instead of sleeping and retrying the default wait.
Resolve the open decision separately; `--terminal` does not answer it.
Several ids block until all settle, `--any` until the first. Its envelope is multi-run, so read `data.runs[]` (for example
`jq '.data.runs[] | {run_id, status, summary}'`), not `data.status`.

A harness-watched plain `taskfleet run wait` delivers results to the parent as
described above; `--notify` is an optional operator hook, not a general session
wake. A run you never waited on has no watcher. Everything persists after
teardown, so a late `run show` still answers.

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

Only with the optional repository tracker enabled, for an issue-driven run outside driver mode the worker owns the issue lifecycle;
you resolve the closing contract while building the brief and write the concrete
slug, status, and agent identity into it rather than metavariables. For status,
a bug closes as `fixed`; a feature/task/improvement/chore closes as `done`.
If the repository
customises types or delivery statuses and no single valid status follows from its
schema, the brief says to stop before implementation rather than guess.

The worker marks work begun with `issuectl update <slug> --status in-progress --json`,
commits only that metadata path, and starts implementation from a clean
tree. After the final validated implementation commit and before `run merge`, it
runs `issuectl close <slug> --status <fixed-or-done> --stamp --as <agent> --json`.
The stamp rewrites the implementation commit's message with a
`Fixes-Issue: @<slug>` trailer, which is what the trailer-driven changelog reads,
without touching its tree; `.data.stamp.status` has to be `stamped` or `already_present`, since `skipped` (detached HEAD, merge commit, signed, or mid
rebase) or a missing stamp means the landing commit would be invisible to the
changelog, and that blocks the merge. The closure metadata path issuectl returns
needs its own commit: commit that metadata in a separate commit, leaving the tree
clean before `run merge`. Do not add a `Fixes-Issue` trailer or close an issue for a freeform run;
in driver mode the driver owns the issue and
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
