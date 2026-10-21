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

<!-- intakectl:analysis:start job:a6e26c24-31b6-4042-af35-ca6edc10a95c generation:0 -->
## Triage analysis

**Verdict:** Confirmed workflow-instruction defect in the current repository; no product-code fix is made by this analysis. “Already landed” here refers to the filed report, not evidence that the requested behavior has been implemented.

**Severity:** Normal. Retained-resource cleanup can suppress independent session saving, lesson capture, tool-friction reporting, and the lightweight scheduling check. There is no demonstrated data destruction, but context can be lost when the session ends.

**Affected area and cause:** `crates/taskfleet/skills/stint-handoff/SKILL.template.md`, under “The order, and why” (lines 286–294), requires every owned terminal run to have empty `preserved_work`, then explicitly directs an unresolved-ownership handoff to commit its narrative and “stop before /wrap-up”. This couples two different results: settling retained resources and saving session knowledge. A clean, already-harvested retained worktree still takes this branch if native cleanup refuses it. The later instruction to call `/wrap-up` cannot override the earlier explicit stop. `crates/taskfleet/src/skill.rs` includes the generated handoff skill in the bundled catalog, so this is shipped workflow text rather than an incidental planning note.

**Reproduction status:** Confirmed by source inspection; the Homebase execution itself is reporter evidence, not independently replayed here. Minimal instruction-level reproduction: an owned terminal run returns nonempty `preserved_work`; its delivery is verified landed and the active checkout is clean; authorized native cleanup remains blocked. Following the current handoff instructions records the unresolved run and commits the narrative, then exits before invoking the available session-saving skill. No destructive cleanup or synthetic run was needed to establish that control flow.

**Fix sketch:** Separate resource settlement from session preservation. Inventory exact owned run ids and record unresolved resources and the next authorized recovery action, then continue independent session-saving work under its actual permissions. Report cleanup as unresolved rather than claiming all resources settled. Retain native discard/merge identity checks, authorization requirements, protection of unmerged work, and the prohibition on managing foreign runs. Do not broaden deletion authority or treat a terminal status as relinquishment. If individual saving actions are unsafe or unavailable, disclose and skip those actions specifically rather than suppressing the entire workflow. Review the neighboring schedule-verification stop (lines 299–304) for the same coupling while keeping invalid scheduling visible; that is a related instruction branch, not an independently observed incident.

**Suggested validation for a future fix:** Exercise handoff scenarios with (1) clean harvested retained work whose native discard is blocked, (2) genuinely unmerged retained work, (3) no retained resources, and (4) no external `/wrap-up` skill. In each case, permitted session preservation should run or use the documented repository-local fallback; unresolved resources must remain accurately reported and untouched without authorization. Verify foreign runs are never controlled, and review rendered skill/snapshot changes plus the repository's required release gate before merging an implementation. This analysis changes only this appended block; it does not disposition the issue or run implementation gates.
<!-- intakectl:analysis:end job:a6e26c24-31b6-4042-af35-ca6edc10a95c generation:0 -->
