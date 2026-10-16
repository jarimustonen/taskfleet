---
created: 2026-10-16
updated: 2026-10-16
type: task
reporter: jari
status: open
priority: high
---

# Environment-neutral bundled skills; issuectl conditional per repository

Jari, 2026-10-16, from the Open Habitat review of the Taskfleet skills (`~/Sources/native-agent-host/issues/taskfleet-skills-for-open-habitat/report.md`): the bundled skills are going to be used by agents on other hosts as well (Open Habitat: a mapped Unix user, a browser instead of a tmux seat, no Homebase, no `tw`, no Telegram, no desktop). Jari's direction: fix them at the source, in Taskfleet, rather than forking variants.

- **Command skills** (`taskfleet-overview`, `taskfleet-run-overview`, `taskfleet-spawn-spinoff`, `worktree-merge`) teach the CLI and must be environment-neutral. They must not assume a desktop seat, a foreground tmux window, a notification hook, or Homebase-specific profiles; where those exist, they are optional.
- **Workflow skills** (`worktree-*`, `fan-out`, `stint-*`, `night-stint-prep`) describe ways of working. issuectl must become conditional and repository-scoped: "if this repository uses issuectl (an `issues/` tree / `issuectl` available), use it for scheduling and results; otherwise work without it". No skill may refer to Homebase, `tw`, Telegram or `notify-jari`.
- Waiting for a run must work as a plain `taskfleet run wait` process the parent awaits, because on Open Habitat the browser shows spinoffs under the parent conversation, and the parent wakes when the wait ends.

## Acceptance Criteria
- [ ] No bundled skill depends on Homebase, `tw`, Telegram, `notify-jari` or a desktop seat.
- [ ] issuectl usage is conditional on the repository using it, in every workflow skill.
- [ ] The skill tests or validator cover these, so the rules survive future edits.
