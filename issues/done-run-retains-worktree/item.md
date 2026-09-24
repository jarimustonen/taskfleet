---
created: 2026-09-24
updated: 2026-09-24
type: bug
status: fixed
priority: high
lane: run-lifecycle
closed: 2026-09-24
commits:
- hash: e3e8880e
  summary: Surface retained resources in done runs and grade report-only holds
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

## Comments

### 2026-09-24T15:04:52Z · @agent

Implementation staged in worker branch; focused and workspace tests plus clippy pass. Required scripts/validate-local-release.sh is blocked at preflight: cargo-deny is unavailable on this host. Issue remains in-progress until the full gate can pass on the recovered branch.

### 2026-09-24T15:05:29Z · @agent

Repository-local release binary reproduced both reported Done runs as preserved_work (one row, two unmerged commits each); run wait --fail-on-error returned 3 and report_only=true for both. Committed implementation at 1d2a0f52. Gate remains blocked by missing cargo-deny; no release or install performed.

## Resolution

### 2026-09-24T15:16:48Z · @issuectl

Recovered and reviewed the preserved commits; full scripts/validate-local-release.sh passed with disposable cargo-deny 0.20.2 and cargo-dist 0.33.0. Done report-only runs now expose retained resources and fail --fail-on-error when resources remain; no release or installed tool changed.
