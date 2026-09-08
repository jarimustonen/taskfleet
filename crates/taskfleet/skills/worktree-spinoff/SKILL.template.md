---
name: worktree-spinoff
description: Spawn an autonomous spinoff worktree agent via `taskfleet run create --kind spinoff` — one fire-and-forget agent that takes a focused task, executes it in its own git worktree, and merges itself back to the source branch. Use when the user says `/worktree-spinoff <task>`, when a parallel sub-task can be handled without interactive review, or when a driver (`/fan-out`) needs to spawn one autonomous unit. NOT for hands-on interactive review (add `--interactive` to `run create` so the supervisor waits for an explicit `run merge`/`run cancel`), N identical units (`/fan-out`), or dependency-ordered features (stint waves).
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# worktree-spinoff

A **spinoff** is one autonomous agent running in its own git worktree,
doing one well-scoped task, and merging itself back to the source branch
when done. No interactive review. The canonical way to launch one is via
`taskfleet`, which owns the run state under
`~/.taskfleet/runs/<run-id>/` — never hand-craft branches or invoke
`workmux`/`create.sh` directly.

If you have not yet read it, read the `taskfleet-overview` skill
first — it defines the run / supervisor / node vocabulary every step
below assumes.

## When to use

- ✅ User said `/worktree-spinoff <task>`.
- ✅ User asked to spawn a "background", "fire-and-forget", or
  "spinoff" worktree for a focused task.
- ✅ The `/fan-out` driver needs to spawn one autonomous unit and pass
  `--parent-run-id` + `--parent-node-id`.
- ❌ User wants a hands-on, human-driven worktree → spawn with `taskfleet run create --interactive` (the supervisor never auto-terminalizes; the human finalizes with `run merge`/`run cancel`). A default spinoff is always headless + autonomous.
- ❌ N≥5 similar independent units → `/fan-out`.
- ❌ Heterogeneous dependency-ordered features → schedule bounded waves through
  `/stint-start` and the issuectl DAG.
- ❌ Substantial research / ADR → use `/worktree-research` or
  `/worktree-technical-decision`. Bug fixes use this spinoff skill with the existing issue
  slug.

## Workflow

### 0. Validate context

1. If the working directory is not a git repo, abort with a clear
   message — the spinoff needs a source branch.
2. `taskfleet version --output json` once per session. Compare
   `.data.version` to `{{CLI_VERSION}}` (see "Install or upgrade"
   below). Refuse to proceed on a major-version mismatch.
3. Capture the **current branch** with `git rev-parse --abbrev-ref HEAD`
   — it becomes the spinoff's source/merge target by default.
4. **Parse caller passthrough flags.** A driver (`/stint`, `/fan-out`,
   `/worktree-bug-analysis`) may prefix the request with `--headless` or
   `--tmux-session <name>` to request a detached window. Strip these from
   the task text and forward them verbatim to `run create` in step 3.
   Note: **there is no `--review` flag.** A caller that wants the spinoff
   to review before merging says so in the task brief (the *quality bar*,
   step 2.4) — a leading `--review` token, if present, is that same intent
   expressed as a flag; fold it into the brief's quality bar, do not pass
   it to `run create` (which would reject it).

### 1. Identify task source

- **Issue-driven**: the user's prompt contains an issue reference
  (`#NN`, `issuectl:slug`, or a bare slug recognised by `issuectl
  --json show`). Read the issue via `issuectl --json show <ref>` and
  use its title + body as the task brief.
- **Freeform**: the user's prompt IS the task brief. Distill a 2–4 word
  title from it for `--title`.

Skip issue-driven detection when both `--parent-run-id` and
`--parent-node-id` are set (driver mode). An orchestrator fanning out
N spinoffs that all reference the same issue would otherwise update
and close that issue N times.

### 2. Build the prompt

The spinoff cannot ask follow-up questions. The `--task` string must be
self-contained. Include:

