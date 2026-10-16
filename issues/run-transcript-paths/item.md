---
created: 2026-10-16
updated: 2026-10-16
type: feature
reporter: jari
status: untriaged
priority: normal
provenance: chat
source_ref: taskfleet:01m51nrfvbqwsybz0gdggz6q4b/request:transcript-paths
originating_run: 01m51nrfvbqwsybz0gdggz6q4b
originating_run_kind: spinoff
related: ['@durable-worker-evidence']
---

# Expose live and evidence transcript paths for run readers

## Description

## Request

Requested by Jari for Open Habitat on 2026-10-16. The browser needs a read-only
spinoff transcript, but Taskfleet's private Pi `--session` file is outside
ordinary Pi history discovery, and teardown replaces it with archived evidence.

Expose each node's harness, native session ID, live transcript path while it
exists, and evidence transcript path after archival in `run show` and
`run list --json`. Reuse Taskfleet's existing evidence identity and archive;
no host-side session registry or browser control of the worker is needed.
The consumer anchors reads at Taskfleet's state root and verifies session
headers/IDs and the live checkout, rather than trusting a path alone.

## Acceptance Criteria

- [ ] A reader can locate the live native transcript and then its original evidence archive after teardown, with stable node/session identity.
- [ ] Missing/unavailable transcripts and unsupported harnesses are explicit; paths confer no continuation or control.
- [ ] Live-to-archive transition and post-worktree-removal reads have contract tests.

## Context

Extends @durable-worker-evidence, which already archives transcripts; this ask
is the public read contract, not another archive or retention implementation.
Open Habitat consumer: `issues/manage-agent-worktrees/architecture.md` and
`issues/implement-spinoff-agents/item.md`. @caller-owned-agent-runs is not required.
