---
created: 2026-10-27
updated: 2026-10-27
type: feature
status: done
priority: normal
provenance: chat
source_ref: taskfleet:01m5yjwkbgynhfd72zk1enhvpn/brief
originating_run: 01m5yjwkbgynhfd72zk1enhvpn
originating_run_kind: spinoff
closed: 2026-10-27
---

# run upgrade-lock: operator overrides for live runs and stale creations

## Description

Jari (2026-10-27): "Pitää olla kyky ohittaa jotkut lukot." Taskfleet must be
upgradable on a host even while Taskfleet runs are live.

Observed refusals of `taskfleet run upgrade-lock` (0.17.x):

- Trude: `upgrade_not_quiescent` because one worker run was live.
- Haapa: `upgrade_state_unverifiable: unpublished creation state at
  ~/.taskfleet/.creating/runs` from creation `01m5sx7xmqpmmp5dv2dm4hes7t`
  aborted 2026-10-25 16:10; its creator is long dead.

Add explicit, separately named operator overrides to `run upgrade-lock`:
accept live runs only when their on-disk state schema matches the target
release's, and move aside a stale creation only when its creator is provably
gone. New admissions stay excluded during activation. Every override is
recorded in a receipt and reported in the JSON envelope.
