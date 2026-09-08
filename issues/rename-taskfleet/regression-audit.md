# Regression and simplification round — 2026-09-08

## Scope and evidence

The user requested a broad correction round that examines all issues before
patching, watches implementing workers, and removes unnecessary architecture
where an observed problem can be solved more directly. The initial inventory at
02b8145 contains 395 issues: 185 done, 132 fixed, 30 obsolete, 20 wontfix,
12 duplicate, four cannot-reproduce, and 12 open. These are issue dispositions,
not counts of independent confirmed regressions.

All 12 open bodies and their supporting analyses were reviewed. Eleven describe
accepted executable work; the rename epic requires separate ecosystem evidence.
An independent read-only review agreed with the groupings below. Execution order
and reservations remain in issuectl, not in this document.

## Common causes and chosen simplifications

| Observed issue group | Source of complexity | Small coherent correction |
| --- | --- | --- |
| `@run-wait-json`, `@phenomenally-noisy-behavior` | Internal facts are discarded and reconstructed by consumers | Serialize the wait loop's existing stop decision; expose recorded source repository and filter by actual Git identity |
| `@intake-bug-taskfleet-6edf517c691a`, `@intake-feature-taskfleet-5565259bd11f`, `@worker-review-scope-discretion`, `@run-merge-stamp`, `@end-end-stint` | Global inventory treated as session ownership; repeated policy and mandatory review; obsolete lifecycle wishlist | One bounded skill flow with launched/adopted run IDs, safe source synchronization, proportionate review, and issue-owned trailer stamping; no persisted stint lifecycle |
| `@cancelled-run-hides-preserved-worktree`, `@intake-feature-taskfleet-41343c4dd3e4` | Historical recovery report confused with current resource ownership | One computed retained-resource observation shared by visibility and explicit audited discard; preserve narrow salvage semantics |
| `@recovery-merge-status` | Successful authoritative merge repairs node but leaves terminal manifest frozen | Reconcile the existing confirmed-merge recovery transition without broad Git-inference repair or another lifecycle |
| `@workmux-project-prefix-emoji` | Taskfleet overwrites display naming owned by workmux | Preserve workmux's actual configured name; stable tmux IDs retain ownership |
| `@canonical-root-regression` | A later local fix reintroduces automatic alternate-home discovery after the clean-break decision | One canonical default and explicit `TASKFLEET_HOME` for nondefault locations; remove discovery/classification machinery |

## Reproductions and review observations

- Installed 0.7.1 `run wait` on a pending run with `--timeout 0s` returns a payload
  with no reason field. Source already has `Stop::{Met,TimedOut}`; another status
  classifier is unnecessary.
- Installed `run list --help` has no repository selector; the list DTO omits the
  source identity already present in the manifest.
- The recorded recovery incident still reads run `failed`, `landed: true`, and
  `report.success: true` with `via: explicit-merge`. The reducer and supervisor
  each independently retain a terminal-manifest guard.
- The emoji worker's actual worktree and main resolve to the same Git common
  directory, and its manifest identifies the correct source repository/branch.
  Native creation pins workmux's cwd to the recorded materialization repository.
  `spawn.rs` then replaces the workmux-generated window name. The observed naming
  defect is therefore not evidence of a disconnected worktree.
- Review of the in-progress repository filter found a path-prefix membership
  shortcut that would include an independent nested repository. The conductor
  requested actual Git identity and a nested-repository exclusion regression.
- `scripts/check-canonical-identity.sh` fails on current source: commit 934a296
  reintroduced alternate-root selection after clean-break a357c0c. The earlier
  dual-root visibility issue was real, but implicit discovery contradicts the
  currently binding contract. Explicit location selection retains operator access
  without discovery, adoption, migration, or installed-tool mutation.

## What stays

The event watermark, shared/exclusive locks, recorded OID-based merge transaction,
verified process identity, and dirty/unmerged-work preservation have concrete
failure history. They are not speculative mechanisms to remove. Simplification
targets competing ownership, duplicated interpretation, and superseded policy.

No background janitor, retention policy, session database, generic cleanup
framework, inferred success, or automatic discard is authorized by this round.

## Completion boundary

