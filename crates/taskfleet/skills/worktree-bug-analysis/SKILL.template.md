---
name: worktree-bug-analysis
description: "Spawn one autonomous read-only worker that analyses an already-filed bug and writes its findings into that issue under `## Triage analysis` (verdict, severity, affected area, repro status, fix sketch), then merges only the issue update back. Use when a bug slug needs understanding before a fix / defer / not-a-bug decision, such as 'analyse', 'understand', or 'figure out why' on an existing bug. Not for fixing it (`/worktree-spinoff <slug>`), filing a new bug, or open-ended multi-source research (`/worktree-research`)."
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# worktree-bug-analysis

A bug-analysis run is one autonomous worker whose deliverable is understanding,
not code. It takes an already-filed bug, reproduces or explains the behaviour,
finds the responsible area, classifies what it found, writes that into the
bug's own issue, and merges the issue update back with `taskfleet run merge`.
The user, or the triage skill that commissioned the run, then decides fix,
defer, or not-a-bug with the analysis in hand. Nobody reviews the worker
interactively and it cannot ask you anything: the brief is the whole
conversation, and the terminal report is all you hear back.

The run is created as `--kind spinoff`; there is no analysis kind, and the
read-only nature lives entirely in the brief. The run mechanics are shared with
`worktree-spinoff`: placement and PTY limits, driver flags, profiles,
`--notify`, `run wait`, the `landed` flag, and the error codes are documented
there and not repeated here. If the run / supervisor / node vocabulary is new
to you, read `taskfleet-overview` first.

## Is this a bug analysis?

It is when the user names an existing bug slug and wants it understood:
"analyse", "understand", "figure out why", "is this real". The triage skill
sends here the bug-like reports that already have a plausible path and material
impact but whose root cause or expected-behaviour status is unclear.

It is not one when the user wants the bug fixed; a fix is `/worktree-spinoff
<slug>`, driven by the same issue, and there is no separate bugfix variant. It
is not one when the bug has not been filed yet: this skill only writes into an
existing issue, so file it first. Open-ended multi-source investigation is
`/worktree-research`. And it is not worth a run when reading the report already
answers the question: each analysis costs a worker round, and bug reports
routinely describe a defect the current release has already fixed or a
read-surface mistake (the wrong field, the wrong command). A quick check against
the installed binary or the current source, done inline, often settles such a
report without spending a worker. Say what you found instead of spawning.

## What is at stake

**The disposition belongs to the user.** The analysis informs a fix / defer /
not-a-bug decision; it does not make it. The worker leaves the issue's status,
closing, and disposition labels alone, and commits the analysis with plain
`git` and a `Refs-Issue: @<slug>` trailer. The brief says it in as many words:
never use `Fixes-Issue`, `issuectl close --stamp`, or close the issue. The
trailer-driven changelog and `issuectl sync-commits` would otherwise record a
fix that never happened, and the user would find a bug marked resolved that is
still there.

**Application code stays untouched.** `run merge` lands the worker's branch on
the source branch with no review. A fix committed by an analysis worker
therefore arrives unreviewed, skips the quality bar a spinoff brief carries, and
pre-empts the decision the analysis was meant to inform. The committed change
is the issue directory only: `issues/<slug>/item.md` and, for a long trace,
`issues/<slug>/analysis.md`. Building the project and running tests or the
binary to reproduce is fine; a throwaway probe reverted before the commit is
fine; anything that survives into the commit outside the issue directory is a
fix and belongs to `/worktree-spinoff <slug>`.

**The heading is an interface.** issuectl derives its `needs_analysis` signal
and the `analysis` projection (`issuectl intake show <slug> --json`,
`issuectl intake queue --needs-analysis`) from the exact `## Triage analysis`
heading and nothing else. An earlier version of this skill let the worker pick
an alternative heading, and the mismatch left analysed issues looking
unanalysed, so every consumer either carried compatibility logic or
commissioned the same analysis again. Findings go under that exact heading.

