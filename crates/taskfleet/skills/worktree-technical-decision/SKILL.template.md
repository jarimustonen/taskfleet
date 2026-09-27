---
name: worktree-technical-decision
description: Spawn an autonomous worktree via `taskfleet run create --kind technical-decision` that drives ONE architectural or technical choice to a recorded ADR (architecture decision record) and merges itself back. Use when the user says "decide whether we should use X or Y", "make the architectural call on Z", "settle the trade-off between A and B", or points at an issue tagged decision/architecture. Not for an opinion (`/llm-consult`), design ideation (`/llm-workshop`), plan review (`/llm-panel`), an open survey with no chosen path (`/worktree-research`), or archaeology ("why did we choose X" is answered from past ADRs and the log, not by spawning).
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# worktree-technical-decision

A technical-decision worktree is one autonomous agent whose deliverable is a
recorded ADR in the repository: it investigates the options, weighs them
through the lenses the decision needs, picks one, writes down the decision with
its rationale and the alternatives it rejected, commits the file, and merges
itself back with `taskfleet run merge`. Nobody reviews it interactively and it
cannot ask you anything: the brief you write is the whole conversation, and the
terminal report it submits is all you hear back. Your work is deciding that the
request is a decision, pinning the question so it cannot drift, writing a brief
the worker can finish alone, creating the run, and telling the caller honestly
how they will learn the outcome.

The run mechanics are shared with `worktree-spinoff`: placement and PTY
limits, driver flags, profiles, `--notify`, `run wait`, the `landed` flag, the
awaiting-input signal, and the error codes are documented there and not
repeated here. If the run / supervisor / node vocabulary is new to you, read
`taskfleet-overview` first.

## Is this a decision?

It is when the user wants one path chosen and the choice written down so that
nobody re-litigates it: "decide whether X or Y", "make the architectural call
on Z", "settle the trade-off", or an issue tagged `decision` / `architecture`
that they want driven to an ADR. The product is a commitment with reasons.

It is not a decision when the user wants an opinion in the conversation
(`/llm-consult`), a design session (`/llm-workshop`), a review of a plan
(`/llm-panel`), or a survey of a space they will read and decide on later
(`/worktree-research`, which deliberately stops short of choosing; a decision
sent there comes back with the question unanswered, and a survey sent here
comes back with a choice nobody asked for). "Why did we choose X" is history:
read the existing ADRs and the commit log and answer inline.

## What is at stake

**The question.** Decisions fail when the question drifts. A worker given
"look at our storage options" will answer some question competently; the user
wanted a specific one. So the brief carries the question as one forward-looking
sentence, the constraints that are not negotiable (existing technology,
deadlines, team skills, regulation), the options genuinely on the table, and
what would make one answer better than another. If the request does not say
which choice is being made or what bounds it, one question to the user before
spawning is cheaper than the run it would misdirect. Everything else, the
title, the ADR path where the repository has a convention, the lenses, the
profile, is a routine call: make it, say what you chose, and continue.

**The record.** An ADR outlives the run by years and is read as the reason a
thing is the way it is; in this repository, `AGENTS.md` cites ADRs as the
grounds for not resurrecting deleted designs. A confident ADR built on a weak
comparison misleads every later reader, which is why the rejected alternatives
and their reasons are part of the deliverable, why evidence comes before
opinion, and why a decision the evidence cannot make is reported as blocked
rather than settled by a coin flip.

**The source branch.** `run merge` rebases the worker's branch onto the run's
recorded source branch and merges it there; the branch checked out when you run
`run create` is the default, and the caller should hear which one it was. The
merge is also taskfleet's only success signal: a worker that finishes and skips
it has delivered nothing the caller can see.

**The worker's report.** Preserving unmerged work is the supervisor's job: a
blocked report, a failure, or a cancel keeps the branch and worktree for a human
to harvest. The real loss is a run that never terminalizes. It stays alive, the
worktree dangles, and anyone waiting on it waits forever. That is why the brief
ends with exactly one terminal path, and why the report's field names are
exact: they are an interface the supervisor and the caller parse.

