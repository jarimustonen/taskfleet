---
name: worktree-research
description: Spawn an autonomous research worktree via `taskfleet run create --kind research` — multi-source background investigation that reads sources, gathers divergent perspectives, and writes a sourced markdown report committed to the repo, then merges itself back. Use when the user asks to research, investigate, survey, look into, or compare options for a topic that needs reading multiple sources and synthesizing into a sourced markdown report. Do NOT use for quick factual lookups (one `WebSearch`), single-doc summaries, debugging, code changes, or forward decisions (`/worktree-technical-decision`).
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# worktree-research

A **research worktree** is one autonomous agent whose deliverable is a
**sourced markdown report**, not code. It reads multiple sources, weighs
divergent perspectives, writes the report into the repo (typically
`research/<slug>.md`), commits, and merges itself back to the source
branch — same self-merge contract as `worktree-spinoff` but with a
prose-output recipe and `WebSearch`/`WebFetch` enabled.

Read `taskfleet-overview` first; read `worktree-spinoff` for the
shared autonomous-merge contract.

## When to use

- ✅ "Research the state of X", "investigate options for Y", "survey
  approaches to Z", "compare A vs B vs C in depth".
- ✅ The deliverable is a written report the user will read and cite
  later.
- ❌ One-shot fact lookup (just `WebSearch`).
- ❌ Single-doc / single-file summary (read it inline).
- ❌ Debugging or code changes → `/worktree-bug-analysis` or `/worktree-spinoff`.
- ❌ "Which option should we pick?" — that is a decision, use
  `/worktree-technical-decision` (it records an ADR).

## Workflow

### 0. Validate context

1. Working directory must be a git repo. Per repo CLAUDE.md, the
   current branch must be clean.
2. `taskfleet version --output json` to confirm
   `{{CLI_VERSION}}` matches the running binary.
3. Capture the current branch as the source/merge target.

### 1. Sharpen the question

Research outputs are only as good as their question. Before spawning,
distill the user's request to:

- **Core question** — one sentence.
- **Scope** — what counts as in-bounds (timeframe, ecosystems,
  platforms, audience).
- **Out-of-scope** — explicit exclusions; prevents the agent from
  wandering.
- **Audience + tone** — engineer-facing? executive-facing? Finnish?
- **Output location** — default `research/<slug>.md`; override if the
  user has a different repo convention.

If any of the above is genuinely missing from the user's prompt, ask
**once** before spawning. A misdirected research run wastes hours.

### 2. Build the prompt

Include in the brief:

1. The four-part framing from step 1 (question / scope / exclusions /
   audience).
2. **Source-quality bar** — prefer primary sources (RFCs, vendor
   docs, original papers) over aggregators; cite every non-trivial
   claim with a URL.
3. **Divergent perspectives** — explicitly require N≥2 distinct
   viewpoints when the topic admits them; do not present a single
   narrative as consensus.
4. **Report structure** — TL;DR (3–5 bullets) → Background → Options /
   Findings → Trade-offs → Citations. Keep claims and citations
   inline.
5. **Done criteria** — file at `research/<slug>.md` exists, committed,
   merged back to source branch.
6. **Repository-local tool safety** — if repository inspection requires
   building taskfleet, use `cargo build --release` and invoke
   `./target/release/taskfleet …` explicitly. During repository work,
   neither workers nor the orchestrator may create, replace, remove, or modify
   the user's installed taskfleet or bundled skills by any mechanism,
   including any `cargo install`, `cargo uninstall`, Homebrew, manual-copy, or
   `skill install` variant.
7. **Tool/sub-workflow failure policy** — copy the disclosure contract below
   into the brief. Required source/tool failure cannot be claimed complete;
   optional failure may continue only when independently safe and disclosed.

Long prompts → temp file + `--prompt-file <path>`.

### 3. Create the run

```
taskfleet run create \
  --kind research \
  --title "<2–4 word slug>" \
  --task "<self-contained research brief>" \
  [--source-branch <branch>] \
  [--idempotency-key <key>]
```

Same flag rules as `worktree-spinoff` — `--kind research`,
`--title`, and `--task`/`--prompt-file` required; `--source-branch`
defaults to the current branch. Output defaults to `--output jsonl`.

**`--harness pi` is supported for research.** A pi worker is
AGENTS.md-native and has none of Claude's Skill/Agent tools or
`/worktree-*` slash commands, so when the resolved harness is `pi` the
CLI auto-prepends a short translation preamble to the worker's prompt
(mapping the `/worktree-merge` close to the plain `taskfleet run
merge` bash, telling it to skip `/llm-review` and sub-agents). You do
not need to hand-translate the brief — write it as usual. This is the
only autonomous kind translated for pi so far; other kinds still assume
a Claude worker.

### 4. Success envelope

```json
{
  "schema_version": 1,
  "data": {
    "run_id": "01HZ...",
    "supervisor": 12345,
    "kind": "research",
    "lifecycle": "autonomous",
    "node_id": "n-...",
    "tmux_window": "<workmux-reported-window-name>",
    "worktree_path": "$HOME/repos/<repo>/worktrees/<title>",
    "branch": "wt/<title>"
  }
}
```

Read `data.run_id` and `data.supervisor`. If supervisor is `null` or
`{"note": "..."}`, surface and stop.

### 5. Report to the caller

Tell the user:

- Run id, branch, tmux window.
- Expected output path: `research/<slug>.md` (or the override they
  specified).
- That the research worktree merges-and-reports itself via
  `taskfleet run merge` — no `/worktree-merge` handoff.
