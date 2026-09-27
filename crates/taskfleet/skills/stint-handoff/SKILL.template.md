---
name: stint-handoff
description: "Explicitly finish a work-session (työrupeama, 'stint'): record current-turn answers, check only runs this session launched or adopted, update TODO.md, verify the issuectl DAG with foreign-run reservations, then `/wrap-up` and any declared reset. Run on the user's explicit go, either directly or after `/stint-start` names it as the next action. Generic and reconstructable from existing facts; adds no durable stint state. NOT the round engine, a global-run drain, bare `/wrap-up`, or a worktree."
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# Stint-handoff — the terminal wrap

You are the orchestrator closing a stint (työrupeama). The point of the handoff is that
the next session, possibly a different agent on a different day, can pick the work up
from the repository alone: the answers the user gave this turn sit where a future reader
will look for them, the `TODO.md` handoff block says where things stand, every worker
this session was responsible for has landed or its leftover work is accounted for, the
schedule the next round will read is valid, and `/wrap-up` has had its chance to save
what was learned. The handoff spawns nothing, deploys nothing, and writes no product
code; the round engine is `/stint-start`.

`/stint-start` normally ends a round by naming `/stint-handoff` as the next action. That
is guidance, not a token: the user's explicit request to finish is sufficient, whether a
direct standalone invocation or one made after answering the round's questions. A green
round or silence is not such a request. Users often bundle answers to the round's
decision questions into the same message as "wrap it up", and those answers exist only
in this context, so recording them is part of the handoff, not something to do after it.

The skill is generic. The project facts it needs, whether there is a test-account reset
preference and where the `TODO.md` handoff block lives, come from the repository's own
`AGENTS.md` and `TODO.md`. Scheduling comes from `issuectl dag --json`, which derives
lane order, dependency state, collision tokens, computed heads, and spawnability from
issue metadata on every read. `TODO.md` used to carry a second copy of the schedule in
prose; the two drifted, and the narrative now never stores schedule state. If
`issuectl dag --help` does not advertise `--reservations`, or the envelope lacks the
fields named below, the project is unmigrated or incompatible: report that and stop
rather than fall back to a prose schedule.

## What is at stake

**Worker work.** A preserved worktree or branch is often the only copy of hours of agent
work, and the supervisor preserves on purpose: a cancel, a failure, a blocked handoff,
and even a `done` run can leave a branch and worktree behind when work was not merged.
A terminal status or a repeated cancel is therefore not relinquishment; only an empty
retained-resource inventory is. Raw-git deletion of a preserved branch or worktree
bypasses the identity checks and the durable authorization record that
`taskfleet run discard` performs, so it can destroy work that was never reviewed and
leaves no trace of who decided. Whether preserved work is salvaged, harvested, or
discarded is the user's call, made with the inventory in front of them.

**Other sessions' runs.** Runs in this repository that this session did not launch
belong to another conductor. Their holds have to be reflected in the schedule so the
verification is honest, but waiting on them, cancelling them, recovering them, or
reporting them as yours interferes with someone else's session. Reading `run show` on
one is fine; managing it is not.

**Human dispositions.** Which issues are accepted for execution, deferred, or closed is
a product decision. An issue mentioned in the narrative, omitted from a round, or
mechanically reported spawnable has not received one. Handoff-time relabelling of
candidates quietly turns review residuals into next round's agenda, or hides accepted
work behind `deferred`, and both cost the user a triage pass to undo.

**`main` and the handoff commit.** The handoff commit is the first thing the next
session reads. It has to contain exactly the handoff files: a pre-existing staged change
folded into it lands unreviewed work under a handoff message, and a `TODO.md` left
modified-but-uncommitted across the wrap means the next worktree branches from an
ambiguous state. Someone else's unstaged edit to `TODO.md` or an answer file is a
parallel session at work, not yours to absorb or overwrite.

**The user's attention.** The handoff runs at the end of a long session, when the user
wants to leave. Ask when the possible outcomes differ in a way they would care about and
the sources cannot tell you which they would choose: which file owns a decision they
gave, what to do with preserved work, what `/wrap-up` may save. Everything else here is
reconstructable from git, Taskfleet, and issuectl; choose, say what you chose, and
continue.

## Where the facts live

**Ownership is the set of full run ids this session launched or explicitly adopted.**
`/stint-start` retains them; that set, not the global run list and not repository
membership, is what the handoff accounts for. If this skill was invoked standalone and
no such ids are present in the conversation, treat session worker ownership as clear;
inferring ownership from a global list would make you responsible for another
conductor's run and block your handoff on work you did not do. A known foreign run,
including one in this repository, never enters the ownership narrative.

