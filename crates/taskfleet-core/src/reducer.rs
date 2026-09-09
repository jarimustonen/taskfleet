//! Event → projection reducer (design.md §1.4).
//!
//! Each event mutates zero or more projection files. Unknown kinds are
//! ignored for forward compatibility. The reducer expects to run under the
//! per-run `flock`.
//!
//! Idempotency contract (per design.md §7.3 at-least-once delivery):
//! `*.created` reducers short-circuit when their projection file already
//! exists; status/resolution reducers are no-ops once the terminal state
//! has been reached. Replaying the same event stream against existing
//! projections is therefore a clean no-op-or-apply.
//!
//! This idempotence is load-bearing for the `applied_seq` watermark
//! (append-then-apply atomicity; see [`crate::schema::Manifest::applied_seq`]
//! and [`crate::events::append_and_apply_event`]). Of the two options the spec
//! offered — make the reducer idempotent, OR have the writer skip events
//! already reflected in the projection — we chose **idempotent reducer**: the
//! existence/terminal guards already present here mean the catch-up replay can
//! re-fold *any* tail event (one whose projection landed before a crash, or one
//! whose projection did not) with the same no-op-or-apply outcome, so the
//! writer needs no per-event "already applied?" probe. The watermark advances
//! only after an event's projections are fsynced, so it can lag the projections
//! but never lead them.
//!
//! ## Manifest counters are derived, not folded
//!
//! The reducers here deliberately do **not** touch the manifest's denormalized
//! `node_count` counter. It is
//! recomputed from the projection directories by
//! [`derive_counters`](crate::projections), invoked from
//! [`advance_applied_seq`](crate::events) at the
//! watermark advance. An earlier design incremented/decremented them inside
//! these reducers, but a crash between a projection write and the follow-on
//! `manifest.json` write could permanently desync them: the replay re-folded
//! the event, hit the `*.created`/terminal idempotency guard above, and skipped
//! the counter mutation that never actually landed. Deriving the counts makes
//! drift impossible — there is no delta to lose. See issue
//! `manifest-counter-desync`. A count-affecting reducer still emits its manifest
//! op to refresh `updated_at`; the counter fields it carries are overwritten by
//! the derive step.
//!
//! Because a count-affecting event rewrites a projection file *and* the
//! manifest counter under the same exclusive `flock`, a reader that scans both
//! together must hold the shared `flock` (`LOCK_SH`) for the whole scan or it
//! could see the projection change without the matching counter (or vice
//! versa). See [`crate::projections`] and design.md §4.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::paths::RunPaths;
use crate::projections::{read_manifest_opt, read_node_opt, write_manifest, write_node};
use crate::report::ReportOrigin;
use crate::schema::{
    ChildRef, Event, EvidenceStatus, IdValidationError, Kind, Lifecycle, Manifest, MergeTxn, Node,
    NodeId, RunId, Status, TmuxIdentity, WorkerEvidence, WorkerExit, STATE_SCHEMA_VERSION,
};

/// Map an id-validation failure on an event-sourced id to a [`CorruptEventLog`]
/// error. An id that fails to parse here came off `events.jsonl` (or a forged
/// event), so the log — not the caller — is the corrupt party.
///
/// [`CorruptEventLog`]: Error::CorruptEventLog
fn corrupt_id(events_path: &Path, ev: &Event, e: &IdValidationError) -> Error {
    Error::CorruptEventLog {
        path: events_path.to_path_buf(),
        reason: format!("event seq={} kind={}: {e}", ev.seq, ev.kind),
    }
}

/// Parse an optional `RunId` from event-data field `field`: missing/null →
/// `None`; a JSON string → validated `Some(RunId)`; a malformed id or a
/// non-string value → [`Error::CorruptEventLog`].
fn opt_run_id(events_path: &Path, ev: &Event, d: &Value, field: &str) -> Result<Option<RunId>> {
    match d.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => RunId::parse_str(s)
            .map(Some)
            .map_err(|e| corrupt_id(events_path, ev, &e)),
        Some(_) => Err(Error::CorruptEventLog {
            path: events_path.to_path_buf(),
            reason: format!(
                "event seq={} kind={} `{field}` must be a JSON string or null",
                ev.seq, ev.kind
            ),
        }),
    }
}

/// Parse an optional `NodeId` from event-data field `field`. See [`opt_run_id`].
fn opt_node_id(events_path: &Path, ev: &Event, d: &Value, field: &str) -> Result<Option<NodeId>> {
    match d.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => NodeId::parse_str(s)
            .map(Some)
            .map_err(|e| corrupt_id(events_path, ev, &e)),
        Some(_) => Err(Error::CorruptEventLog {
            path: events_path.to_path_buf(),
            reason: format!(
                "event seq={} kind={} `{field}` must be a JSON string or null",
                ev.seq, ev.kind
            ),
        }),
    }
}

/// Parse a `kind` value from event `data` for a NEW append, failing closed on
/// anything not a live, creatable kind.
///
/// `Kind`'s `#[serde(other)]` catch-all means every unrecognized string —
/// a removed kind (`code`, `orchestrate`, …), a typo, or a future kind —
/// deserializes to [`Kind::Unknown`] rather than erroring. That read-only
/// catch-all exists so `run list` / `doctor` can decode a legacy on-disk run
/// (ADR §D7); it must NOT let a garbage `kind` slip through the append gate as
/// though it were valid. Mapping `Unknown` back to `None` keeps the reducer's
/// `run.created` / `node.created` / `child.spawned` validation fail-closed, as
/// it was before the 0.2 cut added the catch-all. (Legacy runs are never
/// re-created through this path — their manifest/nodes already exist on disk and
/// are read directly, not replayed from a fresh `*.created`.)
fn data_kind(v: &Value) -> Option<Kind> {
    match serde_json::from_value::<Kind>(v.clone()) {
        Ok(Kind::Unknown) | Err(_) => None,
        Ok(k) => Some(k),
    }
}

fn data_status(v: &Value) -> Option<Status> {
    serde_json::from_value(v.clone()).ok()
}

fn require_status(ev: &Event, path: PathBuf) -> Result<Status> {
    data_status(ev.data.get("status").unwrap_or(&Value::Null)).ok_or_else(|| {
        Error::CorruptEventLog {
            path,
            reason: format!("{} missing/invalid `status`", ev.kind),
        }
    })
}

fn want_str<'a>(events_path: &Path, ev: &Event, d: &'a Value, field: &str) -> Result<&'a str> {
    d.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::CorruptEventLog {
            path: events_path.to_path_buf(),
            reason: format!(
                "event seq={} kind={} missing `{field}` string field",
                ev.seq, ev.kind
            ),
        })
}

/// Read an optional boolean field with strict typing: missing/null → `None`,
/// JSON bool → `Some(b)`, anything else → `CorruptEventLog`. Mirrors
/// [`optional_str`] / [`optional_i32`]; prevents a non-boolean `success` /
/// `cancelled` from being silently coerced to `false` and bypassing the
/// success-XOR-cancelled invariant.
fn optional_bool(events_path: &Path, ev: &Event, d: &Value, field: &str) -> Result<Option<bool>> {
    match d.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(Error::CorruptEventLog {
            path: events_path.to_path_buf(),
            reason: format!(
                "event seq={} kind={} `{field}` must be a JSON boolean or null",
                ev.seq, ev.kind
            ),
        }),
    }
}

fn optional_i32(d: &Value, field: &str, events_path: &Path, ev: &Event) -> Result<Option<i32>> {
    match d.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let raw = v.as_i64().ok_or_else(|| Error::CorruptEventLog {
                path: events_path.to_path_buf(),
                reason: format!(
                    "event seq={} kind={} `{field}` must be integer",
                    ev.seq, ev.kind
                ),
            })?;
            i32::try_from(raw)
                .map(Some)
                .map_err(|_| Error::CorruptEventLog {
                    path: events_path.to_path_buf(),
                    reason: format!(
                        "event seq={} kind={} `{field}` out of i32 range: {raw}",
                        ev.seq, ev.kind
                    ),
                })
        }
    }
}

fn optional_ts(
    d: &Value,
    field: &str,
    events_path: &Path,
    ev: &Event,
) -> Result<Option<DateTime<Utc>>> {
    match d.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => DateTime::parse_from_rfc3339(s)
            .map(|dt| Some(dt.with_timezone(&Utc)))
            .map_err(|_| Error::CorruptEventLog {
                path: events_path.to_path_buf(),
                reason: format!(
                    "event seq={} kind={} `{field}` not RFC3339",
                    ev.seq, ev.kind
                ),
            }),
        Some(_) => Err(Error::CorruptEventLog {
            path: events_path.to_path_buf(),
            reason: format!(
                "event seq={} kind={} `{field}` must be RFC3339 string or null",
                ev.seq, ev.kind
            ),
        }),
    }
}

/// A projection write planned by [`reduce_event_to_ops`] and performed by
/// [`commit_ops`].
///
/// Splitting the reducer into a pure *plan* phase (compute these ops from the
/// current projection state, validating as it goes) and a *commit* phase
/// (write them) means a single branch per kind implements both the pre-append
/// validation gate and the post-append apply — there is no validate/apply
/// mirror to drift out of lockstep, and the projection state is read once
/// rather than twice.
pub(crate) enum ProjectionOp {
    /// Write the run manifest.
    Manifest(Manifest),
    /// Write a node projection.
    Node(Node),
}

/// Commit a planned batch of projection writes, in order.
///
/// Caller must hold the run's [`crate::lock::RunLock`]. Pairs with
/// [`reduce_event_to_ops`]: the ops were computed against the same locked
/// state, and nothing mutates the projections between the plan and this commit
/// (in the append path only `events.jsonl` is written in between), so the
/// planned writes are still valid.
pub(crate) fn commit_ops(paths: &RunPaths, ops: Vec<ProjectionOp>) -> Result<()> {
    for op in ops {
        match op {
            ProjectionOp::Manifest(m) => write_manifest(paths, &m)?,
            ProjectionOp::Node(n) => write_node(paths, &n)?,
        }
    }
    Ok(())
}

/// Plan the projection writes one event implies, *without* performing them.
///
/// This is the single source of truth for both validation and application: it
/// reads the current projection state, enforces every event-payload invariant
/// (returning [`Error::CorruptEventLog`] for a malformed or cross-run event),
/// and returns the exact [`ProjectionOp`]s to commit (empty for a no-op or an
/// unknown `kind`). Because it never writes, it is also the transactional gate
/// run *before* the durable append in
/// [`crate::events::append_and_apply_unlocked`]: a reducer-rejected event is
/// caught here and never reaches `events.jsonl`, so a later replay /
/// `rebuild_projections` can't trip over a poison line. The state-dependent
/// no-op guards live here too (a settled node/run/discussion swallows a late
/// or even malformed event as a clean no-op rather than erroring).
///
/// Caller must hold the run's [`crate::lock::RunLock`].
pub(crate) fn reduce_event_to_ops(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    // An event whose envelope `run_id` doesn't match the run we're folding it
    // into means the log was copied/misrouted — folding it would silently
    // cross-contaminate projections. Reject before planning anything.
    if ev.run_id != paths.run_id {
        return Err(Error::CorruptEventLog {
            path: paths.events(),
            reason: format!(
                "event seq={} envelope run_id {:?} does not match run {:?}",
                ev.seq,
                ev.run_id.as_str(),
                paths.run_id.as_str()
            ),
        });
    }
    // Each event kind is listed explicitly as documentation of the known set;
    // `supervisor.exited` and the `_` fallthrough share a body intentionally.
    #[allow(clippy::match_same_arms)]
    match ev.kind.as_str() {
        "run.created" => reduce_run_created(paths, ev),
        "run.status" => reduce_run_status(paths, ev),
        "node.created" => reduce_node_created(paths, ev),
        "node.status" => reduce_node_status(paths, ev),
        "node.report" => reduce_node_report(paths, ev),
        "node.retry" => reduce_node_retry(paths, ev),
        "worker.exited" => reduce_worker_exited(paths, ev),
        "worker.evidence.archived" => reduce_worker_evidence_archived(paths, ev),
        "worker.evidence.failed" => reduce_worker_evidence_failed(paths, ev),
        "worker.display.retained" => reduce_worker_display_retained(paths, ev),
        "worker.display.expired" => reduce_worker_display_expired(paths, ev),
        "worker.display.unavailable" => reduce_worker_display_unavailable(paths, ev),
        "node.death_observed" => reduce_node_death_observed(paths, ev),
        "node.awaiting_input" => reduce_node_awaiting_input(paths, ev),
        "node.input_resolved" => reduce_node_input_resolved(paths, ev),
        KIND_MERGE_STARTED => reduce_merge_started(paths, ev),
        KIND_MERGE_ABORTED => reduce_merge_aborted(paths, ev),
        "child.spawned" => reduce_child_spawned(paths, ev),
        "supervisor.attached" => reduce_supervisor_attached(paths, ev),
        "supervisor.cursor_advanced" => reduce_supervisor_cursor_advanced(paths, ev),
        "supervisor.exited" => Ok(vec![]),
        // Append-only audit records from `/orchestrate` (decision log +
        // pakkopysäytys). They mutate no projection — the event log is their
        // canonical home — so they fold to a clean no-op. Listed explicitly
        // (rather than relying on the `_` fallthrough) so the append path's
        // transactional gate runs the same no-op plan for them and the intent
        // is documented at the match site. They are NOT `node.report`, so the
        // supervisor never mistakes them for a terminal signal.
        "orchestrator.decision" | "discuss.critical" => Ok(vec![]),
        // At-most-once marker the supervisor appends the first time a run is
        // observed terminal, gating the `run create --notify` completion hook so
        // a restart never re-fires it (issue `no-completion-notification-to-parent`).
        // Mutates no projection — the event log is its only home — so it folds to
        // a clean no-op. Listed explicitly so the append path's transactional gate
        // runs the same no-op plan and the intent is documented here.
        "run.notified" | "run.awaiting_input_notified" => Ok(vec![]),
        // Best-effort teardown audit records from the supervisor's cleanup
        // path. Each mutates no projection — the event log is their only home —
        // so they fold to a clean no-op. Listed explicitly so the append path's
        // transactional gate runs the same no-op plan and the intent is
        // documented here.
        //   - `cleanup.window_missing`: the node's tmux window could not be
        //     located to close it (typically a manually-resolved rebase renamed
        //     the window — issue `worktree-merge-orphans-tmux-window`).
        //   - `cleanup.worktree_missing`: the worktree dir was already gone at
        //     teardown (e.g. removed manually), so nothing to `worktree remove`.
        //   - `cleanup.branch_remove_failed`: `git branch -{d,D}` refused (e.g.
        //     unmerged commits, or the branch is already gone); the run completes
        //     anyway (issue `supervisor-worktree-remove-no-force`).
        //   - `cleanup.branch_preserved`: a BLOCKED terminal report
        //     (`success: false`, no explicit merge) intentionally left the branch
        //     and worktree in place for the human to pick up, instead of tearing
        //     them down (issue `blocked-report-deletes-branch`).
        //   - `cleanup.session_killed`: the run's managed `--headless` tmux
        //     session was torn down once its last managed window was gone, so an
        //     empty session is not left behind (issue
        //     `headless-tmux-session-not-torn-down`).
        //   - `cleanup.session_retained`: the same teardown was skipped because a
        //     human had attached to the session — never yanked out from under
        //     them.
        "cleanup.window_missing"
        | "cleanup.worktree_missing"
        | "cleanup.branch_remove_failed"
        | "cleanup.branch_preserved"
        | "cleanup.discard_authorized"
        | "cleanup.session_killed"
        | "cleanup.session_retained" => Ok(vec![]),
        // Data-integrity audit record: the supervisor found a persisted child
        // run id (in `supervisor.state.json`'s `spawned_children`) that fails
        // `RunId` structural validation and quarantined it — a corrupt id that
        // would otherwise resolve with `.ok()` and be silently skipped every
        // tick, indistinguishable from a child that completed and was torn down
        // (issue `wildly-glorious-food`). It mutates no projection — the event
        // log is its only home — so it folds to a clean no-op. Listed
        // explicitly so the append path's transactional gate runs the same
        // no-op plan and the intent is documented here.
        "supervisor.child_id_quarantined" => Ok(vec![]),
        _ => Ok(vec![]),
    }
}

