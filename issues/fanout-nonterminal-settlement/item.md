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
source_ref: taskfleet:01m4r8nnm9a5a5qe1ybjvtfgw1/review-finding:fanout-nonterminal-settlement
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

# Fan-out driver wait loop has no path for nonterminal settled child

## Description

Location: `crates/taskfleet/skills/fan-out/SKILL.template.md:179-191`; compare `crates/taskfleet/skills/taskfleet-run-overview/SKILL.template.md:186-205`.

The driver brief tells an agent to wait on all live child runs with `run wait --any`, inspect status/summary, and start the next unit for freed slots. It only describes `done` and `failed` as settled results. But `run wait --any` also returns a condition-met result for an attention-required or awaiting-input child that remains `pending`/`running`; a stalled supervisor can similarly settle the wait. Leaving that child in the live set and following the same wait instruction again returns immediately, repeatedly, while the batch never reaches the driver's completion condition. The separate advice to cancel a stuck child is not connected to this nonterminal return path in the driver brief.

Verification: `taskfleet-run-overview` explicitly documents nonterminal settled results and their `attention_required` / `awaiting_input` / `stalled` fields. `crates/taskfleet/src/run/wait.rs` treats them as settled and returns from its wait loop before sleeping. Fan-out's self-contained driver brief does not state how to remove or resolve them; its terminal-only slot model does not apply.

Assessment: CONFIRMED; likelihood OCCASIONAL (clean exit without run merge or unattended input prompt in a batch); readability IMPROVES; architectural impact MINOR; confidence HIGH; FIX (Rule 5), should-fix. Finder: claude-fable-5-1; independent adversarial verifier: gpt-6-astra. Suggested direction: distinguish terminal from nonterminal settlements, retain the reservation, resolve/escalate or cancel the child by its run id, then continue without re-waiting on the unchanged settled condition.

## Resolution

### 2026-10-13T05:45:31Z · @fable

Applied in the bundled skill templates by the conductor session after reviewing each recommendation with Jari.
