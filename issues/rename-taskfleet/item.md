---
created: 2026-09-02
updated: 2026-09-08
type: epic
status: open
priority: high
owner: jari
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

## Comments

### 2026-09-08T12:21:59Z · @ai-agent

Repository regression/simplification round complete: 13 executable issues resolved plus one duplicate intake, seven verified landings, integrated 1100 release tests and two doctests green, warning-denying clippy/rustdoc and local release build green. Fifteen lifecycle/discard tests also pass with PATH containing only explicit Git. Canonical identity inventory is green and now wired into CI. See regression-audit.md for evidence and architectural decisions. Epic remains open because dependent-repository convergence and current live-host evidence are separate completion requirements; installed artifacts and other repositories were not modified.
