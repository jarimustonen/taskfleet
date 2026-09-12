---
created: 2026-09-12
updated: 2026-09-12
type: bug
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:taskfleet-salvage-dry-run-dirty-target-20260911
---

# Salvage dry-run misses dirty target worktree

## Description

Salvage dry-run misses dirty target worktree

`taskfleet run salvage <run-id> --dry-run --output json` reported an eligible salvage plan while the recorded target worktree `/home/jari/Sources/homebase` had unrelated uncommitted changes. Running the same salvage without `--dry-run` immediately failed:

```
{"schema_version":1,"error":{"code":"merge_failed","message":"merge.sh exited 1 merging wt/cdgdeetdye-additive-history-transfer-repair: Error: Uncommitted changes in target worktree (/home/jari/Sources/homebase)\nPlease commit or stash changes in the target before merging","invalid_value":"wt/cdgdeetdye-additive-history-transfer-repair"}}
```

Observed with Taskfleet 0.10.0 on Haapa. After the unrelated change was committed, the same salvage succeeded.

Expected: `run salvage --dry-run` says it performs all read-only eligibility checks and should detect target-worktree dirtiness, returning a structured refusal before the operator relies on the plan. Alternatively its output/help must explicitly state that target cleanliness is deferred, but checking it in dry-run is safer and consistent with the command contract.

Suggested regression: prepare an otherwise salvageable failed/false-failed single-worker run, dirty the target worktree with an unrelated unstaged file, and assert dry-run refuses with the same stable error class as actual salvage while leaving run and Git state unchanged.