**The user's installed taskfleet.** When the repository is taskfleet itself, a
worker may `cargo build --release` and run `./target/release/taskfleet …` from
its worktree, but it never installs, replaces, or removes the user's installed
binary or bundled skills, by `cargo install`, Homebrew, a manual copy, or any
`skill install` variant. The installed release is the user's production tool
and legitimately differs from source `HEAD`.

## Pinning the decision

Write down, before anything else: the question as one sentence posed as a
forward choice ("Should we use X or Y for Z?"); the constraints; the options to
weigh, at least two, with the worker free to add a real one it finds but not to
invent strawmen that make the favourite look good; the lenses the decision
needs, usually architecture, maintainability, and security, plus performance,
cost, or ergonomics where they actually bear on it; and where the ADR lands.

Find the ADR location from the repository, not from habit. Check for an
existing ADR directory (`docs/decisions/`, `docs/adr/`, or whatever
`AGENTS.md` names), its README or template, and the numbering of the files
already there; the worker gets the exact path and the next number. This
repository keeps ADRs at `docs/decisions/<NNNN>-<slug>.md` with a header of
Status / Date / Decider / Issue bullets followed by Context, Decision, and
Consequences. A repository with no ADR convention gets that same layout under
`docs/decisions/`, and the caller hears that you chose it.

## The brief

`run create` prepends generated run context to every worker prompt, including
a custom `--prompt-file`: the exact run id, the `run show --current` ownership
resolver, and the issue-filing boundary (issues a worker files go through
`issuectl intake file` and are born unlaned). That generated text is
authoritative over the brief, so do not restate or weaken it. Only a pi
*research* worker gets a closing recipe in its preamble; a technical-decision
worker gets none, so the brief carries the closing recipe and the
failure-disclosure contract below in full. What the worker still needs from
you:

- **The pinned question, constraints, options, lenses, and ADR path** from
  above.
- **The evidence bar.** Primary sources, the repository's own code and history,
  deterministic checks, and source-grounded scenarios come first; the worker
  applies the required lenses itself against that evidence. A model panel is
  worth running only where the decision holds a genuine unresolved trade-off
  on which independent perspectives add evidence, and the brief or the ADR says
  why it was run. A panel that was not needed adds cost and a false sense of
  consensus; a panel that was needed and came back incomplete cannot support an
  Accepted ADR, because the missing voice may be the dissent.
- **The ADR shape:** title; status `Accepted`; context; the decision; its
  consequences, including each rejected alternative with the reason it lost;
  date and deciders. The repository's own template wins where one exists.
- **Done criteria:** the ADR exists at the agreed path, is committed, and is
  merged back through `run merge`. No code changes ride along; if the decision
  implies implementation, the ADR says so and the report proposes a spinoff,
  which keeps the decision commit reviewable on its own.
- **Repository-local build safety**, the installed-taskfleet rule above, when
  the target repository is taskfleet.
- **What a tie means.** If the worker has a lean, that is the decision, and the
  ADR records how close it was. If the evidence genuinely cannot separate two
  options, the choice belongs to the user: the worker records the fork as an
  awaiting-input signal (the recipe is in `worktree-spinoff`) in case someone is
  watching, and otherwise closes blocked with the unresolved trade-off in
  `discussion_items[]`, leaving the branch with its draft and evidence for the
  user to break the tie and re-spawn or harvest.
- **The closing recipe and the failure-disclosure contract** from the two
  sections below, copied in.

A brief longer than about 2 KB, or one with awkward shell quoting, goes in a
temp file passed as `--prompt-file`; the CLI copies it into the run directory,
so remove the temp file once `run create` returns.

### How the worker closes

The worker takes exactly one terminal path. A decision made and its ADR
committed goes through `run merge`, which rebases and merges the branch and
submits the terminal report stamped `via: "explicit-merge"` in the same call.
A decision that is blocked, by a tie or by a required step that failed, does
not merge; it submits a direct `success: false` report. Taking neither leaves
the run alive; taking both confuses the record.

