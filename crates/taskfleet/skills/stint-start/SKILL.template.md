---
name: stint-start
description: "Run one bounded round of a work-session (työrupeama, 'stint') as the ORCHESTRATOR the user talks to: orient → plan → spawn worktrees → deploy when permitted → give a product-owner report and one exact next action. It may absorb one user-feedback follow-up, then stops for explicit `/stint-handoff`. Use for 'aloitetaan rupeama', 'jatketaan @TODO.md', 'start a work session', 'let's do a round', or bare `/stint-start`. Maximally autonomous and generic: reads repository policy, TODO narrative, issuectl DAG, and session-owned run IDs. NOT a worktree, one-off coding task, durable stint/checkpoint, background loop, or terminal handoff."
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# Stint-start — the work-session round engine

You are the **orchestrator the user talks to**. A stint (työrupeama) is one round of
the standing loop: *pull → plan → spawn worktrees that do the coding → one deploy →
report to the user → absorb feedback.* You conduct; **you do not write feature code in
this session.** The actual implementation happens in worktrees you spawn, so this
conversation's context stays free for orchestration and for talking to the user.

One invocation runs **one bounded round** through the product-owner report. It then emits
**one exact next action** based on the observed stop reason. The user's immediate feedback
may trigger at most one smaller follow-up, executed as the same full Phase 0–4 pass; after
that second report, stop and direct the user to the explicit terminal
**`/stint-handoff`**. Never turn acknowledgement, an empty frontier, or user silence into
another round. There is no persisted stint/checkpoint state, new lifecycle noun, umbrella
skill, or background agent loop.

This skill is **generic**. Every project-specific fact — the deploy command, whether
you may deploy without asking, the green-gate commands, hot files, the test-account
reset preference — is read from the **repo's own `AGENTS.md` and `TODO.md`**. If a
needed fact is missing, **prefer resolving it yourself**: read it from `AGENTS.md` /
`TODO.md` / git, or log a best-judgment decision and proceed (bold first, ask later),
and note it should be documented. Ask the user only when the fact is genuinely
unresolvable *and* blocking (see *Autonomy*). It assumes this toolchain —
**`issuectl`** for issues and the **`/worktree-*`** family (`taskfleet` underneath)
for workers — and is a layer on top of them. Read `taskfleet-overview` and
`worktree-spinoff` before your first spawn.

Scheduling requires **`issuectl dag --json`** with `--reservations`. It is the sole
source for lane order, dependency state, collision tokens, computed heads, and
spawnability. `TODO.md` is only the handoff narrative: never infer or maintain scheduling
structure there. If issuectl, the DAG command, its required JSON fields, or reservation
support is unavailable, stop and report an unmigrated or incompatible project. Never
fall back to a prose schedule.

## Standing discipline (holds across every phase)

