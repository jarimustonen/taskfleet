---
created: 2026-09-18
updated: 2026-09-18
type: improvement
status: done
priority: normal
provenance: other
provenance_detail: Taskfleet implementation run
source_ref: taskfleet:01m2smwwfkmzwt6by907r66qpe/task:latest-stable-rust
originating_run: 01m2smwwfkmzwt6by907r66qpe
originating_run_kind: spinoff
closed: 2026-09-18
---

# Track latest stable Rust for builds and releases

## Description

## Goal

Remove the obsolete Rust 1.85 compatibility target. Build, validate, and release Taskfleet with the current stable Rust/Cargo channel, without promising an unsupported minimum Rust version in published package metadata.

## Acceptance Criteria

- [x] Active build and release validation selects the stable Rust channel and has no old-toolchain gate.
- [x] Package metadata and publish validation do not claim Rust 1.85 compatibility.
- [x] The `time` 0.3.41 pin and RUSTSEC-2026-0009 exception are removed; the lockfile uses a fixed `time` release and cargo-deny is clean.
- [x] Active policy, fixtures, and tests reflect latest-stable operation; historical issue and changelog evidence is preserved.
- [x] The complete local release gate passes with the declared cargo-deny and cargo-dist binaries.

## Resolution

### 2026-09-18T07:17:56Z · @issuectl

Stable Rust/Cargo now owns builds and release validation; untested MSRV metadata and the vulnerable time pin/advisory exception were removed. The complete local release gate passed on stable 1.98.1.
