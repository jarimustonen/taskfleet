//! Wire DTOs for the `run` noun.
//!
//! These decouple the CLI `--json` contract from the on-disk projection
//! (`taskfleet_core::Manifest`). Handlers serialize a `*View` — never the
//! `Manifest` — so the disk schema can evolve without leaking into the
//! public envelope.
//!
//! Deliberately dropped from the wire contract:
//!
//! - `applied_seq` — the reducer's projection watermark (highest event
//!   `seq` whose fold is durably committed). Pure crash-atomicity
//!   bookkeeping on a derived-cache file; meaningless to a wire consumer.
//!
//! `kind` / `lifecycle` / `status` render through the kebab helpers in
//! [`crate::run`] so a new enum variant is a compile error here, not a
//! silent wire change.

use chrono::{DateTime, Utc};
use serde::Serialize;

use taskfleet_core::{Manifest, NodeId, RunId, RunPaths};

use super::{kind_kebab, lifecycle_kebab, status_kebab};
use crate::supervise::pid_file;

/// The distinct conditions a per-run supervisor can be in, as observed from
/// its `<run-dir>/supervisor.pid` file. Replaces the old boolean `alive`,
/// which collapsed four genuinely different situations into `false` and so
/// could not tell "finished cleanly" from "orphaned" from "I/O error" — a
/// consumer reasoning about that boolean risked a wrong `run reattach` /
/// `run cancel` decision (issue `supervisorview-conflates-states`).
///
/// Serialized kebab-case: `alive | dead | not-recorded | unreadable |
/// unknown`.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum SupervisorState {
    /// A PID is recorded and is a live process whose start-time still matches
    /// the record (§7.6 identity check). The supervisor is running.
    Alive,
    /// A PID is recorded but is NOT a live matching process — the supervisor
    /// was started and then died (e.g. SIGTERM), or the PID was recycled by an
    /// unrelated process. This is the orphaned condition the
    /// `supervisor-dead-merge-no-teardown` bug describes: nothing is left to
    /// consume the terminal report or run teardown. Recover with
    /// `run reattach <id>`.
    Dead,
    /// No pid file exists on disk. The supervisor either never materialized, or
    /// exited cleanly (it removes its pid file on a clean tear-down). These two
    /// are indistinguishable at the pid-file layer, so they share one state —
    /// but both are decisively "nothing recorded", distinct from a file that is
    /// present-but-unreadable.
    NotRecorded,
    /// A pid file is present but could not be read or parsed — an I/O error, a
    /// non-integer / out-of-range first token, or a rejected symlink (the read
    /// path fails a symlinked file closed with `ELOOP`). Distinct from
    /// `NotRecorded`: something is there, we just cannot trust it. A consumer
    /// should treat this as "unknown liveness, investigate" rather than
    /// silently as "no supervisor".
    Unreadable,
    /// Not yet probed — the default [`RunSummary::from`] carries until a handler
    /// overrides it via [`RunSummary::with_supervisor`]. Both `run show` and
    /// `run list` always probe, so a wire consumer should not see this there; it
    /// exists so an unprobed summary is never silently rendered as
    /// `NotRecorded`.
    Unknown,
}

impl SupervisorState {
    /// Back-compat convenience: whether the supervisor is running. Only `Alive`
    /// is live; every other state maps to `false`, matching the old boolean's
    /// "not alive" semantics for existing consumers.
    fn is_alive(self) -> bool {
        matches!(self, SupervisorState::Alive)
    }
}

/// Liveness of the run's per-run supervisor, surfaced on `run show` /
/// `run list` so a caller can tell "still working" from "orphaned" from
/// "finished" from "can't tell".
///
/// The authoritative field is [`state`](Self::state) — a
/// [`SupervisorState`] distinguishing all four non-alive conditions. The
/// `alive` boolean is retained for back-compat (it equals `state == Alive`)
/// so existing consumers keep working during migration; new consumers should
/// branch on `state`.
#[derive(Serialize)]
pub struct SupervisorView {
    /// The supervisor PID recorded in `<run-dir>/supervisor.pid`, or `null`
    /// when no readable PID is recorded (`NotRecorded` — never materialized or
    /// cleanly torn down — or `Unreadable`).
    pub pid: Option<u32>,
    /// The distinct supervisor condition — see [`SupervisorState`]. This is the
    /// field to branch on; it disambiguates the cases the `alive` boolean
    /// collapses.
    pub state: SupervisorState,
    /// Back-compat: `true` iff `state` is [`SupervisorState::Alive`]. Retained
    /// so consumers reading `supervisor.alive` keep working; prefer `state`.
    pub alive: bool,
}

