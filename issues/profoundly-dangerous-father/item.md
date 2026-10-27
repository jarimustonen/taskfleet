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
