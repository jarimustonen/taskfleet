---
created: 2026-10-20
updated: 2026-10-22
type: bug
reporter: agent
status: fixed
priority: normal
provenance: other
provenance_detail: Observed installed-tool failure during Homebase stint
source_ref: homebase-stint:2026-10-20:discard:01m5cmshg3cy1x78avsqe0hy0s
lane: resources
lane_seq: 10
closed: 2026-10-22
closed_by: pi-worker
---

# Discard of clean initialized-submodule worktree gets stuck behind its non-force authorization

## Description

## Observed report

Observed during an autonomous Homebase stint on 2026-10-20 with installed taskfleet 0.16.0. A failed, Taskfleet-owned spinoff retained a clean worktree and five commits. Its reviewed changes and issue evidence were subsequently harvested by a new worker and landed with native run merge. The two changed test files were byte-identical on current source. The retained worktree has the repository's two pinned Git submodules initialized.

Commands used one unchanged, nonempty reason throughout:

1. `taskfleet run discard RUN --reason REASON --dry-run --output json` succeeded: cleanliness=clean, force_required=false, unmerged_commits=5, both worktree and branch present.
2. The real call with identical inputs failed exit 2, error.code=`discard_partial_cleanup`: "discard authorization event 10 is durable, but worktree ... was not removed; retry with the same reason and force inputs".
3. One retry with exactly the same reason and no force produced the same error.
4. A read-only `--force --dry-run` probe was refused with `idempotency_conflict`: "node n-0001 has an incomplete discard authorization with different reason/force inputs".
5. `run show` still verifies the worktree and branch, cleanliness=clean, unmerged_commits=5, reason=terminal-work-preserved.

A second, older clean failed worktree from the same recovery chain, whose submodules were not initialized, was discarded successfully with the same style of command. This difference is context, not proof of the underlying Git failure: the CLI's error does not disclose why worktree removal failed. No manual worktree/filesystem/state edits were made to bypass Taskfleet. The blocked worktree remains preserved.

Expected: the dry-run accurately reports any required force or unsupported cleanup condition; the real error exposes the underlying refusal and provides a safe public recovery route. A durable non-force authorization must not strand a clean, verified worktree when the tool's prescribed identical retry cannot progress.

Related history: @intake-bug-taskfleet-0f680b1ff932 is already fixed and describes partial deregistration/read-only module-cache cleanup. This occurrence is on 0.16.0, returns clean JSON, and retained-work verification still succeeds; it is not a claim that the old fix is absent. No new disposable reproduction has been run.

## Acceptance Criteria

- [x] Clean initialized or deinitialized submodule worktrees truthfully preview force requirements and refuse non-force deletion before recording an authorization.
- [x] Already-recorded non-force authorizations support explicit, separately audited force recovery without changing target identity, branch OID pins or reason.
- [x] Dirty registered targets, replacement directories/repositories, changed branch tips and changed reasons remain protected.
- [x] Real disposable Git regressions and the complete local release gate pass.

## Resolution

Discard now inspects both worktree-private module storage (which remains after submodule deinit) and populated gitlink directories. These require deliberate operator --force; dry-run performs the same read-only eligibility and receipt-identity checks. Historical non-force receipts can be upgraded only for their original verified clean submodule target or the previously supported authenticated orphan recovery. A separate cleanup.discard_force_authorized event references the immutable original receipt before deletion. Git refusal detail is included in partial-cleanup errors. Branch-only retries preserve the source Git-directory pin and exact OID-conditional deletion. No supervisor deletion policy or automatic force escalation was added.

Regression coverage uses real disposable submodule worktrees and stripped CLI PATHs: initialized/deinitialized state, historical receipts, absent-branch orphan recovery, partial branch deletion, changed/dirty targets, and a replacement source repository carrying identical refs. No historical fixture or installed binary/skills were modified.

Focused critical review: claude-fable-5-1 finder and gpt-6-astra verifier; two findings corrected, one missing-coverage claim refuted by the existing regression. Assessment and raw evidence are retained in /var/tmp/taskfleet-b6mka4wr19/evidence.

Validation: scripts/validate-local-release.sh passed on clean d7096c80dfd59657dd35ec2096c24c8cb8bdd3da after rebase onto main 156a96bedfff2d6b0879ff7a35ff241d1e22c254: 1256 release tests passed, one configured skip; doctests, rustdoc, snapshots/identity, cargo-deny, release fixtures, packages/distribution and real Shipshape protocol checks passed. Log: /var/tmp/taskfleet-b6mka4wr19/evidence/discard-gate-integrated.log. Two unrelated fixture dependencies found by the gate were independently fixed on main; duplicate worker fixture commits were dropped during rebase. No unresolved review findings or new issues.
