---
created: 2026-09-18
updated: 2026-09-18
type: chore
status: done
priority: normal
provenance: other
provenance_detail: Taskfleet implementation run
source_ref: taskfleet:01m2sm3hg6qqwnd01315ph58bm/task:cargo-dist-0.33.0
originating_run: 01m2sm3hg6qqwnd01315ph58bm
originating_run_kind: spinoff
closed: 2026-09-18
---

# Upgrade cargo-dist contract to 0.33.0

## Description

## Goal

Upgrade Taskfleet's active cargo-dist contract from 0.28.2 to 0.33.0 while preserving release targets, the custom macOS runner, Homebrew publication, attestations, and protected release-wrapper behavior.

## Acceptance Criteria

- [x] `dist-workspace.toml` and generated `.github/workflows/release.yml` use cargo-dist 0.33.0.
- [x] Active release-gate assertions, diagnostics, fixtures, and AGENTS.md identify 0.33.0.
- [x] Historical issue validation records remain unchanged.
- [x] cargo-dist 0.33.0 `generate --check`, `plan`, focused release fixtures, and the sole local release gate pass with the disposable pinned binary.
- [x] No release is cut and no installed Taskfleet binary or skill is changed.

## Resolution

### 2026-09-18T07:17:56Z · @issuectl

Cargo-dist 0.33.0 configuration, generated workflow, topology validation, and release fixtures were adopted and passed the complete local release gate.
