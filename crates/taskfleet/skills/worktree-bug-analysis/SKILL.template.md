---
name: worktree-bug-analysis
description: "Spawn an autonomous READ-ONLY spinoff that analyses ONE existing bug and writes findings back into its issue: reproduce/explain, locate the code path with Read/Grep, classify, estimate severity, and sketch fix scope. It self-merges only the issue update and never changes application code. Use before a fix/defer/not-a-bug decision. Fix with issue-driven `/worktree-spinoff`; use `/worktree-research` for open-ended multi-source research."
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# worktree-bug-analysis — read-only, issue-updating

Analyse ONE already-filed bug in an isolated worktree and record what you found
**in the issue itself** — no code changes, no fix, no new issue. This exists
because the two obvious tools don't fit: `worktree-research` refuses
debugging/bug-investigation topics, and `worktree-bugfix` *fixes* code and
creates a new issue. This skill sits between: it takes an **existing bug slug**,
investigates read-only, and updates that issue.

It is a `spinoff`-shaped run — one autonomous agent that self-merges and does not
pause for the user — so it spawns via `taskfleet run create --kind spinoff`
with a read-only brief. It runs **headless** (its only output is an issue update;
nobody watches it), so a triage batch does not clutter the window list. Read
`taskfleet-overview` first; read `worktree-spinoff` for the shared
autonomous-merge contract that this skill reuses verbatim.

## When to use

- ✅ You have an **existing** bug issue slug that needs understanding before a
  fix/defer/not-a-bug decision.
- ❌ Fixing the bug → `/worktree-spinoff <existing-bug-slug>`.
- ❌ Filing a new bug / any issue that does not yet exist — this skill never
  creates issues.
- ❌ Open-ended multi-source research → `/worktree-research`.

## Hard constraints

1. **Never change application code.** The only files this worker writes are the
   bug's own `issues/<slug>/item.md` (and, if useful, `issues/<slug>/analysis.md`).
   Editing anything under the product source tree is a *fix* — out of scope.
2. **Do not decide the disposition.** Classify and recommend; fix-now / defer /
   not-a-bug stays with the user. Do not close the issue or change its status or
   disposition labels.
3. **Autonomous, no user pause.** Like `worktree-spinoff`: investigate, update
   the issue, self-merge. Do NOT run `/wrap-up`.

## Workflow

### 0. Validate context

1. Working directory must be a git repo. Per repo CLAUDE.md, the current branch
   must be clean.
2. `taskfleet version --output json` to confirm `{{CLI_VERSION}}` matches
   the running binary.
3. Capture the current branch as the source/merge target.

### 1. Resolve the bug slug

The remaining argument (after any agent/layout flags, parsed as `worktree-spinoff`
does) is the **bug issue slug**. It must already exist — if `issues/<slug>/item.md`
is missing, abort with a clear error. This skill does not create issues.

### 2. Build the read-only brief

The brief must be self-contained (a spinoff cannot ask follow-ups). It MUST carry
the read-only hard constraints above **and** end with exactly one terminal path:
completed analysis uses `taskfleet run merge`; analysis blocked by a
required failure uses direct `node report` without merging. Include:

1. **Objective** — understand and scope the bug in `issues/<slug>/item.md`; do
   NOT fix it, do NOT change application code; write findings back into the issue.
2. **Steps**:
   - Read `issues/<slug>/item.md` **and every attachment** under
     `issues/<slug>/attachments/` (screenshots are often the whole report).
   - Reproduce or explain the behaviour; locate the responsible code path with
     **Read/Grep only**. If you cannot reproduce, say why.
   - Classify: **real bug** / **expected behaviour** / **cannot tell**. Estimate
     rough severity and who it hits. Sketch what a fix would touch (files/areas)
     — a sketch, not an implementation.
   - Write findings into `issues/<slug>/item.md` under the exact heading
     `## Triage analysis`: verdict, severity, affected area, repro status, fix
     sketch. Keep it tight; for a long trace add `issues/<slug>/analysis.md`
     and link it.
   - Commit with plain `git` and a `Refs-Issue: @<slug>` trailer. This is read-only
     analysis: never use `Fixes-Issue`, `issuectl close --stamp`, or close the issue.
   - If reproducing requires a local taskfleet build, use `cargo build
     --release` and invoke `./target/release/taskfleet …` explicitly.
     During repository work, neither workers nor the orchestrator may create,
     replace, remove, or modify the user's installed taskfleet or bundled
     skills by any mechanism, including any `cargo install`, `cargo uninstall`,
     Homebrew, manual-copy, or `skill install` variant.
3. **Done criteria** — the issue carries the analysis; the branch is committed
   and merged back; no application code changed.
4. **Tool/sub-workflow failure policy** — copy the disclosure contract below
   into the brief. A required failed/incomplete repro or inspection step cannot
   be claimed complete; optional failure may continue only when safe and
   disclosed.

Long brief → temp file + `--prompt-file <path>` (`mktemp -t bug-analysis-XXXXXX.md`).

### 3. Create the run