impl SupervisorView {
    /// Build a view from a resolved [`SupervisorState`] and optional PID,
    /// keeping the derived `alive` boolean in lockstep with `state`.
    fn new(pid: Option<u32>, state: SupervisorState) -> Self {
        Self {
            pid,
            state,
            alive: state.is_alive(),
        }
    }

    /// Probe `<run-dir>/supervisor.pid` for the recorded supervisor and its
    /// liveness, resolving the exact [`SupervisorState`]:
    ///
    /// - readable record, live + identity-matching → `Alive`
    /// - readable record, dead / recycled → `Dead`
    /// - no file on disk → `NotRecorded`
    /// - file present but unreadable / unparseable → `Unreadable`
    ///
    /// The absent-vs-unreadable distinction comes from a **single** `open()`
    /// (via [`pid_file::classify_pid_record`]), so it cannot race the file
    /// being created or removed between two syscalls, and a real I/O error is
    /// never misread as "no supervisor". Single-file read (the pid file is
    /// CLI-owned and does not route through the run-state projection guards),
    /// so it needs no shared lock: it never participates in a multi-projection
    /// decision.
    pub fn probe(paths: &RunPaths) -> Self {
        match pid_file::classify_pid_record(&paths.supervisor_pid()) {
            pid_file::PidRecord::Present { pid, start_time } => {
                let state = if pid_file::pid_live_with_identity(pid, start_time) {
                    SupervisorState::Alive
                } else {
                    SupervisorState::Dead
                };
                Self::new(Some(pid), state)
            }
            pid_file::PidRecord::Absent => Self::new(None, SupervisorState::NotRecorded),
            pid_file::PidRecord::Unreadable => Self::new(None, SupervisorState::Unreadable),
        }
    }

    /// The "no supervisor probed" default [`RunSummary::from`] carries until a
    /// handler overrides it via [`RunSummary::with_supervisor`]. (`ManifestView`
    /// no longer holds a supervisor — `run show` probes one and attaches it to
    /// the flattened summary row; see `run/show.rs`.)
    fn unknown() -> Self {
        Self::new(None, SupervisorState::Unknown)
    }

    /// The value to feed the stall detectors' `supervisor_alive` parameter
    /// ([`crate::run::stalled::is_stillborn`] / [`stall_kind`]).
    ///
    /// `true` when the supervisor is running OR its state is *indeterminate*
    /// (`Unreadable` / `Unknown` — we cannot prove it is not running, so it must
    /// NOT trigger a stillborn/orphaned diagnosis and its `run reattach` hint);
    /// `false` only when the supervisor is *confirmed not running* (`Dead`, or
    /// `NotRecorded` — the latter IS the stillborn signal). This is the fix for
    /// the conflation issue one layer down: an unreadable pid file used to read
    /// `alive: false` and so could mislead a recovery decision.
    ///
    /// [`stall_kind`]: crate::run::stalled::stall_kind
    pub fn presumed_working(&self) -> bool {
        !matches!(
            self.state,
            SupervisorState::Dead | SupervisorState::NotRecorded
        )
    }
}

