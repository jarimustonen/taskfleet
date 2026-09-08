---
created: 2026-09-02
updated: 2026-09-08
type: epic
status: done
priority: high
owner: jari
closed: 2026-09-08
---

# Establish Taskfleet as the sole product identity

## Goal

Complete the Taskfleet identity across this repository and each separately owned
dependent repository without retaining a second identity in maintained source.

## Decision

ADR 0002 was amended on 2026-09-06 to require a clean break. This repository
ships two Cargo packages, one executable, one state/config root, one environment
and protocol namespace, one skill catalog, and one release/distribution
identity. Immutable external artifacts and git history are not rewritten.

## Active work

- `@taskfleet-zero-legacy-identity` removes the completed transition machinery,
  canonicalizes every maintained surface, and proves a zero-reference inventory.
- Dependent repositories are converged independently under their own repository
  instructions; this repository does not edit or deploy them.

## Completion

Close this epic only after the repository clean-break issue has passed its full
gate and the conductor has separately verified dependent repository convergence.

## Acceptance Criteria

- [x] Taskfleet maintained source passes the canonical identity inventory and full integrated gate.
- [x] All 38 local owned/work repository HEADs were audited; four current-guidance findings were corrected and pushed.
- [x] Historical records and intentional external tap migration metadata remain intact.

## Comments

### 2026-09-08T12:21:59Z · @ai-agent

Repository regression/simplification round complete: 13 executable issues resolved plus one duplicate intake, seven verified landings, integrated 1100 release tests and two doctests green, warning-denying clippy/rustdoc and local release build green. Fifteen lifecycle/discard tests also pass with PATH containing only explicit Git. Canonical identity inventory is green and now wired into CI. See regression-audit.md for evidence and architectural decisions. Epic remains open because dependent-repository convergence and current live-host evidence are separate completion requirements; installed artifacts and other repositories were not modified.

### 2026-09-08T13:32:09Z · @ai-agent

Completed the user-requested independent per-repository HEAD audit across all 38 locally owned/work repositories. Results: 30 clean, four historical-only, four retain current references (Aggountant and CRMctl open issue guidance, Glasspad maintained comments, retired tap README plus intentional formula migration metadata). Verified moving Homebase/Intakectl HEAD deltas without new matches. Exact commits, actionable paths, scope limits, and canonical 0.7.1 tap/release/installed-version evidence are in local-head-verification.md. Epic remains open on these concrete residual references; prior uncertainty about the broad local repository inventory is now resolved. No downstream source edits or release were performed.

## Resolution

### 2026-09-08T13:51:00Z · @issuectl

Completed repository clean-break gate and the requested 38-repository local HEAD convergence audit. All four concrete current-reference findings were corrected in isolated worktree spinoffs, verified from Git and explicit-merge reports, and pushed to each remote main. Historical provenance and the intentional retired-tap formula migration mapping are preserved. See local-head-verification.md for exact landing commits and scope. This closes source convergence; distribution publication proceeds separately, with no installed-tool changes.
