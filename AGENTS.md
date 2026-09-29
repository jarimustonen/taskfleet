# taskfleet

Rust CLI that runs autonomous AI-agent workflows on a developer's machine. It
spawns agents into isolated git worktrees, watches each run with its own
supervisor process, and merges the work back with `run merge`. Run state is
file-based under `~/.taskfleet/runs/<run-id>/`: an append-only `events.jsonl`
plus flock-guarded projections, so every UI reads the same source of truth. The
orchestration workflows ship as bundled skills under
`crates/taskfleet/skills/*/SKILL.template.md` (`/worktree-spinoff`,
`/worktree-research`, `/worktree-technical-decision`, `/fan-out`,
`/stint-start`, `/stint-handoff`, and others).

`ARCHITECTURE.md` is the code map. `crates/taskfleet/AGENTS.md` covers the CLI
crate itself: the snapshot test loop, test-spawn hygiene, profiles, evidence
capture, and the skill installer. This file holds what is true of the whole
repository and would otherwise be learned the hard way.

## Decisions that shape the code

**The supervisor is told, not guessing.** ADR 0001
(`docs/decisions/0001-thin-supervisor-vs-harden.md`, design in
`issues/lifecycle-architecture-review/design.md`) replaced an inference engine
that reconstructed a worker's state from a cross-product of proxies (pid
liveness, tmux panes, branch ancestry, three activity clocks) with a thin
model: a unit is done only when the worker called `run merge`, and every other
terminal outcome is a typed table. The proxies were deleted because patching
one edge case reliably exposed its neighbours. If you find yourself adding a
heuristic that infers done-ness, idleness, or interactivity from ambient
signals, you are rebuilding the thing the ADR removed. The TUI, discussion,
orchestrate, and code-pipeline surfaces were cut in the same release along with
the `worktree-code` / `worktree-bugfix` / `worktree-make-skill` kinds. Bringing
any of them back is a new decision, not a restoration.

**One identity.** ADR 0002 (`docs/decisions/0002-taskfleet-identity.md`): the
workspace is exactly `taskfleet-core` and `taskfleet`, the only executable is
`taskfleet`, state lives only under `~/.taskfleet`, and the code contains no
alias, migration, wrapper, or transition artifact for any earlier name.

**Non-blocking waits are not this repository's problem.** A homebase decision
(ADR 0011 there) superseded the closed `pi-background-jobs-extension` issue.
Session-scoped background commands inside a pi.dev session belong to the host
runtime, which may use the third-party `@aliou/pi-processes` extension; its
processes dying with the pi session is a safety property. Durable background
running belongs behind a separate, harness-neutral runner contract. taskfleet
therefore stays the run-state owner behind `run wait`, the `landed` flag, and
its JSON contracts, and it does not import that extension, reach into its
manager object, assume its process ids or log paths, or use its in-process
EventBus. A pi extension's internals are not a public interface. Do not file a
"build our own pi background-jobs extension" issue here; that option was
evaluated and rejected.

## The tool family

taskfleet is one of a family of AI-first CLIs with shared conventions. In this
repository:

- **issuectl** owns `issues/`, `issues/AGENTS.md`, `.issuectl/AGENTS.md`, and
  the `/issue` skill. The execution DAG (`lane:` / `lane_seq:` frontmatter,
  `issuectl dag`) is the scheduling source of truth.
- **Shipshape** owns the approved `OSS-RELEASE.md` contract and the
  `/shipshape-*` skills. It performs the version bump, the exact intra-workspace
  pin rewrite, the `Cargo.lock` refresh, and the CHANGELOG finalize, and it
  reconciles a cut against registry state.
- **project-canon** ships the `/ai-first-cli-canon` skill, the binding
  reference for any CLI surface work here (strict input validation, `--json`
  output, JSONL logs, no interactive prompts, informative errors, composable
  commands). The installed copy is gitignored on purpose. Change the canon in
  the `project-canon` repository and reinstall it; a repo-local copy drifts.
