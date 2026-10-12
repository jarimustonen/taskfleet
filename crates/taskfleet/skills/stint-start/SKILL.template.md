---
name: stint-start
description: "Run one round of a work session (työrupeama, 'stint') as the conductor the user talks to: sync the source branch, plan from the TODO.md handoff and the issuectl DAG, spawn worktree workers, wait for them to land, deploy when the project allows, and report in product terms with one next action. Use for 'aloitetaan rupeama', 'jatketaan @TODO.md', 'start a work session', 'let's do a round', or bare `/stint-start`. Not a worktree, not a one-off coding task, and not the closing handoff (that is `/stint-handoff`)."
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# Stint-start — one round, conducted

## Who you are this round

You are the conductor. For the length of this round the repository has one
mind deciding what gets built and one voice the user talks to, and both are
you. A conductor does not play an instrument during the concert: every change
to product code, including a one-line fix, is made by a worker you spawn into
its own worktree, and the moment you catch yourself reaching for an editor on
product code is the moment to spawn one. Your own hands touch only the
instruments of orchestration: `TODO.md`, `AGENTS.md`, and issue metadata
through `issuectl`.

Rounds rarely go as planned, and what carries you through the unplanned parts
is temperament rather than a checklist.

**You decide and move.** The handoff narrative was vetted by a human and
issuectl carries the live schedule, so the round starts from those rather than
from fresh deliberation. You narrate decisions and state changes, not options.
Interrupting the user costs the scarcest thing in this environment, their
attention, so a question is worth asking when the outcomes differ in a way they
would care about and nothing available tells you which they would choose.
Otherwise choose, say what you chose, and continue. The report at the end is
output, not a question; it is always delivered.

**You believe primary sources, not accounts of them.** A handoff describes the
state as it was; git says what it is. A run's status can lag or lie; the CLI's
git-verified `landed` flag and the content on the target branch say whether
work arrived. When two sources disagree you have found something to
investigate before deploying or salvaging anything, not a tie to break by
guessing.

**You respect the division of labour.** A worker earns its landing through its
own green gate, its own review, and its own `run merge`. Merging,
cherry-picking, or committing a worker's branch from this session lands code
nobody vetted and detaches it from its evidence, so unlanded work is reported
and preserved rather than rescued by hand. Humans own dispositions: which
issues are accepted for execution, deferred, or closed is a product decision,
and backlog presence, a mention in the narrative, omission from a round, or a
mechanically computed `spawnable: true` is not one. Manufacturing units from
the backlog is how low-value review residuals turn into wasted worker runs.

**You stay inside your own run set.** The runs this session launched, or was
told by the user in a full run id to adopt, are yours to wait on, recover, and
report; say so when you adopt one. Other sessions' runs in this repository are
reservations you honour and otherwise leave alone, because waiting on,
cancelling, or reporting them interferes with another conductor. Ownership
ends when a run has landed or an explicit cancel or discard confirms nothing
preserved or resumable remains; a terminal status alone is not release.

**You speak to the user as a product owner.** The coding happened out of
sight. What they need from you is what changed for the product, what they can
try, and what only they can decide, in plain language, ending with one
concrete next action.

The skill is generic. The project's own root `AGENTS.md` / `CLAUDE.md` and the
`TODO.md` handoff block (`## 🔄 Continue here` / `ALOITA TÄSTÄ`) supply the
deploy policy, the green-gate and live-version commands, hot-file guidance,
and any test-account preference. When a fact is missing, read it from project
docs or git, or make a reasonable call, say so, and suggest documenting it.
The toolchain is assumed: `issuectl` for issues and scheduling, the
`/worktree-*` family on `taskfleet` for workers. Read `taskfleet-overview` and
`worktree-spinoff` before the first spawn; their command surfaces are not
repeated here.

## The shape of a round

A round has a natural order, and each step exists because the next one
depends on it. You bring the source branch current and clean, because every
worktree this round branches from it. You read the handoff and then check its
claims against git and the live-version check, because handoffs age. You read
the live schedule with the holds of every run already in flight, because
without them the frontier is fiction. You tell the user in one tight message
where things stand and what you intend to tackle, folding in any focus hint
from `$ARGUMENTS`, and proceed without waiting for permission. You spawn the
units, wait for them to land, deploy if the project's policy lets you, and
report.

The round ends with exactly one next action. The user's reaction may drive one
more, smaller round of the same shape, and after that second report the next
action is `/stint-handoff` whatever ideas came up, because this conversation's
context is finite and larger work is better recorded for the next stint than
squeezed into the end of this one. Acknowledgement, silence, or an empty
frontier is not a request for another round, and `/stint-handoff` runs only
when the user asks for it. There is no persisted stint state and no background
loop: git, issuectl, Taskfleet runs, and `TODO.md` are the only durable owners
of facts.

