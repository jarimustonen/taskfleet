---
created: 2026-10-25
updated: 2026-10-25
type: bug
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:oh-conductor-2026-10-25-await-text
---

# run wait does not flag a Claude worker waiting on a plain-text question

## Description

run wait does not flag a Claude worker waiting on a plain-text question

Observed 2026-10-25 (open-habitat conductor, run 01m5rd6959p12f9eq8p8dankmp, profile prototype-opus, Claude harness, interactive headless).

The worker finished a step and ended its turn with a plain-text question to the caller ("Either add 216.128.7.99/32 to the pilot's firewall yourself, or tell me to add it ..."), then sat idle at the prompt for ~30 minutes.

`taskfleet run wait <id> --check-in 45m --output json` and `run show` kept reporting status `pending`, `awaiting_input: false`, `attention_required: false`. Only Claude permission prompts (Notification hook, matcher permission_prompt) set awaiting_input; a Stop hook with the worker idle at an empty prompt and no report/merge is not surfaced.

Expected: an idle Claude worker that ended its turn without `run merge`/`node report` (Stop hook fired, no further tool use) is surfaced as awaiting input / attention after a short grace period, so the conductor's wait settles instead of waiting for the next check-in.

Reported by jari via agent:homebase-wrapup.

<!-- intakectl:analysis:start job:1dd2dae4-9b84-4b44-8a0c-65e852758078 generation:0 -->
## Triage analysis

**Verdict:** The reported visibility gap is supported by the current implementation. A Claude worker can ask a human a question in ordinary text and remain alive without publishing any signal that makes `run wait` settle. This is not evidence of a broken wait timer: the missing fact is the request for human input.

**Severity:** Normal. A conductor can lose tens of minutes before inspecting a worker that needs a decision. Work is preserved; this path does not itself authorize cleanup, change run status, or lose commits.

**Affected area and cause:** In `crates/taskfleet/src/claude_hooks.rs`, `parse` maps only `Notification` / `permission_prompt` to an input-opening signal. `idle_prompt` is ignored, and `Stop` is a progress/turn-boundary signal that can resolve solely hook-owned requests, not open one. A plain-text question therefore creates no `node.awaiting_input` event. `crates/taskfleet/src/run/attention.rs::is_attention_required` requires a recorded clean worker-process exit; ending a conversational turn while Claude remains at its prompt is not that exit. `run/wait.rs` settles on an escalated explicit input request or another supported condition, not on the semantics of the last assistant message. Consequently both reported booleans can remain false while status stays pending.

**Reproduction status:** Source-confirmed, not replayed against an authenticated Claude session. The supplied incident is the runtime observation; its original run and transcript were not inspected. The installed binary reports Taskfleet 0.17.0, commit `29a4cb094f17bb9004ec6a02d1830cd381f65988`; no claim is made that a fresh live reproduction was performed. `docs/CLAUDE-PROMPT-SIGNALS.md` explicitly documents this unknown-signal boundary and the bounded `--check-in` inspection route. With `--check-in 45m`, a 30-minute wait alone is expected and does not demonstrate a missed check-in deadline.

**Fix sketch and constraint:** Do not simply translate every Stop or idle notification into awaiting-human-input. Commit `0957b14` deliberately removed idle-based attention because Claude can be waiting for its own background work. The repository's thin-supervisor policy also rejects inferring human decisions from ambient idleness. The narrow direction is an explicit worker handoff/request signal, emitted before asking a blocking human question and consumed by the existing durable awaiting-input contract. Claude has no live Taskfleet `run message` reply channel, so unattended workers needing an unauthorized decision should instead preserve work and submit a failure report with `discussion_items`, as the generated worker policy specifies. Any automatic turn-end fallback would need a separately justified signal that distinguishes human questions from background waits, rather than a grace timer alone.

**Validation for a future change:** Exercise a plain-text human handoff with a still-live Claude process and no telemetry; assert that the chosen explicit signal opens one durable input generation and ordinary wait settles after its grace. Also retain negative coverage for background-task idle/Stop, duplicate events, stale attempts and foreign sessions, plus clearing only the matching generation and terminal-only wait behavior. No code, tracker disposition, or scheduling fields were changed by this analysis.
<!-- intakectl:analysis:end job:1dd2dae4-9b84-4b44-8a0c-65e852758078 generation:0 -->
