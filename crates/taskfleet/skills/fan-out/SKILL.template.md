---
name: fan-out
description: Fan out N≥5 similar, fully independent units as parallel autonomous worktrees via one top-level `--kind fan-out` run plus one parent-pointed child per unit. Each child commits one disjoint output and self-merges. Manages enumeration, concurrency (default 10), manifest state, and resume. Requires a clean git source branch. NOT for generic parallel commands, dependencies, shared-file edits, or per-unit human review; use issuectl-scheduled `/stint-start` waves instead.
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# fan-out

A **fan-out** is a top-level driver run plus N≥5 sibling children, each
processing one of N similar, fully independent units. Every child writes
a disjoint output file (so siblings never edit the same path), commits,
and merges itself back. The driver tracks per-unit state in the run's
manifest and supports resume across interruptions.

Read `taskfleet-overview` first; read `worktree-spinoff` for the
shared autonomous-merge contract; read `worktree-orchestrated` to
understand the child-spawn pattern (`fan-out` uses the same
parent-pointer mechanism but the children are homogeneous instead of
DAG-ordered).

## When to use

- ✅ N≥5 fully independent units doing the same operation on different
  inputs ("convert each receipt PDF to JSON", "regenerate each NFO
  file in `media/`", "apply codemod across 47 packages").
- ✅ Each unit's output is a disjoint file or path (siblings cannot
  race).
- ❌ Fewer than 5 units → just spawn `/worktree-spinoff` N times.
- ❌ Units share output (edit the same file) → serialize focused
  `/worktree-spinoff` runs or restructure to disjoint outputs.
- ❌ Units have dependencies (B needs A's output) → let issuectl own the DAG and run
  bounded waves through `/stint-start`.
- ❌ Per-unit human review required → create each spinoff with explicit `--interactive`.

## Workflow

### 0. Validate context

1. Working directory must be a git repo with a clean current branch.
2. Enumerate the units. The enumeration MUST be deterministic and
   re-derivable from the source branch state (file glob, JSONL input
   file, generated list committed to the repo). Re-enumeration on
   resume must produce the same list in the same order; otherwise the
   manifest cannot resume correctly.
3. Decide concurrency. Default is 10. Raise only if the units are
   genuinely cheap and the machine can take it; lower for heavy
   per-unit cost (model calls, IO bottlenecks).
4. `taskfleet version --output json` to confirm
   `{{CLI_VERSION}}`.

### 1. Build the unit brief template

Each child gets a self-contained brief with one unit interpolated.
The template should include:

1. **Per-unit objective** — what to produce for unit `<id>`.
2. **Input path** — the source the unit reads (the enumerated input).
3. **Output path** — the disjoint file the unit writes. Use the unit
   id or a hash so two unit ids cannot collide.
4. **Done criteria** — output file exists, committed. If a unit changes code,
   copy the repository's exact green gate from `AGENTS.md`, including release
   mode, lockfile enforcement, and warnings-as-errors; do not replace it with a
   debug-mode `cargo test`. For taskfleet this is `cargo fmt --all --check`,
   `cargo clippy --locked --workspace --all-targets -- -D warnings`, `cargo
   nextest run --locked --release --workspace`, `cargo test --locked --release
   --workspace --doc`, and `RUSTDOCFLAGS="-D warnings" cargo doc --locked
   --workspace --no-deps`. Doctests stay separate because nextest does not run
   them. The orchestrator or machine setup provisions nextest with `cargo
   install cargo-nextest --locked`; a child reports it missing rather than
   installing globally. Tool-sensitive tests should approximate bare CI with a
   stripped `PATH`.
5. **Repository-local build safety** — a child may run `cargo build --release`
   and exercise `./target/release/taskfleet …` in its own worktree. During
   repository work, neither children nor the orchestrator may create, replace,
   remove, or modify the user's installed taskfleet or bundled skills by
   any mechanism, including any `cargo install`, `cargo uninstall`, Homebrew,
   manual-copy, or `skill install` variant.
6. **No routine spin-offs or discuss items** — children should run silently
   to completion; surfacing every successful receipt OCR run as a discussion
   item drowns the user. Failed or incomplete tools/sub-workflows are the
   exception and must follow the disclosure contract below.
7. **Tool/sub-workflow failure policy** — copy the disclosure contract below
   into every unit brief and state any finite child-local tool retry bound. A
   required incomplete unit cannot claim success; optional failure may continue
   only when independently safe and disclosed. Child-local tool retries are
   separate from the driver's whole-unit retry budget.
8. **Closing step** — every child MUST take exactly one terminal path, never
   both (see "Terminal report (mandatory)" below). Completed units merge and
   report through `taskfleet run merge`; units blocked by a required
   failure do not merge and submit a direct `success: false` report. Taking
   neither path prevents `child.report` and leaves the concurrency slot held.

### 2. Create the driver run

```
taskfleet run create \
  --kind fan-out \
  --title "<batch-slug>" \
  --task "Driver brief: enumerate <N> units, fan out at concurrency <C>" \
  [--source-branch <branch>] \
  [--idempotency-key <key>]
```

The driver's `--kind fan-out` recipe writes the manifest with all
enumerated units in `pending`, then begins fanning out children up to
the concurrency limit. The driver itself merges nothing.

### 3. Fan out children

For each unit, the driver spawns a child via the same CLI:

```
taskfleet run create \
  --kind fan-out \
  --title "<batch-slug>/<unit-id>" \
  --task "<unit-specific brief>" \
  --source-branch <integration-branch-or-source> \
  --headless \
  --parent-run-id <driver-run-id> \
  --parent-node-id <driver-node-id> \
  --idempotency-key <batch-slug>-<unit-id>
```

Per-unit notes:

- Use a stable `--idempotency-key` per unit (e.g.
  `<batch-slug>-<unit-id>`) so retries on transient errors do not
  double-spawn the unit.
- Prefer `--headless` for fan-out children: a batch of N≥5 (often 20)
  windows would otherwise flood the user's foreground tmux session. With
  `--headless` they land in a detached `headless` session
  (`tmux attach -t headless` to watch); auto-cleanup still closes each
  window on terminal. Use one shared `--tmux-session <batch-slug>` if you
  want the whole batch in its own named session.
- The `child.spawned` event lands on the driver's log; the driver's
  supervisor spawns each child's supervisor (single-arbiter
  invariant).
