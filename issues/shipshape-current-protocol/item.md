---
created: 2026-09-08
updated: 2026-09-08
type: improvement
status: done
priority: high
lane: release-toolchain
closed: 2026-09-08
---

# Use current Shipshape release protocol

## Goal
Update Taskfleet release integration to latest published Shipshape (currently 0.12.2), replacing obsolete exact-0.10.1 assumptions with source-verified current contracts. User explicitly requested this before the release.

## Acceptance
- Use current released Shipshape without downgrading the installed tool.
- Remove superseded local mechanisms where the current engine owns them; retain the load-bearing exact-release-commit main CI gate, authorization-ref policy, canonical two-crate CI publication, and cargo-dist/Homebrew topology.
- Validate actual current-engine behavior with disposable local/stubbed protocols, failure-before-tag tests, repository gates, and updated documentation. No tag/publish/install by worker; conductor cuts release after landing.

## Agent Runs

### 2026-09-08T14:02:03Z · @codex

Implemented against Shipshape 0.12.2 d1d48d692707fee0d98697721e763a59e7ee3fb7 and its release.rs/coordinator sources. The engine now authenticates and restores the bump level from the stored plan, so the wrapper no longer parses private plan JSON or retransmits --bump. The custom adapter remains only because tag_phase still pushes before delegated dist/verify and advance_branch: it holds that tag, advances exact main, waits for green ci.yml on the exact bump SHA, records the protected authorization ref, then resumes the same immutable journal. Contract input is canonical schema v2 with the taskfleet-associated distribution. A disposable bare-origin fixture ran the real 0.12.2 binary through stored-bump cut, held journal, local-only resume/delegated failure, and a completed idempotent advance_branch phase; no public ref or registry was reachable. Release topology/publication/authorization/distribution fixtures also passed.

## Resolution

### 2026-09-08T14:11:38Z · @issuectl

Integrated Shipshape 0.12.2 with canonical schema-v2 distribution input; removed duplicate stored-plan parsing and --bump retransmission while retaining the exact-main-CI held-tag and authorization-ref adapter. Real-engine bare-origin fixture, release fixtures, full Rust/doc/identity gate, snapshots, and local release build passed.
