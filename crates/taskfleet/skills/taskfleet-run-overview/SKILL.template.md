---
name: taskfleet-run-overview
description: Read the output of `taskfleet run list` and `taskfleet run show` to inspect the state of orchestrated agent workflows (worktrees, fan-outs, spinoffs). Use when asked about run status, when triaging an in-flight orchestration, or before deciding whether to spawn, resume, or abort work.
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# taskfleet-run-overview

`taskfleet` is the state owner for agent workflows: worktrees,
fan-outs, orchestrations, and spinoffs. Every workflow is a **run** with
canonical state under `~/.taskfleet/runs/<run-id>/`. These commands
expose that state:

- `taskfleet run list` — every run, newest first; add `--repo <path>` (including
  `--repo .` from a subdirectory or linked worktree) to select runs by their
  recorded git repository identity
- `taskfleet run show <run-id>` — one run, full detail (one-shot)
- `taskfleet run wait <run-id> …` — the blocking counterpart to
  `run show`: poll one or more runs with sane backoff until they reach a
  terminal state (`done | failed | cancelled`), then emit a structured
  summary. Use this instead of hand-rolling a `while … run show … case`
  loop (`--any` returns on the first terminal run; `--timeout <dur>` and
  `--fail-on-error` shape the exit code).

Pass `--output json` for one structured JSON envelope, or `--output jsonl`
for a line-oriented stream. Use `--output text` only when a human is reading
the terminal.

## Envelope

Every success looks like:

```json
{
  "schema_version": 1,
  "data": { ... },
  "warnings": ["..."]
}
```

`schema_version` is the envelope version. If you see a number you do not
recognise, refuse to proceed and report the mismatch — the state shape
may have changed under you. `warnings` is optional; surface it to the
user when present.

## `run list` payload

`data.runs` is an array of summary objects. Sort order is newest-first
by `created_at` (RFC3339).

```json
{
  "data": {
    "runs": [
      {
        "run_id": "01HZ...",
        "kind": "spinoff | research | technical-decision | fan-out",
        "lifecycle": "autonomous | interactive",
        "status": "pending | running | done | failed | cancelled",
        "source_repo": "/path/recorded-at-create | null",
        "created_at": "2026-06-12T10:30:00Z",
        "updated_at": "2026-06-12T10:45:12Z"
      }
    ]
  }
}
```

Fields that drive decisions:

- `kind` — the run's **topology**; picks the right follow-up command (a
  `fan-out` resumes differently from a single `spinoff`).
- `lifecycle` — the run's **how-run category**, set explicitly at
  `run create` from the `--interactive` flag (NOT derived from `kind`).
  `autonomous` (the default — fire-and-forget; the supervisor adjudicates
  exit and tears down) or `interactive` (human-driven — the supervisor
  never auto-terminalizes; it waits for an explicit `run merge` /
  `run cancel`, so an interactive run can sit non-terminal indefinitely by
  design). It is NOT a progress state and never transitions. Read it to
  know *how* a run is driven; read `status` for whether it is *done*.
- `status` — the **terminal-progress field**. Values are `pending`,
  `running`, `done`, `failed`, `cancelled`. **Terminal states are
  `done | failed | cancelled`** — once any of those is set the run is
  settled (the reducer freezes further status changes). Branch on this
  to detect completion.
- `source_repo` — the repository path recorded at creation, or `null` for a
  legacy/skeleton run where identity was not recorded. `run list --repo <path>`
  compares actual git common-dir identity, so main and linked worktrees match
  while an independent nested repository does not.

## `run show` payload

`run show`'s `data` carries the **same flat row a `run list` row does**
at the top level (`run_id`, `kind`, `status`, `title`, `created_at`,
`node_count`, `supervisor`, `stalled`) — so you can address these the
same way across both verbs. `data.manifest` then extends that row with
full detail (`lifecycle`, `updated_at`, `source_*`, `parent_*`,
`open_discussions`, `pending_spinoffs`); `data.counts` carries
denormalised counters; `data.supervisor` is the probed supervisor
liveness; `landed`/`landed_method`/`recoverable_work`/`false_failed` are
`run show`-only computed detail. `report` is the default worker's terminal
report for a **single-worker** run and is `null` before that worker reports or
when the run has multiple nodes. Some kinds add kind-specific fields.

`data.false_failed` (present only when set) flags a **suspected
false-failed run**: the run is `failed` yet git confirms the worker's
content is already in source (`landed: true`, `landed_method:
"git-verified"`) with no `run merge` on record — the raw-git
self-merge-then-death case. It is a **non-mutating hint, never an
auto-success**: the run stays `failed`. Its `resume_hint` steers you to
`taskfleet run salvage <id>`, which records the skipped merge
through the real `run merge` machinery (idempotent against the
already-integrated content) and terminalizes the run to `done` honestly.
Do NOT treat a `false_failed` run as done — run salvage first. Never
finish a run with a raw `git merge`; always use `run merge`/`run
salvage`.

