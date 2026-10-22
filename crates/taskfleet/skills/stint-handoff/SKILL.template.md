---
name: stint-handoff
description: "Finish a work session (työrupeama, 'stint') on the user's explicit request: record the answers they gave this turn where those decisions live, account for every run this session launched or adopted, write the TODO.md handoff narrative, verify the repository schedule when enabled, then save repository lessons and perform only authorized resets. Use when the user says to wrap up or finish the stint, directly or after `/stint-start` named it as the next action. Not the round engine (`/stint-start`), not a bare `/wrap-up`, and not a worktree."
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# Stint-handoff — leaving the room in order

## Repository tracker (optional)

Use issuectl only when **both** repository instructions or an issuectl schema
establish that this repository uses it **and** its CLI is available. A directory named `issues/` alone may belong to another tracker;
check the repository policy before treating it as issuectl storage. This probe
checks the conventional schema-backed case from the repository root:

```sh
# issuectl-capability-probe
if [ ! -f issues/.schema.yaml ]; then
  printf 'tracker-not-used\n'
elif command -v issuectl >/dev/null 2>&1; then
  printf 'issuectl-ready\n'
else
  printf 'issuectl-cli-missing\n'
fi
```

For a documented issuectl tree without that schema path, verify configuration
by reading repository instructions or tracker files, not by running issuectl
against an unidentified `issues/` tree. Check CLI availability separately.
Do not install or initialize a tracker. Without both capabilities, use the free-text task and committed
repository artifacts; skip issue lookup, mutation, closure, trailers, and DAG
commands. If an explicit issue-driven request requires unavailable tracker data
or a required mutation, report that prerequisite and stop before spawning;
do not silently reinterpret a slug or hand-edit tracker metadata. A CLI command
failure in an enabled tracker is an error, not permission to switch modes.
`issuectl-cli-missing` is a disclosed missing capability, not proof that the
repository is tracker-free. If repository policy requires tracker scheduling or
writes for this task/stint, stop; only policy-permitted free-text work may use
the no-issuectl path. Record follow-up findings in the native report's
`spinoff_proposals` or `discussion_items` instead of creating tracker files.
State this capability result as a fact in every worker brief; it does not relax
the generated policy on how an issue, if filed, must be created.

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

## Session preservation and resource settlement

These are separate outcomes. Unresolved retained resources or invalid scheduling
prevent claiming full settlement, not saving the session. After inventorying
what is known, continue permitted lesson capture, tool-friction reporting, and
lightweight schedule/unlaned-issue checks through `/wrap-up` or the local
fallback below. This applies with or without issuectl, including clean harvested
work whose authorized native cleanup is blocked and genuinely unmerged work.
Neither case grants deletion authority: leave retained resources untouched
unless the user has authorized a supported native lifecycle action. Do not
control foreign runs or bypass native checks to make the handoff look clean.

Read an available `/wrap-up` contract before using it. If one of its actions
depends on settled resources, a valid schedule, or another missing prerequisite,
skip only that action and disclose why; continue independent preservation.
Record tool friction through the permitted reporting path, not invented tracker
files. When the external skill is absent, use the repository-local fallback;
its absence alone does not block saving the session. Report session preservation,
resource settlement, and schedule verification separately, with unresolved run
ids, failures, skipped actions, and the next authorized recovery action. Never
claim all resources settled or the schedule valid without evidence.

## Without issuectl: repository-local handoff

If the optional repository tracker is not enabled, use this path instead of
the tracker-backed handoff below. Read the repository's handoff convention and
record the user's decisions in the documents that own them, not in a new tracker.
Inspect every full run id this session launched or explicitly adopted with
`taskfleet run show`; do not infer ownership from a global list. Read reports,
verify delivered content, and inventory retained work even for terminal runs.
Do not cancel, recover, or discard foreign runs. A live owned run or retained
work needs an explicit recovery/retention decision; record it honestly and do
not claim the handoff resolved it.

Write the repository's existing handoff file (default `TODO.md`) with accepted
intent, landed results and validation evidence, decisions still needed, known
unresolved owned run ids and recovery actions, and the next bounded task. Do
not manufacture issue slugs, lanes, reservations, a backlog, or a private DAG.
There is no DAG verification step in this mode. Preserve other sessions' edits;
stage only your exact paths, inspect the staged diff, and commit the narrative
separately from other changes. Push only if repository policy authorizes it.

If an external session-saving skill is available, read its actual contract;
otherwise save durable repository lessons in the most specific existing agent
instructions or owning document and commit those separately. Do not write
personal/global preferences implicitly. A test-account reset is neither assumed
nor automatic: perform one only with explicit project/user authorization and
disclose its result. Report what was saved, unresolved work and failures, and
one next action; a required failure prevents claiming completion.

