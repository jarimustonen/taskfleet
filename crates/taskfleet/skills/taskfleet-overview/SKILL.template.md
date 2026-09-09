---
name: taskfleet-overview
description: First read for any agent that has just discovered the `taskfleet` binary mid-conversation. Teaches the overall shape of the tool — runs, supervisors, nodes, discussions, spinoffs — and the canonical create→supervise→collect-reports cycle. Use when asked "what is taskfleet", when the binary appears in a session for the first time, or before issuing the first non-trivial command.
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# taskfleet-overview

`taskfleet` (binary name: `taskfleet`) is the state owner for
AI-agent workflows: worktrees, fan-outs, orchestrations, and spinoffs.
Every workflow is a **run** with canonical state under
`~/.taskfleet/runs/<run-id>/`. Read this skill once at the start of
any session that touches the tool — every other `taskfleet-*` skill assumes
the vocabulary and conventions defined here.

## Output contract (read this first)

Every machine-readable command emits the canonical envelope:

```json
{"schema_version": 1, "data": {...}, "warnings": ["..."]}
```

- The default `--output` is `jsonl` — one compact envelope per line on
  stdout. AI agents parse it directly with `serde_json::from_str` per
  line.
- `--output json` returns a single pretty-printed document; use it for
  one-shot inspection.
- `--output text` is the human summary; do not parse it.
- Errors print a separate envelope on **stderr** with non-zero exit:
  `{"schema_version": 1, "error": {"code": "<snake_case>", "message": "..."}}`.
  Always branch on `error.code`; the message is human prose.

If `schema_version` is a value you do not recognise, refuse to proceed
— the data shape may have changed under you.

## The verbs you will use most

- `taskfleet version` — binary version, commit, schema versions,
  and the bundled skill catalog (see §17 of the design doc). Run this
  first when you discover the binary; it tells you whether the skill
  you loaded matches the binary on disk.
- `taskfleet skill list` / `skill print <name>` / `skill install <name>`
  — discover, stream, and persist the operating manual for each workflow.
  `skill list --json` declares the supported runtimes and layouts. Install
  defaults to all three (`claude`, `pi`, and `codex`); select one with
  `--agent`, preserve native layouts under another base with `--target`, and
  preview the complete no-write plan with `--dry-run`. Existing files are
  never replaced unless you pass explicit `--force`.
- `taskfleet run create --kind <kind> --title "..." --task "..."`
  — start a new run (kinds: `spinoff`, `research`, `technical-decision`,
  `fan-out`). The brief is `--task "<inline>"` or, for a long brief,
  `--prompt-file <path>` — there is no `--prompt` flag. See the
  `taskfleet-spawn-spinoff` skill for the spinoff specifics.
- `taskfleet run list` / `run show <id>` — inspect runs. See the
  `taskfleet-run-overview` skill for payload shapes.
- `taskfleet run merge <run-id> [--source <branch>] [--report-file <path>]`
  — the closing step for a worktree run: rebase + merge the branch back
  to its source, submit the terminal `node report` (stamped
  `via: "explicit-merge"`), and let the supervisor tear the worktree down.
  One call replaces the old `/worktree-merge` + `node report` pair. See
  the `worktree-merge` skill.
- `taskfleet supervise <run-id>` — the long-lived per-run
  supervisor process; `run reattach` invokes it. Most agents do not call
  this directly.
- `taskfleet session maintain --output json` — bounded, idempotent timer
  maintenance for opt-in persistent worker sessions. It uses only each run's
  recorded socket/server/window/pane ownership, never `$TMUX` or cwd, and does
  not start a missing tmux server.
- `taskfleet node list` / `node show <id>` / `node report` —
  per-unit detail inside a run, and the structured terminal report a
  worker submits to end its run. A worktree worker usually submits this
  report *as part of* `run merge` (its closing step) rather than calling
  `node report` directly; a direct `node report` is for a blocked
  outcome with nothing to merge. Node ids have the form
  `n-` followed by 4–10 ASCII digits (e.g. `n-0001`, the first node of
  every run); the binary rejects any other shape. Never invent a slug
  like `n-driver-001` — discover the id from `run create`'s `node_id`
  field or from `node list`.

## Canonical cycle: create → supervise → collect reports

The flow every workflow follows:

1. **Create**: `taskfleet run create --kind <kind> --title
   "<title>" --task "<the brief>"` returns a `data` payload with
   `run_id`, `kind`, and `lifecycle` (the kind's classification —
   always `autonomous` for the surviving kinds, not a progress value).
   The run's progress starts at `status: pending`, read later via `run show`.
2. **Supervise**: a background supervisor process picks the run up and
   drives the worker agent(s). State transitions land in the run's
   event log; `run show <id>` reads them.
3. **Collect**: when the worker finishes, it submits a structured
   terminal report describing the outcome (success, failures, spin-off
   proposals, discussion items, wrap-up recommendations) — usually as
   part of its `run merge` closing step, which merges the branch and
   submits the report in one call. The orchestrator reads these via
   `node show`.

Two distinct fields, two distinct meanings — do not conflate them:

- **`lifecycle`** carries the run's *classification*, derived from its
  kind and fixed for the run's whole life. Every surviving kind (spinoff,
  fan-out, research, technical-decision) is `autonomous` — it runs to
  completion unattended. It is **not** a progress signal: a brand-new run
  already reads `lifecycle: autonomous`.
- **`status`** carries the run's *progress* and is the field to read
  when deciding whether a run is finished: `pending` / `running` /
  `blocked` / `done` / `failed` / `cancelled`. `done`, `failed`, and
  `cancelled` are terminal. Read `status` (via `run show <id>` →
  `data.manifest.status`), never `lifecycle`, to tell whether work is
  complete.

## Persistent autonomous-worker session (opt-in)

User config may select one named session and bounded inert completed displays:

```toml
[tmux]
default_session = "agents"
persistent = true
completed_window_ttl = "24h"
completed_window_max = 20
```

With no `[tmux]` section behavior is unchanged. `--tmux-session NAME` wins over
that default and selects a **session on the invocation's actual tmux socket**,
not a socket; `--headless` still explicitly selects `headless`. Explicit
interactive runs stay in the current session unless the caller names one.
Completed Pi evidence is archived before the worker pane is stopped and replaced
by a dead, inert display whose cwd is the surviving source repository. Schedule
`taskfleet session maintain --output json` (for example every five minutes)
from a bounded noninteractive timer with `HOME`, `TASKFLEET_HOME`, and `PATH`
set explicitly. The strictest recorded count limit wins within one exact
server/session generation. Expiry removes only the owned pane; a final inert
pane remains as the persistent session anchor. Pane expiry never removes run
events, transcripts, reports, or preserved Git work.

## When to use which skill

- **`taskfleet-overview` (this one)** — sanity-check vocabulary,
  confirm the canonical cycle, locate the right specific skill.
- **`taskfleet-run-overview`** — when you need to read `run list` / `run show`
  output and reason about a run's state.
- **`taskfleet-spawn-spinoff`** — when the user wants to spawn one focused
  autonomous task without interactive review.

If the user asks for behavior these skills do not cover (fan-out,
research, technical-decision), call `--help` first and report back rather
than guessing.

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
  re-run the shell installer). Stop and wait — schema / CLI surface may have changed.
- **Newer than `{{CLI_VERSION}}`**: tell the user the installed skill is
  stale and stop. Refreshing installed bundled instructions is published-tool
  maintenance outside repository work; never run `skill install` as part of
  this workflow.
- **Equal**: proceed normally.
