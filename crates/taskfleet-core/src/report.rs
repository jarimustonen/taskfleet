//! §7.3 terminal-report payload validation (design.md §7.3).
//!
//! Validates the structural shape of a `node.report` payload — `success`
//! required; optional `summary`, `cancelled`/`reason`, `discussion_items`,
//! `spinoff_proposals`, `wrap_up_recommendations` — before the reducer
//! ever projects it. Lives in `taskfleet-core` (not the CLI) so the supervisor
//! can validate child reports with the same rules it would consume
//! (design.md §7.3 step 3), rather than copying the validator or depending
//! on the CLI crate.
//!
//! Errors are domain-typed ([`ReportValidationError`]); the CLI maps them
//! to its `CliError` envelope at the boundary.
//!
//! # Two validation modes
//!
//! - **Strict** ([`validate_report_payload`]) — the whole payload passes or
//!   the first schema violation is returned. Used by `node report` (an agent
//!   self-submission) and merge-recovery's synthesized report, where the caller
//!   controls the shape and a malformed field is a real bug to surface.
//! - **Lenient advisory** ([`sanitize_report_advisory`]) — the REQUIRED and
//!   correctness-bearing fields (`success`, `cancelled`/`reason`) are still
//!   validated strictly, but the *advisory* sections (`summary`,
//!   `discussion_items`, `spinoff_proposals`, `wrap_up_recommendations`) degrade
//!   gracefully: a malformed element is dropped and reported as a machine-readable
//!   [`AdvisoryWarning`] rather than rejecting the whole payload. Used by
//!   `run merge --report-file` so an advisory-field typo can no longer block a
//!   clean, already-committed code merge (issue `merge-report-schema-lenience`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::schema::Kind;

/// The report-payload key under which a typed [`ReportOrigin`] is serialized
/// (issue `typed-report-origin`).
pub const REPORT_ORIGIN_KEY: &str = "origin";

/// The legacy `via` marker `run merge` stamps on the terminal `node.report` it
/// appends after a clean merge, alongside the typed [`ReportOrigin::RunMerge`].
/// This is the taskfleet/taskfleet-core contract point: the CLI
/// (`crates/taskfleet/src/run/merge.rs`) writes it, and — for a legacy on-disk
/// report carrying NO `origin` field — [`ReportOrigin::report_is_confirmed_merge`]
/// reads it as the fallback merge signal. Retained for backward compatibility with
/// pre-typed-origin runs and downgrade-reading older CLIs; the typed origin is the
/// authority for a report that carries one (issue `retire-via-string`). Lives here
/// beside [`REPORT_ORIGIN_KEY`] as a wire-protocol constant, and is re-exported at
/// the crate root (`taskfleet_core::VIA_EXPLICIT_MERGE`) for the CLI writers.
pub const VIA_EXPLICIT_MERGE: &str = "explicit-merge";

/// The typed provenance of a `node.report` — WHO authored it (issue
/// `typed-report-origin`).
///
/// Before this field, `supervise::outcome` split a terminal report's outcome by
/// sniffing string conventions: a `reason` that `starts_with("agent-")` or is one
/// of a hard-coded set meant "supervisor failure", and `via: "explicit-merge"`
/// from *any* author meant "merged". Those conventions are brittle (a new
/// supervisor reason silently misclassifies) and conflate the report's AUTHOR with
/// its content. `ReportOrigin` records the author explicitly on the event, so the
/// outcome table can read a typed fact instead of pattern-matching prose.
///
/// The origin is stamped by the code path that appends the report, never accepted
/// from an untrusted payload: `run merge` stamps [`ReportOrigin::RunMerge`] (the
/// SOLE merge authority — an agent's `node report` cannot assert it; that path
/// normalizes any supplied origin back to [`ReportOrigin::Agent`]), the supervisor
/// stamps [`ReportOrigin::Supervisor`] on every report it synthesizes, and an
/// agent self-submission is [`ReportOrigin::Agent`]. This keeps merge authorization
/// tied to the run-merge path exactly as the legacy `via` marker did — the typed
/// origin is a parallel, higher-fidelity signal, not a new trust boundary.
///
/// Serialized under [`REPORT_ORIGIN_KEY`] with an internal `kind` tag, e.g.
/// `{"kind": "agent"}`, `{"kind": "supervisor"}`,
/// `{"kind": "run-merge", "op_id": "…", "worker_oid": "…"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ReportOrigin {
    /// The worker agent authored this report itself — a `node report`
    /// self-submission (a success handoff, or a blocked `success: false` handoff).
    Agent,
    /// The supervisor/launcher synthesized this report: a told worker-exit
    /// failure, the crash backstop, or a re-spawn-exhausted failure.
    Supervisor,
    /// Stamped by the `run merge` transaction (or its crash recovery) — the ONLY
    /// authority for a merge/success outcome. The immutable transaction OIDs are
    /// carried for provenance/forensics; they are absent only on the legacy
    /// unguarded merge path (no concrete source branch / stubbed git) where no
    /// transaction was recorded, but the discriminant alone still identifies the
    /// report as a genuine `run merge`.
    RunMerge {
        /// The merge transaction's `op_id`, when a transaction was recorded.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        op_id: Option<String>,
        /// The worker tip OID the merge integrated, when a transaction was recorded.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worker_oid: Option<String>,
    },
}

impl ReportOrigin {
    /// Read the typed origin from a report payload, returning `None` for BOTH an
    /// absent `origin` field (a legacy report written before this field existed)
    /// AND a present-but-malformed one (corrupt / hand-edited / a future variant).
    ///
    /// IMPORTANT — `None` does NOT mean "fall back to the legacy `via`/`reason`
    /// string path". That fallback is gated on the `origin` KEY being genuinely
    /// ABSENT (`report.get(REPORT_ORIGIN_KEY).is_none()`), NOT on this returning
    /// `None`. A present-but-malformed origin is "typed, but unknown authority":
    /// it must NEVER re-unlock the forgeable legacy path (a merge on a forged
    /// `via`, a supervisor failure on a spoofed `reason`). Every consumer
    /// (`report_is_confirmed_merge`, `supervise::outcome::classify` /
    /// `is_supervisor_failure`) checks key presence separately for exactly this
    /// reason — do not collapse the two. See `report_is_confirmed_merge`.
    #[must_use]
    pub fn from_report(report: &Value) -> Option<Self> {
        let raw = report.get(REPORT_ORIGIN_KEY)?;
        serde_json::from_value(raw.clone()).ok()
    }

