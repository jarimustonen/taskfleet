---
created: 2026-09-08
updated: 2026-09-08
type: improvement
status: open
priority: high
lane: release-toolchain
---

# Use current Shipshape release protocol

## Goal
Update Taskfleet release integration to latest published Shipshape (currently 0.12.2), replacing obsolete exact-0.10.1 assumptions with source-verified current contracts. User explicitly requested this before the release.

## Acceptance
- Use current released Shipshape without downgrading the installed tool.
- Remove superseded local mechanisms where the current engine owns them; retain the load-bearing exact-release-commit main CI gate, authorization-ref policy, canonical two-crate CI publication, and cargo-dist/Homebrew topology.
- Validate actual current-engine behavior with disposable local/stubbed protocols, failure-before-tag tests, repository gates, and updated documentation. No tag/publish/install by worker; conductor cuts release after landing.
