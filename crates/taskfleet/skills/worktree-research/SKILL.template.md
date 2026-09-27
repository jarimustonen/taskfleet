---
name: worktree-research
description: Spawn an autonomous research worktree via `taskfleet run create --kind research` — one background worker that reads several sources, weighs divergent perspectives, writes a sourced markdown report into the repository, commits it, and merges itself back. Use when the user asks to research, investigate, survey, look into, or compare options for a topic that needs more than one source read and synthesized into a written report. Not for a one-search factual lookup, a single-document summary, debugging (`/worktree-bug-analysis`), code changes (`/worktree-spinoff`), or a forward decision that ends in an ADR (`/worktree-technical-decision`).
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# worktree-research

A research worktree is one autonomous agent whose deliverable is a sourced
markdown report, not code. It reads several sources, weighs the perspectives
that disagree, writes the report into the repository, commits it, and merges
itself back with `taskfleet run merge`. Nobody reviews it interactively, and it
cannot ask you anything: the brief you write is the whole conversation, and the
terminal report it submits is all you hear back. Your work is therefore mostly
deciding that the request is research-shaped, sharpening the question, writing a
brief the worker can finish alone, creating the run, and telling the caller
honestly how they will learn the outcome.

The run mechanics are shared with `worktree-spinoff`: placement and PTY limits,
driver flags, profiles, `--notify`, `run wait`, the `landed` flag, and the
error codes are documented there and not repeated here. If the run /
supervisor / node vocabulary is new to you, read `taskfleet-overview` first.

## Is this research?

It is when the user wants a written survey they will read and cite later:
"research the state of X", "investigate the options for Y", "compare A, B, and C
in depth". The report is the product, and the topic needs several sources
weighed against each other.

It is not research when one web search answers the question, when the user
wants one document summarised (read it inline), when the subject is a bug
(`/worktree-bug-analysis` explains it, `/worktree-spinoff` fixes it), or when
the user is asking which option to pick. That last one is a decision and belongs
to `/worktree-technical-decision`, which records an ADR; a research report
deliberately stops short of choosing, so a decision sent here comes back with
the actual question unanswered.

## What is at stake

**Hours of unattended work aimed by one paragraph.** A research run reads and
writes for a long time with no chance to check back. A brief that misreads the
question or leaves the scope open produces a competent report on the wrong
thing, and the cost is the whole run plus a merge cycle. When the request
genuinely does not say what is being asked, one question to the user is cheaper
than the run it would misdirect. Everything else (title, placement, output path
where the repository has a convention) is a routine call: make it, say what you
chose, and continue.

**The source branch.** `run merge` rebases the worker's branch onto the run's
recorded source branch and merges it there. The branch checked out when you run
`run create` is the default, so the caller should hear which one it was. The
merge is also taskfleet's only success signal: a worker that finishes and skips
it has delivered nothing the caller can see.

**The worker's report.** Preserving unmerged work is the supervisor's job: a
blocked report, a failure, or a cancel keeps the branch and worktree for a human
to harvest. The real loss is a run that never terminalizes. It stays alive, the
worktree dangles, and anyone waiting on it waits forever. That is why the brief
ends with exactly one terminal path, and why the report's field names are exact:
they are an interface the supervisor and the caller parse.

**The user's installed taskfleet.** When the repository under study is
taskfleet itself, a worker may `cargo build --release` and run
`./target/release/taskfleet …` from its worktree, but it never installs,
replaces, or removes the user's installed binary or bundled skills, by
`cargo install`, Homebrew, a manual copy, or any `skill install` variant. The
installed release is the user's production tool and legitimately differs from
source `HEAD`.

## Sharpening the question

Distil the request into a core question in one sentence; the scope that counts
as in bounds (timeframe, ecosystems, platforms, audience); what is explicitly
out of scope, since an open-ended worker otherwise wanders; the audience and
tone (engineer-facing, executive-facing, Finnish); and where the report lands.

The default location is `research/<slug>.md`. Repository conventions override
it: check the repository's `AGENTS.md`, and note that a repository which keeps
planning documents under their issue directory expects an issue-driven report
at `issues/<slug>/analysis.md` rather than under `research/`. The worker is
told the exact path; it does not choose one.

## The brief

