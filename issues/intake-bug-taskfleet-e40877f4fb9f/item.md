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

<!-- intakectl:analysis:start job:8a7a9238-8520-462b-84fe-e926746002f9 generation:0 -->
## Triage analysis

### Root cause

The target-worktree dirtiness check lives entirely inside `merge.sh` — the shell script invoked by `run merge` (and by `run salvage`, which delegates to the same `merge::execute` machinery). Specifically, `merge.sh` acquires a serializing `mkdir` lock, then checks `git -C "$TARGET_PATH" status --porcelain` under the lock. If that check finds uncommitted changes, `merge.sh` exits 1 and `merge::execute` maps that to `merge_failed`.

During `--dry-run`, `merge::execute` never spawns `merge.sh`. The dry-run path (lines ~403-412 of `merge.rs`) only validates the report-file schema, checks run status, resolves the effective source branch, and returns a preview — no git operations on the target worktree are performed. The same applies when `salvage::run` calls `merge::execute` with `dry_run: true` as its pre-fence validation step: the preview shows a clean plan, but the target may be dirty.

### Fix scope

Add a target-worktree dirtiness check in the Rust code during the dry-run path. The check should:

1. Resolve the target worktree path from `git worktree list` using the effective source branch (the same logic `merge.sh` uses).
2. Run `git -C <target-path> status --porcelain`.
3. If the output is non-empty, refuse with a structured error using the same error class (`merge_failed`) the real merge would produce, so the operator sees a stable, actionable refusal before relying on the plan.

This should be added in `merge::execute()` after resolving `effective_source` but before the dry-run early return, or alternatively in `salvage::run()` before it calls the dry-run preview. Placing it in `merge::execute` is better because both `run merge --dry-run` and `run salvage --dry-run` would then surface the same check, and the error code is shared.

### Location

`crates/taskfleet/src/run/merge.rs`, the `execute()` function, around lines 390-410 where `effective_source` is resolved and the dry-run early return happens. Add the target-dirtiness check between report validation and the dry-run return, gated only on `effective_source` being Some (since merge.sh has a fallback to auto-detection when source is None, but the Rust code can't replicate that without invoking git).

### Alternative

If the maintainers prefer to keep the dirtiness check inside `merge.sh` as the single source of truth (avoiding a second, potentially divergent implementation), `--dry-run` could instead be documented as *not* checking target cleanliness, and the help text would be updated to say "Read-only preview: validates report-file schema, resolves source branch, and refuses cancelled/legacy runs. Does NOT check target-worktree cleanliness — a clean preview does not guarantee the real merge will succeed." This is the lower-effort path but leaves the operator with a misleading preview.

### Test coverage

The existing `dry_run_previews_without_mutating` test (`run_salvage.rs` line 138) would need a companion that dirties the target worktree and asserts dry-run refusal with the expected error class. The test setup is straightforward: after seeding the run and adding the worker node, write an untracked file into the target worktree, then call `run salvage --dry-run` and assert the error envelope.
<!-- intakectl:analysis:end job:8a7a9238-8520-462b-84fe-e926746002f9 generation:0 -->
