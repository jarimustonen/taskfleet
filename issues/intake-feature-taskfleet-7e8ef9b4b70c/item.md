---
created: 2026-10-12
updated: 2026-10-15
type: feature
reporter: jari
status: done
priority: normal
provenance: agent:native-agent-host-wrapup
source_ref: agent:native-agent-host-wrapup/reporter:jari/id:nah-wrapup-2026-10-12-parent
lane: control
lane_seq: 30
closed: 2026-10-15
closed_by: agent
---

# Record the parent run of runs a worker spawns

## Description

Record the parent run of runs a worker spawns

Workers in native-agent-host stints spawn their own Homebase helper runs with `taskfleet run create`. Those child runs carry no `parent_run_id` (the parent flags are only used by fan-out drivers), so `run show` cannot say which run started them.

Reconstructing the session hierarchy afterwards required grepping every `~/.taskfleet/runs/*/evidence/n-0001/pi-session.original.jsonl` for the child's run id and taking the earliest match; see native-agent-host `issues/night-stint-2026-10-05/interruptions.md`.

Expected: a run created from inside another run's worktree records the creating run (for example from `run show --current`) as its parent or `created_by`, visible in `run show` and `run list`, without the fan-out driver semantics.

<!-- intakectl:analysis:start job:4834b62d-2ce6-455b-81f8-1f79a8345962 generation:0 -->
## Triage analysis

- **Verdict:** Confirmed feature gap in the inspected checkout; no reproduction is needed because the create-path contract makes the omission explicit.
- **Severity:** Normal. Run ancestry cannot be recovered from run metadata, and reconstructing it requires searching archived Pi transcripts, but this does not prevent runs from executing.
- **Affected area:** `crates/taskfleet/src/run/create.rs`, the run manifest/schema, and the `run show` / `run list` DTOs.
- **Evidence:** `run create` accepts `--parent-run-id` and `--parent-node-id` as a required pair (`create.rs`, around lines 341–392). That pair is the child-spawn/fan-out relationship and participates in parent event handling; it cannot represent arbitrary runs created by a worker. `run show --current` exists for exact current-worktree ownership resolution, but the ordinary create path does not use it to record creator context. The issue's described worker-created helper runs therefore lack an intrinsic creator link, while `run show`/`run list` can only expose recorded manifest data.
- **Fix sketch:** Keep fan-out's `parent_run_id`/`parent_node_id` semantics unchanged. Add distinct optional creator metadata (for example `created_by_run_id`) to newly created run state and its public show/list DTOs; when create runs inside an owned Taskfleet worktree, resolve the exact owner using the existing current-worktree ownership mechanism and persist its full run ID. Do not infer ownership from a branch-name fragment, and do not require a parent node or emit fan-out child-spawn events for this attribution. Cover outside-run creation, valid in-run creation, and ambiguous/stale ownership failing closed or remaining unattributed, according to the intended create contract.
- **Disposition:** Accept as a feature if still open. This is a bounded metadata/visibility improvement; it is not a bug in the existing fan-out parent linkage.

<!-- intakectl:analysis:end job:4834b62d-2ce6-455b-81f8-1f79a8345962 generation:0 -->
