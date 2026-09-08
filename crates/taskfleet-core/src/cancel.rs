//! Single-lock run cancellation.
//!
//! `run cancel` does three things under **one** held [`RunLock`]: refuse a run
//! that is already in a non-cancelled terminal state, synthesize a terminal
//! `node.report` for every still-live node, and append `run.status: cancelled`
//! once. Holding one lock for the whole operation serializes it against other
//! *cooperating* writers (those that honor the lock) so the node reads and the
//! node-report appends can't interleave — which is what made the pre-refactor
//! CLI loop both racy and prone to over-reporting `cancelled_nodes` (it pushed
//! a node id even when the per-node append landed after another process had
//! already settled the node, so the reducer dropped it). Under one lock the
//! node we read is the node we cancel, so the reported count is honest.
//!
//! This is **not crash-atomic**: each `append_and_apply_unlocked` is its own
//! durable append, so a crash or I/O error partway through can leave some nodes
//! cancelled and `run.status` not yet appended. Recovery is convergent — a
//! re-`cancel` of an already-`Cancelled` run scans the still-live stragglers
//! and finishes the job — not transactional rollback.
//!
//! Two consistency properties beyond the single lock:
//!
//! - **Enumeration *and per-node liveness* are from the event log, not the
//!   projection directory.** The node set and each node's current status are
//!   both replayed from `events.jsonl` (the source of truth) in one streaming
//!   pass rather than scanned from `nodes/*.json`. A `node.created` can be
//!   appended+fsynced while its projection write is crash-interrupted
//!   (`events.rs` documents the log leading the projections); a `nodes/` scan
//!   would silently drop that node, mark the run `cancelled`, and let a future
//!   `rebuild_projections` resurrect it as live under a `Cancelled` run. Walking
//!   the log closes that window. Crucially, replaying `node.status` / `node.report`
//!   to derive each node's status (rather than trusting `read_node_opt`) closes a
//!   second window: a *non-cancel* terminal event (e.g. a `node.report`
//!   `success: true`) fsynced but not yet folded leaves a stale-live projection,
//!   and a projection-derived liveness check would over-write that node with a
//!   fresh cancel that diverges on rebuild. The log-derived status settles it as
//!   already-terminal instead — the log wins. (The manifest's `node_count` is
//!   *also* a projection written in the same interrupted fold, so it is no more
//!   authoritative than `nodes/` — and it carries no node ids — which is why we
//!   replay the log rather than trust the counter.)
//!
//! - **Each synthesized event carries a deterministic idempotency key**
//!   (`run-cancel:<run_id>:node:<node_id>` and `run-cancel:<run_id>:run-status`).
//!   If a crash lands an append+fsync but interrupts the projection fold, the
//!   node/run still reads non-terminal, so a re-`cancel` would append a *second*
//!   logical-cancel event (duplicating it for auditors, metrics, and rebuild).
//!   The prior cancel events (scoped by `(kind, key)` for this run) are captured
//!   in the same replay pass, so instead of re-appending, the loop **re-folds
//!   the already-logged event** via [`apply_event`](crate::reducer) — converging a projection
//!   the crash left non-terminal *without* a duplicate log line (a re-fold is a
//!   clean no-op when the projection already agrees). The whole transaction is
//!   then both non-duplicating and projection-convergent.
//!
//! The cancel ledger is built by a *streaming* pass: [`for_each_event_probe`](crate::events)
//! walks `events.jsonl` line by line, parsing only the small envelope + status
//! fields each line needs and materializing a full [`Event`] payload solely for
//! the handful of lines in this run's `run-cancel:<run_id>:` key namespace (the
//! events the re-fold path replays). The whole log is never held in memory, so
//! lock-hold time and peak memory stay bounded even for a run with hundreds of
//! nodes and multi-KB `node.report` payloads.
//!
//! What is still *not* derived from the log here: **run-level** liveness (the
//! terminal-refusal check and `run_was_already_cancelled`) is read from the
//! manifest projection, with the prior-cancel re-fold converging a crash-stranded
//! `run.status`. Deriving the run status from the log too would conflate a
//! crash-stranded `run.status: cancelled` (manifest stale, must re-fold and
//! report a *fresh* cancel) with an already-folded one, since the log is
//! identical in both cases — so the manifest read stays authoritative for the
//! run-level decision, exactly as the per-node convergence path consults
//! `read_node_opt` only to tell those two cases apart.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::events::{append_and_apply_unlocked, excerpt, for_each_event_probe};
use crate::lock::{LockedRun, RunLock};
use crate::paths::RunPaths;
use crate::projections::{read_manifest, read_node_opt};
use crate::reducer::apply_event;
use crate::report::ReportOrigin;
use crate::schema::{Event, NodeId, RunId, Status};

/// Outcome of a [`cancel_run`] transaction. Lets a thin CLI wrapper report
/// honestly what actually changed: which live nodes it converged, which were
/// already settled (skipped, not double-reported), and whether the run itself
/// was already cancelled (a convergence-only no-op rather than a fresh cancel).
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub struct CancelOutcome {
    /// True when the run's manifest was already `Cancelled` on entry, so no
    /// `run.status: cancelled` event was appended. The call still scans and
    /// converges any straggler nodes (an interrupted earlier cancel), so this
    /// is a SUCCESS, not an error: "no-op: run was already cancelled,
    /// converged N additional nodes".
    pub run_was_already_cancelled: bool,
    /// Nodes this cancel transaction ensured are terminally cancelled: live
    /// nodes for which it synthesized and durably appended a terminal cancel
    /// `node.report` (and folded it), plus any node whose cancel `node.report` a
    /// prior interrupted cancel had already durably appended (matched by
    /// `(kind, idempotency_key)`) and which this call converged by *re-folding*
    /// that event rather than re-appending. Either way the node carries a
    /// terminal cancel in the source-of-truth log and its projection is folded
    /// (or, for a still-missing projection, will fold on rebuild — see the
    /// module docs); none is double-reported against a node that was already
    /// terminal on entry.
    pub nodes_cancelled: Vec<NodeId>,
    /// Nodes whose *status* was already terminal on entry and so were skipped —
    /// never double-reported as freshly cancelled.
    pub nodes_already_terminal: Vec<NodeId>,
}

/// Cancel a run in a single locked transaction. Acquires the run's
/// [`RunLock`] once for the whole operation, then delegates to
/// [`cancel_run_unlocked`].
///
/// # Errors
///
/// - [`Error::RunAlreadyTerminal`] if the run is `Done`/`Failed` — refused
///   without mutating state.
/// - I/O / corrupt-log errors from reading the manifest, listing nodes, or
///   appending events.
pub fn cancel_run(paths: &RunPaths, note: Option<&str>) -> Result<CancelOutcome> {
    RunLock::with_lock(paths, |lock| cancel_run_unlocked(lock, paths, note))
}