- How to follow progress: `taskfleet run show <run-id>` for a
  one-shot snapshot, `taskfleet event tail <run-id> --follow` for
  the streaming log, or `taskfleet run wait <run-id>` to block until
  the run is terminal (`done | failed | cancelled`) — no hand-rolled poll
  loop, no wrong-field footgun.

## Terminal report (mandatory)

A research worker MUST take exactly one terminal path, never both. Completed,
mergeable research uses `taskfleet run merge`, which merges and submits the
terminal report stamped `via: "explicit-merge"`. Research blocked by a required
failed or incomplete step does **not** merge; it submits a direct `success:
false` report under "Tool and sub-workflow failure disclosure" below. Omitting
both paths leaves the run alive. The completed path passes a `--report-file`
carrying the full §7.3 payload (validated **before** the merge).

1. **Resolve the exact owning run id** from inside the worktree. Use the
   durable node ownership record, never the branch's display identifier (it is a
   lossy bounded fragment that can repeat, not ownership). The node id defaults
   to `n-0001`:

   ```bash
   run_id="$(taskfleet run show --current --output json | jq -er '.data.run_id')" || {
     echo "failed to resolve exact owning run id" >&2
     exit 1
   }
   ```

   This fails closed on missing, duplicate, stale, or malformed ownership
   evidence. If it fails, stop and report the error; do not guess a run id.

2. **Write the §7.3 payload** to a temp file. These exact field names are
   what the supervisor consumes — do NOT use `discuss`,
   `spinoff_candidates`, or `wrap_up`: an unknown key still passes
   validation, but its contents are silently dropped.

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

   - `success` — **required** boolean. `true` when the work merged
     cleanly; `false` when reporting a blocked or failed outcome.
   - `summary` — optional one-line human-readable result.
   - `discussion_items[]` — decisions that genuinely needed a human
     call. Each: `{"topic": "<non-empty>", "severity":
     "discuss|critical|info", "options": ["…"]}`.
   - `spinoff_proposals[]` — follow-up work worth spawning. Each:
     `{"proposed_title": "<non-empty>", "proposed_kind":
     "spinoff|code|research|bugfix|technical-decision|make-skill|fan-out|orchestrated",
     "rationale": "<why>"}`.
   - `wrap_up_recommendations[]` — array of strings; advice for the
     caller (further reviews, doc updates, additional siblings).

   Even a clean, no-follow-up run submits `{"success": true}` with the
   arrays empty — the call itself is what releases the supervisor.

3. **Merge and report in one call:**

   ```bash
   taskfleet run merge "$run_id" --report-file /tmp/node-report-${run_id}.json
   ```

   This rebases + merges the worktree branch into its source branch and
   submits the §7.3 report in the same call. On a clean merge the per-run
   supervisor consumes the report, winds the run down, and tears down the
   worktree, tmux window, and branch automatically — do **not** manually
   run any tmux/git cleanup. If the source branch is not the run's
   recorded `source_branch`, pass `--source <branch>`.

   On a merge conflict the call exits non-zero with `error.code:
   "merge_failed"` and submits **no** report — the node stays live.
   Resolve the conflict (or run `/complex-rebase`) and re-run the same
   `run merge` call.

A terminal report is **not optional**. Completed work with no `run merge`, or
blocked work with no direct `node report`, leaves the run dangling with no
structured outcome for the caller to read.

## Tool and sub-workflow failure disclosure

Before closing, inventory every failed or detectably incomplete tool, command,
external service, review, panel, or delegated workflow.

A step **required** by the brief or done criteria that remains failed or
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

## Issue Management

Skip when driver-spawned. For standalone issue-driven research, distinguish two cases:

- If research is evidence for a larger issue, commit the report with
  `Refs-Issue: @<slug>` and do not close or stamp the issue.
- If the issue's whole deliverable is this report, make and validate the final report
  commit, then run `issuectl close <slug> --status done --stamp --as <agent> --json`.
  Require `.data.stamp.status` to be `stamped` or `already_present`; otherwise do not
  merge. Commit the exact closure metadata path separately and require a clean tree before
  `taskfleet run merge`. The stamped report commit carries `Fixes-Issue: @<slug>`.

Freeform research has no issue trailer or issue mutation.

## Errors

Same envelope and codes as `worktree-spinoff` (`invalid_arguments`,
`branch_not_found`, `worktree_create_failed`, `idempotent_replay`,
`supervisor_spawn_failed`). One research-specific gotcha: if
`WebSearch`/`WebFetch` is disabled in the worktree's tool allowlist,
the run will be useless — the project's CLAUDE.md / `.workmux.yaml`
must expose those tools to `--kind research`. The CLI does not
currently validate this; surface it to the user if the research output
comes back with "could not fetch sources" notes.

## Install or upgrade `taskfleet`

This skill was installed for `taskfleet {{CLI_VERSION}}`. On the
first invocation in a session, run
`taskfleet version --output json`, compare `.data.version` to
`{{CLI_VERSION}}`, and:

- **Missing**: tell the user to install through a published distribution channel
  outside this repository workflow, then stop.
- **Older**: tell the user to upgrade and stop.
- **Newer**: tell the user the installed skill is stale and stop. Refreshing
  installed bundled instructions is published-tool maintenance outside
  repository work; never run `skill install` as part of this workflow.
- **Equal**: proceed.

## Example

```
/worktree-research Compare WAL implementations in SQLite, DuckDB, and Postgres for write-heavy embedded use
```
