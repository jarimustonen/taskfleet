---
name: stint-start
description: "Run one bounded round of a work-session (työrupeama, 'stint') as the ORCHESTRATOR the user talks to: orient → plan → spawn worktrees → deploy when permitted → give a product-owner report and one exact next action. It may absorb one user-feedback follow-up, then stops for explicit `/stint-handoff`. Use for 'aloitetaan rupeama', 'jatketaan @TODO.md', 'start a work session', 'let's do a round', or bare `/stint-start`. Maximally autonomous and generic: reads repository policy, TODO narrative, issuectl DAG, and session-owned run IDs. NOT a worktree, one-off coding task, durable stint/checkpoint, background loop, or terminal handoff."
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# Stint-start — one round of a work session

You are the conductor the user talks to. A stint (työrupeama) is a standing loop: sync
the source branch, plan from the prepared agenda, spawn worktrees that do the coding,
deploy if the project allows, report to the user in product terms, absorb their feedback.
One invocation of this skill runs one bounded round of that loop and ends with one exact
next action. The user's immediate reaction may drive at most one smaller follow-up, run
as the same kind of full round. After that second report the next action is the separate
`/stint-handoff` skill, whatever new ideas came up; larger work is recorded durably for
the next stint instead. Acknowledgement, silence, or an empty frontier is not a request
for another round. There is no persisted stint or checkpoint state and no background
loop: git, issuectl, Taskfleet runs, and `TODO.md` are the only durable owners of facts.

The point of the shape is that this conversation stays free for orchestration and for
talking to the user. Every code change, including a one-line fix, happens in a worktree
you spawn; if you notice yourself about to edit product code here, that is the signal to
spawn one. Editing `TODO.md`, `AGENTS.md`, and issue files is orchestration and is fine.

The skill is generic. Everything project-specific comes from the repository's own root
`AGENTS.md` / `CLAUDE.md` and the `TODO.md` handoff block (`## 🔄 Continue here` /
`ALOITA TÄSTÄ`): the deploy command, target, and whether you may deploy unasked; the
green-gate and live-version commands; hot-file guidance precise enough to map issues into
lanes and collision tokens; migration rules; any test-account reset preference. When a
fact is missing, read it from project docs or git, or make a best-judgment call, say so,
and suggest documenting it. Ask only when the fact is both unresolvable and blocking.
The toolchain is assumed: `issuectl` for issues and scheduling, the `/worktree-*` family
on top of `taskfleet` for workers. Read the `taskfleet-overview` and `worktree-spinoff`
skills before the first spawn; their command surfaces are not repeated here.

## What is at stake

Interrupting the user costs the scarcest thing in the system, their attention. The
handoff left a vetted narrative and issuectl carries the live schedule, so start executing
from those and narrate decisions and state changes rather than deliberation. A question is
worth asking when the possible outcomes differ in a way the user would care about and the
sources cannot tell you which they would choose. The rest of this section describes what
is precious here, so you can weigh that yourself.

**The source branch and its history.** Parallel sessions push to the same remote, so a
force-push destroys someone else's work; there is no situation in a stint that justifies
one. Leaving `main` modified-but-uncommitted across a phase means the next worktree
branches from an ambiguous state.

**Review evidence.** A worker's commits are accepted only through its own `run merge`,
which carries the green gate and the review it chose. Committing or merging a worker's
work from this session, including cherry-picking a dead worker's branch, lands code nobody
vetted and detaches it from its evidence. Unlanded work is reported and preserved, not
rescued by hand.

**Human dispositions.** Which issues are accepted for execution, deferred, or closed is a
product decision. Backlog presence, a prepared narrative that names an issue, omission
from a round, an empty frontier, or a mechanically computed `spawnable: true` is not such
a decision. Manufacturing units from the backlog is how low-value review residuals turn
into wasted worker runs.

**Other sessions' runs.** Runs in this repository that this session did not launch belong
to someone else. Their resources must be reserved so you cannot collide with them, but
waiting on, cancelling, recovering, or reporting them interferes with another conductor.

