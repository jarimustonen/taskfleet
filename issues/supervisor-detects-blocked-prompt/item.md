---
created: 2026-10-17
updated: 2026-10-17
type: feature
reporter: jari
status: untriaged
priority: high
provenance: chat
---

# Supervisor detects a worker blocked on an interactive prompt; run wait --check-in

## Description

## What happened

Open Habitat conductor session, 2026-10-16/17 on haapa, taskfleet 0.15.0. Run `01m531x5hck7hfx5wrqdrn6xr1` (kind spinoff, `--interactive`, profile `prototype-opus` = `claude --model claude-opus-5-5 --dangerously-skip-permissions`, telemetry absent) sat **seven hours** (19:46 → 02:42 UTC) on a Claude Code safety prompt. Nobody noticed:

- `run show` all night: `status: pending`, `stalled: false`, `awaiting_input: false`, `attention_required: false`, supervisor alive, `telemetry_counts.absent: 1`.
- The conductor's `taskfleet run wait <id>` returned only at its default 6 h `--timeout`, as `pending`, with nothing saying why.
- The worktree had 14 modified files and no file change after 19:46; no commit.

## What it looks like (so the supervisor can recognise it)

The worker ran a compound shell command containing `rm -rf *` after `cd` into a temp directory, plus `unzip` of a Playwright trace. Even with `--dangerously-skip-permissions`, Claude Code stopped with a modal in the pane (captured with `tmux -S /run/user/1000/taskfleet/tmux-<id>.sock capture-pane -p -t default:2.1`):

```
   Run shell command

 │ Dangerous rm operation on
 │ statically-unresolvable target:
 │ /home/jari/Sources/native-agent-host__worktrees/wt-…/web/*

 Do you want to proceed?
 ❯ 1. Yes
   2. No

 Esc to cancel · Tab to amend
```

Signals that were all true for hours and are cheap for the supervisor (it owns the pane and the worktree):

1. The agent process (pid known to the supervisor) is alive, CPU near zero.
2. The pane content did not change for hours (`capture-pane` hash stable).
3. The last lines of the pane match a known interactive prompt: `Do you want to proceed?`, a numbered choice with `❯ 1.`, `Esc to cancel`. Claude Code has other such modals (permission prompts, "What should Claude do instead?" after a denial, plan approval, trust-folder); Pi and Codex have their own.
4. No file in the worktree changed (mtime) for hours; no commit; no node report.

After answering "2. No" the pane showed `Interrupted · What should Claude do instead?` and an empty `❯` input prompt: an idle agent waiting for a user turn, which is the same failure in another form (signals 1, 2, 4 plus an empty input box with no spinner).

## Proposal

- **Supervisor-side detection** for harnesses without telemetry: when the pane is unchanged for N minutes (e.g. 10) and its tail matches a known prompt pattern or an idle input box, set `awaiting_input` (or a distinct `blocked_on_prompt`) with the pane tail as evidence. The existing `run wait` already wakes on attention signals, so callers need no change; `--notify` hooks fire too.
- **`run wait --check-in <dur>`**: return after the interval with a cheap progress summary even when nothing changed (elapsed, last worktree file change, last commit, pane unchanged-for), so a conductor can do a light check every 30–60 min without scraping tmux itself. Exit code distinct from timeout.
- Consider making the default `--timeout` shorter or the timeout envelope carry the same summary.

Pattern lists belong in a small, harness-specific table so new prompts can be added without code changes.