/// The locked body of [`cancel_run`]. The `lock: &LockedRun` witness proves the
/// caller already holds the run's exclusive [`RunLock`]; this is the sanctioned
/// lock-held composition path so the manifest read, the per-node
/// read-then-append loop, and the final `run.status` append all share one
/// critical section (it calls [`append_and_apply_unlocked`], never
/// [`crate::append_and_apply_event`], which would deadlock by re-locking).
pub fn cancel_run_unlocked(
    lock: &LockedRun<'_>,
    paths: &RunPaths,
    note: Option<&str>,
) -> Result<CancelOutcome> {
    let started = std::time::Instant::now();
    let manifest = read_manifest(paths)?;

    // Refuse a non-cancelled terminal run BEFORE touching any node: cancelling
    // a Done/Failed run would synthesize node reports and append a
    // `run.status: cancelled` the reducer's terminal-state guard then drops,
    // so the CLI would claim a transition that never happened. An already-
    // `Cancelled` run is not refused — it falls through to converge stragglers.
    if manifest.status.is_terminal() && manifest.status != Status::Cancelled {
        return Err(Error::RunAlreadyTerminal {
            status: manifest.status,
        });
    }
    let run_was_already_cancelled = manifest.status == Status::Cancelled;

    // Normalize the cancel reason ONCE up front (see [`normalize_cancel_reason`]):
    // a blank `--note` would flow in as `reason: ""`, which the reducer rejects
    // and would brick the run's cancellability. It falls back to the default.
    let reason = normalize_cancel_reason(note);

    // One streaming replay pass over the source-of-truth log: the authoritative
    // node set *and each node's current status* (both immune to the projection
    // crash window), plus the prior cancel events already recorded (so a prior
    // interrupted cancel isn't duplicated — it is re-folded instead).
    let CancelLedger {
        node_status,
        prior_cancel,
    } = read_cancel_ledger(paths)?;

    let mut nodes_cancelled = Vec::new();
    let mut nodes_already_terminal = Vec::new();

    for (nid, log_status) in node_status {
        let key = node_cancel_key(&paths.run_id, &nid);
        // Convergence path first: this run's cancel already logged a
        // `node.report` for this node (a prior, possibly crash-interrupted,
        // cancel). The log is identical whether that report's projection fold
        // landed or not, so `read_node_opt` is what tells the two apart — an
        // already-folded terminal projection is a clean no-op reported as
        // already-terminal, while a crash-stranded still-live projection is
        // converged by re-folding the already-logged event (no duplicate append)
        // and reported as cancelled. This is the only remaining projection read,
        // and it serves convergence, not the liveness decision below.
        if let Some(prior) = prior_cancel.get(&("node.report".to_owned(), key.clone())) {
            if let Some(n) = read_node_opt(paths, &nid)? {
                if n.status.is_terminal() {
                    nodes_already_terminal.push(nid);
                    continue;
                }
            }
            apply_event(paths, prior)?;
            nodes_cancelled.push(nid);
            continue;
        }
        // No prior cancel for this node: the event log is authoritative for
        // liveness. A node the log replays as terminal — a non-cancel terminal
        // (`node.report success` / a `node.status` to a terminal value), or a
        // cancel logged outside this run's key namespace — is already settled
        // and skipped, even if a stale projection still reads live (the window
        // cancel-liveness-from-log closes: the log wins). Only a node the log
        // shows non-terminal (including a `node.created` whose projection write
        // was interrupted — the crash window a `nodes/*.json` scan would drop)
        // gets a synthesized terminal cancel report so the log records it as
        // cancelled and a future rebuild can't resurrect it as live.
        if log_status.is_terminal() {
            nodes_already_terminal.push(nid);
            continue;
        }
        let data = json!({
            "success": false,
            "cancelled": true,
            "reason": reason,
            "summary": "Run cancelled before agent reported.",
            "discussion_items": [],
            "spinoff_proposals": [],
            "wrap_up_recommendations": []
        });
        append_and_apply_unlocked(lock, paths, "node.report", Some(&nid), Some(&key), data)?;
        nodes_cancelled.push(nid);
    }

    if !run_was_already_cancelled {
        let key = run_status_cancel_key(&paths.run_id);
        if let Some(prior) = prior_cancel.get(&("run.status".to_owned(), key.clone())) {
            // A prior interrupted cancel already logged the terminal `run.status`
            // (fsynced before its manifest fold). Re-fold it to converge the
            // manifest instead of appending a duplicate `run.status: cancelled`.
            apply_event(paths, prior)?;
        } else {
            let mut status_data = serde_json::Map::new();
            status_data.insert("status".into(), "cancelled".into());
            // Record the operator note only when one was actually supplied (the
            // trimmed, non-blank value); a blank `--note` leaves the field unset
            // rather than writing an empty string.
            if let Some(n) = note.map(str::trim).filter(|s| !s.is_empty()) {
                status_data.insert("note".into(), n.into());
            }
            append_and_apply_unlocked(
                lock,
                paths,
                "run.status",
                None,
                Some(&key),
                serde_json::Value::Object(status_data),
            )?;
        }
    }

    tracing::debug!(
        target: "taskfleet_core::cancel",
        run_id = %paths.run_id,
        held_ms = started.elapsed().as_millis() as u64,
        nodes_cancelled = nodes_cancelled.len(),
        nodes_already_terminal = nodes_already_terminal.len(),
        "cancel transaction complete",
    );

    Ok(CancelOutcome {
        run_was_already_cancelled,
        nodes_cancelled,
        nodes_already_terminal,
    })
}

/// Outcome of a [`cancel_node`] transaction — a single-node, branch-preserving
/// cancel for one live fan-out child.
///
/// Per-node cancel deliberately leaves the run non-terminal *while any sibling is
/// still live* (design §2.5 — a stuck child is unblocked without killing the
/// batch). But when this cancel settles the **last** live node, the run is rolled
/// up **in the same locked transaction** ([`rolled_up`](Self::rolled_up)) rather
/// than deferred to the supervisor — so a run whose supervisor has died is never
/// stranded non-terminal (llm-review C1).
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub struct NodeCancelOutcome {
    /// The node this call targeted (fully resolved).
    pub node_id: NodeId,
    /// True when this call ensured the node carries a terminal cancel in the
    /// source-of-truth log — either by synthesizing and durably appending a fresh
    /// cancel `node.report`, or by re-folding a prior interrupted cancel's
    /// already-logged event (crash convergence, no duplicate append). False when
    /// the node was already terminal on entry (see `already_terminal`).
    pub cancelled: bool,
    /// True when the node was *already* terminal on entry (merged, failed, or a
    /// prior cancel already folded) — a clean idempotent no-op, never a fresh
    /// cancel. Mutually exclusive with `cancelled`.
    pub already_terminal: bool,
    /// `Some(status)` when this cancel settled the last live node and therefore
    /// rolled the whole run up to a terminal status **under the same lock**
    /// (`Cancelled` when no sibling failed, `Failed` when one did). `None` when
    /// siblings remain live (the run stays live) or the run was already terminal
    /// on entry. Lets the CLI tell the operator whether the run itself is now
    /// settled rather than implying a rollup that might never come.
    pub rolled_up: Option<Status>,
}

/// Cancel exactly ONE live node of a run, preserving its branch + worktree.
/// Acquires the run's [`RunLock`] once and delegates to
/// [`cancel_node_unlocked`].
///
/// This is the fan-out selectivity primitive (design §2.5, issue
/// `per-node-run`): where [`cancel_run`] settles every live node and rolls the
/// run up to `Cancelled` in one shot, this settles a single named node. While
/// any sibling is still live it appends **only** that node's terminal cancel
/// `node.report` (no `run.status`) — the run stays live so the batch keeps
/// running. When it settles the **last** live node it also rolls the run up to a
/// terminal status **in the same locked transaction** (so a dead supervisor can
/// never strand the run non-terminal — llm-review C1). The synthesized terminal
/// cancel `node.report` classifies as [`Cancelled`](crate::Status) →
/// `Teardown::SourceRelative`, so invariant 5 preserves the node's committed work
/// rather than force-deleting it.
///
/// # Errors
///
/// - [`Error::RunAlreadyTerminal`] if the run is `Done`/`Failed` — refused
///   without mutating state (mirrors [`cancel_run`]; an already-`Cancelled` run
///   is *not* refused — its live nodes can still be settled).
/// - [`Error::NodeNotFound`] if `node_id` names no node in the run's log.
/// - I/O / corrupt-log errors from reading the manifest, replaying the log, or
///   appending the report.
pub fn cancel_node(
    paths: &RunPaths,
    node_id: &NodeId,
    note: Option<&str>,
) -> Result<NodeCancelOutcome> {
    RunLock::with_lock(paths, |lock| {
        cancel_node_unlocked(lock, paths, node_id, note)
    })
}

