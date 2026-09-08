---
created: 2026-09-08
updated: 2026-09-08
type: improvement
status: open
priority: high
lane: release-toolchain
---

# Use local validation instead of GitHub test CI

## Description

## Goal
Remove GitHub CI test execution per maintainer instruction on 2026-09-08; retain GitHub distribution publication. Replace the pre-tag GitHub CI dependency with one local validation command on the exact release commit, preserving tag/version/pin correctness and resumable publication.

## Acceptance Criteria
- [ ] Remove main/PR test CI and repeated test jobs from crates.io publication.
- [ ] One local validation command owns the applicable existing checks, and the release wrapper requires it on the exact clean bump commit before recording authorization or pushing a tag.
- [ ] Keep two-crate CI publication, cargo-dist binaries/Homebrew, protected authorization refs and current Shipshape 0.12.2. Update contract, policy, tests and docs consistently without introducing another orchestration framework.
- [ ] Verify local-check failures stop before tag; no missing CI workflow lookup remains in maintained release logic. Worker must not publish or install user tools.
