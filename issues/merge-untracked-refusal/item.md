---
created: 2026-10-13
updated: 2026-10-15
type: bug
reporter: claude
status: in-progress
priority: normal
lane: merge
lane_seq: 10
---

# run merge refuses on unrelated untracked files in the target worktree

`taskfleet run merge` refuses with `merge_failed` when the target branch's
worktree has untracked files, even when they do not overlap the merged
changes.

Seen 2026-10-13 in `~/Sources/native-agent-host`: a spinoff
(`fast-focused-tests`, run `01m4t0xyja6wpxb6afj5fab82g`) had its first merge
refused because the main worktree held another session's untracked
`design-system/prototypes/rounds/2026-10-13-katsaus/`. Two sessions working in
the same main is the normal pattern there (a prototyping session alongside a
conductor), so worker merges fail for reasons unrelated to their work and the
worker has to retry or block.

Expected: untracked files that the fast-forward or merge does not touch should
not block the merge (git itself only refuses when an untracked file would be
overwritten). If refusal is kept for safety, the error should name the paths and
say they are untracked and non-overlapping, so the worker knows to retry rather
than report a conflict.
