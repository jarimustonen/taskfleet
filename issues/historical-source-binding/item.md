---
created: 2026-09-08
updated: 2026-09-09
type: improvement
status: obsolete
priority: high
lane: supervisor
collision: [taskfleet-supervisor]
disposition_note: Historical tree retirement is no longer required for migration; fully archived inactive trees remain recovery holdings. No source-binding implementation or deletion is claimed.
disposition_reason: withdrawn
closed: 2026-09-09
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

## Decisions

### 2026-09-09T11:14:20Z · @codex

The migration cleanup requirement is withdrawn. The seven inactive historical worktrees with null original source_repo are intentionally retained on Gertrud as recovery holdings, with their complete tree/common-Git/index/ref archives preserved and verified on recovery hosts. They have no live ownership and no longer block migrated main repositories or sessions. Close this issue obsolete, disposition withdrawn: no native source-binding feature was implemented, no deletion is authorized or claimed safe by this disposition, and no missing state is claimed reconstructed. Existing native fail-closed guards and all original branches/worktrees remain intact. This is a scope decision for the completed project migration, not an implementation success.

## Resolution

### 2026-09-09T11:14:20Z · @issuectl

Withdrawn migration cleanup requirement; preserve the seven archived historical worktrees and native guard behavior. No feature completion or safe deletion claim.
