---
created: 2026-10-23
updated: 2026-10-23
type: bug
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:oh-2026-10-23-run-message
---

# run message refuses live workers idle at their prompt (worker_not_activ…

## Description

run message refuses live workers idle at their prompt (worker_not_active)

`taskfleet run message <run-id> "<text>"` returns `{"error":{"code":"worker_not_active","message":"node n-0001 is not an active worker"}}` for a live autonomous worker whose agent is idle at its own prompt (turn finished, waiting for input), although the supervisor is alive, the run is `pending`, and the pane accepts input.

Observed 2026-10-23 (taskfleet 0.16.0) four times from an open-habitat/3dbear conductor, with Pi and Claude harness workers, e.g. runs 01m5kagg6pnat4qrbeb95ddt3y, 01m5kcqkpyhk328b386z4d5yn0, 01m5m6ec57rv3m382470ypz2c3, 01m5m61xxkwkws1gbxqtx329tk. Also for a worker actively running a tool (01m5kcqkpyhk328b386z4d5yn0 showed a thinking spinner). Consequence: the conductor had to cancel and respawn workers just to add a clarification, losing progress. The workaround that worked: read TMUX and TMUX_PANE from the environment of `taskfleet run-worker <id>`, then `tmux -S <socket> send-keys -t <pane> -l '<text>'` and `Enter` — the worker read it and continued.

Expected: `run message` delivers to a live worker that is idle at its prompt (the most common case when a worker waits for a decision) and queues or injects for a busy one, with an acknowledgement; a precise error only when the pane/agent is really gone.
