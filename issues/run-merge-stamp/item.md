---
created: 2026-08-15
updated: 2026-09-07
type: feature
status: open
priority: normal
epic: lifecycle-architecture-review
lane: workflow-skills
lane_seq: 40
---

# Stamp issue trailers in issue-driven worktree skills

## Description

## Description

`taskfleet run merge` lands a worker's branch and closes out the run, but it does
NOT stamp a `Fixes-Issue: @<slug>` / `Refs-Issue: @<slug>` git trailer into the landing
commit. issuectl's changelog is trailer-driven (`issuectl changelog <range>` compiles
release notes from those trailers), and issuectl now auto-stamps the trailer at
`issuectl close --stamp` (issue `changelog-trailers-never`, fixed 2026-08-15). But a large
share of work lands via `taskfleet run merge` in downstream repos, whose landing
commits carry no trailer — so those repos' release notes are still incomplete unless every
worker is individually briefed to run `issuectl close --stamp` before merging (this
session's workaround).

## Desired behavior

When `run merge` lands a spinoff/worktree that resolves an issue, stamp the
`Fixes-Issue: @<slug>` trailer into the landing (merge/squash) commit message, in exactly
the format issuectl's `git_trailers` parses. This makes the "zero human discipline" promise
of option 1 end-to-end: release notes accrue correctly regardless of whether a change is
closed through `issuectl close --stamp` directly or merged via `taskfleet run merge`.

## Notes / open questions
- Where does the run learn the issue slug? A spinoff is often issue-driven (bare slug /
  `issuectl:<slug>`); the run's task/brief or an explicit `--fixes <slug>` on `run merge`
  could supply it. Design the slug source (avoid double-stamping when the worker already
  ran `close --stamp`).
- Match issuectl's trailer format exactly (`git interpret-trailers`, canonical last
  paragraph, `--if-exists doNothing` so a hand-stamped trailer isn't duplicated).
- Cross-referenced from issuectl `changelog-trailers-never` (the issuectl half; this is the
  taskfleet half).

Filed from the issuectl 2026-08-15 stint (product-owner: @jari).

## Decisions

### 2026-09-07T18:15:48Z · @jari

Rescoped to a bundled-skill change rather than a new Taskfleet runtime argument. For an issue-driven worker, after the final implementation commit and before `run merge`, run `issuectl close <slug> --stamp --json`, require stamp status `stamped` or `already_present`, then commit the issue closure metadata. Free-form runs without an issue need no trailer; read-only analyses use `Refs-Issue` and do not close.
