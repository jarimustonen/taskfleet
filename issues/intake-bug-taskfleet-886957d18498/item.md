---
created: 2026-10-20
updated: 2026-10-22
type: bug
reporter: jari
status: in-progress
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:oh-wrapup-20261020-idle-prompt
lane: signals
lane_seq: 10
---

# claude-hook reports idle_prompt (awaiting input) while a Claude worker …

## Description

claude-hook reports idle_prompt (awaiting input) while a Claude worker waits on its own background shells

Observed 2026-10-19 on haapa (taskfleet 0.16.0, Claude Code 2.1.280 worker, profile prototype-opus, run 01m59m76bagfp93gs1mchrev11).

`taskfleet run wait ... --any --output json` settled with outcome condition-met; `run show` showed `awaiting_input_detail` with notification_type `idle_prompt`, escalated, topic "Claude reported idle input; inspect the worker and supply direction". The worker was not waiting for a human: its pane showed it had started background shells ("Still running; I'll continue on its completion notice", "2 shells still running") and it later resumed and finished on its own.

Expected: an idle prompt while the Claude session has running background shells (or is awaiting a background-task completion notice) is not awaiting-input; do not open `node.awaiting_input` / settle waits for it, or mark it as low-confidence, so conductors do not treat it as a stuck worker.

<!-- intakectl:analysis:start job:634fe380-81a0-4652-8943-ff9ad5adbba1 generation:0 -->
## Triage analysis

**Verdict:** confirmed false-positive human-attention signal in the current source. The report describes a worker that continued autonomously, not a worker requiring direction. “Landed” here refers to the filed report; this analysis does not establish that a fix has shipped.

**Severity:** moderate operational impact. Conductors can stop waiting or interrupt useful work unnecessarily. This signal does not itself complete, cancel, or tear down a run, so the observed defect is misleading attention/notification rather than loss of worker work.

**Affected area and cause:** `crates/taskfleet/src/claude_hooks.rs` registers both `permission_prompt` and `idle_prompt` notifications. `parse` maps every matching-session, non-subagent `idle_prompt` to `Signal::Idle`; `ingest` opens `node.awaiting_input` with the directive to “supply direction”. Neither stage distinguishes a session waiting on background work from one requiring a human. A later progress/turn hook can resolve a solely hook-owned idle request, but this does not prevent false escalation during a sufficiently long background wait. `crates/taskfleet/src/run/awaiting_input.rs` supplies the default 180-second grace; `run/wait.rs` includes unresolved requests older than that grace in settlement. This explains why `condition-met` can occur before autonomous work finishes: it means the wait condition, including attention, was met, not that the task completed.

**Reproduction status:** the original run and pane observations are reporter evidence, not independently replayed here. Source inspection confirms the unconditional classification and escalation path. The existing `claude_generated_hooks_detect_and_recover_without_worker_telemetry` fixture in `crates/taskfleet/tests/spawn_all_kinds.rs` explicitly asserts that `idle_prompt` opens attention, but does not exercise a background-shell wait. No live household session was modified and no runtime reproduction or test suite was run for this analysis-only change.

**Fix sketch:** stop treating bare `idle_prompt` as sufficient evidence of a required human decision. Prefer advisory/unknown idle visibility that cannot independently settle ordinary waits or page a conductor; retain permission-prompt and explicit worker-authored human requests. If background-task-aware suppression is chosen instead, first establish a documented native lifecycle signal and its behavior across completion, failure, retry and restart. Do not infer background state from pane text, process activity or elapsed idleness. Update `docs/CLAUDE-PROMPT-SIGNALS.md` alongside the chosen behavior.

**Validation needed for a fix:** a session emitting `idle_prompt` while background work remains outstanding must not trigger human-attention settlement after the grace, and later autonomous progress must remain possible. Preserve permission-prompt escalation, manual/mixed request ownership, and stale-session/attempt fencing. Test both background completion and failure, plus a genuinely idle session, so suppressing this false positive does not silently claim all idle sessions are healthy. Until then, inspect an idle-only notification; `run wait --terminal --check-in 30m` is the documented way to continue bounded observation without treating it as completion.
<!-- intakectl:analysis:end job:634fe380-81a0-4652-8943-ff9ad5adbba1 generation:0 -->
