---
created: 2026-09-08
updated: 2026-09-08
type: improvement
status: open
priority: high
lane: supervisor
collision: [taskfleet-supervisor]
---

# Validate explicit source binding for archived historical run retirement

## Goal

Provide a narrowly validated native retirement path for historical failed/cancelled runs whose original source_repo field is null. This blocks seven fully archived inactive Gertrud worktrees in Jari-authorized Haapa migration.

## Observed evidence

Released0.8.2 run discard --dry-run fails preserved_work_unverifiable for all seven. Original manifests and run.created events never recorded source identity; no alternate legacy field or plan contains it. Immutable node.created does contain absolute worktree path, branch and base_commit. Original archived reciprocal Git worktree registrations still match current Git metadata. No current owner exists; caller has full tree/commonGit/index/ref archives, but must not edit manifests or bypass native guards. Diagnosis /tmp/haapa-historical-recovery-diagnosis.md is operational evidence, not the sole description.

## Required behavior

Add the smallest explicit caller-supplied source binding for this legacy-null case, preferably a narrowly scoped discard option rather than a generic metadata editor. Validate candidate repo/commonGit identity against exact original node path, branch/base and reciprocal registration, canonical paths/ownership, and all existing live/unverifiable supervisor/worker guards under the native operation lock. Reject mismatched, nested/replacement, symlinked, missing, live and ambiguous cases; never override a non-null recorded source. Dry-run stays read-only. Before deletion durably audit the original missing identity, supplied validated binding and all existing discard authorization data. Preserve original failed/cancelled status, no success/landing fabrication, no manifest rewriting. Retry semantics retain exact binding/reason/force equality after partial cleanup.

## Acceptance

Hermetic real-Git fixtures reproduce null-original-source failure and prove safe explicit binding with untouched original metadata; negative identity/liveness/replacement/path and partial retry cases. No use against real historical trees from worker. Run exact scripts/validate-local-release.sh plus focused independent review/assessment. Conductor releases production unit, later Homebase distribution-channel update and guarded archived retirement. This is necessary migration recovery, not an unsolicited general repair framework.
