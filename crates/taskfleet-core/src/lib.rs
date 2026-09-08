//! Core library for taskfleet.
//!
//! See `issues/taskfleet-mvp/design.md` for the canonical schema and
//! protocol references. This crate provides:
//!
//! - The on-disk schema types ([`Manifest`], [`Node`], [`Event`]).
//! - Atomic write helpers ([`atomic`]) and per-run advisory `flock`
//!   ([`RunLock`]).
//! - The canonical mutation entry point
//!   ([`append_and_apply_event`]): append one
//!   event and fold it into the projections under the run's `flock`.
//!
//! Higher-level supervisor and CLI logic live in their own crates / issues.
//!
//! `taskfleet-core` is the canonical library surface, so public items are required
//! to carry doc comments (`#![warn(missing_docs)]`). Lint-level policy
//! otherwise lives in the workspace `[workspace.lints]` table (pedantic clippy).
#![warn(missing_docs)]

pub mod atomic;
pub mod cancel;
pub mod envelope;
pub mod error;
pub mod events;
pub mod ids;
pub mod lock;
pub mod paths;
pub mod projections;
pub mod reducer;
pub mod report;
pub mod schema;
pub mod telemetry;

#[cfg(test)]
mod stress_tests;

pub use cancel::{
    cancel_node, cancel_node_unlocked, cancel_run, cancel_run_unlocked, read_node_statuses,
    CancelOutcome, NodeCancelOutcome,
};
pub use envelope::SCHEMA_VERSION;
pub use error::{Error, Result};
pub use events::{
    append_and_apply_event, append_and_apply_idempotent, append_and_apply_unlocked,
    find_prior_with_key, quarantine_corrupt_lines, quarantine_corrupt_lines_unlocked,
    read_all_events, recover_last_seq, replay_unapplied_unlocked, AppendOutcome, AppendResult,
    PriorEvent, Quarantine,
};
pub use ids::{format_node_id, new_op_id, new_run_id};
pub use lock::{Exclusive, LockedRun, RunLock, Shared};
pub use paths::{nofollow, run_dir, validate_run_id, RunPaths};
pub use projections::{read_manifest, read_manifest_opt, read_node, read_node_opt, write_node};
pub use reducer::{plan_projections, validate_event, KIND_MERGE_ABORTED, KIND_MERGE_STARTED};
pub use report::{
    sanitize_report_advisory, validate_report_payload, AdvisoryWarning, ReportOrigin,
    ReportValidationError, SanitizedReport, REPORT_ORIGIN_KEY, VIA_EXPLICIT_MERGE,
};
pub use schema::aggregate_terminal_status;
pub use schema::{
    is_run_id_prefix, AgentSelection, AwaitingInput, ChildRef, Event, IdValidationError, Kind,
    Lifecycle, Manifest, MergeTxn, Node, NodeId, RunId, SelectedAgentCandidate,
    SkippedAgentCandidate, Status, WorkerExit, STATE_SCHEMA_VERSION, SUPPORTED_STATE_SCHEMAS,
};
pub use telemetry::{
    parse_telemetry_update, read_all_telemetry, read_all_telemetry_with_clock, read_telemetry,
    read_telemetry_with_clock, update_telemetry, update_telemetry_with_clock, SystemTelemetryClock,
    TelemetryAccepted, TelemetryClock, TelemetryError, TelemetrySampleStatus, TelemetryState,
    TelemetryUpdate, TelemetryView, TELEMETRY_FRESHNESS_SECS, TELEMETRY_MAX_BYTES,
    TELEMETRY_PROTOCOL_VERSION, TELEMETRY_SCHEMA_VERSION,
};

/// Ensure the taskfleet root directory exists (`<root>/runs`,
/// `<root>/logs`). Idempotent.
pub fn ensure_root(root: &std::path::Path) -> Result<()> {
    for sub in ["runs", "logs"] {
        let p = root.join(sub);
        std::fs::create_dir_all(&p).map_err(|e| Error::io(&p, e))?;
    }
    Ok(())
}
