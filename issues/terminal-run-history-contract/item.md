---
created: 2026-10-16
updated: 2026-10-16
type: task
reporter: jari
status: open
priority: normal
provenance: chat
source_ref: taskfleet:01m51nrfvbqwsybz0gdggz6q4b/request:terminal-run-retention
originating_run: 01m51nrfvbqwsybz0gdggz6q4b
originating_run_kind: spinoff
related: ['@agent-session-retention', '@done-run-retains-worktree', '@durable-worker-evidence']
lane: open-habitat
lane_seq: 40
collision: [supervise-cleanup, run-public-contract]
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

- [ ] Successful merge, failed/cancelled preserved work, and interactive completion retain discoverable records and reports across teardown and supervisor restart.
- [ ] Session-display maintenance cannot delete run records, reports or transcript evidence; this boundary is documented and regression-tested.

## Existing coverage

@agent-session-retention retains bounded inert tmux displays, not the run-history
contract. @durable-worker-evidence archives native transcripts.
@done-run-retains-worktree exposes unmerged resources even on historical Done
runs. Reuse these completed implementations and their tests rather than adding
a retention service, duplicate archive or second history store.

Consumer: native-agent-host `issues/manage-agent-worktrees/architecture.md` and
`issues/implement-spinoff-agents/item.md`.