`run create` prepends generated run context to every worker prompt, including a
custom `--prompt-file`: the exact run id, the `run show --current` ownership
resolver, and the issue-filing boundary (issues a worker files go through
`issuectl intake file` and are born unlaned). A pi research worker additionally
gets a translation note that neutralises Claude-only slash commands and carries
the clean closing recipe with the run id already filled in. That generated text
is authoritative over the brief, so do not restate or weaken it. What the worker
still needs from you:

- **The question, scope, exclusions, audience, and output path** from above.
- **The source-quality bar.** Primary sources (specifications, vendor
  documentation, original papers, source code) over aggregators and
  second-hand summaries, and every non-trivial claim cited inline with a URL,
  so the reader can check it later without redoing the research.
- **Divergent perspectives.** Where the topic admits more than one credible
  view, the report presents at least two and says where they disagree, rather
  than smoothing a contested area into a single narrative. Consensus that is
  not actually there is the most expensive error a survey can make, because
  the reader cannot see it.
- **A structure the user can read top-down:** a summary of three to five
  bullets, background, options or findings, trade-offs, with citations kept
  inline next to the claims they support.
- **Done criteria:** the report exists at the agreed path, is committed, and is
  merged back through `run merge`.
- **Repository-local build safety**, the installed-taskfleet rule above, when
  the target repository is taskfleet.
- **The closing recipe and the failure-disclosure contract** from the two
  sections below: copy the disclosure contract below into the brief together
  with the closing recipe. The pi preamble's own recipe covers only the clean
  path, and a worker on any other harness gets no recipe at all.

A brief longer than about 2 KB, or one with awkward shell quoting, goes in a
temp file passed as `--prompt-file`; the CLI copies it into the run directory,
so remove the temp file once `run create` returns.

### How the worker closes

The worker takes exactly one terminal path. A completed, mergeable report goes
through `run merge`, which rebases and merges the branch and submits the terminal
report stamped `via: "explicit-merge"` in the same call. Research blocked by a
required failure does not merge; it submits a direct `success: false` report.
Taking neither leaves the run alive; taking both confuses the record.

The worker's run id is in its generated preamble. If a recipe has to recover it,
`taskfleet run show --current --output json` returns `.data.run_id` from the
durable ownership record and fails closed on missing, duplicate, stale, or
malformed evidence; the branch name's short fragment is display metadata that
can repeat, so it is never used as the id.

Once the report file is committed, the worker writes the terminal report. These
field names are what the supervisor and the caller read; an unknown key such as
`discuss` or `wrap_up` passes validation and is never read.

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
"<why>"}`, and only those four kinds exist: `run merge` drops a proposal naming
any other kind with a warning, and `node report` rejects the whole file);
`wrap_up_recommendations[]` is an array of strings for the caller. The per-run
path matters because two concurrent workers writing a shared
`/tmp/node-report.json` clobber each other.

Then merge and report in one call:

```bash
taskfleet run merge "$run_id" --report-file /tmp/node-report-${run_id}.json
```

The file is validated before the merge runs. `run merge` defaults to node
`n-0001`, the only node a research run has, and merges into the recorded source
branch unless `--source <branch>` says otherwise. The supervisor then tears
down the worktree, tmux window, and branch within a second or two and the
worker's session ends as its window closes; the worker does not run
`tmux kill-window`, `git worktree remove`, or `git branch -d` itself. On
`error.code: "merge_failed"` no report was submitted and the node stays live:
resolve the conflict (or `/complex-rebase` for a deeply diverged branch),
commit, and re-run the same `run merge` command; the report file is still on
disk.

## Tool and sub-workflow failure disclosure

Before closing, inventory every failed or detectably incomplete tool, command,
external service, review, panel, or delegated workflow.

A step **required** by the brief or the done criteria that remains failed or
incomplete always blocks this attempt. Do not call `run merge`. Write the
existing §7.3 report payload to `/tmp/node-report-${run_id}.json` with top-level
`success: false`, then submit it with `taskfleet node report "$run_id"
n-0001 --from-file /tmp/node-report-${run_id}.json` (`n-0001` is the sole node
in this single-worker run). An **optional/advisory** failure may continue only
when the report is independently complete and safe; disclose it in the full
`success: true` report passed to `taskfleet run merge "$run_id"
--report-file /tmp/node-report-${run_id}.json`, never the minimal auto-report.

Requested completeness is a contract. A requested panel with a missing model
section, truncation marker, malformed output, or missing expected artifact is
incomplete, not representative consensus. Retry only when existing workflow
policy authorizes a finite bound; if none does, do not retry. Record each attempt
and its outcome, then take the required or optional path at exhaustion.

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

You need a git repository and a compatible binary (see "Install or upgrade"
below). Capture the current branch; it is the default source and merge target.
`run create` runs inside tmux or with `--headless` / `--tmux-session <name>`.

```
taskfleet run create \
  --kind research \
  --title "<2–4 word slug>" \
  --task "<self-contained research brief>" \
  [--profile <name>] \
  [--source-branch <branch>] \
  [--headless] \
  [--notify <cmd>] \
  [--idempotency-key <key>] \
  [--dry-run]
