# Taskfleet 🎬

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**Rust CLI for orchestrating AI-agent workflows on a developer's machine.**
It spawns one or many coding agents into isolated git worktrees, supervises
them through a file-based event log, and merges their work back, all behind
one canonical command surface.

Worker creation requires `git`, `tmux`, and `workmux` on `PATH`. Taskfleet owns
the worktree/tmux/launcher transaction itself; it does not depend on Homebase or
`~/.claude/skills/worktree/scripts/create.sh`. Bundled issue workflows also use
`issuectl`.

## What you get

- **One autonomous agent in a worktree.** `run create --kind spinoff` spawns
  an agent that works in its own git worktree and merges itself back with
  `run merge` when done. Zero manual cleanup.
- **N identical units in parallel.** `--kind fan-out` runs many disjoint
  workers, each in its own worktree, each merging itself back.
- **Research and decision workers.** `--kind research` (multi-source
  investigation into a sourced report) and `--kind technical-decision`
  (drives one architectural decision to an ADR).
- **Interactive mode when you want hands on.** `run create --interactive`
  makes the supervisor wait for your explicit `run merge` or `run cancel`
  instead of finalizing the run itself.
- **Run state on disk.** Every spawn writes an append-only event log plus
  crash-safe projections under `~/.taskfleet/runs/<run-id>/`, so any
  consumer (CLI, scripts, a future UI) reads the same source of truth.
- **Work is never silently lost.** Success has exactly one meaning: the
  worker called `run merge`. On any other outcome (failure, cancel, a worker
  that finished but never merged) the supervisor preserves the branch and
  worktree instead of deleting them, and uncommitted changes block teardown.

The orchestration workflows ship as bundled agent skills: install once with
`taskfleet skill install` and commands like `/worktree-spinoff`,
`/worktree-research`, and `/fan-out` appear in your agent sessions. A default
install targets all supported runtimes: Claude Code (`~/.claude/skills/`),
[pi.dev](https://pi.dev) (`~/.pi/agent/skills/`, invoked as `/skill:<name>`),
and Codex (`~/.codex/prompts/<name>.md`). Select one with `--agent
claude|pi|codex`, or use explicit `--agent all`. `--target <dir>` preserves
those layouts beneath a different base, and `--dry-run` previews every write.

## Install

Cargo installs the Taskfleet command:

```bash
cargo install taskfleet
```

The source repository is
[`jarimustonen/taskfleet`](https://github.com/jarimustonen/taskfleet). Homebrew
installs the same command:

```bash
brew install jarimustonen/taskfleet/taskfleet
```

## Quick start

```bash
# Deploy the bundled skills to Claude, pi, and Codex (the default is all):
taskfleet skill install

# Preview an isolated install without writing anything:
taskfleet skill install --target /tmp/taskfleet-skills --dry-run --json

# Verify the installation (expect 0 fail):
taskfleet doctor

# See what's bundled:
taskfleet skill list

# From inside an agent session in any git repo:
#   /worktree-spinoff fix the typo in src/main.rs
# taskfleet handles spawn → work → merge → cleanup.
```

An agent meeting the tool for the first time should read the bundled
overview; it defines the run / supervisor / node vocabulary every other
skill assumes:

```bash
taskfleet skill print taskfleet-overview
```

## How it works

Every spawn is a **run** (`~/.taskfleet/runs/<ulid>/`). A run owns:

- `events.jsonl`: the append-only event log, the canonical source of truth.
- `manifest.json` and `nodes/`: projections reduced from the event log under
  a single per-run flock, with an `applied_seq` watermark so a crash between
  the append and the projection write is replayed on the next lock
  acquisition. State is recoverable from `events.jsonl` alone.
- A per-run **supervisor** process that records told facts (the worker's
  real exit status, the durable `run merge` transition) rather than guessing
  liveness from indirect signals, and that owns worktree and tmux teardown.

Agents read and append events via the CLI; they never touch the projection
files directly. `run merge` itself is a recorded, OID-pinned transaction
across the git refs and the event log, so a crash mid-merge is recovered
instead of stranding work.

Every command follows the family's AI-first CLI conventions: strict input
validation, `--json` / `--output jsonl` envelopes with a schema version,
JSONL logs, meaningful exit codes, and no interactive prompts.

Useful surfaces:

```bash
taskfleet run list                      # all runs
taskfleet run show <run-id> --json      # landed + current preserved_work inventory
taskfleet run wait <run-id> [...]       # block until runs settle
taskfleet run discard <run-id> --reason "superseded" --dry-run # preview audited terminal cleanup
taskfleet event tail <run-id> --follow  # stream the event log
taskfleet config show                   # effective config with per-key source
```

External worker harnesses can report bounded advisory activity through the
public `node telemetry update` endpoint. Its stable, runtime-neutral v1 DTO and
conformance fixtures live in
[`contracts/worker-telemetry-v1/`](contracts/worker-telemetry-v1/). The adapter
runtime is intentionally owned outside this repository.

## Bundled skills

| Skill | Purpose |
|---|---|
| `taskfleet-overview` | First read: the run / supervisor / node vocabulary |
| `taskfleet-run-overview` | Inspect run state (`run list`, `run show`, reports) |
| `taskfleet-spawn-spinoff` | Low-level spawn primitive |
| `worktree` | Router: classifies a request to the right worktree variant |
| `worktree-spinoff` | Autonomous worktree (fire-and-forget, self-merging) |
| `worktree-research` | Autonomous multi-source research into a sourced report |
| `worktree-technical-decision` | Autonomous ADR-producing decision worktree |
| `worktree-bug-analysis` | Read-only bug analysis written back to the issue |
| `worktree-merge` | Close an interactive worktree with one `run merge` |
| `worktree-status` | Plain-language status brief of a worktree session |
| `fan-out` | N identical units in parallel |
| `stint-start` / `stint-handoff` | Session-level orchestration round + handoff |

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all --check
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
```

Repo layout:

- `crates/taskfleet-core/`: schema, event log, locking, reducer, atomic file I/O.
- `crates/taskfleet/`: the `taskfleet` binary, supervisor, and bundled
  skills (`skills/<name>/SKILL.template.md`, embedded at build time).
- `issues/<slug>/`: issues, epics, and their design docs, managed by
  [`issuectl`](https://github.com/jarimustonen/issuectl).
- `docs/decisions/`: architecture decision records, including ADR 0001
  (the thin-supervisor model behind the 0.2 series).

See [ARCHITECTURE.md](ARCHITECTURE.md) for the code map and process
boundaries. `AGENTS.md` contains the operating policy and state-integrity
invariants governing the reducer, lock layer, and teardown paths.

## License

MIT: see [LICENSE](LICENSE).
