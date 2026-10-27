---
created: 2026-10-27
updated: 2026-10-27
type: improvement
status: untriaged
priority: normal
provenance: ai-review
source_ref: taskfleet:01m5yjwkbgynhfd72zk1enhvpn/review-finding:sha1:6c6b2bca9c24e562507681ef7011d58a649c9988
review_source: ai-review
originating_run: 01m5yjwkbgynhfd72zk1enhvpn
originating_run_kind: spinoff
review_target: commit e5528feb (run upgrade-lock operator overrides)
assessment_classification: CONFIRMED
assessment_outcome: SPIN_OFF
review_severity: should-fix
review_confidence: HIGH
labels:
- ai-review-model:gpt-6-astra
- ai-review-model:claude-fable-5-1
---

# Same-schema older writers drop newer projection fields during live-run upgrades

## Description

## Problem

`taskfleet run upgrade-lock --allow-live-runs --target-state-schema <N>` lets
old supervisors/workers keep running beside a newer CLI when both use the same
`STATE_SCHEMA_VERSION`. Schema equality is only a release discipline, not a
field-level compatibility proof:

- `crates/taskfleet-core/src/schema.rs`: `Manifest`, `Node`, `MergeTxn` are
  typed serde structs with no `#[serde(flatten)]` extra map, so unknown fields
  are dropped on deserialize.
- `crates/taskfleet-core/src/projections.rs` (`write_node`/`write_manifest`)
  rewrites the typed struct wholesale.
- Commit 7026d09f (released in v0.17.3) added `Node.agent_pid_start_identity`
  and `MergeTxn.driver_pid_start_identity` while `STATE_SCHEMA_VERSION` stayed 1.

## Triggering condition

A run is accepted live across an upgrade where the newer release added an
optional projection field within the same schema. The newer CLI writes the
field (e.g. `run merge` records `pending_merge.driver_pid_start_identity`);
the older supervisor reduces a later event and rewrites the node without it.

## Impact

The run silently falls back to the older behaviour for that field until it
settles (here: seconds-based PID-reuse detection that 0.17.3 retired). A
future field with stronger safety meaning would be lost the same way.

## Direction

Either preserve unknown fields in core projections (an `extra` flatten map on
the projection structs, so older same-schema writers round-trip newer fields),
or introduce an explicit writer-compatibility marker the upgrade gate can
check, plus a documented rule for when a field addition must bump the schema.
Currently documented as a residual in `docs/UPGRADE-ADMISSION.md`.

Found by an AI review of the upgrade-lock operator overrides (finder
gpt-6-astra, verified by claude-fable-5-1 and local code reading).
