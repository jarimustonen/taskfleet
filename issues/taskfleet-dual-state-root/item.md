---
created: 2026-09-07
updated: 2026-09-07
type: bug
status: fixed
priority: high
lane: unlaned
commits:
- hash: b088812
  summary: retain meaningful adopted state roots
closed: 2026-09-07
---

# Taskfleet selected incidental default state over runs in a nondefault root

## Description

During the staged rename, an active run in a preserved nondefault state root became invisible to default Taskfleet commands after `~/.taskfleet` acquired only incidental `logs/` and `state/` content. Default `run show` selected the default root and returned `run_owner_not_found` / `run_not_found`; setting `TASKFLEET_HOME` explicitly to the preserved root recovered the exact run.

The same two-root state blocked Homebase fleet convergence on Gertrud, Hauis, and Haapa. Homebase correctly refused to choose or merge the roots. Repository work did not delete or move user state.

## Reproduction

- A nondefault root contained user configuration plus active and historical runs.
- The default root began absent or operationally empty.
- A normal/read-only Taskfleet command created or encountered default-root `logs/` and `state/` entries.
- Subsequent default `taskfleet config path`, `run show --current`, or `run show <id>` resolved the default root instead of the root that owned the run.
- Explicit `TASKFLEET_HOME=<preserved-root>` resolved the run.

Observed first in run `01m1vmsczrk4fpcdae24vqcw97`; reproduced as a fleet-level two-root conflict on three hosts.

## Historical acceptance criteria

The fix recorded by this closed issue implemented implicit nondefault-root adoption and classification. That behavior was later rejected by ADR 0002's clean-break identity: current maintained code always defaults to `~/.taskfleet`, while any preserved nondefault location remains reachable only through explicit `TASKFLEET_HOME`. The still-valid safety requirements are:

- [x] Read-only commands do not populate the selected root merely for diagnostics.
- [x] `config path`, `run show`, `run show --current`, supervisors, and worker terminal commands resolve one process-frozen root.
- [x] No state is silently merged, moved, overwritten, or deleted.
- [x] Explicit `TASKFLEET_HOME` selection remains available for a preserved nondefault root.
- [x] The full repository green gate and proportionate independent review pass.

## Operational follow-through

Preserved data at a nondefault location requires an explicit `TASKFLEET_HOME` selection. Any machine-level verification or cleanup remains outside repository implementation and must independently prove what it touches. This historical issue does not claim that user data was migrated or deleted.

## Agent Runs

### 2026-09-07T05:28:00Z · @ai-agent

At the time, implemented deterministic meaningful/incidental root classification in the single resolver, corrected logging causality, and added hermetic coverage for config path, run show/current, supervisor, worker terminal report, explicit overrides, conflicts, and byte preservation. Required two-round llm-review plus assess-findings completed; one confirmed local classifier refinement was applied. Full green gate passed: fmt, clippy -D warnings, 1076 nextest tests, doctests, and rustdoc -D warnings. ADR 0002 later rejected the classifier behavior; the historical result remains recorded here without presenting it as the current contract.

## Resolution

### 2026-09-07T05:28:00Z · @issuectl

Delivered in b088812 after required review, assessment, and complete green gate.
