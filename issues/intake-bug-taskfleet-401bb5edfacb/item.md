---
created: 2026-10-23
updated: 2026-10-24
type: bug
reporter: jari
status: open
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:oh-2026-10-23-run-message
lane: messaging
---

# run message refuses live workers idle at their prompt (worker_not_activ…

## Description

run message refuses live workers idle at their prompt (worker_not_active)

`taskfleet run message <run-id> "<text>"` returns `{"error":{"code":"worker_not_active","message":"node n-0001 is not an active worker"}}` for a live autonomous worker whose agent is idle at its own prompt (turn finished, waiting for input), although the supervisor is alive, the run is `pending`, and the pane accepts input.

Observed 2026-10-23 (taskfleet 0.16.0) four times from an open-habitat/3dbear conductor, with Pi and Claude harness workers, e.g. runs 01m5kagg6pnat4qrbeb95ddt3y, 01m5kcqkpyhk328b386z4d5yn0, 01m5m6ec57rv3m382470ypz2c3, 01m5m61xxkwkws1gbxqtx329tk. Also for a worker actively running a tool (01m5kcqkpyhk328b386z4d5yn0 showed a thinking spinner). Consequence: the conductor had to cancel and respawn workers just to add a clarification, losing progress. The workaround that worked: read TMUX and TMUX_PANE from the environment of `taskfleet run-worker <id>`, then `tmux -S <socket> send-keys -t <pane> -l '<text>'` and `Enter` — the worker read it and continued.

Expected: `run message` delivers to a live worker that is idle at its prompt (the most common case when a worker waits for a decision) and queues or injects for a busy one, with an acknowledgement; a precise error only when the pane/agent is really gone.

<!-- intakectl:analysis:start job:d2d0c2f5-6288-4d21-a559-0125943c1c25 generation:0 -->
## Triage analysis

**Verdict:** confirmed eligibility mismatch for live Pending Pi workers in this checkout; not an idle-prompt detection failure. **Severity:** normal: conductor steering is blocked, potentially forcing cancellation and respawn of productive workers. No fix or disposition is made here.

**Affected area and cause:** `crates/taskfleet/src/run/message.rs` rejects a node unless `node_data.status == Status::Running`, in addition to rejecting terminal runs and recorded worker exits. However, `crates/taskfleet-core/src/reducer.rs::reduce_node_created` initializes native worker nodes as `Pending`, with their process/session evidence and `started_at` already populated. A live spawned worker can therefore fail this gate before the adapter is contacted, whether waiting at a prompt or actively executing tools. The refusal does not inspect prompt activity, PID liveness, or pane input readiness.

**Delivery boundary:** the supported path is Taskfleet-owned Pi with its attempt-specific acknowledged socket adapter. `message-extension.ts` calls `pi.sendUserMessage(request.message, { deliverAs: "steer" })`; acceptance is explicitly not proof of reading or acting. Claude is currently rejected earlier as `message_unsupported`, so the reported Claude `worker_not_active` observations cannot be explained by this exact implementation without the original version/state evidence. Raw tmux input accepting text is not an equivalent delivery acknowledgement and should not become the eligibility test.

**Reproduction status:** source-confirmed rejection for a nonterminal Taskfleet Pi run whose node is Pending and has no recorded exit. No live workers were messaged or modified, and tests were not run during this analysis. Independent repository evidence is recorded in `issues/intake-feature-taskfleet-b8748b48ddd0/item.md`: two installed-0.16.0 Pi workers rejected messages while Pending, then successfully completed and merged. This supports the same diagnosis without claiming a new runtime reproduction. The supplied brief's description of the issue as already confirmed and landed is not evidence of a landed code fix: the restrictive check remains in the checked-out source.

**Fix sketch:** align message eligibility with the native worker lifecycle so eligible Pending workers can reach the existing adapter, while retaining terminal/exited, ownership, harness, session/attempt and adapter checks. Do not synthesize Running from spinner, tmux or idle signals. Add regression coverage for Pending and Running Pi workers receiving matching acknowledgements, terminal/exited refusals, unsupported harnesses and missing/stale adapters, plus an end-to-end native-launch case before any status update. Preserve the distinction between adapter acceptance and a read receipt.
<!-- intakectl:analysis:end job:d2d0c2f5-6288-4d21-a559-0125943c1c25 generation:0 -->

## Decisions

### 2026-10-24T18:44:14Z · @jari

Jari 2026-10-24: live workers must be able to receive run messages. Accepted for immediate fix.