- **glasspad** publishes HTML views to a browser for dashboards and reports. It
  is not part of the build.

## Where things live

Every directory follows one pattern: `AGENTS.md` holds the agent-relevant
knowledge, `CLAUDE.md` is a symlink to it, and an optional `AGENTS-<TOPIC>.md`
splits out a large topic.

Issues are `issues/<slug>/item.md`, flat, with status in the `status:`
frontmatter field rather than the path. Use the `/issue` skill and `issuectl`
for every mutation; the frontmatter is schema-validated and hand edits break
the optimistic-concurrency tokens. Every planning document (plan, analysis,
validation, design, breakdown, todo) lives under its issue directory, so work
that needs a planning document also needs an issue. `TODO.md` is only the
session handoff and a compact stint archive; it is not a schedule.

`history/` (agent scratch) and `target/` are gitignored. An issue that only
points at a `history/` file is unreadable to anyone else, so a review residual
becomes an issue only with an observed occurrence or a self-contained
description. Filing every deferred residual from an automated review
manufactures work nobody asked for.

## Working in this repository

This section is what `/stint-start` reads in Phase 0. Every project-specific
fact an orchestrator or worker needs is here or pointed to from here.

**Coding happens in worktrees.** The orchestrator session plans, spawns, reads,
and reports; it does not edit code. Spawn `/worktree-spinoff <issue-slug>` and
friends. On macOS, five or six simultaneous spawns can exhaust
pseudo-terminals (`fork failed: Device not configured`, surfacing as
`workmux_add_failed` mid-batch), so batches larger than three go `--headless`;
the bundled skills describe the placement flags. `taskfleet event tail
<run-id> --follow` streams completions so you need not poll.

**Git is the truth about landings.** `run` status can lag reality, so confirm
every landing from git before reporting it. Handoffs describe the state as it
was.

**Check the installed binary before spending a worker.** Three stints in a
row, intake reports described either a defect the current release had already
fixed or a misread of the JSON surface (`last_report` versus `report`,
`data.runs[]` versus `data.<field>`). Reproducing against the installed
taskfleet first closed those issues without code.

**Worker deaths are transient.** Re-spawn and adopt the preserved branch
(review, adopt, complete, merge). Hand-merging unreviewed work skips the only
review the change will get. Heavy units legitimately run 54 to 96 minutes; a
long run is not a hang.

**Repository work leaves the installed taskfleet alone.** This is a maintainer
decision (2026-08-23). Build with `cargo build --release` and run
`./target/release/taskfleet …` explicitly. Nothing in repository work creates,
replaces, removes, or modifies the user's installed binary or installed
bundled skills, by any route: `cargo install` or `cargo uninstall` in any form,
Homebrew, manual copies, or any `skill install` invocation, including one from
`./target/release/taskfleet`. The installed release and source `HEAD`
legitimately differ; what lands here reaches the user's tool only through a
published distribution-channel upgrade. There is no stint deploy step, no test
account, and no reset step for this project.

**The green gate is one script.** Run `scripts/validate-local-release.sh`
before merging any worktree. It is the sole maintained aggregation of the
repository and release checks: fmt, clippy with warnings denied, release-mode
nextest, doctests, rustdoc with warnings denied, snapshot and identity checks,
cargo-deny, the shell release fixtures, package archives, the Shipshape
contract, and cargo-dist plan topology. It pins every Rust subprocess to
rustup's current `stable` channel and fails closed when a prerequisite is
missing (`cargo-nextest`, `cargo-deny`, `gh`, `jq`, `python3`, `shipshape`, or
cargo-dist 0.33.0 via `DIST_BIN`). Copy that exact command into worker briefs.
A debug-mode `cargo test` passes in a worktree and turns `main` red. An
orchestrator may install the test runner once with
`cargo install cargo-nextest --locked` (that installs a test runner, not
taskfleet); a worker reports a missing prerequisite rather than installing
globally. Two of the gate's steps exist because nothing else catches their
failures: doctests, because nextest does not run them, and rustdoc, because
neither tests nor clippy notice a dangling intra-doc link. A cut that removes a
symbol leaves any `[`crate::…`]` link to it dangling; this made `main` red once
(`ci-red-main`, 2026-08-15) after a green test-and-clippy round.

