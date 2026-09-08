---
created: 2026-09-08
updated: 2026-09-09
type: feature
status: done
priority: high
lane: supervisor
collision: [taskfleet-supervisor]
closed: 2026-09-09
---

# Archive exact Pi session and terminal worker evidence

## Goal

Preserve exact resumable worker evidence before Taskfleet tears down a completed Pi worker. Jari accepted this as part of moving persistent interactive/autonomous work to Haapa on 2026-09-08; Homebase programme haapa-agent-environment owns host integration, while this repository owns worker/session lifecycle.

## Existing behavior

Explicit --tmux-session already selects an inspectable session, stable pane identities are stored, agent.log captures bounded pane output, and events/terminal reports persist. The missing contract is exact native Pi transcript/session identity plus a verified final archive, not another process launcher. Do not reopen completed capture-agent-output-to-run-dir, capture-agent-pane-by-pane-id or taskfleet-headless-spawn.

## Scope

Record Pi session identity explicitly at launch using a supported Pi CLI/interface; never guess newest transcript or import pi-processes internals. Archive native transcript/resume metadata and a final pane snapshot/log with terminal evidence into the canonical run directory before cleanup. Expose evidence paths/status through run show using append-only events and locked projections. Treat archive failures as visible evidence failures, never falsely claim retention. Preserve lifecycle/typed outcome and worktree safety invariants. Source0.8.0 differs from currently installed0.7.1; worktree-local tests must not replace installed tools.

## Acceptance criteria

- [ ] A real isolated Pi run records exact session UUID/file and original cwd before work.
- [ ] Transcript, resume metadata, final pane evidence and terminal report survive window/worktree cleanup.
- [ ] run show exposes evidence completeness and paths; archive failure is durable and visible.
- [ ] Resume opens the intended archived conversation with an explicit usable cwd.
- [ ] Restart/race tests preserve single-writer, event-lock and typed teardown invariants.

## Validation

Required repository gate: scripts/validate-local-release.sh. Also exercise real isolated Pi/tmux lifecycle without touching default tmux or global installed tools. Required /llm-review and /assess-findings. Conductor owns release through the approved pinned held-tag protocol; worker must not publish/install/deploy or edit Homebase. Shared-session visible-window retention is a separate dependent issue.

## Comments

### 2026-09-08T21:50:04Z · @codex

Accepted native evidence implementation c3a89eeb713d59c42e20ccf1d95258e623fb9fe8 and integrated source. Exact clean full release gate passed, actual authenticated Pi/tmux lifecycle proved byte-identical transcript digest, header-only resume cwd adaptation, prior conversation resume, immutable archive and cleanup. Mandated held-tag wrapper then passed exact0.9.0 bump02bd5a18e92b2aac95db274d49688efd16961174 gate and pushed v0.9.0; publication CI is being observed separately. Source release reservation explicitly released. Report /tmp/haapa-evidence-current.json and /tmp/taskfleet-evidence-release-cut.log. No installed binaries or skills changed during source work.
