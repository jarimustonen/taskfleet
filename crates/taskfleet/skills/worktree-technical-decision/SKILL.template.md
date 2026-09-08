---
name: worktree-technical-decision
description: Spawn an autonomous worktree via `taskfleet run create --kind technical-decision` to drive ONE architectural / technical decision to a recorded ADR and self-merge. Use when the user says "decide whether we should use X or Y", "make the architectural call on Z", "settle the trade-off between A and B", or links an issue tagged decision/architecture. Do NOT use for opinions (`/llm-consult`), design ideation (`/llm-workshop`), plan review (`/llm-panel`), survey/research (`/worktree-research`), or archaeology ("why did we choose X" — historical, not a forward decision).
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# worktree-technical-decision

A **technical-decision worktree** is one autonomous agent whose
deliverable is a **recorded ADR** (architecture decision record) in the
repo — usually `docs/adr/<NNNN>-<slug>.md` or the project's equivalent
location. It investigates options, weighs trade-offs across required
expert lenses, picks one, records the decision with rationale and
explicit rejected alternatives, and merges itself back — same
self-merge contract as `worktree-spinoff`.

Read `taskfleet-overview` first; read `worktree-spinoff` for the
shared autonomous-merge contract; read `worktree-research` for the
contrast — research surveys an open space, technical-decision picks
one path and records the call.

## When to use

- ✅ "Decide whether we should use X or Y".
- ✅ "Make the architectural call on Z".
- ✅ "Settle the trade-off between A and B".
- ✅ Issue tagged `decision` / `architecture` and the user says "drive
  this to an ADR".
- ❌ "What do you think of X" → `/llm-consult` (opinion, no record).
- ❌ "Design a system for X" → `/llm-workshop` (ideation, multi-LLM).
- ❌ "Review my plan" → `/llm-panel` (role panel, no merged ADR).
- ❌ "Compare A vs B vs C in depth" → `/worktree-research` (sourced
  report, no chosen path).
- ❌ "Why did we choose X" → archaeology; read past ADRs and the
  commit log, do not spawn anything.

## Workflow

### 0. Validate context

1. Working directory must be a git repo with a clean current branch.
2. ADR target directory must exist (typically `docs/adr/`). If it does
   not, ask the user where the ADR should land and create the
   directory in the worktree.
3. `taskfleet version --output json` to confirm
   `{{CLI_VERSION}}`.

### 1. Pin the decision question

Decisions fail when the **question** drifts. Lock it down before
spawning:

