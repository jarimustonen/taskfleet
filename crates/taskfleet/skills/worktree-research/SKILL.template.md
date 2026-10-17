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

The run mechanics are shared with `worktree-spinoff`: native placement,
driver flags, profiles, `--notify`, `run wait`, the `landed` flag, and the
error codes are documented there and not repeated here. If the run /
supervisor / node vocabulary is new to you, read `taskfleet-overview` first.

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
can repeat, so it is never used as the id. If the generated preamble's full id
is unavailable, recover it before writing the per-run report:

```bash
run_id="$(taskfleet run show --current --output json | jq -er '.data.run_id')" || {
  echo "failed to resolve exact owning run id" >&2
  exit 1
}
```

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
report payload from "How the worker closes" to `/tmp/node-report-${run_id}.json`
with top-level `success: false`, then submit it with `taskfleet node report
"$run_id" n-0001 --from-file /tmp/node-report-${run_id}.json` (`n-0001` is the
sole node in this single-worker run). An **optional/advisory** failure
may continue only when the report is independently complete and safe; disclose it
in the full `success: true` report passed to `taskfleet run merge "$run_id"
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
Use `--headless` when the parent has no attached terminal. Native tmux remains
Taskfleet's implementation detail, not the human's required interface; explicit
`--tmux-session <name>` or configured placement may also apply.

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

The worker uses the installation's configured default unless explicitly selected.
An autonomous run accepts pi with `worker-v1`
telemetry or Claude without telemetry. Claude uses headless `--print` mode;
its exit is recorded separately from the explicit `run merge` completion.
The profile argv and permission policy must be defined in the user's config,
not the repository's selection-only config. Forward an explicit caller `--profile` as their selection and otherwise let
the installation's configured default apply. When you
are unsure a selector exists, preflight the complete command with `--dry-run`;
`unknown_profile` after materialization costs a run.

The worker's web access is whatever its harness provides. taskfleet does not
check that a research worker can fetch anything, so a report full of "could not
fetch sources" notes is the symptom of a harness with no web tools, and worth
surfacing to the user as such.

## What comes back

The success envelope has the same shape as a spinoff's: `data.run_id` is the
handle for everything that follows, and `data.supervisor` is the supervisor's
pid. A string there instead (`not-spawned-dry-run`, `recorded-on-prior-run`, or
`delegated-to-parent-supervisor` for a run created with the parent flags)
explains why none was spawned; anything else non-numeric means nothing is
driving the worker, which the user needs to hear. `data.branch`,
`data.worktree_path`, and `data.tmux_window` (the actual
`<workmux-reported-window-name>`) name what was created.

Tell the caller the full run id, source branch, and deliverable path or scope.
The worker merges and reports itself; follow the Parent wait and delivery
section above. Return structured run/node ids and branch to a driver caller.
The full report remains readable after teardown. Inspect the committed result
on the target before presenting it as delivered. Retained work is not disposable.

## Issue-driven research

A research run spawned by a driver with the parent flags leaves the issue to
the driver. Otherwise, only with the optional repository tracker enabled, when the request came from an issue, decide which of two
cases holds and write the concrete slug into the brief:

- The report is evidence for a larger issue: the worker commits it with a
  `Refs-Issue: @<slug>` trailer and does not close or stamp the issue.
- The report is the issue's whole deliverable: after the final validated report
  commit and before `run merge`, the worker runs
  `issuectl close <slug> --status done --stamp --as <agent> --json`. The stamp rewrites the report
  commit's message with a `Fixes-Issue: @<slug>` trailer, which is what the
  trailer-driven changelog reads; `.data.stamp.status` has to be `stamped` or
  `already_present`, because `skipped` (detached HEAD, merge commit, signed, mid
  rebase) means the landing commit would be invisible to the changelog, and
  that blocks the merge. The closure metadata path issuectl returns is committed
  separately, and the tree is clean before `run merge`.

Freeform research has no issue trailer and touches no issue.

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
