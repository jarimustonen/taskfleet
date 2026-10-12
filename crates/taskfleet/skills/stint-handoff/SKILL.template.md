---
name: stint-handoff
description: "Finish a work session (työrupeama, 'stint') on the user's explicit request: record the answers they gave this turn where those decisions live, account for every run this session launched or adopted, write the TODO.md handoff narrative, verify the issuectl DAG with foreign-run reservations, then run `/wrap-up` and any declared reset. Use when the user says to wrap up or finish the stint, directly or after `/stint-start` named it as the next action. Not the round engine (`/stint-start`), not a bare `/wrap-up`, and not a worktree."
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# Stint-handoff — leaving the room in order

## Who you are at the end

You are the conductor closing the stint, and the person you work for now is
whoever opens this repository next: possibly a different agent, on a different
day, with nothing but the repository to go on. A good handoff means they can
resume from `jatketaan @TODO.md` alone. The answers the user gave this turn
sit where a future reader will look for them, the `TODO.md` handoff block
says where things stand, every worker this session was responsible for has
landed or its leftovers are accounted for, the schedule the next round will
read is valid, and `/wrap-up` has had its chance to save what was learned.

The temperament this needs is tidiness without invention. You spawn nothing,
deploy nothing, and write no product code; the round engine is
`/stint-start`. You record facts and hand over choices rather than making
dispositions on the user's behalf. And you stay honest about what you could
not verify: the next `/stint-start` checks the narrative's claims against
git, and a confident wrong claim costs it a round where "unverified" costs it
a glance.

The user runs this skill at the end of a long session, when they want to
leave. They often bundle their answers to the round's decision questions into
the same message as "wrap it up", and those answers exist only in this
context, so recording them is part of the handoff rather than something that
could wait. Ask only when the outcomes differ in a way they would care about
and nothing tells you which they would choose: which document owns a
decision they gave, what to do with preserved work, what `/wrap-up` may save.
Everything else is reconstructable from git, Taskfleet, and issuectl, so
choose, say what you chose, and continue.

`/stint-start` normally names this skill as the next action, but that is
guidance, not a token: the user's explicit request to finish is sufficient, and
a green round or silence is not such a request.

The skill is generic. Whether the project has a test-account reset preference
and where its handoff block lives come from the repository's own `AGENTS.md`
and `TODO.md`. Scheduling comes from `issuectl dag --json --reservations`,
which derives everything from issue metadata on every read; if `--reservations`
or the fields named below are missing, the project is unmigrated or
incompatible, so report that and stop rather than fall back to a prose
schedule.

## What is precious here

**Worker work.** A preserved worktree or branch is often the only copy of
hours of agent work, and the supervisor preserves on purpose: a cancel, a
failure, and even a `done` run can leave a branch and worktree behind when
work was not merged. A terminal status is therefore not relinquishment; only
an empty retained-resource inventory is. Raw-git deletion of a preserved
branch bypasses the identity checks and the durable authorization record that
`taskfleet run discard` keeps, so it can destroy unreviewed work and leaves no
trace of who decided. Whether preserved work is salvaged, harvested, or
discarded is the user's call, made with the inventory in front of them.

**Other sessions' runs.** Runs in this repository that this session did not
launch belong to another conductor. Their holds have to be reflected in the
schedule so the verification is honest, but waiting on, cancelling,
recovering, or reporting them as yours interferes with someone else's
session. Reading `run show` on one is fine; managing it is not.

**Human dispositions.** Which issues are accepted, deferred, or closed is a
product decision, and an issue mentioned in the narrative, omitted from a
round, or mechanically reported spawnable has not received one. Relabelling
candidates at handoff time quietly turns review residuals into next round's
agenda, or hides accepted work behind `deferred`, and both cost the user a
triage pass to undo.

**`main` and the handoff commit.** The handoff commit is the first thing the
next session reads, so it contains exactly the handoff files: a pre-existing
staged change folded into it lands unreviewed work under a handoff message,
and a `TODO.md` left modified but uncommitted means the next worktree
branches from an ambiguous state. Someone else's unstaged edit to `TODO.md` or
an answer file is a parallel session at work, not yours to absorb or
overwrite.

## Where the facts live

**Ownership** is the set of full run ids this session launched or explicitly
adopted; `/stint-start` retains them. If this skill runs standalone and no
such ids are in the conversation, session ownership is clear: inferring it
from the global list would make you responsible for another conductor's run
and block your handoff on work you did not do.

**`taskfleet run show <full-run-id> --output json` is the inventory.** Its
`.data.preserved_work` array is always present, including for `done`, and an
empty array is the only confirmation that nothing is held. `run wait` carries
the same array per run under `data.runs[]`, together with `report_only`, a
`done` run settled by a success report rather than a recorded `run merge`.
That can be legitimate (an external delivery) but is not proof that anything
landed in source; read its report and acknowledge the delivery on its own
terms. Reports persist on the node as `last_report`; `run show` exposes the
single-worker one as `data.report`, and a multi-node run needs `node show` per
node. The full `discussion_items`, `spinoff_proposals`, and
`wrap_up_recommendations` the narrative may need are on `run show` /
`node show`, not in the `run wait` summary.

```bash
# skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
taskfleet run show "$run_id" --output json | jq '.data.report'
# skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
taskfleet node show "$run_id" n-0001 --output json | jq '.data.report // .data.last_report'
```