/// The projection file [`commit_ops`] writes for `op`, keyed exactly as the
/// `write_*` helpers key it internally. Shared by [`plan_projections`] (which
/// reports the path) and conceptually by [`commit_ops`] (which writes it), so
/// the enumerated path list can never name a different file than the one the
/// reducer actually fsyncs.
fn op_path(paths: &RunPaths, op: &ProjectionOp) -> PathBuf {
    match op {
        ProjectionOp::Manifest(_) => paths.manifest(),
        ProjectionOp::Node(n) => paths.node(&n.node_id),
    }
}

/// Enumerate the projection files the reducer would write for `event`, in the
/// order `commit_ops` would write them, *without* performing any write.
///
/// This is the single source of truth that ends the CLI/reducer divergence the
/// `projected-paths-into-reducer` issue describes: rather than a hand-maintained
/// list in `taskfleet` that drifts whenever a new projection is added, both the
/// reducer and a caller's preflight (`event create --dry-run`) read the *same*
/// `reduce_event_to_ops` plan. This function maps that plan to file paths;
/// `apply_event` commits it. A new projection added to a reducer arm is
/// therefore reflected here automatically.
///
/// Because it runs the real reducer plan against current projection state, the
/// result is exact, not a guess: a state-dependent no-op (a settled node, an
/// already-created projection, a terminal-guarded transition) yields an empty
/// list — precisely the files `apply_event` would touch, which is none. A
/// malformed-payload event surfaces the same [`Error::CorruptEventLog`] the
/// real apply would, so a dry-run preflight cannot report success for an event
/// the write path would reject.
///
/// Caller should hold the run's [`crate::lock::RunLock`] for a snapshot
/// consistent with a concurrent reducer; a lock-free read is best-effort.
pub fn plan_projections(paths: &RunPaths, event: &Event) -> Result<Vec<PathBuf>> {
    let ops = reduce_event_to_ops(paths, event)?;
    Ok(ops.iter().map(|op| op_path(paths, op)).collect())
}

/// Apply one event to projections: plan via [`reduce_event_to_ops`], then
/// [`commit_ops`]. No-op for unknown `kind`. Caller must hold the run's
/// [`crate::lock::RunLock`].
///
/// Shares the one [`reduce_event_to_ops`] plan with [`plan_projections`]: the
/// paths that function reports are exactly the files this one fsyncs, because
/// both consume the same `ProjectionOp` vector (this commits it; that maps it to
/// paths via [`op_path`]).
///
/// `pub(crate)`: applying an event in isolation (without the matching
/// `events.jsonl` append) is an internal building block used by `cancel` (to
/// re-fold a crash-stranded event) and a future `rebuild_projections_from_events`.
/// External callers mutate state through
/// [`crate::events::append_and_apply_event`] so the log and projections can
/// never diverge.
pub(crate) fn apply_event(paths: &RunPaths, ev: &Event) -> Result<()> {
    let ops = reduce_event_to_ops(paths, ev)?;
    commit_ops(paths, ops)
}

/// Validate one persisted event through the normal reducer plan without
/// committing projection writes. It performs the same checks as projection
/// application and then discards the planned writes. Migration preflight uses
/// this under the run's exclusive lock so it never invents a second
/// event-payload validator.
pub fn validate_event(paths: &RunPaths, ev: &Event) -> Result<()> {
    reduce_event_to_ops(paths, ev).map(|_| ())
}

/// The envelope `node_id` that a `node.*` event must carry, with the same
/// `CorruptEventLog` message `apply_*` produces. Shared by validate/apply so
/// the missing-id check can't drift between them.
fn require_envelope_node_id(events_path: &Path, ev: &Event) -> Result<NodeId> {
    ev.node_id.clone().ok_or_else(|| Error::CorruptEventLog {
        path: events_path.to_path_buf(),
        reason: format!(
            "event seq={} kind={} missing top-level `node_id`",
            ev.seq, ev.kind
        ),
    })
}

fn reduce_run_created(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    // Idempotent: a replayed `run.created` against an existing manifest is a
    // no-op (but validates that `run_id` matches; otherwise the event log
    // is being applied to the wrong run).
    if let Some(existing) = read_manifest_opt(paths)? {
        if existing.run_id != ev.run_id {
            return Err(Error::CorruptEventLog {
                path: paths.manifest(),
                reason: format!(
                    "run.created run_id={} conflicts with existing manifest run_id={}",
                    ev.run_id, existing.run_id
                ),
            });
        }
        return Ok(vec![]);
    }
    let events_path = paths.events();
    let d = &ev.data;
    let kind =
        data_kind(d.get("kind").unwrap_or(&Value::Null)).ok_or_else(|| Error::CorruptEventLog {
            path: events_path.clone(),
            reason: "run.created missing/invalid `kind`".into(),
        })?;
    let lifecycle: Lifecycle = serde_json::from_value(
        d.get("lifecycle").cloned().unwrap_or(Value::Null),
    )
    .map_err(|_| Error::CorruptEventLog {
        path: events_path.clone(),
        reason: "run.created missing/invalid `lifecycle`".into(),
    })?;
    let title = want_str(&events_path, ev, d, "title")?.to_string();
    let agent_selection: Option<crate::schema::AgentSelection> = d
        .get("agent_selection")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| Error::CorruptEventLog {
            path: events_path.clone(),
            reason: format!("run.created invalid `agent_selection`: {e}"),
        })?;
    if let Some(selection) = &agent_selection {
        selection
            .validate()
            .map_err(|reason| Error::CorruptEventLog {
                path: events_path.clone(),
                reason: format!("run.created invalid `agent_selection`: {reason}"),
            })?;
    }
    let m = Manifest {
        schema_version: STATE_SCHEMA_VERSION,
        // Created at the watermark floor; the append path advances it to this
        // event's `seq` (after the manifest is fsynced) in `advance_applied_seq`.
        applied_seq: 0,
        // `run_id == paths.run_id` was verified at `reduce_event_to_ops` entry.
        run_id: paths.run_id.clone(),
        kind,
        lifecycle,
        title,
        status: Status::Pending,
        created_at: ev.ts,
        updated_at: ev.ts,
        source_repo: d
            .get("source_repo")
            .and_then(Value::as_str)
            .map(str::to_string),
        source_branch: d
            .get("source_branch")
            .and_then(Value::as_str)
            .map(str::to_string),
        worktree_root: d
            .get("worktree_root")
            .and_then(Value::as_str)
            .map(str::to_string),
        managed_tmux_session: d
            .get("managed_tmux_session")
            .and_then(Value::as_str)
            .map(str::to_string),
        tmux_retention: retention_policy_from_data(&events_path, d)?,
        notify_cmd: d
            .get("notify_cmd")
            .and_then(Value::as_str)
            .map(str::to_string),
        harness: d.get("harness").and_then(Value::as_str).map(str::to_string),
        agent_selection,
        node_count: 0,
        parent_run_id: opt_run_id(&events_path, ev, d, "parent_run_id")?,
        parent_node_id: opt_node_id(&events_path, ev, d, "parent_node_id")?,
    };
    Ok(vec![ProjectionOp::Manifest(m)])
}

fn reduce_run_status(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    let mut m = match read_manifest_opt(paths)? {
        Some(m) => m,
        None => return Ok(vec![]),
    };
    let new_status = require_status(ev, paths.events())?;
    // Terminal-state guard with one narrow recovery: Failed -> Done is allowed
    // only when this event names an authoritative merge report that was already
    // adopted by the log-derived node fold before this event, every own node is
    // now Done, and the supervisor attests that every linked child is Done.
    // Cancelled (and every other terminal transition) remains immutable.
    if m.status.is_terminal() {
        let recovery_seq = ev
            .data
            .get("recovery_merge_report_seq")
            .and_then(Value::as_u64);
        let children_successful =
            ev.data.get("children_successful").and_then(Value::as_bool) == Some(true);
        let recovery_allowed = m.status == Status::Failed
            && new_status == Status::Done
            && children_successful
            && recovery_seq.is_some_and(|wanted| {
                crate::cancel::read_node_status_facts(paths, Some(ev.seq))
                    .ok()
                    .filter(|facts| {
                        crate::aggregate_terminal_status(facts.iter().map(|fact| fact.status))
                            == Some(Status::Done)
                    })
                    .is_some_and(|facts| {
                        facts
                            .iter()
                            .any(|fact| fact.confirmed_merge_seq == Some(wanted))
                    })
            });
        if !recovery_allowed {
            trace_terminal_noop(ev, m.status, new_status);
            return Ok(vec![]);
        }
    }
    if m.status == new_status {
        return Ok(vec![]);
    }
    m.status = new_status;
    m.updated_at = ev.ts;
    Ok(vec![ProjectionOp::Manifest(m)])
}

/// Reconstruct the fully-qualified tmux identity from `node.created` event
/// data. Returns `Some` only when both `tmux_session` and `tmux_window_id` are
/// present and non-empty — the minimum needed to match a window. `tmux_socket`
/// is optional (a default-socket spawn may emit null); an empty socket is
/// normalized to `None` so the watchdog never invokes `tmux -S ""`.
/// `tmux_pane_id` is likewise optional (create.sh predating it emits nothing);
/// agent-log capture falls back to the window's active pane when absent. Legacy
/// events from a create.sh that predates the qualified fields (or that emit a
/// partial/empty identity) yield `None`, so the node falls back to bare-name
/// matching on `tmux_window`.
fn retention_policy_from_data(
    events_path: &Path,
    data: &Value,
) -> Result<Option<Box<crate::schema::TmuxRetentionPolicy>>> {
    let Some(value) = data.get("tmux_retention") else {
        return Ok(None);
    };
    let policy: crate::schema::TmuxRetentionPolicy = serde_json::from_value(value.clone())
        .map_err(|e| Error::CorruptEventLog {
            path: events_path.to_path_buf(),
            reason: format!("run.created invalid `tmux_retention`: {e}"),
        })?;
    if !policy.persistent
        || policy.completed_window_ttl_secs == 0
        || policy.completed_window_max == 0
    {
        return Err(Error::CorruptEventLog {
            path: events_path.to_path_buf(),
            reason: "run.created tmux_retention must be persistent with positive ttl/max".into(),
        });
    }
    Ok(Some(Box::new(policy)))
}

fn tmux_identity_from_data(d: &Value) -> Option<TmuxIdentity> {
    let nonempty = |key| {
        d.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let session = nonempty("tmux_session")?;
    let window_id = nonempty("tmux_window_id")?;
    Some(TmuxIdentity {
        socket: nonempty("tmux_socket"),
        session,
        window_id,
        // Optional: create.sh predating the field (or a failed pane query)
        // emits no `tmux_pane_id`; capture then falls back to `window_id`.
        pane_id: nonempty("tmux_pane_id"),
        server_pid: d
            .get("tmux_server_pid")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok()),
        server_pid_start_secs: d.get("tmux_server_pid_start_secs").and_then(Value::as_u64),
        server_marker: nonempty("tmux_server_marker"),
    })
}

fn worker_evidence_from_spawn_data(
    events_path: &Path,
    ev: &Event,
    data: &Value,
) -> Result<Option<WorkerEvidence>> {
    let Some(session_id) = data.get("pi_session_id") else {
        if data.get("pi_session_path").is_some() || data.get("pi_session_cwd").is_some() {
            return Err(Error::CorruptEventLog {
                path: events_path.to_path_buf(),
                reason: format!(
                    "event seq={} Pi evidence fields must be all-or-none",
                    ev.seq
                ),
            });
        }
        return Ok(None);
    };
    if session_id.is_null() {
        if data
            .get("pi_session_path")
            .is_some_and(|value| !value.is_null())
            || data
                .get("pi_session_cwd")
                .is_some_and(|value| !value.is_null())
        {
            return Err(Error::CorruptEventLog {
                path: events_path.to_path_buf(),
                reason: format!(
                    "event seq={} Pi evidence fields must be all-or-none",
                    ev.seq
                ),
            });
        }
        return Ok(None);
    }
    let session_id = session_id
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| Error::CorruptEventLog {
            path: events_path.to_path_buf(),
            reason: format!(
                "event seq={} pi_session_id must be a non-empty string",
                ev.seq
            ),
        })?;
    if session_id.len() != 36
        || !session_id.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
    {
        return Err(Error::CorruptEventLog {
            path: events_path.to_path_buf(),
            reason: format!("event seq={} pi_session_id must be a UUID", ev.seq),
        });
    }
    let original_cwd = want_str(events_path, ev, data, "pi_session_cwd")?;
    let live_session_path = want_str(events_path, ev, data, "pi_session_path")?;
    let expected_live_path = format!(
        ".creating/pi-sessions/{}/pi-session-{session_id}.jsonl",
        ev.run_id.as_str()
    );
    if live_session_path != expected_live_path {
        return Err(Error::CorruptEventLog {
            path: events_path.to_path_buf(),
            reason: format!(
                "event seq={} pi_session_path is not the canonical state-relative path",
                ev.seq
            ),
        });
    }
    let attempt = match data.get("attempt") {
        None | Some(Value::Null) => 0,
        Some(value) => value
            .as_u64()
            .and_then(|raw| u32::try_from(raw).ok())
            .ok_or_else(|| Error::CorruptEventLog {
                path: events_path.to_path_buf(),
                reason: format!("event seq={} attempt must be a u32", ev.seq),
            })?,
    };
    Ok(Some(WorkerEvidence {
        attempt,
        session_id: session_id.to_string(),
        original_cwd: original_cwd.to_string(),
        live_session_path: live_session_path.to_string(),
        status: EvidenceStatus::Pending,
        transcript_path: None,
        resume_path: None,
        pane_path: None,
        report_path: None,
        transcript_sha256: None,
        error: None,
    }))
}

fn reduce_node_created(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    let events_path = paths.events();
    // The envelope `node_id` is already a validated `NodeId` (parsed on read),
    // so take it directly — no re-parse needed.
    let node_id = require_envelope_node_id(&events_path, ev)?;
    let is_default_node = node_id.as_str() == "n-0001";
    // Idempotent on replay: skip if the node already exists.
    if read_node_opt(paths, &node_id)?.is_some() {
        return Ok(vec![]);
    }
    let d = &ev.data;
    let kind =
        data_kind(d.get("kind").unwrap_or(&Value::Null)).ok_or_else(|| Error::CorruptEventLog {
            path: events_path.clone(),
            reason: format!(
                "event seq={} kind=node.created missing/invalid `kind`",
                ev.seq
            ),
        })?;
    let n = Node {
        schema_version: STATE_SCHEMA_VERSION,
        node_id,
        // `run_id == paths.run_id` was verified at `reduce_event_to_ops` entry.
        run_id: paths.run_id.clone(),
        parent_node_id: opt_node_id(&events_path, ev, d, "parent_node_id")?,
        kind,
        status: Status::Pending,
        task: d.get("task").and_then(Value::as_str).map(str::to_string),
        worktree_path: d
            .get("worktree_path")
            .and_then(Value::as_str)
            .map(str::to_string),
        branch: d.get("branch").and_then(Value::as_str).map(str::to_string),
        base_sha: d
            .get("base_sha")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        tmux_window: d
            .get("tmux_window")
            .and_then(Value::as_str)
            .map(str::to_string),
        tmux_identity: tmux_identity_from_data(d).map(Box::new),
        evidence: worker_evidence_from_spawn_data(&events_path, ev, d)?,
        retained_display: None,
        retention_unavailable: None,
        agent_pid: optional_i32(d, "agent_pid", &events_path, ev)?,
        agent_pid_start_time: optional_ts(d, "agent_pid_start_time", &events_path, ev)?,
        supervisor_pid: optional_i32(d, "supervisor_pid", &events_path, ev)?,
        children: Vec::new(),
        started_at: Some(ev.ts),
        updated_at: ev.ts,
        last_report: None,
        last_processed_report_seq_by_child: serde_json::Map::default(),
        retry_attempts: 0,
        worker_exit: None,
        pending_merge: None,
        first_death_at: None,
        awaiting_input: None,
    };
    let mut ops = vec![ProjectionOp::Node(n)];
    if let Some(mut m) = read_manifest_opt(paths)? {
        // Materialization is the point at which an implicit source branch is
        // known. Preserve an explicit run.created value; otherwise fold the
        // source discovered by the creator into the manifest in this same
        // locked event application.
        if is_default_node && m.source_branch.is_none() {
            m.source_branch = d
                .get("source_branch")
                .and_then(Value::as_str)
                .filter(|branch| !branch.is_empty())
                .map(str::to_string);
        }
        // `node_count` is derived from the projection directories in
        // `advance_applied_seq`, never incremented here — see the module note
        // and issue `manifest-counter-desync`. This op also refreshes the run's
        // last-activity timestamp.
        m.updated_at = ev.ts;
        ops.push(ProjectionOp::Manifest(m));
    }
    Ok(ops)
}

