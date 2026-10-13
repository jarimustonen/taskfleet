---
created: 2026-10-12
updated: 2026-10-13
type: bug
status: fixed
priority: normal
labels:
- review:should-fix
- lens:docs-accuracy
- via:review-lens-audit
- ai-review-model:claude-fable-5-1
- ai-review-model:gpt-6-astra
provenance: ai-review
source_ref: taskfleet:01m4r8nnm9a5a5qe1ybjvtfgw1/review-finding:fanout-driver-merge-guidance
review_source: ai-review
originating_run: 01m4r8nnm9a5a5qe1ybjvtfgw1
originating_run_kind: spinoff
assessment_classification: CONFIRMED
assessment_outcome: FIX
review_confidence: high
review_severity: medium
review_target: crates/taskfleet/skills/
closed: 2026-10-13
closed_by: fable
---

# Merge guide incorrectly says fan-out drivers cannot merge

## Description

Location: `crates/taskfleet/skills/worktree-merge/SKILL.template.md:207-208`; compare `crates/taskfleet/skills/fan-out/SKILL.template.md:100-101,333-335`.

The merge guide explains `no_worktree` / `no_branch` as "a driver node, not a worker; drivers are not merged." But the bundled fan-out driver is a worker with a worktree and branch; the fan-out closing recipe explicitly requires its driver to call `run merge` to land the integration branch. An agent using the general merge guide to close a fan-out can wrongly treat driver identity as an exclusion from merge and leave a completed batch on its integration branch rather than delivering it to the source.

Verification: the `run merge` code checks whether the node has a worktree path and branch, not whether its kind is fan-out. The fan-out instructions explicitly create the driver with a worktree and tell it to merge after child completion. The contradictory gloss is not a true property of fan-out drivers.

Assessment: CONFIRMED; likelihood OCCASIONAL (generic closing guide used for fan-out); readability IMPROVES; architectural impact MINOR; confidence HIGH; FIX (Rule 5), should-fix. Finder: claude-fable-5-1; independent adversarial verifier: gpt-6-astra. Suggested direction: describe the missing recorded worktree/branch condition without claiming driver identity forbids merging.

## Resolution

### 2026-10-13T05:45:31Z · @fable

Applied in the bundled skill templates by the conductor session after reviewing each recommendation with Jari.