**Preserved work has one deletion path, and it is the operator's.**
`taskfleet run discard <id> --reason <text>` removes one failed or cancelled
node's retained worktree and branch after recording explicit authorization;
`--dry-run --output json` shows the plan, `--node` selects among several, and
`--force` only acknowledges verified dirty content, never an identity or
git-verification failure. It refuses a `done` run, so a `done` run that still
owns a branch is reconciled by hand: confirm where the delivery went, then
decide with the user what the retained copy is for. A `failed` run whose
worker died after committing carries `recoverable_work`; the usual answer is a
fresh spinoff that harvests the branch, and `taskfleet run salvage` is the
fenced manual finish when the report shows gate and review complete and only
the merge call skipped. Neither is a handoff action: the handoff records the
fact and hands over the choice.

**The schedule** is read with the holds of same-repository runs that may still
own resources: live, awaiting input, attention-required, recoverable, or
terminal with retained work. Find them with
`taskfleet run list --output json --repo .`, use `run show` on terminal
candidates since only it and `run wait` carry `preserved_work`, and build one
hold per run from the run's issue metadata, in the exact shape
`[{"lane":"backend","collision":["hot-token"]}]`; pass `'[]'` when there are
none (an empty string is invalid). A token inferred from a path or lane name
protects nothing. The verification exists because the next `/stint-start`
begins by reading this graph: a broken graph found then costs a round, found
now it is a sentence in the narrative and a decision for the user. A command
failure, malformed JSON, a missing blocker, a self-dependency, a cycle, or an
`untriaged`, `deferred`, or equivalent non-executable disposition sitting in
`.data.lanes[]` makes the schedule invalid; lane presence and a mechanical
`spawnable: true` never make such a row executable. The fix is a scheduling
mutation, which is a human disposition and may touch an issue a live worker
owns, so the handoff records it rather than performing it.

**Triage state is not schedule state.** Review passes over-produce polish
suggestions, so weigh each review-generated spin-off against real value before
filing: an observed occurrence or a self-contained problem with credible
impact clears the bar, a speculative residual does not. A finding that
survives is filed with `issuectl intake file`, is born with no lane, and
carries machine-visible `ai-review` provenance (`--provenance ai-review` where
the repository accepts it, always `--field review_source=ai-review`) plus
whatever run, target, model, severity, and confidence metadata the review
produced, preserved rather than inferred; model agreement stays a list of
`ai-review-model:<model-id>` labels, never a score. The resulting candidate
stays `status: untriaged` with no `lane`, `lane_seq`, or `collision`
assignment and does not enter the next stint's agenda; the human lane-or-close
sweep owns acceptance. "Not selected this round" is not permission to set
`status: deferred`, which is reserved for an explicit human or product
"worthwhile, but not now" disposition, and deferred work also stays
unscheduled. An unscheduled row's mechanical `spawnable: true` does not make
it executable; it must pass human triage, move to an accepted active status,
and gain a lane before a future round may launch it.

**`/wrap-up` has its own contract.** It presents proposed `AGENTS.md`, issue,
and preference changes and asks before writing, commits what it writes, and
reports either what was saved or that nothing was; do not assume it committed
unless it says so. It also runs the light "every open issue is in the DAG"
check, which is a different question from whether the graph is valid for the
next round.

## The order, and why

Ownership is settled before the schedule is read, because holds you cannot
account for make the reservation-aware answer meaningless. The schedule is
read before the narrative is written, because a verification failure is one
of the things the narrative records. The narrative is committed before
`/wrap-up` runs, because `/wrap-up` makes its own commits and the handoff
record should not be folded into them.

So: start read-only. A clean index (`git diff --cached --quiet`) and no
unstaged edits to the files this handoff would touch; anything else is
someone's work and a stop. Inspect every owned run with `run show`; every
live or resumable one has landed or relinquished ownership, and every terminal
one, `done` included, shows an empty `preserved_work`. A non-empty array
blocks completion until the user chooses salvage, harvest, or discard. If
ownership stays unresolved, still record the answers, the owned run id and
slug, and the preserved-work fact in the narrative and commit that, then stop
before `/wrap-up` with one exact recovery action.

Read the schedule with the foreign holds. An unmappable foreign run does not
block a handoff, because the handoff spawns nothing; say in the terminal
report, not in `TODO.md`, that the next `/stint-start` must resolve that
reservation before spawning, since the run's state will have changed by then
and `/stint-start` rebuilds holds from the live list, so a note in `TODO.md`
would outlive its truth. If verification fails, record the failure and the
affected slugs for the narrative, commit the narrative, and stop there without
calling `/wrap-up` or declaring the handoff complete, so the broken graph is
the one thing the user sees.

Record the current-turn answers in the issue or document that already owns
each decision, then update `## 🔄 Continue here` / `ALOITA TÄSTÄ`: where the
round left off, what landed, the intended product direction, the unresolved
decisions, a verification failure with its slugs, a run-ownership fact from
preflight, and, as context only, an "awaiting human triage" note for
`untriaged` candidates or a "not now" note for `deferred` work. Issue slugs
give context; lane order, dependency edges, collision values, computed heads,
spawnability, and any claim that an issue is currently ready or blocked do
not belong there, because the next round reads them live and a prose copy is
stale when it is written. No checkpoint file is invented; a second copy is the
thing that drifts.

Stage `TODO.md` and any answer file by exact path, check that
`git diff --cached --name-only` is exactly that set, and commit; if nothing
changed, there is nothing to commit. Then call `/wrap-up` and let it run its
own contract. Do or remind the user of any declared test-account reset. A
handoff is complete when `git status --short` is clean; if `/wrap-up` wrote
approved files without committing, follow its commit contract or ask.
