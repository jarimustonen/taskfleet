---
created: 2026-10-01
updated: 2026-10-01
type: feature
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:nah-wrapup-2026-10-01-run-message
---

# Send a message to a running worker

## Description

Send a message to a running worker

While conducting a stint in native-agent-host on 2026-10-01, Jari changed the instructions for a running release spinoff (run 01m3tzfqbpecxaxd7ad99rpjnt): do not switch back on failure, fix forward instead. taskfleet has no verb to deliver a message to a live worker (`taskfleet --help` lists run/event/node/supervise/... but nothing like `run message`). The conductor had to find the worker's pane in the detached `headless` tmux session and use `tmux send-keys` into the Pi TUI, then check by capture-pane that it arrived.

Requested: `taskfleet run message <run-id> [--node n-0001] <text|--body-file>` that delivers text to the worker's harness input (as steering/follow-up), records it as an event in the run log (who, when, text), and reports whether delivery was confirmed. The ad hoc route has no audit trail, depends on tmux pane discovery, and silently fails if the TUI is not in a typing state.
