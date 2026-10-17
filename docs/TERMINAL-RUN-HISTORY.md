# Terminal run history

Taskfleet retains run history independently of worker processes, tmux displays,
Git branches and worktrees. A terminal run remains discoverable with
`taskfleet run list --json` and inspectable with
`taskfleet run show <full-run-id> --json` after teardown and supervisor restart.
This is the existing file-backed history contract, not a second archive or a
new retention service.

## Consumer reads

Read commands use the selected state root (`TASKFLEET_HOME`, normally
`~/.taskfleet`). Unfiltered list includes terminal runs; a status, kind or repo
filter can intentionally exclude them. Keep the full run ID, not the branch's
short display fragment. Inspect envelope warnings and command failures: an
unreadable record is not proof that a run never existed.

| Fact | Existing JSON location |
| --- | --- |
| Identity, status, lifecycle, attention, creator associations, source repository/branch, created time | `data.runs[]` in list; `data` in show |
| Last run update time | show `data.manifest.updated_at` |
| Default worker's terminal result/failure summary | show `data.report` (`summary`, `reason` when supplied) |
| Landing verdict and verification method | show `data.landed`, `data.landed_method` |
| Currently preserved Git resources | show `data.preserved_work[]` |
| Per-node transcript discovery | list row/show `node_transcripts[]` |
| Any node's report and node timestamps | `taskfleet node show <run-id> <node-id> --json`, `data.report` (also `last_report`), `started_at`, `updated_at` |

List is an inventory, not a copy of show's detailed report/landing/preserved-work
fields. Fetch show for a selected run, and node show for multi-node results.
Missing fields on old records remain missing; Taskfleet does not reconstruct
historic facts from branch names or timestamps.

`status` is progress; `lifecycle` remains `autonomous` or `interactive` even
after completion. A missing supervisor PID or tmux window is not failure.
An interactive worker's exit alone does not complete the run: its parent must
explicitly merge or cancel. A failed/cancelled run can retain unmerged work;
`preserved_work` is a current filesystem/Git inventory, not a historical promise
that the recorded path still exists. An explicit merge can remove the branch
and checkout without removing its result.

Creator associations survive terminalization unchanged. Use
`created_by_session` to group direct browser-conversation spinoffs and
`created_by_run_id` for nested workers, without a host-side association store.
Neither grants control or ownership. See [Creator attribution](CREATOR-ATTRIBUTION.md).

### Git details are native reads

Do not copy diffstats, file lists or commit counts into a host history store.
Read them from the source Git repository on demand using verified commit OIDs.
The `merge.started` event retains `expected_source_oid`, `worker_oid`,
`base_sha` and the source branch. For ordinary Taskfleet-owned merges, the native
producer writes `merge.native_prepared` with the exact post-rebase
`integrated_oid` before moving the source ref, while holding the Git merge lock
and checking the recorded source fence. That event alone is **not** a landing:
a crash can still prevent the ref move.

A successful native merge's retained report carries `origin.op_id`,
`origin.worker_oid` (the **pre-rebase** tip), and optional
`origin.integrated_oid` (the exact integrated post-rebase tip). Read the report
at show `data.report.origin`, or node show `data.report.origin`; match its
`op_id` to `merge.started` through `event tail <run-id>`. After verifying that
both objects still exist as commits in the recorded source repository, use
`expected_source_oid..integrated_oid` for the commit count, diffstat and file
list. Do not use `base_sha` as that range's source boundary: concurrent source
work may have landed since the worker forked.

Crash recovery retains the exact native OID only when the prepared event is
uniquely bound to that transaction and Git verifies the expected source is an
ancestor of it and it is an ancestor of the moved source. This optional evidence
does not change completion authority, fencing or teardown policy. Caller-owned
`merge.rebased` remains a separate intent-bound settlement event; it is not an
ordinary native producer stream, and caller reports do not currently expose
`origin.integrated_oid`.

The optional field is omitted for legacy reports, caller-owned results and
unverifiable recovery evidence. Never substitute `worker_oid` or today's source
tip when it is absent. A report does **not** guarantee a `merge_commit` field.
Externally pruned Git objects can make Git detail unavailable even when the
retained report still names their OIDs. `landed` is the supported landing verdict,
not a historical source-tip lookup.

## Three different resource lifetimes