/// Rewire an existing node to a freshly re-spawned agent after an empty-handed
/// `agent-died` bounded auto-retry (issue `autoretry-agent-died-worker`). The
/// supervisor tore down the dead worker's stale worktree and `create.sh`'d a
/// clean one at the run's source branch; this event carries the new spawn
/// metadata (`branch`, `base_sha`, `worktree_path`, tmux identity, `agent_pid`)
/// plus the audit fields (`attempt`, `reason`).
///
/// It updates the node in place: the new agent's coordinates replace the dead
/// one's, `status` returns to `Pending`, `started_at` is re-stamped so the
/// watchdog's spawn-grace window re-applies to the new agent, `last_report` is
/// cleared, and `retry_attempts` is incremented — the DURABLE, restart-safe
/// bound the watchdog checks before scheduling the next retry.
///
/// Guards, mirroring the other node reducers:
/// - A missing node is a no-op (a retry event whose node was never created).
/// - A TERMINAL node is never resurrected (a settled node is frozen): if a real
///   `node.report` raced in and terminalized the node, the retry is a dead event.
///   This keeps replay robust and preserves the terminal-state invariant.
fn reduce_node_retry(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    let events_path = paths.events();
    let node_id = require_envelope_node_id(&events_path, ev)?;
    let mut n = match read_node_opt(paths, &node_id)? {
        Some(n) => n,
        None => return Ok(vec![]),
    };
    // Terminal-state guard: a settled node is frozen. A late `node.report` that
    // beat this retry to the lock wins; the retry must not resurrect it.
    if n.status.is_terminal() {
        tracing::debug!(
            target: "taskfleet_core::reducer",
            seq = ev.seq, kind = %ev.kind, node_id = %node_id, current = ?n.status,
            "no-op: node.retry against terminal node"
        );
        return Ok(vec![]);
    }
    let d = &ev.data;
    // Rewire to the new agent. Each field mirrors `reduce_node_created`'s parsing
    // so the projection shape is identical to a fresh spawn.
    n.branch = d.get("branch").and_then(Value::as_str).map(str::to_string);
    n.base_sha = d
        .get("base_sha")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    n.worktree_path = d
        .get("worktree_path")
        .and_then(Value::as_str)
        .map(str::to_string);
    n.tmux_window = d
        .get("tmux_window")
        .and_then(Value::as_str)
        .map(str::to_string);
    n.tmux_identity = tmux_identity_from_data(d).map(Box::new);
    n.evidence = worker_evidence_from_spawn_data(&events_path, ev, d)?;
    n.retained_display = None;
    n.retention_unavailable = None;
    n.agent_pid = optional_i32(d, "agent_pid", &events_path, ev)?;
    n.agent_pid_start_time = optional_ts(d, "agent_pid_start_time", &events_path, ev)?;
    n.status = Status::Pending;
    n.started_at = Some(ev.ts);
    n.updated_at = ev.ts;
    n.last_report = None;
    // Drop any in-flight merge transaction from the PREVIOUS attempt: the retry
    // rewires the node to a new branch/worktree/agent, so a `pending_merge` that
    // referenced the dead attempt's branch must not carry forward — recovery would
    // otherwise judge the new attempt from the old worker's merge state (issue
    // `merge-transaction-recovery`, /llm-review finding).
    n.pending_merge = None;
    // Clear the previous attempt's told exit fact: the freshly re-spawned worker
    // is a NEW process, so a stale `worker_exit` must not carry over — otherwise
    // the supervisor's told-fact pass would instantly (mis)judge the new attempt
    // from the dead one's exit (issue `thin-exit-status-launcher`).
    n.worker_exit = None;
    // Clear the previous attempt's first-death anchor: the residual crash backstop
    // must measure the NEW attempt's own post-death grace from scratch, not inherit
    // the dead attempt's timestamp (which would fire the backstop with no grace on
    // the fresh worker's first confirmed death). Issue `typed-supervisor-outcomes`.
    n.first_death_at = None;
    // A retry is a new worker attempt. Never carry an unresolved question from
    // the dead attempt onto the replacement worker.
    n.awaiting_input = None;
    // The event carries its ABSOLUTE attempt number (the supervisor set it to
    // `retry_attempts + 1` at emit time). Assign it directly rather than a blind
    // `+= 1`: this makes the projection a pure function of the event, so a
    // full replay from seq 0, or a (guarded-against but defensive) double-apply,
    // converges to the same `retry_attempts` the log declares — the audit count
    // and the durable bound can never disagree. A legacy/malformed event with no
    // parseable `attempt` falls back to the monotone increment.
    n.retry_attempts = d
        .get("attempt")
        .and_then(Value::as_u64)
        .map_or_else(|| n.retry_attempts.saturating_add(1), |a| a as u32);
    Ok(vec![ProjectionOp::Node(n)])
}

fn reduce_node_status(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    let events_path = paths.events();
    let node_id = require_envelope_node_id(&events_path, ev)?;
    let mut n = match read_node_opt(paths, &node_id)? {
        Some(n) => n,
        None => return Ok(vec![]),
    };
    let new_status = require_status(ev, events_path)?;
    // Terminal-state guard: a settled node never transitions again. See
    // run-cli-read/handoff.md D5.
    if n.status.is_terminal() {
        trace_terminal_noop(ev, n.status, new_status);
        return Ok(vec![]);
    }
    if n.status == new_status {
        return Ok(vec![]);
    }
    n.status = new_status;
    // A terminal `node.status` (e.g. a watchdog-synthesized failure) ends the
    // node's lifecycle, so any in-flight merge transaction is moot — clear it so a
    // `pending_merge` is not stranded on a terminal node (recovery skips terminal
    // nodes, so an uncleared one would dangle forever). Issue
    // `merge-transaction-recovery` (/llm-review finding).
    if new_status.is_terminal() {
        n.pending_merge = None;
        n.awaiting_input = None;
    }
    n.updated_at = ev.ts;
    Ok(vec![ProjectionOp::Node(n)])
}

fn reduce_node_report(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    let events_path = paths.events();
    let node_id = require_envelope_node_id(&events_path, ev)?;
    let mut n = match read_node_opt(paths, &node_id)? {
        Some(n) => n,
        None => return Ok(vec![]),
    };
    // Terminal-state guard *before* payload validation: a node that already
    // reached a terminal state is settled, so a late-arriving report (e.g. an
    // agent success racing a `run cancel`) is a dead event — it must not
    // resurrect the node, and must not even decorate the projection, so
    // `last_report` is left untouched. Guarding first also keeps replay
    // robust: a malformed dead report against a settled node is a clean
    // no-op rather than a `CorruptEventLog` that would brick rebuild of a
    // log `append_and_apply_event` already committed. See run-cli-read/handoff.md
    // D5. (3/4 of /llm-review preferred guard-before-validate over the
    // reverse the issue spec sketched; the required CorruptEventLog cases
    // all target live nodes, so validation still runs for them.)
    if n.status.is_terminal() {
        // ONE exception to the dead-event rule: a late, CONFIRMED explicit-merge
        // report is adopted even against a terminal node (issue
        // `reducer-adopt-explicit-merge`). A watchdog `agent-died` false positive
        // on a long-lived interactive run can terminalize a node BEFORE the user's
        // `run merge` report arrives; an explicit user merge carries strictly
        // higher-fidelity ground truth (the branch demonstrably landed in source)
        // than a watchdog timeout, so it wins. Overwriting `last_report` here is
        // what lets `any_node_merged_explicitly` see the merge and the SUPERVISOR
        // — invariant #5's canonical teardown actor — warrant teardown, instead of
        // the CLI compensating inline (issues `merge-skips-teardown`,
        // `agent-died-merge-no-teardown-interactive`).
        //
        // Scoped tightly, on BOTH sides:
        //   - incoming: a CONFIRMED SUCCESSFUL explicit merge
        //     (`via == "explicit-merge"`, `success == true`, not `cancelled`) —
        //     matches exactly the force-`-D` teardown gate (`node_branch_merged`),
        //     so a failed/cancelled or non-merge late report never resurrects a
        //     settled node and unmerged-work preservation is untouched.
        //   - prior: only a `Failed` or `Done` node (positive whitelist). A
        //     `Cancelled` terminal is a DELIBERATE `run cancel` teardown, not a
        //     watchdog false positive, so a later merge does not override it (it
        //     stays cancelled — matching the existing "late success report keeps the
        //     cancel" reducer contract). The whitelist (rather than `!= Cancelled`)
        //     is future-safe: a new deliberate-teardown terminal added later is not
        //     silently resurrected to Done.
        // Idempotent: if this exact report is already the node's `last_report`,
        // re-folding it on replay is a clean no-op (never churns `updated_at`).
        //
        // The node reducer does not directly project run status: the supervisor
        // owns cross-node/child topology. Once that topology is wholly successful,
        // it emits the narrowly-authorized Failed -> Done recovery `run.status`,
        // whose reducer independently verifies this adopted merge evidence.
        if ReportOrigin::permits_terminal_merge_recovery(n.status, &ev.data) {
            if n.last_report.as_ref() == Some(&ev.data) && n.status == Status::Done {
                return Ok(vec![]);
            }
            tracing::info!(
                target: "taskfleet_core::reducer",
                seq = ev.seq, kind = %ev.kind, node_id = %node_id, prior = ?n.status,
                "adopting late explicit-merge report against terminal node (invariant #5 teardown)"
            );
            n.last_report = Some(ev.data.clone());
            // A confirmed merge is a terminal SUCCESS: the work landed in source.
            // (A false watchdog `Failed` is corrected to `Done`; a genuine `Done`
            // stays `Done` with the merge marker adopted so teardown is warranted.)
            n.status = Status::Done;
            // The merge completed, so any in-flight merge transaction is resolved:
            // clear it so recovery does not later re-examine a settled node
            // (issue `merge-transaction-recovery`).
            n.pending_merge = None;
            n.awaiting_input = None;
            n.updated_at = ev.ts;
            return Ok(vec![ProjectionOp::Node(n)]);
        }
        tracing::debug!(
            target: "taskfleet_core::reducer",
            seq = ev.seq, kind = %ev.kind, node_id = %node_id, current = ?n.status,
            "no-op: node.report against terminal node"
        );
        return Ok(vec![]);
    }
    // Live node: validate the report's terminal outcome. A `node.report`
    // must express exactly one terminal outcome — success/failure XOR
    // cancellation. Anything else (a bare `{}` with neither, or the
    // contradiction `success: true` + `cancelled: true`) is a corrupt event:
    // the reducer is the canonical gate, so reject it rather than silently
    // leaving the node in a dangling state. See design.md §7.7 and
    // node-cli-read/handoff.md D4.
    let new_status = report_terminal_status(&events_path, ev)?;
    n.last_report = Some(ev.data.clone());
    n.status = new_status;
    // A terminal report settles any open human-decision request. A blocked
    // report still preserves the discussion in `last_report`, while avoiding a
    // stale non-terminal awaiting-input flag on the settled node.
    n.awaiting_input = None;
    // Any terminal outcome resolves an in-flight merge transaction: a successful
    // `explicit-merge` report completes it here (the normal, no-crash path), and
    // any other terminal report ends the node's lifecycle so no merge recovery
    // should later fire (issue `merge-transaction-recovery`).
    n.pending_merge = None;
    n.updated_at = ev.ts;
    Ok(vec![ProjectionOp::Node(n)])
}

/// Fold a `worker.exited` event onto the node's `worker_exit` field (design.md
/// §2.1 / A1). This records the launcher shim's **told** exit status as a
/// durable fact; it deliberately does NOT transition `status`. Terminalization
/// is the supervisor's decision via the typed outcome table (§2.6) — a non-zero
/// or signalled exit becomes `failed`, while a clean exit without a merge stays
/// non-terminal (attention-required). Keeping the status transition out of the
/// reducer is what lets the clean-but-unmerged worker remain a visible, resumable
/// state instead of an auto-failed one.
///
/// Payload contract: at least one of `exit_code` (JSON integer) or `signal`
/// (JSON integer) must be present; a payload carrying neither is a corrupt event
/// (the reducer is the canonical gate). The fold is idempotent and **first-write-
/// wins**: once `worker_exit` is set, a replay or a spurious duplicate is a clean
/// no-op, so a full replay from seq 0 converges to the same recorded fact.
fn reduce_worker_exited(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    let events_path = paths.events();
    let node_id = require_envelope_node_id(&events_path, ev)?;
    let attempt = match ev.data.get("attempt") {
        None | Some(Value::Null) => 0,
        Some(_) => evidence_attempt(&events_path, ev)?,
    };
    let code = optional_i32(&ev.data, "exit_code", &events_path, ev)?;
    let signal = optional_i32(&ev.data, "signal", &events_path, ev)?;
    // A worker exit is EXACTLY one of a normal return (code) or a signal death
    // (signal). Neither is meaningless; both is contradictory (a process cannot
    // both return a code and be killed) — reject either rather than record an
    // ambiguous fact the outcome classifier would then have to disambiguate.
    match (code, signal) {
        (Some(_), None) | (None, Some(_)) => {}
        _ => {
            return Err(Error::CorruptEventLog {
                path: events_path,
                reason: format!(
                    "event seq={} kind=worker.exited must carry EXACTLY one of `exit_code` or `signal`",
                    ev.seq
                ),
            });
        }
    }
    let mut n = match read_node_opt(paths, &node_id)? {
        Some(n) => n,
        // No projection to decorate. A `worker.exited` for a node that does not
        // exist folds to nothing — consistent with the other node reducers
        // (`node.report` / `node.status`). In practice the shim validates the node
        // exists before it can record an exit, and normal append ordering always
        // places `node.created` first, so this is only hit for a genuinely orphan
        // event.
        None => return Ok(vec![]),
    };
    // First-write-wins: the shim fires exactly once per worker, so an existing
    // record is a replay/duplicate. Leaving it untouched keeps the fold a pure
    // function of the first exit event and never churns `updated_at`.
    if attempt != n.retry_attempts || n.worker_exit.is_some() {
        return Ok(vec![]);
    }
    n.worker_exit = Some(WorkerExit {
        code,
        signal,
        at: ev.ts,
    });
    // A departed worker cannot proceed on its recommended default. Clear its
    // open request so clean-exit attention and crash/stall handling remain the
    // actionable read-surface verdicts.
    n.awaiting_input = None;
    n.updated_at = ev.ts;
    Ok(vec![ProjectionOp::Node(n)])
}

