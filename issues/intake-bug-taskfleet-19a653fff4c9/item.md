---
created: 2026-08-19
updated: 2026-09-08
type: bug
reporter: jari
status: fixed
priority: normal
provenance: agent-homebase-wrapup
closed: 2026-09-08
---

# run create omits source_repo from fresh run manifest

## Description

run create omits source_repo from fresh run manifest

Observed

From `/Users/jari/Sources/homebase`, this command created a valid autonomous run and worktree:

`taskfleet run create --kind spinoff --headless --source-branch main --title homebase-i2-characterization --prompt-file /tmp/homebase-i2-characterization.md --output json`

The returned worktree was `/Users/jari/Sources/homebase__worktrees/wt-01m0cb7v02-homebase-i2-characterization`, but `taskfleet run show 01m0cb7v02kdftt8vy3gz4390e --output json` reported `.data.manifest.source_repo: null`. The field remained null after the run landed successfully.

Impact

`taskfleet run list` is global across repositories. Stint orchestration must enumerate live/resumable runs and map each relevant run to the current repository before reconstructing issuectl lane/collision reservations. A missing source repo forces inference from an optional worktree path and becomes ambiguous after teardown or for malformed/stillborn runs.

Expected

`run create` should persist the canonical source repository path in `manifest.source_repo` whenever it successfully resolves the source branch and creates a worktree. `run show` and `run list` should expose that durable value after teardown.

Version

`taskfleet 0.4.1` (`8777b2e3c5b891abf396c6486c9e81e17ffcfe85`).

## Resolution

### 2026-09-08T08:39:43Z · @issuectl

Current Taskfleet records the canonical source repository at run creation and retains it after teardown. The reported 0.4.1 omission was corrected by the shipped source_repo implementation.
