---
created: 2026-10-21
updated: 2026-10-22
type: bug
reporter: jari
status: open
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:oh-wrapup-20261021-run-message
lane: skills
lane_seq: 20
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

<!-- intakectl:analysis:start job:57055533-af6e-41f5-8662-a5cf190350d7 generation:0 -->
## Triage analysis

- **Verdict:** Confirmed capability gap: awaiting-input visibility is harness-neutral, but replying through `run message` is Pi-only. This analysis does not change the issue's disposition or implement a fix.
- **Severity:** Normal. A headless Claude worker that needs a decision not pre-authorized in its brief cannot receive the parent's answer through the supported messaging interface. Work preservation limits data-loss risk, but salvage or respawning costs time and can duplicate work.
- **Affected area:** `crates/taskfleet/src/run/message.rs`, the Claude headless launcher, and the decision-handling guidance in `crates/taskfleet/skills/worktree-spinoff/SKILL.template.md`.
- **Evidence:** Installed Taskfleet reports version 0.16.0; its `run message --help --output json` explicitly limits delivery to live Taskfleet-launched Pi workers with an acknowledged extension adapter. The checked-out `message.rs` enforces that harness restriction before checking running status and requires a native Pi session plus its adapter socket. There is no Claude delivery branch. The spinoff template nevertheless describes opening `node.awaiting_input` for genuine decisions without explaining the non-Pi reply limitation.
- **Signal versus answer:** `node.input_resolved` is not an answer channel. In `crates/taskfleet-core/src/reducer.rs`, `reduce_node_input_resolved` checks the request's `event_seq` and clears `awaiting_input`; it neither stores nor delivers a decision. `docs/CLAUDE-PROMPT-SIGNALS.md` likewise describes Claude hooks as notification/progress signals, not prompt-answer mechanisms. Clearing attention alone must not be presented as approval or delivery.
- **Reproduction status:** The report supplies two observed runs and an unsuccessful delivery attempt. Static inspection and installed CLI help confirm the unsupported delivery path; no new worker was spawned and the historical sessions were not inspected or modified. The reported `worker_not_active` code is not fully explained by the current source: a manifest recorded as Claude would first receive `message_unsupported`. Historical manifest/attempt evidence would be needed to reconcile that detail. It does not invalidate the missing Claude transport, and this analysis does not attribute the eventual worker failures to a specific exit mechanism.
- **Fix sketch:** First make the workflow limitation explicit: non-Pi headless briefs must authorize safe defaults in advance, and an unresolved decision requires preserved-work escalation/respawn rather than an assumed reply channel. If live Claude replies are required, add an explicit harness-supported, attempt-bound delivery contract with acknowledgement and tests; a durable event alone cannot make an already-running harness consume input. Keep notification resolution separate from delivery and from permission approval. Regression coverage should include a live Claude headless decision, explicit unsupported delivery, stale attempts, and proof that clearing attention does not deliver an answer.

Validation was read-only source/help inspection plus an append-only diff check; no runtime delivery or full release gate was run for this analysis-only artifact.
<!-- intakectl:analysis:end job:57055533-af6e-41f5-8662-a5cf190350d7 generation:0 -->
