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