The worker's run id is in its generated preamble. If a recipe has to recover
it, `taskfleet run show --current --output json` returns `.data.run_id` from
the durable ownership record and fails closed on missing, duplicate, stale, or
malformed evidence; the branch name's short fragment is display metadata that
can repeat, so it is never used as the id.

Once the ADR is committed, the worker writes the terminal report. These field
names are what the supervisor and the caller read; an unknown key such as
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
follow-up work worth spawning, such as the implementation an ADR mandates
(`{"proposed_title": "<non-empty>", "proposed_kind":
"spinoff|research|technical-decision|fan-out", "rationale": "<why>"}`, and
only those four kinds exist: `run merge` drops a proposal naming any other kind
with a warning, and `node report` rejects the whole file);
`wrap_up_recommendations[]` is an array of strings for the caller. The per-run
path matters because two concurrent workers writing a shared
`/tmp/node-report.json` clobber each other.

Then merge and report in one call:

```bash
taskfleet run merge "$run_id" --report-file /tmp/node-report-${run_id}.json
```

The file is validated before the merge runs. `run merge` defaults to node
`n-0001`, the only node a technical-decision run has, and merges into the
recorded source branch unless `--source <branch>` says otherwise. The
supervisor then tears down the worktree, tmux window, and branch within a
second or two and the worker's session ends as its window closes; the worker
does not run `tmux kill-window`, `git worktree remove`, or `git branch -d`
itself. On `error.code: "merge_failed"` no report was submitted and the node
stays live: resolve the conflict (or `/complex-rebase` for a deeply diverged
branch), commit, and re-run the same `run merge` command; the report file is
still on disk.

The blocked path submits the same file, with `success: false` and the
unresolved trade-off or failure in `discussion_items[]`, directly:

```bash
taskfleet node report "$run_id" n-0001 --from-file /tmp/node-report-${run_id}.json
```

A `success: false` report makes the node and the run `failed`; that is what a
blocked decision looks like in `run show`, and it is correct: the supervisor
winds the run down but preserves the branch and worktree, which `run show`
lists under `preserved_work`, so the draft and evidence are there for the user.

## Tool and sub-workflow failure disclosure

Before closing, inventory every failed or detectably incomplete tool, command,
external service, review, panel, or delegated workflow.

A step **required** by the brief or the done criteria that remains failed or
incomplete always blocks this attempt. Do not call `run merge`. Write the
report payload from "How the worker closes" to `/tmp/node-report-${run_id}.json`
with top-level `success: false`, then submit it with `taskfleet node report
"$run_id" n-0001 --from-file /tmp/node-report-${run_id}.json` (`n-0001` is the
sole node in this single-worker run). An **optional/advisory** failure may
continue only when the ADR is independently complete and safe; disclose it in
the full `success: true` report passed to `taskfleet run merge "$run_id"
--report-file /tmp/node-report-${run_id}.json`, never the minimal auto-report.

Requested completeness is a contract. When a concrete trade-off made a panel
required, a missing model section, truncation marker, malformed output, or
missing expected artifact is incomplete, not representative consensus, and an
incomplete required panel cannot support an Accepted ADR. Do not launch a panel
merely to create this requirement. Retry only when existing workflow policy
authorizes a finite bound; if none does, do not retry. Record each attempt and
its outcome, then take the required or optional path at exhaustion.

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

## Choosing a profile and creating the run

You need a git repository and a compatible binary (see "Install or upgrade"
below). Capture the current branch; it is the default source and merge target.
`run create` runs inside tmux or with `--headless` / `--tmux-session <name>`.

The routing matrix in `taskfleet-overview` sends technical decisions to the
`capable` profile, because a weak comparison becomes a durable record. A
profile is a capability tier the user's `config.toml` maps to a harness and
argv, so the brief never names a concrete model or command. An explicit caller
`--profile` (a driver may prefix the request with one; strip it from the
question text) is their selection and may escalate; do not downgrade it.
Profile names are user-owned, so find out that the one you intend exists before
creating state: run the complete intended command with `--dry-run`. On
`unknown_profile` for the workflow default, an empty `expected` list means the
installation predates profiles and the real call omits `--profile`; a non-empty
list without `capable` means you stop and surface the structured error rather
than pick another name. An unknown profile the caller chose explicitly is theirs
to fix. Any other dry-run error also stops. The real call uses exactly the
selector whose dry-run passed.