/// The locked body of [`cancel_node`]. The `lock: &LockedRun` witness proves the
/// caller already holds the run's exclusive [`RunLock`], so the manifest read,
/// the log replay, the convergence read, the single report append, AND the
/// optional last-node roll-up all share one critical section (it calls
/// [`append_and_apply_unlocked`], never [`crate::append_and_apply_event`], which
/// would deadlock by re-locking).
pub fn cancel_node_unlocked(
    lock: &LockedRun<'_>,
    paths: &RunPaths,
    node_id: &NodeId,
    note: Option<&str>,
) -> Result<NodeCancelOutcome> {
    // Refuse a non-cancelled terminal run up front, mirroring `cancel_run`: a
    // `Done`/`Failed` run's nodes are all terminal, so appending a fresh cancel
    // `node.report` would either bloat the log with a dead post-terminal event or
    // (on rebuild) flip a settled node — divergence. An already-`Cancelled` run
    // is NOT refused: it may still carry a straggler live node to settle
    // (llm-review C4).
    let manifest = read_manifest(paths)?;
    if manifest.status.is_terminal() && manifest.status != Status::Cancelled {
        return Err(Error::RunAlreadyTerminal {
            status: manifest.status,
        });
    }

    // One streaming replay pass over the source-of-truth log gives the
    // authoritative node set *and* each node's log-derived status (both immune to
    // the projection crash window), plus this run's already-logged cancel events
    // so a prior interrupted cancel is re-folded, never duplicated.
    let CancelLedger {
        node_status,
        prior_cancel,
    } = read_cancel_ledger(paths)?;

    // The log is authoritative for the node set: a node whose `node.created` was
    // fsynced but whose projection write was crash-interrupted is still
    // resolvable here (a `nodes/*.json` scan would miss it). A genuinely absent
    // id is a caller error.
    let log_status = node_status
        .iter()
        .find(|(nid, _)| nid == node_id)
        .map(|(_, s)| *s)
        .ok_or_else(|| Error::NodeNotFound {
            node_id: node_id.as_str().to_owned(),
        })?;

    let key = node_cancel_key(&paths.run_id, node_id);

    // Settle the target node. `cancelled` = this call ensured a terminal cancel
    // in the log (fresh append or crash-convergence re-fold); `already_terminal`
    // = the node was already terminal on entry (idempotent no-op).
    let (cancelled, already_terminal) =
        if let Some(prior) = prior_cancel.get(&("node.report".to_owned(), key.clone())) {
            // Convergence path: this run's cancel already logged a `node.report`
            // for this node (a prior, possibly crash-interrupted, per-node or
            // whole-run cancel). The log is identical whether that report's
            // projection fold landed or not, so `read_node_opt` tells the two
            // apart — an already-folded terminal projection is a clean no-op,
            // while a crash-stranded still-live projection is converged by
            // re-folding the already-logged event (no duplicate append).
            let folded_terminal =
                read_node_opt(paths, node_id)?.is_some_and(|n| n.status.is_terminal());
            if folded_terminal {
                (false, true)
            } else {
                apply_event(paths, prior)?;
                (true, false)
            }
        } else if log_status.is_terminal() {
            // No prior cancel: the log is authoritative for liveness. A node the
            // log replays as terminal — a natural success/failure, or a cancel
            // logged outside this run's key namespace — is already settled, even
            // if a stale projection still reads live (the log wins).
            (false, true)
        } else {
            let reason = normalize_cancel_reason(note);
            let data = json!({
                "success": false,
                "cancelled": true,
                "reason": reason,
                "summary": "Node cancelled before agent reported.",
                "discussion_items": [],
                "spinoff_proposals": [],
                "wrap_up_recommendations": []
            });
            append_and_apply_unlocked(lock, paths, "node.report", Some(node_id), Some(&key), data)?;
            (true, false)
        };

    // Last-node roll-up (llm-review C1): if the run is still non-terminal but
    // every node is now terminal, terminalize the run HERE, under the same lock,
    // rather than deferring to the supervisor — which may be dead, leaving the
    // run stranded `pending` with no live node. The aggregate is derived from the
    // log-authoritative `node_status` (with the target's post-cancel status
    // overridden), so it never mis-terminalizes over a stale projection. The
    // decision runs only when NO sibling is live, so the "don't terminalize while
    // a sibling runs" invariant holds.
    let rolled_up = maybe_roll_up_run(
        lock,
        paths,
        manifest.status,
        &node_status,
        node_id,
        cancelled,
        log_status,
        &prior_cancel,
    )?;

    Ok(NodeCancelOutcome {
        node_id: node_id.clone(),
        cancelled,
        already_terminal,
        rolled_up,
    })
}

/// Roll the run up to a terminal status when this per-node cancel settled the
/// last live node. Returns the status appended, or `None` when the run stays
/// live (a sibling is still live) or was already terminal on entry.
///
/// Shares the whole-run cancel's `run-cancel:<run>:run-status` idempotency key
/// (via [`run_status_cancel_key`]) so the three run-status producers in the
/// cancel family — whole-run cancel, this last-node roll-up, and a crash-retry of
/// either — converge on ONE logical `run.status` rather than duplicating it: a
/// prior interrupted append captured in `prior_cancel` is re-folded, and a later
/// `cancel_run` finds the same key and skips (llm-review C2/C4). The supervisor's
/// own `supervise::cleanup::rollup_status` uses a different key, but it fires only
/// while the manifest is non-terminal, so once this append lands the supervisor's
/// tick reads the run terminal and no-ops.
#[allow(clippy::too_many_arguments)]
fn maybe_roll_up_run(
    lock: &LockedRun<'_>,
    paths: &RunPaths,
    run_status_on_entry: Status,
    node_status: &[(NodeId, Status)],
    target: &NodeId,
    target_cancelled: bool,
    target_log_status: Status,
    prior_cancel: &HashMap<(String, String), Event>,
) -> Result<Option<Status>> {
    if run_status_on_entry.is_terminal() {
        // The run was already terminal on entry (an already-`Cancelled` run whose
        // straggler we just settled) — nothing to roll up.
        return Ok(None);
    }
    // The target's effective post-cancel status: `Cancelled` if this call settled
    // it, else its log-derived status (an already-terminal node).
    let effective_target = if target_cancelled {
        Status::Cancelled
    } else {
        target_log_status
    };
    let statuses = node_status
        .iter()
        .map(|(nid, s)| if nid == target { effective_target } else { *s });
    let Some(agg) = crate::aggregate_terminal_status(statuses) else {
        // A sibling is still live — the run stays live, as designed.
        return Ok(None);
    };

    let key = run_status_cancel_key(&paths.run_id);
    if let Some(prior) = prior_cancel.get(&("run.status".to_owned(), key.clone())) {
        // A prior interrupted cancel already logged the terminal `run.status`
        // (fsynced before its manifest fold). Re-fold it to converge the manifest
        // instead of appending a duplicate.
        apply_event(paths, prior)?;
    } else {
        // `Status` serializes kebab-case (`cancelled`/`failed`/`done`) — the
        // exact shape the reducer's `run.status` handler parses.
        append_and_apply_unlocked(
            lock,
            paths,
            "run.status",
            None,
            Some(&key),
            json!({ "status": agg }),
        )?;
    }
    Ok(Some(agg))
}

/// Normalize a `--note` into the terminal cancel report's `reason`. An empty or
/// whitespace-only note would otherwise flow in as `reason: ""`, which the
/// reducer rejects (`CancelledRequiresReason`) — aborting the transaction and,
/// since a retry reuses the same bad note, leaving the node/run permanently
/// un-cancellable. A blank note falls back to the default.
fn normalize_cancel_reason(note: Option<&str>) -> &str {
    note.map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("cancelled by user")
}

/// Cancel-relevant facts replayed from `events.jsonl` in one streaming pass
/// under the held lock.
struct CancelLedger {
    /// Every node a `node.created` event introduced, paired with the status the
    /// log replays for it, deduped and sorted by numeric suffix. The
    /// authoritative live-node set *and* per-node liveness: replayed from the
    /// source of truth, so it includes a node whose projection write was
    /// crash-interrupted (the node a `nodes/*.json` scan would miss) and reports
    /// a node terminal whenever the log says so even if the projection still
    /// reads live (the window [`crate::events`] documents the log leading the
    /// projections through).
    node_status: Vec<(NodeId, Status)>,
    /// Cancel events this run already logged, keyed by `(kind, idempotency_key)`
    /// and limited to this run's `run-cancel:<run_id>:` key namespace (first
    /// occurrence wins, mirroring [`crate::events::find_prior_with_key`]). The
    /// cancel loop looks an entry up by its deterministic key to (a) avoid
    /// re-appending a duplicate and (b) re-fold the event so a crash-stranded
    /// projection converges. Keying by `(kind, key)` — not the bare string —
    /// keeps a coincidental or forged key on an unrelated `kind` from masking a
    /// real cancel append. Only these few lines have their full [`Event`] payload
    /// materialized; every other line is skimmed envelope-only.
    prior_cancel: HashMap<(String, String), Event>,
}

/// Envelope + the few small `data` fields the cancel ledger needs from each
/// line, skimmed by [`for_each_event_probe`] without materializing the
/// (potentially multi-KB) full `data` payload. serde ignores every other field,
/// so a rich `node.report` is scanned but never allocated.
#[derive(Deserialize)]
struct EventSeqProbe {
    seq: u64,
}

#[derive(Deserialize)]
struct CancelProbe {
    seq: u64,
    kind: String,
    #[serde(default)]
    node_id: Option<NodeId>,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    data: CancelProbeData,
}

/// The status-bearing `data` fields of `node.status` / `node.report`. All
/// optional: any other event kind simply leaves them `None`.
#[derive(Deserialize, Default)]
struct CancelProbeData {
    // Keep these as raw JSON values. Besides matching the reducer's strict
    // interpretation exactly (for example, an advisory non-string `via` does
    // not invalidate a typed RunMerge origin), this lets a bounded replay skip
    // future malformed report fields without deserializing them into a typed
    // shape that could reject an earlier recovery event.
    #[serde(default)]
    status: Option<Value>,
    #[serde(default)]
    success: Option<Value>,
    #[serde(default)]
    cancelled: Option<Value>,
    #[serde(default)]
    via: Option<Value>,
    #[serde(default)]
    origin: OriginProbe,
}

#[derive(Default)]
struct OriginProbe {
    present: bool,
    value: Value,
}

impl<'de> Deserialize<'de> for OriginProbe {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self {
            present: true,
            value: Value::deserialize(deserializer)?,
        })
    }
}

impl CancelProbeData {
    fn report_value(&self) -> Value {
        let mut report = serde_json::Map::new();
        if let Some(success) = &self.success {
            report.insert("success".into(), success.clone());
        }
        if let Some(cancelled) = &self.cancelled {
            report.insert("cancelled".into(), cancelled.clone());
        }
        if let Some(via) = &self.via {
            report.insert("via".into(), via.clone());
        }
        if self.origin.present {
            report.insert("origin".into(), self.origin.value.clone());
        }
        Value::Object(report)
    }
}

