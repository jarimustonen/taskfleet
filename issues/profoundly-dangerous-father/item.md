---
created: 2026-10-27
updated: 2026-10-27
type: bug
reporter: conductor
status: untriaged
priority: normal
provenance: other
provenance_detail: open-habitat conductor, Trude 2026-10-27
source_ref: open-habitat:2026-10-27/01m5y18wr663zmxacfkwedv8a3
---

# run wait reports 'supervisor died mid-run' (stalled) while the supervisor and worker are alive

## Description

taskfleet v0.17.1 on Trude. run 01m5y18wr663zmxacfkwedv8a3 (open-habitat, spinoff, prototype-opus Claude, --interactive --headless): `run wait --check-in 45m` settled immediately (waited_ms 0) with stalled: true and error 'supervisor died mid-run; work is stranded and cannot be rolled up', status pending. At the same moment `taskfleet supervise 01m5y18wr663zmxacfkwedv8a3` (pid 711717, age ~24 min) and the worker's claude (pid 711579, cwd the run's worktree) were both alive and the worktree had fresh edits. Every later plain wait returns at once, so a conductor cannot wait on the run without --terminal. Expected: no stall while the recorded supervisor is alive; if liveness is uncertain, say unknown rather than died.

## Comments

### 2026-10-27T10:15:46Z · @agent:taskfleet-run-01m5ybxbbwg76p93t39a3k8cb1

Checked against tmux-format-tab-separator-ubuntu: not the cause. run wait's supervisor liveness (SupervisorView::probe → pid_live_with_identity) is a /proc pid + start-identity probe with no tmux involvement. The run's events.jsonl shows the same misjudgment at merge time: seq 7 `supervisor.exited` reason=stale-on-reattach (key stale-on-reattach:711717:1793083077) for pid 711717 while it was alive; it later exited work-complete itself (seq 15). So the start-identity comparison read a live supervisor as dead, the family of supervisor-false-pid-recycled / tmux-server-identity-clock-drift. Next step: check whether the run's supervisor pid record was the legacy seconds-only form (1 s tolerance, see RecordedStart::matches) under v0.17.1.