    /// True when a terminal `node.report` payload is a CONFIRMED, SUCCESSFUL
    /// `run merge` — the sole authority for a merge/success outcome (issue
    /// `retire-via-string`).
    ///
    /// The typed [`ReportOrigin::RunMerge`] (stamped only by the `run merge`
    /// transaction / its crash recovery — an agent's `node report` is normalized
    /// to [`ReportOrigin::Agent`]) is the authoritative marker. The legacy
    /// `via: "explicit-merge"` string is honored ONLY as a fallback for a report
    /// that carries NO `origin` field at all — a legacy on-disk report written
    /// before the typed origin existed. Gating the `via` fallback on a genuinely
    /// ABSENT origin (not on [`from_report`](Self::from_report) returning `None`)
    /// is what makes the typed field strictly stronger: a report that DOES carry
    /// an `origin` field — parsed or malformed/hand-edited — never earns merge
    /// status on a forged `via` string alone. This mirrors
    /// `supervise::outcome::classify`'s merge gate exactly, so the reducer, the
    /// `landed` fallback, and `run wait`'s `merged` flag all agree on the one
    /// merge truth.
    ///
    /// Requires `success == true` and `cancelled` absent/`false`: a payload
    /// carrying the merge marker but `success: false` (malformed/spoofed) or a
    /// cancel is NOT a merge. Boolean typing is strict — a non-boolean `success`
    /// / `cancelled` reads as not-a-merge rather than erroring, so a replay of
    /// such a dead event stays a clean no-op.
    #[must_use]
    pub fn report_is_confirmed_merge(report: &Value) -> bool {
        let success = matches!(report.get("success"), Some(Value::Bool(true)));
        let not_cancelled = matches!(
            report.get("cancelled"),
            None | Some(Value::Null | Value::Bool(false))
        );
        if !(success && not_cancelled) {
            return false;
        }
        // Prefer the typed origin; the legacy `via` string is authority ONLY when
        // no `origin` field is present (a pre-typed-origin on-disk report).
        let is_run_merge_origin = matches!(
            Self::from_report(report),
            Some(ReportOrigin::RunMerge { .. })
        );
        let origin_present = report.get(REPORT_ORIGIN_KEY).is_some();
        let legacy_via_merge = !origin_present
            && report.get("via").and_then(Value::as_str) == Some(VIA_EXPLICIT_MERGE);
        is_run_merge_origin || legacy_via_merge
    }

    /// Whether a confirmed merge report may replace a node's settled outcome.
    ///
    /// This is the single transition predicate shared by projection reduction
    /// and log-derived status replay. A confirmed `run merge` may repair a
    /// failed node (or refresh an already-done node's authoritative report),
    /// but it never overrides cancellation or any future deliberate terminal
    /// outcome.
    #[must_use]
    pub fn permits_terminal_merge_recovery(status: crate::Status, report: &Value) -> bool {
        matches!(status, crate::Status::Failed | crate::Status::Done)
            && Self::report_is_confirmed_merge(report)
    }

    /// Stamp this origin into a report payload under [`REPORT_ORIGIN_KEY`],
    /// overwriting any existing value. A no-op if `report` is not a JSON object
    /// (callers always pass an object — the §7.3 validator rejects non-objects
    /// before this point).
    pub fn stamp(&self, report: &mut Value) {
        if let Some(obj) = report.as_object_mut() {
            // Serializing a tagged enum with only `Option::None` extra fields
            // yields a plain object, so this never fails for these variants.
            if let Ok(v) = serde_json::to_value(self) {
                obj.insert(REPORT_ORIGIN_KEY.to_string(), v);
            }
        }
    }
}

/// A §7.3 report payload failed structural validation.
///
/// Every variant describes one schema violation. The CLI renders these as
/// a `schema_violation` error; [`ReportValidationError::expected`] supplies
/// the machine-readable `expected` hint for the variants that carry one.
#[derive(Debug, thiserror::Error)]
pub enum ReportValidationError {
    /// The payload root was not a JSON object.
    #[error("report payload must be a JSON object")]
    NotObject,

    /// The required `success` field was absent.
    #[error("report payload missing required field `success`")]
    MissingSuccess,

    /// `success` was present but not a boolean.
    #[error("field `success` must be a boolean")]
    SuccessNotBoolean,

    /// `summary` was present but not a string (or null).
    #[error("field `summary` must be a string")]
    SummaryNotString,

    /// `cancelled` was present but not a boolean.
    #[error("field `cancelled` must be a boolean")]
    CancelledNotBoolean,

    /// `reason` was present but not a string.
    #[error("field `reason` must be a string")]
    ReasonNotString,

    /// `cancelled: true` was paired with `success: true` (§7.7 forbids it).
    #[error("`cancelled: true` requires `success: false`")]
    CancelledRequiresSuccessFalse,

    /// `cancelled: true` lacked a non-empty `reason` string (§7.7).
    #[error("`cancelled: true` requires a non-empty `reason` string")]
    CancelledRequiresReason,

    /// `discussion_items` was present but not an array.
    #[error("field `discussion_items` must be an array")]
    DiscussionItemsNotArray,

    /// A `discussion_items` element was not a JSON object.
    #[error("discussion_items[{index}] must be a JSON object")]
    DiscussionItemNotObject {
        /// Index of the offending element.
        index: usize,
    },

    /// A `discussion_items` element lacked a non-empty `topic` string.
    #[error("discussion_items[{index}].topic must be a non-empty string")]
    DiscussionItemTopicMissing {
        /// Index of the offending element.
        index: usize,
    },

    /// A `discussion_items` element's `severity` was not a string.
    #[error("discussion_items[{index}].severity must be a string")]
    DiscussionItemSeverityNotString {
        /// Index of the offending element.
        index: usize,
    },

    /// `spinoff_proposals` was present but not an array.
    #[error("field `spinoff_proposals` must be an array")]
    SpinoffProposalsNotArray,

    /// A `spinoff_proposals` element was not a JSON object.
    #[error("spinoff_proposals[{index}] must be a JSON object")]
    SpinoffProposalNotObject {
        /// Index of the offending element.
        index: usize,
    },

    /// A `spinoff_proposals` element lacked a non-empty `proposed_title`.
    #[error("spinoff_proposals[{index}].proposed_title must be a non-empty string")]
    SpinoffProposalTitleMissing {
        /// Index of the offending element.
        index: usize,
    },

