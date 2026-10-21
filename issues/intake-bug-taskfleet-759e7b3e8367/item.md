---
created: 2026-10-21
updated: 2026-10-21
type: bug
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:homebase-wrapup-20261020-handoff-cleanup-coupling
---

# stint-handoff skips independent wrap-up when retained-resource cleanup …

## Description

stint-handoff skips independent wrap-up when retained-resource cleanup is blocked

Observed in the installed stint-handoff workflow (Taskfleet 0.16.0) during a Homebase session: one fully harvested, clean retained worktree could not be removed by native discard because of the separately reported @discard-clean-submodule-worktree-stuck. stint-handoff explicitly says that any nonempty preserved_work blocks completion and that the conductor should save the narrative then stop before /wrap-up. Following that instruction skipped the independent session-saving/DAG-check workflow even though the underlying product changes were landed, the active checkout was clean, and the remaining cleanup issue was already recorded.

Jari explicitly corrected this behavior: "Siivoa ja tee wrap up. Ei se wrap up voi olla kiinni ... siivoamattomiin worktreeehn" (clean up and do wrap-up; wrap-up must not be held hostage by unrelated worktree cleanup).

Requested behavior: run the available wrap-up/session-saving workflow independently of retained-resource cleanup, while accurately reporting unresolved owned resources and preserving all native lifecycle/deletion safety. A cleanup blocker may prevent claiming all resources settled, but should not prevent saving lessons, filing tool friction or running the lightweight unlaned-issue check. Do not interpret this as permission to delete unmerged work or control another conductor's runs. This is a workflow-instruction defect, not a request to weaken Taskfleet's run-state or resource identity checks.