/// Full single-run manifest wire view (`run show --json`, nested under
/// `data.manifest`).
///
/// Borrows from the projection: the `show` handler holds the `Manifest`
/// for the lifetime of the emit. Field order and names mirror the
/// established wire contract; the internal `applied_seq` watermark is
/// intentionally absent (see module docs).
///
/// Note: supervisor liveness is deliberately NOT a field here. It is a
/// *computed* probe (not a persisted manifest field), so it lives as a
/// sibling of the other computed `run show` fields (`counts`, `landed`,
/// `stalled`) at the top level of the `data` payload — `data.supervisor`,
/// matching where `run list` rows and the bundled skills expect it (issue
/// `run-show-json-null-fields`: burying it at `data.manifest.supervisor`
/// made a consumer reading `data.supervisor` observe a null).
#[derive(Serialize)]
pub struct ManifestView<'a> {
    pub schema_version: u32,
    pub run_id: &'a RunId,
    pub kind: &'static str,
    pub lifecycle: &'static str,
    pub title: &'a str,
    pub status: &'static str,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub source_repo: Option<&'a str>,
    pub source_branch: Option<&'a str>,
    pub worktree_root: Option<&'a str>,
    /// The code-harness selected for this run's worker (`claude` | `pi` | …),
    /// recorded at `run create`. `null` for a legacy run created before harness
    /// selection existed.
    pub harness: Option<&'a str>,
    /// Recorded create-time profile resolution, or the explicit legacy marker
    /// for manifests that predate/profile-opt out of this metadata.
    pub selection: AgentSelectionView<'a>,
    pub node_count: u32,
    pub parent_run_id: Option<&'a RunId>,
    pub parent_node_id: Option<&'a NodeId>,
}

