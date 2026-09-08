# Pre-implementation validation — 2026-09-08

Independent source review found more duplicated transition logic than the
initial two-guard diagnosis. A one-guard patch would leave the regression open.

## Four existing mechanisms must agree

1. `reducer.rs` adopts a late confirmed merge for a failed node, but the
   log-derived `NodeStatusAcc::observe` in `cancel.rs` ignores all reports after
   terminal node state. Share the actual confirmed-merge transition/authority
   predicate; minimally extend the streaming report facts rather than add a
   second scanner or another raw `via` check.
2. `supervise/cleanup.rs::rollup_status` currently skips every terminal manifest.
   Permit only a failed run's confirmed-merge recovery after a nonempty,
   successfully terminal topology. Keep cancelled/done guards and failed/live
   sibling protection. Linked-child completion belongs to the supervisor;
   all-terminal alone is not proof of all-successful recovery.
3. `reduce_run_status` must validate the same narrow failed-to-done exception.
   Any log evidence consulted during replay must be bounded before the event
   being reduced, so a later merge cannot authorize an earlier transition.
4. The ordinary supervisor rollup key already belongs to the earlier failure.
   Preserve that key for ordinary rollup and use a distinct deterministic
   recovery key tied to durable authorizing merge evidence, such as its event
   sequence. No clock, PID, or random generation is needed.

## Keep existing ownership

The node reducer owns report adoption; the supervisor owns cross-run topology
and run completion; the core reducer validates local deterministic state.
Projecting a manifest directly from the node-report reducer risks duplicating
topology decisions and losing the `run.status` event consumers already observe.
A CLI-only post-merge repair misses restart and crash-recovery paths.

## Behavioral proof

Extend the existing supported failed-run merge integration test to assert the
final run, node, report, and landing result. Cover prior failed-rollup key reuse,
log-fold/projection agreement, forged or malformed authority, cancelled state,
failed/live siblings and linked children, restart/rebuild/idempotency, and replay
ordering. Preserve watermark fault coverage. No new lifecycle state, generic
terminal reopen, or inferred Git success is needed.

## Ordering and fixture clarification

The supervisor currently computes rollup outside the later append lock. A
concrete ordering is therefore: compute Failed, adopt a confirmed merge as node
Done, append the already-computed Failed run status. Recovery must accept that
still-authoritative merge even though its sequence precedes the failure append.
Bound authorization evidence before the proposed recovery event, not after the
failure event. Cover this interleaving directly rather than adding temporal state.

The existing `terminal_but_unmerged_run_still_merges` fixture is an unsupervised
skeleton with a fake merge script. Production `run merge` already reattaches a
previously supervised run, while deliberately leaving never-supervised skeletons
alone. Final run-status evidence needs a truthful supervised fixture; do not
introduce a second rollup owner or change skeleton policy merely to extend that
test's assertions.

This is source-grounded implementation guidance, not completed implementation
or test evidence; the worker must validate it against its final source.