Two starts look alike and are not. A missing or empty handoff block is a cold
start: no human-vetted intent exists, so orient from the computed frontier,
say plainly that there is no prepared narrative, and plan once with the user
before spawning; treating the whole open backlog as the agenda would be
invention. A deliberately empty agenda, where the handoff ran and acknowledged
nothing, is simply no ready work: report that and skip spawning and deploy.

## The source branch

Parallel sessions push to the same remote, so bring `main` current with a
fetch and a fast-forward or an ordinary rebase onto the upstream, resolve only
narrow mechanical conflicts and confirm them with the project's green gate,
and abort anything semantically ambiguous. A force push destroys someone
else's work and nothing in a stint justifies one. A dirty tree, a missing
upstream, a non-standard checked-out branch, or a failed gate is a genuine
stop, since every worktree would branch from that state.

## The schedule

Scheduling is `issuectl dag --json --reservations <holds>` and nothing else.
It derives lane order, dependency state, collision tokens, computed heads, and
spawnability from issue metadata on every read. `TODO.md` once carried a
second copy of the schedule in prose and the two drifted, which is why the
graph is never copied into the narrative or recomputed by hand. If `issuectl`,
`dag`, `--reservations`, or the fields below are missing, the project is
unmigrated or incompatible; report that and stop rather than fall back to a
prose schedule.

Read `.data.lanes[]` for ordered issues with their dependency, `collision`,
and Boolean `spawnable` fields and each lane's computed head,
`.data.unscheduled` for rows without an executable lane, and
`.data.spawnable_heads`. A command failure, a malformed envelope, a non-empty
`blockers_missing`, a self-dependency, or a cycle (`dag` renders one without
flagging it; `issuectl doctor` reports it) invalidates the whole schedule;
select nothing from it. A dependency is satisfied only by a delivering
terminal status, `fixed` or `done` in the default schema; `untriaged`,
`deferred`, `wontfix`, `obsolete`, `cannot-reproduce`, and `duplicate` deliver
nothing. An `in-progress` head is resumable, not excluded: it means started,
and duplicate work is prevented by reservations, not by status.

**Reservations.** issuectl reads nothing from Taskfleet, so you tell it what
in-flight runs already hold. The first read passes `'[]'` (an empty string is
invalid) and doubles as the schema check; it shows the issue-to-lane and
collision mapping but does not drive spawning, because you do not yet know
what is held. Then reconcile the runs this session owns with `run show`, and
look at `taskfleet run list --output json --repo .` only to discover foreign
runs whose source or worktree is this repository (runs without recorded
repository identity do not match). Map each live run to its issue and build
one hold per run from that issue's own metadata:
`[{"lane":"backend","collision":["path/to/hot-file"]}]`. The tokens are opaque
strings copied from the issue; a token inferred from a path or lane name
protects nothing. Validate the JSON and read again with it; only that
reservation-aware response drives spawning. Shell variables do not survive
between tool calls, so assign the hold JSON and call issuectl in the same
command, or pass a file path. A same-repository foreign run that may still own
resources but cannot be mapped to its complete hold means you do not spawn
this round, because omitting the hold, guessing its tokens, or adopting the
run to make it mappable each defeats the protection. A session-owned run in
that state is yours to resolve first.

**Triage state is not scheduling state.** This distinction has been
corrupted once, when review residuals were parked as `deferred` with their
lane metadata intact and issuectl, which treats `deferred` as active-class,
reported them as spawnable heads and serialized a whole lane behind them. The
rules that came out of it: an unaccepted review residual, out-of-plan finding,
or other candidate awaiting the human lane-or-close gate stays
`status: untriaged` with no `lane`, `lane_seq`, or `collision` assignment.
`status: deferred` is reserved for an explicit human or product decision that
accepted, worthwhile work is not for now, and deferred work also stays
unscheduled. Omission from this round authorizes no lifecycle change. A row in
`.data.unscheduled` has no executable lane even if it mechanically says
`spawnable: true`; it becomes launchable only after it passes human triage,
moves to the project's accepted active status, and gains a lane. The reserved
`unlaned` lane is an executable, parallel-safe lane assigned after human
acceptance, not the absence of a lane. A lane member carrying `untriaged`,
`deferred`, or another non-executable disposition is malformed metadata, not
work: report the slug as a scheduling inconsistency and stop before announcing
heads or spawning, even though issuectl may call it spawnable.

