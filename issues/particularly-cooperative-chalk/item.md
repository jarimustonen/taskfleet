---
created: 2026-10-16
updated: 2026-10-16
type: feature
status: untriaged
priority: normal
provenance: ai-review
source_ref: taskfleet:01m521vczarx5bkx086n64t1t2/review-finding:sha1:a812016fda7c4b258a686b029d1c68af15d6cce1
review_source: ai-review
originating_run: 01m521vczarx5bkx086n64t1t2
originating_run_kind: spinoff
labels:
- ai-review-model:gpt-6-astra
- ai-review-model:claude-fable-5-1
assessment_classification: CONFIRMED
assessment_outcome: SPIN_OFF
review_severity: P2
review_target: taskfleet@8464733b0d69cf1342eed64995163fd36c33523d:crates/taskfleet/src/run/merge.rs
---

# Expose the integrated merge OID for Taskfleet-owned runs

## Description

## Goal

Give read-only observers an exact integrated Git object for an ordinary
Taskfleet-owned merge, so they can derive the actual commit count, diffstat and
file list without inventing a historical source tip or copying Git history.
Open Habitat ADR 0017 observes these ordinary native runs, not caller-owned ones.

## Evidence and current limitation

In released Taskfleet 0.15.0 (8464733b0d69cf1342eed64995163fd36c33523d) and source
c9382b75, `crates/taskfleet/src/run/merge.rs:768-774` calls `run_git_merge` from
ordinary `execute()` with a no-op before-ref-move callback. That callback receives
the integrated post-rebase OID but discards it. `ReportOrigin::RunMerge` retains
`op_id` and `worker_oid`, where worker_oid is captured before rebase.

The sole `merge.rebased` producer is in `execute_caller_merge` around lines
1066-1076; dispatch around 172-190 chooses that path only for agent_owner=caller.
It carries caller intent/recovery context, not an ordinary worker result.
`docs/TERMINAL-RUN-HISTORY.md` correctly warns that worker_oid is not a guaranteed
merge commit, but its event-to-Git advice cannot recover an ordinary integrated
OID when rebase changed the hash.

An Open Habitat consumer experiment used that caller-only event and a synthetic
fixture that manufactured it. Source-grounded review exposed the mismatch and
the helper was removed before landing. Landing/report/transcript remain usable;
exact integrated Git details are unavailable rather than guessed. Local source
inspection at both revisions and an independent verifier confirmed the gap.

## Proposed direction

Prefer an optional exact `integrated_oid` on the native RunMerge report (or an
equally explicit ordinary-run read contract). Capture the post-rebase tip while
native merge authority still holds the source fence; preserve it through crash
recovery and teardown. Do not move settlement authority to the observer or
casually reuse caller-owned intent events. Leave legacy/unverifiable results
explicitly without this evidence.

## Acceptance Criteria

- [ ] An actual ordinary Taskfleet-owned rebase-and-merge exposes its exact integrated OID and operation identity through retained native reads.
- [ ] An ordinary merge round-trip fixture uses the real producer stream/report, not a manufactured caller-only event, and verifies the Git range after teardown/restart.
- [ ] Caller-owned intent, fencing and crash recovery retain their existing authority and invariants.
- [ ] Legacy/pruned evidence is explicit; consumer documentation does not imply worker_oid is the historical source result.
