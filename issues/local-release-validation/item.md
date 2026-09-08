---
created: 2026-09-08
updated: 2026-09-08
type: improvement
status: done
priority: high
lane: release-toolchain
closed: 2026-09-08
commits:
- hash: c2892b5203209bfc57c33d570f7eb61e31ef4a9d
  summary: replace GitHub test CI with exact-commit local validation
---

# Use local validation instead of GitHub test CI

## Description

## Goal
Remove GitHub CI test execution per maintainer instruction on 2026-09-08; retain GitHub distribution publication. Replace the pre-tag GitHub CI dependency with one local validation command on the exact release commit, preserving tag/version/pin correctness and resumable publication.

## Acceptance Criteria
- [x] Remove main/PR test CI and repeated test jobs from crates.io publication.
- [x] One local validation command owns the applicable existing checks, and the release wrapper requires it on the exact clean bump commit before recording authorization or pushing a tag.
- [x] Keep two-crate CI publication, cargo-dist binaries/Homebrew, protected authorization refs and current Shipshape 0.12.2. Update contract, policy, tests and docs consistently without introducing another orchestration framework.
- [x] Verify local-check failures stop before tag; no missing CI workflow lookup remains in maintained release logic. Worker must not publish or install user tools.

## Agent Runs

### 2026-09-08T14:20:13Z · @taskfleet-agent

Run 01m20p66se7bshysqzfh7bw9qk started implementation. Local prerequisites found: cargo-nextest present, Rust 1.85 toolchain present, pinned cargo-dist 0.28.2 available at /tmp/taskfleet-cargo-dist-028-target/release/dist, exact Shipshape 0.12.2 present. cargo-deny is missing; full local validation will fail closed until the conductor-provided temporary binary is available. Worker will not install globally.

### 2026-09-08T15:18:01Z · @taskfleet-agent

Run 01m20p66se7bshysqzfh7bw9qk implementation evidence: commit c2892b5203209bfc57c33d570f7eb61e31ef4a9d deletes ci.yml and centralizes local validation while retaining protected ordered publication. The full script on precursor c668d63 passed fmt, clippy, all 1100 release nextest tests, doctests, rustdoc, Rust 1.85 check, cargo-deny, identity/snapshots, every release fixture including the real Shipshape 0.12.2 held/resume protocol, and activation; it then exposed a real CARGO_TARGET_DIR package-path assumption. The helper now derives Cargo's target_directory. Focused default/custom-target fixtures and actual custom-target archives passed at c2892b5. cargo-dist generate/check plus real plan topology, contract validation, Shipshape audit (0 blocking gaps), snapshots/identity, and final clean-tree checks passed at c2892b5. No public refs, registries, installed tools, skills, or generated release.yml were changed. Local macOS execution does not prove Linux runtime behavior; retained publication workflows own cross-platform builds. The conductor will run the complete exact-bump gate during the actual held-tag cut.

## Resolution

### 2026-09-08T15:18:11Z · @issuectl

Implemented and validated under Taskfleet run 01m20p66se7bshysqzfh7bw9qk; conductor retains the actual release cut.