/// Replay `events.jsonl` once, streaming, to build the [`CancelLedger`].
///
/// Reads through [`RunPaths::checked_events`] so a symlinked event log is
/// refused, matching the mutation path — the cancel decision must not be made
/// from content redirected outside the run tree. Uses [`for_each_event_probe`],
/// which shares the crate's torn-tail policy: a crash-truncated final line is
/// dropped as an uncommitted partial write, while any *interior* unparseable
/// line is surfaced as [`Error::CorruptEventLog`] — so a corrupt log fails the
/// cancel loudly rather than silently dropping a node. A missing log yields an
/// empty ledger (run never appended an event — nothing to cancel).
///
/// Per-node status is accumulated by the shared [`NodeStatusAcc`] state machine
/// (which mirrors the reducer's terminal-state guard) — the same accumulator the
/// supervisor's log-authoritative [`read_node_statuses`] uses, so the cancel path
/// and the supervisor roll-up can never derive a different node set or status
/// from the same log. Node ids come out sorted by numeric suffix.
fn read_cancel_ledger(paths: &RunPaths) -> Result<CancelLedger> {
    let events_path = paths.checked_events()?;
    let prefix = format!("run-cancel:{}:", paths.run_id.as_str());
    let mut acc = NodeStatusAcc::default();
    let mut prior_cancel: HashMap<(String, String), Event> = HashMap::new();

    for_each_event_probe::<CancelProbe, _>(&events_path, |probe, raw| {
        acc.observe(&probe);
        // Capture only this run's cancel events, keyed by (kind, key), and only
        // for those materialize the full payload the re-fold path needs.
        if let Some(key) = probe
            .idempotency_key
            .as_deref()
            .filter(|k| k.starts_with(&prefix))
        {
            let entry = (probe.kind.clone(), key.to_owned());
            if let std::collections::hash_map::Entry::Vacant(slot) = prior_cancel.entry(entry) {
                let ev: Event =
                    serde_json::from_slice(raw).map_err(|e| Error::CorruptEventLog {
                        path: events_path.clone(),
                        reason: format!(
                            "cancel ledger: line matched a run-cancel key but is not a \
                         replayable event: {} [{e}]",
                            excerpt(raw)
                        ),
                    })?;
                slot.insert(ev);
            }
        }
        Ok(())
    })?;

    Ok(CancelLedger {
        node_status: acc.finish(),
        prior_cancel,
    })
}

/// The log-authoritative per-node status set for a run, replayed once from
/// `events.jsonl` — the source-of-truth alternative to a `nodes/*.json`
/// projection scan.
///
/// Returns every node a `node.created` event introduced, paired with the status
/// the log replays for it, deduped and sorted by numeric suffix ([`NodeId`]
/// order). Both the node set and each node's status come from the log, so the
/// result includes a node whose `node.created` was fsynced while its projection
/// write was crash-interrupted (the node a `nodes/*.json` scan would silently
/// drop) and reports a node terminal whenever the log says so even if its
/// projection still reads live (the window [`crate::events`] documents the log
/// leading the projections through).
///
/// This is what lets a supervisor's run-status roll-up stay log-authoritative:
/// terminalizing a run from the projection subset can miss a log-visible live
/// node and roll the run up while it is still running, which a later
/// `rebuild_projections` would then resurrect as live under a terminal run
/// (violating "a run must not terminalize while a log-visible node is live" —
/// issue `rollup-status-log-authoritative`). Feeding this into
/// [`aggregate_terminal_status`](crate::aggregate_terminal_status) closes that
/// window. It is the read half [`cancel_node`]'s in-lock self-roll-up already
/// uses via the cancel ledger; both now share the `NodeStatusAcc` state machine
/// so the supervisor tick and the cancel path can never diverge.
///
/// Reads through `RunPaths::checked_events` (a symlinked log is refused) and
/// shares the crate's streaming torn-tail policy: a crash-truncated final line
/// is dropped, an interior unparseable line surfaces as
/// [`Error::CorruptEventLog`], and a missing log yields an empty set.
///
/// **Cost.** One streaming pass over the whole log — `O(total events)`, not
/// `O(nodes)`. Memory stays bounded (each line is skimmed envelope + a few small
/// status fields, never the full `node.report` payload — a run with hundreds of
/// nodes and multi-KB reports is scanned without ever holding a report in
/// memory), but the *work* is linear in the event count, not the node count. A
/// caller that polls this every tick (the supervisor roll-up) re-reads from byte
/// 0 each time; at the tool's scale (tens of nodes, hundreds of events,
/// multi-second ticks) that is negligible, but an incremental fold that resumes
/// from the last consumed offset would be the optimization if a run's log ever
/// grows large enough to matter.
///
/// # Errors
///
/// I/O errors reading the log, a rejected symlinked path, or an interior corrupt
/// event line.
pub fn read_node_statuses(paths: &RunPaths) -> Result<Vec<(NodeId, Status)>> {
    Ok(read_node_status_facts(paths, None)?
        .into_iter()
        .map(|fact| (fact.node_id, fact.status))
        .collect())
}

/// One node's log-derived terminal facts. `confirmed_merge_seq` is present only
/// when the shared terminal-recovery predicate actually adopted that report;
/// seeing an authoritative-looking report elsewhere in the log is insufficient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeStatusFact {
    /// Node whose facts were folded.
    pub node_id: NodeId,
    /// Current status after applying the reducer-equivalent transition rules.
    pub status: Status,
    /// Sequence of the authoritative merge report adopted for this node.
    pub confirmed_merge_seq: Option<u64>,
}

/// Replay node statuses and adopted merge authority, optionally considering
/// only events strictly before `before_seq`. The bound is load-bearing for
/// reducer replay: a future merge report must never authorize an earlier
/// `run.status` event.
pub fn read_node_status_facts(
    paths: &RunPaths,
    before_seq: Option<u64>,
) -> Result<Vec<NodeStatusFact>> {
    let events_path = paths.checked_events()?;
    let mut acc = NodeStatusAcc::default();
    for_each_event_probe::<EventSeqProbe, _>(&events_path, |seq_probe, raw| {
        if before_seq.is_none_or(|bound| seq_probe.seq < bound) {
            let probe: CancelProbe =
                serde_json::from_slice(raw).map_err(|e| Error::CorruptEventLog {
                    path: events_path.clone(),
                    reason: format!(
                        "node-status fold: event before bound is malformed: {} [{e}]",
                        excerpt(raw)
                    ),
                })?;
            acc.observe(&probe);
        }
        Ok(())
    })?;
    Ok(acc.finish_facts())
}

/// Streaming accumulator for log-derived per-node status: the shared state
/// machine behind both the cancel ledger ([`read_cancel_ledger`]) and the
/// supervisor's log-authoritative roll-up ([`read_node_statuses`]), so the two
/// can never derive a different node set or status from the same log.
///
/// Mirrors the reducer's terminal-state guard exactly: `node.created` seeds
/// [`Status::Pending`] (idempotent on replay — a second `node.created` for the
/// same id is a no-op, matching the reducer's existence guard), and
/// `node.status` / `node.report` transition a node only while it is still
/// non-terminal. A `node.status` / `node.report` for an id never introduced by a
/// `node.created` is ignored, exactly as the reducer no-ops a status/report
/// against a non-existent node. Malformed status fields degrade gracefully (the
/// node is left at its current status — for the cancel path that keeps the node
/// non-terminal, hence cancellable) rather than aborting — the append path
/// already validates every committed event, so a `None` transition only arises
/// from a hand-corrupted log.
///
/// **`node.retry` is deliberately not folded, and that is exact.** In the reducer
/// (`reduce_node_retry`) a retry against an already-terminal node is a no-op (a
/// settled node is frozen) and a retry against a *live* node only rewires it back
/// to `Pending`. So `node.retry` never crosses the terminal/live boundary in
/// either direction: a node that is terminal here is terminal in the reducer, and
/// one that is live here (whatever its exact non-terminal value) is live there.
/// Since the only consumers ([`read_node_statuses`] → `aggregate_terminal_status`,
/// and the cancel loop) classify solely on terminal-vs-live, ignoring
/// `node.retry` derives the same answer the reducer would — it is not a missed
/// transition.
#[derive(Default)]
struct NodeStatusAcc {
    /// Node ids in creation order (re-sorted by numeric suffix at [`finish`]).
    order: Vec<NodeId>,
    /// Each node's current log-derived status.
    status: HashMap<NodeId, Status>,
    /// Sequence of the confirmed merge report that the shared recovery
    /// predicate adopted for this node.
    confirmed_merge_seq: HashMap<NodeId, u64>,
}