1. **Goal** — one sentence on what to deliver.
2. **Context** — files, modules, constraints. Quote relative paths.
3. **Done criteria** — concrete and verifiable. Copy the repository's exact
   green-gate commands from its `AGENTS.md`; never substitute a debug build or a
   looser warning policy. In taskfleet itself the worker runs `cargo fmt
   --all --check`, `cargo clippy --locked --workspace --all-targets -- -D
   warnings`, `cargo nextest run --locked --release --workspace`, `cargo test
   --locked --release --workspace --doc`, and `RUSTDOCFLAGS="-D warnings" cargo
   doc --locked --workspace --no-deps`. Nextest and doctests are separate because
   nextest does not run doctests. The orchestrator or machine setup provisions
   nextest with `cargo install cargo-nextest --locked`; a worker reports it
   missing rather than installing globally. Treat ambient `tmux`, harness binaries, and
   other local tools as suspect; approximate a bare CI runner with a stripped
   `PATH` for tool-sensitive tests.
4. **Repository-local build safety** — a worker may run `cargo build --release`
   and exercise `./target/release/taskfleet …` from its own worktree.
   During repository work, neither workers nor the orchestrator may create,
   replace, remove, or modify the user's installed taskfleet or bundled
   skills by any mechanism, including any `cargo install`, `cargo uninstall`,
   Homebrew, manual-copy, or `skill install` variant.
5. **Quality bar** — unless an explicit user, issue, repository, or caller mandate
   selects a workflow, tell the worker to choose and explain proportionate review after
   seeing the final diff. Focused review is enough for small, local, well-covered work;
   security/privacy, destructive, concurrency-sensitive, architectural, broad,
   hard-to-rollback, or weakly tested work needs stronger independent review. Reuse
   adequate existing evidence unless the risk surface changed. `run create` prepends
   generated run context to every worker brief, including custom `--prompt-file` input.
   That context carries the exact run id and the hard issue-filing boundary:
   worker-filed issues use `issuectl intake file`, are born unlaned, and review
   findings carry machine-visible AI-review provenance plus available metadata.
   Do not weaken that rule or tell a worker to execute an `/assess-findings`-
   staged `issuectl create` command verbatim.
6. **Tool/sub-workflow failure policy** — copy the disclosure contract below
   into the brief. A required failed or detectably incomplete step cannot be
   claimed complete; an optional failure may continue only when safe and must
   still be disclosed in the terminal report.
7. **Terminal report** — the brief MUST end with exactly one terminal path
   (see "Terminal report (mandatory)" below): completed work merges and reports
   through `taskfleet run merge`; work blocked by a required failure does
   not merge and submits a direct `success: false` report. Taking neither path
   leaves the run unterminated and the worktree dangling.

If the prompt is longer than ~2 KB or contains characters that complicate
shell quoting, write it to a temp file and pass `--prompt-file
<path>` instead of `--task <string>`. Use `mktemp -t
spinoff-prompt-XXXXXX.md` and clean up after the call returns.

If any of Goal / Context / Done criteria is genuinely missing from the
user's request, ask the user **once** before spawning. A spinoff that
misinterprets the task wastes a worktree and a merge cycle.

### 3. Create the run

```
# skill-example-ci: skip
taskfleet run create \
  --kind spinoff \
  --title "<2–4 word title>" \
  --task "<self-contained brief>" \
  [--source-branch <branch>] \
  [--headless | --tmux-session <name>] \
  [--notify <cmd>] \
  [--parent-run-id <id> --parent-node-id <id>] \
  [--idempotency-key <key>]
```

Flag rules:

- `--kind spinoff` and `--title` are required.
- `--headless` places the agent's tmux window in a detached `headless`
  session instead of the foreground one, so a batch of spinoffs does not
  clutter the user's window list; attach later with `tmux attach -t
  headless`. `--tmux-session <name>` overrides the default session name
  (and implies headless). Auto-cleanup still closes the window on
  terminal. Example: `taskfleet run create --kind spinoff
  --headless --title fix-lint --task "..."`.
- `--task` OR `--prompt-file` (exactly one). Empty/whitespace-only
  strings are rejected upstream — do not strip silently.
- `--source-branch` defaults to the current branch captured in step 0.
- `--parent-run-id` and `--parent-node-id` are mutually required; pass
  both or neither. The `/fan-out` driver passes them; a user-initiated
  `/worktree-spinoff` does not.
- `--idempotency-key` makes the call safe to retry on transient errors
  (network blip, disk full). Use the same key on retry and the CLI
  returns the original run without spawning twice.
- `--notify <cmd>` registers a completion hook the supervisor runs when
  the run reaches a terminal state (`done | failed | cancelled`), before
  teardown — the push signal that tells this session the spinoff finished
  without you polling. The command runs via `sh -c` with `TASKFLEET_RUN_ID`,
  `TASKFLEET_STATUS`, `TASKFLEET_SUMMARY`, `TASKFLEET_RUN_KIND`, and `TASKFLEET_RUN_TITLE` in
  its environment. It also fires for an unresolved `node.awaiting_input` after
  the grace window with `TASKFLEET_STATUS=awaiting-input`, `TASKFLEET_AWAITING_INPUT=1`,
  and the discussion array in `TASKFLEET_AWAITING_INPUT_JSON`. Delivery is
  **at-least-once**: the healthy path fires
  once, but a supervisor crash mid-fire can re-fire on restart, so write a
  command that tolerates running more than once (an idempotent file
  write / notification, not something that double-counts). Pass it **only
  if you have a real sink** the harness watches — e.g. appending a line to
  a file (`--notify 'printf "%s %s\n" "$TASKFLEET_RUN_ID" "$TASKFLEET_STATUS" >>
  ~/.taskfleet-completions'`) or a desktop toast (`--notify 'terminal-notifier
  -message "$TASKFLEET_SUMMARY"'` / `notify-send`). Without such a sink, do
  **not** promise the user a notification; use the `run wait` approach
  under "Following progress" instead. See "Reporting completion back to
  this session" below. Note the command runs in the **supervisor's**
  environment (a long-lived detached process), not your login shell — a
  desktop-toast hook may need the session's `DISPLAY` /
  `DBUS_SESSION_BUS_ADDRESS`; a file/FIFO sink is the robust choice.
- Output defaults to `--output jsonl` — one compact envelope per line.

### 4. Success envelope

```json
{
  "schema_version": 1,
  "data": {
    "run_id": "01HZ...",
    "dir": "$HOME/.taskfleet/runs/01HZ...",
    "supervisor": 12345,
    "kind": "spinoff",
    "lifecycle": "autonomous",
    "node_id": "n-...",
    "tmux_window": "<workmux-reported-window-name>",
    "worktree_path": "$HOME/repos/<repo>/worktrees/<title>",
    "branch": "wt/<title>"
  }
}
```

Read `data.run_id` — that is the handle for every follow-up
(`run show`, `node list`, `discussion list`). Read `data.supervisor` to
confirm the per-run supervisor process is alive; if it is `null` or the
field is `{"note": "..."}`, surface the note to the user and stop —
something blocked the supervisor spawn.

### 5. Report to the caller

Tell the user:

- Run id, kind (`spinoff`), source/merge branch.
- Tmux window name (so they can attach to the reported tmux session and
  select that window if curious; do not guess a session name).
- That the spinoff merges-and-reports itself via `taskfleet run
  merge` — no `/worktree-merge` handoff from them.
- How to follow progress: `taskfleet run show <run-id>` (or
  `--output jsonl` for one-line summaries).
- **How completion reaches them.** The spinoff runs out-of-band; nothing
  re-invokes this session by itself. Do **not** claim you will "let them
  know when it's done" unless you have actually wired one of the two
  mechanisms in "Reporting completion back to this session" below.
  Otherwise state plainly that they should check `run show <run-id>`, or
  ask you to wait on it.

When invoked from a driver, return the structured payload (run id, node
id, branch, tmux window) to the calling skill instead of a human
summary — the driver needs the IDs to poll completion.