fn evidence_attempt(events_path: &Path, ev: &Event) -> Result<u32> {
    ev.data
        .get("attempt")
        .and_then(Value::as_u64)
        .and_then(|raw| u32::try_from(raw).ok())
        .ok_or_else(|| Error::CorruptEventLog {
            path: events_path.to_path_buf(),
            reason: format!("event seq={} attempt must be a u32", ev.seq),
        })
}

fn reduce_worker_evidence_archived(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    let events_path = paths.events();
    let node_id = require_envelope_node_id(&events_path, ev)?;
    let attempt = evidence_attempt(&events_path, ev)?;
    let session_id = want_str(&events_path, ev, &ev.data, "session_id")?.to_string();
    let mut values = Vec::new();
    for field in [
        "transcript_path",
        "resume_path",
        "pane_path",
        "report_path",
        "transcript_sha256",
    ] {
        values.push(want_str(&events_path, ev, &ev.data, field)?.to_string());
    }
    let expected_prefix = format!("evidence/{}/", node_id.as_str());
    for (index, suffix) in [
        "pi-session.original.jsonl",
        "pi-session.resume.jsonl",
        "final-pane.log",
        "terminal-report.json",
    ]
    .iter()
    .enumerate()
    {
        if values[index] != format!("{expected_prefix}{suffix}") {
            return Err(Error::CorruptEventLog {
                path: events_path,
                reason: format!(
                    "event seq={} evidence artifact path is not canonical",
                    ev.seq
                ),
            });
        }
    }
    if values[4].len() != 64
        || !values[4]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::CorruptEventLog {
            path: events_path,
            reason: format!(
                "event seq={} transcript_sha256 is not lowercase SHA-256",
                ev.seq
            ),
        });
    }
    let mut node = match read_node_opt(paths, &node_id)? {
        Some(node) => node,
        None => return Ok(vec![]),
    };
    let Some(evidence) = node.evidence.as_mut() else {
        return Err(Error::CorruptEventLog {
            path: events_path,
            reason: format!(
                "event seq={} evidence archive has no recorded Pi session",
                ev.seq
            ),
        });
    };
    if evidence.attempt != attempt || evidence.session_id != session_id {
        return Ok(vec![]);
    }
    if evidence.status == EvidenceStatus::Complete {
        return Ok(vec![]);
    }
    evidence.transcript_path = Some(values.remove(0));
    evidence.resume_path = Some(values.remove(0));
    evidence.pane_path = Some(values.remove(0));
    evidence.report_path = Some(values.remove(0));
    evidence.transcript_sha256 = Some(values.remove(0));
    evidence.status = EvidenceStatus::Complete;
    evidence.error = None;
    node.updated_at = ev.ts;
    Ok(vec![ProjectionOp::Node(node)])
}

fn reduce_worker_display_retained(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    let events_path = paths.events();
    let node_id = require_envelope_node_id(&events_path, ev)?;
    let display: crate::schema::RetainedDisplay =
        serde_json::from_value(ev.data.clone()).map_err(|e| Error::CorruptEventLog {
            path: events_path.clone(),
            reason: format!("event seq={} invalid retained display: {e}", ev.seq),
        })?;
    let mut node = match read_node_opt(paths, &node_id)? {
        Some(node) => node,
        None => return Ok(vec![]),
    };
    if display.attempt != node.retry_attempts || display.ownership_marker.is_empty() {
        return Ok(vec![]);
    }
    if node.retained_display.is_some() {
        return Ok(vec![]);
    }
    node.retained_display = Some(Box::new(display));
    node.updated_at = ev.ts;
    Ok(vec![ProjectionOp::Node(node)])
}

fn reduce_worker_display_expired(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    let events_path = paths.events();
    let node_id = require_envelope_node_id(&events_path, ev)?;
    let marker = want_str(&events_path, ev, &ev.data, "ownership_marker")?;
    let attempt = evidence_attempt(&events_path, ev)?;
    let mut node = match read_node_opt(paths, &node_id)? {
        Some(node) => node,
        None => return Ok(vec![]),
    };
    let Some(display) = node.retained_display.as_mut() else {
        return Ok(vec![]);
    };
    if display.attempt != attempt
        || display.ownership_marker != marker
        || display.expired_at.is_some()
    {
        return Ok(vec![]);
    }
    display.expired_at = Some(ev.ts);
    node.updated_at = ev.ts;
    Ok(vec![ProjectionOp::Node(node)])
}

fn reduce_worker_display_unavailable(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    let events_path = paths.events();
    let node_id = require_envelope_node_id(&events_path, ev)?;
    let attempt = evidence_attempt(&events_path, ev)?;
    let reason = want_str(&events_path, ev, &ev.data, "reason")?;
    let mut node = match read_node_opt(paths, &node_id)? {
        Some(node) => node,
        None => return Ok(vec![]),
    };
    if attempt != node.retry_attempts || node.retained_display.is_some() {
        return Ok(vec![]);
    }
    if node.retention_unavailable.is_none() {
        node.retention_unavailable = Some(reason.to_string());
        node.updated_at = ev.ts;
    }
    Ok(vec![ProjectionOp::Node(node)])
}

fn reduce_worker_evidence_failed(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    let events_path = paths.events();
    let node_id = require_envelope_node_id(&events_path, ev)?;
    let attempt = evidence_attempt(&events_path, ev)?;
    let session_id = want_str(&events_path, ev, &ev.data, "session_id")?.to_string();
    let detail = want_str(&events_path, ev, &ev.data, "error")?.to_string();
    let mut node = match read_node_opt(paths, &node_id)? {
        Some(node) => node,
        None => return Ok(vec![]),
    };
    let Some(evidence) = node.evidence.as_mut() else {
        return Err(Error::CorruptEventLog {
            path: events_path,
            reason: format!(
                "event seq={} evidence failure has no recorded Pi session",
                ev.seq
            ),
        });
    };
    if evidence.attempt != attempt || evidence.session_id != session_id {
        return Ok(vec![]);
    }
    if evidence.status == EvidenceStatus::Complete {
        return Ok(vec![]);
    }
    evidence.status = EvidenceStatus::Failed;
    evidence.error = Some(detail);
    node.updated_at = ev.ts;
    Ok(vec![ProjectionOp::Node(node)])
}

/// Fold a `node.death_observed` event onto [`Node::first_death_at`], recording
/// the FIRST tick on which the supervisor saw this node's worker confirmed-dead
/// with no told `worker.exited` and no merge — the durable anchor for the
/// residual crash backstop's fixed post-death grace (design.md §2.1a, issue
/// `typed-supervisor-outcomes`).
///
/// The anchor is the event's own timestamp (`ev.ts`) — no payload field needed.
/// The fold is **first-write-wins**: the anchor is monotonic, so a later
/// re-observation (a supervisor restart still seeing the dead pid) never resets
/// the clock, and a full replay from seq 0 converges to the first observation. A
/// `node.death_observed` for a missing or already terminal node folds to nothing
/// (the backstop is moot once the node settles).
fn reduce_node_death_observed(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    let events_path = paths.events();
    let node_id = require_envelope_node_id(&events_path, ev)?;
    let mut n = match read_node_opt(paths, &node_id)? {
        Some(n) => n,
        None => return Ok(vec![]),
    };
    // First-write-wins (monotonic anchor). No-op once the backstop is moot or a
    // higher-fidelity fact exists — a terminal node, a told `worker.exited`, a
    // landed report, or an in-flight merge transaction. The supervisor's emitter
    // already gates on all of these under the exclusive lock; mirroring them here
    // keeps a from-scratch replay convergent regardless of caller.
    if n.first_death_at.is_some()
        || n.status.is_terminal()
        || n.worker_exit.is_some()
        || n.last_report.is_some()
        || n.pending_merge.is_some()
    {
        return Ok(vec![]);
    }
    n.first_death_at = Some(ev.ts);
    n.updated_at = ev.ts;
    Ok(vec![ProjectionOp::Node(n)])
}

/// Fold an agent's explicit request for a human decision onto the node without
/// changing its status. This is deliberately non-terminal: the worker may still
/// resolve the fork itself or proceed with its stated default.
///
/// The first open signal wins until a matching `node.input_resolved` clears it,
/// so retries or duplicate writes cannot move the grace clock forward. The
/// event timestamp, not a caller-supplied payload timestamp, is the durable
/// restart-safe anchor.
fn reduce_node_awaiting_input(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    const MAX_ITEMS: usize = 8;
    const MAX_TOPIC_CHARS: usize = 512;
    const MAX_OPTIONS: usize = 16;
    const MAX_OPTION_CHARS: usize = 256;

    let events_path = paths.events();
    let node_id = require_envelope_node_id(&events_path, ev)?;
    // Validate before every state-dependent no-op. An ignored duplicate or late
    // event must still be structurally valid before it enters the durable log.
    let items = ev
        .data
        .get("discussion_items")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty() && items.len() <= MAX_ITEMS)
        .ok_or_else(|| Error::CorruptEventLog {
            path: events_path.clone(),
            reason: format!(
                "event seq={} kind=node.awaiting_input requires 1..={MAX_ITEMS} `discussion_items`",
                ev.seq
            ),
        })?;
    for (index, item) in items.iter().enumerate() {
        let obj = item.as_object().ok_or_else(|| Error::CorruptEventLog {
            path: events_path.clone(),
            reason: format!(
                "event seq={} discussion_items[{index}] must be an object",
                ev.seq
            ),
        })?;
        let topic = obj.get("topic").and_then(Value::as_str).unwrap_or("");
        let default = obj
            .get("recommended_default")
            .and_then(Value::as_str)
            .unwrap_or("");
        let options = obj.get("options").and_then(Value::as_array);
        let options_valid = options.is_some_and(|values| {
            !values.is_empty()
                && values.len() <= MAX_OPTIONS
                && values.iter().all(|v| {
                    v.as_str().is_some_and(|s| {
                        !s.trim().is_empty() && s.chars().count() <= MAX_OPTION_CHARS
                    })
                })
                && values.iter().any(|v| v.as_str() == Some(default))
        });
        if topic.trim().is_empty()
            || topic.chars().count() > MAX_TOPIC_CHARS
            || default.trim().is_empty()
            || !options_valid
        {
            return Err(Error::CorruptEventLog {
                path: events_path.clone(),
                reason: format!(
                    "event seq={} discussion_items[{index}] requires bounded non-empty `topic`, 1..={MAX_OPTIONS} bounded string `options`, and a `recommended_default` present in options",
                    ev.seq
                ),
            });
        }
    }

    let mut n = match read_node_opt(paths, &node_id)? {
        Some(n) => n,
        None => return Ok(vec![]),
    };
    if n.status.is_terminal() || n.worker_exit.is_some() {
        return Ok(vec![]);
    }
    if let Some(open) = n.awaiting_input.as_mut() {
        // A later fork joins the current open generation without moving its
        // restart-safe clock or notification key. Bound the aggregate too.
        if open.discussion_items.len() + items.len() > MAX_ITEMS {
            return Err(Error::CorruptEventLog {
                path: events_path,
                reason: format!(
                    "event seq={} would exceed {MAX_ITEMS} open discussion items",
                    ev.seq
                ),
            });
        }
        open.discussion_items.extend(items.iter().cloned());
    } else {
        n.awaiting_input = Some(Box::new(crate::schema::AwaitingInput {
            opened_at: ev.ts,
            event_seq: ev.seq,
            discussion_items: items.clone(),
        }));
    }
    n.updated_at = ev.ts;
    Ok(vec![ProjectionOp::Node(n)])
}

/// Clear the current open decision request. `event_seq` is mandatory and
/// fences the resolve to the generation the worker observed, so a delayed
/// timeout cannot clear a newer question opened after the old one resolved.
fn reduce_node_input_resolved(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    let events_path = paths.events();
    let node_id = require_envelope_node_id(&events_path, ev)?;
    // Validate before the no-open no-op so malformed events never enter the log
    // merely because their projection happens to be absent today.
    let seq = ev
        .data
        .get("event_seq")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::CorruptEventLog {
            path: events_path.clone(),
            reason: format!(
                "event seq={} kind=node.input_resolved requires unsigned `event_seq`",
                ev.seq
            ),
        })?;
    let mut n = match read_node_opt(paths, &node_id)? {
        Some(n) => n,
        None => return Ok(vec![]),
    };
    let Some(open) = n.awaiting_input.as_ref() else {
        return Ok(vec![]);
    };
    if seq != open.event_seq {
        return Ok(vec![]);
    }
    n.awaiting_input = None;
    n.updated_at = ev.ts;
    Ok(vec![ProjectionOp::Node(n)])
}

/// The event kind `run merge` appends BEFORE mutating git to record the
/// in-flight merge transaction (design.md §2.1b / A2). Its `data` payload is a
/// serialized [`MergeTxn`]; the reducer folds it onto [`Node::pending_merge`].
pub const KIND_MERGE_STARTED: &str = "merge.started";

/// The event kind recovery appends when it resolves a pending merge transaction
/// by REJECTING it — the recorded source ref never moved (the git mutation never
/// landed), so the worker's branch + work are preserved and the transaction is
/// cleared. Its `data` carries `op_id` (which transaction) and `reason`.
pub const KIND_MERGE_ABORTED: &str = "merge.aborted";

/// Fold a `merge.started` event onto [`Node::pending_merge`], recording the
/// in-flight `run merge` transaction BEFORE the git mutation so a crash between
/// the git merge and the terminal `explicit-merge` report can be resolved
/// deterministically by OID (design.md §2.1b / A2, issue
/// `merge-transaction-recovery`). The reducer deliberately does NOT transition
/// `status`: recording a transaction is not a terminal outcome.
///
/// Payload contract: the `data` is a serialized [`MergeTxn`]; a payload missing
/// required fields is a corrupt event (the reducer is the canonical gate, so the
/// append is rejected before any byte is written). The fold is idempotent —
/// re-folding the same `op_id` on replay is a clean no-op — and last-write-wins
/// across a fresh attempt's larger `op_id` (each `run merge` re-reads
/// `expected_source_oid`, so the newest record is authoritative).
fn reduce_merge_started(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    let events_path = paths.events();
    let node_id = require_envelope_node_id(&events_path, ev)?;
    let txn: MergeTxn =
        serde_json::from_value(ev.data.clone()).map_err(|e| Error::CorruptEventLog {
            path: events_path.clone(),
            reason: format!(
                "event seq={} kind=merge.started has an invalid MergeTxn payload: {e}",
                ev.seq
            ),
        })?;
    let mut n = match read_node_opt(paths, &node_id)? {
        Some(n) => n,
        None => return Ok(vec![]),
    };
    // A terminal node has no in-flight merge to track — `run merge` is refused on
    // a terminal run at the CLI, so this is a dead/duplicate event. Ignore it
    // (never resurrect the projection).
    if n.status.is_terminal() {
        return Ok(vec![]);
    }
    // Idempotent: re-folding the SAME transaction on replay must not churn
    // `updated_at`.
    if n.pending_merge.as_ref().map(|t| t.op_id.as_str()) == Some(txn.op_id.as_str()) {
        return Ok(vec![]);
    }
    n.pending_merge = Some(Box::new(txn));
    n.updated_at = ev.ts;
    Ok(vec![ProjectionOp::Node(n)])
}

