---
created: 2026-09-08
updated: 2026-09-08
type: feature
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:reitti-cli-wrapup-run-list-repo-filter-20260908
---

# Filter run list by source repository

## Description

## Summary

`taskfleet run list --output json` currently supports filtering by status and kind, but not by source repository, source branch root, or worktree-path prefix.

## Impact

During a reitti-cli stint handoff, the command returned nearly 3,000 lines covering runs from every project on the machine when the caller needed only the one run associated with `/Users/jari/Sources/reitti-cli`. The output hit the coding harness's 50 KB display truncation. Piping the full JSON through jq by `worktree_path` was possible, but requires the caller to receive and parse the entire global run history and know a path-prefix convention.

## Expected

Add a first-class machine-readable filter such as `--source-repo <PATH>` (exact naming to follow Taskfleet's existing surface) that returns only runs whose recorded source repository matches the resolved repository identity. Prefer repository identity over a brittle worktree-path prefix, and document behavior for removed worktrees and relocated clones. The filter should compose with `--status` and `--kind` and preserve the existing envelope.

## Triage analysis

**Classification:** Feature request — already implemented.

**Investigation result:** The requested capability already exists. `taskfleet run list`
has a `--repo <PATH>` flag that filters runs by git common-dir identity. The
flag accepts `.`, paths inside linked worktrees, and absolute paths; runs without
recorded repository identity do not match. The filter composes with `--status` and
`--kind` and preserves the existing JSON envelope.

**Implementation surface:** The feature was delivered in commit `9e44dce` ("Make run
read contracts self-describing"), which:

1. Added `RunSummary::source_repo` to the `run list --json` wire, exposing each
   run's recorded repository path verbatim (null for legacy/skeleton runs).
2. Added `list::Args::repo` and the `repository_identity` resolver in
   `list.rs`, which resolves the selector and each distinct recorded source to
   Git's absolute `--git-common-dir`, caching repeated values. This matches
   linked worktrees (same common-dir) and excludes independent nested
   repositories (different common-dir) without title or path-prefix inference.
3. Updated `SKILL.template.md` to document `--repo` and the `source_repo` field.

**Coverage:** The commit's test surface (not examined in this read-only triage)
covers `.`, subdirectories, linked worktrees, nested repos, sibling repos, and
unrecorded identity.

**Implementation issue:** The dedicated implementation issue
`phenomenally-noisy-behavior` was created from this intake and closed as `done`
in the same commit. The run-id from the reported incident is recoverable from the
reitti-cli stint context but is not needed for this analysis.

**Recommendation:** Close this intake item as `duplicate/implemented`. The
feature is live in the repository HEAD and available in the installed binary
(release containing commit `9e44dce` or later). No further code work is needed.

## Reporter

Jari (via reitti-cli wrap-up)