    /// A `spinoff_proposals` element's `proposed_kind` was not a string.
    #[error("spinoff_proposals[{index}].proposed_kind must be a string")]
    SpinoffProposalKindNotString {
        /// Index of the offending element.
        index: usize,
    },

    /// A `spinoff_proposals` element's `proposed_kind` was not a known [`Kind`].
    #[error("spinoff_proposals[{index}].proposed_kind `{kind}` is not a known kind")]
    SpinoffProposalKindUnknown {
        /// Index of the offending element.
        index: usize,
        /// The rejected kind string.
        kind: String,
    },

    /// A `spinoff_proposals` element's `rationale` was not a string (or null).
    #[error("spinoff_proposals[{index}].rationale must be a string")]
    SpinoffProposalRationaleNotString {
        /// Index of the offending element.
        index: usize,
    },

    /// A declared string-array field was not an array.
    #[error("field `{field}` must be an array")]
    FieldNotArray {
        /// The offending field name.
        field: String,
    },

    /// An element of a declared string-array field was not a string.
    #[error("{field}[{index}] must be a string")]
    FieldElementNotString {
        /// The offending field name.
        field: String,
        /// Index of the offending element.
        index: usize,
    },

    /// A nested path expected to hold a string array was not an array.
    #[error("{path} must be an array")]
    PathNotArray {
        /// Dotted/indexed path to the offending value.
        path: String,
    },

    /// An element at a nested string-array path was not a string.
    #[error("{path}[{index}] must be a string")]
    PathElementNotString {
        /// Dotted/indexed path to the offending array.
        path: String,
        /// Index of the offending element.
        index: usize,
    },
}

impl ReportValidationError {
    /// The machine-readable `expected` hint for this error, if any.
    ///
    /// Mirrors the `with_expected(...)` payloads the CLI previously
    /// attached inline, so callers can surface the same structured hint.
    #[must_use]
    pub fn expected(&self) -> Option<Value> {
        match self {
            Self::MissingSuccess | Self::SuccessNotBoolean => {
                Some(serde_json::json!({"field": "success", "type": "boolean"}))
            }
            // Source the accepted kinds from the enum so the hint can never
            // drift from what the validator actually accepts (see
            // `Kind::WIRE_NAMES` and its serde round-trip test).
            Self::SpinoffProposalKindUnknown { .. } => Some(serde_json::json!(Kind::WIRE_NAMES)),
            _ => None,
        }
    }
}

/// Validate a §7.3 report payload's structural shape.
///
/// Rejects anything obviously not a report before the reducer ever sees
/// it, so the caller can name the offending field instead of bubbling a
/// generic `CorruptEventLog`. Keeps the current validation logic verbatim;
/// this is a relocation, not a tightening.
///
/// # Errors
///
/// Returns a [`ReportValidationError`] describing the first schema
/// violation found.
pub fn validate_report_payload(data: &Value) -> Result<(), ReportValidationError> {
    let obj = data.as_object().ok_or(ReportValidationError::NotObject)?;

    validate_required_fields(obj)?;

    if let Some(v) = obj.get("summary") {
        if !v.is_string() && !v.is_null() {
            return Err(ReportValidationError::SummaryNotString);
        }
    }

    validate_discussion_items(obj.get("discussion_items"))?;
    validate_spinoff_proposals(obj.get("spinoff_proposals"))?;
    validate_string_array(
        obj.get("wrap_up_recommendations"),
        "wrap_up_recommendations",
    )?;
    Ok(())
}

/// Validate the REQUIRED, correctness-bearing fields — `success` and the
/// `cancelled`/`reason` §7.7 cross-constraints. These are strict in BOTH
/// validation modes: they gate the terminal outcome and teardown, so a malformed
/// one is never degraded to a warning (issue `merge-report-schema-lenience`).
fn validate_required_fields(
    obj: &serde_json::Map<String, Value>,
) -> Result<(), ReportValidationError> {
    // `success` is the one strictly required field per §7.3. A cancel-
    // synthesized report (§7.7) may carry `cancelled: true` AND
    // `success: false` — both are still booleans on the wire.
    let success = obj
        .get("success")
        .ok_or(ReportValidationError::MissingSuccess)?;
    if !success.is_boolean() {
        return Err(ReportValidationError::SuccessNotBoolean);
    }

    let cancelled = match obj.get("cancelled") {
        None | Some(Value::Null) => false,
        Some(v) => v
            .as_bool()
            .ok_or(ReportValidationError::CancelledNotBoolean)?,
    };
    let reason = match obj.get("reason") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_str().ok_or(ReportValidationError::ReasonNotString)?),
    };

    // §7.7: a cancel-synthesized report carries `cancelled: true,
    // success: false, reason: <non-empty>`. Allowing `success: true`
    // alongside `cancelled: true` would persist a contradiction (the
    // reducer prioritizes `cancelled`, so the node would be cancelled
    // while `last_report.success == true`).
    if cancelled {
        // `success` was confirmed a boolean above, so this never panics;
        // `expect` documents that invariant rather than masking a reorder
        // bug behind `unwrap_or(false)`.
        if success
            .as_bool()
            .expect("success validated as boolean above")
        {
            return Err(ReportValidationError::CancelledRequiresSuccessFalse);
        }
        match reason {
            Some(s) if !s.trim().is_empty() => {}
            _ => return Err(ReportValidationError::CancelledRequiresReason),
        }
    }
    Ok(())
}

fn validate_discussion_items(v: Option<&Value>) -> Result<(), ReportValidationError> {
    let arr = match v {
        Some(Value::Array(a)) => a,
        Some(_) => return Err(ReportValidationError::DiscussionItemsNotArray),
        None => return Ok(()),
    };
    for (i, item) in arr.iter().enumerate() {
        validate_discussion_item(item, i)?;
    }
    Ok(())
}

/// Validate ONE `discussion_items` element. Extracted so both the strict
/// validator (first error wins) and the lenient sanitizer (drop the offending
/// element, keep the rest) share the exact same per-element rules.
fn validate_discussion_item(item: &Value, index: usize) -> Result<(), ReportValidationError> {
    let obj = item
        .as_object()
        .ok_or(ReportValidationError::DiscussionItemNotObject { index })?;
    let topic = obj.get("topic").and_then(Value::as_str);
    if topic.is_none_or(|t| t.trim().is_empty()) {
        return Err(ReportValidationError::DiscussionItemTopicMissing { index });
    }
    if let Some(sev) = obj.get("severity") {
        if !sev.is_string() {
            return Err(ReportValidationError::DiscussionItemSeverityNotString { index });
        }
        // §7.3 example lists "discuss|critical" but the design
        // calls for forward-compatibility — accept any string and
        // let the supervisor interpret unknown severities. (A
        // CLI-side closed-set check would deadlock agents shipped
        // ahead of a CLI release; see review #2/DeepSeek and #15
        // /Claude.)
    }
    if let Some(opts) = obj.get("options") {
        validate_string_array_at(opts, &format!("discussion_items[{index}].options"))?;
    }
    Ok(())
}

