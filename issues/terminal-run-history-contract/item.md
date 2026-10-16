---
created: 2026-10-16
updated: 2026-10-16
type: task
reporter: jari
status: done
priority: normal
provenance: chat
source_ref: taskfleet:01m51nrfvbqwsybz0gdggz6q4b/request:terminal-run-retention
originating_run: 01m51nrfvbqwsybz0gdggz6q4b
originating_run_kind: spinoff
related: ['@agent-session-retention', '@done-run-retains-worktree', '@durable-worker-evidence']
lane: open-habitat
lane_seq: 40
collision: [supervise-cleanup, run-public-contract]
closed: 2026-10-16
closed_by: pi
---

# Guarantee terminal run history survives teardown

## Description

## Request

Requested by Jari for Open Habitat on 2026-10-16: terminal runs must remain
listed and inspectable with their reports after worker/window/worktree teardown,
so a parent conversation still shows what its spinoffs did.

Current `run list` walks retained manifests, so this is a request to document
and prove the consumer contract, not a report that current releases delete runs.
Keep status, lifecycle, attention, landed, report/merge commit, source branch/OID,
preserved work, failure summary and timestamps available through list/show
as appropriate. The host derives diffstats from Git instead of storing copies.

## Acceptance Criteria

- [x] Successful merge, failed/cancelled preserved work, and interactive completion retain discoverable records and reports across teardown and supervisor restart.
- [x] Session-display maintenance cannot delete run records, reports or transcript evidence; this boundary is documented and regression-tested.

## Existing coverage

@agent-session-retention retains bounded inert tmux displays, not the run-history
contract. @durable-worker-evidence archives native transcripts.
@done-run-retains-worktree exposes unmerged resources even on historical Done
runs. Reuse these completed implementations and their tests rather than adding
a retention service, duplicate archive or second history store.

Consumer: native-agent-host `issues/manage-agent-worktrees/architecture.md` and
`issues/implement-spinoff-agents/item.md`.

## Comments

### 2026-10-16T09:20:33Z · @pi

Implemented as the existing-history guarantee, without product-code changes,
new stores, retention services or DTO changes.

Acceptance evidence:
- `crates/taskfleet/tests/e2e_spinoff.rs` now reuses the native round-trip
  fixture for autonomous/interactive merge and failed/cancelled preserved
  commits. Each verifies retained list/show status, lifecycle, attention,
  source/creator facts, report and transcript discovery after teardown and
  after a fresh `supervise --once` invocation. Failed/cancelled paths verify
  retained worktree, branch and one unmerged commit. Existing failed-run merge
  recovery continues to cover supervisor reattachment.
- `crates/taskfleet/tests/session_maintenance.rs` adds deterministic public
  maintenance with owned/active/foreign/shared/last-anchor display doubles,
  repeated invocation, stripped PATH, exact pane-only removal, append-only
  events, retained manifest identity/report and byte-identical original,
  resume, pane and terminal-report artifacts.
- Existing evidence tests prove real capture preserves original bytes and
  native identity across checkout removal and restarted capture, while only
  the resume header changes. Existing transcript fixtures distinguish missing,
  unavailable, not-recorded and unsupported evidence. Existing private tmux
  tests cover actual display isolation. These are cited, not duplicated, in
  `docs/TERMINAL-RUN-HISTORY.md`, linked from ARCHITECTURE and RUN-TRANSCRIPTS.

Validation: `TMPDIR=/var/tmp/taskfleet-history-build scripts/validate-local-release.sh`
passed on clean commit e6525b11b6acc167e39ea544df177ca85b73d810.
Log: `/var/tmp/taskfleet-history-gate-e6525b11.log` (1235 tests passed,
1 skipped, plus the complete fmt/clippy/docs/package/release-fixture gate).
Focused release tests passed (7 native round trips + 3 maintenance tests),
including a separate run with PATH containing only declared Git and absolute
fixture tools; Linux manager services are stubbed explicitly. No household
services or installed Taskfleet/skills were modified.

Review: focused source-grounded self-review of the changed tests/docs against
list/show DTOs, maintenance expiry, typed teardown and evidence capture. No
independent reviewer was needed: no material cleanup or concurrency code changed.
Consumer limitation documented rather than redesigned: list is inventory, show
supplies report/landing/preserved work; the current report has origin.worker_oid
(pre-rebase), not a guaranteed merge_commit field. Git details need verified Git
reads. This unit does not validate browser integration or real service-parent
restart survival; Linux lifetime has separate smoke evidence. Initial fixture
and Clippy failures were corrected before the passing complete gate.
