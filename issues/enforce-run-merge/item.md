---
created: 2026-08-15
updated: 2026-09-08
type: improvement
status: wontfix
priority: normal
epic: lifecycle-architecture-review
closed: 2026-09-08
---

# Enforce run merge (prevent raw-git self-merge) instead of only detecting it

## Description

The `raw-git-selfmerge-false-failed` work added a **reactive** read-time hint: `run show` surfaces a `false_failed` warning when a worker hand-merged its branch into source with raw git (bypassing `run merge`) and then died. This detects the aftermath but does not prevent recurrence — an agent can keep bypassing `run merge` and the user keeps running `run salvage`.

The issue's other stated option was to **enforce** that `run merge` is mandatory. That is a forward design direction, deliberately deferred out of the 0.2 observability fix.

Surfaced by llm-review (anthropic #16) during the `raw-git-selfmerge-false-failed` review.

**Candidate approaches (needs its own design):**
- A worktree-scoped git hook (`pre-push` / `pre-merge`) installed at worker spawn that refuses a raw merge into the run's source branch.
- Live supervisor detection while the agent is still alive, so it can be told to redo the finish through `run merge`.
- Harness / bundled-SKILL prompt-level reinforcement.

**Constraints:** must not reintroduce any auto-success heuristic (invariant 7); likely lands cleanly with the 0.2.1 pi.dev plugin + durable operation lease (design §2.7). New subsystem with its own security model and lifecycle — too large to bundle into the lifecycle fix.

## Resolution

### 2026-09-08T08:39:43Z · @issuectl

Do not add a worktree Git-hook or enforcement subsystem. Taskfleet keeps run merge as the only success truth and reinforces that contract through its owned workflow skills; the proposed prevention machinery is disproportionate and adds a separate security/lifecycle surface.
