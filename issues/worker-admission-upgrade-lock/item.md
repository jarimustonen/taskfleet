---
created: 2026-09-29
updated: 2026-09-29
type: task
reporter: jari
status: done
priority: high
lane: run-state
collision: [crates/taskfleet/src/run/create.rs, crates/taskfleet-core/src/lock.rs]
closed: 2026-09-29
---

# Add worker admission interlock for safe upgrades

## Description

Homebase latest-stable Taskfleet promotion cannot use a run-list snapshot: run create in Taskfleet 0.11.1 materializes workers before publication and direct callers do not take a shared global gate. Add a crash-safe cross-process worker-admission protocol: creation holds a shared lock from before materialization through publication (including child and retry paths); an operator-facing exclusive upgrade lock blocks new admissions, waits for quiescence and protects version+skill replacement/verification/rollback. Define finite timeout and machine-readable refusal; preserve live workers and avoid deadlocks, stale leases and changing installed binaries during repo work. Exercise concurrent create versus upgrade and crash/recovery with native subprocess fixtures. Homebase will consume the released contract in @fleet-shared-cli-latest. Gate: scripts/validate-local-release.sh. No persistent install or release cut from a worker worktree.

## Comments

### 2026-09-29T05:47:12Z · @agent

2026-09-29: Worker run 01m3nrx25hv44m48xnc23e7env preserved a reviewed admission gate (release-mode nextest 1128 pass), but required scripts/validate-local-release.sh did not complete: cargo-deny was missing. Conductor provisioned disposable cargo-deny 0.20.2 and checksum-verified cargo-dist 0.33.0 under /var/tmp/taskfleet-release-tools; further preflight reveals scripts/shipshape-release.sh rejects installed/published Shipshape 0.12.4 and still allows only 0.12.3 while Homebrew stable is 0.12.4. Release wrapper admission is a separate existing untriaged intake item @surprisingly-full-need; do not bypass the required gate, fake public tap state or merge until policy and gate are reconciled.

## Resolution

### 2026-09-29T07:19:07Z · @issuectl

Admission-aware creation and trusted exclusive upgrade lock implemented; release gate passed (1128/1128 nextest) on 35e134af.