**`taskfleet run show <full-run-id> --output json` is the inventory.** Its
`.data.preserved_work` array is always present, including for `done`, and lists every
retained worktree or branch; an empty array is the only confirmation that nothing is
held. `run wait` carries the same array per run under `data.runs[]`, together with
`report_only`: a `done` run settled by a success report rather than a recorded
`run merge`. That can be legitimate (an external delivery), but it is not proof that
anything landed in source; read its report and acknowledge the delivery on its own
terms. Worker reports persist on the node as `last_report`; `run show` exposes the
single-worker one as `data.report`, and a multi-node run needs `node show` per node:

```bash
# skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
taskfleet run show "$run_id" --output json | jq '.data.report'
# Node-level projection-compatible probe:
# skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
taskfleet node show "$run_id" n-0001 --output json |
  jq '.data.report // .data.last_report'
```

`run wait` is multi-run, so probe it with `jq '.data.runs[] | {run_id, status, summary}'`,
not `.data.status`. It folds in a summary only; the complete `discussion_items`,
`spinoff_proposals`, and `wrap_up_recommendations` that the narrative may need are on
`run show` / `node show`.

**Preserved work has exactly one deletion path, and it is the operator's.**
`taskfleet run discard <id> --reason <text>` removes one failed or cancelled node's
retained worktree and branch after recording explicit authorization; `--dry-run --output
json` shows the plan first, `--node` selects among several retained rows, and `--force`
only acknowledges verified staged, modified, or untracked content, never an identity or
git-verification failure. It refuses a `done` run, so a `done` run that still owns a
branch or worktree is reconciled by hand: confirm where the delivery went, then decide
with the user what the retained copy is for. A `failed` run whose worker died after
committing carries `recoverable_work`; the usual answer is a fresh spinoff that harvests
the branch, and `taskfleet run salvage` is the fenced manual finish when the report shows
gate and review complete and only the merge call was skipped. Neither is a handoff
action: the handoff records the fact and hands over the choice.

**The schedule is `issuectl dag --json --reservations '<holds>'`.** Read `.data.lanes[]`,
`.data.unscheduled`, and `.data.spawnable_heads`. The holds argument tells issuectl what
in-flight runs already own, because it reads nothing from Taskfleet itself; pass `'[]'`
when there are none (an empty string is invalid). A hold is the exact issuectl hold array
shape, one object per run, values copied from the run's issue metadata:
`[{"lane":"backend","collision":["hot-token"]}]`. Inferring a token from a path or lane
name protects nothing. The verification exists because the next `/stint-start` begins by
reading this graph: a broken graph found then costs a round, found now it is a sentence
in the narrative and a decision for the user. A command failure, malformed JSON, a
missing blocker, a self-dependency, a cycle, or an `untriaged`, `deferred`, or equivalent
non-executable disposition sitting in `.data.lanes[]` makes the schedule invalid; lane
presence and a mechanical `spawnable: true` never make such a row executable. The fix
is a scheduling mutation, which is a human disposition and may touch an issue a live
worker owns, so the handoff records it rather than performing it, and does not encode a
workaround in `TODO.md`.

**Triage state is not schedule state.** Review passes over-produce polish suggestions,
so weigh each review-generated spin-off (from `/llm-review`, a panel, or an
`/assess-findings` cascade) against real value before filing: an observed occurrence or
a self-contained problem with credible impact clears the bar, a speculative residual
does not. A finding that survives is filed with `issuectl intake file`, is created with
no lane assignment, and carries machine-visible `ai-review` provenance (`--provenance
ai-review` where the repository accepts it, always `--field review_source=ai-review`)
plus whatever run/target/model/assessment/severity/confidence metadata the review
produced. Named model agreement stays a list of `ai-review-model:<model-id>` labels,
never a corroboration score. The resulting unaccepted candidate stays
`status: untriaged`, with no `lane`, `lane_seq`, or `collision` assignment, and does not
enter the next stint's agenda: the human lane-or-close sweep owns scheduling and
acceptance. "Not selected this round" is not permission to set `status: deferred`; that
status is reserved for an explicit human/product “worthwhile, but not now” disposition,
and deferred work also stays unscheduled. An unscheduled row's mechanical
`spawnable: true` does not make it executable; it must pass human triage, move to an
accepted active status, and gain a lane before a future round may launch it.

**`/wrap-up` has its own contract.** It presents proposed `AGENTS.md`, issue, and
preference changes and asks before writing, commits whatever it writes, and reports
either what was saved or that nothing was. Do not assume it committed unless it says so.
It also runs the light "every open issue is in the DAG" check; the handoff's schedule
verification is the different question of whether the graph is valid for the next
round.

## The sequence