## Genuine decision forks: signal, never prompt

An autonomous worker MUST NOT stop at an interactive stdin prompt. If a genuine
human decision is unavoidable, write one or more report-shaped discussion items
(`topic`, non-empty string `options`, and `recommended_default`) to JSON and
open durable run state:

```bash
taskfleet event create "$run_id" --kind node.awaiting_input --node-id n-0001 \
  --from-file /tmp/awaiting-input-${run_id}.json \
  --idempotency-key "awaiting-input:${run_id}:<short-topic>"
request_seq="$(taskfleet --output json run show "$run_id" | \
  jq -r '.data.awaiting_input_detail.event_seq')"
```

The file shape is
`{"discussion_items":[{"topic":"…","options":["…"],"recommended_default":"…"}]}`.
This makes `run show` / `run list` observable immediately; after three minutes
(unless `TASKFLEET_AWAITING_INPUT_GRACE_SECS` overrides it), `run wait` settles and a
registered `--notify` hook fires with `TASKFLEET_STATUS=awaiting-input` and
`TASKFLEET_AWAITING_INPUT_JSON`.

Do not wait indefinitely. Either (a) wait at most five minutes without opening
an interactive prompt, then emit `node.input_resolved` with
`{"event_seq":$request_seq}` and proceed on the stated recommended default:

```bash
printf '{"event_seq":%s}\n' "$request_seq" > /tmp/input-resolved-${run_id}.json
taskfleet event create "$run_id" --kind node.input_resolved --node-id n-0001 \
  --from-file /tmp/input-resolved-${run_id}.json
```

Alternatively, (b) immediately submit a terminal blocked
report with `success:false` and the same `discussion_items` via
`taskfleet node report "$run_id" n-0001 --from-file <report>`. The blocked
path preserves the branch and worktree for the human. If the fork resolves by
other evidence before the timeout, emit the same generation-fenced
`node.input_resolved` before continuing.

## Terminal report (mandatory)

A spinoff MUST take exactly one terminal path, never both. Completed,
mergeable work uses `taskfleet run merge`, which rebases + merges the
branch and submits the terminal `node report` stamped `via: "explicit-merge"`.
Work blocked by a required failed or incomplete step does **not** call `run
merge`; it submits a direct `success: false` report as specified in "Tool and
sub-workflow failure disclosure" below. Omitting both paths leaves the run
alive and the worktree dangling.

For the completed path, the brief instructs the spinoff to run the following
once the work is committed and ready to land, before its session ends:

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

2. **Write the §7.3 payload** to a temp file. These exact field names are
   what the supervisor consumes — do NOT use `discuss`,
   `spinoff_candidates`, or `wrap_up`: an unknown key still passes
   validation, but its contents are silently dropped.

   ```bash
   cat > /tmp/node-report-${run_id}.json <<'JSON'
   {
     "success": true,
     "summary": "<one-line outcome>",
     "discussion_items": [],
     "spinoff_proposals": [],
     "wrap_up_recommendations": []
   }
   JSON
   ```

   - `success` — **required** boolean. `true` when the work merged
     cleanly; `false` when reporting a blocked or failed outcome.
   - `summary` — optional one-line human-readable result.
   - `discussion_items[]` — decisions that genuinely needed a human
     call. Each: `{"topic": "<non-empty>", "severity":
     "discuss|critical|info", "options": ["…"]}`.
   - `spinoff_proposals[]` — follow-up work worth spawning. Each:
     `{"proposed_title": "<non-empty>", "proposed_kind":
     "spinoff|code|research|bugfix|technical-decision|make-skill|fan-out|orchestrated",
     "rationale": "<why>"}`.
   - `wrap_up_recommendations[]` — array of strings; advice for the
     caller (further reviews, doc updates, additional siblings).

