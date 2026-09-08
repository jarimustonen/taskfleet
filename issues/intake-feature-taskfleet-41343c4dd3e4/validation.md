# Pre-implementation validation — 2026-09-08

The regression-round conductor and an independent read-only reviewer checked
the accepted joint design against current source. The product scope remains
one computed resource inventory and one explicit discard command.

## Reuse, with exact semantics

- Read manifest and nodes together using the shared-lock pattern in `run/show.rs`.
- Reuse Git cleanliness, HEAD, commit-count, and removal operations in `git/repo.rs`.
  A new small exact registration query is needed: `Git::main_worktree` returns
  the first entry and does not prove ownership of the selected target.
- Reuse the worker identity policy in `run/salvage.rs`, but correct its current
  uncertainty distinction when extracting shared logic. `positive_live_identity`
  returns no match both for a verified PID mismatch and an unavailable OS
  start-time probe. `classify_worker` currently treats either as gone when a
  recorded identity exists. Without a told exit, an alive process with an
  unavailable identity must remain unverifiable. Positive proof of the original
  live worker continues to override a stale told exit.
- Use existing supervisor PID classification, locked append-and-apply, and
  projection-neutral audit handling. Do not nest a locking append inside an
  already-held run lock.

## Clarifications needed for a convergent command

1. An interrupted removal can legitimately leave only the recorded branch.
   That state cannot require the already-removed worktree to remain registered.
   Verify the recorded repository, surviving branch, and prior authorization;
   distinguish an absent path from a different existing directory at that path.
2. `append_and_apply_event` deduplicates an existing key without comparing its
   payload. Look up authorization for the run/node and compare reason/force
   inputs for an incomplete operation. Hashing different reasons into separate
   keys does not enforce the accepted conflicting-retry refusal.
3. No resources plus no authorization is not evidence that an authorized discard
   occurred. Keep manually absent resources distinct from a completed recorded
   discard; do not invent audit history.

These are bounded domain checks, not reasons to add another transaction journal,
cleanup scheduler, persistent cleanliness cache, rescue-ref lifecycle, or policy
engine. Tests must exercise the actual destructive boundaries in disposable
repositories. This source review did not execute tests or authorize deletion of
any real retained work.