/// Fold a `merge.aborted` event, clearing [`Node::pending_merge`] iff it names
/// the transaction being aborted (`op_id` match). Recovery appends this when it
/// determines a pending merge's git mutation never landed (the source ref is
/// still at `expected_source_oid`) or moved unexpectedly — the transaction is
/// rejected, the worker's branch + work are preserved, and the node stays
/// whatever non-terminal status it was (a retry may re-attempt the merge).
///
/// The `op_id` guard is what keeps this from clobbering a *newer* transaction: a
/// stale `merge.aborted` for a superseded attempt (a different `op_id`) is a
/// clean no-op. Deliberately
/// does NOT transition `status`.
fn reduce_merge_aborted(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    let events_path = paths.events();
    let node_id = require_envelope_node_id(&events_path, ev)?;
    let op_id = ev
        .data
        .get("op_id")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::CorruptEventLog {
            path: events_path.clone(),
            reason: format!(
                "event seq={} kind=merge.aborted is missing string `op_id`",
                ev.seq
            ),
        })?;
    let mut n = match read_node_opt(paths, &node_id)? {
        Some(n) => n,
        None => return Ok(vec![]),
    };
    // Clear only the transaction this event names. A mismatch (already resolved,
    // or a newer attempt is pending) is a clean no-op.
    match n.pending_merge.as_ref() {
        Some(t) if t.op_id == op_id => {}
        _ => return Ok(vec![]),
    }
    n.pending_merge = None;
    n.updated_at = ev.ts;
    Ok(vec![ProjectionOp::Node(n)])
}

/// Emit an observability trace for a status event dropped by the terminal
/// guard. Re-applying the *same* terminal status is routine idempotent replay
/// (`debug`); an event carrying a *different* status is a real conflict that
/// should not occur on a well-formed log (`warn`) — e.g. a `done` node being
/// told to go `cancelled`. The guard no-ops either way; the level is the only
/// difference, so a genuine corruption signal is visible without flooding
/// logs on every replay.
fn trace_terminal_noop(ev: &Event, current: Status, incoming: Status) {
    if current == incoming {
        tracing::debug!(
            target: "taskfleet_core::reducer",
            seq = ev.seq, kind = %ev.kind, status = ?current,
            "no-op: status re-applied to terminal target"
        );
    } else {
        tracing::warn!(
            target: "taskfleet_core::reducer",
            seq = ev.seq, kind = %ev.kind, current = ?current, incoming = ?incoming,
            "no-op: ignored conflicting transition from terminal target"
        );
    }
}

/// Derive the terminal status a `node.report` event asserts, enforcing the
/// success-XOR-cancelled invariant with strict boolean typing.
///
/// `cancelled: true` (with `success: false` or absent) → [`Status::Cancelled`].
/// Otherwise `success` must be present: `true` → [`Status::Done`], `false` →
/// [`Status::Failed`]. Neither field (bare `{}`), the contradiction
/// `success: true` + `cancelled: true`, or a non-boolean `success` /
/// `cancelled` is a [`Error::CorruptEventLog`].
fn report_terminal_status(events_path: &Path, ev: &Event) -> Result<Status> {
    let corrupt = |reason: String| Error::CorruptEventLog {
        path: events_path.to_path_buf(),
        reason,
    };
    let cancelled = optional_bool(events_path, ev, &ev.data, "cancelled")?.unwrap_or(false);
    let success = optional_bool(events_path, ev, &ev.data, "success")?;
    if cancelled {
        if success == Some(true) {
            return Err(corrupt(format!(
                "event seq={} kind=node.report has contradictory `success: true` with `cancelled: true`",
                ev.seq
            )));
        }
        Ok(Status::Cancelled)
    } else {
        match success {
            Some(true) => Ok(Status::Done),
            Some(false) => Ok(Status::Failed),
            None => Err(corrupt(format!(
                "event seq={} kind=node.report must set boolean `success` or `cancelled: true`",
                ev.seq
            ))),
        }
    }
}

fn reduce_child_spawned(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    // `child.spawned` is written to the PARENT run's events; the parent
    // spawning node is `ev.node_id`, the child run/node lives in `data`.
    let events_path = paths.events();
    let parent_node_id = ev.node_id.clone().ok_or_else(|| Error::CorruptEventLog {
        path: events_path.clone(),
        reason: format!(
            "event seq={} kind=child.spawned missing parent `node_id`",
            ev.seq
        ),
    })?;
    let child_run_id = RunId::parse_str(want_str(&events_path, ev, &ev.data, "child_run_id")?)
        .map_err(|e| corrupt_id(&events_path, ev, &e))?;
    let child_node_id = NodeId::parse_str(
        ev.data
            .get("child_node_id")
            .and_then(Value::as_str)
            .unwrap_or("n-0001"),
    )
    .map_err(|e| corrupt_id(&events_path, ev, &e))?;
    let mut n = match read_node_opt(paths, &parent_node_id)? {
        Some(n) => n,
        None => return Ok(vec![]),
    };
    let new_ref = ChildRef {
        run_id: child_run_id,
        node_id: child_node_id,
    };
    if n.children.iter().any(|c| c == &new_ref) {
        // Already recorded — pure no-op so replayed events don't churn
        // `updated_at` or the projection file.
        return Ok(vec![]);
    }
    n.children.push(new_ref);
    n.updated_at = ev.ts;
    Ok(vec![ProjectionOp::Node(n)])
}

/// `supervisor.attached` records the supervisor PID watching the envelope
/// node onto `Node.supervisor_pid`. Event-sourced replacement for the
/// supervisor's former direct `write_node` (issue
/// `supervisor-state-not-event-sourced`), so a from-scratch projection
/// rebuild reproduces the field.
///
/// Latest-wins: a later attach (a supervisor restart binds a fresh PID)
/// overrides the recorded value. Re-applying an event that carries the
/// already-recorded PID is a pure no-op, so replay never churns the
/// projection file's `updated_at`.
fn reduce_supervisor_attached(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    let events_path = paths.events();
    let node_id = require_envelope_node_id(&events_path, ev)?;
    let raw = ev
        .data
        .get("pid")
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::CorruptEventLog {
            path: events_path.clone(),
            reason: format!(
                "event seq={} kind=supervisor.attached missing/invalid `pid`",
                ev.seq
            ),
        })?;
    let pid = i32::try_from(raw).map_err(|_| Error::CorruptEventLog {
        path: events_path.clone(),
        reason: format!(
            "event seq={} kind=supervisor.attached `pid` out of i32 range: {raw}",
            ev.seq
        ),
    })?;
    let mut n = match read_node_opt(paths, &node_id)? {
        Some(n) => n,
        None => return Ok(vec![]),
    };
    if n.supervisor_pid == Some(pid) {
        return Ok(vec![]);
    }
    n.supervisor_pid = Some(pid);
    n.updated_at = ev.ts;
    Ok(vec![ProjectionOp::Node(n)])
}