**A developer machine is not a bare CI runner.** A test that depends on an
ambient `tmux`, an installed harness binary, or any other undeclared tool
passes locally and fails in CI. Exercise tool-sensitive tests with a stripped
`PATH` containing only the explicitly required toolchain and stubs.

**Per-worktree green does not imply integrated green.** After a
multi-worktree round, run the gate again on the integrated `main`. Two failure
modes have only ever shown up there. A test-isolation flake can stay latent
until several workers' tests coexist in one run (2026-07-25: an
order-dependent hook-file TOCTOU in `supervise::notify` passed in five separate
worktrees and failed in the combined suite). And a DAG lane predicts the files
an issue will *likely* touch, but a fix can legitimately land elsewhere, so
two "disjoint-lane" spinoffs can collide (2026-08-10: a fix predicted for
`supervise/*` landed in `run/*` and integrated `main` did not compile). Prefer
sequencing any two units that might both touch the `run show` / `RunSummary`
DTO surface.

**Hot files are edited one worktree at a time:**
`crates/taskfleet-core/src/{events,lock,reducer,schema}.rs` and
`crates/taskfleet/src/supervise/*`. They carry the state-integrity invariants
below, and two concurrent edits there do not merge safely even when the diffs
are textually disjoint. The light `harness/` launcher is no longer a hot
cluster since the heavy layer was deleted (2026-08-14).

**Toolchain.** taskfleet builds and releases with rustup's newest `stable`.
Old compilers are not compatibility targets and the crate metadata
deliberately declares no `rust-version` floor. A dependency security update
wins over compatibility with an older compiler.

**Staying in sync and pushing `main` needs no asking.** Parallel worktree
sessions push under you, so `git pull --rebase` then `git push` before and
after a round, before a handoff, and before a release is expected. This
deliberately overrides the global "never push without being asked" default
for this project.

**Killing a supervisor.** `pgrep -lf "taskfleet supervise"` lists supervisors
from every repository on the machine, and this was learned twice in one
session by killing the wrong one. `taskfleet run cancel <run-id>` is the
graceful path and leaves no orphans. If you have to kill by process, identify
the run first: `tmux list-windows -a` shows an emoji prefix on each `wt-*`
window naming its source project (🎬 is taskfleet), and `git worktree list`
in the right repository confirms the worktree is yours. Scope any `pkill`
pattern to the checkout path, never to the product name, so production
supervisors from `~/.cargo/bin` or Homebrew are untouched.

## Releasing

**What is at stake.** Pushing a `vX.Y.Z` tag is the release. It starts both
publication workflows: `.github/workflows/publish-crates.yml` publishes
`taskfleet-core` and then the exact-pinned `taskfleet` to crates.io, and
cargo-dist's generated `release.yml` builds the binaries, the GitHub Release,
and the canonical Homebrew tap `jarimustonen/homebrew-taskfleet` (the mac
target on a self-hosted runner; the canonical install is
`brew install jarimustonen/taskfleet/taskfleet`). crates.io versions are
permanent and can only be yanked, so a bad tag is fixed forward with a new
patch, not undone. Nothing publishes from a local machine: a local
`cargo publish` would race the workflow and leave a half-published saga.

**Autonomy.** The maintainer decided (2026-08-05) that cutting a release
needs no permission: the conductor decides when a release is warranted and
pushes the tag. The default cadence is to ship often, cutting whenever
something production-ready lands rather than batching. Autonomy removes the
prompt, not the care: the version is right, the changelog is finalized, the
snapshots are regenerated, the tree is clean, and the exact commit being
tagged has passed the complete local gate.

