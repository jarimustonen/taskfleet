---
created: 2026-09-29
updated: 2026-09-29
type: bug
reporter: agent
status: open
priority: high
lane: run-state
collision: [crates/taskfleet/src/run/admission.rs]
---

# Upgrade lock timeout leaves activation descendants running

## Description

Observed in Homebase @fleet-shared-cli-latest review of Taskfleet 0.11.5: run upgrade-lock kills only immediate trusted activation child on timeout, then releases its exclusive gate; Homebrew, skill-install or subprocess descendants may continue modifying installed software without gate or rollback. Reproduce with activation process that spawns a child and blocks. Own a bounded process group or equivalent native descendant containment; on timeout terminate group, wait/reap with finite grace, preserve exclusion until no activation descendants can mutate, and fail closed when containment cannot be proved. On success and nonzero exits also define gate ownership and descendant termination semantics. Tests inject nested child, delayed writes and crash/timeout; no real installed tools affected. Exact gate scripts/validate-local-release.sh, no publication from worker branch.
