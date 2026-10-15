---
created: 2026-10-13
updated: 2026-10-15
type: bug
status: fixed
priority: normal
lane: lifecycle
lane_seq: 20
collision: [supervise-cleanup]
closed: 2026-10-15
closed_by: agent
---

# Flaky test: managed_session_retained_when_attached fails under the full suite

## Observed

2026-10-13, `cargo test -p taskfleet --release` on commit 469d9503 plus an uncommitted prose-only change to `crates/taskfleet/skills/night-stint-prep/SKILL.template.md`:

```
thread 'supervise::cleanup::tests::managed_session_retained_when_attached' panicked at crates/taskfleet/src/supervise/cleanup.rs:3398:9:
assertion `left == right` failed: the skip must be recorded once
  left: 0
 right: 1
```

The same test passed when run alone against the same tree (`cargo test -p taskfleet --release managed_session_retained_when_attached`), and the full suite passed on the immediately following rerun with no code change (938 passed). The earlier full run that day had also passed. So the failure is order- or timing-dependent inside the combined suite, not deterministic.

## Why it matters

The test guards safety gate #2 of session cleanup: a tmux session with an attached human is retained and a `cleanup.session_retained` event is recorded. A flake here makes the green gate red at random, which the repository's `AGENTS.md` names as the failure mode that only shows up on integrated `main` (compare the 2026-07-25 `supervise::notify` TOCTOU). It reads as "the skip was not recorded", so it may also hide a real race in how the event is appended or read back under concurrent tests.

## Where to look

`crates/taskfleet/src/supervise/cleanup.rs` around the test and `cleanup_managed_session_with`, and whatever `events_of_kind` reads: the test uses a fake tmux script and a temp run dir, so shared state across tests (environment variables, a shared fake-tmux log path, or the current directory) is the first suspect.

## Resolution

### 2026-10-15T18:44:04Z · @agent

Reproduced ETXTBSY under the full release-mode libtest suite; fake tmux scripts now run through a stable shell interpreter. An open writer regression still verifies both no kill and one retention event. Workspace release nextest passed (1186 tests); stripped-PATH regression passed.