```
# First issue this command with --dry-run; remove only --dry-run after it passes.
taskfleet run create \
  --kind technical-decision \
  --title "<adr-slug>" \
  --task "<self-contained decision brief>" \
  [--profile <selected-profile>] \
  [--source-branch <branch>] \
  [--headless] \
  [--notify <cmd>] \
  [--idempotency-key <key>] \
  [--dry-run]
```

Exactly one of `--task` and `--prompt-file`. `--idempotency-key` makes a retry
after a transient error return the original run instead of spawning a second
one. Output defaults to `--output jsonl`.

## What comes back

The success envelope has the same shape as a spinoff's: `data.run_id` is the
handle for everything that follows, and `data.supervisor` is the supervisor's
pid. A string there instead (`not-spawned-dry-run`, `recorded-on-prior-run`, or
`delegated-to-parent-supervisor` for a run created with the parent flags)
explains why none was spawned; anything else non-numeric means nothing is
driving the worker, which the user needs to hear. `data.branch`,
`data.worktree_path`, and `data.tmux_window` name what was created.

Tell the user the run id, the source branch, the tmux window and the session it
is in, the path the ADR will land at, and that the run merges and reports
itself, so no `/worktree-merge` is needed from them. Be precise about how
completion reaches them: the run is out of band and nothing re-invokes this
session by itself. Promise "I'll tell you when it's done" only if you wired
`--notify` or started a background `taskfleet run wait <run-id>` through a
harness facility that re-invokes you; otherwise say plainly that they check
`taskfleet run show <run-id>` or ask you to wait on it. `run wait` blocks until
the run is terminal without a hand-rolled poll loop, and `taskfleet event tail
<run-id> --follow` streams the log. Settled is not landed: read the `landed`
flag from `run wait` or `run show` rather than checking git ancestry yourself.
A `failed` run whose report carries a tie is the blocked path working as
designed, not a crash; the branch is preserved and the user breaks the tie. The
report persists on the node and `run show` exposes it as `data.report`, so a
late `run show` still answers after teardown.

## Issue-driven decisions

A run spawned by a driver with the parent flags leaves the issue to the driver.
Otherwise, when the decision came from an issue, the ADR is that issue's
deliverable and the worker closes it. Write the concrete slug into the brief and
have the ADR name it. After the final validated ADR commit and before
`run merge`, the worker runs `issuectl close <slug> --status done --stamp --as
<agent> --json`. The stamp rewrites the ADR commit's message with a
`Fixes-Issue: @<slug>` trailer, which is what the trailer-driven changelog
reads; `.data.stamp.status` has to be `stamped` or `already_present`, because
`skipped` (detached HEAD, merge commit, signed, mid rebase) means the landing
commit would be invisible to the changelog, and that blocks the merge. The
closure metadata path issuectl returns is committed separately, and the tree is
clean before `run merge`. A blocked decision leaves the issue open.

A freeform decision adds no trailer and touches no issue.

## Errors

`run create` fails with the same envelope and codes as `worktree-spinoff`
(`invalid_arguments`, `no_tmux_session`, `base_ref_not_found`,
`workmux_add_failed`, `unknown_profile`, `supervisor_spawn_failed`, and the
rest); branch on `error.code`, the message is prose. Nothing in the create path
is decision-specific.

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
  then stop; the `run create --kind technical-decision` surface may have
  changed.
- **Newer**: tell the user the installed skill is stale and stop. Refreshing
  installed bundled instructions is published-tool maintenance outside
  repository work; never run `skill install` as part of this workflow.
- **Equal**: proceed.

## Example

```
/worktree-technical-decision Choose between event-sourced and CRUD storage for the taskfleet run state
```
