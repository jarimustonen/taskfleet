---
created: 2026-09-18
updated: 2026-09-18
type: chore
status: in-progress
priority: normal
provenance: other
provenance_detail: Taskfleet implementation run
source_ref: taskfleet:01m2sm3hg6qqwnd01315ph58bm/task:cargo-dist-0.33.0
originating_run: 01m2sm3hg6qqwnd01315ph58bm
originating_run_kind: spinoff
---

# Upgrade cargo-dist contract to 0.33.0

## Description

## Goal

Upgrade Taskfleet's active cargo-dist contract from 0.28.2 to 0.33.0 while preserving release targets, the custom macOS runner, Homebrew publication, attestations, and protected release-wrapper behavior.

## Acceptance criteria

- `dist-workspace.toml` and generated `.github/workflows/release.yml` use cargo-dist 0.33.0.
- Active release-gate assertions, diagnostics, fixtures, and AGENTS.md identify 0.33.0.
- Historical issue validation records remain unchanged.
- cargo-dist 0.33.0 `generate --check`, `plan`, focused release fixtures, and the sole local release gate pass with the disposable pinned binary.
- No release is cut and no installed Taskfleet binary or skill is changed.
