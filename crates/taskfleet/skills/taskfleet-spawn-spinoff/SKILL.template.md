---
name: taskfleet-spawn-spinoff
description: Spawn an autonomous spinoff worktree via taskfleet — a single fire-and-forget agent that takes a focused task, executes it in its own git worktree, and merges itself back. Use when the user wants a parallel sub-task handled without interactive review.
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# taskfleet-spawn-spinoff

A **spinoff** is one autonomous agent run in its own git worktree, doing
one well-scoped task, and merging itself back to the source branch when
done. No interactive review. The canonical way to launch one is via
`taskfleet`, not by hand-crafting branches.

## Invocation

```
taskfleet run create \
  --kind spinoff \
  --title "<2–4 word slug>" \
  --task "<the task in one paragraph>" \
  --source-branch <branch>
```

(The default is `--output jsonl` — one compact envelope per line on
stdout. Add `--output text` for a human-readable summary or `--output
json` for pretty-printed JSON.)

- `--kind spinoff` is required; it picks the autonomous worker recipe.
- `--title` is required; a short slug that names the run, the branch
  (`wt/<short>-<title>`), and the tmux window.
- `--task` is the *entire* brief the spinoff agent will see. It must
  be self-contained — the spinoff does not share your conversation
  history. State the goal, the constraints, and what "done" looks like.
  For a long brief, write it to a file and pass `--prompt-file <path>`
  instead of inlining via `--task`.
- `--source-branch` is the branch the worktree forks from, and the
  branch the spinoff merges itself back into when done. Default is the
  current branch when omitted.

## Success envelope

```json
{
  "schema_version": 1,
  "data": {
    "run": {
      "id": "01HZ...",
      "kind": "spinoff",
      "lifecycle": "autonomous",
      "status": "pending",
      "source_branch": "main",
      "target_branch": "main"
    }
  }
}
```

`lifecycle` is the run's category (`autonomous` for spinoffs) and never
changes. `status` is the progress field: starts at `pending`, transitions
to `running` once the worker picks it up, and reaches a terminal value
(`done | failed | cancelled`) when the run settles. Branch on `status`
to detect completion. Use `taskfleet run show <id>` to follow
progress (see the `taskfleet-run-overview` skill for the response shape).

## When to use a spinoff vs. other variants

- **Spinoff** — one focused autonomous task with proportionate review chosen after the
  final diff. "Update all docstrings in module X." "Refactor helper Y into its own crate."
- **Interactive worktree** — user wants a hands-on, human-driven session:
  add `--interactive` to `run create` so the supervisor waits for an
  explicit `run merge`/`run cancel` (a default spinoff is always headless +
  autonomous). Not this skill's default path.
- **Fan-out** — N similar independent units (≥5). Not this skill — use
  `run create --kind fan-out`.

## Writing the prompt

The prompt is the most failure-prone input. The spinoff cannot ask you
follow-up questions. Include:

1. **Goal** — one sentence on what to deliver.
2. **Context** — files, modules, or constraints that matter. Quote
   paths.
3. **Done criteria** — concrete, verifiable. "All tests pass" or "no
   `clippy::pedantic` warnings introduced."
4. **Quality bar** — unless explicitly mandated, the worker chooses and explains review
   depth after the final diff. Focused review suits small, local, well-covered changes;
   security/privacy, destructive, concurrency-sensitive, architectural, broad,
   hard-to-rollback, or weakly tested changes need stronger independent review. Reuse
   adequate existing evidence unless the risk surface changed.
5. **Failure and closing contract** — copy the disclosure contract below into
   the brief. The brief ends with exactly one terminal path: completed work
   reports through `run merge`; work blocked by a required failure reports
   directly with `success: false` and does not merge.

If any of those are missing, ask the user before spawning. A spinoff
that misinterprets the task wastes a worktree and a merge cycle.

## Decision forks

An autonomous worker never blocks on an interactive prompt. At a genuine fork,
it records `node.awaiting_input` with a non-empty `discussion_items` array whose
items carry `topic`, string `options`, and `recommended_default`. It then either
resolves the marker with `node.input_resolved` and follows that default after a
bounded five-minute wait, or submits a terminal `success:false` blocked report
with the same discussion items. The signal is visible immediately and propagates
to `run wait` / the registered notify hook after the grace window.

## Terminal report (mandatory)

The worker MUST take exactly one terminal path, never both. Completed,
mergeable work writes the existing §7.3 report payload with top-level `success:
true`, then runs `taskfleet run merge "$run_id" --report-file
/tmp/node-report-${run_id}.json`. Work blocked by a required failed or incomplete
step takes the direct-report path below and does not merge. Omitting both paths
leaves the run unterminated.

## Tool and sub-workflow failure disclosure

Before closing, inventory every failed or detectably incomplete tool, command,
external service, review, panel, or delegated workflow.

A step **required** by the brief or done criteria that remains failed or
incomplete always blocks this attempt. Do not call `run merge`. Write the
existing §7.3 report payload to `/tmp/node-report-${run_id}.json` with top-level
`success: false`, then submit it with `taskfleet node report "$run_id"
n-0001 --from-file /tmp/node-report-${run_id}.json` (`n-0001` is the sole node
in this single-worker run). An **optional/advisory** failure may continue only
when the deliverable is independently complete and safe; disclose it in the
full `success: true` report passed to `taskfleet run merge "$run_id"
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

## Errors

Standard error envelope on stderr, non-zero exit. Likely codes:

- `invalid_argument` — bad branch name, missing `--task`/`--prompt-file`
- `branch_not_found` — `--source-branch` does not exist locally
- `worktree_create_failed` — git refused (uncommitted changes,
  conflicting worktree)

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
