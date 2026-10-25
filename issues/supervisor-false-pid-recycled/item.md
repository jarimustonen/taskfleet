---
created: 2026-10-25
updated: 2026-10-25
type: bug
reporter: conductor
status: untriaged
priority: normal
provenance: other
source_ref: open-habitat-conductor:2026-10-25:msflake-runs
---

# Supervisor reaps live workers as agent-pid-recycled under heavy CPU load

## Description

Observed 2026-10-25 on Trude (Linux arm64, 8 cores, taskfleet v0.17.1, account jarimustonen): two consecutive Pi workers (`--profile capable`, harness pi) for open-habitat issue `microsoft-catalog-test-flake` were declared dead with `agent-pid-recycled` while they were visibly alive and working (Pi's "Working" spinner was the last thing in agent.log; the window was then killed by supervisor cleanup and the branch preserved dirty). Runs 01m5t4253naprtw5bcjb69mcc5 and 01m5t7dc8s8dvs225yfqnp0w7j (events: `node.death_observed` with empty data, then `node.report` "Agent for node n-0001 stopped responding: agent-pid-recycled"). Both workers were running a deliberate CPU load test (16–24 `bash -c 'while :; do :; done'` hogs; load average 27–47). Claude workers running at the same time survived.

Likely cause (unverified): `watchdog::check_liveness` compares the recorded `start_time` with `pid_start_time(pid)` from sysinfo and returns `Recycled` when they differ by more than 1 s. If sysinfo derives a process's start time from a boot time computed as now − uptime (rather than /proc/stat `btime`), the value jitters under heavy load and a live worker reads as recycled. A false `Recycled` is terminal and tears down a live worker.

Expected: a live worker is never reaped under CPU load. Compare start times in clock ticks from /proc/<pid>/stat (field 22), or with a tolerance that cannot be exceeded by scheduling delay, and confirm recycling by another signal (e.g. the process's comm/cmdline or the tmux pane pid) before terminalizing.
