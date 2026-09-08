---
created: 2026-09-08
updated: 2026-09-08
type: improvement
status: untriaged
priority: normal
provenance: other
provenance_detail: transferred from issuectl review finding
source_ref: issuectl:tolerably-wet-summer
---

# Taskfleet bug analysis should emit canonical triage heading

## Description

# Taskfleet bug analysis should emit canonical triage heading

## Description

## Observed occurrence

Taskfleet 0.7.1's bundled `/worktree-bug-analysis` skill permits an analysis worker to append either `## Triage analysis` or `## Suspected Root Cause`. issuectl 0.18.3 intentionally derives `needs_analysis` and its `analysis` projection only from the exact `## Triage analysis` heading.

The issuectl `/issue-intake` caller now handles the released mismatch conservatively by checking the alternative parsed H2 before spawning and by not claiming that Taskfleet guarantees issuectl's canonical heading. The cross-repository contract itself remains divergent.

## Impact

A Taskfleet worker that chooses the alternative heading leaves issuectl's canonical `needs_analysis` signal true. Every consumer must carry compatibility logic or risk repeated analysis. Standardizing the Taskfleet worker contract on `## Triage analysis` would make the producer and consumer agree without broadening issuectl's exact-heading API.

## Scope

Update Taskfleet's bundled `/worktree-bug-analysis` contract and its generated copies/tests so completed issue enrichment uses the exact `## Triage analysis` heading. Coordinate the released contract before removing issuectl's caller-side compatibility handling. Do not change issuectl as part of this follow-up.