**The deploy target.** A test or staging server is cheap to redeploy; production is not.
A red gate deployed anywhere costs a debugging round for everyone. The project's policy
says which kind of target you have.

**Worker work in flight.** A concurrent issuectl write to an issue a live worktree owns
races that worker's own status writes. A hold released too early lets a second worker
start on the same files.

## Where the facts live

**Scheduling is `issuectl dag --json --reservations …`, nothing else.** It derives lane
order, dependency state, collision tokens, computed heads, and spawnability from issue
metadata on every read. `TODO.md` is only the handoff narrative; the schedule used to be
maintained in prose there too and the two copies drifted, which is why the graph is never
copied into it or recomputed by hand. If issuectl, `dag`, its `--reservations` option, or
the JSON fields below are unavailable, the project is unmigrated or incompatible: report
that and stop rather than fall back to a prose schedule. Read `.data.lanes[]` for ordered
issues with their dependency, `collision`, and Boolean `spawnable` fields and each lane's
computed head, `.data.unscheduled` for rows without an executable lane, and numeric
`.data.spawnable_heads`. A command failure, a malformed envelope, a missing blocker (a
non-empty `blockers_missing`), a self-dependency, or a cycle (`dag` renders one without
flagging it; `issuectl doctor` reports it) makes the whole schedule invalid; select
nothing from it. A dependency is satisfied only by a delivering terminal status: in the
default schema `fixed` and `done` deliver, while `untriaged`, `deferred`, `wontfix`,
`obsolete`, `cannot-reproduce`, and `duplicate` do not; a project that customizes
statuses names its own equivalents. An `in-progress` head is resumable, not excluded:
`in-progress` means started, and duplicate work is prevented by reservations, not by
status.

**Triage state is not scheduling state.** A row in `.data.unscheduled` has no executable
lane even if it mechanically says `spawnable: true`; never launch it until it has passed
the human lane-or-close gate, moved to the project's accepted active status, and gained a
lane. An unaccepted review residual, out-of-plan finding, or other candidate awaiting that
gate stays `status: untriaged` with no `lane`, `lane_seq`, or `collision` assignment.
Omission from this round alone authorizes no lifecycle change. `status: deferred` is
reserved for an explicit human/product decision that accepted, worthwhile work is not for
now; deferred work also stays unscheduled and is not pulled into a round because the
narrative mentions it. The reserved `unlaned` lane is an executable, parallel-safe lane
value, not the absence of a lane, and is assigned only after human acceptance of work
confirmed parallel-safe. A lane member carrying `untriaged`, `deferred`, or an equivalent
non-executable disposition is malformed metadata, not work: report the slug as a
scheduling inconsistency and stop before announcing heads or spawning, even though
issuectl may mechanically report it spawnable.

**Run truth is Taskfleet's, and only `run merge` is success.** A spinoff is asynchronous;
its spawn call returns immediately, and `taskfleet run wait <run-id> …` blocks until the
run settles. Settled is not landed: `run show` can report a false `failed` or `pending`
for a worker that committed and merged, and `run wait` can settle a `done` run as
`report_only: true`, meaning agent-reported success without a recorded merge, which is
legitimate for an external delivery but is never proof of source landing. The landing
signal is the CLI's `landed` Boolean, on both `run wait` and `run show`. It is
git-verified against the current target tip by patch-id equivalence with an ancestry
safety net, so it survives your rebases of local `main`. Its companion `landed_method`
says how it was decided: `git-verified` means git positively found the work landed or
absent; `report-marker` means git could not run (the branch is already torn down) and the
durable merge marker decided; `unverified` means the CLI could not confirm, which is a
reason to verify by content, never a reason to respawn or salvage. Do not check landing
with `git merge-base --is-ancestor <worker-branch> <target>`: rebasing local `main` onto
`origin/main` replays the worker's merge under a new hash while the worker's branch ref
keeps the old one, so the check answers "not landed" for fully merged content. That trap
fired twice in one real stint and nearly caused a destructive respawn of landed work;
`landed` exists to replace it. If you double-check by hand, look for the expected content
on the actual target the run merged into (`git diff <base>..<target> -- <paths>`, or the
files and symbols themselves), not for the worker branch and not for a commit subject,
which rebase and squash rewrite. When `landed` and a manual check disagree, that is a
reconciliation point to investigate before deploying or salvaging anything.