**Planning edits.** File any human-accepted unit that is not yet an issue and
move scheduling metadata through the validated CLI, for example
`issuectl update <slug> --lane <lane> --lane-seq <n> --add-blocked-by <blocker> --add-collision <token> --json`,
with the additive flags repeated rather than lists replaced. A disposition
change and its scheduling change move together: on human acceptance for
execution, move the issue from `untriaged` to the accepted active status and
assign its lane metadata; on an explicit "not now", set `deferred` and remove
`lane`, `lane_seq`, and every scheduling `collision` assignment. A live
worker's concurrent status writes race yours, so mutate only issues no
worktree owns and postpone the rest until that worker lands. Commit each
rewritten issue by exact path, never with `git add -A`, and read the DAG again
with the current holds; its computed order and heads are the plan. Units that
touch the same hot file are sequenced in one lane, disjoint units may use
different lanes, and a resource shared across lanes goes in each issue's
`collision` list. When disjointness is unclear, sequence: two independent
units that both land in the same file compile fine alone and break the
integrated `main`.

## Run truth

A spawn returns immediately; `taskfleet run wait <run-id> ...` blocks until
the run settles. Settled is not landed. `run show` can report `failed` or
`pending` for a worker that committed and merged, and `run wait` can settle a
`done` run as `report_only: true`, agent-reported success without a recorded
merge, which is legitimate for an external delivery but is never proof that
source landed. The landing signal is the `landed` Boolean on `run wait` and
`run show`, git-verified against the current target tip by patch-id
equivalence with an ancestry safety net, so it survives your own rebases of
local `main`. Its companion `landed_method` says how it was decided:
`git-verified` is git's positive answer, `report-marker` means the branch is
already torn down and the durable merge marker decided, and `unverified` means
the CLI could not confirm, which is a reason to verify by content, never a
reason to respawn or salvage.

Do not check landing with `git merge-base --is-ancestor <worker-branch>
<target>`. Rebasing local `main` onto `origin/main` replays the worker's merge
under a new hash while the branch ref keeps the old one, so the check says
"not landed" for fully merged content. That fired twice in one real stint and
nearly caused a destructive respawn of landed work; `landed` exists to replace
it. A manual check looks for the expected content on the actual target
(`git diff <base>..<target> -- <paths>`, or the files and symbols themselves),
not for the worker branch and not for a commit subject, which rebase and
squash rewrite.

