---
created: 2026-11-06
updated: 2026-11-06
type: feature
reporter: jari
status: done
priority: normal
provenance: chat
source_ref: open-habitat:design-named-workers
lane: named-workers
lane_seq: 1
closed: 2026-11-06
closed_by: claude
---

# Named resident workers that work in rounds, land, sleep and resume

## Description

Requested by Jari (2026-11-06) from open-habitat; design and his decisions:
`open-habitat` `issues/design-named-workers/result.md` and its
`## Decision` sections (read them there: `~/src/open-habitat/issues/design-named-workers/`).

Named (resident) support workers that work in rounds. A worker finishes a
round, lands it, reports, and its process exits; its worktree, branch and
native session (Pi session file, Claude session id) stay. The next message
resumes the same session in the same worktree.

Requested surface (names may change in the ADR):

- `run create --resident --name <name>`; the name resolves only among the
  repository's live runs and refuses ambiguity.
- `taskfleet node round-report --summary-file <f>` records
  `node.round_reported`; the supervisor treats the exit that follows as the
  end of a round, not a crash or `attention_required`. Typed, told state,
  no inferred idleness (ADR 0001).
- `run wait` settles on a round report immediately (no awaiting-input grace).
- `run message <name|id>` to a worker between rounds: first rebase the
  worktree's branch onto the source branch tip (main into the worker's
  tree), then start a new attempt from the same native session (Pi
  `--session`, Claude `--resume`) with the message as its first input; the
  message closes the open round report. A live Pi worker keeps today's
  adapter path. Conflicts go to the worker as the round's first task.
- `run land <id>`: rebase and merge the round's commits into the source
  branch while keeping worktree, branch and session; allowed only while
  the worker process is not running.
- Idle expiry (configurable, default 24 h after the last round report):
  with everything landed, remove the worktree and keep the session; a
  later message recreates the worktree at the same path from the source
  branch and resumes (Claude resumes only in the same directory). With
  unlanded work, preserve it and raise attention; never merge automatically.
- Skill text: a "Named support workers" section in the worktree skills
  (draft in the open-habitat result.md), including that the person never
  messages a worker directly; the parent agent relays.

Habitat will display these runs and wake its conversation on a round
report (open-habitat `named-workers-in-habitat`), so `run show`/events
must expose the round state and the latest report summary.

## Acceptance Criteria

- [x] ADR recording the resident/round model and how it fits ADR 0001
- [x] the surface above works for Pi and Claude workers, with tests of round report, resume after exit, land, rebase-on-message, expiry and revival
- [x] bundled skills updated; `scripts/validate-local-release.sh` green
