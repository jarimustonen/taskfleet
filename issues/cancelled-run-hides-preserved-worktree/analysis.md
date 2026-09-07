# Joint product recommendation: make preserved work visible and disposable

This analysis also resolves the product question in [`intake-feature-taskfleet-41343c4dd3e4`](../intake-feature-taskfleet-41343c4dd3e4/analysis.md).

## Recommendation

Add one read-only `preserved_work` field to `run show`, and one explicit command:

```text
taskfleet run discard <run-id> --reason <text> [--node <node-id>] [--force] [--dry-run]
```

`preserved_work` answers **what Taskfleet still owns**. `run salvage` continues to mean **finish and merge reviewed work**. `run discard` means **record an operator decision and remove preserved work without merging it**. It is a domain transition rather than CRUD deletion: the run record and its history remain.

This is the smallest coherent change. It serves both `failed` and `cancelled` terminal runs without changing either status, weakening teardown safety, or adding another lifecycle.

## Current behavior in product terms

Taskfleet is protecting the user correctly but reporting the result incompletely.

- `run merge` is the only success truth. It authorizes full teardown.
- Failed and blocked outcomes preserve the branch and worktree.
- Cancellation is not permission to destroy work. The supervisor removes a cancelled node only when source-relative commit checks, worktree cleanliness, and actual-HEAD checks prove that removal is safe. A dirty, unmerged, detached, or unverifiable tree is preserved.
- Preservation is audited as `cleanup.branch_preserved`, but that event is not projected into `run show`.
- `recoverable_work` is a narrower signal. It is stamped only on a supervisor-generated failed report when the branch has commits ahead of source. It intentionally does not describe uncommitted files, ordinary failed handoffs, or cancelled runs.
- A repeated `run cancel` only converges cancellation. It does not mean “discard whatever cancellation preserved.” A failed run cannot be cancelled at all.
- `run salvage` is a merge path for a single-worker pending/running/blocked/failed run. It deliberately rejects a cancelled run because cancellation is terminal and cannot later be converted to success.

The resulting product defect is visibility and disposition, not preservation. A terminal run can look settled while its recorded worktree still contains user work. The only way to discover or discard it today is raw Git archaeology, which bypasses Taskfleet's audit history.

## Minimal approaches considered

### 1. Broaden `recoverable_work`

Make `recoverable_work` include dirty cancelled worktrees and every preserved failed worktree.

**Reject.** This changes an established field from “failed worker left committed work that may merge” to “some owned resource remains.” A dirty cancelled tree may be worth inspecting but is not eligible for `run salvage`. The field would conflate visibility with mergeability and would break callers that treat `recoverable_work.recoverable` as a salvage signal.

### 2. Make repeated `run cancel` clean up, or add `run cancel --force`

**Reject.** Cancellation answers “stop this run”; discard answers “I reviewed the retained work and authorize deletion.” Combining them would make an idempotent retry of cancellation potentially destructive. It also would not naturally serve already-failed runs.

### 3. Add `preserved_work` plus `run discard`

**Recommend.** One computed read field exposes the hold regardless of how it arose. One explicit, audited terminal command disposes of holds from either failed or cancelled runs. Existing merge, salvage, cancellation, and supervisor outcome semantics remain unchanged.

## Read contract

Add an always-present array to the top-level `data` returned by `run show`:

```json
{
  "status": "cancelled",
  "recoverable_work": null,
  "preserved_work": [
    {
      "node_id": "n-0001",
      "worktree_path": "/repo/.worktrees/wt/example",
      "worktree_present": true,
      "branch": "wt/example",
      "branch_present": true,
      "cleanliness": "dirty",
      "unmerged_commits": 0,
      "verification": "verified"
    }
  ]
}
```

The field is a list because a fan-out can retain more than one node's resources. It is empty, not omitted, when no Taskfleet-owned worktree or branch remains:

```json
{
  "status": "failed",
  "preserved_work": []
}
```

A failed run with a clean retained worktree and one unmerged commit is still visible:

```json
{
  "status": "failed",
  "preserved_work": [
    {
      "node_id": "n-0001",
      "worktree_path": "/repo/.worktrees/wt/example",
      "worktree_present": true,
      "branch": "wt/example",
      "branch_present": true,
      "cleanliness": "clean",
      "unmerged_commits": 1,
      "verification": "verified"
    }
  ]
}
```

