---
created: 2026-10-21
updated: 2026-10-21
type: bug
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:oh-wrapup-20261021-run-message
---

# run message cannot reach headless Claude workers, so awaiting_input can…

## Description

run message cannot reach headless Claude workers, so awaiting_input cannot be answered

Observed 2026-10-20 (taskfleet 0.16.0, open-habitat conductor session on haapa).

Two headless spinoff workers launched with `taskfleet run create --kind spinoff --headless` (Claude Code profile) opened `node.awaiting_input` asking the conductor a yes/no decision (runs 01m5cak1xk4t2n6zpywft87paj, 01m5cak2v9zktdts4c7xaygx61). The conductor tried to answer:

    taskfleet run message 01m5cak1xk4t2n6zpywft87paj "Conductor decision: ..." --output json
    -> {"code":"worker_not_active","message":"node n-0001 is not an active worker"}

The workers were alive (running focused tests). `run message` documents support only for Pi workers with an acknowledged extension adapter; headless Claude workers have no tmux pane either (run show: tmux_window null), so there is no supported channel to deliver the answer. Both workers eventually ended `failed` with their work preserved, and the conductor had to salvage or respawn harvest runs.

Expected: a supported way for the parent to answer `awaiting_input` for any worker kind (e.g. a durable `node.input_resolved` payload carrying the decision that the worker's harness surfaces, or message delivery for Claude headless workers), or a clear statement in worktree-spinoff/stint-start that briefs must pre-decide everything for non-Pi workers.
