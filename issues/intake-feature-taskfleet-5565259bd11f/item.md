---
created: 2026-09-02
updated: 2026-09-08
type: feature
reporter: jari
status: done
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:taskfleet-stint-start-safe-rebase-20260902
lane: workflow-skills
lane_seq: 20
closed: 2026-09-08
closed_by: pi
---

# stint-start should safely rebase a clean diverged main

## Description

stint-start should safely rebase a clean diverged main

## Observed

`stint-start` 0.5.1 Phase 0 requires `git pull --ff-only` and instructs the conductor to stop whenever the pull cannot fast-forward. In a 3DBear stint the working tree was clean, local `main` was one commit ahead and two commits behind `origin/main`, and upstream had not touched the local commit's file. The command aborted as designed, but the session stopped and reported a vague source-code divergence to the product owner. The user had to identify the intended recovery explicitly: run pull with rebase and push.

Exact failing command and result:

```text
git pull --ff-only
fatal: Not possible to fast-forward, aborting.
```

This conflicts with the consuming repository's documented normal workflow, `git pull --rebase && git push`, and creates an unnecessary check-in during a maximally autonomous stint.

## Expected

When all of these are true:

- the current branch is the repository's normal source branch;
- the working tree and index are clean;
- there is ordinary local-ahead/remote-ahead divergence;
- no force operation is required;

`stint-start` should attempt a safe fetch plus rebase onto `origin/main`, then continue if it succeeds. It should stop only for a dirty tree, a rebase conflict, an incompatible branch policy, or another genuinely ambiguous state. The operation must never force-push.

If automatic rebase is intentionally out of scope, the skill should at least describe the condition plainly and recommend the exact safe recovery instead of exposing a generic “parallel source versions” explanation.

## Impact

The current behavior interrupts otherwise prepared autonomous rounds, confuses non-technical product owners, and requires a manual instruction for the repository's standard synchronization operation.

## Decisions

### 2026-09-07T18:15:48Z · @jari

Accepted with broadened autonomy: try fast-forward, then a normal rebase on a clean source branch; allow the agent to resolve clearly mechanical/simple conflicts and verify them with the repository green gate. Abort and surface semantically ambiguous or broad conflicts. Never force-push.