If Git cannot inspect a present resource, do not hide it and do not guess:

```json
{
  "cleanliness": "unverifiable",
  "unmerged_commits": null,
  "verification": "unverifiable"
}
```

The field should be computed from current filesystem/Git state for terminal `failed` and `cancelled` nodes, using node ownership metadata read with the manifest under the run's shared lock. A historical `cleanup.branch_preserved` event may explain why preservation first happened, but it must not be the existence test: the supervisor may have died before recording it, and resources may later have been removed manually.

`recoverable_work` remains unchanged and can remain null while `preserved_work` is non-empty. This distinction is useful:

- `preserved_work` means “inspect or dispose of these owned resources.”
- `recoverable_work` means “this failed run has committed work that the existing salvage path may be able to merge.”

Keep this field on `run show` only for the first slice. `run list` remains the cheap inventory; terminal handoff tooling can list terminal runs and show the relevant ones. Do not add another persisted node field merely to cache a live Git observation.

## Command contract

### Normal use

Discard one single-worker hold:

```bash
taskfleet run discard 01m1vdtqn42hzadxhga0xc35ma \
  --reason "superseded by the converged infrastructure change"
```

When more than one `preserved_work` row exists, `--node` is required. When exactly one exists, it is selected without requiring `--node`:

```bash
taskfleet run discard <run-id> --node n-0003 --reason "superseded"
```

A truthful dry-run performs every read-only eligibility and Git check and reports the exact resources and force requirement without appending an event or deleting anything:

```bash
taskfleet run discard <run-id> --reason "superseded" --dry-run --json
```

A successful JSON result should be explicit rather than require a follow-up inference:

```json
{
  "run_id": "01m1vdtqn42hzadxhga0xc35ma",
  "node_id": "n-0001",
  "discarded": true,
  "already_discarded": false,
  "removed": {"worktree": true, "branch": true},
  "audit_event": {
    "kind": "cleanup.discard_authorized",
    "seq": 42,
    "actor": "jari",
    "reason": "superseded by the converged infrastructure change"
  }
}
```

The command retains the manifest, node projection, terminal report, agent log, and complete event history. It changes neither run nor node status and never emits a success report.

### Exact safety checks

Before any destructive Git operation, `run discard` must:

1. Resolve the exact run and node and reject a legacy read-only run.
2. Require the run and selected node to be terminal `failed` or `cancelled`. Reject `done` and every live status.
3. Require `--node` when more than one retained node is a candidate; reject an unknown or non-retained node.
4. Refuse while the run supervisor is positively alive. Treat an unreadable supervisor identity as unverifiable and refuse.
5. Reuse the salvage-grade worker identity rule: positive proof that the original worker is alive overrides a stale exit report. Refuse a live worker and an alive-but-unverifiable PID. Accept only a told exit, no PID, or a PID proven gone/recycled.
6. Verify that the recorded path is an exact Git worktree registered to the recorded source repository. Reject path, repository, or checked-out-branch ownership contradictions. A detached HEAD is allowed only when the worktree ownership itself is verified; its commits are being explicitly discarded, not mistaken for merged work.
7. Run `git status --porcelain --untracked-files=all`. A Git error or any other unverifiable ownership/cleanliness result always refuses, including with `--force`.
8. If the worktree is dirty, refuse unless `--force` was supplied. The error and dry-run payload must say that `--force` will delete staged, modified, and untracked files. The audit event must record that force was used and the pre-delete cleanliness snapshot.
9. Append the durable authorization event through the normal locked append-and-apply path **before** deleting anything. Then remove the worktree and only the node's recorded branch. A verified-clean worktree uses non-force `git worktree remove`, preserving Git's atomic dirty-race refusal. A dirty worktree may use force only after the explicit `--force` authorization. Branch deletion may use `-D` because this command, unlike cancellation, is the recorded discard authorization.

These checks deliberately do not require the branch to be merged. Deleting clean but unmerged commits is the core meaning of `run discard`; the required command name and reason make that decision explicit. By contrast, dirty content needs the additional `--force` acknowledgement, and unverifiable content is never deleted.

