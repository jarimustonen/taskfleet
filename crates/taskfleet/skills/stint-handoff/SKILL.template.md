---
name: stint-handoff
description: "Explicitly finish a work-session (työrupeama, 'stint'): record current-turn answers, check only runs this session launched or adopted, update TODO.md, verify the issuectl DAG with foreign-run reservations, then `/wrap-up` and any declared reset. Run on the user's explicit go, either directly or after `/stint-start` names it as the next action. Generic and reconstructable from existing facts; adds no durable stint state. NOT the round engine, a global-run drain, bare `/wrap-up`, or a worktree."
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# Stint-handoff — the terminal wrap

You are the **orchestrator** closing out a stint (työrupeama). This skill is the
**terminal wrap only**: leave the repo in a state a fresh agent can resume from. It does
**not** spawn worktrees, deploy, or run a round — that is the round engine
**`/stint-start`**. Run this at session end, on the user's go.

`/stint-start` normally ends its bounded round by naming `/stint-handoff` as the exact next
action. That is guidance, not a token or prerequisite: the user's explicit request to
finish is sufficient, including a direct standalone invocation or one made after supplying
answers. Record any current-turn answers before wrapping; a green round or silence alone
is not authorization.

This skill is **generic**: project specifics (the test-account reset preference and where
the `TODO.md` handoff block lives) are read from the repo's own `AGENTS.md` / `TODO.md`.
Scheduling comes only from `issuectl dag --json`; `TODO.md` remains a narrative and never
stores a second scheduling graph. Verify `issuectl dag --help` advertises
`--reservations`. If the command or required JSON surface is unavailable, stop and report
an unmigrated or incompatible project rather than falling back to prose.

## Standing discipline

- **Keep main clean.** The handoff edits are orchestration, not product code — you make
  them in this session. But commit them promptly and on their own (see the commit step); never
  leave `TODO.md` modified-but-uncommitted across the wrap.
- **Read scheduling, never duplicate it.** Use `issuectl dag --json` for lane order,
  dependency state, collision tokens, computed heads, and spawnability. Do not copy those
  fields into the handoff narrative.
- **Scrutinise spin-off quality before folding.** Before filing a
  review-generated spin-off (from `/llm-review`, a review panel, or an
  `/assess-findings` cascade), weigh it critically against real value. Early-maturity
  review passes over-produce low-value polish suggestions; file only an observed
  occurrence or a self-contained problem with credible impact. A finding that survives
  uses `issuectl intake file`, is created with no lane assignment, and carries
  machine-visible `ai-review` provenance plus available run/target/model/assessment/
  severity/confidence metadata.
  Named model agreement stays a list, never a corroboration score. The resulting
  unaccepted candidate stays `status: untriaged`, with no `lane`, `lane_seq`, or
  `collision` assignment. Do not add it to the next stint's agenda: the
  human lane-or-close sweep owns scheduling and acceptance. “Not selected this round” is
  not permission to set
  `status: deferred`; that status is reserved for an explicit human/product “worthwhile,
  but not now” disposition, and deferred work also remains unscheduled.
- **Ask conversationally.** Never `AskUserQuestion` (global CLAUDE.md).
- **Read worker reports before writing the next handoff.** A terminal report is persisted as
  `last_report` on its node. For a single-worker run, prefer the public
  `run show` surface. For a multi-node run, inspect each node:

  ```bash
  # skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
  taskfleet run show "$run_id" --output json | jq '.data.report'
  # Node-level projection-compatible probe:
  # skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
  taskfleet node show "$run_id" n-0001 --output json |
    jq '.data.report // .data.last_report'
  ```

  `run wait` is multi-run: its results are in `data.runs[]`, so probe it with
  `jq '.data.runs[] | {run_id, status, summary}'`, not `.data.status`. The wait
  result folds in a summary; `run show`/`node show` expose the complete
  `discussion_items`, `spinoff_proposals`, and `wrap_up_recommendations` needed
  for the next handoff.
- **Propose, don't presume.** `/wrap-up` presents proposed `AGENTS.md`/issue/preference
  changes and asks before writing; don't assume it committed unless it reports saved
  changes.

## Steps (propose; run only on the user's go)

0. **Preflight (read-only, session-owned IDs only).** Inspect `git status --short` and
   require a clean index (`git diff --cached --quiet`) before editing. Pre-existing staged
   changes, or unstaged changes to TODO/an answer file this finish would edit, are a hard
   stop: do not absorb or overwrite them. Start from the full run IDs retained by
   `/stint-start`: runs
   this session launched or explicitly adopted. Inspect each with `taskfleet run show
   <full-run-id> --output json`. If this skill was invoked standalone and no such IDs are
   present in the conversation, treat session worker ownership as clear; do not infer it
   from a global list or repository membership. Every session-owned live, awaiting-input,
   recoverable, or otherwise resumable worker must have landed or relinquished ownership.
   For every session-owned failed/cancelled run, read the always-present
   `.data.preserved_work` array from `run show`. A
   terminal status or a repeated cancel alone is not relinquishment. An empty array confirms
   no retained worktree/branch. A non-empty array blocks handoff until the operator chooses
   salvage/manual harvest or explicitly authorizes `taskfleet run discard <id> --reason
   <text>` (use `--dry-run --output json` first, `--node` for multiple rows, and `--force`
   only for reviewed verified-dirty content). Never raw-Git-delete or auto-discard it. If
   ownership stays unresolved, mark handoff blocked and skip
   schedule verification, but continue through steps 2–3 so current-turn answers and that
   owned run ID, slug, and preserved-work fact are recorded. Then stop before `/wrap-up`
   with one exact ownership-recovery action. A known foreign run, including one in this
   repository, never blocks as session-owned work and never enters the ownership narrative.
   Otherwise read the current TODO handoff block and any live-version evidence it will
   claim; write "unverified" rather than guessing.
