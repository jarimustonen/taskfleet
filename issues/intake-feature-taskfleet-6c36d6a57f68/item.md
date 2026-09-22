---
created: 2026-09-22
updated: 2026-09-22
type: feature
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:native-agent-host-stint-wait-strategy-20260922
---

# Let stint-start choose aggregate or per-run waiting

<!-- intakectl:analysis:start job:ab247e81-938e-42ff-9893-566cda96db48 generation:0 -->
## Triage analysis

### Current state

The `/stint-start` skill currently prescribes a single aggregate waiting strategy unconditionally. Phase 2 says:

> Launch disjoint units in parallel, then wait. Record each spawn's run id; after a parallel batch, block on `taskfleet run wait <id> …` and confirm each landing

And the standing discipline reinforces:

> Block on `taskfleet run wait <run-id> …` for owned runs to know they have *settled* before you sequence the next unit or enter Phase 3

This always uses `--all` semantics (the default for `taskfleet run wait`) — the conductor blocks until every listed run is terminal before inspecting any result. Fast workers that finish early sit idle while the conductor waits for a straggler, delaying the opportunity to review their output, feed findings into later work, or sequence a dependent wave.

### Available CLI primitives

`taskfleet run wait` already supports both strategies through its `--all` / `--any` flags:

- **`--all`** (default): return once *every* listed run is terminal. Single blocking call for a batch. One `landed` check pass after the call returns.
- **`--any`**: return as soon as *one* listed run is terminal. The caller can inspect that run's result immediately, then re-issue `run wait` for the remaining runs (possibly with `--any` again, or switch to `--all` for the stragglers).

The `--any` flag exists today — the skill simply never uses it.

### Design options

**Option A — Explicit planning choice (recommended).**
Add a deliberate decision point in Phase 1 (planning). For each parallel batch, the conductor classifies the units:

- **Aggregate strategy** (`--all`): choose when the batch has a dependency chain where later units need *all* earlier results before starting, or when review/deploy only makes sense on the complete batch. Default for sequenced hot-file units.
- **Per-run strategy** (`--any` + per-run re-wait): choose when units are fully independent and each one's result can be reviewed, presented, or acted on as soon as it lands. The conductor spawns the batch, then enters a loop: `run wait --any <all-ids>` → inspect the settled run's report → possibly present or sequence → remove that id from the wait set → repeat until empty.

This classification is natural during Phase 1 decomposition and file-collision analysis: units touching different subsystems with no shared review dependency are candidates for per-run waiting.

**Option B — Heuristic auto-selection.**
Derive the strategy from spawn-time properties: number of units, hot-file overlap, known duration spread. Simpler for the skill text but brittle — duration is unknowable at spawn time, and file-collision analysis already happens in Phase 1 so the explicit choice costs nothing extra.

### Integration points

The change touches two places in the skill:

1. **Phase 1 (Plan the round)** — after decomposing into units and resolving file collisions, add a brief classification step: for each parallel batch, state whether aggregate or per-run waiting applies and why. This is prose in the plan announcement, not a new data structure.

2. **Phase 2 (Orchestrate)** — replace the single blanket `run wait <id> …` instruction with two code paths:
   - Aggregate: `taskfleet run wait --all <id> …` then confirm each `landed` flag (the current flow, unchanged).
   - Per-run: spawn batch, then `taskfleet run wait --any <id> …` → inspect report → present/sequence → remove id → loop until no ids remain. Each per-run completion also confirms its `landed` flag.

Phase 3's precondition ("every launched run has settled and its `landed` flag is true") already holds for both strategies: per-run waiting proves each run individually before deploy.

### Risks

- **Notification wake semantics**: per-run waiting uses `--any` which returns as soon as *one* run settles. The harness wake/notification contract is satisfied either way — `run wait --any` is a blocking CLI call, same as `--all`. No polling happens regardless.
- **Skill prose complexity**: adding two code paths makes the skill longer. Mitigate by keeping the aggregate path as the default and documenting per-run as an opt-in classification, not a required choice.
- **Edge case — mixed batch**: if one unit needs aggregate waiting (e.g. a blocker) but others in the same batch don't, split the batch. The classification step handles this naturally.

### Recommendation

Accept the feature. The change is local to `SKILL.template.md` — no CLI changes needed (the `--any` flag already exists), no new data types, no reducer work. The effort is:
- Add the classification step to Phase 1 (~3–5 lines of prose).
- Add the per-run waiting loop to Phase 2 (~10–15 lines of instruction text).
- Keep aggregate waiting as the default path, referenced from the current text.

Estimated scope: one spinoff worktree touching a single file.
<!-- intakectl:analysis:end job:ab247e81-938e-42ff-9893-566cda96db48 generation:0 -->

## Description

Let stint-start choose aggregate or per-run waiting

## Problem

The Taskfleet-bundled `/stint-start` skill currently tells the conductor to launch a disjoint parallel batch and then block on one multi-run `taskfleet run wait <id> …`. That is correct when no result can be used until the whole batch settles, but it also delays inspection of a fast worker when another worker runs much longer.

Observed in the native-agent-host stint: `define-mvp-scope` completed well before `probe-pi-control`, while the one aggregate waiter remained blocked. The completed definition could and should be reviewed immediately; the user had to point out the missed opportunity.

## Expected behavior

Do not prescribe either strategy unconditionally. Teach the conductor to choose deliberately:

- use one aggregate multi-run wait when the whole batch must settle before any review, decision, next wave, or integration action;
- use one independently watched wait per run when individual results can be reviewed, presented, or used as soon as they land;
- preserve the existing requirement that Phase 3 waits for every owned run and verifies each `landed` flag;
- avoid polling and use harness wake/notification semantics for either strategy.

The skill should make this an explicit planning choice based on dependency and review needs, not a blanket "always monitor separately" rule.