fn validate_spinoff_proposals(v: Option<&Value>) -> Result<(), ReportValidationError> {
    let arr = match v {
        Some(Value::Array(a)) => a,
        Some(_) => return Err(ReportValidationError::SpinoffProposalsNotArray),
        None => return Ok(()),
    };
    for (i, item) in arr.iter().enumerate() {
        validate_spinoff_proposal(item, i)?;
    }
    Ok(())
}

/// Validate ONE `spinoff_proposals` element. Extracted so both the strict
/// validator and the lenient sanitizer share the exact same per-element rules —
/// including the `proposed_title`/`proposed_kind` field names whose intuitive
/// typos (`title`/`detail`) motivated the lenient mode (issue
/// `merge-report-schema-lenience`).
fn validate_spinoff_proposal(item: &Value, index: usize) -> Result<(), ReportValidationError> {
    let obj = item
        .as_object()
        .ok_or(ReportValidationError::SpinoffProposalNotObject { index })?;
    let title = obj.get("proposed_title").and_then(Value::as_str);
    if title.is_none_or(|t| t.trim().is_empty()) {
        return Err(ReportValidationError::SpinoffProposalTitleMissing { index });
    }
    let kind_str = obj
        .get("proposed_kind")
        .and_then(Value::as_str)
        .ok_or(ReportValidationError::SpinoffProposalKindNotString { index })?;
    // Reject unknown kinds at the boundary so the supervisor never
    // has to translate a generic `CorruptEventLog` for the user. The
    // accepted set is the enum's *creatable* wire names — the read-only
    // `Kind::Unknown` catch-all is deliberately excluded (a proposal must
    // name a live kind), so this checks membership rather than round-tripping
    // through serde (which would silently map any unknown string to
    // `Kind::Unknown`).
    if !Kind::WIRE_NAMES.contains(&kind_str) {
        return Err(ReportValidationError::SpinoffProposalKindUnknown {
            index,
            kind: kind_str.to_string(),
        });
    }
    if let Some(rationale) = obj.get("rationale") {
        if !rationale.is_string() && !rationale.is_null() {
            return Err(ReportValidationError::SpinoffProposalRationaleNotString { index });
        }
    }
    Ok(())
}

/// Path-aware string-array validator. Used for nested fields where the
/// caller wants to embed an index in the error message.
fn validate_string_array_at(v: &Value, path: &str) -> Result<(), ReportValidationError> {
    let arr = v
        .as_array()
        .ok_or_else(|| ReportValidationError::PathNotArray {
            path: path.to_string(),
        })?;
    for (i, item) in arr.iter().enumerate() {
        if !item.is_string() {
            return Err(ReportValidationError::PathElementNotString {
                path: path.to_string(),
                index: i,
            });
        }
    }
    Ok(())
}

fn validate_string_array(v: Option<&Value>, field: &str) -> Result<(), ReportValidationError> {
    let arr = match v {
        Some(Value::Array(a)) => a,
        Some(_) => {
            return Err(ReportValidationError::FieldNotArray {
                field: field.to_string(),
            })
        }
        None => return Ok(()),
    };
    for (i, item) in arr.iter().enumerate() {
        if !item.is_string() {
            return Err(ReportValidationError::FieldElementNotString {
                field: field.to_string(),
                index: i,
            });
        }
    }
    Ok(())
}

/// A machine-readable warning that one advisory report field (or one element of
/// an advisory array) was dropped during lenient sanitization
/// (issue `merge-report-schema-lenience`).
///
/// Emitted by [`sanitize_report_advisory`] and surfaced in `run merge`'s JSON
/// envelope so an agent reads a structured record of what was discarded instead
/// of regex-parsing prose. The dropped data was never correctness-bearing (see
/// [`sanitize_report_advisory`] for the strict/advisory split), so a warning —
/// not a rejected merge — is the right severity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdvisoryWarning {
    /// The advisory field the drop applies to, e.g. `spinoff_proposals`.
    pub field: String,
    /// The element index dropped, when the field itself was kept but one element
    /// was invalid. Absent (`None`) when the entire field was dropped (e.g. it was
    /// present but not an array).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    /// Human-readable reason — the underlying [`ReportValidationError`] rendered.
    pub reason: String,
}

impl AdvisoryWarning {
    /// Render as a single human-readable line for the CLI's `warnings` string list,
    /// e.g. `dropped spinoff_proposals[0]: … must be a non-empty string`.
    #[must_use]
    pub fn to_message(&self) -> String {
        match self.index {
            Some(i) => format!("dropped {}[{i}]: {}", self.field, self.reason),
            None => format!("dropped advisory field `{}`: {}", self.field, self.reason),
        }
    }
}

/// The result of [`sanitize_report_advisory`]: a report with malformed *advisory*
/// sections removed, plus the machine-readable [`AdvisoryWarning`]s describing
/// every drop.
#[derive(Debug, Clone)]
pub struct SanitizedReport {
    /// The report with malformed advisory fields/elements dropped. Required and
    /// correctness-bearing fields are untouched (they were validated strictly).
    pub report: Value,
    /// One entry per dropped advisory field or element; empty when nothing was
    /// dropped (the report validated cleanly).
    pub warnings: Vec<AdvisoryWarning>,
}