## With issuectl: tracker-backed handoff

The remainder applies only with the optional repository tracker enabled. Missing
or incompatible DAG support is a disclosed blocker for this mode; it never
authorizes hand editing tracker metadata or fabricating a schedule.

## Who you are at the end

You are the conductor closing the stint, and the person you now work for is
whoever opens this repository next: possibly a different agent, on a different
day, with nothing but the repository to go on. A good handoff lets them resume
from `jatketaan @TODO.md` alone. The answers the user gave this turn sit where
a future reader will look for them, the `TODO.md` handoff block says where
things stand, every worker this session was responsible for has landed or its
leftovers are accounted for, the schedule the next round will read is valid,
and `/wrap-up` has had its chance to save what was learned.

**Your craft is a narrative that survives being checked.** The next
`/stint-start` opens by testing the handoff's claims against git, the
live-version check, and the DAG. A claim it confirms costs it a glance. A
confident claim it refutes costs it a round of investigation and its trust in
the rest of the text. A claim marked unverified costs it a glance and keeps
its trust. So write what you established and how, write what you could not
establish as exactly that, and leave out whatever the next round reads live
anyway. The narrative carries intent, decisions, and the state of the work;
it does not carry facts a tool recomputes.

**You record and hand over; you do not decide for the user.** Tidiness
without invention. When something at the end is not what you expected, the
three questions that ran the round still apply. What does a primary source
say: `run show` for what a run owns, git for what landed, the DAG for the
schedule. Whose decision is it: preserved work, issue dispositions, and what
`/wrap-up` may save belong to the user; another conductor's run belongs to
them; the record belongs to you. What would be lost: a deleted branch is
irreplaceable, a paragraph in `TODO.md` is a minute. You spawn nothing, deploy
nothing, and write no product code; the round engine is `/stint-start`.

**You decide and move on everything else.** The user runs this skill when
they want to leave, and they often bundle their answers to the round's
decision questions into the same message as "wrap it up". Those answers exist
only in this context, so recording them is part of the handoff rather than
something that could wait. Ask only when the outcomes differ in a way they
would care about and nothing tells you which they would choose: which
document owns a decision they gave, what to do with preserved work, what
`/wrap-up` may save. Everything else is reconstructable from git, Taskfleet,
and issuectl, so choose, say what you chose, and continue.

`/stint-start` normally names this skill as the next action, but that is
guidance, not a token: the user's explicit request to finish is sufficient,
and a green round or silence is not such a request.

## What the project supplies

The skill is generic. Whether the project has a test-account reset preference
and where its handoff block lives come from the repository's own `AGENTS.md`
and `TODO.md`. Scheduling comes from `issuectl dag --json --reservations`,
which derives everything from issue metadata on every read; if
`--reservations` or the fields named below are missing, the project is
unmigrated or incompatible, so report that and skip schedule-dependent actions
rather than fall back to a prose schedule. Continue independent session
preservation. `taskfleet-run-overview` carries the full field lists of `run show`
and `run wait`; this file repeats only what the handoff turns on.

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
candidates at handoff time turns review residuals into next round's agenda or
hides accepted work behind `deferred`, and either costs the user a triage
pass to undo.

**`main` and the handoff commit.** The handoff commit is the first thing the
next session reads, so it contains exactly the handoff files: a pre-existing
staged change folded into it lands unreviewed work under a handoff message,
and a `TODO.md` left modified but uncommitted means the next worktree
branches from an ambiguous state. Someone else's unstaged edit to `TODO.md`
or an answer file is a parallel session at work, not yours to absorb or
overwrite.

## Where the facts live

**Ownership** is the set of full run ids this session launched or explicitly
adopted; `/stint-start` retains them. If this skill runs standalone and no
such ids are in the conversation, session ownership is clear: inferring it
from the global list would make you responsible for another conductor's run
and block your handoff on work you did not do.

**`run show` is the inventory.** Its `.data.preserved_work` array is always
present, including for `done`, and an empty array is the only confirmation
that nothing is held. `run wait` carries the same array per run under
`data.runs[]`, together with `report_only`, a `done` run settled by a success
report rather than a recorded `run merge`. That can be legitimate (an
external delivery) but is not proof that anything landed in source; read its
report and acknowledge the delivery on its own terms. Reports persist on the
node as `last_report`; `run show` exposes the single-worker one as
`data.report`, and a multi-node run needs `node show` per node. The full
`discussion_items`, `spinoff_proposals`, and `wrap_up_recommendations` the
narrative may need are on `run show` / `node show`, not in the `run wait`
summary.

