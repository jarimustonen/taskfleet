---
created: 2026-08-21
updated: 2026-09-07
type: feature
reporter: jari
status: open
priority: normal
related: ['@split-stint-start-handoff', '@stint-start-autonomous', '@stint-handoff-intake-check', '@add-configurable-agent', '@config-subcommand', '@pi-background-jobs-extension']
lane: workflow-skills
lane_seq: 50
---

# Remove end-to-end stint friction without durable lifecycle state

_Source: skills/stint-*_

## Description

Turn the current collection of `/stint-start`, `/stint-handoff`, `/worktree-status`, and `/wrap-up` skills into one resumable end-to-end stint lifecycle. A normal invocation should be able to:

1. orient and execute the prepared work autonomously;
2. run the handoff and wrap-up preparation automatically when the work settles;
3. present the remaining questions and test/decision points in the product-owner style of `worktree-status`;
4. pause durably for the user's answers;
5. accept an explicit acknowledgement/finalization after those answers are recorded; and
6. leave the repository and stint state ready for the next `/stint-start` without a manual chain of skill invocations.

The desired experience is a bounded loop that advances autonomously until the next meaningful user-feedback checkpoint, rather than an unbounded agent loop or a sequence the user must manually remember.

## User-owned policy and prompt context

Explore a user-owned taskfleet configuration layer, using the existing `~/.taskfleet/config.toml` conventions (and `$TASKFLEET_HOME`) rather than embedding Jari-specific behavior in public source artifacts. Candidate settings include:

- whether automatic handoff/wrap-up is enabled;
- a path to a user-owned prompt/instruction file whose text is appended to handoff/wrap-up context;
- autonomy policy for commits, pull/rebase, push, deploy, and release decisions;
- which actions are allowed autonomously, which require a checkpoint, and which are forbidden;
- checkpoint presentation/finalization behavior.

The design must decide precedence and ownership rather than creating a second conflicting policy source. In particular, repository `AGENTS.md` currently owns project-specific green gates, deploy commands, release mechanics, and explicit autonomy decisions. Ideation must determine which values belong in user config, repo config/policy, or agent instructions, and expose the effective layered result with source provenance. Free-form prompt text may supplement policy but must not silently override structured safety gates.

Public bundled skills, help text, snapshots, examples, and defaults must remain neutral and contain no personal paths, names, accounts, release preferences, or private workflow assumptions.

## Harness loop / pi.dev integration

Investigate how pi.dev can advance the lifecycle again after an asynchronous worker settles or after the user acknowledges a checkpoint. Keep the architecture harness-neutral:

- taskfleet owns durable stint/checkpoint state and a composable CLI/JSON contract;
- a pi.dev extension/runtime adapter may observe or wait on that neutral contract and inject a follow-up turn;
- taskfleet must not import `@aliou/pi-processes`, access its manager/EventBus, assume its process IDs or log paths, or make a session-scoped pi process the durable state owner;
- the existing homebase ADR 0011 decision and the obsolete `@pi-background-jobs-extension` issue are constraints, not a proposal to rebuild a custom background-jobs extension here.

The adapter may belong outside this repository. This issue should identify that boundary and stage follow-up work in the owning repository rather than coupling it into taskfleet.

## Phase 1 — workflow ideation and decision (first delivery)

Start with an ideation/design phase tailored to Jari's actual work pattern. Do not implement the lifecycle before this phase is reviewed.

1. Map the current happy path and interruption paths across `stint-start`, `stint-handoff`, `worktree-status`, and `wrap-up`.
2. Walk through concrete scenarios with Jari:
   - a fully green autonomous round with no questions;
   - a round that ends with test/decision questions;
   - feedback that triggers another small work round;
   - a failed gate, recoverable worker, or preserved worktree;
   - a release-capable repository versus a repository with no deploy/release step;
   - closing the session now versus keeping the pi.dev loop alive.
3. Propose a small explicit state machine and vocabulary, including the durable user-feedback checkpoint and the final acknowledgement that makes the next stint eligible.
4. Compare at least two ownership models: skill-only orchestration versus a thin taskfleet stint/checkpoint state surface with skills as policy/execution clients.
5. Design the layered config and effective-policy inspection surface, including safe defaults and conflict handling.
6. Define the neutral wake/resume protocol that a pi.dev adapter could consume without harness coupling.
7. Record the chosen design under this issue (`design.md` and, if architectural, an ADR via the technical-decision workflow), with phased implementation slices and migration/backward-compatibility notes.
8. Stop at a human review checkpoint before scheduling implementation slices.

## Design questions

- Is a stint itself durable taskfleet state, or is only its user checkpoint durable while issuectl/TODO remain the scheduling and handoff sources?
- What is the exact command/skill that acknowledges a checkpoint, records decisions, and marks the next stint ready?
- Can automatic wrap-up safely run while any worker remains live, awaiting input, recoverable, or repository-unidentified?
- Which steps are mandatory invariants versus configurable conveniences?
- How are policies represented so `config show --json` can explain every effective value and source?
- How does a repository tighten user defaults without being able to grant itself unsafe authority?
- How does a no-pi environment complete the exact same lifecycle manually through composable CLI commands?
- What are the loop bounds and escape hatches so an adapter cannot spin indefinitely without a user checkpoint?

## Acceptance criteria

### Phase 1

- Current workflow and failure paths are documented from the shipped skills and CLI surfaces.
- Jari's target workflow is captured through scenario-based ideation, not inferred solely from existing prose.
- A reviewed state-machine proposal defines start, executing, preparing-handoff, awaiting-user, finalizing, and ready-for-next-stint semantics (names may change).
- Config ownership, precedence, safe defaults, and effective-policy observability are decided.
- The taskfleet/pi.dev boundary honors the harness-neutral constraint and names any external follow-up repository.
- Implementation is split into independently reviewable issues only after the design checkpoint.

### Eventual outcome

- One normal entrypoint can drive a stint from start to the next meaningful user checkpoint.
- User answers can be recorded and explicitly acknowledged/finalized, after which the next stint starts from a coherent durable state.
- Existing individual skills remain usable and composable; environments without the pi adapter are not second-class.
- No personal policy or private prompt text is compiled into or committed with taskfleet.
- Autonomous commit/push/deploy/release behavior is explainable, bounded by effective policy, and fails closed when policy or required commands are ambiguous.

## Related work

- `@split-stint-start-handoff`
- `@stint-start-autonomous`
- `@stint-handoff-intake-check`
- `@add-configurable-agent`
- `@config-subcommand`
- `@pi-background-jobs-extension` (obsolete; records the superseding boundary decision)

## Decisions

### 2026-09-07T18:15:49Z · @jari

Selected Option D from `analysis.md` as the first delivery: remove command-chain friction without adding durable stint state, a new Taskfleet lifecycle noun, another scheduler, or a background agent loop. The start path should carry one bounded round through the PO report and emit one exact next action; feedback may drive at most one bounded follow-up; terminal handoff remains the explicit finish. Reassess a durable checkpoint only after observing what friction remains.
