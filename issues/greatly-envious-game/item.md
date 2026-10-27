---
created: 2026-10-27
updated: 2026-10-27
type: bug
reporter: conductor
status: fixed
priority: normal
provenance: other
provenance_detail: release cut 0.17.2 from a Pi agent on Trude
source_ref: release:0.17.2/creator-session-env
closed: 2026-10-27
closed_by: agent:taskfleet-run-01m5ybxbbwg76p93t39a3k8cb1
commits:
- hash: fb48da8e
  summary: 'test: keep envelope snapshots free of ambient creator attribution'
---

# envelope snapshot tests read the ambient TASKFLEET_CREATOR_SESSION

## Description

Cutting 0.17.2 from inside a Pi agent (TASKFLEET_CREATOR_SESSION set) made the bump hook regenerate run_show_json/jsonl and event_tail_jsonl with created_by_session from the caller's environment, and the cut stopped ('bump hook changed a file outside the version-bearing snapshots'). The snapshot tests must clear TASKFLEET_CREATOR_SESSION (and other ambient attribution) for their child processes so they are hermetic.

## Resolution

### 2026-10-27T10:15:38Z · @agent:taskfleet-run-01m5ybxbbwg76p93t39a3k8cb1

The envelope snapshot command builder now clears TASKFLEET_CREATOR_SESSION (the only creator-attribution variable taskfleet reads; PI_SESSION_ID/PI_SESSION_FILE are not read). Verified: with TASKFLEET_CREATOR_SESSION=pi:<uuid> set, run_show_envelopes and event_tail_envelopes fail without the change and pass with it.