impl NodeStatusAcc {
    /// Fold one skimmed event line ([`CancelProbe`]) into the per-node map.
    fn observe(&mut self, probe: &CancelProbe) {
        match probe.kind.as_str() {
            "node.created" => {
                if let Some(nid) = &probe.node_id {
                    if !self.status.contains_key(nid) {
                        self.order.push(nid.clone());
                        self.status.insert(nid.clone(), Status::Pending);
                    }
                }
            }
            "node.status" => {
                if let Some(nid) = &probe.node_id {
                    if let Some(cur) = self.status.get_mut(nid) {
                        if !cur.is_terminal() {
                            if let Some(ns) = probe
                                .data
                                .status
                                .as_ref()
                                .and_then(Value::as_str)
                                .and_then(parse_status)
                            {
                                *cur = ns;
                            }
                        }
                    }
                }
            }
            "node.report" => {
                if let Some(nid) = &probe.node_id {
                    if let Some(cur) = self.status.get_mut(nid) {
                        let report = probe.data.report_value();
                        if cur.is_terminal() {
                            // Keep this exactly aligned with `reduce_node_report`:
                            // only an authoritative successful merge may repair
                            // Failed/Done, and cancellation is immutable.
                            if ReportOrigin::permits_terminal_merge_recovery(*cur, &report) {
                                *cur = Status::Done;
                                self.confirmed_merge_seq.insert(nid.clone(), probe.seq);
                            }
                        } else if let Some(ns) = report_terminal_status(
                            probe.data.success.as_ref().and_then(Value::as_bool),
                            probe.data.cancelled.as_ref().and_then(Value::as_bool),
                        ) {
                            *cur = ns;
                            if ns == Status::Done
                                && ReportOrigin::report_is_confirmed_merge(&report)
                            {
                                self.confirmed_merge_seq.insert(nid.clone(), probe.seq);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Consume into the sorted `(NodeId, Status)` list. Node ids are sorted by
    /// numeric suffix (not lexically), so a run past the digit-width boundary
    /// where `n-10000` would otherwise sort before `n-9999` stays intuitive (see
    /// [`NodeId`]). A validated `NodeId` is `n-` + ASCII digits (≤10, so it fits
    /// in u64); the `unwrap_or` keeps the sort total for a hypothetical
    /// unparseable body.
    fn finish(self) -> Vec<(NodeId, Status)> {
        self.finish_facts()
            .into_iter()
            .map(|fact| (fact.node_id, fact.status))
            .collect()
    }

    fn finish_facts(mut self) -> Vec<NodeStatusFact> {
        self.order.sort_by_key(|id| {
            id.as_str()
                .strip_prefix("n-")
                .and_then(|d| d.parse::<u64>().ok())
                .unwrap_or(0)
        });
        self.order
            .into_iter()
            .map(|id| NodeStatusFact {
                status: self.status[&id],
                confirmed_merge_seq: self.confirmed_merge_seq.get(&id).copied(),
                node_id: id,
            })
            .collect()
    }
}

/// Parse a `node.status` / `run.status` status string into a [`Status`],
/// returning `None` for an unrecognized value (treated as "no transition" so a
/// corrupt status never aborts the cancel). Goes through serde so the kebab-case
/// mapping can never drift from the [`Status`] enum.
fn parse_status(s: &str) -> Option<Status> {
    serde_json::from_value(Value::String(s.to_owned())).ok()
}

/// Derive the terminal status a `node.report` asserts from its `success` /
/// `cancelled` flags, mirroring the reducer's success-XOR-cancelled rule but
/// *lenient*: a bare/contradictory report yields `None` (no transition — the
/// node stays live and is cancelled) rather than the reducer's
/// [`Error::CorruptEventLog`]. The append path rejects such a report before it
/// is ever committed against a live node, so a `None` here only arises from a
/// hand-corrupted log, where leaving the node cancellable is the safe default.
fn report_terminal_status(success: Option<bool>, cancelled: Option<bool>) -> Option<Status> {
    if cancelled.unwrap_or(false) {
        // `cancelled: true` with `success: true` is contradictory → no transition.
        if success == Some(true) {
            return None;
        }
        Some(Status::Cancelled)
    } else {
        match success {
            Some(true) => Some(Status::Done),
            Some(false) => Some(Status::Failed),
            None => None,
        }
    }
}

/// Deterministic idempotency key for the synthesized cancel `node.report` of
/// one node. Stable in `(run_id, node_id)` so a re-`cancel` after a crash that
/// fsynced the report but never folded its projection finds the prior event and
/// does not append a duplicate logical-cancel.
fn node_cancel_key(run_id: &RunId, node_id: &NodeId) -> String {
    format!("run-cancel:{}:node:{}", run_id.as_str(), node_id.as_str())
}

/// Deterministic idempotency key for the run's terminal `run.status: cancelled`
/// event. Stable in `run_id` for the same crash-retry reason as
/// [`node_cancel_key`].
fn run_status_cancel_key(run_id: &RunId) -> String {
    format!("run-cancel:{}:run-status", run_id.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{append_and_apply_event, append_event_with_seq, read_all_events};
    use crate::lock::ACQUIRE_COUNT;
    use tempfile::TempDir;

    /// Count `node.report` events recorded in the log for one node id.
    fn report_count(paths: &RunPaths, nid: &str) -> usize {
        read_all_events(&paths.events())
            .unwrap()
            .iter()
            .filter(|e| {
                e.kind == "node.report" && e.node_id.as_ref().map(NodeId::as_str) == Some(nid)
            })
            .count()
    }

    fn fresh_run(tmp: &TempDir) -> RunPaths {
        let run_id = "01jxsnap000000000000000000";
        let dir = tmp.path().join(run_id);
        std::fs::create_dir_all(&dir).unwrap();
        RunPaths::new(dir, run_id).unwrap()
    }

    /// Parse a `NodeId` for a test append call (the typed envelope id).
    fn nid(s: &str) -> NodeId {
        NodeId::parse_str(s).unwrap()
    }

    /// Drive a run to `count` live nodes (n-0001..) under a created manifest.
    fn bootstrap(paths: &RunPaths, count: usize) {
        append_and_apply_event(
            paths,
            "run.created",
            None,
            None,
            json!({ "kind": "spinoff", "lifecycle": "autonomous", "title": "t" }),
        )
        .unwrap();
        for i in 1..=count {
            let node_id = nid(&format!("n-{i:04}"));
            append_and_apply_event(
                paths,
                "node.created",
                Some(&node_id),
                None,
                json!({ "kind": "spinoff" }),
            )
            .unwrap();
        }
    }

    fn node_status(paths: &RunPaths, nid: &str) -> Status {
        let id = NodeId::parse_str(nid).unwrap();
        crate::read_node(paths, &id).unwrap().status
    }

    #[test]
    fn cancel_running_run_converges_live_nodes_and_settles_run() {
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 2);

        let out = cancel_run(&paths, Some("stop")).unwrap();
        assert!(!out.run_was_already_cancelled);
        assert_eq!(
            out.nodes_cancelled
                .iter()
                .map(NodeId::as_str)
                .collect::<Vec<_>>(),
            vec!["n-0001", "n-0002"]
        );
        assert!(out.nodes_already_terminal.is_empty());
        assert_eq!(node_status(&paths, "n-0001"), Status::Cancelled);
        assert_eq!(
            crate::read_manifest(&paths).unwrap().status,
            Status::Cancelled
        );
    }

    #[test]
    fn cancel_done_run_is_refused_without_mutation() {
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 1);
        // Settle the single node, then the run, to Done.
        append_and_apply_event(
            &paths,
            "node.report",
            Some(&nid("n-0001")),
            None,
            json!({ "success": true }),
        )
        .unwrap();
        append_and_apply_event(
            &paths,
            "run.status",
            None,
            None,
            json!({ "status": "done" }),
        )
        .unwrap();
        let before = read_all_events(&paths.events()).unwrap().len();

        let err = cancel_run(&paths, None).unwrap_err();
        assert!(
            matches!(
                err,
                Error::RunAlreadyTerminal {
                    status: Status::Done
                }
            ),
            "got {err:?}"
        );
        assert_eq!(
            read_all_events(&paths.events()).unwrap().len(),
            before,
            "a refused cancel must not append any event"
        );
        assert_eq!(crate::read_manifest(&paths).unwrap().status, Status::Done);
    }

    #[test]
    fn recancel_cancelled_run_converges_straggler_node() {
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 2);
        // Simulate an interrupted cancel: run is Cancelled, but n-0002 is still
        // live (its node.report never landed).
        append_and_apply_event(
            &paths,
            "node.report",
            Some(&nid("n-0001")),
            None,
            json!({ "success": false, "cancelled": true, "reason": "x" }),
        )
        .unwrap();
        append_and_apply_event(
            &paths,
            "run.status",
            None,
            None,
            json!({ "status": "cancelled" }),
        )
        .unwrap();
        assert_eq!(node_status(&paths, "n-0002"), Status::Pending);

        let out = cancel_run(&paths, None).unwrap();
        assert!(out.run_was_already_cancelled);
        assert_eq!(
            out.nodes_cancelled
                .iter()
                .map(NodeId::as_str)
                .collect::<Vec<_>>(),
            vec!["n-0002"],
            "only the straggler converges"
        );
        assert_eq!(
            out.nodes_already_terminal
                .iter()
                .map(NodeId::as_str)
                .collect::<Vec<_>>(),
            vec!["n-0001"]
        );
        assert_eq!(node_status(&paths, "n-0002"), Status::Cancelled);
    }

    #[test]
    fn recancel_fully_converged_run_is_a_clean_noop() {
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 1);
        let _ = cancel_run(&paths, None).unwrap(); // first cancel converges everything
        let before = read_all_events(&paths.events()).unwrap().len();

        let out = cancel_run(&paths, None).unwrap();
        assert!(out.run_was_already_cancelled);
        assert!(out.nodes_cancelled.is_empty(), "nothing left to converge");
        assert_eq!(
            out.nodes_already_terminal
                .iter()
                .map(NodeId::as_str)
                .collect::<Vec<_>>(),
            vec!["n-0001"]
        );
        assert_eq!(
            read_all_events(&paths.events()).unwrap().len(),
            before,
            "a fully-converged re-cancel appends nothing"
        );
    }

    #[test]
    fn already_terminal_node_is_not_over_reported() {
        // The honesty guard: a node already settled (terminal) on entry is
        // reported under `nodes_already_terminal`, never `nodes_cancelled`,
        // even though it sits in nodes/ alongside a live node.
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 2);
        // n-0001 finishes on its own (Done) before the cancel.
        append_and_apply_event(
            &paths,
            "node.report",
            Some(&nid("n-0001")),
            None,
            json!({ "success": true }),
        )
        .unwrap();

        let out = cancel_run(&paths, None).unwrap();
        assert_eq!(
            out.nodes_cancelled
                .iter()
                .map(NodeId::as_str)
                .collect::<Vec<_>>(),
            vec!["n-0002"]
        );
        assert_eq!(
            out.nodes_already_terminal
                .iter()
                .map(NodeId::as_str)
                .collect::<Vec<_>>(),
            vec!["n-0001"]
        );
        assert_eq!(
            node_status(&paths, "n-0001"),
            Status::Done,
            "Done node untouched"
        );
        assert_eq!(node_status(&paths, "n-0002"), Status::Cancelled);
    }

    #[test]
    fn cancel_run_with_no_nodes_dir_settles_run_only() {
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        append_and_apply_event(
            &paths,
            "run.created",
            None,
            None,
            json!({ "kind": "spinoff", "lifecycle": "autonomous", "title": "t" }),
        )
        .unwrap();

        let out = cancel_run(&paths, None).unwrap();
        assert!(!out.run_was_already_cancelled);
        assert!(out.nodes_cancelled.is_empty());
        assert!(out.nodes_already_terminal.is_empty());
        assert_eq!(
            crate::read_manifest(&paths).unwrap().status,
            Status::Cancelled
        );
    }

    #[test]
    fn blank_note_falls_back_to_default_reason_and_does_not_brick_cancel() {
        // A `--note ""` (or whitespace-only) must NOT flow an empty `reason`
        // into the synthesized report — that would be rejected by the reducer
        // mid-loop and leave the run permanently un-cancellable. It normalizes
        // to the default reason and the cancel completes cleanly.
        for blank in ["", "   ", "\n\t"] {
            let tmp = TempDir::new().unwrap();
            let paths = fresh_run(&tmp);
            bootstrap(&paths, 1);

            let out = cancel_run(&paths, Some(blank)).unwrap();
            assert_eq!(
                out.nodes_cancelled
                    .iter()
                    .map(NodeId::as_str)
                    .collect::<Vec<_>>(),
                vec!["n-0001"],
                "blank note {blank:?} still converges the live node"
            );
            assert_eq!(node_status(&paths, "n-0001"), Status::Cancelled);
            let report = crate::read_node(&paths, &NodeId::parse_str("n-0001").unwrap())
                .unwrap()
                .last_report
                .expect("cancel report recorded");
            assert_eq!(report["reason"], "cancelled by user");
        }
    }

    #[test]
    fn nodes_are_converged_in_numeric_not_lexical_order() {
        // Past the digit-width boundary, lexical order would place n-10000
        // before n-9999. The numeric sort keeps the reported order intuitive.
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        append_and_apply_event(
            &paths,
            "run.created",
            None,
            None,
            json!({ "kind": "spinoff", "lifecycle": "autonomous", "title": "t" }),
        )
        .unwrap();
        for node in ["n-9999", "n-10000", "n-0001"] {
            append_and_apply_event(
                &paths,
                "node.created",
                Some(&nid(node)),
                None,
                json!({ "kind": "spinoff" }),
            )
            .unwrap();
        }

        let out = cancel_run(&paths, None).unwrap();
        assert_eq!(
            out.nodes_cancelled
                .iter()
                .map(NodeId::as_str)
                .collect::<Vec<_>>(),
            vec!["n-0001", "n-9999", "n-10000"],
        );
    }

    #[test]
    fn cancel_synthesizes_report_for_node_with_missing_projection() {
        // The crash window this fix closes: a `node.created` was appended+fsynced
        // to the log, but its projection write (`nodes/n-NNNN.json`) was
        // interrupted. A `nodes/*.json` scan would not see n-0002 and would
        // cancel the run while leaving a created-but-never-cancelled node a
        // future rebuild could resurrect as live. Enumerating from the event log
        // sees it and synthesizes the cancel report.
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 2);
        // Delete n-0002's projection file, leaving its `node.created` event in
        // the log — exactly the interrupted-fold state.
        let n2 = NodeId::parse_str("n-0002").unwrap();
        std::fs::remove_file(paths.node(&n2)).unwrap();
        assert!(
            read_node_opt(&paths, &n2).unwrap().is_none(),
            "projection gone"
        );

        let out = cancel_run(&paths, Some("stop")).unwrap();
        // Both nodes are cancelled — the projection-present n-0001 AND the
        // projection-missing n-0002.
        assert_eq!(
            out.nodes_cancelled
                .iter()
                .map(NodeId::as_str)
                .collect::<Vec<_>>(),
            vec!["n-0001", "n-0002"],
            "the node with a missing projection is still cancelled"
        );
        assert!(out.nodes_already_terminal.is_empty());
        // The source-of-truth log now carries a terminal cancel report for the
        // node whose projection was missing — so a rebuild reconstructs it as
        // Cancelled, not live.
        assert_eq!(report_count(&paths, "n-0002"), 1);
        assert_eq!(
            crate::read_manifest(&paths).unwrap().status,
            Status::Cancelled
        );
    }

    #[test]
    fn cancel_takes_the_run_lock_exactly_once() {
        // The single-lock honesty guarantee: the whole transaction (N node
        // reports + the run.status append) runs under ONE flock acquisition, not
        // one per appended event. Spy on `RunLock::acquire` to prove it.
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 5);

        // Bootstrap itself takes the lock once per append; only the cancel call
        // is under measurement.
        ACQUIRE_COUNT.with(|c| c.set(0));
        let out = cancel_run(&paths, Some("stop")).unwrap();
        assert_eq!(out.nodes_cancelled.len(), 5);
        assert_eq!(
            ACQUIRE_COUNT.with(std::cell::Cell::get),
            1,
            "cancel must take the run lock exactly once, not once per node (N+1)"
        );
    }

    #[test]
    fn cancel_does_not_duplicate_a_node_report_already_in_the_log() {
        // Crash-retry idempotency: a prior cancel appended+fsynced a node's
        // cancel `node.report` (carrying the deterministic key) but crashed
        // before folding its projection, so the node still reads live. A
        // re-cancel must NOT append a second logical-cancel event for it.
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 1); // run.created (seq 1) + node.created (seq 2)

        // Durably append the cancel report WITH the deterministic key, but
        // without folding it — the node stays Pending (live), modeling the
        // fsynced-but-not-applied window.
        let node = nid("n-0001");
        let key = node_cancel_key(&paths.run_id, &node);
        RunLock::with_lock(&paths, |lock| {
            append_event_with_seq(
                lock,
                &paths,
                3,
                "node.report",
                Some(&node),
                Some(&key),
                json!({ "success": false, "cancelled": true, "reason": "x" }),
            )
        })
        .unwrap();
        assert_eq!(node_status(&paths, "n-0001"), Status::Pending);
        assert_eq!(report_count(&paths, "n-0001"), 1);

        let out = cancel_run(&paths, None).unwrap();
        // The node converges (it is reported cancelled) but no duplicate report
        // is appended — the log still holds exactly one `node.report` for it.
        assert_eq!(
            out.nodes_cancelled
                .iter()
                .map(NodeId::as_str)
                .collect::<Vec<_>>(),
            vec!["n-0001"],
        );
        assert_eq!(
            report_count(&paths, "n-0001"),
            1,
            "the already-logged cancel report must not be duplicated"
        );
        // Convergence: the crash-stranded projection is folded from the
        // already-logged event, so the node reads Cancelled (not the stale
        // Pending) even though no new event was appended for it.
        assert_eq!(
            node_status(&paths, "n-0001"),
            Status::Cancelled,
            "the already-logged cancel must be re-folded, not just skipped"
        );
        assert_eq!(
            crate::read_manifest(&paths).unwrap().status,
            Status::Cancelled
        );
    }