**Who cuts.** Only the conductor. Workers in worktrees may run package and
distribution checks and `scripts/shipshape-release.sh plan`, but they do not
cut, tag, publish, install, move state, mutate a tap, or edit another
repository; a worker's branch is not `main`. Package verification inside the
repository uses `cargo package --workspace --no-verify`, because the exact
`taskfleet-core = "=<version>"` pin is not registry-visible until the cut.

**How.** `OSS-RELEASE.md` is the contract and describes the transaction;
`scripts/shipshape-release.sh` (`plan <bump>`, `cut <plan-id>`,
`resume <run-id>`, `verify <run-id>`, `check-tool`) is the only way to drive
it here, and `/shipshape-release` chooses the SemVer bump. The wrapper exists
because Shipshape's own protocol has no pause between its bump commit and its
tag push: the wrapper journals a held tag push, advances `main` to the bump
commit, runs `scripts/validate-local-release.sh` on that exact clean `HEAD`,
rechecks local and remote `main`, creates the protected authorization ref that
both workflows verify, and only then resumes. It checks the published stable Shipshape formula, release digest, source tag,
and executable bytes on every operation; the local gate exercises the current
release engine against a disposable held-tag protocol fixture. Unverifiable
artifacts, a stale installed version, or an incompatible protocol fail closed
without pinning routine compatible upgrades to a numbered allowlist. Two traps follow from this design. A bare
`shipshape release resume` while the tag is still local skips the gate; use
the wrapper's `resume`. And once the journal says the tag was pushed,
publishing may already be underway: resume and verify the existing run
(`shipshape release list --json`, `shipshape release show <run-id> --json`)
rather than retagging or publishing by hand. If local validation finds a
defect in the bump commit, abandon the run
(`shipshape release abandon <run-id> --reason <reason>`), remove the local
tag, fix forward, and plan a new cut. The workflow's `scripts/publish-crates.sh`
owns crates.io reconciliation and the wrapper's `verify` owns cross-leg
verification; a hand-rolled crates.io probe without a meaningful `User-Agent`
once raised a false alarm. Local validation on macOS does not prove Linux
runtime behaviour; the publication workflow's cross-platform build does.

## State-integrity invariants

Seven invariants govern the on-disk run state and the autonomous-spinoff
loop. Each is easy to violate from inside a hot code path without noticing,
and each has a module doc that carries the full mechanics and the issue that
motivated it. Read those before touching the reducer, the lock layer, `run
merge`, or supervisor cleanup. What follows is what each one protects.

1. **`applied_seq` watermark** (`crates/taskfleet-core/src/events.rs`). The
   reducer advances `manifest.applied_seq` only after every projection an
   event touches is fsynced, and replays events above the watermark on the
   next lock acquisition. Every event-appending path goes through the
   `LockedRun` witness and the `append_and_apply_*` API. Calling a `write_*`
   projection helper directly leaves a projection that no replay will repair.

2. **`LockedRun` witness** (`crates/taskfleet-core/src/lock.rs`). Compile-time
   proof that the caller holds the run's exclusive flock before appending.
   Only the exclusive guard mints one. Thread it through rather than
   bypassing with an `#[allow]`; the type is the whole enforcement.

3. **Shared lock on every multi-file read** (`RunLock::with_shared_lock`).
   The reducer writes `manifest.json` and `nodes/*` under the exclusive lock,
   so a reader that touches more than one of them in a single decision without
   `LOCK_SH` can observe a half-applied set.

4. **Progress polls `status`, never `lifecycle`.** `Lifecycle` is
   `Autonomous | Interactive`, set once at `run create` from the explicit
   `--interactive` flag and never transitioning; it is not derived from the
   kind. `Status` is `Pending | Running | Blocked | Done | Failed | Cancelled`;
   nothing produces `Blocked` since the discussion cut, but the variant stays
   so historic runs still decode. A skill that polls `lifecycle` for a
   terminal value hangs forever, which was a real bug
   (`skill-progress-polling-wrong-field`). In interactive mode the supervisor
   never auto-terminalizes or tears down from a dead pid or worker exit; it
   waits for an explicit `run merge` or `run cancel`.

