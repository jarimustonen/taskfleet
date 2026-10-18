---
created: 2026-10-18
updated: 2026-10-18
type: bug
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:oh-oom-pending-20261018
---

# OOM-killed interactive worker leaves its run pending forever

## Description

OOM-killed interactive worker leaves its run pending forever

Observed 2026-10-17 on haapa (open-habitat, taskfleet 0.16.0), run 01m555vkxb9qe0erb7vv8k0vtx (kind spinoff, profile prototype-opus, --interactive, harness claude).

The kernel OOM killer (user journal: "app.slice: The kernel OOM killer killed some processes in this unit", 15:12 UTC) killed the Claude worker mid-work. Afterwards:
- `taskfleet run show` kept `status: pending`, `recoverable_work: null`, `preserved_work: []`, `report: null` for over an hour; `run wait` never settled.
- The worker's tmux pane (socket under /run/user/1000/taskfleet/) showed only a bare shell prompt in the worktree.
- The worktree held 2 commits plus uncommitted changes.

Expected: the supervisor notices the agent process has exited without a report or merge and settles the run as failed / agent-died with `recoverable_work` (as it does for other deaths), so `run wait` returns and the conductor can retry with a harvest.

Workaround used: commit the WIP in the worktree by hand, `run cancel`, spawn a fresh run that adopts the branch, later `run discard --force`.

Related but different: `supervisor-detects-blocked-prompt` (a live agent stuck at a prompt).

<!-- intakectl:analysis:start job:423862af-a9f4-4a4d-a4c3-24f9b2fbba81 generation:0 -->
## Triage analysis

**Verdict:** The reported non-terminal state is consistent with an explicit interactive-lifecycle policy, not a missed OOM-specific watchdog branch. The operational problem is real: a conductor waiting for an interactive worker to auto-fail can wait indefinitely. Whether that policy should change requires a product decision; this analysis does not disposition the issue.

**Severity:** Normal. Recovery requires operator intervention, but the reported commits and dirty worktree survive. The null recovery fields do not imply that work was lost.

**Affected area and cause:** `crates/taskfleet/src/supervise/mod.rs`, `watchdog_tick`, returns immediately when `manifest.lifecycle.is_interactive()`. This bypasses both recorded failing `worker.exited` handling and the dead-process crash backstop, regardless of whether the pane contains an agent or only a shell. Consequently no synthetic failed report is produced and the run cannot roll up to failed on that evidence alone. ADR 0001, D6, explicitly reserves interactive termination to `run merge` or `run cancel`: a human may intentionally quit and restart the agent. The cost is that an actual OOM death also stays non-terminal.

`crates/taskfleet/src/run/show.rs` only exposes `recoverable_work` from a failed run's report. Its `preserved_work` inventory calls `run/retained.rs::observe_terminal`, which requires terminal run and node statuses. Thus pending + null report + null recovery block + empty preserved inventory is an expected combination even when the working tree contains valuable changes.

**Reproduction status:** Confirmed by source inspection at the v0.16.0-based checkout, not by inducing another OOM or inspecting the original run's journal/state. Existing regression tests `interactive_run_ignores_dead_pid` and `interactive_run_ignores_worker_exit_failure` explicitly assert no terminal status or synthesized report for these inputs; they were inspected, not executed in this analysis-only worker. The original OOM attribution remains reporter-supplied. A deterministic regression scenario needs no memory exhaustion: an interactive run with a confirmed-dead worker, or a recorded failing exit, reaches the same early return. An unbounded wait cannot depend on automatic worker-death settlement in this mode; bounded waits/check-ins avoid an indefinite caller block.

**Fix sketch:** Do not merely remove the interactive early return: that would also terminalize intentional quit/restart sessions and reverse the existing contract. If automatic failure is wanted for unattended work, use autonomous lifecycle rather than `--interactive`. If interactive workers must additionally auto-fail on crashes, first define how an authoritative crash is distinguished from an intentional stop and how restart remains safe; then change the lifecycle policy and its paired regression tests, retaining branch/worktree preservation. Separately, exposing non-terminal stopped-worker attention and worktree guidance could improve recovery without granting automatic teardown authority. Until a policy change, the reported commit-WIP / explicit cancel / adopt workflow is consistent with manual lifecycle ownership; forced discard should remain a separate deliberate action after recovery is verified.
<!-- intakectl:analysis:end job:423862af-a9f4-4a4d-a4c3-24f9b2fbba81 generation:0 -->