/// `supervisor.cursor_advanced` mirrors the supervisor's per-child report
/// cursor onto the envelope (parent) node's `last_processed_report_seq_by_child`
/// map. Event-sourced replacement for the supervisor's former direct
/// `write_node` of that map (issue `supervisor-state-not-event-sourced`).
///
/// The cursor is monotonic: a `report_seq` at or below the recorded
/// high-water mark for this child is a no-op, so replaying the same event —
/// or an older out-of-order one — never moves the cursor backward or churns
/// the projection. This is the §7.3 idempotency guarantee at the reducer
/// boundary.
fn reduce_supervisor_cursor_advanced(paths: &RunPaths, ev: &Event) -> Result<Vec<ProjectionOp>> {
    let events_path = paths.events();
    let node_id = require_envelope_node_id(&events_path, ev)?;
    let child_run_id = want_str(&events_path, ev, &ev.data, "child_run_id")?;
    // Validate the child id even though it only becomes a map key — a forged
    // event must not smuggle a path-shaped or malformed run id into the
    // projection.
    RunId::parse_str(child_run_id).map_err(|e| corrupt_id(&events_path, ev, &e))?;
    let report_seq = ev
        .data
        .get("report_seq")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::CorruptEventLog {
            path: events_path.clone(),
            reason: format!(
                "event seq={} kind=supervisor.cursor_advanced missing/invalid `report_seq`",
                ev.seq
            ),
        })?;
    let mut n = match read_node_opt(paths, &node_id)? {
        Some(n) => n,
        None => return Ok(vec![]),
    };
    if let Some(prev) = n
        .last_processed_report_seq_by_child
        .get(child_run_id)
        .and_then(Value::as_u64)
    {
        if report_seq <= prev {
            return Ok(vec![]);
        }
    }
    n.last_processed_report_seq_by_child
        .insert(child_run_id.to_string(), Value::from(report_seq));
    n.updated_at = ev.ts;
    Ok(vec![ProjectionOp::Node(n)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Event;
    use chrono::Utc;
    use tempfile::TempDir;

    fn event(run_id: &str) -> Event {
        Event {
            ts: Utc::now(),
            seq: 1,
            kind: "run.status".into(),
            run_id: RunId::parse_str(run_id).unwrap(),
            node_id: None,
            idempotency_key: None,
            data: serde_json::json!({ "status": "running" }),
        }
    }

    #[test]
    fn orchestrator_decision_and_discuss_critical_reduce_to_noop() {
        // The /orchestrate audit kinds are append-only: the reducer must plan
        // ZERO projection ops for them regardless of payload, so the event log
        // is their sole home and no projection is created or mutated.
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let rid = RunId::parse_str(run_id).unwrap();
        let dir = crate::run_dir(tmp.path(), &rid);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = RunPaths::new(dir, run_id).unwrap();

        // Bootstrap a manifest so we can prove the audit events leave it
        // byte-for-byte untouched (no counter churn, no status drift).
        let mut created = event(run_id);
        created.kind = "run.created".into();
        created.data = serde_json::json!({
            "kind": "spinoff", "lifecycle": "autonomous", "title": "t"
        });
        apply_event(&paths, &created).expect("run.created applies");
        let manifest_before = std::fs::read(paths.manifest()).unwrap();

        for (seq, kind) in [(10u64, "orchestrator.decision"), (11, "discuss.critical")] {
            let mut ev = event(run_id);
            ev.seq = seq;
            ev.kind = kind.into();
            // A non-trivial payload to prove the reducer ignores it wholesale.
            ev.data = serde_json::json!({ "summary": "x", "arbitrary": [1, 2, 3] });
            let ops = reduce_event_to_ops(&paths, &ev).expect("audit kind reduces cleanly");
            assert!(ops.is_empty(), "{kind} must plan no projection ops");
            // apply_event is the plan+commit path; it must also be a clean no-op.
            apply_event(&paths, &ev).expect("audit kind applies as no-op");
        }

        // The manifest is unchanged and no stray projection dirs appeared.
        assert_eq!(
            std::fs::read(paths.manifest()).unwrap(),
            manifest_before,
            "audit events must not mutate the manifest"
        );
        assert!(!paths.nodes_dir().exists(), "no node projection created");
    }

    #[test]
    fn run_created_folds_harness_when_present_and_defaults_none() {
        let tmp = TempDir::new().unwrap();

        // A `run.created` carrying `harness` folds it onto the manifest.
        let run_id = "01jxhrnsaa0000000000000001";
        let rid = RunId::parse_str(run_id).unwrap();
        let dir = crate::run_dir(tmp.path(), &rid);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = RunPaths::new(dir, run_id).unwrap();
        let mut created = event(run_id);
        created.kind = "run.created".into();
        created.data = serde_json::json!({
            "kind": "spinoff", "lifecycle": "autonomous", "title": "t",
            "harness": "pi", "harness_source": "flag",
        });
        apply_event(&paths, &created).expect("run.created applies");
        let m = read_manifest_opt(&paths).unwrap().unwrap();
        assert_eq!(m.harness.as_deref(), Some("pi"));

        // A `run.created` WITHOUT `harness` (legacy / claude) leaves it `None`.
        let run_id2 = "01jxhrnsaa0000000000000002";
        let rid2 = RunId::parse_str(run_id2).unwrap();
        let dir2 = crate::run_dir(tmp.path(), &rid2);
        std::fs::create_dir_all(&dir2).unwrap();
        let paths2 = RunPaths::new(dir2, run_id2).unwrap();
        let mut created2 = event(run_id2);
        created2.kind = "run.created".into();
        created2.data =
            serde_json::json!({ "kind": "spinoff", "lifecycle": "autonomous", "title": "t" });
        apply_event(&paths2, &created2).expect("run.created applies");
        let m2 = read_manifest_opt(&paths2).unwrap().unwrap();
        assert_eq!(m2.harness, None);
    }

    /// Bootstrap a run manifest + one live `n-0001` spinoff node, returning its
    /// paths. Used by the `node.retry` reducer tests.
    fn bootstrap_retry_node(tmp: &TempDir, run_id: &str) -> RunPaths {
        let rid = RunId::parse_str(run_id).unwrap();
        let dir = crate::run_dir(tmp.path(), &rid);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = RunPaths::new(dir, run_id).unwrap();
        let mut created = event(run_id);
        created.kind = "run.created".into();
        created.data =
            serde_json::json!({ "kind": "spinoff", "lifecycle": "autonomous", "title": "t" });
        apply_event(&paths, &created).expect("run.created applies");
        let mut node = event(run_id);
        node.seq = 2;
        node.kind = "node.created".into();
        node.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        node.data = serde_json::json!({
            "kind": "spinoff",
            "branch": "wt/foo",
            "worktree_path": "/tmp/old-wt",
            "agent_pid": 111,
        });
        apply_event(&paths, &node).expect("node.created applies");
        paths
    }

    /// `node.retry` rewires the node to the freshly re-spawned agent, returns it to
    /// `Pending`, re-stamps `started_at`, and increments the durable
    /// `retry_attempts` bound (issue `autoretry-agent-died-worker`).
    #[test]
    fn node_retry_rewires_node_and_increments_attempts() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let paths = bootstrap_retry_node(&tmp, run_id);

        let mut retry = event(run_id);
        retry.seq = 3;
        retry.kind = "node.retry".into();
        retry.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        retry.data = serde_json::json!({
            "attempt": 1,
            "reason": "agent-died",
            "branch": "wt/foo-r1",
            "base_sha": "a".repeat(40),
            "worktree_path": "/tmp/new-wt",
            "agent_pid": 222,
            "tmux_session": "s",
            "tmux_window_id": "@9",
        });
        apply_event(&paths, &retry).expect("node.retry applies");

        let n = read_n0001(&paths);
        assert_eq!(n.retry_attempts, 1, "attempt bound incremented");
        assert_eq!(
            n.branch.as_deref(),
            Some("wt/foo-r1"),
            "rewired to new branch"
        );
        assert_eq!(n.worktree_path.as_deref(), Some("/tmp/new-wt"));
        assert_eq!(n.agent_pid, Some(222), "rewired to new agent pid");
        assert_eq!(n.status, Status::Pending, "node returns to pending");
        assert!(n.last_report.is_none());
        assert_eq!(
            n.tmux_identity.as_ref().map(|t| t.window_id.as_str()),
            Some("@9"),
            "rewired tmux identity"
        );

        // A second retry increments again — the bound is monotone.
        let mut retry2 = event(run_id);
        retry2.seq = 4;
        retry2.kind = "node.retry".into();
        retry2.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        retry2.data = serde_json::json!({
            "attempt": 2, "reason": "agent-died", "branch": "wt/foo-r2",
            "worktree_path": "/tmp/new-wt-2", "agent_pid": 333,
        });
        apply_event(&paths, &retry2).expect("node.retry applies");
        assert_eq!(read_n0001(&paths).retry_attempts, 2);
    }

    #[test]
    fn stale_worker_evidence_and_exit_do_not_cross_retry_generation() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let paths = bootstrap_retry_node(&tmp, run_id);
        let nid = NodeId::parse_str("n-0001").unwrap();
        let current_session = "018f5f64-b137-7d44-b2b4-4f02c3f646e8";

        let mut retry = event(run_id);
        retry.seq = 3;
        retry.kind = "node.retry".into();
        retry.node_id = Some(nid.clone());
        retry.data = serde_json::json!({
            "attempt": 1, "reason": "agent-died", "branch": "wt/foo-r1",
            "worktree_path": "/tmp/new-wt", "agent_pid": 222,
            "pi_session_id": current_session,
            "pi_session_path": format!(".creating/pi-sessions/{run_id}/pi-session-{current_session}.jsonl"),
            "pi_session_cwd": "/tmp/new-wt"
        });
        apply_event(&paths, &retry).unwrap();

        let mut stale_failure = event(run_id);
        stale_failure.seq = 4;
        stale_failure.kind = "worker.evidence.failed".into();
        stale_failure.node_id = Some(nid.clone());
        stale_failure.data = serde_json::json!({
            "attempt": 0,
            "session_id": "118f5f64-b137-7d44-b2b4-4f02c3f646e8",
            "error": "old attempt"
        });
        apply_event(&paths, &stale_failure).unwrap();
        assert_eq!(
            read_n0001(&paths).evidence.unwrap().status,
            EvidenceStatus::Pending
        );

        let mut stale_exit = event(run_id);
        stale_exit.seq = 5;
        stale_exit.kind = "worker.exited".into();
        stale_exit.node_id = Some(nid.clone());
        stale_exit.data = serde_json::json!({"attempt":0,"exit_code":9});
        apply_event(&paths, &stale_exit).unwrap();
        assert!(read_n0001(&paths).worker_exit.is_none());

        let mut archived = event(run_id);
        archived.seq = 6;
        archived.kind = "worker.evidence.archived".into();
        archived.node_id = Some(nid);
        archived.data = serde_json::json!({
            "attempt":1, "session_id":current_session,
            "transcript_path":"evidence/n-0001/pi-session.original.jsonl",
            "resume_path":"evidence/n-0001/pi-session.resume.jsonl",
            "pane_path":"evidence/n-0001/final-pane.log",
            "report_path":"evidence/n-0001/terminal-report.json",
            "transcript_sha256":"0".repeat(64)
        });
        apply_event(&paths, &archived).unwrap();
        assert_eq!(
            read_n0001(&paths).evidence.unwrap().status,
            EvidenceStatus::Complete
        );
    }

    /// A `node.retry` against an already-terminal node is a dead event: the
    /// terminal-state invariant holds, so a late retry never resurrects a settled
    /// node (a real report that raced in wins).
    #[test]
    fn node_retry_against_terminal_node_is_noop() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let paths = bootstrap_retry_node(&tmp, run_id);

        // Terminalize the node via a success report.
        let mut report = event(run_id);
        report.seq = 3;
        report.kind = "node.report".into();
        report.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        report.data = serde_json::json!({ "success": true });
        apply_event(&paths, &report).expect("node.report applies");
        assert_eq!(read_n0001(&paths).status, Status::Done);

        let mut retry = event(run_id);
        retry.seq = 4;
        retry.kind = "node.retry".into();
        retry.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        retry.data = serde_json::json!({
            "attempt": 1, "reason": "agent-died", "branch": "wt/foo-r1",
            "worktree_path": "/tmp/new-wt", "agent_pid": 222,
        });
        apply_event(&paths, &retry).expect("node.retry applies as no-op");

        let n = read_n0001(&paths);
        assert_eq!(n.status, Status::Done, "terminal node not resurrected");
        assert_eq!(n.retry_attempts, 0, "no increment against terminal node");
        assert_eq!(n.agent_pid, Some(111), "not rewired");
    }

    #[test]
    fn apply_event_rejects_event_from_a_different_run() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let rid = RunId::parse_str(run_id).unwrap();
        let dir = crate::run_dir(tmp.path(), &rid);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = RunPaths::new(dir, run_id).unwrap();

        // An event whose envelope names a different run must not be folded.
        let foreign = event("02jxsnap000000000000000000");
        let err = apply_event(&paths, &foreign).expect_err("cross-run event must be rejected");
        assert!(matches!(err, Error::CorruptEventLog { .. }), "got {err:?}");

        // The matching run_id is accepted (no projection exists yet, so
        // `run.status` is a clean no-op rather than an error).
        let mine = event(run_id);
        apply_event(&paths, &mine).expect("matching run_id must be accepted");
    }

    #[test]
    fn tmux_identity_from_data_reads_qualified_fields() {
        let d = serde_json::json!({
            "tmux_socket": "/private/tmp/tmux-501/default",
            "tmux_session": "taskfleet",
            "tmux_window_id": "@42",
        });
        let id = tmux_identity_from_data(&d).expect("qualified identity");
        assert_eq!(id.socket.as_deref(), Some("/private/tmp/tmux-501/default"));
        assert_eq!(id.session, "taskfleet");
        assert_eq!(id.window_id, "@42");
        // No pane_id in this event → None (back-compat / older create.sh).
        assert_eq!(id.pane_id, None);

        // Null socket is tolerated — session + window_id are the minimum.
        let d2 = serde_json::json!({
            "tmux_socket": null,
            "tmux_session": "taskfleet",
            "tmux_window_id": "@7",
        });
        let id2 = tmux_identity_from_data(&d2).expect("identity without socket");
        assert_eq!(id2.socket, None);
        assert_eq!(id2.window_id, "@7");

        // A create.sh that emits `tmux_pane_id` is folded into the identity.
        let d3 = serde_json::json!({
            "tmux_session": "taskfleet",
            "tmux_window_id": "@42",
            "tmux_pane_id": "%7",
        });
        let id3 = tmux_identity_from_data(&d3).expect("identity with pane");
        assert_eq!(id3.pane_id.as_deref(), Some("%7"));
        assert_eq!(id3.capture_target(), "%7");

        // Explicit `tmux_pane_id: null` (create.sh emits null when its pane
        // query failed) must fold to None — never `Some("null")`.
        let d4 = serde_json::json!({
            "tmux_session": "taskfleet",
            "tmux_window_id": "@42",
            "tmux_pane_id": null,
        });
        let id4 = tmux_identity_from_data(&d4).expect("identity with null pane");
        assert_eq!(id4.pane_id, None);
        assert_eq!(id4.capture_target(), "@42");
    }

    #[test]
    fn tmux_identity_from_data_back_compat_is_none() {
        // Legacy create.sh: no qualified fields at all.
        let legacy = serde_json::json!({ "tmux_window": "🚀 wt/x" });
        assert!(tmux_identity_from_data(&legacy).is_none());
        // Partial (window_id without session) is also insufficient → None.
        let partial = serde_json::json!({ "tmux_window_id": "@42" });
        assert!(tmux_identity_from_data(&partial).is_none());
    }

    /// End-to-end: a `node.created` event carrying the qualified fields folds
    /// them into `Node.tmux_identity`; one without them leaves it `None`.
    #[test]
    fn node_created_populates_tmux_identity() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let rid = RunId::parse_str(run_id).unwrap();
        let dir = crate::run_dir(tmp.path(), &rid);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = RunPaths::new(dir, run_id).unwrap();

        let mut ev = event(run_id);
        ev.seq = 2;
        ev.kind = "node.created".into();
        ev.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        ev.data = serde_json::json!({
            "kind": "spinoff",
            "tmux_window": "🚀 wt/x",
            "tmux_socket": "/private/tmp/tmux-501/default",
            "tmux_session": "taskfleet",
            "tmux_window_id": "@42",
        });
        apply_event(&paths, &ev).expect("node.created applies");
        let n = read_node_opt(&paths, &NodeId::parse_str("n-0001").unwrap())
            .unwrap()
            .unwrap();
        let id = n.tmux_identity.expect("qualified identity recorded");
        assert_eq!(id.session, "taskfleet");
        assert_eq!(id.window_id, "@42");
        assert_eq!(n.tmux_window.as_deref(), Some("🚀 wt/x"));

        // A second run with a legacy event leaves tmux_identity None.
        let run2 = "02jxsnap000000000000000000";
        let rid2 = RunId::parse_str(run2).unwrap();
        let dir2 = crate::run_dir(tmp.path(), &rid2);
        std::fs::create_dir_all(&dir2).unwrap();
        let paths2 = RunPaths::new(dir2, run2).unwrap();
        let mut ev2 = event(run2);
        ev2.seq = 2;
        ev2.kind = "node.created".into();
        ev2.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        ev2.data = serde_json::json!({ "kind": "spinoff", "tmux_window": "🚀 wt/y" });
        apply_event(&paths2, &ev2).expect("legacy node.created applies");
        let n2 = read_node_opt(&paths2, &NodeId::parse_str("n-0001").unwrap())
            .unwrap()
            .unwrap();
        assert!(n2.tmux_identity.is_none());
        assert_eq!(n2.tmux_window.as_deref(), Some("🚀 wt/y"));
    }

    #[test]
    fn node_materialization_populates_missing_manifest_source_branch() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000001";
        let rid = RunId::parse_str(run_id).unwrap();
        let dir = crate::run_dir(tmp.path(), &rid);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = RunPaths::new(dir, run_id).unwrap();

        let mut created = event(run_id);
        created.kind = "run.created".into();
        created.data = serde_json::json!({
            "kind": "spinoff", "lifecycle": "autonomous", "title": "t"
        });
        apply_event(&paths, &created).unwrap();
        assert!(read_manifest_opt(&paths)
            .unwrap()
            .unwrap()
            .source_branch
            .is_none());

        let mut node = event(run_id);
        node.seq = 2;
        node.kind = "node.created".into();
        node.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        node.data = serde_json::json!({
            "kind": "spinoff",
            "source_branch": "main",
            "worktree_path": "/tmp/wt/pending"
        });
        apply_event(&paths, &node).unwrap();

        let manifest = read_manifest_opt(&paths).unwrap().unwrap();
        assert_eq!(manifest.status, Status::Pending);
        assert_eq!(manifest.source_branch.as_deref(), Some("main"));
        let node = read_n0001(&paths);
        assert_eq!(node.worktree_path.as_deref(), Some("/tmp/wt/pending"));
    }

    #[test]
    fn node_materialization_preserves_explicit_manifest_source_branch() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000002";
        let rid = RunId::parse_str(run_id).unwrap();
        let dir = crate::run_dir(tmp.path(), &rid);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = RunPaths::new(dir, run_id).unwrap();

        let mut created = event(run_id);
        created.kind = "run.created".into();
        created.data = serde_json::json!({
            "kind": "spinoff", "lifecycle": "autonomous", "title": "t",
            "source_branch": "release"
        });
        apply_event(&paths, &created).unwrap();

        let mut node = event(run_id);
        node.seq = 2;
        node.kind = "node.created".into();
        node.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        node.data = serde_json::json!({
            "kind": "spinoff", "source_branch": "main"
        });
        apply_event(&paths, &node).unwrap();

        assert_eq!(
            read_manifest_opt(&paths)
                .unwrap()
                .unwrap()
                .source_branch
                .as_deref(),
            Some("release")
        );
    }

    /// Bootstrap a run with a single `n-0001` node via the event-sourced path,
    /// returning its paths. Shared by the supervisor-state replay tests below.
    fn seed_run_with_node(tmp: &TempDir, run_id: &str) -> RunPaths {
        let rid = RunId::parse_str(run_id).unwrap();
        let dir = crate::run_dir(tmp.path(), &rid);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = RunPaths::new(dir, run_id).unwrap();

        let mut created = event(run_id);
        created.kind = "run.created".into();
        created.data = serde_json::json!({
            "kind": "spinoff", "lifecycle": "autonomous", "title": "t"
        });
        apply_event(&paths, &created).expect("run.created applies");

        let mut node = event(run_id);
        node.seq = 2;
        node.kind = "node.created".into();
        node.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        node.data = serde_json::json!({ "kind": "spinoff" });
        apply_event(&paths, &node).expect("node.created applies");
        paths
    }

    fn read_n0001(paths: &RunPaths) -> Node {
        read_node_opt(paths, &NodeId::parse_str("n-0001").unwrap())
            .unwrap()
            .unwrap()
    }

    #[test]
    fn awaiting_input_clock_is_durable_first_write_wins_and_resolve_is_fenced() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxwd0000000000000000000w";
        let paths = seed_run_with_node(&tmp, run_id);
        let nid = Some(NodeId::parse_str("n-0001").unwrap());
        let opened_at: chrono::DateTime<Utc> = "2026-08-16T12:00:00Z".parse().unwrap();
        let mut open = event(run_id);
        open.seq = 3;
        open.ts = opened_at;
        open.kind = "node.awaiting_input".into();
        open.node_id = nid.clone();
        open.data = serde_json::json!({ "discussion_items": [{
            "topic": "Which scope?",
            "options": ["small", "large"],
            "recommended_default": "small"
        }] });
        apply_event(&paths, &open).unwrap();
        let first = read_n0001(&paths).awaiting_input.unwrap();
        assert_eq!(first.opened_at, opened_at);
        assert_eq!(first.event_seq, 3);

        // A duplicate/restarted supervisor observation cannot restart the clock.
        let mut duplicate = open.clone();
        duplicate.seq = 4;
        duplicate.ts = opened_at + chrono::Duration::hours(1);
        apply_event(&paths, &duplicate).unwrap();
        let still_first = read_n0001(&paths).awaiting_input.unwrap();
        assert_eq!(still_first.opened_at, opened_at);
        assert_eq!(still_first.event_seq, 3);

        // A stale timeout for another generation cannot clear this request.
        let mut stale = event(run_id);
        stale.seq = 5;
        stale.kind = "node.input_resolved".into();
        stale.node_id = nid.clone();
        stale.data = serde_json::json!({ "event_seq": 2 });
        apply_event(&paths, &stale).unwrap();
        assert!(read_n0001(&paths).awaiting_input.is_some());

        let mut resolved = stale;
        resolved.seq = 6;
        resolved.data = serde_json::json!({ "event_seq": 3 });
        apply_event(&paths, &resolved).unwrap();
        assert!(read_n0001(&paths).awaiting_input.is_none());
    }

    #[test]
    fn awaiting_input_rejects_missing_default_without_mutating_projection() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxwd0000000000000000000x";
        let paths = seed_run_with_node(&tmp, run_id);
        let mut open = event(run_id);
        open.seq = 3;
        open.kind = "node.awaiting_input".into();
        open.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        open.data = serde_json::json!({ "discussion_items": [{
            "topic": "Which scope?", "options": ["small", "large"]
        }] });
        assert!(reduce_event_to_ops(&paths, &open).is_err());
        assert!(read_n0001(&paths).awaiting_input.is_none());
    }

    #[test]
    fn awaiting_input_validation_is_state_independent_and_worker_exit_clears_it() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxwd0000000000000000000y";
        let paths = seed_run_with_node(&tmp, run_id);
        let nid = Some(NodeId::parse_str("n-0001").unwrap());

        let mut open = event(run_id);
        open.seq = 3;
        open.kind = "node.awaiting_input".into();
        open.node_id = nid.clone();
        open.data = serde_json::json!({ "discussion_items": [{
            "topic": "Which scope?", "options": ["small", "large"],
            "recommended_default": "small"
        }] });
        apply_event(&paths, &open).unwrap();

        let mut malformed_duplicate = open.clone();
        malformed_duplicate.seq = 4;
        malformed_duplicate.data = serde_json::json!({ "discussion_items": [] });
        assert!(reduce_event_to_ops(&paths, &malformed_duplicate).is_err());

        let mut exited = event(run_id);
        exited.seq = 5;
        exited.kind = "worker.exited".into();
        exited.node_id = nid;
        exited.data = serde_json::json!({ "exit_code": 0 });
        apply_event(&paths, &exited).unwrap();
        let node = read_n0001(&paths);
        assert!(node.awaiting_input.is_none());
        assert!(node.worker_exit.is_some());

        let mut delayed_open = open;
        delayed_open.seq = 6;
        assert!(reduce_event_to_ops(&paths, &delayed_open)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn input_resolved_requires_generation_even_when_nothing_is_open() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxwd0000000000000000000z";
        let paths = seed_run_with_node(&tmp, run_id);
        let mut resolved = event(run_id);
        resolved.seq = 3;
        resolved.kind = "node.input_resolved".into();
        resolved.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        resolved.data = serde_json::json!({});
        assert!(reduce_event_to_ops(&paths, &resolved).is_err());
    }

    #[test]
    fn awaiting_input_default_must_be_one_of_options() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxwd00000000000000000010";
        let paths = seed_run_with_node(&tmp, run_id);
        let mut open = event(run_id);
        open.seq = 3;
        open.kind = "node.awaiting_input".into();
        open.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        open.data = serde_json::json!({ "discussion_items": [{
            "topic": "Which scope?", "options": ["small", "large"],
            "recommended_default": "other"
        }] });
        assert!(reduce_event_to_ops(&paths, &open).is_err());
    }

    fn merge_started_event(run_id: &str, seq: u64, op_id: &str, expected: &str) -> Event {
        let mut ev = event(run_id);
        ev.seq = seq;
        ev.kind = KIND_MERGE_STARTED.into();
        ev.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        ev.data = serde_json::json!({
            "op_id": op_id,
            "source_branch": "main",
            "worker_branch": "wt/worker",
            "expected_source_oid": expected,
            "worker_oid": "cafebabecafebabecafebabecafebabecafebabe",
            "base_sha": null,
            "driver_pid": 4242,
            "driver_pid_start_secs": null,
            "started_at": "2026-08-15T00:00:00Z",
        });
        ev
    }

    /// `merge.started` records the in-flight transaction on `pending_merge`
    /// without transitioning the node's status.
    #[test]
    fn merge_started_records_pending_transaction() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let paths = seed_run_with_node(&tmp, run_id);

        apply_event(&paths, &merge_started_event(run_id, 3, "op-1", "aaa")).unwrap();
        let n = read_n0001(&paths);
        assert_eq!(
            n.status,
            Status::Pending,
            "recording a merge is not terminal"
        );
        let txn = n.pending_merge.expect("transaction recorded");
        assert_eq!(txn.op_id, "op-1");
        assert_eq!(txn.expected_source_oid, "aaa");
    }

    /// `merge.aborted` clears the pending transaction it names, leaving the node
    /// live; a stale abort for a different `op_id` is a clean no-op.
    #[test]
    fn merge_aborted_clears_matching_transaction_only() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let paths = seed_run_with_node(&tmp, run_id);
        apply_event(&paths, &merge_started_event(run_id, 3, "op-1", "aaa")).unwrap();

        // A stale abort for a different op_id does nothing.
        let mut stale = event(run_id);
        stale.seq = 4;
        stale.kind = KIND_MERGE_ABORTED.into();
        stale.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        stale.data = serde_json::json!({ "op_id": "op-OTHER", "reason": "x" });
        apply_event(&paths, &stale).unwrap();
        assert!(
            read_n0001(&paths).pending_merge.is_some(),
            "stale abort is a no-op"
        );

        // The matching abort clears it; the node stays live.
        let mut abort = event(run_id);
        abort.seq = 5;
        abort.kind = KIND_MERGE_ABORTED.into();
        abort.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        abort.data = serde_json::json!({ "op_id": "op-1", "reason": "no mutation" });
        apply_event(&paths, &abort).unwrap();
        let n = read_n0001(&paths);
        assert!(
            n.pending_merge.is_none(),
            "matching abort clears the transaction"
        );
        assert_eq!(n.status, Status::Pending, "abort does not terminalize");
    }

    /// A terminal `node.report` (the normal, no-crash completion) clears any
    /// pending merge transaction.
    #[test]
    fn terminal_report_clears_pending_merge() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let paths = seed_run_with_node(&tmp, run_id);
        apply_event(&paths, &merge_started_event(run_id, 3, "op-1", "aaa")).unwrap();

        let mut report = event(run_id);
        report.seq = 4;
        report.kind = "node.report".into();
        report.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        report.data = serde_json::json!({ "success": true, "via": "explicit-merge" });
        apply_event(&paths, &report).unwrap();
        let n = read_n0001(&paths);
        assert_eq!(n.status, Status::Done);
        assert!(
            n.pending_merge.is_none(),
            "completed merge clears the transaction"
        );
    }

    /// Regression (issue `retire-via-string`): the terminal-node adoption
    /// exception now keys on the typed `RunMerge` origin, NOT a forgeable `via`
    /// string. A late report against a `Failed` node that carries an `Agent`
    /// origin (as every `node report` self-submission does) plus a forged
    /// `via: "explicit-merge"` must NOT be adopted — the node stays `Failed`. A
    /// present-but-malformed origin is likewise not adopted. Only a genuine
    /// `RunMerge`-origin report (or a legacy report with NO origin field) is
    /// adopted and corrects the node to `Done`.
    #[test]
    fn late_merge_adoption_requires_run_merge_origin_not_forged_via() {
        let tmp = TempDir::new().unwrap();

        // Helper: seed a fresh run (distinct id), drive n-0001 to Failed, apply a
        // late report, and return the resulting node status.
        let drive = |run_id: &str, report_data: Value| -> Status {
            let paths = seed_run_with_node(&tmp, run_id);
            // Terminalize the node as Failed (a watchdog-synthesized failure).
            let mut fail = event(run_id);
            fail.seq = 3;
            fail.kind = "node.status".into();
            fail.node_id = Some(NodeId::parse_str("n-0001").unwrap());
            fail.data = serde_json::json!({ "status": "failed" });
            apply_event(&paths, &fail).unwrap();
            assert_eq!(read_n0001(&paths).status, Status::Failed);
            // The late report under test.
            let mut report = event(run_id);
            report.seq = 4;
            report.kind = "node.report".into();
            report.node_id = Some(NodeId::parse_str("n-0001").unwrap());
            report.data = report_data;
            apply_event(&paths, &report).unwrap();
            read_n0001(&paths).status
        };

        // Forged: Agent origin + a hand-set `via` — NOT adopted, stays Failed.
        let mut agent_forged = serde_json::json!({ "success": true, "via": "explicit-merge" });
        crate::ReportOrigin::Agent.stamp(&mut agent_forged);
        assert_eq!(
            drive("01jxsnap000000000000000001", agent_forged),
            Status::Failed,
            "an Agent-origin report with a forged via must not be adopted"
        );

        // Present-but-malformed origin + forged via — NOT adopted, stays Failed.
        let malformed = serde_json::json!({
            "success": true, "via": "explicit-merge", "origin": "garbage-not-an-object"
        });
        assert_eq!(
            drive("01jxsnap000000000000000002", malformed),
            Status::Failed,
            "a malformed origin must not re-unlock the legacy via adoption path"
        );

        // Genuine RunMerge origin (no `via` at all) — adopted, corrected to Done.
        let mut run_merge = serde_json::json!({ "success": true });
        crate::ReportOrigin::RunMerge {
            op_id: Some("op-1".into()),
            worker_oid: Some("cafebabe".into()),
        }
        .stamp(&mut run_merge);
        assert_eq!(
            drive("01jxsnap000000000000000003", run_merge),
            Status::Done,
            "a genuine RunMerge-origin report is adopted and corrects Failed→Done"
        );

        // Legacy report (no origin field) with `via` — still adopted (backward
        // compat with pre-typed-origin on-disk runs).
        let legacy = serde_json::json!({ "success": true, "via": "explicit-merge" });
        assert_eq!(
            drive("01jxsnap000000000000000004", legacy),
            Status::Done,
            "a legacy via-only report (no origin field) is still adopted"
        );
    }

    /// A terminal `node.status` (e.g. a watchdog-synthesized failure) clears any
    /// in-flight merge transaction, so `pending_merge` is never stranded on a
    /// terminal node where recovery would refuse to look (/llm-review finding).
    #[test]
    fn terminal_node_status_clears_pending_merge() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let paths = seed_run_with_node(&tmp, run_id);
        apply_event(&paths, &merge_started_event(run_id, 3, "op-1", "aaa")).unwrap();
        assert!(read_n0001(&paths).pending_merge.is_some());

        let mut status = event(run_id);
        status.seq = 4;
        status.kind = "node.status".into();
        status.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        status.data = serde_json::json!({ "status": "failed" });
        apply_event(&paths, &status).unwrap();
        let n = read_n0001(&paths);
        assert_eq!(n.status, Status::Failed);
        assert!(
            n.pending_merge.is_none(),
            "terminal status clears the transaction"
        );
    }

    /// Replaying `supervisor.attached` from scratch reproduces
    /// `Node.supervisor_pid` — the field is now event-sourced, not a
    /// projection-only write (issue `supervisor-state-not-event-sourced`).
    #[test]
    fn supervisor_attached_sets_supervisor_pid() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let paths = seed_run_with_node(&tmp, run_id);
        assert_eq!(read_n0001(&paths).supervisor_pid, None);

        let mut ev = event(run_id);
        ev.seq = 3;
        ev.kind = "supervisor.attached".into();
        ev.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        ev.data = serde_json::json!({ "pid": 47820 });
        apply_event(&paths, &ev).expect("supervisor.attached applies");
        assert_eq!(read_n0001(&paths).supervisor_pid, Some(47820));
    }

    /// A second attach with a different pid overrides (latest-wins); a replay
    /// of the *same* pid is a pure no-op that does not churn `updated_at`.
    #[test]
    fn supervisor_attached_latest_wins_and_idempotent_on_replay() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let paths = seed_run_with_node(&tmp, run_id);

        let mut ev = event(run_id);
        ev.kind = "supervisor.attached".into();
        ev.node_id = Some(NodeId::parse_str("n-0001").unwrap());

        ev.seq = 3;
        ev.data = serde_json::json!({ "pid": 100 });
        apply_event(&paths, &ev).expect("first attach applies");
        assert_eq!(read_n0001(&paths).supervisor_pid, Some(100));

        // A restart binds a fresh pid: latest-wins.
        ev.seq = 4;
        ev.data = serde_json::json!({ "pid": 200 });
        apply_event(&paths, &ev).expect("second attach applies");
        let after_second = read_n0001(&paths);
        assert_eq!(after_second.supervisor_pid, Some(200));

        // Replaying the latest event again is a no-op: the planned ops are
        // empty and the projection bytes (including `updated_at`) are unchanged.
        let ops = reduce_event_to_ops(&paths, &ev).expect("replay reduces cleanly");
        assert!(ops.is_empty(), "re-applying same pid must plan no ops");
        apply_event(&paths, &ev).expect("replay applies as no-op");
        assert_eq!(read_n0001(&paths).updated_at, after_second.updated_at);
    }

    /// Replaying `supervisor.cursor_advanced` from scratch reproduces
    /// `Node.last_processed_report_seq_by_child`.
    #[test]
    fn supervisor_cursor_advanced_sets_report_cursor() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let paths = seed_run_with_node(&tmp, run_id);
        let child = "02jxsnap000000000000000000";

        let mut ev = event(run_id);
        ev.seq = 3;
        ev.kind = "supervisor.cursor_advanced".into();
        ev.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        ev.data = serde_json::json!({ "child_run_id": child, "report_seq": 7 });
        apply_event(&paths, &ev).expect("cursor_advanced applies");

        let n = read_n0001(&paths);
        assert_eq!(
            n.last_processed_report_seq_by_child.get(child),
            Some(&Value::from(7u64))
        );
    }

    /// The cursor is monotonic and idempotent: re-applying the same
    /// `(child_run_id, report_seq)` is a no-op, an older seq never moves the
    /// cursor backward, and a higher seq advances it. A second distinct child
    /// gets its own independent entry.
    #[test]
    fn supervisor_cursor_advanced_is_monotonic_and_idempotent() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let paths = seed_run_with_node(&tmp, run_id);
        let child_a = "02jxsnap000000000000000000";
        let child_b = "03jxsnap000000000000000000";

        let mut ev = event(run_id);
        ev.kind = "supervisor.cursor_advanced".into();
        ev.node_id = Some(NodeId::parse_str("n-0001").unwrap());

        ev.seq = 3;
        ev.data = serde_json::json!({ "child_run_id": child_a, "report_seq": 5 });
        apply_event(&paths, &ev).expect("seq 5 applies");

        // Replay the exact same event — no-op, plans zero ops.
        let ops = reduce_event_to_ops(&paths, &ev).expect("replay reduces cleanly");
        assert!(ops.is_empty(), "re-applying same cursor must plan no ops");

        // An older seq must not move the cursor backward.
        ev.seq = 4;
        ev.data = serde_json::json!({ "child_run_id": child_a, "report_seq": 3 });
        let ops = reduce_event_to_ops(&paths, &ev).expect("older seq reduces cleanly");
        assert!(ops.is_empty(), "older seq must plan no ops");
        apply_event(&paths, &ev).expect("older seq applies as no-op");
        assert_eq!(
            read_n0001(&paths)
                .last_processed_report_seq_by_child
                .get(child_a),
            Some(&Value::from(5u64))
        );

        // A higher seq advances; an independent child gets its own entry.
        ev.seq = 5;
        ev.data = serde_json::json!({ "child_run_id": child_a, "report_seq": 9 });
        apply_event(&paths, &ev).expect("higher seq applies");
        ev.seq = 6;
        ev.data = serde_json::json!({ "child_run_id": child_b, "report_seq": 1 });
        apply_event(&paths, &ev).expect("second child applies");

        let n = read_n0001(&paths);
        assert_eq!(
            n.last_processed_report_seq_by_child.get(child_a),
            Some(&Value::from(9u64))
        );
        assert_eq!(
            n.last_processed_report_seq_by_child.get(child_b),
            Some(&Value::from(1u64))
        );
    }

    /// Both new kinds reject a malformed payload at the reducer boundary so a
    /// forged event can never write a corrupt projection.
    #[test]
    fn supervisor_state_events_reject_malformed_payloads() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let paths = seed_run_with_node(&tmp, run_id);
        let nid = Some(NodeId::parse_str("n-0001").unwrap());

        // Missing pid.
        let mut ev = event(run_id);
        ev.seq = 3;
        ev.kind = "supervisor.attached".into();
        ev.node_id = nid.clone();
        ev.data = serde_json::json!({});
        assert!(matches!(
            reduce_event_to_ops(&paths, &ev),
            Err(Error::CorruptEventLog { .. })
        ));

        // Missing envelope node_id.
        ev.node_id = None;
        ev.data = serde_json::json!({ "pid": 1 });
        assert!(matches!(
            reduce_event_to_ops(&paths, &ev),
            Err(Error::CorruptEventLog { .. })
        ));

        // cursor_advanced: malformed child_run_id.
        let mut ev2 = event(run_id);
        ev2.seq = 4;
        ev2.kind = "supervisor.cursor_advanced".into();
        ev2.node_id = nid.clone();
        ev2.data = serde_json::json!({ "child_run_id": "../etc", "report_seq": 1 });
        assert!(matches!(
            reduce_event_to_ops(&paths, &ev2),
            Err(Error::CorruptEventLog { .. })
        ));

        // cursor_advanced: missing report_seq.
        ev2.data = serde_json::json!({ "child_run_id": "02jxsnap000000000000000000" });
        assert!(matches!(
            reduce_event_to_ops(&paths, &ev2),
            Err(Error::CorruptEventLog { .. })
        ));
    }

    /// The append gate stays fail-closed after the 0.2 cut added
    /// `Kind`'s `#[serde(other)]` catch-all: a `run.created` / `node.created`
    /// whose `kind` is a removed kind (`code`, …) or plain garbage must still be
    /// rejected as `CorruptEventLog`, NOT silently accepted as `Kind::Unknown`.
    /// (Legacy runs are never re-created through the reducer — their manifest is
    /// read directly from disk via the permissive `Kind::Unknown` decode.)
    #[test]
    fn removed_or_garbage_kind_in_created_events_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let rid = RunId::parse_str(run_id).unwrap();
        let dir = crate::run_dir(tmp.path(), &rid);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = RunPaths::new(dir, run_id).unwrap();

        for bad in ["code", "orchestrate", "bugfix", "make-skill", "garbage"] {
            let mut ev = event(run_id);
            ev.kind = "run.created".into();
            ev.node_id = None;
            ev.data = serde_json::json!({ "kind": bad, "lifecycle": "autonomous", "title": "t" });
            assert!(
                matches!(
                    reduce_event_to_ops(&paths, &ev),
                    Err(Error::CorruptEventLog { .. })
                ),
                "run.created with kind {bad:?} must be rejected, not folded to Unknown"
            );
        }

        // A surviving creatable kind still folds cleanly (guards against a
        // false positive that rejects everything).
        let mut ok = event(run_id);
        ok.kind = "run.created".into();
        ok.node_id = None;
        ok.data = serde_json::json!({ "kind": "spinoff", "lifecycle": "autonomous", "title": "t" });
        assert!(reduce_event_to_ops(&paths, &ok).is_ok());
    }

    /// Snapshot every projection file under `paths` to a `path → inode` map.
    ///
    /// An atomic projection write is temp-file + rename, so a rewritten file
    /// always lands a *fresh inode* — even when its bytes are byte-for-byte
    /// identical (e.g. a manifest op that refreshes `updated_at` to the same
    /// timestamp). Comparing inodes therefore detects every write the reducer
    /// makes, with no false negatives a content diff would suffer. `events.jsonl`
    /// and `.lock` are excluded: `apply_event` never touches them.
    #[cfg(unix)]
    fn projection_inodes(paths: &RunPaths) -> std::collections::BTreeMap<PathBuf, u64> {
        use std::os::unix::fs::MetadataExt;
        let mut consider = vec![paths.manifest()];
        for dir in [paths.nodes_dir()] {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for ent in rd.flatten() {
                    let p = ent.path();
                    if p.extension().and_then(|s| s.to_str()) == Some("json") {
                        consider.push(p);
                    }
                }
            }
        }
        let mut map = std::collections::BTreeMap::new();
        for p in consider {
            if let Ok(md) = std::fs::symlink_metadata(&p) {
                if md.file_type().is_file() {
                    map.insert(p, md.ino());
                }
            }
        }
        map
    }

    /// The exhaustive parity guarantee `projected-paths-into-reducer` requires:
    /// for an event applied against a given state, the paths
    /// [`plan_projections`] reports MUST equal the files [`apply_event`]
    /// actually writes. Plan first (against pre-apply state), apply, then diff
    /// the projection inodes — a file is "written" iff it is newly present or
    /// its inode changed. `expect_writes` guards the test itself: when set, the
    /// touched set must be non-empty, so a kind that silently stopped writing
    /// can't pass by matching an empty plan against an empty diff.
    #[cfg(unix)]
    fn assert_plan_matches_apply(paths: &RunPaths, ev: &Event, expect_writes: bool) {
        use std::collections::BTreeSet;
        let before = projection_inodes(paths);
        let planned: BTreeSet<PathBuf> = plan_projections(paths, ev)
            .unwrap_or_else(|e| panic!("plan_projections({}) errored: {e:?}", ev.kind))
            .into_iter()
            .collect();
        apply_event(paths, ev)
            .unwrap_or_else(|e| panic!("apply_event({}) errored: {e:?}", ev.kind));
        let after = projection_inodes(paths);
        let touched: BTreeSet<PathBuf> = after
            .iter()
            .filter(|(p, ino)| before.get(*p) != Some(*ino))
            .map(|(p, _)| p.clone())
            .collect();
        assert_eq!(
            planned, touched,
            "kind={}: plan_projections must name exactly the files apply_event writes",
            ev.kind
        );
        if expect_writes {
            assert!(
                !touched.is_empty(),
                "kind={}: expected this event to write at least one projection",
                ev.kind
            );
        }
    }

    /// Drive every event kind through a dependency-ordered lifecycle on real
    /// runs, asserting plan/apply parity at each step. Covers the writing kinds
    /// (run/node/supervisor/child) in states where they
    /// project, plus the no-op kinds (audit records, `supervisor.exited`,
    /// terminal-guarded transitions) where both the plan and the apply touch
    /// nothing.
    #[cfg(unix)]
    #[test]
    fn plan_projections_matches_apply_for_every_kind() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let rid = RunId::parse_str(run_id).unwrap();
        let dir = crate::run_dir(tmp.path(), &rid);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = RunPaths::new(dir, run_id).unwrap();
        let nid = || Some(NodeId::parse_str("n-0001").unwrap());
        let child = "02jxsnap000000000000000000";

        // Helper to build a fresh envelope at a monotonic seq.
        let mut next_seq = 0u64;
        let mut at = |kind: &str, node_id, data| {
            next_seq += 1;
            Event {
                ts: Utc::now(),
                seq: next_seq,
                kind: kind.into(),
                run_id: rid.clone(),
                node_id,
                idempotency_key: None,
                data,
            }
        };

        // run.created → manifest.json
        assert_plan_matches_apply(
            &paths,
            &at(
                "run.created",
                None,
                serde_json::json!({ "kind": "spinoff", "lifecycle": "autonomous", "title": "t" }),
            ),
            true,
        );
        // run.status (pending → running) → manifest.json
        assert_plan_matches_apply(
            &paths,
            &at(
                "run.status",
                None,
                serde_json::json!({ "status": "running" }),
            ),
            true,
        );
        // node.created → nodes/n-0001.json + manifest.json
        assert_plan_matches_apply(
            &paths,
            &at(
                "node.created",
                nid(),
                serde_json::json!({ "kind": "spinoff" }),
            ),
            true,
        );
        // node.status (pending → running) → nodes/n-0001.json
        assert_plan_matches_apply(
            &paths,
            &at(
                "node.status",
                nid(),
                serde_json::json!({ "status": "running" }),
            ),
            true,
        );
        // supervisor.attached → nodes/n-0001.json (still non-terminal)
        assert_plan_matches_apply(
            &paths,
            &at(
                "supervisor.attached",
                nid(),
                serde_json::json!({ "pid": 4242 }),
            ),
            true,
        );
        // supervisor.cursor_advanced → nodes/n-0001.json
        assert_plan_matches_apply(
            &paths,
            &at(
                "supervisor.cursor_advanced",
                nid(),
                serde_json::json!({ "child_run_id": child, "report_seq": 3 }),
            ),
            true,
        );
        // child.spawned → nodes/n-0001.json (parent node)
        assert_plan_matches_apply(
            &paths,
            &at(
                "child.spawned",
                nid(),
                serde_json::json!({ "child_run_id": child, "child_node_id": "n-0001" }),
            ),
            true,
        );
        // node.report success → nodes/n-0001.json (now terminal)
        assert_plan_matches_apply(
            &paths,
            &at("node.report", nid(), serde_json::json!({ "success": true })),
            true,
        );
        // Terminal-guarded no-ops: a settled node swallows further transitions,
        // so both the plan and the apply touch nothing.
        assert_plan_matches_apply(
            &paths,
            &at(
                "node.status",
                nid(),
                serde_json::json!({ "status": "failed" }),
            ),
            false,
        );
        // No-op audit / lifecycle kinds: zero projections by design.
        for kind in [
            "supervisor.exited",
            "orchestrator.decision",
            "discuss.critical",
            "cleanup.window_missing",
        ] {
            assert_plan_matches_apply(&paths, &at(kind, None, serde_json::json!({})), false);
        }
    }

    /// A `worker.exited` carrying a clean `exit_code: 0` folds onto the node's
    /// `worker_exit` field as a clean exit — and does NOT transition `status`
    /// (terminalization is the supervisor's decision via the typed table).
    #[test]
    fn worker_exited_records_clean_exit_without_transitioning_status() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let paths = bootstrap_retry_node(&tmp, run_id);

        let mut ev = event(run_id);
        ev.seq = 3;
        ev.kind = "worker.exited".into();
        ev.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        ev.data = serde_json::json!({ "exit_code": 0 });
        apply_event(&paths, &ev).expect("worker.exited applies");

        let n = read_node_opt(&paths, &NodeId::parse_str("n-0001").unwrap())
            .unwrap()
            .unwrap();
        let exit = n.worker_exit.expect("worker_exit recorded");
        assert_eq!(exit.code, Some(0));
        assert_eq!(exit.signal, None);
        assert!(exit.is_clean());
        assert_eq!(
            n.status,
            Status::Pending,
            "the exit fact never transitions status"
        );
    }

    /// A `worker.exited` carrying a `signal` records it as a failure; and the fold
    /// is first-write-wins — a replayed/duplicate exit event never overwrites the
    /// first recorded fact (replay-safety for the `applied_seq` watermark).
    #[test]
    fn worker_exited_records_signal_and_is_first_write_wins() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let paths = bootstrap_retry_node(&tmp, run_id);
        let nid = NodeId::parse_str("n-0001").unwrap();

        let mut ev = event(run_id);
        ev.seq = 3;
        ev.kind = "worker.exited".into();
        ev.node_id = Some(nid.clone());
        ev.data = serde_json::json!({ "signal": 9 });
        apply_event(&paths, &ev).expect("worker.exited applies");

        let n = read_node_opt(&paths, &nid).unwrap().unwrap();
        let exit = n.worker_exit.expect("worker_exit recorded");
        assert_eq!(exit.signal, Some(9));
        assert!(exit.is_failure());

        // A later, conflicting exit event (e.g. a replay of a different value) is a
        // clean no-op: the first fact stands.
        let mut dup = event(run_id);
        dup.seq = 4;
        dup.kind = "worker.exited".into();
        dup.node_id = Some(nid.clone());
        dup.data = serde_json::json!({ "exit_code": 0 });
        apply_event(&paths, &dup).expect("duplicate worker.exited applies as no-op");
        let n2 = read_node_opt(&paths, &nid).unwrap().unwrap();
        assert_eq!(
            n2.worker_exit.unwrap().signal,
            Some(9),
            "first-write-wins: the replayed exit must not overwrite the recorded fact"
        );
    }

    /// A `worker.exited` carrying neither `exit_code` nor `signal` is malformed —
    /// the reducer is the canonical gate and rejects it as `CorruptEventLog` rather
    /// than record an empty fact.
    #[test]
    fn worker_exited_without_code_or_signal_is_corrupt() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let paths = bootstrap_retry_node(&tmp, run_id);

        let mut ev = event(run_id);
        ev.seq = 3;
        ev.kind = "worker.exited".into();
        ev.node_id = Some(NodeId::parse_str("n-0001").unwrap());
        ev.data = serde_json::json!({});
        match reduce_event_to_ops(&paths, &ev) {
            Err(Error::CorruptEventLog { .. }) => {}
            Ok(_) => panic!("an empty worker.exited payload must be rejected, not applied"),
            Err(other) => panic!("expected CorruptEventLog, got {other:?}"),
        }

        // Carrying BOTH is contradictory (a process cannot both return a code and
        // be killed) — also rejected.
        ev.data = serde_json::json!({ "exit_code": 0, "signal": 9 });
        match reduce_event_to_ops(&paths, &ev) {
            Err(Error::CorruptEventLog { .. }) => {}
            Ok(_) => panic!("a worker.exited with both fields must be rejected"),
            Err(other) => panic!("expected CorruptEventLog, got {other:?}"),
        }
    }

    /// `node.death_observed` records the residual crash backstop's first-death
    /// anchor (`first_death_at`) as `ev.ts`, is **first-write-wins** (a later
    /// re-observation never resets the monotonic anchor), and is a no-op against a
    /// terminal node (the backstop is moot once settled). Issue
    /// `typed-supervisor-outcomes`.
    #[test]
    fn node_death_observed_records_first_death_first_write_wins() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let paths = bootstrap_retry_node(&tmp, run_id);
        let nid = NodeId::parse_str("n-0001").unwrap();

        let mut ev = event(run_id);
        ev.seq = 3;
        ev.kind = "node.death_observed".into();
        ev.node_id = Some(nid.clone());
        ev.data = serde_json::json!({});
        apply_event(&paths, &ev).expect("node.death_observed applies");
        let first = read_node_opt(&paths, &nid)
            .unwrap()
            .unwrap()
            .first_death_at
            .expect("first_death_at recorded");
        assert_eq!(first, ev.ts, "the anchor is the event's own timestamp");

        // A later re-observation is first-write-wins: the monotonic anchor holds.
        let mut later = event(run_id);
        later.seq = 4;
        later.kind = "node.death_observed".into();
        later.node_id = Some(nid.clone());
        later.ts = ev.ts + chrono::Duration::seconds(30);
        later.data = serde_json::json!({});
        apply_event(&paths, &later).expect("re-observation applies as no-op");
        assert_eq!(
            read_node_opt(&paths, &nid).unwrap().unwrap().first_death_at,
            Some(first),
            "first-write-wins: a re-observation must not reset the anchor"
        );
    }

    /// `node.death_observed` is a no-op once a higher-fidelity fact exists — here a
    /// told `worker.exited` — so a from-scratch replay converges to the same state
    /// the supervisor's lock-guarded emitter would produce (the backstop is moot
    /// once the shim recorded a real exit). Issue `typed-supervisor-outcomes`.
    #[test]
    fn node_death_observed_noop_when_worker_exit_present() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let paths = bootstrap_retry_node(&tmp, run_id);
        let nid = NodeId::parse_str("n-0001").unwrap();

        // A told exit lands first.
        let mut exit = event(run_id);
        exit.seq = 3;
        exit.kind = "worker.exited".into();
        exit.node_id = Some(nid.clone());
        exit.data = serde_json::json!({ "exit_code": 0 });
        apply_event(&paths, &exit).unwrap();

        // A death observation for the same node folds to nothing.
        let mut death = event(run_id);
        death.seq = 4;
        death.kind = "node.death_observed".into();
        death.node_id = Some(nid.clone());
        death.data = serde_json::json!({});
        apply_event(&paths, &death).expect("applies as no-op");
        assert_eq!(
            read_node_opt(&paths, &nid).unwrap().unwrap().first_death_at,
            None,
            "a told worker.exited makes the crash backstop moot; no anchor recorded"
        );
    }

    /// `node.retry` clears the previous attempt's told exit fact: the re-spawned
    /// worker is a NEW process, so a stale `worker_exit` must not carry over (it
    /// would make the supervisor mis-judge the fresh attempt from the dead one's
    /// exit). Issue `thin-exit-status-launcher`.
    #[test]
    fn node_retry_clears_worker_exit() {
        let tmp = TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let paths = bootstrap_retry_node(&tmp, run_id);
        let nid = NodeId::parse_str("n-0001").unwrap();

        // Record a failing exit on the first attempt.
        let mut exit = event(run_id);
        exit.seq = 3;
        exit.kind = "worker.exited".into();
        exit.node_id = Some(nid.clone());
        exit.data = serde_json::json!({ "exit_code": 7 });
        apply_event(&paths, &exit).unwrap();
        assert!(read_node_opt(&paths, &nid)
            .unwrap()
            .unwrap()
            .worker_exit
            .is_some());

        // Retry re-spawns the node — the stale exit fact must be gone.
        let mut retry = event(run_id);
        retry.seq = 4;
        retry.kind = "node.retry".into();
        retry.node_id = Some(nid.clone());
        retry.data = serde_json::json!({
            "attempt": 1,
            "reason": "agent-died",
            "branch": "wt/foo",
            "worktree_path": "/tmp/new-wt",
            "agent_pid": 222,
        });
        apply_event(&paths, &retry).unwrap();

        let n = read_node_opt(&paths, &nid).unwrap().unwrap();
        assert!(
            n.worker_exit.is_none(),
            "node.retry must clear the previous attempt's worker_exit"
        );
        assert_eq!(
            n.status,
            Status::Pending,
            "retry returns the node to Pending"
        );
    }
}
