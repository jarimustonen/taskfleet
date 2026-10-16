---
created: 2026-10-16
updated: 2026-10-16
type: bug
reporter: jari
status: untriaged
priority: normal
provenance: agent:open-habitat-wrapup
source_ref: agent:open-habitat-wrapup/reporter:jari/id:oh-taskfleet-terminal-supervisors-01m509jz-01m50a7
---

# Terminal failed supervisors block authorized worktree discard indefinit…

## Description

Terminal failed supervisors block authorized worktree discard indefinitely

Taskfleet v0.13.1 kept supervisors alive for terminal failed spinoff runs for over ten hours, and `taskfleet run discard <id> --dry-run --reason ... --output json` refused cleanup with `error.code: supervisor_live`, even though `run show` and `node show` both reported terminal `failed`, the worker had exited with code 129, and the retained worktree/branch was the only remaining resource. Reproduced on runs `01m509jzxm2p46k23vzm2khzye` (supervisor PID 757688) and `01m50a7xgsm945ne1w9nmhf0m7` (PID 884597), both created 2026-10-15. The two precise `taskfleet supervise <id>` processes had parent 1905, had been alive for 10h44m/10h33m, and node report `worker-exited-nonzero` was recorded 17:36/17:46 UTC. `run cancel` refuses an already-terminal run, so there is no documented CLI path to get past a live terminal supervisor. For this incident, after verifying both PID command lines matched those exact terminal run ids and no worker remained, we sent SIGTERM to the two per-run supervisors; Taskfleet then reported `supervisor.state=not-recorded` and `run discard --dry-run` and `run discard` succeeded, recording audit events. Expected: terminal supervisor exits on its own or offers a fenced CLI cleanup path, so an operator can discard authorized retained work without killing taskfleet's internal process by PID. Please investigate why these supervisors stayed alive after a terminal failure and make cleanup safe/available without raw OS signals. This is a tool lifecycle issue, not a request to loosen `discard`'s live-supervisor guard for running work.