- Children inherit the same `--kind fan-out` so the supervisor applies the fan-out merge
  policy (merge to the integration branch). Taskfleet reports the tmux display name
  unchanged from workmux; never infer a kind-specific prefix or emoji.

### 4. Drive concurrency + resume

The driver tails its own event log and the manifest:

- Up to `<C>` units in `status: running` at any moment.
- On each `child.report`, inspect its top-level `success`. With `success: true`,
  mark the unit `done`, retain any `Tool/sub-workflow failure —` disclosure from
  an optional failure, and spawn the next unit. With `success: false`, take the
  failed-attempt path and retain its disclosure.
- Retry a failed unit up to the finite whole-unit count chosen in the driver
  brief (state whether it counts total attempts or retries after the first),
  using the same idempotency key. This budget is separate from child-local tool
  retries. On exhaustion mark the unit `failed` and continue independent units.
- Retain disclosures from every attempt, including an attempt later superseded
  by success and a successful unit that continued after an optional failure.
  The aggregate report identifies unit and attempt and never reduces these to a
  failed count. If any required unit remains failed, aggregate `success` is
  `false`; continuing other units preserves results but does not make the
  requested batch complete.
- On driver interruption (Ctrl-C, supervisor crash), `taskfleet
  run reattach <driver-run-id>` rebuilds state from the manifest +
  event log and resumes from the first non-`done` unit.

### 5. Success envelope (driver)

```json
{
  "schema_version": 1,
  "data": {
    "run_id": "01HZ-DRIVER",
    "supervisor": 12345,
    "kind": "fan-out",
    "lifecycle": "autonomous",
    "tmux_window": "<workmux-reported-window-name>",
    "branch": "wt/<batch-slug>"
  }
}
```

### 6. Report to the caller

