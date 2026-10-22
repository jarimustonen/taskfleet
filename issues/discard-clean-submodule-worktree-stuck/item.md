---
created: 2026-10-20
updated: 2026-10-22
type: bug
reporter: agent
status: open
priority: normal
provenance: other
provenance_detail: Observed installed-tool failure during Homebase stint
source_ref: homebase-stint:2026-10-20:discard:01m5cmshg3cy1x78avsqe0hy0s
lane: resources
lane_seq: 10
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
