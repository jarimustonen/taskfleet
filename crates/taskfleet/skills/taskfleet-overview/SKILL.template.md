---
name: taskfleet-overview
description: First read for any agent that has just discovered the `taskfleet` binary mid-conversation. Explains what taskfleet is, its vocabulary (runs, nodes, supervisors, kinds, profiles), the output envelope, the create → supervise → merge cycle, and which bundled skill to open next. Use when asked "what is taskfleet", when the binary appears in a session for the first time, or before issuing the first non-trivial taskfleet command.
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# taskfleet-overview

`taskfleet` is the state owner for AI-agent workflows on a developer's machine.
It spawns an agent into an isolated git worktree, supervises it with a per-run
process, and merges its work back. Every workflow is a **run** with canonical
state under `<taskfleet home>/runs/<run-id>/` (the home is `TASKFLEET_HOME`,
default `~/.taskfleet`): an append-only `events.jsonl` plus projections such as
`manifest.json` and `nodes/`. Any command or UI reading a run reads that same
truth. Every other bundled `taskfleet-*` and `worktree-*` skill assumes the
vocabulary here.

The binary's `--help` text is the authoritative description of every verb and
flag; this skill tells you what the help text cannot: how the pieces fit, which
fields mean what, and where earlier mistakes came from.

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

## Vocabulary

- **Run** — one workflow, identified by a ULID. Created with `run create`.
- **Kind** — the run's topology: `spinoff` (one focused coding task),
  `research`, `technical-decision`, `fan-out` (a parent run with one child run
  per unit). Kind is about the work, not about how it is supervised.
- **Node** — one unit of work inside a run. Every run starts with node
  `n-0001`. Node ids are `n-` followed by 4–10 ASCII digits and the binary
  rejects anything else, so never invent a slug such as `n-driver-001`; take
  the id from `run create`'s `node_id` field or from `node list`.
- **Supervisor** — the long-lived `taskfleet supervise <run-id>` process that
  drives the run's worker, records what it is told, and tears the worktree,
  tmux window, and branch down when the run ends. `run create` starts it;
  `run reattach` restarts a dead one. Agents rarely invoke it directly.
- **Profile** — a named, user-owned executable definition (harness plus argv)
  in the taskfleet home's `config.toml`, selected with `run create --profile`.
- **Report** — the structured terminal document a worker submits to end its
  node. It is usually submitted through `run merge`, not directly.

## Output contract

Every machine-readable command emits the canonical envelope, except the two
streaming ones: `skill print` writes the raw file and `event tail` writes one
raw event per line followed by a single terminating envelope.

```json
{"schema_version": 1, "data": {...}, "warnings": ["..."]}
```

The default `--output` is `jsonl`, one compact envelope per line on stdout;
`--output json` (or `--json`) is a single pretty document; `--output text` is a
human summary and not stable enough to parse. Errors are a separate envelope on
**stderr** with a non-zero exit:
`{"schema_version": 1, "error": {"code": "<snake_case>", "message": "..."}}`,
often with `invalid_value` and `expected` fields. The `code` is the stable
part; the message is prose that may change between versions. A
`schema_version` you do not recognise means the data shape may have moved under
you, so treat what you parse from it as unverified.

## The cycle: create → supervise → merge

`run create --kind <kind> --title "..."` with the brief as `--task "<inline>"`
or `--prompt-file <path>` (exactly one of the two; there is no `--prompt`
flag) creates the run, materializes the worktree, launches the worker, and
starts the supervisor. Its `data` payload carries `run_id`, `node_id`, `kind`,
`lifecycle`, and the worktree location. The run's progress starts at
`status: pending`.

The supervisor then records what it is told: worker exit, telemetry, reports.
`run show <id>` reads the current state; `run wait <id>...` blocks until runs
settle (terminal or attention-required) and is the right tool instead of a
hand-rolled polling loop. A timeout is not completion. `event tail <id> --follow` streams the event log for live monitoring.

The worker closes with `run merge <run-id> [--source <branch>]
[--report-file <path>]`, which rebases and merges the branch into its source,
submits the terminal report stamped `via: "explicit-merge"`, and lets the
supervisor tear the worktree down. This one call is the **only success truth**
in taskfleet: the supervisor never infers "done" from a merged-looking branch,
an idle pane, or a clean exit. A worker that exits cleanly without calling
`run merge` leaves the run non-terminal with `attention_required: true`; the
operator finishes it with `run salvage` or cancels it (`run discard` accepts
only a failed or cancelled run). A direct `node report`
is for a blocked outcome with nothing to merge. The `worktree-merge` skill
covers the closing step; `taskfleet-run-overview` covers reading run state and
what each state calls for.

