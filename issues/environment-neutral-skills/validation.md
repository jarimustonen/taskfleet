# Environment-neutral skills — implementation and review evidence

## Scope

All 14 bundled templates remain the upstream family. Tracker-backed workflows
retain issuectl's schedule and issue lifecycle; tracker-free stints instead run
accepted free-text tasks serially. No private DAG or replacement issue store is
introduced. A missing CLI is distinguished from a repository without a tracker,
and repository-required tracker operations remain blockers.

Parents await the public `run wait` process using their existing harness wake
facility, or bounded foreground waits. Timeouts, killed observers, attention,
landing, full reports, and retained work are distinguished. Native Taskfleet
merge/report/supervision authority remains unchanged. No run merge, state,
supervisor, or worker-preamble implementation was edited. No host-specific
browser assertions, personal notification service, human terminal attachment,
or universally named executable profile is introduced.

## Deterministic evidence

- `cargo nextest run --locked --release -p taskfleet --test skill_examples --test skill`:
  initial run passed all 62 tests; repeated after review corrections.
- The documented shell probe itself executes under an empty/declared-stub PATH
  for each tracker-bearing template, crossing repository configuration and CLI
  availability. An unrelated `issues/` directory is present even in non-tracker
  cases. The stub exits 99 if invoked: detection cannot execute the tracker.
- Rendered-section snapshots pin actionable tracker, wait, routing, free-text
  analysis, tracker-free stint, and profile contracts. Every accepted snapshot
  was read, including the amended snapshot after review.
- Existing CLI-example parsing and self-contained failure-disclosure tests
  remain enabled. Narrow forbidden-dependency checks supplement, rather than
  replace, executable probe tests and reviewed prose snapshots.
- The exact full pre-merge gate is `scripts/validate-local-release.sh`, run on a
  clean committed tree. Its final result is recorded in the issue closure and
  native run report; this document does not substitute for that result.

Build scratch is disk-backed under `/var/tmp/taskfleet-portable-build`. Test
homes and native-tool fixtures remain disposable; installed Taskfleet and
bundled skills are not changed. These are instruction/CLI contract tests, not a
claim to have run a live browser/provider journey on another host.

## Independent instruction review

One independent reviewer, `claude-fable-5-1`, ran two calls: executor and
cross-skill lenses. Context included every bundled callee and the issue goal.
Both calls returned complete findings. Final localized corrections were checked
by the worker and deterministic tests, not by a repeated model panel. Raw local
review output was `history/portable-skills-review.txt`; the durable assessment is
below. No performance-corpus write was made, because this worker must not mutate
another repository.

| Finding | Assessment and disposition |
| --- | --- |
| `.data.stamp.status` allegedly wrong | Rejected. This run explicitly requires that path; installed issuectl JSON uses a `data` envelope. An older source checkout cited by the reviewer is not the active CLI contract. |
| Missing tracker CLI conflated with no tracker | Fixed. Three-state probe plus explicit repository-policy blocker; only permitted free-text work may continue, with disclosure. |
| Nonexistent inline stint handoff fallback | Fixed. Printable bundled sibling and a concrete repository-local fallback, with missing-workflow disclosure. |
| Foreground wait exceeds harness timeout | Fixed. Bounded native waits, killed-observer recovery, no worker-failure inference from lost wait. |
| Generated preamble allegedly forces tracker installation/filing | No code change. It constrains how an issue is filed, not a requirement to install a tracker or create an issue. Briefs state missing capability and put follow-ups in native reports without weakening provenance rules. |
| Unidentified issue tree might be adopted | Fixed. Repository instructions/schema establish use; configuration verification is read-only file inspection, not an exploratory tracker command. |
| Bug-analysis router still requires a slug | Fixed. Router and description accept report paths/text when tracker-free; report slug is defined from its title. |
| Profile-routing sentence contradicts omission/default | Fixed. Explicit selections are forwarded; otherwise configured defaults apply; `profile_required` surfaces choices instead of guessing. |
| Pi default, repository-specific ADR assertion, notify-as-parent-wake residue | Fixed. Installation-owned selection, neutral ADR rationale, operator hook distinguished from parent wait. |
| Conversation-only status runtime boilerplate | Already corrected during review: no tools or fresh evidence gathering in that skill. |

## Failure / incomplete-command inventory

- The reviewer independently ran `skill_examples` while the new contract snapshot
  was being amended. Its snapshot assertion failed and produced
  `skill_examples__portable_workflow_contracts.snap.new`; the review itself
  returned complete findings. This was a development snapshot mismatch, not
  accepted missing validation. The updated snapshot was read and accepted, and
  tests rerun; no pending `.snap.new` is permitted by the full gate.
- Early discovery commands returned nonzero for an unmatched `rg` search and
  absent `scripts/*skill*` / pre-build `target` paths. These were read-only
  inventory probes, not required validation failures; source tests were found
  under `crates/taskfleet/tests/skill_examples.rs` and the release binary was
  built locally. No tool installation or service access followed those probes.

This file is the stable redacted failure record referenced by the terminal
report; transient detailed logs remain under this worktree's ignored `history/`.
