---
created: 2026-10-20
updated: 2026-10-20
type: bug
reporter: homebase-execution-acceptance-agent
status: untriaged
priority: normal
provenance: other
provenance_detail: Observed directly during live heavy-command execution rollout acceptance
source_ref: homebase:hb-exec-acceptance:2026-10-20:caller-create-fence-preflight
---

# Caller-owned create leaves a manifest-less reservation after writer-fence preflight failure

## Description

Observed on Haapa while validating Homebase heavy-command routing, using the installed `taskfleet 0.16.0` (commit `4b3f69abaded08ba8e2a684ab8a30175cc1a2179`). No worker/Pi session was requested or launched; this was a disposable caller-owned Git-only workspace.

The existing same-user `~/.taskfleet` and `~/.taskfleet/runs` directories had mode 0775. A real create with all required caller-owned fields failed:

```
taskfleet run create --source-repo /home/jari/Sources/issuectl \
 --agent-owner caller --kind spinoff --title automatic-routing-acceptance \
 --task 'Disposable Git-only acceptance workspace. Parent runs ordinary cargo check through shared offload; no source edits or agent launch.' \
 --source-branch main --idempotency-key hb-exec-issuectl-worktree-acceptance-20261020
```

Returned `writer_fence_unavailable`: `caller writer fence unavailable: state root, run parent and run must be same-user 0700; preserve the run and checkout`.

The 0700 requirement is understood; this report does not request weakening it. However, the failure occurred after leaving a worktree/branch reservation. After correcting those two directory permissions to 0700 and retrying the exact key with umask 077, create returned:

```
creation_uncertain: run 01m5ch0zbz0qn6hbv5cevcqhpj: staging directory lacks manifest; no recorded run.created identity; preserve /home/jari/Sources/.taskfleet-worktrees/01m5ch0zbz0qn6hbv5cevcqhpj and branch wt/01m5ch0zbz0qn6hbv5cevcqhpj
```

`git worktree list --porcelain` confirms the branch and worktree at Issuectl main `0877f0e5143e9550a87cc8873558940794b20cae`. It did not appear in `taskfleet run list`. No source edits or commits were made in it. An ordinary `cargo check --locked --workspace` there subsequently succeeded through the execution runner, independently of this lifecycle failure.

Attempted supported cleanup:

```
taskfleet run discard 01m5ch0zbz0qn6hbv5cevcqhpj --force \
 --settlement-key execution-acceptance-cleanup \
 --reason 'Parent-only routing acceptance fixture; no worker launched, no source edits, check completed'
```

Returned `invalid_settlement_key`: `--settlement-key is only for caller-owned runs`, although the reservation was created by caller-owned create. The manifest-less worktree and reservation were preserved rather than guessing at state mutations.

Expected: check writer-fence prerequisites before creating Git resources, or expose an unambiguous supported recover/abandon path for this partial creation. Preserve uncertain user work; do not classify the missing-manifest reservation as a normal run for settlement validation. The retained fixture identity above is available for investigation and cleanup; no live process belongs to it.