`run show` and `run wait` expose `preserved_work`, always an array, even for `done`: any
retained worktree or branch is an open hold, not a finished handoff, and
`run wait --fail-on-error` exits 3 when a `report_only` done run still owns one (a
confirmed merge may still be inside the supervisor's teardown window). A `failed` run
whose worker died after committing carries a `recoverable_work` block, which `run wait`
summarizes per run (`recoverable=<n> unmerged commit(s) merge cleanly on <branch>`).
Worker reports persist on the node as `last_report`; read them through the CLI rather
than run files. `run show` carries the single-worker report as `data.report`; a
multi-node run needs `node show` per node:

```bash
# skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
taskfleet run show "$run_id" --output json | jq '.data.report'
# Node-level projection-compatible probe:
# skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
taskfleet node show "$run_id" n-0001 --output json |
  jq '.data.report // .data.last_report'
```

`run wait` accepts several runs, so its envelope is `data.runs[]`, not `data.<field>`;
probe it with `jq '.data.runs[] | {run_id, status, summary}'`. It folds in a summary
only; the full `discussion_items`, `spinoff_proposals`, and `wrap_up_recommendations`
that sequence later lane work are on `run show` / `node show`.

**Ownership is the set of full run IDs this session launches or explicitly adopts.**
That set, not repository membership or the global run list, is what you wait on, recover,
report, and hand to `/stint-handoff`. Adoption means the user or calling workflow names a
full run ID and tells this session to take responsibility, and you state the adoption
before acting on it. Listing, showing, reserving, mentioning, or discovering a run never
adopts it. Ownership ends only when the run has landed or an explicit cancel/abandon path
confirms no preserved or resumable work remains; a terminal status alone is not release.

## The round

### Phase 0 — Orient

Start from a clean, current source branch: verify the checkout is the repository's
normal source branch and both index and worktree are clean, `git fetch`, then
`git merge --ff-only @{upstream}`; when local and remote have ordinary ahead/behind
divergence, `git rebase @{upstream}`. Resolve only clearly mechanical, narrow conflicts
and confirm the result with the repository green gate; abort the rebase and surface
anything semantically ambiguous or broad. Never force-push. A dirty tree, an
incompatible branch policy, a missing upstream, or a failed gate is a genuine stop, since
every worktree this round would branch from that state.

Read the operating policy from `AGENTS.md` / `CLAUDE.md` and the handoff block, then
establish ground truth from git rather than from the handoff's claims: does `main` match
what is live (the project's live-version check)? Is there merged-but-undeployed work, or a
half-finished worktree branch? Handoffs describe the state as it was; git says what it is.

Read the live schedule and reconstruct holds. Check `issuectl dag --help` shows
`--reservations` and `issuectl update --help` shows the mutation flags you will use.
The first read uses an empty hold set and doubles as the schema gate:

```bash
reservations='[]'
issuectl dag --json --reservations "$reservations"
```

It gives you the issue-to-lane and collision mapping and confirms the required fields
exist. It must not drive spawning, because you do not yet know which resources are
already held. Reconcile the full IDs this session already owns with `run show`. Then look
at `taskfleet run list --output json` (its `--repo .` filter narrows to this repository;
runs without recorded repository identity do not match it) only to discover live runs
whose recorded source or worktree belongs here. Those foreign runs are reservations, not
owned work: their resources go into the hold set, and nothing else about them is your
business. Map each relevant live run's issue slug through the first response and build
the exact issuectl hold shape, one object per run even when lanes match:
`[{"lane":"backend","collision":["path/to/hot-file"]}]`. Each object carries the issue's
lane and its complete `collision` array; the tokens are opaque strings copied from issue
metadata, never inferred from paths or lane names, because issuectl reads nothing from
Taskfleet and a guessed token protects nothing. Validate the JSON, then re-run with it;
only this reservation-aware response may drive spawning. When there are no holds, the
second read still passes `'[]'`; `--reservations ""` is invalid. Shell variables do not
survive between tool calls, so assign the hold JSON and invoke issuectl in the same
command, or pass a recorded file path.

A session-owned run you cannot map to its complete hold is in an unresolved state
(awaiting input, recoverable work); resolve that first. A same-repository foreign run that
may still own resources (live, awaiting input, attention-required, recoverable, or
terminal with preserved work) whose complete reservation cannot be identified means you
stop **spawning** as unverifiable, since omitting the hold, inferring its tokens, or
adopting the run to make it mappable each defeats the protection. A failed command,
malformed JSON, or invalid graph stops the round; never retry without reservations or
patch around it in `TODO.md`.

Then orient the user in one tight message: where things stand, what the sync brought in,
the computed head per lane, what is blocked or unscheduled, and what you propose to
tackle this round (fold in the `$ARGUMENTS` focus hint). Proceed without waiting for
permission.

### Phase 1 — Plan

The work list is already prepared: the handoff narrative supplies intent and the
reservation-aware DAG supplies the executable frontier. Fold in the focus hint and
anything the user names this round, and do not re-ask them to confirm a plan they already
supplied. Two situations look similar and are not. A missing or empty handoff block is a
cold start: no human-vetted intent exists, so orient from the computed frontier, say
plainly that there is no prepared narrative, and do one planning pass with the user
before spawning; treating the whole open backlog as the agenda would be invention. A
deliberately empty agenda (the handoff ran and acknowledged nothing) is simply "no ready
work": report that and skip spawn and deploy.

Decompose into independent worktree units and resolve collisions through issue metadata:
units that touch the same hot file are sequenced in one lane, disjoint units may use
different lanes, and a resource shared across lanes belongs in each issue's `collision`
list. When disjointness is unclear, sequence; two "independent" units that both land in
the same file compile fine alone and break the integrated `main`. Classify each unit: a
clear, well-scoped bug or task is a direct autonomous fix; a large or genuinely ambiguous
feature goes design-first.

File any human-accepted planned unit that is not yet an issue and update scheduling
metadata through the validated CLI, for example
`issuectl update <slug> --lane <lane> --lane-seq <n> --add-blocked-by <blocker> --add-collision <token> --json`,
repeating the additive flags rather than replacing lists. A disposition change is also a
scheduling change and the two move together: on human acceptance for execution, move the
issue from `untriaged` to the project's accepted active status and assign its lane
metadata; on an explicit human/product “not now” decision, set `deferred` and remove
`lane`, `lane_seq`, and every scheduling `collision` assignment. Never leave a
non-executable disposition holding a lane. Only mutate an issue that no live worktree
owns; postpone the rest until that worker lands. Commit each rewritten issue path by exact
name, never `git add -A`, verify a clean tree, and re-run
`issuectl dag --json --reservations "$reservations"`; its computed order, blockers,
heads, and spawnability are the plan. Announce the plan in one short message, saying
what runs in parallel and what is sequenced, and proceed unless something is truly
ambiguous.

### Phase 2 — Orchestrate

Each unit has an explicit landing contract; know before launch where it lands and
confirm the actual landing from `landed` before counting it toward the deploy pile.

| Unit shape | Spawn | Lands |
|---|---|---|
| Clear fix for an already-filed bug | `/worktree-spinoff --headless <slug>` (use the bare slug or `issuectl:<slug>`, not `#<slug>`, since hyphenated slugs are not guaranteed to parse behind a `#`) | current branch (main) |
| Well-scoped autonomous task | `/worktree-spinoff --headless <task>` | current branch (main) |
| Architectural choice | `/worktree-technical-decision <task>` (ADR first; implementation is a later issue-driven spinoff) | current branch (main) |
| Hands-on review required | `taskfleet run create --kind spinoff --interactive …` with the same complete worker brief | current branch after explicit `run merge` |

Every self-merging spinoff passes `--headless`: the worker's window goes to the detached
`headless` tmux session instead of the user's window list, and macOS runs out of
pseudo-terminals around five or six foreground spawns. Attach with
`tmux attach -t headless` when curious; cleanup still closes each window on terminal. An
explicitly `--interactive` run stays foreground and the supervisor waits for a deliberate
`run merge` or `run cancel`. There is no separate bugfix kind; a filed bug is an
issue-driven spinoff. A dependency graph of several features is not one unit either:
issuectl stays the sole DAG owner and dependency-ordered work runs as bounded stint waves,
never as an integration orchestrator with its own state.

The worker chooses proportionate review after seeing the final diff; there is no
`--review` flag, so put the quality bar in the brief: assess the diff's risk, complexity,
test coverage, and existing review evidence, then choose and explain a proportionate
review level in the terminal report. A small, local, well-covered change may use focused
self-review or one targeted reviewer; security or authorization boundaries, destructive
changes, concurrency, broad refactors, unfamiliar or architecturally significant work,
difficult rollback, or weak coverage warrant stronger independent review (normally
`/llm-review` plus `/assess-findings`). Explicit mandates from the user, issue,
repository policy, or calling workflow still win. Reuse adequate existing evidence unless
the run materially changes the reviewed risk surface.

Launch disjoint units in parallel, recording each spawn's full run ID, and choose the
wait shape from the batch's dependencies and review needs: if no review, decision, next
wave, or integration action can use an individual result before the whole batch settles,
use one aggregate `taskfleet run wait <id> …` (default `--all`). If any result can be
reviewed, presented, or used independently, start one `taskfleet run wait <id>` per run
through the harness's independently watched background-command facility. Give each
waiter a completion/failure wake notification; when one wakes, inspect that run's result
and report, act on what is usable, and leave the other waiters running. A background
wait is still the public blocking CLI, not a Taskfleet job API: do not shell-background
commands or poll `run show`/process output to imitate notifications. If the harness
cannot notify on independent command completion, fall back to one blocking aggregate
`run wait --all` rather than promise an early wake. Hot-file units are sequenced
strictly: launch, `run wait`, confirm `landed`, then launch the next so it branches from
the first's landed result. Do not enter Phase 3 until **every owned run** has settled and
each `landed` flag is true; an early result never waives that barrier. A worker that did
not land its merge is reported, and `main` stays clean.

**Reserve at launch, not at first commit.** Immediately after spawning, add the run's
issue slug, run ID, lane, and complete collision-token list to conductor memory, and pass
the full current hold array to every later DAG read in the same shell invocation. Launch
only a lane issue whose own `spawnable` field is true **and** whose status is an
executable, triaged status in the project's schema; a lane-assigned `untriaged` or
`deferred` row is the inconsistency described above, not a head. Also exclude every slug
already held in conductor memory, because the reservation schema carries resource tokens,
not issue identity; and remember that a hold on `unlaned` reserves no other `unlaned`
issue, only its collision tokens and its slug. Release a hold only after `run show`
confirms the run landed, or an explicit cancel/abandon path confirms nothing preserved or
resumable remains. `run wait` returning is not release: awaiting-input and recoverable
runs settle while still owning work, and awaiting-input requests are resolved before you
select again. Spawn breadcrumbs and run status do not go in `TODO.md`; issue status
belongs to the worker and live holds to conductor memory.

**Worker death is usually transient, and the preserved work is not yours to merge.** A
worker can die (`run wait` / `run show` report an `agent-died` `failed` run) after
committing clean, mergeable work but before `run merge`. The supervisor preserves the
branch and stamps `recoverable_work` on the report. Its gate and review evidence are
incomplete, so it is not merged from here. Retry with harvest: a fresh `/worktree-spinoff --headless`
for the same issue whose brief names the preserved branch and asks the worker to inspect
and validate the stranded commits, adopt them onto a fresh branch off current `main`,
complete the green gate, choose proportionate review (reusing recorded evidence where the
risk surface is unchanged), and merge. This is a retry, not a base-agent swap; the
model and harness are fine, the process died. One death is not a systemic problem, and a
long run is not a hang: heavy-LLM units (design-first, multi-round review) legitimately
run 54–96 minutes, so keep waiting. `taskfleet run salvage <run-id>` is the fenced
manual finish that drives `run merge` from the preserved worktree as it stands; it fits
an attention-required run whose report shows the gate and review already completed and
only the merge call was skipped, not a run whose evidence is incomplete. Once the harvest
has landed, the dead run's preserved branch and worktree are orphans; removing them is a
deliberate operator step (`taskfleet run discard <run-id> --reason <text>`, with
`--dry-run` first), not something the conductor does in passing. Salvage of a genuinely
dead worktree that nothing re-landed is likewise the user's deliberate call.

### Phase 3 — Deploy

Deploy is conditional on project policy from the root `AGENTS.md`. The precondition is
that every session-owned run has settled with `landed` true and the pile on `main` is
green under the project's own gate commands. A failed gate halts the deploy: report it,
spawn a fix worktree, wait, confirm its landing, and re-run the full gate before
reconsidering. If the project grants deploy-without-asking (typical in an active test
cycle against a test or staging server), deploy directly with the project's exact
command, including any required environment, flags, and post-deploy steps, then verify
live with the project's health or smoke check and report the outcome. If policy requires
confirmation, the target is production, or `AGENTS.md` is silent on autonomy, ask once
and suggest documenting a deploy-autonomy policy; the cost of a wrong production deploy
is the one place a question is clearly cheaper than a guess. Deploys run one at a time.
If the project has no deploy step for a stint (changes land on `main` and a human
promotes later), say so and move on.

### Phase 4 — Report and one next action

The coding happened out of sight, so gather the durable facts first: landed commits,
closed issues and analyses, worker reports, green-gate and deploy evidence, and the full
IDs of runs this session owns. Then write the product-owner snapshot directly, using
`/worktree-status`'s presentation discipline (Summary · Ready to test · Decisions
needed · Discussion points · Spin-offs) without running a second pass merely to
reformat. The report is output, not a question; always deliver it.

End with exactly one concrete next action. This rule applies to **every** return from
Phases 0–3, including a hard stop, which may skip the report formatting but still ends
with one command or one specific requested decision, never an "A or B" menu:

- unresolved test or decision feedback: ask for the specific answer(s); that reply may
  drive the single bounded follow-up below;
- a session-owned unsettled run: the exact `taskfleet run wait <full-run-id>` or
  `run show` command;
- failed validation, ambiguous ownership, or preserved work: one exact inspection,
  repair, abort, retry-with-harvest, or decision action, with the failing command where
  relevant;
- no unresolved feedback after the first report: `/stint-handoff`;
- after the one feedback follow-up report: `/stint-handoff` regardless of new work ideas.

Do not promise an automatic wake. A harness may wait on the public command, but without
a wake facility the copyable next action is what the user has.

### Feedback

The snapshot hands the user things to act on, and their reaction decides what happens
next. Light feedback, a handful of small asks, gets one fresh Phase 0–4 pass on the
smaller work list if this invocation has not yet spent its follow-up; it is a full round,
every change still goes through a worktree, and its report's only next action is
`/stint-handoff`. If the follow-up is already spent, record the asks durably for the next
stint. Heavy feedback does not fit in this session's context: land it in the affected
issues, documentation, and `TODO.md` first, then move to the handoff. Either way keep
scheduling current before any re-run: new issues or changed dependencies go through
issuectl and are committed by exact path before the re-run reads the DAG, and even a
captured-without-re-run round records the metadata so the handoff sees the live graph.

`/stint-handoff` is a separate skill and never runs merely because this round went
green; the user invokes it.

## Non-goals

- Not a worktree, and does not create one directly; it delegates to the `/worktree-*`
  family.
- Does not write code in this session.
- Not for a single one-off coding task; that is `/worktree` (the router).
- Not the terminal handoff or wrap-up; that is explicit `/stint-handoff`.
- No durable stint/checkpoint state, new lifecycle noun, umbrella skill, or background
  loop. Existing run, issue, git, and TODO facts remain the only durable owners.
- Hardcodes no project facts; reads them from the repository's `AGENTS.md` / `TODO.md`.
