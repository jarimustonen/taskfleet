---
created: 2026-08-24
updated: 2026-09-08
type: task
status: done
priority: normal
provenance: other
provenance_detail: Observed missing external-package dependency during taskfleet worker control-plane E2E rollout
source_ref: taskfleet:01m0sxn5pfrtybkgyqrkvzspsm/follow-up:external-pi-worker-telemetry-adapter
originating_run: 01m0sxn5pfrtybkgyqrkvzspsm
originating_run_kind: spinoff
closed: 2026-09-08
---

# Deliver the external pi worker-telemetry adapter package

## Description

## Problem

The separately owned production pi.dev worker-telemetry adapter package does not yet exist in the available source tree or installed pi package set. The taskfleet repository publishes `contracts/worker-telemetry-v1/` and validates it with a harness-neutral fake driver, but that does not execute real pi lifecycle hooks, package installation, coalescing timers, or shutdown callbacks.

## Required delivery

Create and publish the external pi package that consumes `contracts/worker-telemetry-v1/` and uses only documented public pi hooks. Run the contract's `adapter_sequences` in the package with a virtual monotonic clock and fake sender, including event storms, long tools, endpoint failures, refresh, and bounded shutdown. Validate an ordinary package install only in an isolated temporary home/package root, never against user-global pi or taskfleet state.

## Completion evidence

- Package source and release identity are named.
- All adapter-owned conformance sequences pass in the package.
- An isolated installation launches configured autonomous pi and emits accepted samples through the public endpoint.
- Failure disclosure and rollback are demonstrated without changing global pi settings or installed taskfleet.

## Resolution

### 2026-09-08T08:40:25Z · @issuectl

The external Pi telemetry adapter was delivered in private repository jarimustonen/taskfleet-pi-telemetry, passed its conformance and package gates, and was fleet-pinned and load-tested on reachable machines.
