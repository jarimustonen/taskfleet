---
created: 2026-09-08
updated: 2026-09-08
type: bug
status: fixed
priority: high
lane: release-toolchain
closed: 2026-09-08
---

# Read build paths from the current Cargo invocation

## Description

## Bug
The pre-tag local validation for release run 01M20SQVZF1QR1BNPZJXYCQ74Q passed 1100 tests, then rustdoc failed because taskfleet/build.rs tried to read skills from an already removed Shipshape release worktree. The conductor used a shared CARGO_TARGET_DIR. The build script compiles CARGO_MANIFEST_DIR into its executable with env!, so a cached build-script binary retains the original checkout path instead of reading Cargo's invocation environment.

## Acceptance Criteria
- [x] Resolve the manifest directory from the build-script process environment at runtime.
- [x] A focused relocation regression proves the same compiled build script works after its original directory disappears and receives a different runtime manifest directory.
- [x] Validate actual Cargo checks with shared output across temporary checkout removal, and retain all previously passed unrelated Rust test evidence; conductor runs the full exact-bump gate for the next release.
- [x] Fold unpublished 0.8.0 notes back into Unreleased for the next engine-owned 0.8.1 cut; do not change package versions or publish from the worker.

The 0.8.0 run is abandoned, its local tag removed, and no target was published. This is a source fix, not a cache-clearing workaround.

## Resolution

### 2026-09-08T16:03:35Z · @issuectl

Root cause: `env!("CARGO_MANIFEST_DIR")` compiled the disposable Shipshape checkout into the cached build-script executable. With a shared target, a later Cargo invocation reused that executable after the checkout had been removed and rustdoc panicked while reading `skills`. The fix resolves `CARGO_MANIFEST_DIR` from the build-script process environment with `var_os` + `PathBuf`; the remaining compile-time `CARGO_PKG_VERSION` is package identity and does not carry the path defect.

Validation evidence:
- Focused relocation regression passed twice against one compiled build script with two runtime manifest/OUT_DIR pairs after deleting its deliberately stale compile path. Against the original 8dfcad3 `build.rs`, the same regression panicked on the removed path and exited 101.
- Rust gate passed: fmt, clippy with warnings denied, 1100/1100 release nextest tests (1 leaky, 1 skipped), 2/2 doctests, and rustdoc with warnings denied. Version snapshots and canonical identity passed.
- Actual Cargo relocation passed using a fresh shared target: `cargo doc --locked --workspace --no-deps` ran in an owned disposable worktree, that worktree was removed, and docs succeeded from the current worktree with cached build-script executables byte-identical.
- Rust 1.85 workspace/all-targets check and cargo-deny passed. Release shell fixtures, held-tag and real Shipshape 0.12.2 protocol checks, release activation, both workspace package archives, cargo-dist generation/plan topology, contract validation, and zero blocking audit gaps passed.
- The abandoned unpublished 0.8.0 changelog notes are folded into Unreleased without duplicate generated issue-title bullets. The held-tag fixture now seeds its own simulated finalized changelog section, so it remains valid after an abandoned bump.

No package version, tag, publication, installation, or cache-clearing workaround was performed. The conductor owns the 0.8.1 bump and exact final release-SHA gate.