If worktree removal succeeds but branch deletion fails, return a structured non-zero partial-cleanup error, leave the audit event and run history intact, and report the surviving branch in `preserved_work`. Never print `discarded: true` until both recorded resources are absent.

### Audit and idempotency

Use one no-projection audit kind, `cleanup.discard_authorized`, containing at least:

- full run and node identity (the event envelope already carries these);
- actor (the effective local account, with a stable UID fallback);
- timestamp (the event envelope);
- required non-empty reason;
- recorded worktree path and branch;
- pre-delete cleanliness and commit-ahead observations;
- whether `--force` was supplied.

The event is durable authorization, so it is written before crossing the filesystem deletion boundary. This avoids the unaudited crash window that would result from recording only after deletion. The command then converges the authorized deletion. An interrupted call can be retried safely.

Use an idempotency key derived from the run, node, and normalized authorization inputs. Repeating the same call appends no duplicate authorization and continues any incomplete worktree/branch cleanup. Once both resources are absent, repeated calls return exit 0 with:

```json
{
  "discarded": false,
  "already_discarded": true,
  "removed": {"worktree": false, "branch": false}
}
```

A retry whose reason or force acknowledgement differs from an existing incomplete authorization must fail with an `idempotency_conflict` rather than silently rewriting audit intent. After a completed discard, any repeated call is a no-op and must not create a second historical decision.

## Salvage remains separate

For a failed run, `run salvage` remains the reviewed path that fences the prior writer and delegates to the recorded, OID-recoverable `run merge` transaction. `run discard` is the opposite decision and must never call merge.

For a cancelled run, cancellation remains final: do not allow `run salvage` to rewrite it to success. `preserved_work` gives the exact path and branch for inspection or manual copying into a new run/branch. Once the owner decides the retained copy is unnecessary, `run discard` records and executes that decision.

This is an intentional distinction:

- **visibility:** `run show … .preserved_work`;
- **salvage/finish:** `run salvage` where the existing status contract permits it, otherwise inspect/copy the preserved path;
- **explicit discard:** `run discard` for terminal failed or cancelled holds.

## What not to build

- Do not add a new run status such as `abandoned`, `cleaned`, or `preserved`.
- Do not add a second lifecycle or a generic cleanup state machine.
- Do not make cancellation destructive or reinterpret a repeated cancel as discard.
- Do not weaken `TerminalOutcome` or make any non-merge outcome earn the existing full teardown policy.
- Do not auto-discard based on age, telemetry, PID absence alone, or a “looks empty” heuristic.
- Do not broaden or rename `recoverable_work`.
- Do not let `run discard` mark a run done, synthesize a merge report, or change `landed`.
- Do not add a background janitor, retention policy, or automatic terminal-run garbage collection.
- Do not persist live Git cleanliness in the node DTO; report it as a current observation.

## Small implementation slices

1. **Visibility slice:** add the computed `preserved_work` array to `run show` and text output, with no mutation. Review independently against shared-lock reads and conservative Git classification.
2. **Disposition slice:** add `run discard`, dry-run, the authorization audit event, and convergent cleanup for one selected node. Reuse existing Git/PID safety helpers where their semantics match; do not route through `TerminalOutcome::teardown` because discard is a separate explicit operator authorization.
3. **Workflow wording slice:** update bundled run-overview/handoff guidance to inspect `preserved_work` and choose salvage, manual harvest, or discard. Keep this with the CLI surface release so examples cannot drift.

Likely tests are compact:

- DTO/snapshot tests for empty, clean, dirty, branch-only, multi-node, and unverifiable `preserved_work` states;
- real-Git integration tests for clean failed discard, dirty cancelled refusal, dirty forced discard, untracked files under `status.showUntrackedFiles=no`, detached HEAD, wrong-repository/path refusal, and partial branch-delete failure;
- process-identity tests for live, exited, recycled, and unverifiable workers/supervisors;
- event/replay tests proving authorization is append-before-delete, projection-neutral, idempotent, and retained after restart;
- retry tests for interrupted cleanup, repeated completed calls, and conflicting authorization inputs;
- invariant regressions proving status/`landed` do not change and `run merge` remains the only success truth.

The implementation should continue to use locked append-and-apply for events, shared locking for multi-projection reads, and fail-closed Git checks. Telemetry has no role in either visibility or deletion authorization.