3. **Merge and report in one call** by passing that payload to
   `run merge` via `--report-file`. The file is validated *before* the
   merge runs, and the rich §7.3 fields are carried in the same call:

   ```bash
   taskfleet run merge "$run_id" --report-file /tmp/node-report-${run_id}.json
   ```

   `run merge` defaults to node `n-0001` (a single-worker kind always
   has exactly one node), so the node id is no longer needed. A spinoff
   with **no** follow-up items (empty discussion_items / spinoff_proposals
   / wrap_up_recommendations) may skip the temp file entirely and submit a
   minimal auto-report:

   ```bash
   taskfleet run merge "$run_id"
   ```

   This rebases + merges the worktree branch into its recorded source
   branch and submits a minimal `{success, summary}` report. The call
   itself is what releases the supervisor.

   On a clean merge the supervisor winds the run down and tears down the
   worktree, tmux window, and branch automatically within a second or
   two — the agent's session ends as the window closes. Do **not** run
   `tmux kill-window`, `git worktree remove`, or `git branch -d`
   yourself, and do not re-verify or re-submit if `run show` still reads
   `pending` for a moment.

   **Conflict path:** if `run merge` exits non-zero with
   `error.code: "merge_failed"` it does **not** submit a report and the
   node stays live — resolve the conflict (or run `/complex-rebase` for
   deeply-diverged branches) and re-run `taskfleet run merge
   "$run_id" --report-file /tmp/node-report-${run_id}.json`.

A terminal report is **not optional**. Completed work with no `run merge`, or
blocked work with no direct `node report`, leaves the run dangling with no
structured outcome for the caller to read.

## Tool and sub-workflow failure disclosure

Before closing, inventory every failed or detectably incomplete tool, command,
external service, review, panel, or delegated workflow.

A step **required** by the brief or done criteria that remains failed or
incomplete always blocks this attempt. Do not call `run merge`. Write the
existing §7.3 report payload to `/tmp/node-report-${run_id}.json` with top-level
`success: false`, then submit it with `taskfleet node report "$run_id"
n-0001 --from-file /tmp/node-report-${run_id}.json` (`n-0001` is the sole node
in this single-worker run). An **optional/advisory** failure may continue only
when the deliverable is independently complete and safe; disclose it in the
full `success: true` report passed to `taskfleet run merge "$run_id"
--report-file /tmp/node-report-${run_id}.json`, never the minimal auto-report.

Requested completeness is a contract. A requested panel with a missing model
section, truncation marker, malformed output, or missing expected artifact is
incomplete, not representative consensus. Retry only when existing workflow
policy authorizes a finite bound; if none does, do not retry. Record each attempt
and its outcome, then take the required or optional path at exhaustion.

Create one aggregate `discussion_items[]` entry for the run whose `topic` starts
`Tool/sub-workflow failure —`. Cover every distinct failure, coalescing repeated
attempts of the same one: tool/workflow and purpose; expected completeness;
observed exit/error/incompleteness; attempts; affected step; whether work
continued and why safe; suggested bug surface; and a stable artifact/log path
when available. Put actionable retry/recover/accept/file steps in item-level
`options`. Keep the complete entry, including options, at most 2 KiB. Include
only a short redacted excerpt; never copy secrets, credentials, personal data,
environment dumps, or unbounded logs. Set top-level `summary` and `success` to
distinguish blocked from completed; do not put them inside the discussion item.
Existing prose fields suffice, so do not add a schema or terminal state.

## Issue Management

Skip this section in driver mode (`--parent-run-id` set). The driver
owns issue interaction.

When issue-driven and not in driver mode, resolve the issue type while building the brief:
a bug closes as `fixed`; a feature/task/improvement/chore closes as `done`. If the
repository customizes types or delivery statuses and no single valid status follows from
its schema, stop before implementation rather than guess. Put the **concrete** slug,
status, and agent identity—not the metavariables below—into this closing contract:

1. Mark work begun with `issuectl update <slug> --status in-progress --json`. Stage and
   commit only that issue metadata path, then require a clean tree before implementation.
2. Make and validate the final implementation commit. Before `run merge`, run
   `issuectl close <slug> --status <fixed-or-done> --stamp --as <agent> --json`.