`data.supervisor.state` is the field to branch on — it distinguishes the
conditions the legacy `alive` boolean collapses: `alive` (running),
`dead` (started then died / recycled — orphaned, recover with `run
reattach`), `not-recorded` (never launched or cleanly torn down),
`unreadable` (pid file present but can't be parsed — investigate), and
`unknown` (not probed; you won't see it on `run show`/`run list`, which
always probe). `data.supervisor.alive` is retained for back-compat and
equals `state == "alive"` — prefer `state`, since only it tells
"orphaned" from "finished" from "I/O error".

```json
{
  "data": {
    "run_id": "01HZ...",
    "kind": "fan-out",
    "status": "running",
    "title": "...",
    "created_at": "...",
    "node_count": 10,
    "supervisor": { "pid": 65745, "state": "alive", "alive": true },
    "stalled": false,
    "manifest": {
      "schema_version": 1,
      "run_id": "01HZ...",
      "kind": "fan-out",
      "lifecycle": "autonomous",
      "title": "...",
      "status": "running",
      "created_at": "...",
      "updated_at": "...",
      "node_count": 10,
      "open_discussions": 0,
      "pending_spinoffs": 0
    },
    "counts": { "nodes": 10 },
    "report": null
  }
}
```

Both `data.status` (flat) and `data.manifest.status` resolve to the same
value; the flat path matches `run list`, the nested one is kept for
back-compat.

## Reading a worker report back

A terminal worker report is persisted on the node projection as
`last_report`. The read surface avoids needing that projection detail:
`run show` exposes the default worker's report at `data.report` for a
single-worker run. For a multi-node run it is `null`, so inspect each worker
with `node show`, which keeps `data.last_report` and also exposes an identical
`data.report` alias.

```bash
# skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
taskfleet run show "$run_id" --output json | jq '.data.report'
# Node-level projection-compatible probe:
# skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
taskfleet node show "$run_id" n-0001 --output json |
  jq '.data.report // .data.last_report'
```

Do not apply `run show` paths to `run wait`: waiting can cover several run
ids, so its outcomes live in `data.runs[]`. Read `data.outcome` first:
`condition-met` means the requested `all`/`any` condition was met, while
`timed-out` means the timeout elapsed first. This is authoritative even when a
pipeline loses the process exit code. A valid wait probe is:

```bash
taskfleet run wait "$run_id" --output json |
  jq '.data | {outcome, runs: [.runs[] | {run_id, status, summary}]}'
```

Thus `.data.status` is intentionally null on a `run wait` response. `run wait`
folds in a summary; use `run show` or `node show` to read the full
`discussion_items`, `spinoff_proposals`, and `wrap_up_recommendations` arrays.

## Decision rules

1. **Triaging "is this still going?"** — read `data.manifest.status`.
   Terminal values (`done | failed | cancelled`) mean the run is settled
   and the supervisor has either already torn it down or is about to.
   Anything else (`pending | running`) is still live.
2. **Waiting for completion** — use `taskfleet run wait <id> --output json`
   and branch first on `.data.outcome`; never treat `timed-out` as completion.
   Then inspect `.data.runs[]`. Do NOT poll `lifecycle` — it is the category,
   not a progress field, and never transitions.
3. **Deciding whether to spawn more work** — list runs first. If a
   `fan-out` is already `running` on the same scope, do not start a
   second; resume or wait.
4. **Surfacing problems to the user** — when `status == "failed"`, the
   event log has the cause; quote it instead of guessing.
5. **Schema drift** — if `schema_version` does not match what this skill
   describes, stop and tell the user the skill is out of date with the
   installed binary.
6. **Unblocking ONE stuck fan-out child** — cancel a whole run with
   `taskfleet run cancel <run-id>`; cancel a single live node with
   `taskfleet run cancel <run-id> --node <node-id>`. The per-node form
   is branch-preserving (source-relative teardown — a child's committed
   work is never force-deleted) and does NOT terminalize the run while
   other nodes are still live: the supervisor rolls the run up
   (`done | failed | cancelled`) only once every node has settled. Both
   forms are idempotent — a duplicate cancel of an already-terminal
   node/run reports it settled rather than erroring.

## Errors

Failures print a JSON envelope to **stderr** with non-zero exit:

```json
{"schema_version": 1, "error": {"code": "<code>", "message": "..."}}
```

Common codes: `run_not_found`, `state_unreadable`, `schema_mismatch`.
Always read the `code`; the `message` is human prose and may change.

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
  re-run the shell installer). Stop and wait — `run list` / `run show` payload shape
  may have changed.
- **Newer than `{{CLI_VERSION}}`**: tell the user the installed skill is
  stale and stop. Refreshing installed bundled instructions is published-tool
  maintenance outside repository work; never run `skill install` as part of
  this workflow.
- **Equal**: proceed normally.