    #[test]
    fn cancel_does_not_duplicate_run_status_already_in_the_log() {
        // The run-status analogue: a prior cancel fsynced `run.status: cancelled`
        // (with its deterministic key) but crashed before folding the manifest,
        // so the manifest still reads non-terminal. A re-cancel must not append a
        // second `run.status: cancelled`.
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 0); // run.created only (seq 1)

        let key = run_status_cancel_key(&paths.run_id);
        RunLock::with_lock(&paths, |lock| {
            append_event_with_seq(
                lock,
                &paths,
                2,
                "run.status",
                None,
                Some(&key),
                json!({ "status": "cancelled" }),
            )
        })
        .unwrap();
        // Manifest never folded the cancel, so it is not terminal here.
        assert_ne!(
            crate::read_manifest(&paths).unwrap().status,
            Status::Cancelled
        );
        let before = read_all_events(&paths.events()).unwrap().len();

        let out = cancel_run(&paths, None).unwrap();
        assert!(!out.run_was_already_cancelled);
        assert_eq!(
            read_all_events(&paths.events()).unwrap().len(),
            before,
            "no duplicate run.status appended when one is already logged"
        );
        // Convergence: the manifest is folded from the already-logged
        // `run.status: cancelled` instead of being left stale.
        assert_eq!(
            crate::read_manifest(&paths).unwrap().status,
            Status::Cancelled,
            "the already-logged run.status must be re-folded, not just skipped"
        );
    }

    #[test]
    fn cancel_skips_node_terminal_in_log_despite_stale_live_projection() {
        // cancel-liveness-from-log: a non-cancel terminal event (here a
        // `node.status` to a terminal value) was fsynced to the log but its
        // projection fold was crash-interrupted, so `nodes/n-0001.json` still
        // reads the stale live (Pending) status. The cancel must derive liveness
        // from the LOG and treat the node as already-terminal — never
        // synthesizing a cancel that would over-write the log's terminal and
        // diverge on a future rebuild (which replays node.status: done FIRST and
        // drops the later cancel).
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 2); // run.created(1) + node.created n-0001(2), n-0002(3)

        // Raw-append (no fold) a terminal `node.status` for n-0001: the log
        // records it Done, but the projection stays the stale crash-window
        // Pending.
        let n1 = nid("n-0001");
        RunLock::with_lock(&paths, |lock| {
            append_event_with_seq(
                lock,
                &paths,
                4,
                "node.status",
                Some(&n1),
                None,
                json!({ "status": "done" }),
            )
        })
        .unwrap();
        assert_eq!(
            node_status(&paths, "n-0001"),
            Status::Pending,
            "projection is the stale, crash-stranded live status"
        );

        let out = cancel_run(&paths, Some("stop")).unwrap();
        // n-0001 is settled by the log, NOT freshly cancelled; only the
        // genuinely live n-0002 is cancelled.
        assert_eq!(
            out.nodes_already_terminal
                .iter()
                .map(NodeId::as_str)
                .collect::<Vec<_>>(),
            vec!["n-0001"],
            "the log-terminal node is reported already-terminal, not cancelled"
        );
        assert_eq!(
            out.nodes_cancelled
                .iter()
                .map(NodeId::as_str)
                .collect::<Vec<_>>(),
            vec!["n-0002"],
        );
        // No cancel report was synthesized for n-0001: the log still holds zero
        // `node.report` lines for it, so a rebuild reconstructs it from the
        // `node.status: done` (Done), not a divergent Cancelled.
        assert_eq!(
            report_count(&paths, "n-0001"),
            0,
            "no cancel over-write was appended for the log-terminal node"
        );
    }

    #[test]
    fn cancel_skips_node_with_unfolded_success_report_in_log() {
        // The issue's headline case: a `node.report { success: true }` fsynced
        // but not folded leaves a stale-live projection. Liveness from the log
        // settles the node as Done (already-terminal); the old projection-derived
        // check would have wrongly cancelled it over its already-logged success.
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 1); // run.created(1) + node.created n-0001(2)
        let n1 = nid("n-0001");
        RunLock::with_lock(&paths, |lock| {
            append_event_with_seq(
                lock,
                &paths,
                3,
                "node.report",
                Some(&n1),
                None,
                json!({ "success": true }),
            )
        })
        .unwrap();
        assert_eq!(
            node_status(&paths, "n-0001"),
            Status::Pending,
            "stale live projection (success report fsynced but not folded)"
        );

        let out = cancel_run(&paths, None).unwrap();
        assert_eq!(
            out.nodes_already_terminal
                .iter()
                .map(NodeId::as_str)
                .collect::<Vec<_>>(),
            vec!["n-0001"],
        );
        assert!(
            out.nodes_cancelled.is_empty(),
            "a node the log shows Done must not be cancelled"
        );
        assert_eq!(
            report_count(&paths, "n-0001"),
            1,
            "only the original success report remains; no cancel was appended"
        );
    }

    #[test]
    fn cancel_ledger_streams_large_report_payloads() {
        // The streaming ledger skims each line's envelope + a few small status
        // fields, never materializing the (here multi-KB) `node.report` `data`
        // payload. A node settled by such a report is still correctly seen as
        // terminal from the log, and a live sibling is still cancelled — proving
        // liveness is derived without holding whole reports in memory.
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 2);
        let big = "x".repeat(64 * 1024);
        append_and_apply_event(
            &paths,
            "node.report",
            Some(&nid("n-0001")),
            None,
            json!({ "success": true, "summary": big }),
        )
        .unwrap();
        assert_eq!(node_status(&paths, "n-0001"), Status::Done);

        let out = cancel_run(&paths, Some("stop")).unwrap();
        assert_eq!(
            out.nodes_already_terminal
                .iter()
                .map(NodeId::as_str)
                .collect::<Vec<_>>(),
            vec!["n-0001"],
        );
        assert_eq!(
            out.nodes_cancelled
                .iter()
                .map(NodeId::as_str)
                .collect::<Vec<_>>(),
            vec!["n-0002"],
        );
    }

    // --- per-node cancel (`cancel_node`) -----------------------------------

    #[test]
    fn cancel_node_settles_one_node_and_leaves_the_run_and_siblings_live() {
        // The fan-out headline: cancelling one live child settles ONLY that node,
        // preserves it as Cancelled, and leaves the run + every sibling untouched
        // and non-terminal — the supervisor's rollup (not this call) terminalizes
        // the batch later.
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 3);

        let out = cancel_node(&paths, &nid("n-0002"), Some("stuck")).unwrap();
        assert_eq!(out.node_id.as_str(), "n-0002");
        assert!(out.cancelled);
        assert!(!out.already_terminal);
        assert_eq!(out.rolled_up, None, "siblings live → run not rolled up");

        assert_eq!(node_status(&paths, "n-0002"), Status::Cancelled);
        assert_eq!(node_status(&paths, "n-0001"), Status::Pending);
        assert_eq!(node_status(&paths, "n-0003"), Status::Pending);
        assert!(
            !crate::read_manifest(&paths).unwrap().status.is_terminal(),
            "no run.status is appended by a per-node cancel while siblings are live"
        );
        // The synthesized report carries the branch-preserving cancel shape.
        let report = crate::read_node(&paths, &nid("n-0002"))
            .unwrap()
            .last_report
            .expect("cancel report recorded");
        assert_eq!(report["cancelled"], true);
        assert_eq!(report["success"], false);
        assert_eq!(report["reason"], "stuck");
    }

    #[test]
    fn cancel_node_unknown_id_is_node_not_found() {
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 1);
        let err = cancel_node(&paths, &nid("n-0009"), None).unwrap_err();
        assert!(
            matches!(err, Error::NodeNotFound { ref node_id } if node_id == "n-0009"),
            "got {err:?}"
        );
    }

    #[test]
    fn cancel_node_on_already_terminal_node_is_idempotent_noop() {
        // A node that finished on its own (Done) is reported already-terminal,
        // never freshly cancelled, and no cancel report is appended over its
        // success.
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 2);
        append_and_apply_event(
            &paths,
            "node.report",
            Some(&nid("n-0001")),
            None,
            json!({ "success": true }),
        )
        .unwrap();

        let out = cancel_node(&paths, &nid("n-0001"), None).unwrap();
        assert!(!out.cancelled);
        assert!(out.already_terminal);
        assert_eq!(node_status(&paths, "n-0001"), Status::Done, "untouched");
        assert_eq!(report_count(&paths, "n-0001"), 1, "no cancel over-write");
    }

    #[test]
    fn cancel_node_twice_does_not_duplicate_the_report() {
        // Idempotent duplicate per-node cancel: the second call converges/no-ops
        // and never appends a second cancel `node.report`.
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 2);

        let first = cancel_node(&paths, &nid("n-0001"), Some("x")).unwrap();
        assert!(first.cancelled);
        assert_eq!(report_count(&paths, "n-0001"), 1);

        let second = cancel_node(&paths, &nid("n-0001"), Some("x")).unwrap();
        assert!(!second.cancelled);
        assert!(second.already_terminal);
        assert_eq!(
            report_count(&paths, "n-0001"),
            1,
            "a duplicate per-node cancel must not append a second report"
        );
        assert_eq!(node_status(&paths, "n-0001"), Status::Cancelled);
    }

    #[test]
    fn cancel_node_converges_a_crash_stranded_prior_cancel_without_duplicating() {
        // Crash-retry: a prior cancel fsynced the node's cancel `node.report`
        // (with the deterministic key) but crashed before folding the projection,
        // so the node still reads live. A re-cancel re-folds the logged event
        // (node → Cancelled) without appending a second report.
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 1); // run.created(1) + node.created(2)
        let node = nid("n-0001");
        let key = node_cancel_key(&paths.run_id, &node);
        RunLock::with_lock(&paths, |lock| {
            append_event_with_seq(
                lock,
                &paths,
                3,
                "node.report",
                Some(&node),
                Some(&key),
                json!({ "success": false, "cancelled": true, "reason": "x" }),
            )
        })
        .unwrap();
        assert_eq!(node_status(&paths, "n-0001"), Status::Pending);

        let out = cancel_node(&paths, &node, None).unwrap();
        assert!(out.cancelled, "the stranded cancel is converged");
        assert!(!out.already_terminal);
        assert_eq!(report_count(&paths, "n-0001"), 1, "no duplicate append");
        assert_eq!(node_status(&paths, "n-0001"), Status::Cancelled);
    }

    #[test]
    fn cancel_node_resolves_a_node_with_a_missing_projection() {
        // The log — not the `nodes/*.json` scan — is authoritative for the node
        // set: a node whose projection write was crash-interrupted is still
        // cancellable (and its cancel report lands so a rebuild reconstructs it
        // Cancelled, not live).
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 2);
        let n2 = nid("n-0002");
        std::fs::remove_file(paths.node(&n2)).unwrap();
        assert!(read_node_opt(&paths, &n2).unwrap().is_none());

        let out = cancel_node(&paths, &n2, Some("stop")).unwrap();
        assert!(out.cancelled);
        // The source-of-truth log now carries the terminal cancel report, so a
        // rebuild reconstructs the node Cancelled (its projection stays absent —
        // the reducer folds a report without resurrecting a deleted projection,
        // exactly as the whole-run cancel does).
        assert_eq!(report_count(&paths, "n-0002"), 1);
    }

    #[test]
    fn cancel_node_blank_note_falls_back_to_default_reason() {
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 1);
        let out = cancel_node(&paths, &nid("n-0001"), Some("   ")).unwrap();
        assert!(out.cancelled);
        let report = crate::read_node(&paths, &nid("n-0001"))
            .unwrap()
            .last_report
            .expect("cancel report recorded");
        assert_eq!(report["reason"], "cancelled by user");
    }

    #[test]
    fn cancel_node_takes_the_run_lock_exactly_once() {
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 3);
        ACQUIRE_COUNT.with(|c| c.set(0));
        let out = cancel_node(&paths, &nid("n-0002"), Some("x")).unwrap();
        assert!(out.cancelled);
        assert_eq!(
            ACQUIRE_COUNT.with(std::cell::Cell::get),
            1,
            "per-node cancel must take the run lock exactly once"
        );
    }

    #[test]
    fn cancel_last_live_node_rolls_the_run_up_under_the_same_lock() {
        // Cancelling the final live node terminalizes the run HERE (llm-review
        // C1) — not deferred to a possibly-dead supervisor. Every node cancelled,
        // none failed → the run rolls up to Cancelled in the same transaction.
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 2);

        // First cancel: n-0002 still live → run stays live.
        let first = cancel_node(&paths, &nid("n-0001"), Some("x")).unwrap();
        assert!(first.cancelled);
        assert_eq!(first.rolled_up, None);
        assert!(!crate::read_manifest(&paths).unwrap().status.is_terminal());

        // Second cancel settles the last live node → the run rolls up.
        let out = cancel_node(&paths, &nid("n-0002"), Some("x")).unwrap();
        assert!(out.cancelled);
        assert_eq!(out.rolled_up, Some(Status::Cancelled));
        assert_eq!(node_status(&paths, "n-0001"), Status::Cancelled);
        assert_eq!(node_status(&paths, "n-0002"), Status::Cancelled);
        assert_eq!(
            crate::read_manifest(&paths).unwrap().status,
            Status::Cancelled,
            "the last per-node cancel terminalizes the run itself"
        );
    }

    #[test]
    fn cancel_last_live_node_rolls_up_to_failed_when_a_sibling_failed() {
        // A genuine failure dominates the roll-up: cancelling the last live node
        // of a batch where a sibling already failed rolls the run up to Failed
        // (not Cancelled).
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 2);
        append_and_apply_event(
            &paths,
            "node.report",
            Some(&nid("n-0001")),
            None,
            json!({ "success": false }),
        )
        .unwrap();
        assert_eq!(node_status(&paths, "n-0001"), Status::Failed);

        let out = cancel_node(&paths, &nid("n-0002"), Some("x")).unwrap();
        assert!(out.cancelled);
        assert_eq!(out.rolled_up, Some(Status::Failed));
        assert_eq!(crate::read_manifest(&paths).unwrap().status, Status::Failed);
    }

    #[test]
    fn cancel_last_live_node_rolls_up_to_cancelled_on_done_plus_cancelled_mix() {
        // Some siblings merged (Done), the last is cancelled, none failed → the
        // batch rolls up to Cancelled (nothing failed, not a clean all-Done).
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 2);
        append_and_apply_event(
            &paths,
            "node.report",
            Some(&nid("n-0001")),
            None,
            json!({ "success": true }),
        )
        .unwrap();
        assert_eq!(node_status(&paths, "n-0001"), Status::Done);

        let out = cancel_node(&paths, &nid("n-0002"), Some("x")).unwrap();
        assert!(out.cancelled);
        assert_eq!(out.rolled_up, Some(Status::Cancelled));
        assert_eq!(
            crate::read_manifest(&paths).unwrap().status,
            Status::Cancelled
        );
    }

    #[test]
    fn cancel_node_refuses_a_done_run() {
        // Mirror `cancel_run`'s guard (llm-review C4): a Done/Failed run is
        // refused rather than appending a dead post-terminal cancel.
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 1);
        append_and_apply_event(
            &paths,
            "node.report",
            Some(&nid("n-0001")),
            None,
            json!({ "success": true }),
        )
        .unwrap();
        append_and_apply_event(
            &paths,
            "run.status",
            None,
            None,
            json!({ "status": "done" }),
        )
        .unwrap();
        let before = read_all_events(&paths.events()).unwrap().len();

        let err = cancel_node(&paths, &nid("n-0001"), None).unwrap_err();
        assert!(
            matches!(
                err,
                Error::RunAlreadyTerminal {
                    status: Status::Done
                }
            ),
            "got {err:?}"
        );
        assert_eq!(
            read_all_events(&paths.events()).unwrap().len(),
            before,
            "a refused per-node cancel must not append any event"
        );
    }

    #[test]
    fn cancel_node_last_node_shares_run_status_key_with_whole_run_cancel() {
        // The last-node roll-up uses the whole-run cancel's `run-status` key, so a
        // later `cancel_run` converges on the SAME logical run.status rather than
        // appending a duplicate.
        let tmp = TempDir::new().unwrap();
        let paths = fresh_run(&tmp);
        bootstrap(&paths, 1);
        let out = cancel_node(&paths, &nid("n-0001"), Some("x")).unwrap();
        assert_eq!(out.rolled_up, Some(Status::Cancelled));
        let run_status_events = read_all_events(&paths.events())
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "run.status")
            .count();
        assert_eq!(run_status_events, 1);

        // A whole-run cancel now finds the run already cancelled and converges —
        // no second run.status.
        let cr = cancel_run(&paths, Some("x")).unwrap();
        assert!(cr.run_was_already_cancelled);
        let run_status_events = read_all_events(&paths.events())
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "run.status")
            .count();
        assert_eq!(
            run_status_events, 1,
            "cancel_run must not duplicate the run.status the last-node roll-up wrote"
        );
    }
}
