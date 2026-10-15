---
created: 2026-10-12
updated: 2026-10-15
type: bug
reporter: claude
status: fixed
priority: normal
provenance: agent
provenance_detail: native-agent-host wrap-up 2026-10-12
lane: control
lane_seq: 10
closed: 2026-10-15
closed_by: agent
---

# run wait returns immediately while a run awaits input

## Description

`taskfleet run wait <id>` on a spinoff whose node has an open `node.awaiting_input` settles immediately with `status: pending` once the grace window has passed, and keeps doing so on every later call until the worker resolves the signal. A caller that wants the terminal outcome has to loop with sleeps around `run wait`, which the skills tell it not to do.

Observed with run 01m46ejsy4eyx4krx1hy2fqsqe (taskfleet 0.11.6) on 2026-10-05.

Expected: a way to wait for terminal status only (for example `run wait --terminal`), or `run wait` reporting awaiting-input once and then blocking until the signal is resolved or the run terminates.

## Comments

### 2026-10-15T17:38:52Z · @agent

Resolution: Added run wait --terminal to ignore non-terminal attention for the wait predicate while retaining default decision wake. Covered resolution followed by terminalization, timeout, --all/--any, and status versus landed in integration tests.
