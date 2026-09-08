---
created: 2026-08-21
updated: 2026-09-08
type: feature
reporter: jari
status: done
priority: normal
related: ['@split-stint-start-handoff', '@stint-start-autonomous', '@stint-handoff-intake-check', '@add-configurable-agent', '@config-subcommand', '@pi-background-jobs-extension']
lane: workflow-skills
lane_seq: 50
closed: 2026-09-08
closed_by: pi
---

# Remove end-to-end stint friction without durable lifecycle state

_Source: bundled `stint-start` and `stint-handoff` skills_

## Goal

Implement the accepted Option D as a small, skill-only workflow simplification. A normal
`/stint-start` invocation carries one bounded round through a product-owner report and
ends with one exact next action. The user's immediate feedback may drive at most one
bounded follow-up. `/stint-handoff` remains the explicit, independently invocable finish.

## Ownership and safety

- issuectl remains the sole scheduling DAG owner; `TODO.md` remains narrative handoff.
- Repository `AGENTS.md` remains the source for green gates, deploy/release mechanics,
  hot files, and repository-specific authority.
- Taskfleet runs remain worker truth; `run merge` remains the only success truth.
- The conductor retains full IDs of runs it launched or explicitly adopted. Only those
  runs can block this session's handoff.
- Known foreign runs, including runs in the same repository, do not become session-owned
  and do not enter its handoff narrative. Their issue resources still become reservations
  so a new round cannot collide with them.
- No force push. A clean source branch may fast-forward or rebase onto upstream; only
  narrow mechanical conflicts may be resolved automatically, followed by the repository
  green gate.
- Workers choose and justify proportionate review after seeing the final diff. Explicit
  review mandates still win, and existing evidence is reused unless the risk surface
  changes.
- Issue-driven implementation workers close with `issuectl close --stamp`, verify a
  stamped/already-present trailer, then commit closure metadata before `run merge`.
  Freeform work and read-only analysis remain distinct.

## Bounded journey

1. `/stint-start` orients from git, repository policy, TODO narrative, the reservation-aware
   issuectl DAG, and explicit session run IDs.
2. It executes one prepared round through workers, validation, and any authorized deploy.
3. It presents one product-owner report and exactly one next action: answer named feedback,
   run one exact wait/recovery command, or invoke `/stint-handoff`.
4. One immediate user-feedback response may trigger one additional full bounded pass.
5. After that pass, larger/new work is recorded durably and the exact next action is
   `/stint-handoff`.
6. `/stint-handoff` may also be invoked directly. An explicit user finish request is
   sufficient; no prior next-action token exists. It records current-turn answers in their
   existing canonical issue/docs/TODO owner, checks only session-owned run IDs, verifies
   the DAG with foreign-run reservations, invokes `/wrap-up`, and verifies cleanliness.

## Acceptance criteria

- One normal start invocation reaches a product-owner report and emits one precise next
  action without requiring the user to remember a chain of status skills.
- At most one user-feedback follow-up round occurs per invocation; silence and an empty
  frontier never authorize another round.
- Terminal finish remains explicit and `/stint-handoff` remains directly composable.
- Unrelated global runs do not block or pollute handoff; reservations still prevent
  repository resource collisions.
- The workflow adds no persisted session state, config framework, lifecycle noun, umbrella
  skill, Taskfleet scheduler, or background agent loop.
- Public artifacts remain neutral and preserve repository policy and explicit authorization
  precedence.
- Rendered bundled skills and regression scenarios cover the bounded journey, direct
  handoff invocation, safe source synchronization, review discretion, issue stamping, and
  foreign-run isolation.

## Superseded exploration

The original proposal explored durable stint/checkpoint state, user-owned automation
configuration, and a harness wake/resume protocol. Those ideas and Options A–C remain in
`analysis.md` as historical exploration, not active requirements. They must not guide this
slice. Reassess durable checkpointing only after observing friction that remains after
Option D.

## Decisions

### 2026-09-07T18:15:49Z · @jari

Selected Option D from `analysis.md` as the first delivery: remove command-chain friction
without adding durable stint state, a new Taskfleet lifecycle noun, another scheduler, or
a background agent loop. The start path should carry one bounded round through the PO
report and emit one exact next action; feedback may drive at most one bounded follow-up;
terminal handoff remains the explicit finish. Reassess a durable checkpoint only after
observing what friction remains.