```
taskfleet run create \
  --kind spinoff \
  --headless \
  --title "bug-analysis-<slug>" \
  --prompt-file <brief-file> \
  [--source-branch <branch>]
```

- `--kind spinoff` + `--title` + `--prompt-file` (or `--task`) required — same
  flag rules as `worktree-spinoff`.
- `--headless` is the default for this skill: the run's only deliverable is an
  issue update, so its window belongs in the detached `headless` session, not the
  user's window list. Auto-cleanup still closes it on terminal. Drop `--headless`
  only if the user explicitly wants to watch the analysis live.
- `--source-branch` defaults to the current branch from step 0.

### 4. Success envelope

Same shape as `worktree-spinoff` (`run_id`, `supervisor`, `kind: spinoff`,
`node_id`, `tmux_window`, `worktree_path`, `branch`). Read `data.run_id`; if
`data.supervisor` is `null` or `{"note": "..."}`, surface it and stop.

### 5. Report to the caller

- Run id, branch, and the issue path `issues/<slug>/item.md`.
- That the worker self-merges the issue update via `taskfleet run merge` — no
  `/worktree-merge` handoff.
- Follow progress with `taskfleet run show <run-id>` or
  `taskfleet run wait <run-id>`; the headless window is reachable with
  `tmux attach -t headless` if the user wants to watch.

When spawned by another skill rather than invoked directly by the user, return the
structured payload (run id, node id, branch) to the caller instead of a human summary.

## Terminal report (mandatory)

A bug-analysis worker MUST take exactly one terminal path, never both.
Completed, mergeable analysis uses `taskfleet run merge`, which merges and
submits the report in one call. Analysis blocked by a required failed or
incomplete step does **not** merge; it submits a direct `success: false` report
under "Tool and sub-workflow failure disclosure" below. Omitting both paths
leaves the run pending. For the completed path, run the following once the issue
update is committed:

1. **Resolve the exact owning run id** from inside the worktree. Use the
   durable node ownership record, never the branch's display identifier (it is a
   lossy bounded fragment that can repeat, not ownership):

   ```bash
   run_id="$(taskfleet run show --current --output json | jq -er '.data.run_id')" || {
     echo "failed to resolve exact owning run id" >&2
     exit 1
   }
   ```

   This fails closed on missing, duplicate, stale, or malformed ownership
   evidence. If it fails, stop and report the error; do not guess a run id.

2. **Write the §7.3 payload** to a temp file — `success: true`, `summary` = the
   one-line verdict (e.g. "real bug, medium sev, in <area>"), the three arrays
   usually empty (a fix worth doing goes in `spinoff_proposals[]` with
   `proposed_kind: "bugfix"`):

   ```bash
   cat > /tmp/node-report-${run_id}.json <<'JSON'
   { "success": true, "summary": "<verdict>", "discussion_items": [], "spinoff_proposals": [], "wrap_up_recommendations": [] }
   JSON
   ```

3. **Merge and report in one call:**

   ```bash
   taskfleet run merge "$run_id" --report-file /tmp/node-report-${run_id}.json
   ```

   On a clean merge the supervisor consumes the report and tears down the
   worktree/window/branch — do not run any manual tmux/git cleanup. On a merge
   conflict it exits non-zero with `error.code: "merge_failed"` and submits no
   report; resolve (or `/complex-rebase`) and re-run the same call. **Do not**
   close the issue or change its status or disposition labels — that decision is the
   user's.

## Tool and sub-workflow failure disclosure

Before closing, inventory every failed or detectably incomplete tool, command,
external service, review, panel, or delegated workflow.

A step **required** by the brief or done criteria that remains failed or
incomplete always blocks this attempt. Do not call `run merge`. Write the
existing §7.3 report payload to `/tmp/node-report-${run_id}.json` with top-level
`success: false`, then submit it with `taskfleet node report "$run_id"
n-0001 --from-file /tmp/node-report-${run_id}.json` (`n-0001` is the sole node
in this single-worker run). An **optional/advisory** failure may continue only
when the issue analysis is independently complete and safe; disclose it in the
full `success: true` report passed to `taskfleet run merge "$run_id"
--report-file /tmp/node-report-${run_id}.json`, never the minimal auto-report.

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

## Non-goals

- Does NOT fix bugs or touch application code — use `/worktree-spinoff` for the fix.
- Does NOT create issues — it only updates an existing one.
- Does NOT decide fix/defer/not-a-bug, close the issue, or change its status or
  disposition labels.
- Does NOT run open-ended multi-source research — that's `/worktree-research`.

## Install or upgrade `taskfleet`

This skill was installed for `taskfleet {{CLI_VERSION}}`. On the first
invocation in a session, run `taskfleet version --output json`, compare
`.data.version` to `{{CLI_VERSION}}`: **Missing** → tell the user to install
through a published distribution channel outside this repository workflow and
stop; **Older** → tell the user to upgrade and stop; **Newer** → tell
the user the installed skill is stale and stop (refreshing bundled instructions
is published-tool maintenance outside repository work; never run `skill install`
as part of this workflow); **Equal** → proceed.

## Example

```
/worktree-bug-analysis checkout-total-off-by-one
```
