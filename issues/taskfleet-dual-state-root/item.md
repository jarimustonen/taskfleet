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

# Taskfleet selects incidental canonical state over adopted legacy runs

## Description

An active run created under the adopted legacy state root `~/.orchestratectl` became invisible to default Taskfleet commands after the canonical `~/.taskfleet` root acquired only incidental `logs/` and `state/` content. Default `run show` then selected the canonical root and returned `run_owner_not_found` / `run_not_found`; setting `TASKFLEET_HOME=$HOME/.orchestratectl` recovered the exact run.

The same dual-root state now blocks Homebase fleet convergence on Gertrud, Hauis, and Haapa. Homebase correctly refuses to choose or merge the roots. Do not solve this by deleting or moving user state manually.

## Reproduction

- Existing adopted legacy root: `~/.orchestratectl/config.toml` plus active/historical runs.
- Canonical root begins absent or operationally empty.
- A normal/read-only Taskfleet command creates or encounters canonical `logs/` and `state/` entries.
- Subsequent default `taskfleet config path`, `run show --current`, or `run show <id>` resolves the canonical root instead of the legacy root that owns the run.
- Explicit `TASKFLEET_HOME=~/.orchestratectl` resolves the run.

Observed first in run `01m1vmsczrk4fpcdae24vqcw97`; reproduced as a fleet-level dual-root conflict on three hosts.

## Acceptance Criteria

- [x] Root selection remains pinned to the adopted legacy root when it contains the user configuration/run history and the canonical root contains only Taskfleet-created incidental runtime directories or files.
- [x] Read-only commands do not populate a competing root or alter later root selection.
- [x] `config path`, `run show`, `run show --current`, supervisors, and worker terminal commands resolve one consistent root for the process/session contract.
- [x] Truly distinct meaningful state in both roots still fails closed; no state is silently merged, moved, overwritten, or deleted.
- [x] Hermetic tests cover fresh canonical, sole legacy, incidental canonical alongside meaningful legacy, and genuinely conflicting dual-root states.
- [x] Existing `TASKFLEET_HOME` explicit override semantics remain unchanged.
- [x] The full repository green gate passes and production-code changes receive `/llm-review` plus `/assess-findings` before merge.

## Operational follow-through

After a fixed release is published and installed through the normal distribution channel, Homebase must verify all preserved runs/configuration, clean up only artifacts proven incidental, and rerun fleet convergence on Gertrud, Hauis, and Haapa. Repository work itself must not install Taskfleet or mutate the real state roots.

## Agent Runs

### 2026-09-07T05:28:00Z · @ai-agent

Implemented deterministic meaningful/incidental root classification in the single resolver, corrected logging causality, and added hermetic coverage for config path, run show/current, supervisor, worker terminal report, explicit overrides, conflicts, and byte preservation. Required two-round llm-review plus assess-findings completed; one confirmed local classifier refinement was applied. Full green gate passed: fmt, clippy -D warnings, 1076 nextest tests, doctests, and rustdoc -D warnings.

## Resolution

### 2026-09-07T05:28:00Z · @issuectl

Delivered in b088812 after required review, assessment, and complete green gate.
