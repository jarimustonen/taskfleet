---
created: 2026-09-30
updated: 2026-09-30
type: bug
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:nah-2026-09-30-teardown-gomod
---

# Teardown leaves worktree dirs when Go module cache is read-only

## Description

Teardown leaves worktree dirs when Go module cache is read-only

Observed in native-agent-host (2026-09-30, taskfleet 0.11.6): 31 spinoff worktree directories (~46 GB) stayed on disk after successful `run merge` teardown. `git worktree list` no longer listed them, but the directories remained with a `.git` file pointing at a removed gitdir, and `run show` reported `preserved_work` rows with `cleanliness: unverifiable`, reason `done-with-unverifiable-retained-work`.

Cause: the repo's gate sets `GOMODCACHE=<worktree>/.cache/ci-local/go-mod`; Go writes the module cache read-only (0555 dirs), so recursive removal fails with Permission denied and teardown silently leaves the tree.

Expected: teardown removes the worktree directory completely (e.g. `chmod -R u+w` before removal, or retry after making the tree writable) and reports failure instead of leaving an unverifiable retained-work row.

Repro: in a worktree, `GOMODCACHE=$PWD/.cache/gomod go mod download`, then let the run merge and tear down.
<!-- intakectl:analysis:start job:0a310917-84b6-4166-85c0-f9014dd9e6b8 generation:0 -->
## Triage analysis

- **Verdict:** Confirmed; the reported behavior is consistent with the teardown implementation.
- **Severity:** Moderate. A successful merged run can leave a large worktree directory consuming substantial disk space, while its gitdir reference is stale and taskfleet no longer tracks it as a worktree.
- **Affected area:** `crates/taskfleet/src/supervise/cleanup.rs` and `crates/taskfleet/src/git/repo.rs`. Cleanup calls `git worktree remove` (forced on confirmed merges); the wrapper only reports command success/failure. On failure with the directory still present, cleanup preserves it and records a preservation outcome. There is no writable-permission repair/retry path.
- **Reproduction status:** The issue report describes an observed occurrence in native-agent-host with 31 leftover directories (~46 GB), and gives a concrete `GOMODCACHE` reproduction. Source inspection confirms the missing recovery path; this worker did not rerun the environment-specific reproduction.
- **Fix sketch:** After an authorized removal attempt fails, distinguish permission-related removal failure from a safety refusal. For an explicitly merged worktree, make the retained tree writable (for example, recursively grant owner write/search permissions) and retry removal; report any remaining failure clearly. Keep non-merge teardown fail-closed and do not weaken its preservation guarantees. Add a regression test using a read-only nested directory that exercises the retry and verifies the directory is gone.
<!-- intakectl:analysis:end job:0a310917-84b6-4166-85c0-f9014dd9e6b8 generation:0 -->
