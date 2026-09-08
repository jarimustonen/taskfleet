---
created: 2026-08-21
updated: 2026-09-08
type: bug
status: in-progress
priority: normal
lane: run-read-surfaces
lane_seq: 10
---

# run wait JSON cannot distinguish a timeout from a settled result

## Description

## Observed

`taskfleet run wait` returns a JSON envelope that gives a caller no direct way to tell
whether it settled because every run reached a terminal state, or because `--timeout` (or the
6-hour default) elapsed first.

The envelope carries only:

```json
{
  "schema_version": 1,
  "data": {
    "waited_ms": 21600190,
    "condition": "all",
    "runs": [
      {"run_id": "01m0fathfnk4dexmz93kqnkeag", "status": "pending", "merged": false,
       "landed": false, "landed_method": "git-verified", "stalled": false,
       "attention_required": false, "awaiting_input": false}
    ]
  },
  "warnings": []
}
```

There is no `timed_out` flag, no `condition_met` boolean, and no reason field. `waited_ms` is
~21600000 here, but a caller cannot key on that: it is only recognisable if you already know
the default, and it says nothing when an explicit `--timeout` was passed.

## Impact — observed twice in one session

While orchestrating an ossctl stint I called `run wait` on four runs, then on two. Both times
it returned with runs still `pending`, and both times the envelope read as "everything
settled". The only way to notice is to re-derive the terminal set yourself and check every
element of `runs[]` against it — which defeats the purpose of the folded summary `run wait`
exists to provide.

For an orchestrator this has consequences. The stint workflow's rule is: do not enter the
deploy phase until every launched run has settled. A caller that trusts a timed-out envelope
can proceed to deploy while workers are still running.

The exit code does encode this (0 settled, 2 timeout, 3 under `--fail-on-error`), but that is
lost the moment the command is piped — which is the normal way to consume `--output json`. An
AI-first CLI should not require preserving `$?` through a pipeline to interpret a
machine-readable payload; the JSON should be self-describing.

## Expected

`data` states the outcome explicitly — e.g. `condition_met: false`, or an
`outcome: "settled" | "timed_out"` enum alongside the existing `condition` and `waited_ms`.
Ideally also name which run ids were still non-terminal, so a caller can act without
re-deriving the terminal set.

## Environment

taskfleet 0.4.1 (commit c15d6af4e12e728ce102a933ce17f9f4c2f18dee), macOS.

## Close condition

Close as fixed when a timed-out `run wait --output json` is distinguishable from a settled one
by reading `data` alone, without consulting the process exit code.

## Comments

### 2026-09-08T09:20:05Z · @codex

2026-09-08 regression/simplification stint: implement with phenomenally-noisy-behavior in one run-read-surfaces worktree. Serialize existing Stop decision; expose recorded source_repo rather than infer ownership. Runtime visibility/discard follows after this lands to avoid DTO collisions.

### 2026-09-08T09:38:59Z · @codex

Implementation design: the wait loop's existing Stop decision is now the sole source of data.outcome (condition-met | timed-out); no second settle classifier or derived pending-id vocabulary was added. Existing per-run attention/awaiting-input/stall semantics and exit grading remain unchanged.
