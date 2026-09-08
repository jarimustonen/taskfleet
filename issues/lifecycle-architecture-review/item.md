---
created: 2026-08-12
updated: 2026-08-17
type: epic
owner: jari
status: done
priority: normal
closed: 2026-08-17
---

# Re-examine the run/supervisor/agent lifecycle architecture

## Description

~57% of open issues (and 58% of bugs) cluster in the run/supervisor/agent-lifecycle subsystem. Hypothesis: the supervisor INFERS a distributed process's lifecycle from indirect signals (pid liveness, tmux pane, git branch, node report) — a polling-inference model whose edge cases are combinatorial, so patching never shrinks the list. This epic reviews the current design feature-by-feature, audits unused-complexity drag, surveys protocol/state-machine alternatives, and drives a keep-and-harden vs re-architect decision to an ADR. Evidence: DAG cluster analysis 2026-08-12.

## Issues (final statuses at close)

Phase 1–2 deliverables, all done: `arch-lifecycle-map-rootcause` (analysis.md),
`arch-feature-usage-audit` (feature-audit.md, 717-run evidence),
`arch-supervision-alternatives` (alternatives.md), `arch-redesign-design-session`
(design.md), `arch-decision-rearchitect-vs-harden` (the ADR,
`docs/decisions/0001-thin-supervisor-vs-harden.md`).

Implementation children, all terminal: `thin-exit-status-launcher`,
`merge-transaction-recovery`, `typed-supervisor-outcomes`, `typed-report-origin`,
`retire-via-string`, `rollup-status-log-authoritative`, `interactive-flag`,
`per-node-run`, `attention-required-run-surface`, `raw-git-selfmerge-false-failed`,
`non-merge-teardown-dirty-worktree`, `detached-head-teardown-commit-loss`,
`cut-pipeline-floor-harness-heavy`, `cut-run-kinds-discussion-machinery` (done/fixed);
the review-residue and unreachable-precondition children closed
obsolete/wontfix/duplicate in the stint-2 triage.

Still open at close, deliberately — ordinary follow-ups that outgrew the epic:
`config-show-layered-view` (surface) and `run-merge-stamp` remain scheduled on
their own. `enforce-run-merge` and `shell-quote-dedup` were subsequently closed
`wontfix` after product triage; their `epic:` links stay as provenance.

## Close note (2026-08-17)

Closed **done** on Jari's call. The epic delivered what it set out to decide:
the root-cause analysis (inference vs told state), DECISION-1 (cut/keep/reframe),
DECISION-2 (thin supervisor model) → ADR 0001, and the 0.2.0–0.2.2 releases that
implemented it. The deferred 0.2.1 protocol path (pi.dev self-reporting plugin,
durable operation lease) is intentionally NOT re-filed here — file fresh issues
if/when thin-model field data shows they are needed.
