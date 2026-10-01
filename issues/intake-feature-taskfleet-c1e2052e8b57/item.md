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

<!-- intakectl:analysis:start job:4463e284-8022-4497-8c8b-3b55391ca7e6 generation:0 -->
## Triage analysis

- **Verdict:** Valid feature request; the described workaround is concrete and was needed to steer a live release worker.
- **Severity:** Normal. This is an operational control and auditability gap, but the report identifies a workaround and no lost work or safety incident.
- **Affected area:** Taskfleet run/supervisor control plane and worker-harness input delivery.
- **Repro status:** The reported occurrence is specific (run `01m3tzfqbpecxaxd7ad99rpjnt`, 2026-10-01); it is not independently reproduced here.
- **Assessment:** A useful `run message` command should not treat finding a tmux pane or sending keystrokes as proof of delivery. It needs a defined worker-input capability with an explicit result (accepted/unsupported/not deliverable), and an append-only event recording sender, time, target node, and message. Avoid recording sensitive message contents in ordinary event logs without an explicit policy; the request's audit requirement should settle this alongside retention and redaction. Delivery semantics should also distinguish harness acceptance from the worker having read or acted on the message.
- **Fix sketch:** First define a narrow harness adapter contract for injecting text into a live worker, including capability detection and confirmation. Then expose it as `taskfleet run message <run-id> [--node ...] <text|--body-file>`, validate run/node state, and append a message event under the run lock. Return structured JSON with delivery status and event identity. Unsupported harnesses or inactive workers should fail clearly rather than falling back to tmux discovery or implying success.
<!-- intakectl:analysis:end job:4463e284-8022-4497-8c8b-3b55391ca7e6 generation:0 -->
