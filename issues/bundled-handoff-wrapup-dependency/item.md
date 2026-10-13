---
created: 2026-10-12
updated: 2026-10-13
type: bug
status: fixed
priority: normal
labels:
- review:should-fix
- lens:architecture
- via:review-lens-audit
- ai-review-model:claude-fable-5-1
- ai-review-model:gpt-6-astra
provenance: ai-review
source_ref: taskfleet:01m4r8nnm9a5a5qe1ybjvtfgw1/review-finding:bundled-handoff-wrapup-dependency
review_source: ai-review
originating_run: 01m4r8nnm9a5a5qe1ybjvtfgw1
originating_run_kind: spinoff
assessment_classification: CONFIRMED
assessment_outcome: FIX_WITH_CARE
review_confidence: high
review_severity: medium
review_target: crates/taskfleet/skills/
closed: 2026-10-13
closed_by: fable
---

# Bundled stint handoff requires an unavailable wrap-up skill

## Description

Location: `crates/taskfleet/skills/stint-handoff/SKILL.template.md:189-194,237-242`; compare the bundled catalog in `crates/taskfleet/src/skill.rs` and the stint-start handoff route.

The handoff skill defines completion in terms of calling `/wrap-up` to save session learnings and run the remaining issue check. The standard Taskfleet skill installation does not supply `/wrap-up`, and handoff has no alternative instruction when it is absent. A user who installed only the published Taskfleet skills can record and commit the TODO narrative, but cannot finish the prescribed handoff as written. This does not affect installations that already provide `/wrap-up` separately.

Verification: `stint-handoff` says "Then call `/wrap-up` and let it run its own contract"; earlier it describes responsibilities but does not authorize an equivalent fallback. The catalog of fourteen bundled skills has no `wrap-up`. The requirement is a terminal step, unlike optional references to `/complex-rebase`.

Assessment: CONFIRMED; likelihood OCCASIONAL (a clean installation without Jari's other skills); readability IMPROVES; architectural impact MODERATE (handoff and overview contract); confidence HIGH; FIX_WITH_CARE (Rule 4), should-fix. Finder: claude-fable-5-1; independent adversarial verifier: gpt-6-astra. Suggested direction: explicitly state the external prerequisite or give a harness-neutral fallback that records learnings and the light issue check before declaring handoff complete.

## Resolution

### 2026-10-13T05:45:31Z · @fable

Applied in the bundled skill templates by the conductor session after reviewing each recommendation with Jari.