Inside a worker's worktree, `run show --current` resolves the exact owning run
from durable ownership metadata. A branch name contains a run-id-looking
fragment, but it is not authoritative and may be ambiguous.

## `status` and `lifecycle` are different fields

`status` is progress: `pending` / `running` / `done` / `failed` /
`cancelled`, with the last three terminal. Read it from `run show` (top-level
`data.status` or `data.manifest.status`) to tell whether work is finished.

`lifecycle` is the run's supervision category, fixed at creation:
`autonomous` (the default; the supervisor adjudicates the worker's exit) or
`interactive` (set by `run create --interactive`; the supervisor never
auto-terminalizes and waits for an explicit `run merge` or `run cancel`
because a human owns the lifecycle). It never transitions. An agent that once
polled `lifecycle` for a terminal value hung forever because the field never
matches; that is why this distinction is spelled out.

## Worker profile routing

Profile ownership sits in two layers. User configuration defines executable
profiles under `[profiles.<name>]` in the taskfleet home's `config.toml`,
including harness and argv. Repository configuration may select names but
cannot define executables. Bundled workflow skills forward an explicit profile
selection and otherwise omit `--profile` to use the configured default. Issue briefs describe capability and
risk; they never name a concrete model or command, because the user's config
decides what a name means on this machine.

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

Profile choice is not correctness evidence. Primary sources, source-grounded
scenarios, and deterministic tests are. For a bounded implementation, the
default review ceiling is one focused final-diff review; panels or repeated
reviews need a concrete risk or unresolved trade-off to justify their cost.

## Persistent worker sessions (opt-in)

Native tmux is Taskfleet's implementation, not a required human interface.
Use `--headless` for an unattended parent without an attached terminal.
By default autonomous workers spawn a tmux window in the current session and
the window disappears at teardown; `--headless` or `--tmux-session <name>`
place it in a detached session instead, which matters on macOS where a batch
of more than about four simultaneous spawns can exhaust pseudo-terminals. A
user may instead opt into one named persistent session:

```toml
[tmux]
default_session = "agents"
persistent = true
completed_window_ttl = "24h"
completed_window_max = 20
```

With this, a completed Pi worker's evidence (transcript, pane history, report)
is archived first and the pane is replaced by an inert display whose cwd is the
source repository, so a human can still look at what happened. Expiry of those
displays is done by `taskfleet session maintain`, meant to run from a bounded
noninteractive timer with `HOME`, `TASKFLEET_HOME`, and `PATH` set explicitly.
It acts only on panes taskfleet recorded as its own, never starts a missing
tmux server, and never removes run events, transcripts, reports, or preserved
git work. An explicit `--tmux-session` still wins over the configured default,
and it names a session on the invocation's socket, not a socket.

## Which skill to open next

- **`taskfleet-run-overview`** — reading `run list` / `run show` / `run wait`
  output and deciding whether to wait, salvage, cancel, or discard.
- **`worktree`** — the router for a free-form "do this in a worktree" request;
  it delegates to `worktree-spinoff`, `worktree-research`,
  `worktree-technical-decision`, `worktree-bug-analysis`, or `fan-out`.
- **`worktree-spinoff`** / **`taskfleet-spawn-spinoff`** — spawning one
  autonomous task.
- **`worktree-merge`** — the closing step of any worktree run.
- **`stint-start`** / **`stint-handoff`** — running a bounded work session as
  an orchestrator. The handoff ends by calling a `/wrap-up` skill that is not
  bundled here; without it, the handoff does that work itself.
- **`night-stint-prep`** — preparing the handoff for an unattended overnight
  session before anyone runs `stint-start`.

`taskfleet skill list` shows the bundled catalog and `skill print <name>`
streams any of them. For anything these skills do not cover, the binary's
`--help` is more reliable than a guess.

## Skill and binary versions

This skill was rendered for `taskfleet {{CLI_VERSION}}`. Bundled skills are
the worker's operating manual and are validated in CI against the exact binary
they ship with, so a matching version means the commands here parse. When the
installed binary differs from `{{CLI_VERSION}}` (read `.data.version` from
`taskfleet version --json`), a flag or field described here may not exist, and
the help text of the installed binary outranks this document. Tell the user
about the mismatch; the remedy is upgrading the binary through the channel
they installed it from (`brew upgrade jarimustonen/taskfleet/taskfleet` or the
shell installer) or refreshing the installed skills, and both are the user's
tool maintenance rather than something to do silently inside another task. A
missing binary is the same: say so and let the user install it. Inside the
taskfleet repository itself, never touch the installed binary or skills at
all; maintained `HEAD` and the installed release are allowed to differ.