Tell the user:

- Driver run id, total unit count, concurrency.
- Source/merge branch (every child merges here).
- How to follow: `run show <driver-run-id>` for aggregate counts,
  `event tail <driver-run-id> --follow` for per-unit events.
- Estimated wall-clock if the per-unit cost is known.
- That `run reattach` is the resume path on interruption.

## Terminal report (mandatory)

A child MUST take exactly one terminal path, never both. A completed unit uses
`taskfleet run merge`, which merges the branch and submits the terminal
report stamped `via: "explicit-merge"`. A unit blocked by a required failed or
incomplete step does **not** merge and submits a direct `success: false` report
under "Tool and sub-workflow failure disclosure" below. Either report releases
the concurrency slot; omitting both stalls the batch.

A typical successful fan-out unit has nothing structured to surface, so it may
use the minimal completed path as its final action:

1. **Resolve the exact owning run id** from inside the worktree. Use the
   durable node ownership record, never the branch's display identifier (it is a
   lossy bounded fragment that can repeat, not ownership):

   ```bash
   run_id="$(taskfleet run show --current --output json | jq -er '.data.run_id')" || {
     echo "failed to resolve exact owning run id" >&2
     exit 1
   }
   ```

   This fails closed on missing, duplicate, stale, or malformed ownership
   evidence. If it fails, stop and report the error; do not guess a run id.

2. **Merge and report in one step.** No `--source` is needed — the
   fan-out source/integration branch is the child's recorded
   `source_branch`, and `run merge` defaults to it:

   ```bash
   taskfleet run merge "$run_id"
   ```

   With no `--report-file`, `run merge` submits a minimal
   `{success: true, summary}` report — exactly what a silent fan-out unit
   needs. On a clean merge the child's supervisor winds the unit down,
   mirrors `child.report` onto the driver's log, closes the tmux window,
   removes the worktree, deletes the branch, and frees the slot for the
   next pending unit. No manual tmux/git cleanup.

   On a merge conflict/failure `run merge` exits non-zero with
   `error.code: "merge_failed"` and submits **no** report — the node
   stays live. Resolve the conflict (or run `/complex-rebase`) and re-run
   `taskfleet run merge "$run_id"`.

3. **If the unit genuinely has follow-up** — discussion items, spin-off
   proposals, or wrap-up recommendations — write a §7.3 payload to a temp
   file and pass it with `--report-file`. The file is validated **before**
   the merge, so a malformed report aborts cleanly without merging. Use
   these exact field names; an unknown key like `discuss` /
   `spinoff_candidates` / `wrap_up` passes validation but its contents
   are silently dropped.

   ```bash
   cat > /tmp/node-report-${run_id}.json <<'JSON'
   {
     "success": true,
     "summary": "<unit-id>: <one-line outcome>",
     "discussion_items": [],
     "spinoff_proposals": [],
     "wrap_up_recommendations": []
   }
   JSON

   taskfleet run merge "$run_id" --report-file /tmp/node-report-${run_id}.json
   ```

   - `success` — **required** boolean. `true` when the unit's output
     committed and merged; `false` when the unit failed (the driver
     records it as `failed` and continues).
   - `summary` — optional one-line result; prefix with the unit id so
     the driver's aggregate log is legible.
   - `discussion_items[]` / `spinoff_proposals[]` /
     `wrap_up_recommendations[]` — normally empty for fan-out units; see
     `worktree-spinoff` for the full per-field shape if a unit genuinely
     needs to surface one.

A terminal report is **not optional**. Completed work with no `run merge`, or
blocked work with no direct `node report`, holds its concurrency slot forever.

## Tool and sub-workflow failure disclosure

Before closing, inventory every failed or detectably incomplete tool, command,
external service, review, panel, or delegated workflow.

A step **required** by the unit brief or done criteria that remains failed or
incomplete always blocks this attempt. Do not call `run merge`. Write the
existing §7.3 report payload to `/tmp/node-report-${run_id}.json` with top-level
`success: false`, then submit it with `taskfleet node report "$run_id"
n-0001 --from-file /tmp/node-report-${run_id}.json` (`n-0001` is the sole node
in this child run). An **optional/advisory** failure may continue only when the
unit output is independently complete and safe; disclose it in the full
`success: true` report passed to `taskfleet run merge "$run_id"
--report-file /tmp/node-report-${run_id}.json`, never the minimal form.