```

Exactly one of `--task` and `--prompt-file`. `--idempotency-key` makes a retry
after a transient error return the original run instead of spawning a second
one. Output defaults to `--output jsonl`.

The worker runs under the pi harness by default, and on an installation with
executable profiles configured an autonomous run accepts only a pi candidate
with `worker-v1` telemetry; `--harness claude` is honoured only on an
installation that predates profiles. The routing matrix in `taskfleet-overview`
recommends no profile for research, so forward an explicit caller `--profile`
as their selection and otherwise let the installation's default apply. When you
are unsure a selector exists, preflight the complete command with `--dry-run`;
`unknown_profile` after materialization costs a run.

The worker's web access is whatever its harness provides. taskfleet does not
check that a research worker can fetch anything, so a report full of "could not
fetch sources" notes is the symptom of a harness with no web tools, and worth
surfacing to the user as such.

## What comes back

The success envelope has the same shape as a spinoff's: `data.run_id` is the
handle for everything that follows, and `data.supervisor` is the supervisor's
pid. A string there instead (`not-spawned-dry-run`, `recorded-on-prior-run`)
explains why none was spawned; anything else non-numeric means nothing is
driving the worker, which the user needs to hear. `data.branch`,
`data.worktree_path`, and `data.tmux_window` name what was created.

Tell the user the run id, the source branch, the tmux window and the session it
is in, the path the report will land at, and that the run merges and reports
itself, so no `/worktree-merge` is needed from them. Be precise about how
completion reaches them: a research run is out of band and nothing re-invokes
this session by itself. Promise "I'll tell you when it's done" only if you wired
`--notify` or started a background `taskfleet run wait <run-id>` through a
harness facility that re-invokes you; otherwise say plainly that they check
`taskfleet run show <run-id>` or ask you to wait on it. `run wait` blocks until
the run is terminal without a hand-rolled poll loop, and `taskfleet event tail
<run-id> --follow` streams the log. Settled is not landed: read the `landed`
flag from `run wait` or `run show` rather than checking git ancestry yourself.
The report persists on the node and `run show` exposes it as `data.report`, so
a late `run show` still answers after teardown.

## Issue-driven research

A research run spawned by a driver with the parent flags leaves the issue to
the driver. Otherwise, when the request came from an issue, decide which of two
cases holds and write the concrete slug into the brief:

- The report is evidence for a larger issue: the worker commits it with a
  `Refs-Issue: @<slug>` trailer and does not close or stamp the issue.
- The report is the issue's whole deliverable: after the final validated report
  commit and before `run merge`, the worker runs `issuectl close <slug>
  --status done --stamp --as <agent> --json`. The stamp rewrites the report
  commit's message with a `Fixes-Issue: @<slug>` trailer, which is what the
  trailer-driven changelog reads; `.data.stamp.status` has to be `stamped` or
  `already_present`, because `skipped` (detached HEAD, merge commit, signed, mid
  rebase) means the landing commit would be invisible to the changelog, and
  that blocks the merge. The closure metadata path issuectl returns is committed
  separately, and the tree is clean before `run merge`.

Freeform research adds no trailer and touches no issue.

## Errors

`run create` fails with the same envelope and codes as `worktree-spinoff`
(`invalid_arguments`, `no_tmux_session`, `base_ref_not_found`,
`workmux_add_failed`, `unknown_profile`, `supervisor_spawn_failed`, and the
rest); branch on `error.code`, the message is prose. Nothing in the create path
is research-specific.

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
  then stop; the `run create --kind research` surface may have changed.
- **Newer**: tell the user the installed skill is stale and stop. Refreshing
  installed bundled instructions is published-tool maintenance outside
  repository work; never run `skill install` as part of this workflow.
- **Equal**: proceed.

## Example

```
/worktree-research Compare WAL implementations in SQLite, DuckDB, and Postgres for write-heavy embedded use
```
