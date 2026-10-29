---
created: 2026-10-29
updated: 2026-10-29
type: bug
reporter: agent
status: untriaged
priority: normal
provenance: other
provenance_detail: open-habitat worker run (Jari's test-server finding 28.10.)
source_ref: taskfleet:01m64kqx0vavm1pnwjnjf60dyn/headless-pi-worker-trust-prompt
originating_run: 01m64kqx0vavm1pnwjnjf60dyn
originating_run_kind: spinoff
---

# Unattended Pi worker stops at Pi's project trust prompt in its new worktree

## Description

# Unattended Pi worker stops at Pi's project trust prompt in its new worktree

## Observed

Open Habitat test server, 2026-10-28 (Habitat pilot.447): `taskfleet run create
--kind spinoff --headless` with harness `pi`, profile `builtin`, started Pi in
`~/src/open-habitat__worktrees/wt-…`. Pi stopped at once at its project trust
prompt (*Trust / Trust parent folder (…) / Trust (this session only) / Do not
trust*). Nobody answers a headless run's terminal, so the run stayed `pending`
with only the Pi session header written, indefinitely. Habitat now shows such
a worker as waiting for input at its terminal (open-habitat issue
`headless-pi-worker-trust-prompt`), but the run itself cannot proceed.

## Cause

Pi 1.3 asks for project trust when the cwd has trust-requiring project
resources: `.pi/{settings.json,extensions,skills,…}` or `.agents/skills` in
the cwd or an ancestor (`hasTrustRequiringProjectResources`). A new worktree
path is not in `~/.pi/agent/trust.json` unless an ancestor is, so every fresh
worktree of a repository that carries `.agents/skills` prompts. Taskfleet's
builtin Pi command is plain `["pi"]`, so the interactive TUI prompt is the
default. On Jari's own machines the prompt does not appear only because
`~/src` is trusted in `trust.json`; an account without that entry (every
Habitat user, whose browser Pi runs in RPC mode and never records a decision)
hits it on the first worker.

## Expected

A Taskfleet-launched Pi never blocks on the trust prompt. Suggested rule: the
worktree inherits the trust decision Pi has for the run's source checkout
(`ProjectTrustStore.get(source_repo)` semantics: nearest entry in
`~/.pi/agent/trust.json`), passed as `--approve` when trusted and
`--no-approve` otherwise (Pi 1.3 `cli/args.js`), so no new trust is granted
beyond what the person already gave the same repository. At least for
autonomous/headless runs, an undecided source must resolve to `--no-approve`
rather than a prompt. Recording the worktree path in `trust.json` instead
would leave entries behind for every removed worktree.
