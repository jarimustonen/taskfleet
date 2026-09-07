---
created: 2026-09-01
updated: 2026-09-07
type: improvement
reporter: jari
status: open
priority: normal
labels: [skills, review-workflow]
provenance: chat
source_ref: chat:2026-09-01/stint-review-scope-discretion
lane: workflow-skills
lane_seq: 30
---

# Let workers choose proportionate review depth

## Description


`stint-start` currently requires the conductor to add `/llm-review` plus `/assess-findings` to every spinoff that touches production code:

> When a unit touches **production code**, tell the spinoff in its task to **run `/llm-review` (+ `/assess-findings`) before merging**.

This removes the implementing agent's ability to choose a review level proportionate to its actual diff. The problem was observed during a production incident hotfix whose runtime change was one SQL-fragment separator, a small helper, a plugin version bump, and focused tests. The implementing agent explicitly said that, absent the forced instruction, a targeted code review plus staging smoke would have been sufficient. Nevertheless, the generated brief required a full multi-model review and assessment, and a later harvest brief repeated it even though the production diff had already been reviewed.

The current rule therefore adds substantial latency and model cost to small, well-understood changes. It also encourages conductors to decide review scope before the worker has seen the final implementation, while the worker is the actor best positioned to assess the completed diff's complexity and risk.

## Reproduction

1. Invoke `/stint-start` for a small fix that touches production code.
2. Follow Phase 2's current review instruction when composing the spinoff brief.
3. Observe that the brief must require `/llm-review` and `/assess-findings`, regardless of the eventual diff size, test evidence, prior review evidence, or the worker's own risk assessment.
4. Ask the worker after implementation whether that process would have been justified without the mandate. For the motivating SQL hotfix, the answer was no.

## Recommended change

Make the worker responsible for selecting and explaining the review depth after it has implemented and tested the change. `stint-start` should encourage review, but should not prescribe a full multi-model workflow solely because a diff touches production code.

The brief should ask for a proportionate, evidence-based decision and define cases where stronger review is expected. Examples include security or privacy boundaries, authorization, destructive migrations, concurrency, broad refactors, unfamiliar architecture, difficult rollback, and changes whose tests cannot adequately exercise the risk.

The conductor may still mandate a specific review when the user, issue, repository policy, or higher-level workflow explicitly requires it. Existing review evidence should be reused unless the new diff materially changes the reviewed risk surface.

## Suggested prompt example

```text
After implementation and tests, assess the final diff's risk and complexity and choose a proportionate review level. You may use a focused self-review or targeted reviewer for a small, local, well-covered change. Use `/llm-review` plus `/assess-findings` when the diff is broad, security/privacy-sensitive, destructive, concurrency-sensitive, architecturally significant, difficult to roll back, or weakly covered by tests. Record the chosen level and rationale in the terminal report. If the issue, user, or repository explicitly mandates a review workflow, follow that requirement. Reuse prior review evidence unless this run materially changes the reviewed risk surface.
```

The retry-with-harvest section should follow the same rule. A harvest worker must inspect and validate stranded commits, but it should not be forced to repeat a complete multi-model review when the commits already have adequate review evidence and the harvest adds only a small, independently testable change.

## Acceptance Criteria

- [ ] Replace the unconditional production-code multi-model review mandate in `stint-start` with worker-selected, risk-proportionate review guidance.
- [ ] Include a concrete prompt example that gives the implementing worker authority to choose and justify review depth.
- [ ] Preserve explicit review mandates from the user, issue, repository policy, or calling workflow.
- [ ] Update retry-with-harvest guidance to reuse adequate prior review evidence and repeat review only when the risk surface materially changes.
- [ ] Add or update bundled-skill snapshots/tests covering the new wording.
- [ ] Verify the generated installed `stint-start` skill contains the updated guidance.

## Decisions

### 2026-09-07T18:15:48Z · @jari

Accepted. Replace the unconditional full multi-model review mandate with worker-selected, evidence-based review depth after implementation. Small local well-covered changes may use focused review; security/privacy, destructive, concurrency-sensitive, architectural, broad, hard-to-rollback, or weakly tested changes retain stronger review. Explicit user, issue, repository, or caller mandates still win, and adequate existing evidence should not be repeated unless the risk surface changes.
