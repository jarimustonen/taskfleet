---
created: 2026-09-08
updated: 2026-09-08
type: bug
status: open
priority: high
lane: release-toolchain
---

# Read build paths from the current Cargo invocation

## Description

## Bug
The pre-tag local validation for release run 01M20SQVZF1QR1BNPZJXYCQ74Q passed 1100 tests, then rustdoc failed because taskfleet/build.rs tried to read skills from an already removed Shipshape release worktree. The conductor used a shared CARGO_TARGET_DIR. The build script compiles CARGO_MANIFEST_DIR into its executable with env!, so a cached build-script binary retains the original checkout path instead of reading Cargo's invocation environment.

## Acceptance Criteria
- [ ] Resolve the manifest directory from the build-script process environment at runtime.
- [ ] A focused relocation regression proves the same compiled build script works after its original directory disappears and receives a different runtime manifest directory.
- [ ] Validate actual Cargo checks with shared output across temporary checkout removal, and retain all previously passed unrelated Rust test evidence; conductor runs the full exact-bump gate for the next release.
- [ ] Fold unpublished 0.8.0 notes back into Unreleased for the next engine-owned 0.8.1 cut; do not change package versions or publish from the worker.

The 0.8.0 run is abandoned, its local tag removed, and no target was published. This is a source fix, not a cache-clearing workaround.