- **Run state:** `<state-root>/runs/<run-id>/manifest.json`, append-only
  `events.jsonl` and `nodes/*.json` retain the run and terminal reports.
- **Worker evidence:** `evidence/<node-id>/` in that run retains the original
  native transcript, resume copy, final pane and terminal report when captured.
  Evidence capture precedes teardown; a failed capture vetoes cleanup and is
  retried from durable state.
- **Display/Git resources:** the supervisor may remove a merged worktree and
  branch and close worker panes. Persistent sessions optionally retain bounded
  inert displays. Their TTL/count expiry is only display maintenance, not
  history expiry. Non-merge cleanup preserves source-relative unmerged work.

`taskfleet session maintain --json` can expire only an exact owned inert pane:
recorded socket, server PID/start identity, session, window, pane and ownership
markers must match. Active panes, ownership drift, shared windows and the last
session anchor are preserved. A missing server is recorded without starting one.
The command can append `worker.display.expired` and update projections; it
cannot delete manifests, events, reports, evidence or preserved Git work.
Run-state retention has no display TTL/count limit. This is not protection
against an operator deleting the state root or filesystem loss; back it up if
needed. `run discard --force` is separate explicit Git-resource disposal, not a
history expiration command.

## Transcripts after teardown

Use [Run transcripts](RUN-TRANSCRIPTS.md), not Pi's default session directory.
Prefer the available original archive once advertised. Open paths relative to
the specified anchor and verify native identity/header and containment.
The original is immutable captured bytes, including the original checkout cwd;
the resume copy's rewritten cwd makes it unsuitable as the original transcript.

A supported Pi transcript can genuinely be `missing`, `not-recorded` or
`unavailable`; these differ from `unsupported` (currently including Claude).
A complete capture is not proof that an externally deleted file still exists.
An unsupported transcript does not hide a retained run or its report. Discovery
is read-only and never confers continuation or settlement authority.

## Regression evidence

- `tests/e2e_spinoff.rs`: the native merged round trip and terminal-history
  variants force a real hash-changing native rebase, verify the exact Git range
  after teardown/restart and later source advancement, and cover failed/cancelled preserved
  commits. They read list/show after worker/window/worktree cleanup, re-enter
  the supervisor from terminal state, and re-read the retained report,
  attribution and transcript discovery. Native spawn uses real disposable Git,
  declared worker/tmux/workmux doubles and the Linux user-manager fixture.
  `failed_supervised_run_merge_recovers_done_and_tears_down` separately covers
  a failed run's recovery and supervisor reattachment.
- `run::merge::native_evidence_tests`: real native producer crash windows before
  and after the ref move, with Git-only PATH; recovery preserves the exact rebased
  OID after later source advancement and supervisor Git teardown. Prepared
  evidence alone cannot complete an unlanded merge. `merge_recovery` tests cover
  absent, duplicate, mismatched, malformed and missing-object native evidence;
  core report tests cover backward decoding without the optional field.
- `tests/session_maintenance.rs::display_expiry_keeps_durable_history_and_evidence`:
  deterministic public-command expiry with a strict tmux double and stripped
  PATH, repeated twice; only the owned inert pane is removed. Manifest identity,
  append-only event prefix, node report and every evidence file survive;
  active/foreign/shared/last-anchor displays are left alone.
- `src/session.rs` tests
  `real_private_tmux_expiry_preserves_unrelated_split_and_is_idempotent` and
  `real_retention_archives_before_killing_only_owned_pane_and_keeps_active_worker`
  exercise the real private tmux boundary when tmux is available. The
  deterministic test above does not depend on that optional executable.
- `src/supervise/evidence.rs::archives_exact_transcript_and_header_only_resume_copy`
  already proves original bytes and identity survive checkout removal and
  restarted capture, with only the resume header rewritten.
  `failed_capture_is_durable_and_a_later_retry_can_complete` proves retry.
- `tests/run_transcripts.rs` already distinguishes unsupported/legacy, missing
  and unusable evidence, locks multi-node reads, and retains a list history row
  with a warning when node inventory is unreadable.

These tests establish Taskfleet history retention, not browser integration or
survival of a real service-parent restart. The latter has its own bounded smoke
and prerequisites in [Linux lifetime](LINUX-LIFETIME.md).