The order below matters for reasons that are not visible from the steps themselves, so
it is stated. Ownership is settled before the schedule is read, because holds you cannot
account for make the reservation-aware answer meaningless. The schedule is read before
the narrative is written, because a verification failure is one of the things the
narrative records. The narrative is committed before `/wrap-up` runs, because
`/wrap-up` makes its own commits and the handoff record should not be folded into them.

0. **Preflight, read-only.** Inspect `git status --short` and require a clean index
   (`git diff --cached --quiet`). Pre-existing staged changes, or unstaged changes to
   `TODO.md` or an answer file this handoff would edit, are a stop: they are someone's
   work, not yours to absorb. Then inspect every session-owned run with `run show`.
   Every session-owned live, awaiting-input, recoverable, or otherwise resumable worker
   has to have landed or relinquished ownership, and every session-owned terminal run,
   `done` included, has to show an empty `.data.preserved_work`. A non-empty array
   blocks the handoff until the user chooses salvage, harvest, or discard. If ownership
   stays unresolved, skip step 1 but continue through steps 2–3 so the current-turn
   answers and the owned run id, slug, and preserved-work fact are recorded and
   committed, then stop before `/wrap-up` with one exact ownership-recovery action.
   Otherwise read the current handoff block and whatever live-version evidence it will
   claim; where you cannot verify a claim, write "unverified" rather than guess, since
   the next `/stint-start` checks these claims against git and a wrong one costs it a
   round.
1. **Read the live schedule without adopting foreign work.** Look at the global run list
   (`taskfleet run list --output json`, `--repo .` narrows to this repository) for
   same-repository runs that may still own resources: live, awaiting-input,
   attention-required, recoverable, or terminal with retained work. Use `run show` on
   the terminal candidates, since only it and `run wait` carry `preserved_work`. Map
   each such run to its hold from issue metadata and run
   `issuectl dag --json --reservations '<foreign-holds-json>'`. An unmappable foreign
   run does not block terminal handoff, because the handoff spawns nothing; say in the
   terminal report, not in the narrative, that the next `/stint-start` must resolve that
   reservation before spawning. It goes in the report because the run's state will have
   changed by the next session and `/stint-start` rebuilds holds from the live list; a
   note in `TODO.md` would outlive its truth. If verification fails for any of the
   reasons above, record the failure and the affected slugs for the narrative and
   continue only through the narrative commit; do not call `/wrap-up` or declare the
   handoff complete, so the broken graph is the one thing the user sees.
2. **Record current-turn answers, then update the handoff narrative.** A product or
   technical answer the user supplied goes into the issue or document that already owns
   that decision; `TODO.md` gets only the resumable summary, and no checkpoint file is
   invented, because a second copy is the thing that drifts. Update
   `## 🔄 Continue here` / `ALOITA TÄSTÄ` so a fresh agent can resume from
   `jatketaan @TODO.md`: where the round left off, what landed, the intended product
   direction, and the unresolved decisions, including a verification failure with its
   slugs, a run-ownership fact from preflight, and, as context only, an "awaiting human
   lane-or-close triage" note for `untriaged` candidates or a "not now" note for
   `deferred` work. Issue slugs give context; lane order, dependency edges, collision
   values, computed heads, spawnability, and any claim that an issue is currently ready,
   blocked, or headed do not belong here, because the next round reads them live and a
   copy in prose is already stale when it is written.
3. **Commit the handoff records on their own.** Stage `TODO.md` and any canonical
   answer file by exact path, never `git add -A`, and check that
   `git diff --cached --name-only` equals exactly that set before committing. If no
   answer or narrative changed, there is nothing to commit; do not manufacture an empty
   commit.
4. **`/wrap-up`.** With the narrative committed and the schedule verified, call it and
   let it run its own contract.
5. **Test-account reset.** If the project's `AGENTS.md` or `TODO.md` declares a reset
   preference, do it or remind the user; if none is declared, there is nothing to do.
6. **Verify the terminal state.** `git status --short` should be clean. If `/wrap-up`
   wrote approved files without committing, follow its commit contract or ask. A
   handoff is not complete while `main` is dirty.

## Non-goals

- Not the round engine: it does not pull, plan, spawn worktrees, or deploy; that is
  `/stint-start`.
- Not a bare `/wrap-up`: it first records answers, updates the narrative, and verifies
  the issuectl schedule, then calls `/wrap-up`.
- Not a worktree, and does not create one.
- No durable stint/checkpoint or global-run ownership. Answers land only in existing
  canonical issue, documentation, and `TODO.md` owners, and only explicit session run
  ids are checked.
- Does not write product code; its direct edits are answer records and the `TODO.md`
  handoff narrative. `/wrap-up` may separately propose other changes.
- Hardcodes no project facts; reads them from the repository's `AGENTS.md`, `TODO.md`,
  and issue metadata.
