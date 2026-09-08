---
created: 2026-09-08
updated: 2026-09-08
type: bug
reporter: jari
status: duplicate
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:homebase-wrapup-taskfleet-recovery-status-20260907
duplicate_of: recovery-merge-status
closed: 2026-09-08
closed_by: ai-agent
---

# Recovery merge leaves landed run failed

## Description

## Analysis

### Root cause

The run status is terminalized by the supervisor's watchdog **before** the recovery merge arrives. The watchdog's crash-backstop (`agent-died` false positive) appends a `node.report` with `success: false` and the reducer terminalizes the node to `Failed`. The supervisor then rolls the run up to `Failed` and exits. When `run merge --report-file` later runs, the reducer **adopts** the merge report at `crates/taskfleet-core/src/reducer.rs` line 837 — it sets `node.status = Done`, `landed = true`, `report.success = true` — but deliberately does **not** reconcile the run manifest status. The NOTE at line 837 says:

> *the RUN manifest is intentionally NOT reconciled here (it may stay `Failed` if a supervisor already rolled it up from the watchdog terminal). That is the pre-existing `false-failed-after-merge` symptom … only an ALREADY-rolled-up terminal manifest stays put, because reconciling a settled run status is a distinct change to the run-status terminal guard, deliberately out of scope.*

### Why the reattached supervisor doesn't fix it

`ensure_report_consumer` reattaches a supervisor. But the supervisor's `rollup_status` (`crates/taskfleet/src/supervise/cleanup.rs:121`) explicitly returns `None` when the manifest is already terminal:

```rust
pub fn rollup_status(paths: &RunPaths, children_all_terminal: bool) -> Option<Status> {
    let manifest = read_manifest_opt(paths).ok().flatten()?;
    if manifest.status.is_terminal() {
        return None;  // <-- gates here on the already-terminal Failed status
    }
    // ...
}
```

So the reattached supervisor sees `status: failed` (terminal), skips rollup entirely, runs `cleanup_terminal_nodes` (which tears down the node — worktree, tmux window, branch — because the adopted merge marker makes teardown warranted), fires the notify hook, and exits `work-complete`. The run status is never reconciled to `Done`.

### Fix options

Three approaches, in decreasing invasiveness:

**A — Reducer reconciles the run status when adopting a merge against a terminal manifest** (reverses the "deliberately out of scope" NOTE). After setting `node.status = Done`, if `manifest.status.is_terminal()` and `manifest.status != Status::Done`, the reducer appends a `run.status` event to reconcile the manifest to the status `aggregate_terminal_status` would produce (which, with all nodes `Done`, is `Done`). This is the cleanest fix: the status is correct at append time, no supervisor dependency.

**B — Supervisor rollup re-evaluates status when a terminal-but-warranted run has an adopted merge.** `rollup_status` could be extended: when the manifest is already terminal but `any_node_merged_explicitly` is true, recompute the status from node statuses anyway (the manifest's terminal guard is stale because a merge retroactively succeeded). The reattached supervisor would then append `run.status = done`.

**C — `run merge` itself appends `run.status` after adoption when the manifest is terminal.** After the reducer adoption, if the manifest status is `Failed`, emit a `run.status = done` event before calling `ensure_report_consumer`. This is the simplest but adds a second write domain (the merge command already has the lock path).

### Recommendation

**Approach A** — fix at the reducer level so the invariant "a successful merge never leaves a `failed` run" is maintained at append time, not deferred to a supervisor that may or may not be running.

## Observed

Taskfleet run `01m1xd8tvgqvvf0281btqn04gt` first submitted a required `success:false` node report after `taskfleet run merge` was blocked by an unrelated dirty target worktree. After preserving and committing the unrelated file, the conductor reran:

```sh
taskfleet run merge 01m1xd8tvgqvvf0281btqn04gt --report-file <success-report> --output json
```

The command returned `merged: true`, restarted the supervisor, removed the worktree and branch, persisted a new `success:true` explicit-merge report, and `run show` reports `landed: true` with `landed_method: report-marker`. However `.data.manifest.status` remains `failed`.

## Expected

A supported recovery merge that lands and records a successful terminal report should reconcile the public run status to `done`, or the CLI should reject recovery from a failed terminal state before merging. It must not expose the contradictory durable state `status: failed`, `landed: true`, `report.success: true` after successful cleanup.

## Impact

Status consumers and handoff preflight can classify successfully recovered work as a failed run even though no recoverable worktree, branch, or ownership remains.

## Resolution

### 2026-09-08T11:58:54Z · @ai-agent

Duplicate of @recovery-merge-status: same observed run 01m1xd8tvgqvvf0281btqn04gt, successful explicit merge, and failed run manifest. The canonical issue was already reproduced and its worktree correction is in validation during this regression round. This intake is linked without a second implementation or another status owner.
