---
created: 2026-09-24
updated: 2026-09-24
type: bug
status: open
priority: high
lane: run-lifecycle
---

# Done run retains unmerged worktree invisibly

## Description

Taskfleet 0.11.1 treats an agent-authored `node.report {success:true}` without `run merge` as node Done and rolls a single-worker run up to Done, while source-relative cleanup preserves a worktree with commits not in the source branch. In the observed taskfleet repository, runs 01m20dmw4x1cr63yzq476j7nzn and 01m20bgrzjbhh7ceazqt1b3d29 are Done with preserved worktrees (two commits each ahead of main) and `run show.preserved_work: []` because retained.rs filters to Failed/Cancelled. This makes a stint say complete while Git work remains.

## Acceptance criteria

- Define an explicit, coherent terminal contract for ordinary `success:true` reports that skipped `run merge`, without inventing merge authority or losing valid report-only work (e.g. external quarantine deliveries). Prefer a minimal safe change; document how callers finish or acknowledge a no-merge workflow. Maintain compatibility with historic event replay and the merge transaction invariants.
- Surface every remaining owned worktree/branch in a Done run in the read surface, not only Failed/Cancelled, with an unambiguous reason/attention signal. Ensure wait/stint consumers cannot mistake Done for verified landed work.
- Preserve existing fail-closed cleanup: never force-delete unmerged or dirty work just to make Done look clean.
- Cover merge success, report-only success with unmerged commits, clean no-work report-only success, cancel/failure and legacy reports with focused tests, plus repository green gate (`scripts/validate-local-release.sh`).
- Verify local binary only; do not install taskfleet or its skills while working in this repository. Do not cut a release from the worker; conductor owns it.

## Evidence

`crates/taskfleet-core/src/reducer.rs::report_terminal_status` maps any `success:true` to Done; `crates/taskfleet/src/supervise/cleanup.rs` preserves unmerged commits; `crates/taskfleet/src/run/retained.rs::observe` excludes Done. Recent explicit-merge runs do tear down normally.
