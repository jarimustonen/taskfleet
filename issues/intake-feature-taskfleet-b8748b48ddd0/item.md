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

<!-- intakectl:analysis:start job:6ff0e458-1812-44fa-8f57-e910e15135db generation:0 -->
## Triage analysis

The core request is already implemented in this checkout and exposed by the installed binary. `taskfleet run message --help` advertises `taskfleet run message <RUN_ID> [TEXT] [--node <NODE>] [--body-file <PATH>]`; the default node is `n-0001`. Text is positional, not `--text`, and file input is `--body-file`, not `--from-file`.

Implementation evidence: commit `762e491` introduced the acknowledged Pi adapter; `f61158b` and `f08f042` refined launch, receipt, and cleanup handling. `crates/taskfleet/src/run/message.rs` validates nonblank UTF-8 input up to 32 KiB, checks run/node eligibility under the run lock, and records `worker.message.attempted` and `worker.message.result`. The attempted event includes sender, UID, byte length, and worker attempt; event timestamps provide timing. Neither message text nor a text hash is recorded.

Delivery uses the launcher's attempt-specific Unix socket and bundled `crates/taskfleet/src/run/message-extension.ts`, which invokes `pi.sendUserMessage(..., { deliverAs: "steer" })`. The acknowledgement must match message and session identities. Success means adapter acceptance only: `read_or_acted` remains `unconfirmed`. Connection failure is distinguished from an unknown outcome after communication starts; no tmux pane guessing or send-keys is used.

Scope limits are explicit: only Taskfleet-launched Pi workers with the adapter are supported. Claude, caller-owned sessions, and older workers without the adapter are rejected as unsupported; terminal/exited workers are rejected as inactive. There is no read notification. Thus the practical Pi steering need is covered, while Claude delivery, the requested text hash, and optional read receipts are not implemented.

Verification was read-only: installed CLI help, implementation, and commit history were inspected. Existing unit tests cover matching acknowledgements, wrong identities, and stale sockets; they were not rerun, and no live worker was messaged. No issue status or scheduling decision is changed by this analysis.
<!-- intakectl:analysis:end job:6ff0e458-1812-44fa-8f57-e910e15135db generation:0 -->