impl<'a> From<&'a Manifest> for ManifestView<'a> {
    fn from(m: &'a Manifest) -> Self {
        Self {
            schema_version: m.schema_version,
            run_id: &m.run_id,
            kind: kind_kebab(m.kind),
            lifecycle: lifecycle_kebab(m.lifecycle),
            title: &m.title,
            status: status_kebab(m.status),
            created_at: m.created_at,
            updated_at: m.updated_at,
            source_repo: m.source_repo.as_deref(),
            source_branch: m.source_branch.as_deref(),
            worktree_root: m.worktree_root.as_deref(),
            harness: m.harness.as_deref(),
            selection: m.agent_selection.as_ref().map_or(
                AgentSelectionView::Legacy("legacy-unrecorded"),
                AgentSelectionView::Recorded,
            ),
            node_count: m.node_count,
            parent_run_id: m.parent_run_id.as_ref(),
            parent_node_id: m.parent_node_id.as_ref(),
        }
    }
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum AgentSelectionView<'a> {
    Recorded(&'a taskfleet_core::AgentSelection),
    Legacy(&'static str),
}

/// One row of `run list --json`.
///
/// Owned: built inside each run's lock from a short-lived manifest.
#[allow(clippy::struct_excessive_bools)] // orthogonal wire facts, not a state cross-product
#[derive(Serialize)]
pub struct RunSummary {
    pub run_id: String,
    pub kind: String,
    /// How-run state (`autonomous` | `interactive`), from the explicit
    /// `--interactive` flag at create — NOT a progress signal. Surfaced here so an
    /// agent scanning `run list` can tell a human-driven run (which the supervisor
    /// never auto-terminalizes) from an autonomous one WITHOUT confusing it with
    /// `status`. Poll `status` for completion; read `lifecycle` for how it's driven
    /// (design.md §6, state-integrity invariant 4).
    pub lifecycle: &'static str,
    pub status: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    /// Source repository recorded at run creation. This is durable provenance,
    /// not a title/path guess; legacy or skeleton runs may legitimately carry
    /// `null` when no repository identity was recorded.
    pub source_repo: Option<String>,
    /// Merge target recorded for the run. Populated when the worktree is
    /// materialized, including while the run remains `pending`.
    pub source_branch: Option<String>,
    /// Default worker's materialized worktree. Set by the list/show handlers
    /// from the node projection under the same shared-lock snapshot.
    pub worktree_path: Option<String>,
    /// The code-harness selected for this run's worker (`claude` | `pi` | …),
    /// recorded at `run create`. `null` for a legacy run created before harness
    /// selection existed.
    pub harness: Option<String>,
    pub node_count: u32,
    /// Liveness of the run's per-run supervisor. Defaults to the "unknown"
    /// probe from `From`; the `list` handler overrides it with a real probe
    /// via [`RunSummary::with_supervisor`] (it already holds each run's paths).
    pub supervisor: SupervisorView,
    /// Computed hint (never persisted): true for an undriven `--kind
    /// orchestrate` driver run past the stall grace window — the silent-zombie
    /// signature from issue `peculiarly-muddled-caption`. Defaults to `false`
    /// from `From`; the `list` handler overrides it via
    /// [`RunSummary::with_stalled`] after reading the driver node under the
    /// shared lock. See [`crate::run::stalled`].
    pub stalled: bool,
    /// Computed hint (never persisted): true for a *stillborn* run — pending, a
    /// dead/absent supervisor, zero nodes, and no forward progress since
    /// creation — the "supervisor died before creating any worker node"
    /// signature from issue `supervisor-dies-before-worker-node`. Kind-agnostic
    /// (any run created but never started), unlike [`Self::stalled`], which is
    /// the orchestrate-driver-specific idle shape. Defaults to `false` from
    /// `From`; the `list` / `show` handlers override it via
    /// [`RunSummary::with_stillborn`] from the same shared-lock snapshot. See
    /// [`crate::run::stalled::is_stillborn`].
    ///
    /// Relationship to [`Self::stalled`]: the two *underlying detections* are
    /// mutually exclusive by construction (stillborn requires `node_count == 0`;
    /// the orchestrate stall requires a driver node, i.e. `node_count >= 1`), so
    /// a single run is never simultaneously an orchestrate stall AND stillborn.
    /// But `stalled` is the umbrella "pending yet not progressing" flag — set
    /// for *either* shape (matching `run show` / `run wait`) — so a stillborn
    /// run carries BOTH `stillborn: true` and `stalled: true`. Read `stillborn`
    /// for the specific never-started diagnosis; read `stalled` for the generic
    /// "needs attention" signal.
    pub stillborn: bool,
    /// Computed hint (never persisted): true for an *attention-required* run — a
    /// worker that exited cleanly (`worker.exited` code 0) but left the node
    /// non-terminal because it skipped `run merge` (design.md §2.5 / A5, issue
    /// `attention-required-run-surface`). Distinct from [`Self::stalled`]: the
    /// supervisor may well be alive and healthy; the run is stuck because the
    /// *worker* finished without merging, so the remediation is a manual finish
    /// (`run merge` from the worktree) or `run cancel`, NOT `run reattach`.
    /// Defaults to `false` from `From`; the `list` / `show` handlers override it
    /// via [`RunSummary::with_attention`] from the same shared-lock node snapshot.
    /// See [`crate::run::attention`].
    pub attention_required: bool,
    /// Resume context for an attention-required run — pending age, last-observed
    /// worker pid, worktree path, source branch, and a one-line resume hint — so a
    /// PO can find and finish the stuck worktree. `None` (omitted from the wire)
    /// unless [`Self::attention_required`] is true. Set together with it via
    /// [`RunSummary::with_attention`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attention: Option<crate::run::attention::AttentionView>,
    /// Durable, explicit signal that the worker needs a human decision. Unlike
    /// `attention_required`, this is emitted while the worker is still alive and
    /// carries the open report-shaped discussion objects.
    pub awaiting_input: bool,
    /// Number of open decision items. Kept at top level for cheap list filtering.
    pub open_discussion_count: usize,
    /// Whether the advisory telemetry scan completed. False never changes or
    /// suppresses canonical run fields; the envelope warning explains why.
    pub telemetry_available: bool,
    /// Bounded observational counts of advisory samples by freshness state.
    /// Meaningful when `telemetry_available` is true; never affects lifecycle.
    pub telemetry_counts: crate::run::telemetry::TelemetryCounts,
    /// Full question/options/default context, omitted when no request is open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub awaiting_input_detail: Option<crate::run::awaiting_input::AwaitingInputView>,
}

impl RunSummary {
    /// Attach a probed [`SupervisorView`], replacing the `From`-provided
    /// "unknown" default.
    #[must_use]
    pub fn with_supervisor(mut self, supervisor: SupervisorView) -> Self {
        self.supervisor = supervisor;
        self
    }

    /// Attach the default worker's materialized worktree path.
    #[must_use]
    pub fn with_worktree_path(mut self, worktree_path: Option<String>) -> Self {
        self.worktree_path = worktree_path;
        self
    }

    /// Set the computed `stalled` hint, replacing the `From`-provided `false`
    /// default.
    #[must_use]
    pub fn with_stalled(mut self, stalled: bool) -> Self {
        self.stalled = stalled;
        self
    }

    /// Set the computed `stillborn` hint, replacing the `From`-provided `false`
    /// default.
    #[must_use]
    pub fn with_stillborn(mut self, stillborn: bool) -> Self {
        self.stillborn = stillborn;
        self
    }

    /// Attach the attention-required verdict and (when true) its resume context,
    /// replacing the `From`-provided `false` / `None` defaults. Pass `None` for a
    /// run that is not attention-required.
    ///
    /// **Enforces attention-over-stall precedence** (design.md §2.5): a clean-
    /// exited worker whose supervisor *also* died matches BOTH the attention shape
    /// and the orphaned/stillborn stall shape, but the correct remediation is the
    /// manual finish (`run merge`), not `run reattach`. So when attention is
    /// present this clears `stalled` / `stillborn`, keeping the two verdicts
    /// mutually exclusive on every wire and human surface (they were computed
    /// independently upstream). Call this AFTER `with_stalled` / `with_stillborn`
    /// so the precedence resolution wins — every call site does.
    #[must_use]
    pub fn with_attention(
        mut self,
        attention: Option<crate::run::attention::AttentionView>,
    ) -> Self {
        self.attention_required = attention.is_some();
        if self.attention_required {
            // Attention wins: suppress any stall verdict so the summary can never
            // emit contradictory `attention_required: true` + `stalled: true`.
            self.stalled = false;
            self.stillborn = false;
        }
        self.attention = attention;
        self
    }

    /// Attach bounded telemetry freshness counts. This setter has no access to
    /// any lifecycle or outcome field by design.
    #[must_use]
    pub fn with_telemetry_counts(
        mut self,
        telemetry_counts: crate::run::telemetry::TelemetryCounts,
        available: bool,
    ) -> Self {
        self.telemetry_available = available;
        self.telemetry_counts = telemetry_counts;
        self
    }

    /// Attach an explicit open human-decision request. It is additive to liveness
    /// hints: an orphaned supervisor still needs reattachment. Terminal runs
    /// defensively suppress stale node-level requests.
    #[must_use]
    pub fn with_awaiting_input(
        mut self,
        awaiting: Option<crate::run::awaiting_input::AwaitingInputView>,
    ) -> Self {
        let awaiting = (!matches!(self.status.as_str(), "done" | "failed" | "cancelled"))
            .then_some(awaiting)
            .flatten();
        self.awaiting_input = awaiting.is_some();
        self.open_discussion_count = awaiting.as_ref().map_or(0, |v| v.open_discussion_count);
        self.awaiting_input_detail = awaiting;
        self
    }
}

impl From<&Manifest> for RunSummary {
    fn from(m: &Manifest) -> Self {
        Self {
            run_id: m.run_id.to_string(),
            kind: kind_kebab(m.kind).to_string(),
            lifecycle: lifecycle_kebab(m.lifecycle),
            status: status_kebab(m.status).to_string(),
            title: m.title.clone(),
            created_at: m.created_at,
            source_repo: m.source_repo.clone(),
            source_branch: m.source_branch.clone(),
            worktree_path: None,
            harness: m.harness.clone(),
            node_count: m.node_count,
            supervisor: SupervisorView::unknown(),
            stalled: false,
            stillborn: false,
            attention_required: false,
            attention: None,
            awaiting_input: false,
            open_discussion_count: 0,
            telemetry_available: false,
            telemetry_counts: crate::run::telemetry::TelemetryCounts::default(),
            awaiting_input_detail: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use taskfleet_core::{Kind, Lifecycle, Status};

    fn ts() -> DateTime<Utc> {
        "2024-01-01T00:00:00Z".parse().unwrap()
    }

    fn sample() -> Manifest {
        Manifest {
            schema_version: 1,
            applied_seq: 1,
            run_id: RunId::parse_str("01arz3ndektsv4rrffq69g5fav").unwrap(),
            kind: Kind::Spinoff,
            lifecycle: Lifecycle::Autonomous,
            title: "seed-run".to_string(),
            status: Status::Pending,
            created_at: ts(),
            updated_at: ts(),
            source_repo: None,
            source_branch: None,
            worktree_root: None,
            managed_tmux_session: None,
            tmux_retention: None,
            notify_cmd: None,
            harness: None,
            agent_selection: None,
            node_count: 0,
            parent_run_id: None,
            parent_node_id: None,
        }
    }

    #[test]
    fn view_pins_wire_shape() {
        let m = sample();
        let got = serde_json::to_value(ManifestView::from(&m)).unwrap();
        assert_eq!(
            got,
            json!({
                "schema_version": 1,
                "run_id": "01arz3ndektsv4rrffq69g5fav",
                "kind": "spinoff",
                "lifecycle": "autonomous",
                "title": "seed-run",
                "status": "pending",
                "created_at": "2024-01-01T00:00:00Z",
                "updated_at": "2024-01-01T00:00:00Z",
                "source_repo": null,
                "source_branch": null,
                "worktree_root": null,
                "harness": null,
                "selection": "legacy-unrecorded",
                "node_count": 0,
                "parent_run_id": null,
                "parent_node_id": null,
            })
        );
    }

    /// `applied_seq` is the reducer watermark, not wire state: bumping it on
    /// the projection leaves the DTO output byte-identical.
    #[test]
    fn applied_seq_does_not_leak() {
        let base = serde_json::to_value(ManifestView::from(&sample())).unwrap();
        let mut bumped = sample();
        bumped.applied_seq = 999;
        let after = serde_json::to_value(ManifestView::from(&bumped)).unwrap();
        assert_eq!(base, after, "applied_seq leaked into run DTO");
        assert!(
            after.get("applied_seq").is_none(),
            "applied_seq must be absent from the wire contract"
        );
    }

    #[test]
    fn summary_pins_wire_shape() {
        let m = sample();
        let got = serde_json::to_value(RunSummary::from(&m)).unwrap();
        assert_eq!(
            got,
            json!({
                "run_id": "01arz3ndektsv4rrffq69g5fav",
                "kind": "spinoff",
                "lifecycle": "autonomous",
                "status": "pending",
                "title": "seed-run",
                "created_at": "2024-01-01T00:00:00Z",
                "source_repo": null,
                "source_branch": null,
                "worktree_path": null,
                "harness": null,
                "node_count": 0,
                "supervisor": { "pid": null, "state": "unknown", "alive": false },
                "stalled": false,
                "stillborn": false,
                "attention_required": false,
                "awaiting_input": false,
                "open_discussion_count": 0,
                "telemetry_available": false,
                "telemetry_counts": {
                    "absent": 0,
                    "current": 0,
                    "stale": 0,
                    "clock_unreliable": 0,
                    "invalid": 0
                },
            })
        );
    }

    #[test]
    fn awaiting_input_is_additive_to_stall_but_suppressed_when_terminal() {
        use taskfleet_core::AwaitingInput;
        let open = AwaitingInput {
            opened_at: ts(),
            event_seq: 9,
            discussion_items: vec![json!({
                "topic": "scope", "options": ["small"],
                "recommended_default": "small"
            })],
        };
        let view = crate::run::awaiting_input::AwaitingInputView::build(&open, ts());
        let live = RunSummary::from(&sample())
            .with_stalled(true)
            .with_awaiting_input(Some(view.clone()));
        assert!(live.stalled);
        assert!(live.awaiting_input);
        assert_eq!(live.awaiting_input_detail.unwrap().event_seq, 9);

        let mut terminal = sample();
        terminal.status = Status::Done;
        let done = RunSummary::from(&terminal).with_awaiting_input(Some(view));
        assert!(!done.awaiting_input);
        assert_eq!(done.open_discussion_count, 0);
    }

    /// `with_attention(Some(..))` flips `attention_required` and nests the resume
    /// context; `with_attention(None)` leaves the field absent from the wire.
    #[test]
    fn attention_view_flattens_onto_summary() {
        use crate::run::attention::AttentionView;
        use taskfleet_core::WorkerExit;
        let now: DateTime<Utc> = "2024-01-01T00:10:00Z".parse().unwrap();
        let exit = WorkerExit {
            code: Some(0),
            signal: None,
            at: "2024-01-01T00:00:00Z".parse().unwrap(),
        };
        let view = AttentionView::build(
            "01arz3ndektsv4rrffq69g5fav",
            now,
            &exit,
            Some(4242),
            Some("/tmp/wt/seed".to_string()),
            Some("main".to_string()),
        );
        let got =
            serde_json::to_value(RunSummary::from(&sample()).with_attention(Some(view))).unwrap();
        assert_eq!(got["attention_required"], json!(true));
        // Age is now - worker_exit.at = 10 min.
        assert_eq!(got["attention"]["pending_age_secs"], json!(600));
        assert_eq!(got["attention"]["exited_at"], json!("2024-01-01T00:00:00Z"));
        assert_eq!(got["attention"]["worker_pid"], json!(4242));
        assert_eq!(got["attention"]["worktree_path"], json!("/tmp/wt/seed"));
        assert_eq!(got["attention"]["source_branch"], json!("main"));
        assert_eq!(
            got["attention"]["reason"],
            json!(crate::run::attention::ATTENTION_REASON)
        );
        assert!(got["attention"]["resume_hint"]
            .as_str()
            .unwrap()
            .contains("run merge"));

        // Not attention-required → the nested block is omitted entirely.
        let plain = serde_json::to_value(RunSummary::from(&sample()).with_attention(None)).unwrap();
        assert_eq!(plain["attention_required"], json!(false));
        assert!(plain.get("attention").is_none());
    }

    /// Precedence: `with_attention(Some(..))` suppresses a previously-set stall
    /// verdict, so the wire can never carry contradictory
    /// `attention_required: true` + `stalled: true` (design.md §2.5).
    #[test]
    fn attention_suppresses_stall_on_summary() {
        use crate::run::attention::AttentionView;
        use taskfleet_core::WorkerExit;
        let exit = WorkerExit {
            code: Some(0),
            signal: None,
            at: ts(),
        };
        let view = AttentionView::build("r", ts(), &exit, None, None, None);
        let got = serde_json::to_value(
            RunSummary::from(&sample())
                .with_stalled(true)
                .with_stillborn(true)
                .with_attention(Some(view)),
        )
        .unwrap();
        assert_eq!(got["attention_required"], json!(true));
        assert_eq!(
            got["stalled"],
            json!(false),
            "attention must suppress stall"
        );
        assert_eq!(got["stillborn"], json!(false));
    }

    /// `SupervisorView::probe` reads the real `supervisor.pid` file and resolves
    /// the exact [`SupervisorState`] for each condition, no longer collapsing
    /// absent / dead / unreadable into one `{pid: null, alive: false}`.
    #[test]
    fn supervisor_probe_resolves_distinct_states() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path().join("01arz3ndektsv4rrffq69g5fav");
        std::fs::create_dir_all(&run_dir).unwrap();
        let paths = RunPaths::new(run_dir, "01arz3ndektsv4rrffq69g5fav").unwrap();

        // No pid file on disk → NotRecorded (never launched / cleanly torn down).
        let v = SupervisorView::probe(&paths);
        assert_eq!(v.pid, None);
        assert_eq!(v.state, SupervisorState::NotRecorded);
        assert!(!v.alive);

        // Our own live pid (written with its start-time) → Alive.
        let our_pid = std::process::id();
        pid_file::write_pid(&paths.supervisor_pid(), our_pid).unwrap();
        let v = SupervisorView::probe(&paths);
        assert_eq!(v.pid, Some(our_pid));
        assert_eq!(v.state, SupervisorState::Alive);
        assert!(v.alive, "our own recorded pid must read alive");

        // A recorded-but-dead pid (guaranteed-free high value) → Dead.
        std::fs::write(paths.supervisor_pid(), "2147483646").unwrap();
        let v = SupervisorView::probe(&paths);
        assert_eq!(v.pid, Some(2_147_483_646));
        assert_eq!(v.state, SupervisorState::Dead);
        assert!(!v.alive, "a dead recorded pid must read not-alive");

        // A present-but-unreadable pid file (non-integer garbage) → Unreadable,
        // distinct from NotRecorded — the file is there, we just can't trust it.
        std::fs::write(paths.supervisor_pid(), "not-a-pid").unwrap();
        let v = SupervisorView::probe(&paths);
        assert_eq!(v.pid, None);
        assert_eq!(v.state, SupervisorState::Unreadable);
        assert!(!v.alive);
    }

    /// The `alive` boolean stays in lockstep with `state` for back-compat: it is
    /// `true` only for [`SupervisorState::Alive`], `false` for every other.
    #[test]
    fn alive_boolean_tracks_state() {
        assert!(SupervisorView::new(Some(1), SupervisorState::Alive).alive);
        for state in [
            SupervisorState::Dead,
            SupervisorState::NotRecorded,
            SupervisorState::Unreadable,
            SupervisorState::Unknown,
        ] {
            assert!(
                !SupervisorView::new(None, state).alive,
                "{state:?} must not read alive"
            );
        }
    }

    /// A symlink planted where the pid file belongs is rejected by the
    /// `O_NOFOLLOW` open and classified `Unreadable` (present but untrustworthy),
    /// NOT `NotRecorded` — the security-relevant case the single-open classifier
    /// preserves.
    #[test]
    #[cfg(unix)]
    fn supervisor_probe_symlink_is_unreadable() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let run_dir = dir.path().join("01arz3ndektsv4rrffq69g5fav");
        std::fs::create_dir_all(&run_dir).unwrap();
        let paths = RunPaths::new(run_dir, "01arz3ndektsv4rrffq69g5fav").unwrap();

        // Point supervisor.pid at an otherwise-valid target via a symlink.
        let target = dir.path().join("elsewhere.pid");
        std::fs::write(&target, format!("{}", std::process::id())).unwrap();
        std::os::unix::fs::symlink(&target, paths.supervisor_pid()).unwrap();

        let v = SupervisorView::probe(&paths);
        assert_eq!(v.state, SupervisorState::Unreadable);
        assert_eq!(v.pid, None);
        assert!(!v.alive);
    }

    /// `presumed_working` is the value fed to the stall detectors: `false`
    /// (flaggable) only for the *confirmed* not-running states `Dead` /
    /// `NotRecorded`; `true` (suppress the diagnosis) for the running `Alive`
    /// and the *indeterminate* `Unreadable` / `Unknown` — so an unreadable pid
    /// file can never mislead a stillborn/orphaned recovery decision.
    #[test]
    fn presumed_working_suppresses_indeterminate_states() {
        let flaggable = |s| !SupervisorView::new(None, s).presumed_working();
        assert!(!flaggable(SupervisorState::Alive));
        assert!(flaggable(SupervisorState::Dead));
        assert!(flaggable(SupervisorState::NotRecorded));
        assert!(
            !flaggable(SupervisorState::Unreadable),
            "Unreadable is indeterminate: must NOT flag stillborn/orphaned"
        );
        assert!(
            !flaggable(SupervisorState::Unknown),
            "Unknown is indeterminate: must NOT flag stillborn/orphaned"
        );
    }

    /// The wire spelling of every `SupervisorState` variant is pinned (a rename
    /// is a wire-contract change, not a silent refactor).
    #[test]
    fn supervisor_state_wire_spellings() {
        for (state, wire) in [
            (SupervisorState::Alive, "alive"),
            (SupervisorState::Dead, "dead"),
            (SupervisorState::NotRecorded, "not-recorded"),
            (SupervisorState::Unreadable, "unreadable"),
            (SupervisorState::Unknown, "unknown"),
        ] {
            assert_eq!(serde_json::to_value(state).unwrap(), json!(wire));
        }
    }
}
