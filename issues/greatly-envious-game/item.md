---
created: 2026-10-27
updated: 2026-10-27
type: bug
reporter: conductor
status: untriaged
priority: normal
provenance: other
provenance_detail: release cut 0.17.2 from a Pi agent on Trude
source_ref: release:0.17.2/creator-session-env
---

# envelope snapshot tests read the ambient TASKFLEET_CREATOR_SESSION

## Description

Cutting 0.17.2 from inside a Pi agent (TASKFLEET_CREATOR_SESSION set) made the bump hook regenerate run_show_json/jsonl and event_tail_jsonl with created_by_session from the caller's environment, and the cut stopped ('bump hook changed a file outside the version-bearing snapshots'). The snapshot tests must clear TASKFLEET_CREATOR_SESSION (and other ambient attribution) for their child processes so they are hermetic.