5. **The supervisor is the only teardown actor, and unmerged work survives
   teardown** (`crates/taskfleet/src/supervise/cleanup.rs`). `merge.sh` does
   not touch tmux or worktrees; the supervisor sees the terminal report, rolls
   the run up, and tears down. Its tmux window lookup is session-scoped and
   exact-cwd, because a prefix match once found the user's own pane. Teardown
   force-removes a worktree and force-deletes a branch only after a confirmed
   explicit `run merge`. On every other path (blocked, failed, cancelled, a
   plain success that skipped `run merge`) it preserves the branch and
   worktree whenever there is anything to lose, and it fails closed: commits
   not reachable from the run's own recorded source branch, uncommitted or
   untracked changes, a detached or differently-branched HEAD whose commit is
   not in source, or any git error along the way all produce a
   `cleanup.branch_preserved` event instead of a removal. Non-force
   `git worktree remove` is the last safety net on those paths, since git
   refuses to discard a tree that a race dirtied. `run discard --force` is a
   separate, explicitly authorized operator action that records who and why;
   it does not change any outcome or status. The `--notify` hook fires on the
   terminal transition before teardown and is at-least-once by the owner's
   choice: spawn first, record the `run.notified` marker after, tracked
   separately from `cleaned`. Reordering either silently drops notifications
   on a crash. The module doc lists the exact guards, reason strings, and the
   known residual TOCTOU (`detached-head-teardown-toctou`).

6. **`run merge` is a recorded, OID-recoverable transaction**
   (`crates/taskfleet/src/run/{merge,merge_recovery}.rs`,
   `crates/taskfleet/scripts/merge.sh`). The merge spans git refs and the event
   log and is not atomic across them, so it records `merge.started` with the
   expected source OID and the worker OID before touching git, guards the
   fast-forward with a compare-and-swap against that expected OID, and lets
   recovery resolve exactly that one transaction against immutable OIDs once
   the driver process is confirmed dead. Recovery completes the missing
   `explicit-merge` report only when the recorded worker OID is git-verified
   integrated into the moved source; otherwise it rejects and preserves the
   work. Verifying against the mutable worker branch, or a broad "branch is an
   ancestor of source, so done" inference, is the deleted git-reconcile probe
   under another name. The remaining non-atomic windows are documented in the
   module; the operation lease that closes them is deferred (design §2.7).

7. **Terminal outcomes are a typed table**
   (`crates/taskfleet/src/supervise/outcome.rs`). `TerminalOutcome::classify`
   maps a terminal report to one outcome and `TerminalOutcome::teardown` maps
   that to the single teardown policy it authorizes; `cleanup_node` reads the
   table rather than re-sniffing report JSON, so a new outcome cannot default
   into destroying a branch. The primary completion signal is the told
   `worker.exited` fact. Pid liveness is only the crash backstop: it fails a
   node only when the worker is confirmed gone with no merge and a persisted
   post-death grace (anchored to `Node.first_death_at`,
   `TASKFLEET_DEATH_GRACE_SECS`) has elapsed, and it re-checks under the lock so
   an exit or merge that landed in the window wins. A clean exit with no merge
   stays non-terminal and attention-required: the worker skipped `run merge`,
   and auto-failing it would discard a finished unit.

## Conventions the bundled skills depend on

- Worker reports go to `/tmp/node-report-${run_id}.json`. A shared
  `/tmp/node-report.json` lets concurrent spinoffs clobber each other.
- Skill prose polls `manifest.status`, per invariant 4.
- CLI-surface and bundled-skill changes need the insta snapshot loop in
  `crates/taskfleet/AGENTS.md`, with every accepted change reviewed; the skill
  catalog pin test is edited by hand.
