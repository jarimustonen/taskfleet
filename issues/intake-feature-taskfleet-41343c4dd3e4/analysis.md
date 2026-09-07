# Disposition: one terminal-work discard path

Treat this feature and [`cancelled-run-hides-preserved-worktree`](../cancelled-run-hides-preserved-worktree/analysis.md) as one product change. The joint analysis contains the proposed contract and safety rules.

The recommended disposition is:

- add `run show.data.preserved_work` as a live inventory of retained worktrees/branches for terminal failed and cancelled nodes;
- add `taskfleet run discard <run-id> --reason <text> [--node <node-id>] [--force] [--dry-run]` for an explicit, audited, idempotent removal decision;
- keep the manifest, reports, logs, and events after discard;
- keep failed and cancelled statuses unchanged;
- require a dead/proven-gone worker, exact worktree ownership, and verifiable Git state;
- refuse dirty work unless `--force` explicitly acknowledges it, and refuse unverifiable work even with force.

Use `discard`, not `abandon` or generic `cleanup`: it states what happens to the preserved work without implying a new run lifecycle. `run salvage` remains the merge/finish path for eligible failed runs; cancellation remains final. Repeated cancel must stay non-destructive.

Do not add an `abandoned` status, a cleanup lifecycle, automatic expiry, or raw-Git cleanup guidance as the product interface.
