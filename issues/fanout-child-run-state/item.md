---
created: 2026-10-12
updated: 2026-10-12
type: bug
status: untriaged
priority: normal
labels:
- review:should-fix
- lens:docs-accuracy
- via:review-lens-audit
- ai-review-model:claude-fable-5-1
- ai-review-model:gpt-6-astra
provenance: ai-review
source_ref: taskfleet:01m4r8nnm9a5a5qe1ybjvtfgw1/review-finding:fanout-child-run-state
review_source: ai-review
originating_run: 01m4r8nnm9a5a5qe1ybjvtfgw1
originating_run_kind: spinoff
assessment_classification: CONFIRMED
assessment_outcome: FIX
review_confidence: high
review_severity: medium
review_target: crates/taskfleet/skills/
---

# Fan-out run guide targets driver nodes instead of child runs

## Description

Location: `crates/taskfleet/skills/taskfleet-run-overview/SKILL.template.md:150-152,218-220`. Compare `crates/taskfleet/skills/fan-out/SKILL.template.md:109-125,327-335`.

The run-state guide says a fan-out has no single report and advises `run cancel <driver-id> --node <id>` to cancel one child. In the documented fan-out, the driver is a one-node run with its own aggregate report and each child is a separate one-node run with its own run id. A conductor following the guide when one batch unit needs cancellation targets a nonexistent node in the driver rather than `run cancel <child-run-id>`, and may miss the driver's summary when interpreting the batch.

Verification: `fan-out` explicitly says `--parent-run-id <driver-run-id> --parent-node-id n-0001` records `children[]`, that each child has its own terminal report, and that each child run is cancelled by its own id. The driver report is the batch truth. The single-worker `run show` report projection applies to the driver, which has node `n-0001`.

Assessment: CONFIRMED; likelihood OCCASIONAL (when a fan-out unit needs intervention); readability IMPROVES; architectural impact MINOR; confidence HIGH; FIX (Rule 5), should-fix. Finder: claude-fable-5-1; independent adversarial verifier: gpt-6-astra. Suggested direction: distinguish driver report from individual child-run reports, and cancel a child by its run id.