3. Require `.data.stamp.status` to be `stamped` or `already_present`; `skipped` or a
   missing stamp blocks the merge. The stamp rewrites the implementation commit with
   `Fixes-Issue: @<slug>` without changing its tree.
4. Stage the exact closure metadata path returned by issuectl, commit that metadata in a
   separate commit, and require a clean tree before `taskfleet run merge`.

Do not add a `Fixes-Issue` trailer or close an issue for a freeform run. Driver mode keeps
issue interaction with the driver. The worker performs these calls itself; the spawning
skill must not race it.

## Errors

Failures print a JSON envelope to **stderr** with non-zero exit:

```json
{"schema_version": 1, "error": {"code": "<code>", "message": "..."}}
```

Always branch on `error.code`; the message is human prose.

Likely codes:

- `invalid_arguments` — missing/empty `--title` or `--task`, both
  `--task` and `--prompt-file` set, or `--parent-run-id` /
  `--parent-node-id` mismatched.
- `branch_not_found` — `--source-branch` does not exist locally. Fetch
  or correct the name; do not auto-create.
- `worktree_create_failed` — git refused (dirty working tree on the
  source branch, conflicting worktree path, locked branch). Report to
  the user; the source branch likely has uncommitted changes that must
  be committed or stashed first.
- `idempotent_replay` — informational; the `--idempotency-key` matched
  a prior run. The returned envelope describes that prior run; no new
  spawn happened.
- `supervisor_spawn_failed` — the supervisor process could not be
  started. The run dir exists but no one is driving the worker. Tell
  the user to inspect `<dir>/supervisor.stderr.log` and consider
  `taskfleet run reattach <run-id>`.

If `--dry-run` is set, the CLI validates inputs and emits a
`dry_run: true` envelope without materializing anything.

## Following progress

The spinoff runs asynchronously. To inspect status **once**:

- `taskfleet run show <run-id>` — current status, node states,
  recent events.
- `taskfleet event tail <run-id> --follow` — streaming
  event log.
- `taskfleet node list <run-id>` — per-unit detail (a
  spinoff has exactly one worker node).
- `taskfleet node show <run-id> n-0001` — the structured terminal
  report `taskfleet run merge` submits as it merges the branch (see
  "Terminal report (mandatory)").

**Completion: block with `run wait`.** To wait until the run settles,
use the binary's blocking primitive instead of a hand-rolled poll loop —
the correct backoff, the terminal set (**`done | failed | cancelled`**),
and the "branch on `manifest.status`, never `lifecycle`" rule all live
inside `run wait`:

```bash
# Block until the run reaches a terminal state (exit 0 = settled).
taskfleet run wait "$run_id"
```

`run wait` exits `0` once the run is terminal, `2` if `--timeout`
elapsed first, and `3` under `--fail-on-error` when the settled run was
`failed`/`cancelled`. Its JSON folds the terminal report `summary` in,
so you rarely need a follow-up `run show`. Pass several run-ids to block
until **all** settle (add `--any` to return on the first). This
supersedes the old `while … run show … case` snippet, which broke under
zsh word-splitting and routinely polled the wrong field.

**Settled ≠ landed — read the `landed` flag, not `merge-base --is-ancestor`.**
Both `run wait` and `run show` surface a `landed` boolean plus a `landed_method`
(`git-verified` | `report-marker` | `unverified`). Trust it as the landing
signal.

### Reading a worker report back

The terminal report is persisted: it is **not** under a projection field named
`report`. `node show` preserves the projection-native `data.last_report` and
also exposes the consumer-facing `data.report` alias. For a single-worker
spinoff, `run show` exposes the same report at `data.report`; multi-node runs
must read each worker with `node show`:

```bash
# skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
taskfleet run show "$run_id" --output json | jq '.data.report'
# Node-level projection-compatible probe:
# skill-example-ci: skip (the parser validates CLI argv, not shell pipelines)
taskfleet node show "$run_id" n-0001 --output json |
  jq '.data.report // .data.last_report'
```

