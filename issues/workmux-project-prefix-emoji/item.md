---
created: 2026-09-08
updated: 2026-09-08
type: bug
reporter: jari
status: fixed
priority: normal
provenance: chat
lane: worker-materialization
lane_seq: 10
collision: [crates/taskfleet/src/run/spawn.rs]
closed: 2026-09-08
closed_by: codex
---

# Restore the workmux project prefix emoji on Taskfleet worktree windows

## Description

Taskfleet kun käynnistää ynt noit aworktreetä, niin niistä jää puuttumaan tuo prefix hymiö joka ennen poimittiin workmux konffista

## Resolution

### 2026-09-08T10:10:43Z · @codex

Taskfleet now leaves display naming to workmux, records the resulting name alongside stable tmux IDs, and regression coverage verifies both configured naming and source-repository cwd. All required gates passed.
