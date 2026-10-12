---
created: 2026-10-12
updated: 2026-10-12
type: bug
reporter: claude
status: untriaged
priority: normal
provenance: agent
provenance_detail: native-agent-host wrap-up 2026-10-12
---

# Supervisor stays alive after its run fails, blocking discard

## Description

Run 01m48n4sf63jbd3h59e38f9k63 (taskfleet 0.11.6): the worker exited with status 129 (SIGHUP) and the run was terminalized `failed`. Its `taskfleet supervise` process stayed alive for hours (`run show` reported the supervisor `alive`), and `taskfleet run discard <id> --force` refused with `supervisor_live` on every attempt. The discard succeeded only after the supervisor pid was killed by hand.

Expected: the supervisor exits once the run is terminal and teardown is done, or `run discard` can stop a supervisor that has nothing left to supervise.