/// Validate a §7.3 report payload with **lenient advisory handling** — the
/// merge-first posture of `run merge --report-file` (issue
/// `merge-report-schema-lenience`).
///
/// The REQUIRED, correctness-bearing fields are still strict — a violation here
/// returns `Err` exactly as [`validate_report_payload`] would:
/// - the payload root must be a JSON object,
/// - `success` must be present and boolean, and
/// - the `cancelled`/`reason` §7.7 cross-constraints must hold.
///
/// These gate the node's terminal outcome and the supervisor's teardown, so a
/// malformed one is never silently degraded — that would risk mis-terminalizing a
/// node or stranding teardown.
///
/// Everything else is **advisory** and degrades gracefully: a malformed value is
/// dropped from the returned [`SanitizedReport::report`] and recorded as an
/// [`AdvisoryWarning`] instead of failing the call. This covers:
/// - `summary` (a non-string/non-null scalar → field dropped),
/// - `discussion_items` / `spinoff_proposals` (a non-array → whole field dropped;
///   an invalid element → that WHOLE element dropped, valid siblings kept),
/// - `wrap_up_recommendations` (a non-array → field dropped; a non-string element
///   → that element dropped).
///
/// **Element granularity is coarse by design:** an element is validated as a unit,
/// so a malformed *nested* value (e.g. a non-string inside a `discussion_items[i].
/// options` array, or a bad `spinoff_proposals[i].proposed_kind`) drops the entire
/// containing element — its valid siblings like `topic` go with it. This keeps the
/// lenient rules byte-identical to the strict per-element validators (no rule
/// drift) at the cost of not salvaging partial elements; salvaging a typo is what
/// motivated leniency, and dropping one malformed proposal is an acceptable price.
///
/// This is what stops an advisory-field typo (the recurring `title`/`detail`
/// instead of `proposed_title`/`proposed_kind`/`rationale`) from rejecting the
/// whole terminal report and blocking a clean, already-committed code merge.
///
/// **Provenance is NOT validated here — the caller owns it.** This function
/// touches only the required and advisory fields above; every OTHER top-level key
/// (`origin`, `via`, and any unknown agent key) is preserved verbatim from the
/// input. It deliberately does NOT establish provenance trust: a caller that
/// persists the result MUST stamp the authoritative [`ReportOrigin`] itself (as
/// `run merge` does after this returns) — never trust a payload-supplied `origin`
/// / `via`. See [`ReportOrigin`]'s "never accepted from an untrusted payload"
/// contract; this sanitizer is a shape check, not a trust boundary.
///
/// # Errors
///
/// Returns a [`ReportValidationError`] only for a violation of a required field
/// (root shape, `success`, `cancelled`/`reason`). Advisory violations never error.
pub fn sanitize_report_advisory(data: &Value) -> Result<SanitizedReport, ReportValidationError> {
    let obj = data.as_object().ok_or(ReportValidationError::NotObject)?;

    // Required/correctness-bearing fields stay strict.
    validate_required_fields(obj)?;

    let mut out = obj.clone();
    let mut warnings = Vec::new();

    // `summary` — advisory descriptive scalar. Drop a malformed one.
    if let Some(v) = obj.get("summary") {
        if !v.is_string() && !v.is_null() {
            out.remove("summary");
            warnings.push(AdvisoryWarning {
                field: "summary".to_string(),
                index: None,
                reason: ReportValidationError::SummaryNotString.to_string(),
            });
        }
    }

    sanitize_element_array(
        &mut out,
        "discussion_items",
        ReportValidationError::DiscussionItemsNotArray,
        validate_discussion_item,
        &mut warnings,
    );
    sanitize_element_array(
        &mut out,
        "spinoff_proposals",
        ReportValidationError::SpinoffProposalsNotArray,
        validate_spinoff_proposal,
        &mut warnings,
    );
    sanitize_string_array_field(&mut out, "wrap_up_recommendations", &mut warnings);

    Ok(SanitizedReport {
        report: Value::Object(out),
        warnings,
    })
}

/// Leniently sanitize one advisory array-of-objects field in place: a
/// present-but-non-array field is dropped whole; each element that fails
/// `validate` is dropped, valid siblings retained. Every drop appends an
/// [`AdvisoryWarning`]. Indices in the warnings are the ORIGINAL element
/// positions, so they line up with what the agent wrote.
fn sanitize_element_array(
    obj: &mut serde_json::Map<String, Value>,
    field: &str,
    not_array_err: ReportValidationError,
    validate: fn(&Value, usize) -> Result<(), ReportValidationError>,
    warnings: &mut Vec<AdvisoryWarning>,
) {
    let Some(v) = obj.get(field) else { return };
    let Some(arr) = v.as_array() else {
        obj.remove(field);
        warnings.push(AdvisoryWarning {
            field: field.to_string(),
            index: None,
            reason: not_array_err.to_string(),
        });
        return;
    };
    let mut kept = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        match validate(item, i) {
            Ok(()) => kept.push(item.clone()),
            Err(e) => warnings.push(AdvisoryWarning {
                field: field.to_string(),
                index: Some(i),
                reason: e.to_string(),
            }),
        }
    }
    obj.insert(field.to_string(), Value::Array(kept));
}