**The source branch and the run's lifecycle.** `run merge` rebases the worker's
branch onto the run's recorded source branch and merges it there; the branch
checked out when you run `run create` is the default, and it is the only
success signal taskfleet has. Preserving unmerged work is the supervisor's job:
a blocked report, a failure, or a cancel keeps the branch and worktree for a
human. The real loss is a run that never terminalizes, which stays alive with a
dangling worktree while a triage batch waits on it. That is why the brief ends
with exactly one terminal path.

**The user's installed taskfleet.** When the repository under analysis is
taskfleet itself, the worker may `cargo build --release` and run
`./target/release/taskfleet …` from its worktree, but it never installs,
replaces, or removes the user's installed binary or bundled skills, by
`cargo install`, Homebrew, a manual copy, or any `skill install` variant. The
installed release is the user's production tool and legitimately differs from
source `HEAD`.

**The user's attention.** The only input this skill needs is the slug. A slug
whose `issues/<slug>/item.md` does not exist is the one thing worth stopping
for: report it and do not create the issue. Everything else (placement, title,
profile) is a routine call: make it, say what you chose, and continue.

## Before you spawn

You need a git repository with the current branch clean (workmux refuses to
materialize over uncommitted changes) and a compatible binary (see "Install or
upgrade" below). Capture the current branch; it is the default source and merge
target, and the caller should hear which one it was.

The request is parsed as `worktree-spinoff` parses a driver request: leading
`--headless`, `--tmux-session <name>`, or `--profile <name>` tokens are forwarded
to `run create`, and what remains is the bug slug. `issuectl show <slug> --json`
confirms the issue exists and gives you its title for the brief.

## The brief

`run create` prepends generated run context to every worker prompt, including a
custom `--prompt-file`: the exact run id, the `run show --current` ownership
resolver, and the issue-filing boundary (issues a worker files go through
`issuectl intake file` and are born unlaned). That generated text is
authoritative over the brief, so do not restate or weaken it. What the worker
still needs from you:

- **Objective.** Understand and scope the bug in `issues/<slug>/item.md` and
  write the findings back into that issue; do not fix it and do not change
  application code, for the reasons above.
- **What to read.** The item and every file under `issues/<slug>/attachments/`;
  a screenshot attached with `issuectl attach` is often the whole report.
- **What to establish.** Reproduce the behaviour or explain it from the code,
  and locate the responsible code path with file references. Name the commit or
  release examined, because reports often describe an already-fixed defect. If
  reproduction fails, say what was tried and why it did not work rather than
  guessing.
- **The verdict.** Real bug, expected behaviour, or cannot tell; rough severity
  and who it hits; and a sketch of what a fix would touch (files or areas, not
  an implementation). For "cannot tell", say whether a specific plausible path
  with material impact remains, because that is what separates a clarification
  lane from a close recommendation downstream.
- **Where it goes.** Into `issues/<slug>/item.md` under the exact heading
  `## Triage analysis`: verdict, severity, affected area, repro status, fix
  sketch. Keep it tight, since the reader is deciding, not debugging; a long
  trace goes to `issues/<slug>/analysis.md`, linked from the section.
- **The commit.** Plain `git` with a `Refs-Issue: @<slug>` trailer; never use
  `Fixes-Issue`, `issuectl close --stamp`, or close the issue.
- **Repository-local build safety**, the installed-taskfleet rule above, when
  the target repository is taskfleet.
- **Done criteria.** The issue carries the analysis under the heading, the
  branch is committed and merged back through `run merge`, and no application
  code changed.
- **The closing recipe and the failure-disclosure contract** from the two
  sections below: copy the disclosure contract below into the brief together
  with the closing recipe. The generated preamble carries no closing recipe for
  a spinoff-kind worker, so the brief is the only place it will see one.

A brief this long goes in a temp file (`mktemp -t bug-analysis-XXXXXX.md`)
passed as `--prompt-file`; the CLI copies it into the run directory, so remove
the temp file once `run create` returns.

### How the worker closes

The worker takes exactly one terminal path. A completed analysis goes through
`run merge`, which rebases and merges the branch and submits the terminal
report stamped `via: "explicit-merge"` in the same call. An analysis blocked by
a required failure does not merge; it submits a direct `success: false` report.
Taking neither leaves the run alive; taking both confuses the record.

The worker's run id is in its generated preamble. If a recipe has to recover it,
`taskfleet run show --current --output json` returns `.data.run_id` from the
durable ownership record and fails closed on missing, duplicate, stale, or
malformed evidence; the branch name's short fragment is display metadata that
can repeat, so it is never used as the id:

```bash
run_id="$(taskfleet run show --current --output json | jq -er '.data.run_id')" || {
  echo "failed to resolve exact owning run id" >&2
  exit 1
}
```

Once the issue update is committed, the worker writes the terminal report.
These field names are what the supervisor and the caller read; an unknown key
passes validation and is never read.

```bash
cat > /tmp/node-report-${run_id}.json <<'JSON'
{
  "success": true,
  "summary": "<one-line verdict, e.g. real bug, medium severity, in <area>>",
  "discussion_items": [],
  "spinoff_proposals": [],
  "wrap_up_recommendations": []
}
JSON
```

`success` is the one required field and `summary` is the verdict in one line;
that line is what `run wait` and a triage briefing surface. The three arrays are
usually empty. A fix worth doing goes in `spinoff_proposals[]` as
`{"proposed_title": "<non-empty>", "proposed_kind": "spinoff", "rationale":
"<why>"}`; only `spinoff`, `research`, `technical-decision`, and `fan-out`
exist as kinds, so a proposal naming `bugfix` is dropped by `run merge` with a
warning and rejects the whole file under `node report`. The per-run path
matters because two concurrent workers writing a shared `/tmp/node-report.json`
clobber each other.

Then merge and report in one call:

```bash
taskfleet run merge "$run_id" --report-file /tmp/node-report-${run_id}.json
```

The file is validated before the merge runs. `run merge` defaults to node
`n-0001`, the only node this run has, and merges into the recorded source
branch. The supervisor then tears down the worktree, tmux window, and branch
within a second or two and the worker's session ends as its window closes; the
worker does not run `tmux kill-window`, `git worktree remove`, or
`git branch -d` itself. On `error.code: "merge_failed"` no report was submitted
and the node stays live: resolve the conflict (or `/complex-rebase` for a
deeply diverged branch), commit, and re-run the same `run merge` command; the
report file is still on disk. The issue's status and labels stay as they were
throughout; the decision is the user's.

## Tool and sub-workflow failure disclosure

Before closing, inventory every failed or detectably incomplete tool, command,
external service, review, panel, or delegated workflow.

A step **required** by the brief or the done criteria that remains failed or
incomplete always blocks this attempt. Do not call `run merge`. Write the
report payload from "How the worker closes" to `/tmp/node-report-${run_id}.json`
with top-level `success: false`, then submit it with `taskfleet node report
"$run_id" n-0001 --from-file /tmp/node-report-${run_id}.json` (`n-0001` is the
sole node in this single-worker run). An **optional/advisory** failure
may continue only when the issue analysis is independently complete and safe;
disclose it in the full `success: true` report passed to `taskfleet run merge
"$run_id" --report-file /tmp/node-report-${run_id}.json`, never the minimal
auto-report.

Requested completeness is a contract. A missing requested reproduction,
inspection result, attachment, or expected artifact is incomplete and cannot be
presented as complete. Retry only when existing workflow policy authorizes a
finite bound; if none does, do not retry. Record each attempt and its outcome,
then take the required or optional path at exhaustion.

Create one aggregate `discussion_items[]` entry for the run whose `topic` starts
`Tool/sub-workflow failure —`. Cover every distinct failure, coalescing repeated
attempts of the same one: tool/workflow and purpose; expected completeness;
observed exit/error/incompleteness; attempts; affected step; whether work
continued and why safe; suggested bug surface; and a stable artifact/log path
when available. Put actionable retry/recover/accept/file steps in item-level
`options`. Keep the complete entry, including options, at most 2 KiB. Include
only a short redacted excerpt; never include secrets, credentials, personal
data, environment dumps, or unbounded logs. Set top-level `summary` and
`success` to distinguish blocked from completed; do not put them inside the
discussion item. Existing prose fields suffice, so do not add a schema or
supervisor state.

## Creating the run

```
taskfleet run create \
  --kind spinoff \
  --headless \
  --title "bug-analysis-<slug>" \
  --prompt-file <brief-file> \
  [--profile <name>] \
  [--source-branch <branch>] \
  [--notify <cmd>] \
  [--idempotency-key <key>] \
  [--dry-run]
```

Headless placement is the default here, not an option: the run's only output is
an issue update, nobody watches it, and a triage batch of five analyses would
otherwise fill the user's window list and, on macOS, exhaust pseudo-terminals
mid-batch. Drop `--headless` only when the user says they want to watch the
analysis live; a caller's `--tmux-session <name>` replaces it. The title
`bug-analysis-<slug>` makes the run recognisable in `run list` and the window
name. `--source-branch` defaults to the branch you captured. `--idempotency-key`
makes a retry after a transient error return the original run instead of
spawning a second one. Output defaults to `--output jsonl`.

The profile routing matrix in `taskfleet-overview` has no row for bug analysis:
forward an explicit caller `--profile` as their selection and otherwise let the
installation's default apply. When you are unsure a selector exists, preflight
the complete command with `--dry-run`; `unknown_profile` after materialization
costs a run.

## What comes back

The success envelope has the same shape as a spinoff's: `data.run_id` is the
handle for everything that follows, and `data.supervisor` is the supervisor's
pid. A string there instead (`not-spawned-dry-run`, `recorded-on-prior-run`, or
`delegated-to-parent-supervisor`) explains why none was spawned; anything else
non-numeric means nothing is driving the worker, which the user needs to hear.
`data.branch`, `data.worktree_path`, and `data.tmux_window` name what was
created.

Tell the user the run id, the source branch, the issue path
`issues/<slug>/item.md`, and that the run merges and reports itself, so no
`/worktree-merge` is needed from them. Be precise about how completion reaches
them: the run is out of band and nothing re-invokes this session by itself.
Promise "I'll tell you when it's done" only if you wired `--notify` or started a
background `taskfleet run wait <run-id>` through a harness facility that
re-invokes you; otherwise say plainly that they check `taskfleet run show
<run-id>` or ask you to wait on it, and that `tmux attach -t headless` shows
the worker live. Settled is not landed: read the `landed` flag from `run wait`
or `run show` rather than checking git ancestry yourself. The analysis itself
is visible on the source branch afterwards, in `git log -- issues/<slug>` or as
`.data.analysis` from `issuectl intake show <slug> --json`; a triage caller
confirms landing that way before mapping the verdict onto a recommendation.

When spawned by another skill rather than invoked directly by the user, return
the structured payload (run id, node id, branch) to the caller instead of a
human summary; it needs the ids to poll.

## Errors

`run create` fails with the same envelope and codes as `worktree-spinoff`
(`invalid_arguments`, `no_tmux_session`, `base_ref_not_found`,
`workmux_add_failed`, `unknown_profile`, `supervisor_spawn_failed`, and the
rest); branch on `error.code`, the message is prose. Nothing in the create path
is analysis-specific. A missing `issues/<slug>/item.md` is your error to report
before `run create`, not one of its codes.

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
  then stop; the `run create` surface may have changed.
- **Newer**: tell the user the installed skill is stale and stop. Refreshing
  installed bundled instructions is published-tool maintenance outside
  repository work; never run `skill install` as part of this workflow.
- **Equal**: proceed.

## Example

```
/worktree-bug-analysis checkout-total-off-by-one
```
