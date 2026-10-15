---
created: 2026-10-13
updated: 2026-10-15
type: bug
status: fixed
priority: normal
lane: docs
lane_seq: 10
closed: 2026-10-15
closed_by: agent
---

# run cancel --node help describes the retired multi-node fan-out model

## Observed

`taskfleet run cancel --help` (installed 0.13.1) describes `--node`:

> Cancel exactly ONE live node (e.g. `n-0002`) instead of the whole run — for unblocking a single stuck fan-out child without killing the batch.

The bundled `fan-out` skill does not run children as nodes of one run. The driver is a single-node run and every unit is its own run created with `--parent-run-id` / `--parent-node-id`; a stuck unit is cancelled with `run cancel <child-run-id>`. The family-level skill review on 2026-10-12 corrected the same claim in `taskfleet-run-overview` (`fanout-child-run-state`, fixed in 0.13.0), but the binary's own help still carries the old model. An agent that trusts `--help` over the skill, which the AI-first CLI canon tells it to do, will try to cancel a child as `--node` of the driver and hit a nonexistent node.

Source: `crates/taskfleet/src/run/mod.rs` around line 266 (the `Cancel` variant's `--node` doc comment), and the `run cancel` command description above it, which has the same phrasing.

## Suggested direction

Describe `--node` as what it is: cancelling one live node of a multi-node run, branch preserved, run stays live until siblings settle. Mention a bundled fan-out only to say that its units are separate runs cancelled by their own id. The help text is in insta snapshots, so the snapshot loop in `crates/taskfleet/AGENTS.md` applies.

## Resolution

Updated `run cancel` and `--node` help to distinguish nodes of one run from bundled fan-out children with their own run ids. Added JSON/text help assertions and reviewed the help snapshots.

## Acceptance Criteria

- [x] Help identifies fan-out children as separate runs and tells callers to cancel by child run id.
- [x] Focused help test and snapshots cover the corrected wording.