/// Leniently sanitize one advisory string-array field in place: a
/// present-but-non-array field is dropped whole; each non-string element is
/// dropped, string siblings retained.
fn sanitize_string_array_field(
    obj: &mut serde_json::Map<String, Value>,
    field: &str,
    warnings: &mut Vec<AdvisoryWarning>,
) {
    let Some(v) = obj.get(field) else { return };
    let Some(arr) = v.as_array() else {
        obj.remove(field);
        warnings.push(AdvisoryWarning {
            field: field.to_string(),
            index: None,
            reason: ReportValidationError::FieldNotArray {
                field: field.to_string(),
            }
            .to_string(),
        });
        return;
    };
    let mut kept = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        if item.is_string() {
            kept.push(item.clone());
        } else {
            warnings.push(AdvisoryWarning {
                field: field.to_string(),
                index: Some(i),
                reason: ReportValidationError::FieldElementNotString {
                    field: field.to_string(),
                    index: i,
                }
                .to_string(),
            });
        }
    }
    obj.insert(field.to_string(), Value::Array(kept));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- valid payloads ---

    #[test]
    fn validates_minimal_success_payload() {
        let v = json!({"success": true});
        assert!(validate_report_payload(&v).is_ok());
    }

    #[test]
    fn validates_full_success_payload() {
        let v = json!({
            "success": true,
            "summary": "did the thing",
            "discussion_items": [
                {"topic": "naming", "severity": "discuss", "options": ["a", "b"]},
            ],
            "spinoff_proposals": [
                {"proposed_title": "follow-up", "proposed_kind": "spinoff", "rationale": "later"},
            ],
            "wrap_up_recommendations": ["rebase", "squash"],
        });
        assert!(validate_report_payload(&v).is_ok());
    }

    #[test]
    fn discussion_item_unknown_severity_accepted_for_forward_compat() {
        // Forward-compat: a supervisor may add new severities without a
        // CLI release. The validator only enforces severity is a string.
        let v = json!({
            "success": true,
            "discussion_items": [{"topic": "x", "severity": "info"}],
        });
        assert!(validate_report_payload(&v).is_ok());
    }

    #[test]
    fn cancel_synthesized_report_shape_ok() {
        // Mirror of run cancel's synthesized payload (run/cancel.rs).
        let v = json!({
            "success": false,
            "cancelled": true,
            "reason": "cancelled by user",
            "summary": "Run cancelled before agent reported.",
            "discussion_items": [],
            "spinoff_proposals": [],
            "wrap_up_recommendations": [],
        });
        assert!(validate_report_payload(&v).is_ok());
    }

    // --- invalid payloads ---

    #[test]
    fn non_object_root_rejected() {
        let v = json!([1, 2, 3]);
        assert!(matches!(
            validate_report_payload(&v),
            Err(ReportValidationError::NotObject)
        ));
    }

    #[test]
    fn missing_success_rejected() {
        let v = json!({"summary": "no success field"});
        let err = validate_report_payload(&v).unwrap_err();
        assert!(matches!(err, ReportValidationError::MissingSuccess));
        // Missing `success` carries the structured `expected` hint.
        assert_eq!(
            err.expected(),
            Some(json!({"field": "success", "type": "boolean"}))
        );
    }

    #[test]
    fn success_variants_carry_field_type_hint() {
        // Both `success` errors reproduce the exact CLI hint, byte-for-byte.
        let hint = Some(json!({"field": "success", "type": "boolean"}));
        assert_eq!(ReportValidationError::MissingSuccess.expected(), hint);
        assert_eq!(ReportValidationError::SuccessNotBoolean.expected(), hint);
    }

    #[test]
    fn summary_must_be_string() {
        let v = json!({"success": true, "summary": 42});
        assert!(matches!(
            validate_report_payload(&v),
            Err(ReportValidationError::SummaryNotString)
        ));
    }

    #[test]
    fn discussion_item_options_non_array_rejected() {
        let v = json!({
            "success": true,
            "discussion_items": [{"topic": "x", "options": "not-an-array"}],
        });
        assert!(matches!(
            validate_report_payload(&v),
            Err(ReportValidationError::PathNotArray { .. })
        ));
    }

    #[test]
    fn cancelled_requires_non_whitespace_reason() {
        let v = json!({"success": false, "cancelled": true, "reason": "   "});
        assert!(matches!(
            validate_report_payload(&v),
            Err(ReportValidationError::CancelledRequiresReason)
        ));
    }

    #[test]
    fn non_boolean_success_rejected() {
        let v = json!({"success": "yes"});
        assert!(matches!(
            validate_report_payload(&v),
            Err(ReportValidationError::SuccessNotBoolean)
        ));
    }

    #[test]
    fn discussion_item_missing_topic_rejected() {
        let v = json!({
            "success": true,
            "discussion_items": [{"severity": "discuss"}],
        });
        assert!(matches!(
            validate_report_payload(&v),
            Err(ReportValidationError::DiscussionItemTopicMissing { index: 0 })
        ));
    }

    #[test]
    fn discussion_item_non_string_severity_rejected() {
        let v = json!({
            "success": true,
            "discussion_items": [{"topic": "x", "severity": 42}],
        });
        assert!(matches!(
            validate_report_payload(&v),
            Err(ReportValidationError::DiscussionItemSeverityNotString { index: 0 })
        ));
    }

    #[test]
    fn discussion_item_options_must_be_strings() {
        let v = json!({
            "success": true,
            "discussion_items": [{"topic": "x", "options": [1, 2]}],
        });
        assert!(matches!(
            validate_report_payload(&v),
            Err(ReportValidationError::PathElementNotString { index: 0, .. })
        ));
    }

    #[test]
    fn spinoff_unknown_proposed_kind_rejected() {
        let v = json!({
            "success": true,
            "spinoff_proposals": [{"proposed_title": "x", "proposed_kind": "not-a-kind"}],
        });
        let err = validate_report_payload(&v).unwrap_err();
        assert!(matches!(
            err,
            ReportValidationError::SpinoffProposalKindUnknown { index: 0, .. }
        ));
        // Unknown kind surfaces the exact closed-set of known kinds, and
        // that set is the enum's own wire names (no drift).
        assert_eq!(err.expected(), Some(json!(crate::schema::Kind::WIRE_NAMES)));
        assert_eq!(
            err.expected(),
            Some(json!([
                "spinoff",
                "research",
                "technical-decision",
                "fan-out"
            ]))
        );
    }

    #[test]
    fn spinoff_missing_kind_rejected() {
        let v = json!({
            "success": true,
            "spinoff_proposals": [{"proposed_title": "x"}],
        });
        assert!(matches!(
            validate_report_payload(&v),
            Err(ReportValidationError::SpinoffProposalKindNotString { index: 0 })
        ));
    }

    #[test]
    fn cancelled_requires_success_false() {
        let v = json!({"success": true, "cancelled": true, "reason": "x"});
        assert!(matches!(
            validate_report_payload(&v),
            Err(ReportValidationError::CancelledRequiresSuccessFalse)
        ));
    }

    #[test]
    fn cancelled_requires_reason() {
        let v = json!({"success": false, "cancelled": true});
        assert!(matches!(
            validate_report_payload(&v),
            Err(ReportValidationError::CancelledRequiresReason)
        ));
    }

    // --- ReportOrigin (issue `typed-report-origin`) ---

    #[test]
    fn report_origin_round_trips_through_a_report() {
        let cases = [
            ReportOrigin::Agent,
            ReportOrigin::Supervisor,
            ReportOrigin::RunMerge {
                op_id: Some("op-123".into()),
                worker_oid: Some("deadbeef".into()),
            },
            ReportOrigin::RunMerge {
                op_id: None,
                worker_oid: None,
            },
        ];
        for origin in cases {
            let mut report = json!({ "success": true });
            origin.stamp(&mut report);
            assert_eq!(
                ReportOrigin::from_report(&report),
                Some(origin.clone()),
                "round-trip: {origin:?}"
            );
        }
    }

    #[test]
    fn report_origin_serializes_with_kind_tag() {
        let mut report = json!({ "success": true });
        ReportOrigin::Agent.stamp(&mut report);
        assert_eq!(report["origin"], json!({ "kind": "agent" }));

        let mut merge = json!({ "success": true });
        ReportOrigin::RunMerge {
            op_id: Some("op-9".into()),
            worker_oid: Some("abc123".into()),
        }
        .stamp(&mut merge);
        assert_eq!(
            merge["origin"],
            json!({ "kind": "run-merge", "op_id": "op-9", "worker_oid": "abc123" })
        );

        // A bare run-merge (legacy unguarded path) omits the null OID fields.
        let mut bare = json!({ "success": true });
        ReportOrigin::RunMerge {
            op_id: None,
            worker_oid: None,
        }
        .stamp(&mut bare);
        assert_eq!(bare["origin"], json!({ "kind": "run-merge" }));
    }

    #[test]
    fn report_origin_absent_or_malformed_is_none() {
        // A legacy report with no origin field.
        assert_eq!(ReportOrigin::from_report(&json!({ "success": true })), None);
        // A malformed origin is treated as absent (conservative fallback), never
        // an error that could brick classification.
        assert_eq!(
            ReportOrigin::from_report(&json!({ "origin": "not-an-object" })),
            None
        );
        assert_eq!(
            ReportOrigin::from_report(&json!({ "origin": { "kind": "bogus" } })),
            None
        );
    }

    #[test]
    fn report_origin_stamp_overwrites_a_supplied_value() {
        // The enforcement `node report` relies on: stamping Agent discards any
        // caller-supplied merge/supervisor origin.
        let mut report = json!({
            "success": true,
            "origin": { "kind": "run-merge", "op_id": "spoofed" }
        });
        ReportOrigin::Agent.stamp(&mut report);
        assert_eq!(
            ReportOrigin::from_report(&report),
            Some(ReportOrigin::Agent)
        );
    }

    #[test]
    fn report_is_confirmed_merge_prefers_typed_origin() {
        // A RunMerge origin authorizes a merge even with NO legacy `via` string.
        let mut merged = json!({ "success": true });
        ReportOrigin::RunMerge {
            op_id: Some("op-1".into()),
            worker_oid: Some("abc".into()),
        }
        .stamp(&mut merged);
        assert!(ReportOrigin::report_is_confirmed_merge(&merged));

        // A bare RunMerge origin (legacy unguarded path, no OIDs) still counts.
        let mut bare = json!({ "success": true });
        ReportOrigin::RunMerge {
            op_id: None,
            worker_oid: None,
        }
        .stamp(&mut bare);
        assert!(ReportOrigin::report_is_confirmed_merge(&bare));
    }

    #[test]
    fn report_is_confirmed_merge_legacy_via_only_when_origin_absent() {
        // Legacy report (no origin field): the `via` marker is honored.
        assert!(ReportOrigin::report_is_confirmed_merge(&json!({
            "success": true, "via": "explicit-merge"
        })));

        // Present-but-Agent origin + a forged `via`: NOT a merge. Merge authority
        // is the run-merge path; an agent report can't fabricate one on `via`.
        let mut agent = json!({ "success": true, "via": "explicit-merge" });
        ReportOrigin::Agent.stamp(&mut agent);
        assert!(
            !ReportOrigin::report_is_confirmed_merge(&agent),
            "an Agent-origin report must not be a merge even with a forged via"
        );

        // Present-but-MALFORMED origin + a forged `via`: NOT a merge — a corrupt
        // origin field must not re-unlock the legacy via path.
        assert!(!ReportOrigin::report_is_confirmed_merge(&json!({
            "success": true, "via": "explicit-merge", "origin": "garbage-not-an-object"
        })));
        assert!(!ReportOrigin::report_is_confirmed_merge(&json!({
            "success": true, "via": "explicit-merge", "origin": { "kind": "bogus" }
        })));
    }

    #[test]
    fn report_is_confirmed_merge_requires_success_and_not_cancelled() {
        // success:false with a merge marker is not a merge (malformed/spoofed).
        assert!(!ReportOrigin::report_is_confirmed_merge(&json!({
            "success": false, "via": "explicit-merge"
        })));
        // A RunMerge origin on a success:false report is likewise not a merge.
        let mut neg = json!({ "success": false });
        ReportOrigin::RunMerge {
            op_id: None,
            worker_oid: None,
        }
        .stamp(&mut neg);
        assert!(!ReportOrigin::report_is_confirmed_merge(&neg));
        // A cancelled report never counts, even with a RunMerge origin riding along.
        let mut cancelled = json!({ "success": false, "cancelled": true, "reason": "x" });
        ReportOrigin::RunMerge {
            op_id: None,
            worker_oid: None,
        }
        .stamp(&mut cancelled);
        assert!(!ReportOrigin::report_is_confirmed_merge(&cancelled));
        // Non-boolean success (strict typing) is not a merge.
        assert!(!ReportOrigin::report_is_confirmed_merge(&json!({
            "success": "true", "via": "explicit-merge"
        })));
    }

    #[test]
    fn report_origin_stamp_on_non_object_is_noop() {
        let mut not_obj = json!([1, 2, 3]);
        ReportOrigin::Agent.stamp(&mut not_obj);
        assert_eq!(not_obj, json!([1, 2, 3]));
    }

    #[test]
    fn wrap_up_must_be_string_array() {
        let v = json!({
            "success": true,
            "wrap_up_recommendations": ["ok", 42],
        });
        assert!(matches!(
            validate_report_payload(&v),
            Err(ReportValidationError::FieldElementNotString { index: 1, .. })
        ));
    }

    // --- lenient advisory sanitization (issue `merge-report-schema-lenience`) ---

    #[test]
    fn sanitize_clean_report_has_no_warnings() {
        let v = json!({
            "success": true,
            "summary": "did the thing",
            "discussion_items": [{"topic": "naming", "severity": "discuss"}],
            "spinoff_proposals": [
                {"proposed_title": "follow-up", "proposed_kind": "spinoff", "rationale": "later"},
            ],
            "wrap_up_recommendations": ["rebase"],
        });
        let out = sanitize_report_advisory(&v).unwrap();
        assert!(out.warnings.is_empty());
        assert_eq!(out.report, v);
    }

    #[test]
    fn sanitize_drops_typoed_spinoff_proposal_with_warning() {
        // The exact glasspad-stint foot-gun: `title`/`detail` instead of the
        // schema's `proposed_title`/`proposed_kind`/`rationale`. Strict validation
        // would reject the whole report and block the merge; lenient drops the
        // proposal and warns.
        let v = json!({
            "success": true,
            "summary": "green, reviewed, committed",
            "spinoff_proposals": [{"title": "do X later", "detail": "because Y"}],
        });
        // Strict path rejects it (the behavior the issue is fixing).
        assert!(validate_report_payload(&v).is_err());
        // Lenient path merges: no error, proposal dropped, one warning.
        let out = sanitize_report_advisory(&v).unwrap();
        assert_eq!(out.warnings.len(), 1);
        assert_eq!(out.warnings[0].field, "spinoff_proposals");
        assert_eq!(out.warnings[0].index, Some(0));
        assert_eq!(out.report["spinoff_proposals"], json!([]));
        // Required + descriptive fields survive untouched.
        assert_eq!(out.report["success"], json!(true));
        assert_eq!(out.report["summary"], json!("green, reviewed, committed"));
    }

    #[test]
    fn sanitize_keeps_valid_siblings_drops_only_bad_element() {
        let v = json!({
            "success": true,
            "spinoff_proposals": [
                {"proposed_title": "keep me", "proposed_kind": "spinoff"},
                {"title": "typo, drop me"},
                {"proposed_title": "keep me too", "proposed_kind": "research"},
            ],
        });
        let out = sanitize_report_advisory(&v).unwrap();
        assert_eq!(out.warnings.len(), 1);
        assert_eq!(out.warnings[0].index, Some(1));
        let kept = out.report["spinoff_proposals"].as_array().unwrap();
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0]["proposed_title"], json!("keep me"));
        assert_eq!(kept[1]["proposed_title"], json!("keep me too"));
    }

    #[test]
    fn sanitize_drops_non_array_advisory_field_whole() {
        let v = json!({
            "success": true,
            "discussion_items": "not-an-array",
            "wrap_up_recommendations": {"oops": true},
        });
        let out = sanitize_report_advisory(&v).unwrap();
        assert_eq!(out.warnings.len(), 2);
        // Both malformed fields removed entirely.
        assert!(out.report.get("discussion_items").is_none());
        assert!(out.report.get("wrap_up_recommendations").is_none());
        let fields: Vec<&str> = out.warnings.iter().map(|w| w.field.as_str()).collect();
        assert!(fields.contains(&"discussion_items"));
        assert!(fields.contains(&"wrap_up_recommendations"));
        // A whole-field drop carries no element index.
        assert!(out.warnings.iter().all(|w| w.index.is_none()));
    }

    #[test]
    fn sanitize_drops_non_string_wrap_up_element() {
        let v = json!({
            "success": true,
            "wrap_up_recommendations": ["rebase", 42, "squash"],
        });
        let out = sanitize_report_advisory(&v).unwrap();
        assert_eq!(out.warnings.len(), 1);
        assert_eq!(out.warnings[0].index, Some(1));
        assert_eq!(
            out.report["wrap_up_recommendations"],
            json!(["rebase", "squash"])
        );
    }

    #[test]
    fn sanitize_drops_malformed_summary() {
        let v = json!({"success": true, "summary": 42});
        let out = sanitize_report_advisory(&v).unwrap();
        assert_eq!(out.warnings.len(), 1);
        assert_eq!(out.warnings[0].field, "summary");
        assert!(out.report.get("summary").is_none());
    }

    #[test]
    fn sanitize_still_rejects_missing_required_success() {
        // Required-field strictness is preserved: no `success` → error, no merge.
        let v = json!({"summary": "no success field"});
        assert!(matches!(
            sanitize_report_advisory(&v),
            Err(ReportValidationError::MissingSuccess)
        ));
    }

    #[test]
    fn sanitize_still_rejects_non_boolean_success() {
        let v = json!({"success": "yes", "spinoff_proposals": [{"title": "x"}]});
        assert!(matches!(
            sanitize_report_advisory(&v),
            Err(ReportValidationError::SuccessNotBoolean)
        ));
    }

    #[test]
    fn sanitize_still_rejects_cancelled_contradiction() {
        // The §7.7 cross-constraint is correctness-bearing (it drives terminal
        // outcome), so it stays strict even in lenient mode.
        let v = json!({"success": true, "cancelled": true, "reason": "x"});
        assert!(matches!(
            sanitize_report_advisory(&v),
            Err(ReportValidationError::CancelledRequiresSuccessFalse)
        ));
    }

    #[test]
    fn sanitize_rejects_non_object_root() {
        let v = json!([1, 2, 3]);
        assert!(matches!(
            sanitize_report_advisory(&v),
            Err(ReportValidationError::NotObject)
        ));
    }

    #[test]
    fn sanitize_preserves_unknown_and_provenance_fields() {
        // The sanitizer is a SHAPE check, not a trust boundary: it must pass
        // through `origin`, `via`, and unknown agent keys untouched. (`run merge`
        // re-stamps the authoritative origin/via afterward — provenance trust is
        // the caller's job, per the doc contract.)
        let v = json!({
            "success": true,
            "origin": {"kind": "agent"},
            "via": "explicit-merge",
            "custom_agent_key": {"nested": [1, 2, 3]},
            "spinoff_proposals": [{"title": "typo, drop me"}],
        });
        let out = sanitize_report_advisory(&v).unwrap();
        assert_eq!(out.warnings.len(), 1, "only the bad proposal is dropped");
        assert_eq!(out.report["origin"], json!({"kind": "agent"}));
        assert_eq!(out.report["via"], json!("explicit-merge"));
        assert_eq!(out.report["custom_agent_key"], json!({"nested": [1, 2, 3]}));
    }

    #[test]
    fn sanitize_nested_options_drops_whole_discussion_item() {
        // A malformed nested `options` drops the ENTIRE element (coarse by design),
        // taking the otherwise-valid `topic` with it. Documented behavior.
        let v = json!({
            "success": true,
            "discussion_items": [
                {"topic": "keep", "severity": "discuss"},
                {"topic": "drop me", "options": ["ok", 42]},
            ],
        });
        let out = sanitize_report_advisory(&v).unwrap();
        assert_eq!(out.warnings.len(), 1);
        assert_eq!(out.warnings[0].index, Some(1));
        let kept = out.report["discussion_items"].as_array().unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["topic"], json!("keep"));
    }

    #[test]
    fn advisory_warning_message_shapes() {
        let elem = AdvisoryWarning {
            field: "spinoff_proposals".to_string(),
            index: Some(2),
            reason: "boom".to_string(),
        };
        assert_eq!(elem.to_message(), "dropped spinoff_proposals[2]: boom");
        let whole = AdvisoryWarning {
            field: "discussion_items".to_string(),
            index: None,
            reason: "not an array".to_string(),
        };
        assert_eq!(
            whole.to_message(),
            "dropped advisory field `discussion_items`: not an array"
        );
    }
}