- **Orchestrate, don't code.** Every code change — feature, bugfix, or one-line
  trivia — goes through a worktree, never this session. If you catch yourself about
  to edit product code, stop and spawn a worktree. (Editing `TODO.md`, `AGENTS.md`,
  and issue files as part of orchestration is fine — that's not product code.)
- **Keep main clean; worktrees own their commits.** Parallel worktrees branch off
  main's current state. Never leave main modified-but-uncommitted across a phase, and
  **never commit a worker's in-progress work for it** — each worktree commits its own
  changes. If a worker didn't land, report it; do not rescue it by committing on main.
- **Read scheduling from issuectl.** After Phase 0 reconstructs holds, every work-selection
  read must use `issuectl dag --json --reservations "$reservations"`. A bootstrap read
  with `[]` may resolve hold metadata but must never drive spawning until run enumeration
  proves the hold set is actually empty. Read `.data.lanes[]` for ordered issues and
  computed heads, each issue's dependency, collision, and `spawnable` fields,
  `.data.unscheduled`, and numeric `.data.spawnable_heads`. An `in-progress` head remains
  resumable; reservations, not status, prevent duplicate work. Never copy this graph into
  `TODO.md` or recompute what the command already derives. A command failure, malformed
  envelope, missing blocker, self-dependency, or cycle makes the schedule invalid: stop
  before selecting or spawning work. A dependency is satisfied only by a delivering
  terminal status. In the default schema, `fixed` and `done` deliver; `untriaged`,
  `deferred`, `wontfix`, `obsolete`, `cannot-reproduce`, and `duplicate` do not. Use the
  consuming project's equivalents if it customizes statuses.
- **Triage state is not scheduling state.** An entry in `.data.unscheduled` has no
  executable lane, even if that row mechanically says `spawnable: true`; never launch it
  until it has passed the human lane-or-close gate, moved to the project's accepted active
  status, and gained a lane. An unaccepted review residual, out-of-plan finding, or other
  candidate awaiting that gate stays `status: untriaged` with no `lane`, `lane_seq`, or
  `collision` assignment. Omission from this round alone authorizes no lifecycle change.
  `status: deferred` is reserved for an explicit human/product decision that an accepted
  worthwhile item is “not now”; deferred work also stays unscheduled. The reserved
  `unlaned` lane is an executable, parallel-safe lane value, not the absence of a lane; use
  it only after human acceptance when the work is confirmed parallel-safe. A lane member
  carrying `untriaged`, `deferred`, or the consuming project's equivalent non-executable
  disposition is malformed metadata, not work: never announce or launch it as a head,
  even if issuectl mechanically reports `spawnable: true`; report the inconsistency and
  stop before selecting from that schedule.
- **Autonomous spinoffs run headless.** Every self-merging spinoff you spawn directly
  (`/worktree-spinoff`) passes `--headless`, so the round's workers land in the detached
  `headless` tmux session instead of cluttering the user's window list; attach with
  `tmux attach -t headless` only when curious. Auto-cleanup still closes each window on
  terminal. An explicitly interactive `taskfleet run create --kind spinoff --interactive`
  stays foreground and waits for deliberate `run merge` or `run cancel`.
- **Own explicit run IDs; sync them with `run wait`; trust `landed`.** A spinoff runs
  **asynchronously** — its spawn call returns immediately. Keep the full ID of every run
  this session launches or explicitly adopts; that set, not repository membership or the
  global run list, defines this stint's ownership and is handed to `/stint-handoff`.
  Adoption means the user or calling workflow names a full run ID and explicitly tells this
  session to take responsibility, and the conductor states that adoption before acting.
  Listing, showing, reserving, mentioning, or discovering a run never adopts it. Block on
  `taskfleet run wait <run-id> …` for owned runs to know they have *settled* before
  you sequence the next unit or enter Phase 3. But do **not** trust run *status* as proof
  the work landed: `taskfleet run show` can report a false `failed` / `pending` even
  when the worker committed **and** merged. **To confirm a landing, read the CLI's
  `landed` boolean** (surfaced by both `run wait` and `run show`). It is git-verified against
  the *current* target tip — patch-id equivalence plus an ancestry safety net — so it stays
  correct after you rebase local `main`. The companion `landed_method` tells you the evidence:
  `git-verified` (git decided), `report-marker` (git could not run — branch already torn
  down — so the durable `run merge` marker decided), or `unverified`. Settled ≠ landed; the
  `landed` flag is the landed signal.
  - **⚠️ Do NOT git-verify with `git merge-base --is-ancestor <worker-branch> <target>`.**
    In a busy repo you rebase local `main` onto `origin/main` every round; that **replays
    the worker's merge under a new hash** while the worker **branch ref stays at its
    pre-rebase hash**, so `--is-ancestor` returns a **false "not landed"** even though the
    content is fully merged. This trap fired twice in one real stint and nearly triggered a
    destructive re-spawn / hand-salvage of already-merged work. The CLI `landed` flag exists
    precisely to replace this check.
  - **`landed: false` is not always "not landed."** If `landed_method` is `git-verified`,
    trust it — git positively found unlanded work (or genuine absence). If it is `unverified`,
    the CLI *could not confirm* (missing inputs, transient git error) — do **not** auto-respawn
    or salvage on that alone; verify by content first.
  - If you must double-check by hand, verify by **content on the actual target the run merged
    into** (usually local `main`, or the integration branch for an orchestrated child) — never
    by the worker branch ref. Check for the expected files/symbols on that target, or the
    intended diff (`git diff <base>..<target> -- <paths>`); a `git log … | grep <subject>` is
    weak (subjects change under rebase/squash and can collide). If `landed` and your manual
    check disagree, treat it as a reconciliation point — block and investigate rather than
    auto-deploying or auto-salvaging.
- **Read every settled worker's report before planning the next wave.** The persisted
  projection field is `last_report`; use the read surface rather than opening run files.
  For a single-worker run, `run show` has `data.report`. Multi-node runs must
  inspect each node:

  ```bash
  # skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
  taskfleet run show "$run_id" --output json | jq '.data.report'
  # Node-level projection-compatible probe:
  # skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
  taskfleet node show "$run_id" n-0001 --output json |
    jq '.data.report // .data.last_report'
  ```

  `run wait` can return several runs, so its envelope is `data.runs[]`, not
  `data.<field>`: use `jq '.data.runs[] | {run_id, status, summary}'` and never
  `.data.status`. It folds in `summary`; use `run show`/`node show` for the full
  `discussion_items`, `spinoff_proposals`, and `wrap_up_recommendations` needed
  to sequence later lane work.
- **One deploy at a time.** Never parallel deploys.
- **Ask conversationally.** Never `AskUserQuestion` (global CLAUDE.md).

## Autonomy

Autonomy is **maximal: just go**. `/stint-handoff` has left a prepared narrative and
issuectl carries the live schedule, so **trust those sources and start executing**. Do not
re-derive or re-confirm the plan with the user.
Run orienting → planning → orchestration → deploy → report autonomously; narrate state
changes and decisions, not internal deliberation. When a fact is missing, prefer reading
it or logging a best-judgment decision and proceeding, rather than asking. Pause only
for: (a) a genuine fork the handoff could **not** have resolved and where a wrong call
would be costly to undo, (b) deploy go/no-go **if** the project has not pre-authorised
deploys (see Phase 3), (c) the transition to handoff/wrap-up, which is a separate skill
(`/stint-handoff`) you propose and run only on the user's go. The product-owner status
report (Phase 4) is **output, not a question** — always deliver it.

"Prefer best-judgment and proceed" governs **reversible scheduling / implementation-detail**
choices only. It never overrides these hard stops — halt or pause, don't guess:
- a **missing green-gate or migration command** for work that needs it (don't skip the gate);
- a **deploy target/autonomy** that `AGENTS.md` leaves ambiguous (Phase 3's rule wins);
- an **ambiguous file collision** — sequence the units, never guess parallel (Phase 1);
- the **landing-verification** warning — a `landed`/manual-check disagreement blocks, and
  `landed_method: unverified` is never grounds to auto-respawn or auto-deploy;
- **cold start** with no prepared plan (Phase 1) — a single planning pass, not invention;
- **human disposition** — never infer acceptance, closure, or deferral from backlog
  presence, prepared narrative, omission from a round, an empty frontier, or mechanical
  spawnability. Only an explicit human/product disposition can move a candidate through
  the lane-or-close gate.

## Phases

### Phase 0 — Orient (bootstrap)

1. **Synchronize the clean source branch.** Verify the current branch is the repository's
   normal source branch and both index and worktree are clean. Run `git fetch`, then try
   `git merge --ff-only @{upstream}`. If local and remote have ordinary ahead/behind
   divergence, run `git rebase @{upstream}` on the clean source branch. Resolve only clearly mechanical, narrow
   conflicts and verify the result with the repository green gate; abort the rebase and
   surface semantically ambiguous or broad conflicts. Never force-push. A dirty tree,
   incompatible branch policy, missing upstream, or failed verification is a hard stop.
2. **Read the operating policy** from the repo's root `AGENTS.md` and `CLAUDE.md`, and
   the `TODO.md` handoff block (`## 🔄 Continue here` / `ALOITA TÄSTÄ`). Gather the deploy
   command and autonomy, deploy target, green-gate commands, live-version check, hot-file
   guidance, migration rules, and test-account reset preference. Hot-file guidance must
   name shared files or file families precisely enough to map issue work into serial lanes
   and cross-lane collision tokens. Deploy policy must state the exact command, target,
   and autonomy; the live-version check and green gate must be executable commands. If a
   required fact is missing, resolve it from project docs or git where possible; only ask
   when it is both unresolvable and blocking, and recommend documenting it.
3. **Establish ground truth from git**, not from the handoff's claims: is `main` equal to
   what's deployed (use the project's live-version check)? Is there merged-but-undeployed
   work or a half-finished worktree branch? Compare `git log --oneline` with the handoff's
   stated live image or version.
4. **Read the live schedule and reconstruct holds.** Verify the prerequisite first with
   `issuectl dag --help` and verify scheduling mutations with `issuectl update --help`.
   Set `reservations='[]'` and run the command once to obtain the
   issue-to-lane/collision mapping:

   ```bash
   reservations='[]'
   issuectl dag --json --reservations "$reservations"
   ```

   This bootstrap read is also the schema gate: confirm `.data.lanes[]`,
   `.data.unscheduled`, and numeric `.data.spawnable_heads` exist, and lane issues carry
   `collision` arrays plus Boolean `spawnable` values. If any field is absent, stop as
   incompatible rather than guessing. If an `untriaged`, `deferred`, or equivalent
   non-executable disposition appears in `.data.lanes[]`, report its slug as a scheduling
   inconsistency and stop before announcing heads, selecting, or spawning; lane presence
   and `spawnable: true` do not prove human acceptance. Reconcile the full IDs already
   launched or explicitly adopted by this session with `run show`. Separately inspect
   `taskfleet run list --output json` only to discover live runs whose recorded source or
   worktree belongs to this repository. Such foreign runs are **reservations, not owned
   work**: include their resources below so this session cannot collide with them, but do
   not wait for, cancel, recover, report, or copy them into this stint's handoff. Map every
   relevant live run's issue slug through the first DAG response, then replace
   `reservations` with the exact issuectl hold-array shape, one object per run:
   `[{"lane":"backend","collision":["path/to/hot-file"]}]`. Two live runs are two array
   objects, even when their lanes match. Include the issue's lane and its complete
   `.collision` array; collision tokens are exact opaque strings copied from issue
   metadata, not inferred paths or lane names. Re-run with that payload; only this
   reservation-aware response may drive spawning. If there are no holds, the second read
   still uses `[]`. Validate assembled JSON before use. If a **session-owned** run cannot
   be mapped to its complete hold, resolve its awaiting-input, retry-with-harvest,
   cancellation, or other ownership state first. It relinquishes ownership only after it
   lands or a terminal cancel/abandon path confirms no preserved or resumable work remains.
   An unrelated run with an identifiable reservation remains foreign and does not block.
   If a same-repository foreign run may still own resources (live, awaiting input,
   attention-required, recoverable, or terminal with preserved work) but its complete
   reservation cannot be identified, stop **spawning** as unverifiable; never omit the hold,
   infer opaque collision tokens, or adopt/manage the run merely to make it mappable. If the command fails, its JSON
   is malformed, or the graph is invalid, stop; never retry without reservations or patch
   around the failure in `TODO.md`.
5. **Orient the user** in one tight message: where things stand, what the pull brought
   in, the computed head per lane, what is blocked or unscheduled, and what you propose to
   tackle this round (fold in the `$ARGUMENTS` focus hint). Then proceed without waiting
   for permission to start.

### Phase 1 — Plan the round

The work-list is **already prepared**: use the `## 🔄 Continue here` narrative for intent
and a reservation-aware DAG read for the current executable frontier. Shell variables do
not survive between tool calls: for every read, assign the current JSON and invoke issuectl
in the same shell command, or pass a recorded reservation-file path. Never emit
`--reservations ""`; pass `'[]'` explicitly when run enumeration proves there are no
holds. Fold in the `$ARGUMENTS` focus hint and any items the user explicitly names this
round, but do **not** re-ask the user to confirm the plan the handoff already supplied.

- **Cold start (no prepared narrative).** If the handoff block is missing or empty, do not
  invent a plan and do not silently treat the entire open backlog as this round's agenda.
  Orient from issuectl's computed frontier, state plainly that no prepared narrative
  exists, and do one planning pass with the user before spawning. This is a legitimate
  pause because no human-vetted intent exists.
- A **deliberately empty** prepared agenda (handoff ran, acked nothing, no active work) is
  not a cold start — report "no ready work" and skip spawn/deploy rather than manufacturing
  units from the backlog.

Then:

- **Decompose** into independent worktree units.
- **Resolve file collisions through issue metadata.** Units that touch the same hot file
  must be sequenced in one lane; disjoint units can use different lanes. Shared resources
  across lanes belong in each issue's `collision` list. If disjointness is unclear,
  sequence the units.
- **Update issue metadata for the round.** File any human-accepted planned unit that is
  not yet an issue. Use the validated CLI operations, for example `issuectl update <slug>
  --lane <lane> --lane-seq <n> --add-blocked-by <blocker> --add-collision <token> --json`;
  repeat the additive flags as needed rather than replacing lists. Never edit a second
  scheduling copy in prose. Only mutate an issue that is not yet owned by a live worktree
  because a concurrent issuectl write races that worker; otherwise postpone the metadata
  change until it lands. Do not schedule an unaccepted residual, out-of-plan finding, or
  candidate merely because it appears in `.data.unscheduled`, says `spawnable: true`, or
  is named in prepared intent; leave it `untriaged` and free of lane, lane-sequence, and
  collision assignments until the human lane-or-close gate accepts it. A disposition
  change is also a scheduling change: on human acceptance for execution, move the issue
  from `untriaged` to the project's accepted active status and assign its lane metadata;
  on an explicit human/product “not now” decision, set `deferred` and remove `lane`,
  `lane_seq`, and every scheduling `collision` assignment. Use only operations advertised
  by the validated `issuectl update --help`, perform the status and scheduling changes
  together, and never leave a non-executable disposition holding a lane. The live-worktree
  guard above still applies. Omission from this round authorizes neither transition. A
  deferred issue remains unscheduled and is not pulled into a round merely because
  prepared intent names it—the human/product disposition must first change. Commit every
  rewritten issue path by exact name, never `git add -A`, before Phase 2 and verify a clean
  tree. Re-run `issuectl dag --json --reservations "$reservations"` and use its computed
  order, blockers, heads, and spawnability as the plan.
- **Classify each unit:** a clear, well-scoped bug/task → direct autonomous fix; a
  big or genuinely ambiguous feature → design-first.
- **Announce the plan** in one short message (which units, what's parallel vs
  sequenced). Proceed unless something is truly ambiguous.

### Phase 2 — Orchestrate (spawn worktrees; never code here)

Spawn the right worktree skill per unit. Each unit has an **explicit landing
contract** — know before launch where it lands, and **verify the actual landing from
git** before counting it toward the deploy pile:

| Unit shape | Spawn | Lands |
|---|---|---|
| Clear fix for an already-filed bug | `/worktree-spinoff --headless <slug>` (issue-driven; use the bare slug or `issuectl:<slug>`, **not** `#<slug>` because hyphenated slugs are not guaranteed to parse behind a `#`) | current branch (main) |
| Well-scoped autonomous task | `/worktree-spinoff --headless <task>` | current branch (main) |
| Architectural choice | `/worktree-technical-decision <task>` (ADR first; implementation is a later issue-driven spinoff) | current branch (main) |
| Hands-on review required | `taskfleet run create --kind spinoff --interactive …` with the same complete worker brief | current branch after explicit `run merge` |

- **Autonomous spinoffs are headless** (see Standing discipline) — pass `--headless`
  on every `/worktree-spinoff`. Explicit `--interactive` runs stay foreground and do not
  auto-terminalize.
- **The worker chooses proportionate review after seeing the final diff.** Put this
  quality bar in the brief (there is no `--review` passthrough flag): assess the final
  diff's risk, complexity, test coverage, and existing review evidence; choose and explain
  a proportionate review level in the terminal report. A small, local, well-covered change
  may use focused self-review or one targeted reviewer. Use stronger independent review
  (normally `/llm-review` plus `/assess-findings`) for security/privacy or authorization
  boundaries, destructive changes, concurrency, broad refactors, unfamiliar or
  architecturally significant work, difficult rollback, or weak test coverage. Explicit
  mandates from the user, issue, repository policy, or calling workflow still win. Reuse
  adequate existing evidence unless this run materially changes the reviewed risk surface.
- **An already-filed bug uses `/worktree-spinoff --headless <slug>`.** There is no
  separate bugfix kind or skill.
- **A multi-feature dependency graph is not one Phase-2 unit.** Keep issuectl as the sole
  DAG owner and execute dependency-ordered work as bounded stint waves. Do not create an
  integration orchestrator or second campaign state.
- **Launch disjoint units in parallel, then wait.** Record each spawn's run id; after a
  parallel batch, block on `taskfleet run wait <id> …` and confirm each landing via the
  CLI's `landed` flag before counting it (NOT `merge-base --is-ancestor` — see the landing
  warning above). **Sequence hot-file units strictly:** launch → `run wait` → confirm
  `landed` → *then* launch the next (so it branches off the first's landed result).
  Do not enter Phase 3 until every launched run has settled and its `landed` flag is true.
  If a worker doesn't land its merge, **report it and leave main clean** —
  do not commit its work yourself. Salvage of a genuinely-dead worktree is a deliberate,
  separate manual step the user oversees, not an automatic conductor action.
- **Recoverable worker death → retry-with-harvest, never hand-merge.** A worker can die
  (`run wait` / `run show` report an `agent-died` **failed** run) after it committed clean,
  mergeable work but before it called `run merge`. The supervisor preserves that branch and
  stamps a `recoverable_work` block onto the failed report, which `run wait` surfaces per
  run (`recoverable=<n> unmerged commit(s) merge cleanly on <branch>` when
  `recoverable: true`, `merges_cleanly: true`, `unmerged_commits > 0`). When you see that:
  - **Do NOT hand-merge the preserved branch from this session.** Those commits are
    not yet accepted as complete — their green-gate and review evidence may be incomplete —
    and merging them yourself both breaks "never commit a worker's work for it" and lands
    unvetted code. Cherry-picking or
    `git merge`-ing it here is the wrong move.
  - **Re-spawn a fresh worktree pointed at the preserved branch** — a `/worktree-spinoff
    --headless` for the *same issue* whose brief names the preserved branch and instructs
    it to: inspect and validate the stranded commits, **adopt** them (cherry-pick / re-apply
    onto a fresh branch off current main), complete the green gate, and merge. Reuse recorded
    review evidence; select and explain proportionate additional review after the final diff,
    repeating stronger review only when evidence is inadequate or the risk surface changed.
    This is **retry-with-harvest**: a
    fresh reviewing agent finishes the dead worker's work — **not** a hand-merge, and
    **not** a base-agent swap (the model/harness is fine; the process just died).
  - **Deaths are transient — the retry usually lands.** Don't infer a systemic problem from
    one `agent-died`; re-spawn and let it run. And a **long** run is not a hang:
    heavy-LLM units (design-first, multi-round review) legitimately run **54–96 min**, so
    keep waiting on `run wait` rather than assuming a second death.
  - **After the harvest lands, the superseded preserved branch/worktree is an orphan.**
    Once the retry has git-verified-merged the same work, the original dead worker's branch
    and worktree are safe to remove — but that removal is a **deliberate, human-overseen
    cleanup**, not an automatic conductor action (the intended `run salvage` command will
    fold this in; until it ships, retry-with-harvest is the manual stand-in).
- **Reserve at launch, not at first commit.** Immediately after spawning, add that run's
  issue slug, run id, lane, and complete collision-token list to conductor memory; the
  JSON passed to issuectl contains each hold's `lane` and `collision` fields. Before every
  subsequent pick, materialize the complete current hold array and pass it in the same
  shell invocation, for example `reservations='[...]'; issuectl dag --json --reservations
  "$reservations"`. Launch only a lane issue whose own `spawnable` field is true **and**
  whose status is an executable, triaged status in the consuming project's schema; never
  launch `untriaged`, `deferred`, or an equivalent non-executable disposition even if it
  is lane-assigned and mechanically spawnable. Treat that combination as the scheduling
  inconsistency defined above. Separately exclude every slug already held in conductor
  memory because the reservation schema carries resource tokens, not issue identity.
  issuectl treats `unlaned` as parallel-safe, so a hold carrying that lane does
  not reserve other `unlaned` issues; collision tokens and the separate slug guard still
  apply. Do not release a hold merely because `run wait` returned: awaiting-input or
  recoverable runs can settle while retaining resumable work. Resolve awaiting-input
  requests before selecting again. Release a hold only after `run show` confirms the run
  has landed, or an explicit cancel/abandon path confirms no preserved or resumable
  ownership remains. This closes both the pre-commit and
  attention-required windows. Do not write spawn breadcrumbs or run status to `TODO.md`;
  issue status belongs to the worker and live holds belong to conductor memory.

### Phase 3 — Deploy (the conductor owns this — when the project permits)

Deploy is **conditional on project policy**, read from the repo's root `AGENTS.md`:

- **Precondition:** the pile is in `main` and **green** — run the project's green-gate
  commands first (typecheck/build/smoke). If a gate fails, **halt the deploy, report
  the failure, and spawn a fix worktree** for it (then wait, git-verify its landing, and
  re-run the full green gate before reconsidering the deploy); do not deploy red.
- **Deploy autonomy:** if the project grants deploy-without-asking (typical in an
  active test-cycle, where deploy targets a **test/staging server**, not production),
  deploy directly. If it requires confirmation, targets production, or `AGENTS.md` is
  **silent** on autonomy, ask once and suggest documenting a deploy-autonomy policy.
- Run **one** deploy with the project's exact command from `AGENTS.md` (including any
  required env export, flags, and post-deploy steps). Never parallel. Then **verify
  live** (the project's health check / smoke) and report the outcome.

If the project has no deploy step for a stint (e.g. changes land on main and a human
promotes later), skip this phase and say so.

### Phase 4 — Product-owner report and one next action

This next-action rule applies to **every** return from Phases 0–3, including hard stops.
A stop may skip normal report formatting, but it still ends with one concrete command or
one specific requested decision—never an “A or B” menu.

The coding happened in detached worktrees, so first gather the durable facts: landed
commits, closed issues and analyses, worker reports, green-gate/deploy evidence, and the
full IDs of runs this session owns. Use `/worktree-status`'s presentation discipline to
write the product-owner snapshot directly: Summary · Ready to test · Decisions needed ·
Discussion points · Spin-offs. Do not create a second status pass merely to reformat facts.

End with exactly one concrete next action:

- unresolved test/decision feedback: ask for the specific answer(s); that reply may drive
  the single bounded follow-up allowed below;
- a session-owned unsettled run: print the exact `taskfleet run wait <full-run-id>` or
  `run show` command needed next;
- failed validation, ambiguous ownership, or preserved work: name one exact inspection,
  repair, abort, retry-with-harvest, or decision action (including the concrete failing
  command where relevant);
- no unresolved feedback after the first report: `/stint-handoff`;
- after the one feedback follow-up report: `/stint-handoff` regardless of new work ideas
  (record larger work durably for the next stint).

Do not promise automatic wake. A harness may wait on the public command, but lack of a
wake facility leaves the copyable next action unchanged.

### Phase 5 — Absorb the user's feedback

The `/worktree-status` snapshot hands the user things to act on — items to test,
discussion points, spin-off calls. This is where they react, and their reactions
decide what happens next.

- **Light feedback** (a handful of small asks) → if this invocation has not yet consumed
  its feedback budget, run one fresh Phase 0–4 pass on the smaller work-list. This is the
  same full round, not "phases in miniature", and every change still goes through a
  worktree. After its product-owner report, the only next action is `/stint-handoff`.
  If the budget was already consumed, record the asks durably for the next stint.
- **Heavy feedback** (a lot comes back) → don't try to carry it in this session's
  context. **Land it durably first** — update the affected **issues**,
  **documentation**, and **`TODO.md`** so nothing is lost — *then* move to the handoff.

**Keep issue scheduling current before any re-run.** If feedback files new issues or
changes a dependency, update its scheduling frontmatter through issuectl and commit the
exact rewritten issue paths before a feedback re-run consults `issuectl dag --json`.
If you capture feedback without a re-run, still record the metadata so the eventual
handoff sees the accurate live graph.

Once feedback is acted on or captured durably, the bounded invocation is done. Emit the
single next action required by Phase 4. Terminal finish remains explicit:
`/stint-handoff` is a separate skill and never runs merely because this round went green.

## Non-goals

- **Not a worktree**, and does not create one directly — it delegates to the
  `/worktree-*` family.
- **Does not write code** in this session — every change goes through a worktree.
- **Not for a single one-off coding task** — that's `/worktree` (router).
- **Not the terminal handoff/wrap-up** — that's explicit `/stint-handoff`.
- **No durable stint/checkpoint state, new lifecycle noun, umbrella skill, or background
  loop.** Existing run, issue, git, and TODO facts remain the only durable owners.
- **Hardcodes no project facts** — reads them from the repo's AGENTS.md/TODO.md.
