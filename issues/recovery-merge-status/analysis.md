# Manual triage analysis

## Verdict

This is a confirmed, material state-consistency bug. Taskfleet supports a user-invoked recovery merge after a run has already rolled up to `failed`, and the merge may land successfully, but the durable run manifest remains `failed` while the node and public landing fields report success.

## Production evidence

Run `01m1xd8tvgqvvf0281btqn04gt` has this event sequence:

1. `merge.started` at seq 4, followed by `merge.aborted` at seq 5 when the target worktree was dirty.
2. A `success:false` `node.report` at seq 6.
3. `run.status {"status":"failed"}` at seq 7.
4. A second `merge.started` at seq 12 after the unrelated dirty file was preserved.
5. A confirmed `success:true`, `via:"explicit-merge"` report with `origin.kind:"run-merge"` at seq 13.

The final projections contradict each other:

- `run show`: `status:"failed"`, `landed:true`, `landed_method:"report-marker"`, and a successful explicit-merge report.
- `node show`: `status:"done"` with the same successful report.
- The worktree and branch were successfully removed, so no recoverable work remains.

## Code path

This is not the crash-recovery classifier in `run/merge_recovery.rs`. The second merge was a normal user-invoked `taskfleet run merge` after the first merge had aborted and cleared its pending transaction.

The relevant behavior is split across two intentional guards:

- `taskfleet-core/src/reducer.rs` adopts a late, confirmed explicit-merge report from `run merge` even for an already failed node and reconciles that node to `Done`.
- `taskfleet/src/supervise/cleanup.rs::rollup_status` immediately returns `None` whenever the run manifest is already terminal. The reattached supervisor therefore cannot reconcile the manifest from `Failed` to `Done`.

The reducer comments explicitly describe the stale terminal manifest as a previously known, out-of-scope residual. The reported production sequence demonstrates that the residual is reachable through a supported recovery workflow and affects public status consumers.

## Focused test evidence

`cargo nextest run --locked --release -p taskfleet --test run_merge terminal_but_unmerged_run_still_merges` passed. That regression test proves that Taskfleet intentionally permits `run merge` against a failed manifest when the worktree still exists. It asserts only that the merge proceeds; it does not assert the final run status, which is the coverage gap exposed here.

An attempted exact-name core test invocation selected zero tests because the test is nested under the reducer module; it provides no additional evidence and is not counted as a successful behavioral test.

## Recommended contract

Keep the supported recovery merge and add a narrow typed recovery transition from run `Failed` to `Done` only after a confirmed successful `run merge` report has been adopted and the complete run topology is now successfully terminal.

Do not generally reopen or rewrite terminal runs, and do not treat arbitrary `success:true` reports or forged `via` fields as authority. The transition must remain tied to the existing confirmed `origin.kind:"run-merge"` / operation evidence and preserve the append-only failure and recovery history.

Add an end-to-end regression assertion that begins with a failed run manifest, performs the supported merge, and verifies coherent final run/node/landing/report state. Multi-node semantics must remain conservative: a recovered node must not erase an independent failed node.