1. **Read the live schedule without adopting foreign work.** After preflight proves there
   are no session-owned holds, inspect the global Taskfleet list for same-repository runs
   that may still own resources: live, awaiting-input, attention-required, recoverable, or
   terminal candidates. `preserved_work` is show-only: run `taskfleet run show <id>
   --output json` for relevant terminal candidates and treat only a non-empty
   `.data.preserved_work` as a retained-resource hold. Never read that field from `run list`
   or adopt/manage a foreign run merely to inspect it. Convert every mappable run to the
   exact issuectl hold array shape, one object per run: `[{"lane":"backend","collision":["hot-token"]}]`.
   Values are copied from issue metadata; never infer them. Do not wait for, adopt, or
   manage those runs. Run `issuectl dag --json --reservations '<foreign-holds-json>'`
   (`[]` when none) and read `.data.lanes[]`, `.data.unscheduled`, and
   `.data.spawnable_heads`. An unmappable foreign run does not block terminal handoff—this
   skill spawns nothing—but state in the terminal report (not the handoff narrative) that
   a future `/stint-start` must resolve that reservation before spawning. If the command
   fails, its JSON is malformed, the graph has a missing blocker, self-dependency, or cycle, or an
   `untriaged`, `deferred`, or equivalent non-executable disposition appears in
   `.data.lanes[]`, record the verification failure and affected slug for the narrative and
   continue only through the narrative commit; lane presence and mechanical
   `spawnable: true` never make that row executable. Do not call `/wrap-up` or declare the
   handoff complete. Do not mutate issue scheduling during terminal wrap or encode a
   workaround in `TODO.md`.
2. **Record current-turn answers, then update the `TODO.md` handoff narrative.** Put a
   supplied product/technical answer in its canonical issue or documentation when one
   already owns that decision; put only the resumable summary in `TODO.md`. Do not invent
   a checkpoint file or duplicate a canonical answer. Update `## 🔄 Continue here` /
   `ALOITA TÄSTÄ` so a fresh agent can resume from `jatketaan @TODO.md`: where the round
   left off, what landed, the intended product direction, and unresolved decisions. Issue slugs may provide context, including a concise “awaiting human
   lane-or-close triage” note for unscheduled `untriaged` candidates, an explicit
   human/product “not now” note for unscheduled `deferred` work, or a run-ownership fact
   from preflight. Mark triage mentions as context only: they are not accepted, scheduled,
   executable, or part of the prepared execution agenda. Keep the states distinct; never
   convert an out-of-plan or merely unselected candidate to deferred during handoff.
   Recording that verification failed and naming involved slugs—including a
   non-executable disposition found in a lane—is an unresolved decision, not a copied
   schedule. Otherwise do not describe any issue as currently ready, blocked, headed, or
   spawnable, and do not copy lane order, dependency edges, collision values,
   computed-head flags, or spawnability into this file. In particular, an unscheduled
   row's mechanical `spawnable: true` does not make it executable; it must pass human
   triage, move to an accepted active status, and gain a lane before a future round may
   launch it.
3. **Commit the finish records immediately, on their own.** Stage `TODO.md` and any
   canonical answer file by exact path (never `git add -A`). Inspect
   `git diff --cached --name-only` and require it to equal exactly that intended path set
   before committing, so unrelated work cannot be folded into `/wrap-up` or the handoff
   commit. If no answer or narrative changed, do not manufacture an empty commit.
4. **`/wrap-up`**: if schedule verification failed in step 1, stop here and report the
   committed narrative plus the failure. Otherwise `/wrap-up` will present proposed
   `AGENTS.md` / issue / preference changes and ask before writing; do not assume it
   committed unless it reports saved changes.
5. **Test-account reset.** If the project's `AGENTS.md` / `TODO.md` declares a reset
   preference, do it or remind the user. If the project declares none, skip this step.
6. **Verify terminal state.** Run `git status --short`. If `/wrap-up` wrote approved files
   without committing, follow its commit contract (or ask the user). Do not declare the
   handoff complete while main is dirty.

## Non-goals

- **Not the round engine** — it does not pull, plan, spawn worktrees, or deploy; that is
  `/stint-start`.
- **Not a bare `/wrap-up`** — this first updates the `TODO.md` handoff narrative and
  verifies the issuectl schedule, then calls `/wrap-up`.
- **Not a worktree**, and does not create one.
- **No durable stint/checkpoint or global-run ownership.** It records answers only in
  existing canonical issue/docs/TODO owners and checks only explicit session run IDs.
- **Does not write product code** — its direct edits are current-turn answer records and
  the `TODO.md` handoff narrative; `/wrap-up` may separately propose other changes.
- **Hardcodes no project facts** — reads them from the repo's AGENTS.md/TODO.md and issue
  metadata.