Every implementation must pass the repository green gate, relevant snapshots,
and meaningful regressions. Integrated main receives its own validation. The
canonical identity check is included because Rust-only validation missed the
reintroduced compatibility layer.

The rename epic cannot be closed solely from repository-local tests. A read-only
ecosystem check found contradictory current naming guidance in the canon owner,
stale downstream convergence records, and unavailable current live-host evidence.
Those repositories and installed artifacts are outside this repository's mutation
scope. The existing clean preserved rename worktree also contains two unlanded
documentation commits; it is left intact rather than mistaken for an empty orphan.

The initial findings above distinguish reproduction evidence from the verified
landings and final validation recorded below.

## Verified landings

The conductor verified both the durable merge report and the resulting source on
main for these units:

- Window naming (`fde47ad`, closure `2017893`): workmux owns the display name;
  Taskfleet retains stable tmux identities. The regression rejects any rename and
  checks the actual workmux cwd against the disposable source repository. All five
  gates passed (1,076 nextest tests). A bounded cold release build timed out; its
  managed retry completed successfully. Independent final review found no blocker.
- Read contracts (`9e44dce`, closure `a6b3d55`): direct serialization of the existing
  wait stop decision, recorded repository provenance, and exact Git common-dir
  filtering. Real-Git coverage includes linked worktrees, subdirectories,
  independent nested repositories, sibling repositories, and missing provenance.
  All five gates passed (1,077 nextest tests), relevant snapshots were reviewed,
  and the local release build passed.
- Canonical default root (`54f8e31`, closure `c696495`): the default derives only
  from HOME plus the canonical directory; explicit location and internal worker
  binding remain validated. All five gates passed (1,072 nextest tests), as did
  disposable-home and stripped-PATH checks and the local release build. The
  conductor reran the identity inventory on main: zero retired-identity references.
- Workflow guidance (`222dfd1`, closure `06ac685`): one bounded round, exact
  session-owned run IDs, explicit composable handoff, proportionate review, and
  issuectl-owned commit stamping. Historical kind-emoji examples were corrected
  to workmux-owned naming. All five gates passed (1,077 nextest tests), rendered
  contracts and catalog snapshots were reviewed, and local release skill output
  was inspected. All five assigned workflow issues are closed.
- Retained-work visibility and discard (`a38d9f5`, closure `3d669f6`): both
  assigned issues are closed. All five final gates, the identity inventory, and
  the local release build passed. Thirteen disposable real-Git command tests
  also passed with PATH containing only an explicit Git link. Final help/show
  snapshots and rendered overview/handoff guidance were reviewed. Initial clippy
  findings and expected snapshot mismatches were corrected before the final gate;
  no required validation failure remained. The conductor verified the canonical
  merge report and resulting source on main.
- Canonical triage producer (`192b346`, closure `398f224`): the analysis skill
  requires the exact heading consumed by issuectl. Its rendered-contract test
  ignores line wrapping while rejecting the former alternative. All five gates,
  the identity inventory, local release build, rendered output inspection, and
  issue doctor passed. The focused suite produced no new snapshots. The conductor
  verified the durable merge report and actual main diff; the worktree was removed.
- Recovery status (`4461872`, closure `27b3565`): one shared authority predicate
  and the existing streaming fold now support narrowly authorized Failed-to-Done
  recovery. All five final gates passed (1,099 nextest tests), as did the identity
  inventory and local release build. The supervised disposable-tool integration
  exercises failure, successful recovery, matching public status/landing/report,
  teardown, and one deterministic recovery event. Initial Clippy and fixture
  environment/timing findings were corrected before the successful full gate.
  The conductor verified the durable report, actual main code, and removal of
  the session worktree. `dfa70e7` separately connects the existing identity check
  to CI's existing version-snapshots job.

All seven implementation units have landed. These per-unit results do not replace
the final integrated gate.

## Additional observed producer mismatch

During the round, another agent transferred `@canonical-triage-heading` from
issuectl review (`fbe3f41`). The conductor verified that Taskfleet's current bug
analysis template permits either `Triage analysis` or `Suspected Root Cause`,
while issuectl's current intake implementation derives its analysis field from
the exact former heading. This directly fits the user's request to learn from
other agents and remove redundant consumer interpretation. The bounded fix was
accepted and scheduled under that request: make this producer emit the agreed
heading, with a rendered-contract regression. No consumer parser or compatibility
layer is added, and issuectl's repository and installed artifacts remain untouched.

