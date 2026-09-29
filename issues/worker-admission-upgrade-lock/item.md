---
created: 2026-09-29
updated: 2026-09-29
type: task
reporter: jari
status: open
priority: high
lane: run-state
collision: [crates/taskfleet/src/run/create.rs, crates/taskfleet-core/src/lock.rs]
---

# Add worker admission interlock for safe upgrades

## Description

Homebase latest-stable Taskfleet promotion cannot use a run-list snapshot: run create in Taskfleet 0.11.1 materializes workers before publication and direct callers do not take a shared global gate. Add a crash-safe cross-process worker-admission protocol: creation holds a shared lock from before materialization through publication (including child and retry paths); an operator-facing exclusive upgrade lock blocks new admissions, waits for quiescence and protects version+skill replacement/verification/rollback. Define finite timeout and machine-readable refusal; preserve live workers and avoid deadlocks, stale leases and changing installed binaries during repo work. Exercise concurrent create versus upgrade and crash/recovery with native subprocess fixtures. Homebase will consume the released contract in @fleet-shared-cli-latest. Gate: scripts/validate-local-release.sh. No persistent install or release cut from a worker worktree.