```bash
taskfleet run show <run-id> --output json
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
terminal with retained work. Find them with the repository-scoped list, use
`run show` on terminal candidates since only it and `run wait` carry
`preserved_work`, and build one hold per run from the run's issue metadata,
in the exact shape `[{"lane":"backend","collision":["hot-token"]}]`; pass
`'[]'` when there are none (an empty string is invalid). A token inferred from
a path or lane name protects nothing.

```bash
taskfleet run list --output json --repo .
```

The verification exists because the next `/stint-start` begins by reading
this graph: a broken graph found then costs a round, found now it is a
sentence in the narrative and a decision for the user. A command failure,
malformed JSON, a missing blocker, a self-dependency, a cycle, or an
`untriaged`, `deferred`, or equivalent non-executable disposition sitting in
`.data.lanes[]` makes the schedule invalid; lane presence and a mechanical
`spawnable: true` never make such a row executable, and a `deferred` row in a
lane serializes everything behind it on work nobody approved, because
issuectl treats `deferred` as active-class. The fix is a scheduling mutation, which
is a human disposition. If the user has already supplied that decision and no
live worker owns the issue, apply it through issuectl and verify again;
otherwise record the defect and the pending mutation for the next round
rather than inventing a disposition to make the graph pass.

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

**`/wrap-up` has its own contract, and it is not bundled with Taskfleet.**
Read its installed instructions if present; do not assume its permissions,
checks, or commits. When absent, save repository lessons in the most specific
`AGENTS.md` or owning issue/document, verify the enabled tracker schedule and
note unscheduled work without accepting it, and commit exact paths separately
from the handoff. Neither route authorizes global preference changes.

## The order, and why

Ownership is inventoried before the schedule is read, because unresolved holds
must remain visible in the reservation-aware answer; cleanup need not succeed
for independent preservation to proceed. The schedule is
read before the narrative is written, because a verification failure is one
of the things the narrative records. The narrative is committed before
`/wrap-up` runs, because `/wrap-up` makes its own commits and the handoff
record should not be folded into them.

So: start read-only. A clean index (`git diff --cached --quiet`) and no
unstaged edits to the files this handoff would touch; anything else is
someone's work and a stop. Inspect every owned run with `run show`; every
live or resumable one needs its ownership accounted for, and every terminal
one, `done` included, needs its `preserved_work` inspected. A non-empty array
means resource settlement is incomplete, even if git verifies that its work
was harvested. Record unresolved ownership, the owned run id and slug, the
preserved-work fact, any native cleanup failure, and the next authorized
recovery action in the narrative. Commit that record and continue independent
`/wrap-up` work or the local fallback; do not salvage, harvest, or discard
merely to finish this skill.

Read the schedule with the foreign holds. An unmappable foreign run does not
block a handoff, because the handoff spawns nothing; say in the terminal
report, not in `TODO.md`, that the next `/stint-start` must resolve that
reservation before spawning. The run's state will have changed by then and
`/stint-start` rebuilds holds from the live list, so a note in `TODO.md`
would outlive its truth. If verification fails, record the failure and the
affected slugs for the narrative and commit it. Keep the invalid schedule
explicit in the final report; skip only actions that require valid scheduling,
not independent `/wrap-up` work or the local fallback. Do not fabricate holds,
accept issues, or claim verification succeeded to get past a failure.

Record the current-turn answers in the issue or document that already owns
each decision, then update `## 🔄 Continue here` / `ALOITA TÄSTÄ`: where the
round left off, what landed and how you confirmed it, the intended product
direction, the unresolved decisions, a verification failure with its slugs, a
run-ownership fact from preflight, and, as context only, an "awaiting human
triage" note for `untriaged` candidates or a "not now" note for `deferred`
work. Issue slugs give context; lane order, dependency edges, collision
values, computed heads, spawnability, and any claim that an issue is currently
ready or blocked do not belong there, because the next round reads them live
and a prose copy is stale when it is written. No checkpoint file is invented
for the same reason: a second copy is the thing that drifts.

Stage `TODO.md` and any answer file by exact path, check that
`git diff --cached --name-only` is exactly that set, and commit; if nothing
changed, there is nothing to commit. Then call `/wrap-up` and let it run its
own contract, or do its work yourself when it is not installed. Perform a test-account reset only with explicit repository/user authorization;
report its result rather than treating a preference as permission. Session
preservation is complete when its permitted saving work is committed and
`git status --short` is clean; if `/wrap-up` wrote approved files without
committing, follow its commit contract or ask. A clean checkout does not prove
resource settlement or schedule validity: report either as incomplete when
unresolved, even after session preservation succeeds.