## Findings from observing implementation

The retained-work implementation received focused independent review because it
adds an explicitly destructive command. These findings were corrected inside the
assigned work rather than filed as speculative residual issues:

- The existing worker-identity helper conflated an unavailable OS start-time probe
  with a verified recycled PID. Extraction now preserves the distinction in one
  shared observer, retaining positive live proof over a stale exit report.
- Exact Git identity requires preserving path content and comparing the actual
  worktree's common directory. A disposable real-Git experiment replaced a stale
  registered path with an independent repository: Git refused deletion, but the
  initial Taskfleet observer would have authorized it first. The ownership check
  now refuses that contradiction before authorization.
- A locked projection can still lag a durable event. Mutation eligibility now
  uses the existing replay logic before reading; dry-run refuses lagging state
  without replay or truncation. The replay prelude was extracted from existing
  append logic, not implemented as another recovery mechanism.
- Retrying a recorded decision must reuse its target binding and actor, not just
  its idempotency key. Historical completed discards do not compete with current
  retained resources for node selection; all-absent retries reuse a real prior
  decision without another event or persisted selection record.
- Display and deletion share the resource observation, verification result, and
  node-file reader. For a present worktree, commit counts use its actual HEAD;
  branch-only resources use the recorded branch.
- The canonical identity inventory was absent from CI. The final worker connected
  that existing check to the existing cheap CI job, closing the observed gap
  without another validation framework.

The implementation gates and final integrated validation remain the completion
criterion; review findings alone are not a success claim.

The recovery review found the same ownership problem at a different boundary:
the child-tail terminal flag means its report was consumed, not that the child
succeeded. Recovery now uses fresh child membership and manifest evidence while
retaining the existing report-consumption order. Catch-up, membership discovery,
decision, and append share the parent lock; discovery propagates errors without
reacquiring that lock. The event reducer checks only bounded local log facts, so
replay never reads another run or borrows authority from a future report. A
deterministic key identifies the exact authorizing merge report independently of
the earlier failed roll-up. Independent bounded re-review found the reported
missing-child race resolved and no new locking/read-failure blocker.

Source synchronization also brought in `@intake-bug-taskfleet-8db39a4f7cdb`
(`42fec0a`). It describes the exact same observed recovery run as
`@recovery-merge-status` and was closed as a linked duplicate, without launching
another worker or adding a second status owner. This is intake deduplication,
not an additional independently fixed bug. Current landing identifiers above
reflect the clean source rebase onto that intake commit.

## Completed repository round

The integrated code at `27b3565` passed all five gates: rustfmt, warning-denying
Clippy, 1,100 release nextest tests, two release doctests, and warning-denying
rustdoc. The suite's one skipped test is the pre-existing explicitly ignored
expensive core stress test. `cargo build --locked --release` also passed.

The conductor then executed the two native lifecycle tests and all 13 discard
tests directly with PATH containing only an explicit Git link: all 15 passed.
These use disposable state, repositories, and private tmux/workmux fixtures;
they do not depend on installed agent tools. The locally built CLI also returned
the expected repository provenance, Done/landed state, empty preserved-resource
list, and `condition-met` wait outcome for the completed session run. Rendered
triage output required the exact heading, and discard help exposed dry-run.

The identity and version-snapshot inventories passed, no `.snap.new` remained,
and issuectl doctor reported no findings. All seven session-owned runs have a
durable successful merge report and verified main changes; their worktrees were
removed by their supervisors. The unrelated preserved rename worktree remains
intact.

Final inventory: 398 issues, with 138 fixed, 192 done, 13 duplicate, 30 obsolete,
20 wontfix, four cannot-reproduce, and one open epic. This round resolved 13
executable issues and one duplicate intake. The remaining rename epic requires
separate external convergence/live-host evidence; no repository-local coding
task remains queued. Main is published only after the local integrated gate,
and its exact push CI result is checked and reported in the session's final
response. No release, installed-binary/skill change, user-state migration,
automatic retained-work deletion, or other-repository mutation was performed.
