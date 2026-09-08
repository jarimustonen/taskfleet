---
created: 2026-08-20
updated: 2026-09-08
type: feature
reporter: jari
status: done
priority: normal
provenance: agent:homebase-triage
source_ref: homebase:tg-bug-jari-1287848359-12920321-7d8c-4199-832b-0f120de9ab13
closed: 2026-09-08
---

# run show cannot identify a run repository once its worktree is gone

## Description

# run show cannot identify a run repository once its worktree is gone

## Description

run show cannot identify a run repository once its worktree is gone

## Observed

`taskfleet run show <id> --output json` exposes no field naming the repository a run belongs to. For a live run the repo is inferable from `worktree_path`, but once the worktree is torn down that field is null and nothing else identifies the repo. A stalled run reports worktree_path = null, source_branch = "main", status = "pending". Since `source_branch` is `main` for nearly every run, it disambiguates nothing.

Full top-level key set from `run show --output json`: attention_required, awaiting_input, counts, created_at, harness, kind, landed, landed_method, lifecycle, manifest, node_count, open_discussion_count, report, run_id, source_branch, stalled, status, stillborn, supervisor, title, worktree_path.

## Expected

A stable field identifying the owning repository (repo root path and/or repo name), recorded at run create and retained after teardown, surfaced by run show and ideally filterable in run list.

## Why this matters: the concrete blocked workflow

The /stint-handoff skill mandates a preflight verifying that no live, awaiting-input, or resumable worker still owns work before a session wraps. That requires answering "which repo does this run belong to?" for every non-terminal run.

On 2026-08-20 that preflight found 7 non-terminal runs on the machine. Six were old stalled runs with dead supervisors and no worktree, and run show could not say which repository any of them belonged to. The only way through was grepping raw JSON under the run state directory by hand, which worked only for the one run that still had a worktree recorded. For the other six the answer had to be inferred from the run title.

So the preflight that the handoff skill requires cannot currently be completed from the CLI read surface once runs age out. An orchestrator either guesses or reads private state.

## Impact

Blocks a documented wrap-up safety check from being answered programmatically. Not a correctness bug in run execution; a gap in the read surface.

## Related, do not merge with this

The six perpetually-pending stalled runs are separately tracked as intake-bug-taskfleet-169460ea27e7 (stale pending runs clutter run list and look like live workers). This request is only about repo identity being unavailable, which would still matter if that cleanup shipped.

## Resolution

### 2026-09-08T08:40:25Z · @issuectl

Taskfleet now persists and exposes source_repo on run read surfaces; the accepted follow-up agenda separately covers canonical repository filtering.
