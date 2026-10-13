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
source_ref: taskfleet:01m4r8nnm9a5a5qe1ybjvtfgw1/review-finding:spinoff-dry-run-source-branch
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

# Spinoff dry-run promises to validate missing source branches

## Description

Location: `crates/taskfleet/skills/taskfleet-spawn-spinoff/SKILL.template.md:257-260`; related profile preflight recipe in `crates/taskfleet/skills/taskfleet-overview/SKILL.template.md:134-139`.

The spinoff guide says the complete `run create --dry-run` "validates everything and materializes nothing." A missing `--source-branch` can pass this preflight but fails the real create call (`base_ref_not_found`). An agent relying on the advertised all-clear may begin a run expecting a usable source branch only to discover the error after the real attempt; dry-run does not guarantee materialization success.

Verification: the release-mode 0.12.0 binary was built in the review checkout and exercised with disposable HOME, TASKFLEET_HOME, and Git repository: a nonexistent `--source-branch` passed `run create --dry-run`. In source, `crates/taskfleet/src/run/create.rs` returns the dry-run envelope before source-branch resolution during materialization. The tests validating fenced CLI examples only test parsing with `--help`, not this behaviour.

Assessment: CONFIRMED; likelihood OCCASIONAL (a typo or stale local branch); readability IMPROVES; architectural impact NONE; confidence HIGH; FIX (Rule 5), should-fix. Finder: claude-fable-5-1; independent adversarial verifier: gpt-6-astra. Suggested direction: describe dry-run as an argument/profile/placement preflight, not a guarantee that the named source branch exists.

## Resolution

### 2026-10-13T05:45:31Z · @fable

Applied in the bundled skill templates by the conductor session after reviewing each recommendation with Jari.
