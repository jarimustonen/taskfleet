---
created: 2026-10-16
updated: 2026-10-17
type: task
reporter: jari
status: done
priority: high
lane: skills
lane_seq: 10
collision: [bundled-skills]
closed: 2026-10-17
closed_by: worker
---

# Environment-neutral bundled skills; issuectl conditional per repository

Jari, 2026-10-16, from the Open Habitat review of the Taskfleet skills (`~/Sources/native-agent-host/issues/taskfleet-skills-for-open-habitat/report.md`): the bundled skills are going to be used by agents on other hosts as well (Open Habitat: a mapped Unix user, a browser instead of a tmux seat, no Homebase, no `tw`, no Telegram, no desktop). Jari's direction: fix them at the source, in Taskfleet, rather than forking variants.

- **Command skills** (`taskfleet-overview`, `taskfleet-run-overview`, `taskfleet-spawn-spinoff`, `worktree-merge`) teach the CLI and must be environment-neutral. They must not assume a desktop seat, a foreground tmux window, a notification hook, or Homebase-specific profiles; where those exist, they are optional.
- **Workflow skills** (`worktree-*`, `fan-out`, `stint-*`, `night-stint-prep`) describe ways of working. issuectl must become conditional and repository-scoped: "if this repository uses issuectl (an `issues/` tree / `issuectl` available), use it for scheduling and results; otherwise work without it". No skill may refer to Homebase, `tw`, Telegram or `notify-jari`.
- Waiting for a run must work as a plain `taskfleet run wait` process the parent awaits, because on Open Habitat the browser shows spinoffs under the parent conversation, and the parent wakes when the wait ends.

## Acceptance Criteria
- [x] No bundled skill depends on Homebase, `tw`, Telegram, `notify-jari` or a desktop seat.
- [x] issuectl usage is conditional on the repository using it, in every workflow skill.
- [x] The skill tests or validator cover these, so the rules survive future edits.

## Comments

### 2026-10-17T13:44:20Z · @worker

Implemented upstream environment-neutral bundled skills, with repository+CLI-gated issuectl, tracker-free serial stints and report analysis, plain native wait/wake contracts, configured profiles, and optional cross-runtime helpers. Reviewed all accepted contract/catalog snapshots. Independent claude-fable-5-1 executor and cross-skill review completed (two calls); assessed findings and localized fixes are recorded in validation.md. Exact scripts/validate-local-release.sh passed on clean 49ae36033316221b0f5f29708c615f07a72bfeb7: 1238 tests passed, 1 skipped by existing suite configuration; doctests, docs, clippy, package/distribution/release checks passed. Gate ran with process-local umask 077 and worker-owned private disk-backed TMPDIR; earlier clippy, permission-fixture, scratch-parent, and snapshot failures were corrected and disclosed in validation.md. Preclosure ready returned false while criteria were still unchecked; all three are now verified. No installed tools, other runs, native run/state/supervisor code, or external repository were changed. No live browser/provider journey is claimed.
