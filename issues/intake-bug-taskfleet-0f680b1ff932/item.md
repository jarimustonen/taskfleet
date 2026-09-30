---
created: 2026-09-30
updated: 2026-09-30
type: bug
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:nah-2026-09-30-discard-partial
---

# run discard leaves partial state after first call; retries refuse

## Description

run discard leaves partial state after first call; retries refuse

Observed 2026-09-30 (taskfleet 0.11.6) on three failed spinoff runs in native-agent-host whose worktrees were clean (dry-run: force_required false, cleanliness clean).

1. `taskfleet run discard <id> --reason "<R>" --output json` printed output that was not a single JSON envelope (jq: "Invalid numeric literal at line 1, column 11"), removed the git worktree registration but left the branch and directory.
2. Retry with a different reason: `idempotency_conflict` ("incomplete discard authorization with different reason/force inputs").
3. Retry with the same reason: `preserved_work_unverifiable`, because the directory remained without registration (the read-only Go module cache prevented removal; see the companion teardown bug).
4. After removing the directories manually, the same-reason retry completed and `preserved_work` became empty.

Expected: `--output json` always emits one envelope; discard is atomic or resumable from its own partial state (recognizes the worktree it already unregistered) instead of refusing as unverifiable.

<!-- intakectl:analysis:start job:3f8e799e-88d0-4a47-98af-76bf38f0fffe generation:0 -->
## Triage analysis

- **Verdict:** Confirmed, with a distinct cleanup failure mode contributing to the observed sequence.
- **Severity:** Medium. A failed discard can leave a partially removed worktree/branch state and then prevent an operator from retrying the authorized cleanup through the CLI. Recovery currently requires manual filesystem cleanup, and the initial `--output json` response is not machine-parseable.
- **Affected area:** `crates/taskfleet/src/run/discard.rs` and retained-work observation in `crates/taskfleet/src/run/retained.rs`.
- **Repro status:** The report describes three observed runs. Source inspection confirms discard writes its authorization before removing the worktree and branch. A cleanup failure therefore leaves a durable authorization. On retry, a changed reason/force conflicts while resources remain; if the worktree registration is gone but its directory survives, the observation is unverifiable and discard refuses to proceed. `emit` does select an envelope for JSON output, so the reported mixed output likely arises on an error path or adjacent output handling rather than the successful payload path; the exact source of the malformed first response is not established by this inspection.
- **Fix sketch:** Make retries resume safely from the recorded authorization and partial-removal facts without weakening repository/branch identity checks; distinguish an already-unregistered, previously authorized worktree from unrelated unverifiable retained data. Ensure JSON mode emits exactly one envelope on both success and errors, and add regression tests for worktree removal failure after registration removal, same/different-input retries, and JSON error output.
<!-- intakectl:analysis:end job:3f8e799e-88d0-4a47-98af-76bf38f0fffe generation:0 -->
