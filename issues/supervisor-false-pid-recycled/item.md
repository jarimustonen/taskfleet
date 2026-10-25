---
created: 2026-10-25
updated: 2026-10-25
type: bug
reporter: conductor
status: fixed
priority: high
provenance: other
source_ref: open-habitat-conductor:2026-10-25:msflake-runs
closed: 2026-10-25
closed_by: agent:taskfleet-run-01m5t7sh2k42k77drb89tcsgx0
commits:
- hash: 7026d09f
  summary: decide PID reuse from the kernel start identity
- hash: 330e041c
  summary: refuse to fence a live worker whose identity is unreadable
- hash: 790147eb
  summary: refuse to fence a live worker whose identity is unreadable (amended 330e041c, which never left the worktree)
---

# Supervisor reaps live workers as agent-pid-recycled under heavy CPU load

## Description

Observed 2026-10-25 on Trude (Linux arm64, 8 cores, taskfleet v0.17.1, account jarimustonen): two consecutive Pi workers (`--profile capable`, harness pi) for open-habitat issue `microsoft-catalog-test-flake` were declared dead with `agent-pid-recycled` while they were visibly alive and working (Pi's "Working" spinner was the last thing in agent.log; the window was then killed by supervisor cleanup and the branch preserved dirty). Runs 01m5t4253naprtw5bcjb69mcc5 and 01m5t7dc8s8dvs225yfqnp0w7j (events: `node.death_observed` with empty data, then `node.report` "Agent for node n-0001 stopped responding: agent-pid-recycled"). Both workers were running a deliberate CPU load test (16–24 `bash -c 'while :; do :; done'` hogs; load average 27–47). Claude workers running at the same time survived.

Likely cause (unverified): `watchdog::check_liveness` compares the recorded `start_time` with `pid_start_time(pid)` from sysinfo and returns `Recycled` when they differ by more than 1 s. If sysinfo derives a process's start time from a boot time computed as now − uptime (rather than /proc/stat `btime`), the value jitters under heavy load and a live worker reads as recycled. A false `Recycled` is terminal and tears down a live worker.

Expected: a live worker is never reaped under CPU load. Compare start times in clock ticks from /proc/<pid>/stat (field 22), or with a tolerance that cannot be exceeded by scheduling delay, and confirm recycling by another signal (e.g. the process's comm/cmdline or the tmux pane pid) before terminalizing.

## Resolution

### 2026-10-25T19:47:07Z · @agent:taskfleet-run-01m5t7sh2k42k77drb89tcsgx0

Confirmed cause: not CPU load itself. sysinfo 0.32.1 computes Linux start_time as /proc/stat btime + starttime/HZ, and btime is wall clock minus boot clock, so it moves whenever NTP steps the clock. On Trude timesyncd is pinned at +500 ppm and steps the slow guest clock; the failed runs' own handshakes (ticks plus seconds) show btime at 1792902403 (18:09 UTC) and 1792902401 (19:08), against 1792902399/398 later, so the recorded seconds went 2-4 s stale and the 1 s tolerance read live workers as recycled. Eight hogs for 50 s left btime unchanged; the drift accumulates over a run's life (~2 s/hour here).

Fix: record the kernel's immutable start identity (Linux /proc/<pid>/stat field 22 ticks, macOS pbi_start) on the node (agent_pid_start_identity), as a third token in supervisor.pid, and in MergeTxn (driver_pid_start_identity). watchdog::RecordedStart compares it exactly and uses seconds only for records that predate it. Covers the watchdog, supervisor pid-file exclusivity/reattach/admission, merge-driver recovery, salvage classify+fence, and evidence quiesce; the spawn handshake decides from the identity alone. Salvage now refuses to fence a live pid whose identity is unreadable. Regression tests use drifted seconds with a matching identity (watchdog, pid_file, salvage, merge_recovery, spawn handshake). Full gate scripts/validate-local-release.sh green on the merged commit. Remaining seconds-based consumer: tmux server identity in session.rs (retention only, non-destructive) filed as tmux-server-identity-clock-drift. Runs created by older binaries keep the seconds comparison until they finish.