- **Question** — one sentence, posed as a forward choice ("Should we
  use X or Y for Z?").
- **Constraints** — non-negotiable bounds (existing tech, deadlines,
  team skills, regulatory).
- **Options to consider** — at least two; the agent may add more if
  the space genuinely contains them but should not invent strawmen.
- **Expert lenses required** — typically architect + maintainability +
  security; add perf / cost / ergonomics as relevant.
- **Deliverable location** — ADR path.

If any of the above is missing, ask **once** before spawning.

### 2. Build the prompt

1. Pinned decision question + constraints.
2. Options to consider.
3. **Lens application** — agent runs the equivalent of `/llm-panel`
   over the question (architect, maintainability, security, plus
   topic-specific lenses) and synthesizes a recommendation.
4. **ADR structure** — Title / Status (Accepted) / Context / Decision
   / Consequences (including explicitly-rejected alternatives with
   reasons) / Date / Authors. Project-specific ADR templates take
   precedence if present.
5. **Done criteria** — ADR file exists at the agreed path, committed,
   merged back. No code changes unless the ADR mandates them (and
   even then, prefer a follow-up bugfix / code worktree to keep the
   ADR commit clean).
6. **Repository-local tool safety** — if evidence gathering requires building
   taskfleet, use `cargo build --release` and invoke
   `./target/release/taskfleet …` explicitly. During repository work,
   neither workers nor the orchestrator may create, replace, remove, or modify
   the user's installed taskfleet or bundled skills by any mechanism,
   including any `cargo install`, `cargo uninstall`, Homebrew, manual-copy, or
   `skill install` variant.
7. **Tool/sub-workflow failure policy** — copy the disclosure contract below
   into the brief. In particular, an incomplete required lens panel blocks the
   decision; surviving responses cannot stand in for the requested panel.

### 3. Create the run

```
taskfleet run create \
  --kind technical-decision \
  --title "<adr-slug>" \
  --task "<self-contained decision brief>" \
  [--source-branch <branch>] \
  [--idempotency-key <key>]
```

Same flag rules as `worktree-spinoff`. Output defaults to
`--output jsonl`.

### 4. Success envelope

```json
{
  "schema_version": 1,
  "data": {
    "run_id": "01HZ...",
    "supervisor": 12345,
    "kind": "technical-decision",
    "lifecycle": "autonomous",
    "tmux_window": "<workmux-reported-window-name>",
    "branch": "wt/<adr-slug>"
  }
}
```

### 5. Report to the caller

Tell the user:

- Run id, branch, tmux window, expected ADR path.
- That the worktree self-merges once the ADR is committed.
- How to follow: `run show <run-id>` for a one-shot snapshot, `event
  tail <run-id> --follow` for the streaming log, or `run wait <run-id>`
  to block until the run is terminal (`done | failed | cancelled`)
  instead of hand-rolling a poll loop.

## Terminal report (mandatory)

The merge and the terminal `node report` are now **one call**. The run
stays alive until a terminal `node report` lands; until then the per-run
supervisor keeps polling, `taskfleet run show` reads `status:
pending` forever, and the tmux window never closes — the user sees a
worktree that looks stuck when the work is actually done.

There are two closing paths, and the brief MUST instruct the agent to take
exactly one terminal path, never both, before its session ends:

- **Decision made + ADR committed → close with `run merge`.** A single
  `taskfleet run merge` rebases + merges the worktree branch into
  its source branch **and** submits the terminal `node report` for you
  (stamped `via: "explicit-merge"`). Pass your §7.3 payload with
  `--report-file` so the rich `discussion_items` / `spinoff_proposals` /
  `wrap_up_recommendations` ride along in the same call. There is no
  separate `node report` step on this path, and no `/worktree-merge`.
- **Genuinely blocked / needs the user → `node report` with
  `success: false`, no merge.** A real lens tie (see Errors) does NOT
  merge; it stops and submits a direct `node report` carrying the
  unresolved trade-off. The branch stays unmerged until the user breaks
  the tie and re-spawns.

1. **Resolve the exact owning run id and node id** from inside the
   worktree. Use the durable node ownership record, never the branch's display
   identifier, which is a lossy bounded fragment that can repeat, not ownership:

   ```bash
   run_id="$(taskfleet run show --current --output json | jq -er '.data.run_id')" || {
     echo "failed to resolve exact owning run id" >&2
     exit 1
   }
   node_id="n-0001"   # a single-worker kind always has exactly one node
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

3. **Close the run.**

   **Success path — ADR committed, ready to land.** One call merges and
   reports; the `--report-file` payload is validated *before* the merge:

   ```bash
   taskfleet run merge "$run_id" --report-file /tmp/node-report-${run_id}.json
   ```

   `run merge` rebases + merges the branch into the run's recorded
   `source_branch` (override with `--source <branch>`; it auto-detects
   main/master if none is recorded), then submits the §7.3 report it
   was handed. On a clean merge the per-run supervisor winds the run
   down and tears down the worktree, tmux window, and branch
   automatically — do **not** manually run tmux/git cleanup, and do not
   call `node report` yourself on this path. A merge conflict/failure
   exits non-zero with `error.code: "merge_failed"` and submits **no**
   report (the node stays live); resolve the conflict (or
   `/complex-rebase`) and re-run `run merge`.

   **Blocked path — needs the user, no merge.** Submit the report
   directly, with `success: false` and a populated `discussion_items[]`:

   ```bash
   taskfleet node report "$run_id" "$node_id" --from-file /tmp/node-report-${run_id}.json
   ```

   This records the node terminal without merging — `taskfleet
   node show <run-id> <node-id>` reports `status: done` with your report
   attached. The supervisor still winds the run down, but the branch is
   left unmerged for the user.

This step is **not optional**. A successful merge needs the report in
the same `run merge` call; a blocked run needs the direct `node report`.
Either way, no terminal report leaves the run dangling with no
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
when the ADR is independently complete and safe; disclose it in the full
`success: true` report passed to `taskfleet run merge "$run_id"
--report-file /tmp/node-report-${run_id}.json`, never a minimal auto-report.

Requested completeness is a contract. A requested panel with a missing model
section, truncation marker, malformed output, or missing expected artifact is
incomplete, not representative consensus. A required incomplete lens panel
cannot support an Accepted ADR. Retry only when existing workflow policy
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

## Issue Management

If issue-driven, make and validate the final ADR commit, then before `run merge`:

1. Run `issuectl close <slug> --status done --stamp --as <agent> --json`.
2. Require `.data.stamp.status` to be `stamped` or `already_present`; otherwise do not
   merge. The ADR commit now carries `Fixes-Issue: @<slug>`.
3. Commit the exact issue closure metadata path separately and require a clean tree.

A freeform decision has no issue trailer or issue mutation.

## Errors

Same envelope and codes as `worktree-spinoff`. One technical-decision
specific behavior: if the lens panel returns a genuine tie, the agent
does NOT pick randomly — it does **not** merge; it stops and submits a
direct `node report` with `success: false` plus a `discussion_items[]`
entry naming the unresolved trade-off (see "Terminal report (mandatory)"
for the payload shape). The user breaks the tie and re-spawns. A decision
that *is* resolved (ADR committed) lands the other way — via `run merge`,
which merges and reports in one call.

## Install or upgrade `taskfleet`

This skill was installed for `taskfleet {{CLI_VERSION}}`. Compare
`.data.version` from `taskfleet version --output json` to
`{{CLI_VERSION}}`:

- **Missing**: tell the user to install through a published distribution channel
  outside this repository workflow, then stop.
- **Older**: ask the user to upgrade; stop.
- **Newer**: tell the user the installed skill is stale and stop. Refreshing
  installed bundled instructions is published-tool maintenance outside
  repository work; never run `skill install` as part of this workflow.
- **Equal**: proceed.

## Example

```
/worktree-technical-decision Choose between event-sourced and CRUD storage for the taskfleet run state
```
