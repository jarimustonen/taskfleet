---
created: 2026-10-25
updated: 2026-10-25
type: bug
reporter: agent
status: open
priority: high
lane: worker-messaging
lane_seq: 1
collision: [crates/taskfleet/src/run/spawn.rs, supervise-cleanup]
---

# terminal runs leak creating message adapters

Observed 2026-10-25 on haapa with taskfleet 0.17.0: after two native intakectl one-shot analysis runs reached `done` (01m5rp8rpwhq270phadp6f0t8q and 01m5rqhyzshrq6k4bm1d4cjq88), their Pi message adapter files `~/.taskfleet/.creating/message-<run>.ts` remained. `run upgrade-lock` then refused activation with `upgrade_state_unverifiable: unpublished creation state at .../.creating/message-<run>.ts`, so the 0.17.1 rollout needed manual removal (copies kept under /var/tmp/agent-keep/native-launcher-01m5rp5q/ and native-launcher-01m5rqg7/). The first of those runs also kept its supervisor alive after `done` until SIGTERM. An empty `.creating/runs/01m5ch0zbz0qn6hbv5cevcqhpj` directory from 2026-10-20 (no such run) blocked the same way.

A read-only look by an intakectl worker points at `supervise/evidence.rs` `remove_message_adapter` running only on successful evidence cleanup, with the no-evidence non-Claude return skipping it (plausible, not confirmed). Expected: terminal runs never leave `.creating` entries behind, and upgrade-lock either recognizes a stale entry whose run is terminal or offers a supported recovery command instead of manual deletion.