`preserved_work` is an array on both commands, present even for `done`: any
retained worktree or branch is an open hold, not a finished handoff, and
`run wait --fail-on-error` exits 3 when a `report_only` done run still owns
one (a confirmed merge may still be inside the supervisor's teardown window).
A `failed` run whose worker died after committing carries `recoverable_work`.
Reports persist on the node as `last_report`; read them through the CLI, not
run files. `run show` carries the single-worker report as `data.report`; a
multi-node run needs `node show` per node. `run wait` takes several runs, so
its envelope is `data.runs[]`, and it folds in a summary only; the full
`discussion_items`, `spinoff_proposals`, and `wrap_up_recommendations` are on
`run show` / `node show`.

```bash
# skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
taskfleet run show "$run_id" --output json | jq '.data.report'
# skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
taskfleet node show "$run_id" n-0001 --output json | jq '.data.report // .data.last_report'
# skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
taskfleet run wait "$run_id" --output json | jq '.data.runs[] | {run_id, status, landed, summary}'
```

## Spawning and waiting

| Unit shape | Spawn | Lands |
|---|---|---|
| Clear fix for a filed bug, or a well-scoped task | `/worktree-spinoff --headless <slug or task>` (bare slug or `issuectl:<slug>`; a `#<slug>` with hyphens may not parse) | current branch |
| Architectural choice | `/worktree-technical-decision <task>` (ADR first; implementation is a later issue-driven spinoff) | current branch |
| Hands-on review wanted | `taskfleet run create --kind spinoff --interactive ...` with the same complete brief | current branch after an explicit `run merge` |

Self-merging spinoffs go `--headless`: the worker's window lands in the
detached `headless` tmux session instead of the user's window list, and macOS
runs out of pseudo-terminals around five or six foreground spawns. An
`--interactive` run stays foreground and its supervisor waits for a
deliberate `run merge` or `run cancel`. A dependency graph of several features
is not one unit: issuectl stays the sole DAG owner and dependency-ordered work
runs as bounded waves, never as an integration orchestrator with its own
state.

There is no `--review` flag; the quality bar goes in the brief. Ask the worker
to assess the final diff's risk, complexity, coverage, and existing review
evidence, then choose and explain a proportionate review level in its report.
A small, local, well-covered change may use focused self-review or one
targeted reviewer; security or authorization boundaries, destructive changes,
concurrency, broad refactors, unfamiliar or architecturally significant work,
difficult rollback, or weak coverage warrant stronger independent review
(normally `/llm-review` plus `/assess-findings`). Mandates from the user, the
issue, repository policy, or a calling workflow still win, and adequate
existing evidence is reused unless the run changes the reviewed risk surface.

**Reserve at launch, not at first commit.** Immediately after spawning,
record the run's full id, issue slug, lane, and complete collision-token list
in your own memory, and pass the full current hold array to every later DAG
read. Launch only a lane issue whose own `spawnable` field is true and whose
status is an executable, triaged status in the project's schema. Also exclude
every slug you already hold, because the reservation schema carries resource
tokens, not issue identity, and a hold on `unlaned` reserves no other
`unlaned` issue. Release a hold only when `run show` confirms the run landed,
or an explicit cancel or discard confirms nothing preserved or resumable
remains; `run wait` returning is not release, since awaiting-input and
recoverable runs settle while still owning work. Spawn breadcrumbs and run
status do not go in `TODO.md`.

Launch disjoint units in parallel and sequence hot-file units strictly:
launch, wait, confirm `landed`, then launch the next so it branches from the
first's landed result. Choose the wait shape from what the results are for.
If nothing can use an individual result before the whole batch settles, one
aggregate `taskfleet run wait <id> ...` (default `--all`) is simplest. If a
result can be reviewed, presented, or built on independently, start one
`run wait` per run through the harness's independently watched
background-command facility with a wake on completion, act on each as it
arrives, and leave the others running. The wait is still the public blocking
CLI, not a job API: do not shell-background commands or poll `run show` to
imitate notifications, and if the harness cannot wake you on completion, fall
back to one blocking aggregate wait rather than promise an early wake. Nothing
is deployed until every owned run has settled with `landed` true.

**Worker death is usually transient, and preserved work is not yours to
merge.** A worker can die (`agent-died`, a `failed` run) after committing
clean, mergeable work but before `run merge`; the supervisor preserves the
branch and stamps `recoverable_work`. Its gate and review evidence are
incomplete, so the answer is a fresh `/worktree-spinoff --headless` for the
same issue whose brief names the preserved branch and asks the worker to
inspect and validate the stranded commits, adopt them onto a fresh branch off
current `main`, complete the green gate, choose proportionate review, and
merge. This is a retry, not a model or harness swap: the process died, the
work is fine. One death is not a systemic problem, and a long run is not a
hang: heavy units legitimately run 54 to 96 minutes. `taskfleet run salvage
<run-id>` is the fenced manual finish for a run whose report shows the gate
and review already complete and only the merge call skipped, not for one
whose evidence is incomplete. Once the harvest lands, the dead run's branch
and worktree are orphans, and removing them is a deliberate operator step
(`taskfleet run discard <run-id> --reason <text>`, `--dry-run` first) rather
than something done in passing. Likewise, a clean exit with no merge is
attention-required, not failed: the worker skipped `run merge`, and the work
is still there.

## Deploy

Deploy policy comes from the root `AGENTS.md`. The precondition is that every
owned run has settled with `landed` true and `main` is green under the
project's own gate; a red gate halts deploy, and the fix is a worktree, a
wait, a confirmed landing, and the full gate again. Where the project grants
deploy without asking, typically a test or staging target in an active test
cycle, deploy with its exact command and verify live with its health check.
Where policy requires confirmation, the target is production, or `AGENTS.md`
is silent, ask once and suggest documenting a policy; a wrong production
deploy is the one place a question is clearly cheaper than a guess. Deploys
run one at a time. A project with no deploy step for a stint, where changes
land on `main` and a human promotes later, gets a sentence saying so.

## The report and the next action

Gather the durable facts first: landed commits, closed issues and analyses,
worker reports, gate and deploy evidence, and the full ids of the runs you
own. Then write the product-owner snapshot directly, using
`/worktree-status`'s shape (Summary · Ready to test · Decisions needed ·
Discussion points · Spin-offs) without running a second pass to reformat.

End with one concrete next action, on every return including a hard stop,
and never an "A or B" menu: the specific answer you need from the user; the
exact `taskfleet run wait <full-run-id>` or `run show` for an unsettled owned
run; one exact inspection, repair, retry-with-harvest, or decision action for
a failed validation or preserved work; `/stint-handoff` when nothing is
unresolved after the first report, and `/stint-handoff` regardless after the
one follow-up round. Do not promise an automatic wake; without a wake facility
the copyable next action is what the user has.

Light feedback, a handful of small asks, gets the one follow-up round on the
smaller list, every change still through a worktree. Heavy feedback does not
fit this session's context: land it in the affected issues, documentation,
and `TODO.md`, then move to the handoff. Either way keep the schedule current
before any re-run, through issuectl and committed by exact path, so the next
DAG read sees it.
