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

The skill should make this an explicit planning choice based on dependency and review needs, not a blanket “always monitor separately” rule.