`run wait` deliberately has a different envelope because it can wait for many
runs: read outcomes from `data.runs[]` (for example,
`jq '.data.runs[] | {run_id, status, summary}'`), not `data.status`. Use
`run show` or `node show` above when you need all four report fields
(`summary`, `discussion_items`, `spinoff_proposals`, and
`wrap_up_recommendations`). Do **not** git-verify a landing with
`git merge-base --is-ancestor <worker-branch> <target>`: if the caller rebased
local `main` (routine on a busy repo), the worker's merge is replayed under a new
hash while the branch ref stays put, so `--is-ancestor` returns a **false "not
landed"** even though the work is fully merged. The CLI's `landed` flag is
git-verified against the *current* target tip (patch-id equivalence plus an
ancestry net) and stays correct across that rebase; when git cannot run (the
branch was already torn down) it falls back to the durable `run merge` marker and
reports `landed_method: report-marker`. A `landed: false` with method
`unverified` means "could not confirm", not "confirmed missing" — verify by
**content on the actual target** (expected files/symbols, or the intended diff),
never by the worker branch ref, before concluding the work did not land.

## Reporting completion back to this session

A spinoff is fire-and-forget: `run create` returns immediately and the
supervisor tears the run down out-of-band, so **nothing re-invokes this
conversation when it finishes** unless you arrange it. If you told the
user "I'll tell you when it's done", wire one of these at spawn time —
otherwise you cannot deliver it:

1. **`--notify <cmd>` (push).** Registered at `run create` (see step 3),
   the supervisor runs the command once on the terminal transition with
   `TASKFLEET_RUN_ID` / `TASKFLEET_STATUS` / `TASKFLEET_SUMMARY` (and `TASKFLEET_RUN_KIND` /
   `TASKFLEET_RUN_TITLE`) in its environment. Point it at a sink your harness
   observes — a file/FIFO append the harness tails, or a desktop
   notification. Best for true fire-and-forget: no watcher process has to
   stay alive.
2. **Background `run wait` (pull, harness re-invoke).** If your harness
   re-invokes the agent when a launched background task exits, run
   `taskfleet run wait "$run_id"` as that background task at spawn
   time. The harness wakes you with its terminal summary. Only works if
   you background it **at spawn** — a fire-and-forget spinoff you never
   waited on has no watcher.

If you wire neither, be honest with the user: the run proceeds on its
own and they (or a later explicit `run wait` / `run show <run-id>`) must
check it — the run dir, its terminal `manifest.status`, and the node's
terminal report all persist after teardown, so a late `run show` still
answers.

## Install or upgrade `taskfleet`

This skill was installed for `taskfleet {{CLI_VERSION}}`. On the
first invocation in a session, run
`taskfleet version --output json`, parse the JSON, and read
`.data.version`. Compare it to `{{CLI_VERSION}}`:

- **Missing**: tell the user to install through a published distribution channel
  outside this repository workflow, then stop.

- **Older than `{{CLI_VERSION}}`**: tell the user the skill expects
  `{{CLI_VERSION}}` and suggest upgrading via the same channel they
  originally used (`brew upgrade jarimustonen/taskfleet/taskfleet` or
  re-run the shell installer). Stop and wait — the `run create --kind spinoff` flag
  surface may have changed.
- **Newer than `{{CLI_VERSION}}`**: tell the user the installed skill is
  stale and stop. Refreshing installed bundled instructions is published-tool
  maintenance outside repository work; never run `skill install` as part of
  this workflow.
- **Equal**: proceed normally.

## Examples

```
# Freeform spinoff
/worktree-spinoff Process receipts batch 2026-05 with vision OCR

# Issue-driven (skill reads issue NN, builds task brief from it)
/worktree-spinoff #142

# Driver mode — /fan-out passes these
taskfleet run create --kind spinoff \
  --title "u-003-receipts" \
  --task "..." \
  --source-branch fan-out/2026-05 \
  --parent-run-id 01HZ... \
  --parent-node-id n-0001
```
