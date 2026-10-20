---
created: 2026-10-20
updated: 2026-10-20
type: bug
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:oh-wrapup-20261020-idle-prompt
---

# claude-hook reports idle_prompt (awaiting input) while a Claude worker …

## Description

claude-hook reports idle_prompt (awaiting input) while a Claude worker waits on its own background shells

Observed 2026-10-19 on haapa (taskfleet 0.16.0, Claude Code 2.1.280 worker, profile prototype-opus, run 01m59m76bagfp93gs1mchrev11).

`taskfleet run wait ... --any --output json` settled with outcome condition-met; `run show` showed `awaiting_input_detail` with notification_type `idle_prompt`, escalated, topic "Claude reported idle input; inspect the worker and supply direction". The worker was not waiting for a human: its pane showed it had started background shells ("Still running; I'll continue on its completion notice", "2 shells still running") and it later resumed and finished on its own.

Expected: an idle prompt while the Claude session has running background shells (or is awaiting a background-task completion notice) is not awaiting-input; do not open `node.awaiting_input` / settle waits for it, or mark it as low-confidence, so conductors do not treat it as a stuck worker.
