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
