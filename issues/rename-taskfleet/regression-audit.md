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

This document records the initial findings and implementation boundaries, not a
claim that the fixes or final validation have completed.

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

These per-unit results do not replace the final integrated gate. Retained-work
disposition and recovery-status work are still in progress.
