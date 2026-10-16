---
created: 2026-10-16
updated: 2026-10-16
type: feature
reporter: jari
status: done
priority: normal
provenance: chat
source_ref: taskfleet:01m51nrfvbqwsybz0gdggz6q4b/request:transcript-paths
originating_run: 01m51nrfvbqwsybz0gdggz6q4b
originating_run_kind: spinoff
related: ['@durable-worker-evidence']
lane: open-habitat
lane_seq: 30
collision: [supervise-cleanup, run-public-contract]
closed: 2026-10-16
closed_by: pi
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

- [x] A reader can locate the live native transcript and then its original evidence archive after teardown, with stable node/session identity.
- [x] Missing/unavailable transcripts and unsupported harnesses are explicit; paths confer no continuation or control.
- [x] Live-to-archive transition and post-worktree-removal reads have contract tests.

## Context

Extends @durable-worker-evidence, which already archives transcripts; this ask
is the public read contract, not another archive or retention implementation.
Open Habitat consumer: `issues/manage-agent-worktrees/architecture.md` and
`issues/implement-spinoff-agents/item.md`. @caller-owned-agent-runs is not required.

## Comments

### 2026-10-16T09:02:46Z · @pi

Delivered additive node_transcripts in run show/list, preserving existing evidence fields. Contract: docs/RUN-TRANSCRIPTS.md; deterministic and filesystem evidence: validation.md. Full scripts/validate-local-release.sh passed on clean 18ebdc16e43b5a273c5de03899a66494f11c55c1 (1231 passed, one configured skip); close --stamp changed only its commit message to 1d7c780622c2ec34cdd1e13c63aff61137d367ed. Focused release suites and stripped-PATH transcript/Linux user-manager fixture tests passed. No installed binary/skills, release, or other repository was changed. Current attempt/Pi evidence only; file paths are advisory read-only hints, not control authority. Conductor owns integrated-main gate and release.
