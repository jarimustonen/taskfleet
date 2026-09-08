---
created: 2026-08-15
updated: 2026-09-08
type: improvement
status: wontfix
priority: normal
epic: lifecycle-architecture-review
closed: 2026-09-08
---

# Dedupe shell_single_quote across run resume hints

## Description

`shell_single_quote` (single-quote shell escaping for copy-paste-safe run ids in resume hints) is duplicated verbatim in `crates/taskfleet-cli/src/run/false_failed.rs` and `crates/taskfleet-cli/src/run/attention.rs`. The two copies can drift.

Surfaced by llm-review (anthropic #10) during the `raw-git-selfmerge-false-failed` review.

**Scope:** extract the helper to one shared location (e.g. `crate::run::shell_quote` or `crate::output`), update both call sites, keep the existing hostile-id unit tests. Trivial, low-risk cleanup; kept out of the lifecycle fix to keep that change narrow.

## Resolution

### 2026-09-08T08:40:25Z · @issuectl

The tiny duplicated helper is stable and locally tested; extracting it would add churn without meaningful product or correctness value.
