---
created: 2026-10-21
updated: 2026-10-21
type: feature
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:oh-wrapup-20261021-tf-message
---

# Send a message to a live worker run (run message / node message)

## Description

Send a message to a live worker run (run message / node message)

A conductor sometimes needs to pass the user's new instruction to a worker that is already running (e.g. "keep this a common-sense fix, no testing-of-tests", "do not touch the reclaim timer, another agent owns it"). Taskfleet has no CLI for this. On 2026-10-21 (Open Habitat conductor) the only route was finding the worker's pane in the private socket `/run/user/<uid>/taskfleet/tmux-*.sock` and `tmux -S <sock> send-keys -t <session:win.pane> -l "<msg>"` + Enter; Pi queued it. This depends on harness internals, has no delivery record, and the pane lookup is by cwd guessing.

Request: `taskfleet run message <run-id> [--node n-0001] --text|--from-file` that delivers a steering message to the live worker through the harness's supported input path (Pi queue / Claude steer), records it as a run event (who, when, text hash), and reports delivered / not-live / unsupported-harness honestly. Optional: `--notify-when-read` if the harness can tell.
