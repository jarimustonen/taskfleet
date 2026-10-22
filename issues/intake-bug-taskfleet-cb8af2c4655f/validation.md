# Decision-reply guidance validation

This change fixes instructions, not harness transport. Supported autonomous
Claude profiles and message handling remain unchanged.

## Source-grounded executor scenarios

Checked against `run message --help --output json`,
`crates/taskfleet/src/run/message.rs`, `harness/prompt.rs`,
`taskfleet-core/src/reducer.rs::reduce_node_input_resolved`, and
`docs/CLAUDE-PROMPT-SIGNALS.md`:

| Scenario | Expected instruction and production boundary |
| --- | --- |
| Live Taskfleet Pi with acknowledged adapter | `run message` can deliver steering; matching id/session acknowledgement means acceptance, not reading or approval. Existing `stub_ack_requires_matching_identity_and_does_not_echo_body` / `wrong_ack_is_not_receipt` tests exercise the protocol boundary. |
| Headless Claude | Manifest harness check rejects delivery with `message_unsupported` before activity checks. Brief authorizes safe defaults or requires terminal failure plus discussion, retaining work for a decision and respawn. Claude autonomous selection remains supported. |
| Caller-owned Pi | `agent_owner != Taskfleet` rejects delivery; being Pi does not grant the native adapter contract. Same safe default / preserved-work escalation applies. |
| Older Pi without adapter | Missing native session evidence, extension file, or socket rejects delivery as unsupported. A stale socket fails as undeliverable; neither case is an implicit reply. |
| New decision beyond the brief | Report `success: false` with `discussion_items`, do not merge or wait indefinitely. The native Claude fixture opens attention, demonstrates unsupported delivery, clears that exact generation, then reports failure and verifies unapproved work survives without landing. |
| Attention clear | Reducer reads `event_seq`, clears only a matching open request, and delivers no message. It stores no decision or approval. Workers must act on actual authorized decisions or pre-authorized defaults, never the clear itself. |

The reported historical `worker_not_active` response does not establish why the
old Claude attempts failed: current source rejects a Claude manifest earlier
with `message_unsupported`. Historical manifest/attempt evidence was not
inspected. The static harness restriction proves the capability gap without
inventing a historical cause.

## Regression boundary and review

The rendered `skill print` production path is snapshot-tested with an empty
PATH for both workflows. The generated common preamble test pins a concrete
run-bound failure-report command for Pi and Claude, including custom briefs.
The native headless Claude scenario uses a disposable HOME/repository, declared
Git and tmux/workmux stubs with stripped PATH, and a deterministic sleeping
harness stub. It tests the prescribed commands and teardown, not a model's
compliance or a real authenticated Claude session.

Focused source/instruction self-review: the common prompt matches the shipped
skills, optional attention is separate from terminal reporting, a safe default
requires prior authority, message acceptance is not action, and no profile or
transport implementation changed. No model panel was needed for this bounded
instruction-only change. No changes to stint-handoff or taskfleet-run-overview.