Requested completeness is a contract. A missing required command result,
source, or expected artifact is incomplete and cannot be presented as complete.
Retry only when this unit brief authorizes a finite bound; if none does, do not
retry. Record each attempt and its outcome, then take the required or optional
path at exhaustion. The driver separately owns whole-unit retries.

Create one aggregate `discussion_items[]` entry for the child whose `topic`
starts `Tool/sub-workflow failure —`. Cover every distinct failure, coalescing
repeated attempts of the same one: tool/workflow and purpose; expected
completeness; observed exit/error/incompleteness; attempts; affected step;
whether work continued and why safe; suggested bug surface; and a stable
artifact/log path when available. Put actionable retry/recover/accept/file steps
in item-level `options`. Keep the complete entry, including options, at most 2
KiB. Include only a short redacted excerpt; never include secrets, credentials,
personal data, environment dumps, or unbounded logs. Set top-level `summary` and
`success` to distinguish blocked from completed; do not put them inside the
discussion item. Existing prose fields suffice, so do not add a schema or
supervisor state.

## Issue Management

Fan-out children do NOT touch `issuectl`. The driver owns issue
interaction (typically a single epic issue closed when the batch is
done) so that N children referencing the same issue do not race.

## Errors

Likely codes:

- `invalid_arguments` — missing/empty `--title` or `--task`, bad
  concurrency value.
- `enumeration_empty` — the enumeration step produced zero units.
  Refuse to spawn; surface the cause.
- `manifest_corrupt` — resume detected a manifest schema mismatch.
  Tell the user to inspect `<dir>/manifest.json`; do not silently
  drop units.
- `child_spawn_failed` — see `worktree-orchestrated`; the driver
  retries with the same idempotency key.
- `worktree_create_failed` on a child — the driver records the unit
  as `failed` and continues with the rest; do not abort the whole
  batch on one git refusal.

## Following progress

- `taskfleet run show <driver-run-id>` — aggregate counts
  (pending / running / done / failed) and the next units to fan out.
- `taskfleet event tail <driver-run-id> --follow` —
  authoritative stream; `child.spawned` and `child.report` events
  arrive here per unit. For per-child progress read
  `data.manifest.status` (terminal: `done | failed | cancelled`) via
  `taskfleet run show <child-id>`.
- `taskfleet node list <driver-run-id>` — per-unit table.
- `taskfleet node show <child-run-id> <child-node-id>` — terminal report for one
  unit (the child's closing `taskfleet run merge` is what *writes*
  it, merging and reporting in one step; see "Terminal report
  (mandatory)").
- `taskfleet run wait <child-id> <child-id> …` — **block until
  every** listed child settles (terminal `done | failed | cancelled`),
  with the correct backoff baked in. This is the multi-run primitive:
  pass all child run-ids to wait for the whole batch in one process
  (add `--any` to return on the first, `--fail-on-error` to exit
  non-zero if any child failed). Use it instead of a `for id in …`
  shell loop — that pattern silently word-splits under zsh.

## Install or upgrade `taskfleet`

This skill was installed for `taskfleet {{CLI_VERSION}}`. Compare
`.data.version` from `taskfleet version --output json` to
`{{CLI_VERSION}}`:

- **Missing**: tell the user to install through a published distribution channel
  outside this repository workflow, then stop.
- **Older**: ask the user to upgrade; stop — fan-out child-spawn
  semantics may have changed.
- **Newer**: tell the user the installed skill is stale and stop. Refreshing
  installed bundled instructions is published-tool maintenance outside
  repository work; never run `skill install` as part of this workflow.
- **Equal**: proceed.

## Example

```
/fan-out Convert every PDF in corporate/receipts/2026-05/ to JSON via vision OCR — one output file per input under corporate/receipts/2026-05/json/
```
