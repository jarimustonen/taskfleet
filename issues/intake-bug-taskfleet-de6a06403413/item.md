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
