//! On-disk state schema types per `design.md` §1.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The current state-on-disk schema version this crate writes.
pub const STATE_SCHEMA_VERSION: u32 = 1;

/// All state-schema versions this crate can read.
pub const SUPPORTED_STATE_SCHEMAS: &[u32] = &[1];

/// Crockford base32 alphabet in lowercase (excludes `i`, `l`, `o`, `u`). The
/// charset for the bare ULID of a [`RunId`].
const CROCKFORD_LOWER: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// True iff every byte of `s` is a lowercase Crockford base32 character.
fn all_crockford_lower(s: &str) -> bool {
    s.bytes().all(|b| CROCKFORD_LOWER.contains(&b))
}

/// True iff `s` is a syntactically valid (possibly partial) prefix of a
/// [`RunId`]: non-empty, no longer than a full ULID, every character a lowercase
/// Crockford base32 digit, and a first character within ULID's `0..=7`
/// timestamp bound. Used by the CLI to resolve an unambiguous run-id prefix
/// (like `git`) — a value failing this is a malformed argument (`invalid_run_id`),
/// not a legitimate-but-unknown prefix. The first-char bound is enforced because
/// no valid `RunId` can begin outside `0..=7`, so an `8…`/`9…` prefix is
/// impossible rather than merely absent — reporting it as malformed keeps the
/// error class honest and consistent with how [`RunId::parse_str`] rejects a
/// full-length id with the same leading digit.
pub fn is_run_id_prefix(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= RunId::LEN
        && all_crockford_lower(s)
        && matches!(s.as_bytes().first(), Some(b'0'..=b'7'))
}

/// Error returned when a typed identifier fails parse-time validation.
///
/// Every [`RunId`] and [`NodeId`] is
/// constructed only through its `parse_str` constructor (or the equivalent
/// validating `Deserialize`), so any value that reaches a path helper has
/// already been checked for prefix, charset, and length. This is the
/// path-traversal guard: a raw id containing `/`, `..`, or a leading dot can
/// never be turned into one of these newtypes, so it can never name a file
/// outside the run directory.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdValidationError {
    /// The value carried the right prefix (or needs none) but its body had the
    /// wrong length or used characters outside the permitted charset.
    #[error("invalid {kind} id {value:?}: expected {expected}")]
    InvalidFormat {
        /// Which id type rejected the value (`run`, `node`).
        kind: &'static str,
        /// The offending raw value.
        value: String,
        /// Human-readable description of the accepted shape (e.g. `n-NNNN`).
        expected: &'static str,
    },
    /// The value did not start with the id type's required prefix (`n-`).
    #[error("invalid {kind} id: wrong prefix, expected {expected}")]
    WrongPrefix {
        /// Which id type rejected the value.
        kind: &'static str,
        /// Human-readable description of the accepted shape.
        expected: &'static str,
    },
}

impl IdValidationError {
    /// The id type that rejected the value (`run`, `node`).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidFormat { kind, .. } | Self::WrongPrefix { kind, .. } => kind,
        }
    }

    /// The accepted-shape hint, suitable for the `expected` field of a CLI
    /// error envelope.
    pub fn expected(&self) -> &'static str {
        match self {
            Self::InvalidFormat { expected, .. } | Self::WrongPrefix { expected, .. } => expected,
        }
    }
}

/// Generate the shared trait surface for a validated id newtype: `as_str`,
/// `FromStr`, `Display`, `Debug`, `Ord` / `PartialOrd` (lexicographic over the
/// inner string), `Serialize` (as the bare string), and a validating
/// `Deserialize` (delegates to `parse_str`, so reading an old file with a
/// malformed id fails loudly rather than silently widening the type). Each
/// newtype supplies its own `parse_str` in a separate `impl` block.
///
/// `Ord` / `PartialOrd` are derived, so they forward to the inner `String`'s
/// ordering — i.e. plain `&str` byte comparison. For the fixed-width ULID form
/// ([`RunId`]) this preserves the natural time ordering ULIDs encode in their
/// lexical sort.
///
/// CAVEAT — this ordering is lexical, *not* numeric or semantic: [`NodeId`] is
/// `n-` + a variable-width number, so once the counter grows a digit the byte
/// order diverges from the numeric order: `n-10000 < n-9999`. Do not sort
/// `NodeId`s expecting ascending node number; parse the body if you need that.
///
/// The trait is provided for `BTreeMap`/`BTreeSet` keys and stable sorts.
macro_rules! id_newtype {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            /// The validated id as a string slice. There is no mutable or
            /// owned-`String` accessor by design: the inner value can never be
            /// mutated into an unvalidated state after construction.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::str::FromStr for $name {
            type Err = IdValidationError;

            /// Parse via the newtype's own `parse_str`; lets callers use the
            /// `str::parse` / `FromStr` ecosystem (`s.parse::<RunId>()?`).
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse_str(s)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}({:?})", stringify!($name), self.0)
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.0)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                Self::parse_str(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

id_newtype! {
    /// A validated run identifier: a lowercase ULID (26 Crockford base32
    /// characters whose first character keeps the encoded timestamp within
    /// ULID's 48-bit range). Mirrors what [`crate::new_run_id`] emits.
    RunId
}

impl RunId {
    /// Accepted-shape hint shared by every rejection.
    const EXPECTED: &'static str = "26-char lowercase Crockford base32 ULID";
    /// Canonical length of a ULID in Crockford base32. Public so CLI-side prefix
    /// resolution can branch on "full id vs. prefix" without mirroring the
    /// constant (which would silently drift if the id shape ever changed).
    pub const LEN: usize = 26;

    /// Parse and validate a `run_id`. Accepts only the 26-character lowercase
    /// ULID shape; rejects wrong length, non-Crockford characters, and a first
    /// character outside `0..=7` (which would overflow ULID's 48-bit timestamp).
    pub fn parse_str(s: &str) -> Result<Self, IdValidationError> {
        let reject = || IdValidationError::InvalidFormat {
            kind: "run",
            value: s.to_string(),
            expected: Self::EXPECTED,
        };
        if s.len() != Self::LEN || !all_crockford_lower(s) {
            return Err(reject());
        }
        // The first base32 char carries the top 5 bits of the 128-bit ULID;
        // the 48-bit timestamp cannot overflow only if it is in `0..=7`.
        if !(b'0'..=b'7').contains(&s.as_bytes()[0]) {
            return Err(reject());
        }
        Ok(Self(s.to_string()))
    }
}

id_newtype! {
    /// A validated node identifier: `n-` followed by 4 or more ASCII digits
    /// (e.g. `n-0001`). Mirrors what [`crate::format_node_id`] emits.
    NodeId
}

impl NodeId {
    /// Accepted-shape hint shared by every rejection.
    const EXPECTED: &'static str = "n-NNNN (n- followed by 4-10 ASCII digits)";

    /// Parse and validate a `node_id`. Requires the `n-` prefix followed by
    /// 4 to 10 ASCII digits; rejects anything else (wrong prefix, too few or
    /// too many digits, non-digit body). The 10-digit ceiling covers the full
    /// `u32` counter range [`crate::format_node_id`] draws from while bounding
    /// the filename length (a defense against `ENAMETOOLONG` from a forged id).
    pub fn parse_str(s: &str) -> Result<Self, IdValidationError> {
        let body = s.strip_prefix("n-").ok_or(IdValidationError::WrongPrefix {
            kind: "node",
            expected: Self::EXPECTED,
        })?;
        if (4..=10).contains(&body.len()) && body.bytes().all(|b| b.is_ascii_digit()) {
            Ok(Self(s.to_string()))
        } else {
            Err(IdValidationError::InvalidFormat {
                kind: "node",
                value: s.to_string(),
                expected: Self::EXPECTED,
            })
        }
    }
}

/// The run/node kind enum (design.md §1.2).
///
/// The 0.2 subtractive cut removed the `code`, `orchestrate`, `orchestrated`,
/// `bugfix`, and `make-skill` kinds (the interactive + DAG-driver topologies and
/// the two phantom variants that were behaviourally `Spinoff`). The surviving
/// kinds are all autonomous. [`Kind::Unknown`] is a read-only catch-all so a
/// legacy on-disk run recorded under a since-removed kind still deserializes —
/// `doctor` / `run list` report it, never delete it (ADR §D7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// Autonomous fire-and-forget task that merges itself back (`/worktree-spinoff`).
    Spinoff,
    /// Autonomous multi-source research worktree (`/worktree-research`).
    Research,
    /// Drives one architectural decision to an ADR (`/worktree-technical-decision`).
    TechnicalDecision,
    /// Parallel fan-out of many identical units (`/fan-out`).
    FanOut,
    /// A kind this build no longer models — a legacy run recorded on disk under a
    /// kind removed in the 0.2 cut (`code` / `orchestrate` / `orchestrated` /
    /// `bugfix` / `make-skill`), or any future/unknown wire value. Read-only:
    /// `#[serde(other)]` maps every unrecognized kind here so `doctor` / `run
    /// list` can still surface such a run rather than faulting on it (ADR §D7).
    /// It is NEVER a creatable kind — it is absent from [`Kind::WIRE_NAMES`], so
    /// no CLI surface or report validator accepts it as input.
    #[serde(other)]
    Unknown,
}

impl Kind {
    /// The kebab-case wire name for this kind — the same string serde
    /// (de)serializes via `rename_all = "kebab-case"`.
    ///
    /// The exhaustive `match` is deliberate: adding a `Kind` variant fails
    /// to compile until its wire name is listed here, so [`Kind::WIRE_NAMES`]
    /// and any caller that advertises the accepted kinds (e.g. the report
    /// validator's `expected` hint) cannot silently drift from the enum.
    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Kind::Spinoff => "spinoff",
            Kind::Research => "research",
            Kind::TechnicalDecision => "technical-decision",
            Kind::FanOut => "fan-out",
            Kind::Unknown => "unknown",
        }
    }

    /// Every *creatable* kind's kebab-case wire name, in declaration order.
    /// Single source of truth for "the set of accepted kinds" — see
    /// [`Kind::wire_name`]. Excludes [`Kind::Unknown`], which is a read-only
    /// catch-all, never a valid input.
    pub const WIRE_NAMES: &'static [&'static str] = &[
        Kind::Spinoff.wire_name(),
        Kind::Research.wire_name(),
        Kind::TechnicalDecision.wire_name(),
        Kind::FanOut.wire_name(),
    ];

    /// Default how-run [`Lifecycle`] for a kind — the value a run gets when
    /// created WITHOUT `--interactive`. Every kind defaults to autonomous; the 0.2
    /// cut removed the `code` kind that used to imply interactivity, so
    /// interactivity is no longer kind-derived — it is the explicit `--interactive`
    /// flag ([`Lifecycle`] docs, design.md §2/§6). This method only seeds the
    /// default; it must NOT be read as "this kind is (non-)interactive".
    /// [`Kind::Unknown`] (a legacy on-disk run) reads as autonomous too; it is
    /// never freshly supervised, so the value only ever feeds read-only display.
    pub fn lifecycle(self) -> Lifecycle {
        match self {
            Kind::Spinoff
            | Kind::Research
            | Kind::TechnicalDecision
            | Kind::FanOut
            | Kind::Unknown => Lifecycle::Autonomous,
        }
    }

    /// Whether this kind is a **top-level, single-node, autonomous worker** —
    /// one detached agent that materializes its own worktree and self-merges,
    /// with no children and no parent DAG driving it. These are exactly the
    /// kinds eligible for the supervisor's bounded auto-retry on an empty-handed
    /// `agent-died` (issue `autoretry-agent-died-worker`).
    ///
    /// Excludes `FanOut` (a multi-unit driver — its driver node has no agent of
    /// its own) and [`Kind::Unknown`] (a legacy on-disk run, never freshly
    /// supervised).
    ///
    /// The exhaustive `match` fails to compile when a new `Kind` is added, forcing
    /// a deliberate eligibility decision rather than a silent default.
    #[must_use]
    pub fn is_autonomous_single_node_worker(self) -> bool {
        match self {
            Kind::Spinoff | Kind::Research | Kind::TechnicalDecision => true,
            Kind::FanOut | Kind::Unknown => false,
        }
    }
}

/// How a run is driven — its **how-run** state (design.md §2, §6).
///
/// This is an **explicit told fact**, set once at `run create` from the
/// `--interactive` flag and never transitioned. It is deliberately NOT derived
/// from [`Kind`]: the 0.2 cut removed the `code` kind that used to carry
/// interactivity accidentally, and interactivity is now orthogonal to topology —
/// *any* run can be marked interactive (`told, not guessed`, `target-state-0.2.md
/// §2`/§4). Do not reintroduce a `Kind`-derived inference; `Kind::lifecycle`
/// exists only to seed the default for a run created without the flag.
///
/// `Lifecycle` is a *category*, not a progress signal — an agent tracking
/// completion polls `manifest.status` (`Pending | Running | Done | Failed |
/// Cancelled`), NEVER `lifecycle`, whose value never changes (state-integrity
/// invariant 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Lifecycle {
    /// Agent runs to completion unattended; the supervisor adjudicates exit
    /// (the told `worker.exited` fact, then the residual crash backstop).
    Autonomous,
    /// Human-driven: the supervisor **never** auto-terminalizes or auto-tears-down
    /// from a dead pid or a worker exit — it waits for an explicit `run merge`
    /// (→ teardown) or `run cancel`. The human owns the whole lifecycle
    /// (design.md §6).
    Interactive,
}

impl Lifecycle {
    /// True for [`Lifecycle::Interactive`] — the human-driven, supervisor-hands-off
    /// how-run state. The single predicate the supervisor consults to suppress its
    /// automatic terminalization/teardown machinery (design.md §6).
    #[must_use]
    pub fn is_interactive(self) -> bool {
        matches!(self, Lifecycle::Interactive)
    }
}

/// Run/node status (design.md §1.2).
///
/// `Done`, `Failed`, and `Cancelled` are **terminal**: once a run or node
/// reaches one of them its `status` must never change again. The reducer
/// enforces this — `apply_run_status`, `apply_node_status`, and
/// `apply_node_report` are all no-ops once [`Status::is_terminal`] holds — so
/// a late-arriving event (e.g. an agent success report racing a `run cancel`)
/// cannot resurrect a settled state. Only the `status` field is frozen;
/// other projection fields may still be mutated by non-status events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// Created but not yet started.
    Pending,
    /// Actively executing.
    Running,
    /// Stalled awaiting input (e.g. an open discussion).
    Blocked,
    /// Completed successfully (terminal).
    Done,
    /// Completed with failure (terminal).
    Failed,
    /// Terminated before completion by an operator or parent (terminal).
    Cancelled,
}

impl Status {
    /// True for the terminal states `Done | Failed | Cancelled`. A run or
    /// node in a terminal state is settled: the reducer treats any further
    /// *status* transition as a no-op. "Settled" applies to `status` only —
    /// non-status projection fields (e.g. `Node::children` via
    /// `child.spawned`, or manifest counters) can still change.
    pub fn is_terminal(self) -> bool {
        matches!(self, Status::Done | Status::Failed | Status::Cancelled)
    }
}

/// Aggregate a set of node statuses into the run's rolled-up terminal status,
/// or `None` when the run is not yet complete.
///
/// The single, shared roll-up rule — used both by the supervisor's per-tick
/// `rollup_status` and by [`cancel_node`](crate::cancel_node)'s in-lock
/// self-roll-up (so the two can never diverge). A **three-way** classification
/// (design §2.5, "rollup terminalizes the run cancelled/done/failed once every
/// node is terminal"):
///
/// - `None` if the set is empty (a freshly-created run must not vacuously
///   complete) or if ANY node is still live (`Pending`/`Running`/`Blocked`);
/// - `Some(Status::Failed)` if any node genuinely `Failed` (a real failure
///   dominates the batch outcome);
/// - `Some(Status::Cancelled)` if no node failed but at least one was
///   `Cancelled` (a deliberate per-node/whole-run cancel — nothing failed, but
///   the batch did not fully complete; branch-preserving work is untouched);
/// - `Some(Status::Done)` when every node is `Done`.
pub fn aggregate_terminal_status<I>(statuses: I) -> Option<Status>
where
    I: IntoIterator<Item = Status>,
{
    let mut any = false;
    let mut any_failed = false;
    let mut any_cancelled = false;
    for s in statuses {
        any = true;
        match s {
            Status::Done => {}
            Status::Failed => any_failed = true,
            Status::Cancelled => any_cancelled = true,
            // Any live node means the run is not done yet.
            Status::Pending | Status::Running | Status::Blocked => return None,
        }
    }
    if !any {
        return None;
    }
    Some(if any_failed {
        Status::Failed
    } else if any_cancelled {
        Status::Cancelled
    } else {
        Status::Done
    })
}

/// Compact profile resolution recorded at create time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSelection {
    /// Compact selection schema version (currently 1).
    pub schema_version: u32,
    /// Requested and selected user profile name.
    pub profile: String,
    /// Precedence layer that supplied the request.
    pub selection_source: String,
    /// Explicit create-time interaction mode.
    pub interaction: String,
    /// Declared profile capability tier.
    pub capability: String,
    /// Declared profile residency class.
    pub residency: String,
    /// Legacy harness alias requested at the winning layer, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_harness: Option<String>,
    /// First statically eligible candidate.
    pub selected: SelectedAgentCandidate,
    /// Earlier candidates and their single deterministic skip reasons.
    #[serde(default)]
    pub fallback: Vec<SkippedAgentCandidate>,
}

/// Exact selected candidate pinned for launch and retry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectedAgentCandidate {
    /// Zero-based position in the profile's ordered candidate list.
    pub candidate_index: u8,
    /// Selected harness (`pi` or `claude`).
    pub harness: String,
    /// Exact user-owned argv; never a shell string.
    pub command: Vec<String>,
    /// Declared telemetry adapter protocol, when configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<String>,
}

impl SelectedAgentCandidate {
    /// Whether recorded policy configures the public worker telemetry v1
    /// adapter. This is configuration support, not runtime attestation or a
    /// claim that any sample has arrived.
    #[must_use]
    pub fn supports_worker_telemetry_v1(&self) -> bool {
        self.harness == "pi" && self.telemetry.as_deref() == Some("worker-v1")
    }
}

/// One rejected candidate and its first applicable reason.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkippedAgentCandidate {
    /// Zero-based position in the profile's ordered candidate list.
    pub candidate_index: u8,
    /// Candidate harness.
    pub harness: String,
    /// Stable skip reason code.
    pub reason: String,
}

impl AgentSelection {
    /// Validate semantic bounds and closed vocabularies at the durable event
    /// boundary, independently of the user-config parser that constructed it.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err(format!(
                "unsupported selection schema_version {}",
                self.schema_version
            ));
        }
        let name = self.profile.as_bytes();
        if name.is_empty()
            || name.len() > 63
            || !name[0].is_ascii_lowercase()
            || !name
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
            || self.profile.ends_with('-')
            || self.profile.contains("--")
        {
            return Err("invalid profile name".into());
        }
        if !matches!(
            self.selection_source.as_str(),
            "cli"
                | "environment"
                | "repository-per-kind"
                | "user-per-kind"
                | "repository-default"
                | "user-default"
                | "builtin-harness"
        ) {
            return Err("invalid selection_source".into());
        }
        if !matches!(
            self.interaction.as_str(),
            "autonomous" | "explicit-interactive"
        ) || !matches!(
            self.capability.as_str(),
            "fast" | "capable" | "ultra-capable"
        ) || !matches!(self.residency.as_str(), "local" | "remote")
        {
            return Err("invalid interaction, capability, or residency".into());
        }
        validate_selected_candidate(&self.selected)?;
        if self.interaction == "autonomous"
            && (self.selected.harness != "pi"
                || self.selected.telemetry.as_deref() != Some("worker-v1"))
        {
            return Err("autonomous selection requires pi with worker-v1 telemetry".into());
        }
        if self.selected.candidate_index >= 8
            || self.fallback.len() != usize::from(self.selected.candidate_index)
        {
            return Err("candidate index/count exceeds profile bound".into());
        }
        let mut prior = None;
        for (expected_index, skipped) in self.fallback.iter().enumerate() {
            if usize::from(skipped.candidate_index) != expected_index
                || prior.is_some_and(|value| skipped.candidate_index <= value)
                || !matches!(skipped.harness.as_str(), "pi" | "claude")
                || !matches!(
                    skipped.reason.as_str(),
                    "executable_missing"
                        | "autonomous_harness_unsupported"
                        | "telemetry_unsupported"
                )
            {
                return Err("invalid fallback candidate index, harness, or reason".into());
            }
            if skipped.reason == "autonomous_harness_unsupported"
                && (self.interaction != "autonomous" || skipped.harness == "pi")
            {
                return Err("inconsistent autonomous harness skip reason".into());
            }
            if skipped.reason == "telemetry_unsupported"
                && (self.interaction != "autonomous" || skipped.harness != "pi")
            {
                return Err("inconsistent telemetry skip reason".into());
            }
            prior = Some(skipped.candidate_index);
        }
        Ok(())
    }
}

fn validate_selected_candidate(candidate: &SelectedAgentCandidate) -> Result<(), String> {
    if !matches!(candidate.harness.as_str(), "pi" | "claude")
        || candidate.command.is_empty()
        || candidate.command.len() > 32
        || candidate
            .command
            .iter()
            .any(|arg| arg.is_empty() || arg.len() > 4096 || arg.contains('\0'))
        || candidate.command.iter().map(String::len).sum::<usize>() > 16_384
    {
        return Err("invalid selected harness or command".into());
    }
    match (candidate.harness.as_str(), candidate.telemetry.as_deref()) {
        (_, None) | ("pi", Some("worker-v1")) => Ok(()),
        _ => Err("invalid selected telemetry declaration".into()),
    }
}

/// `manifest.json` (design.md §1.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// State-schema version this file was written with.
    pub schema_version: u32,
    /// Watermark: the highest event `seq` whose projection fold is durably
    /// committed. Events in `events.jsonl` with `seq > applied_seq` are
    /// *unapplied tail* events — replayed into the projections on the next
    /// lock acquisition before any new append (see
    /// [`crate::events::append_and_apply_event`]). This is what makes
    /// append-then-apply atomic across a reducer crash: the event log can run
    /// ahead of the projections, but the gap is always healed before the next
    /// writer observes stale state.
    ///
    /// `#[serde(default)]` so a legacy `manifest.json` written before this
    /// field existed deserializes with `applied_seq = 0`. Such a manifest
    /// self-migrates on its next write: the catch-up replay re-folds the whole
    /// log — every event a no-op, because legacy state was already projected
    /// synchronously under the old append-then-apply path — and advances the
    /// watermark to `last_seq`. No separate migration pass or schema bump is
    /// required (the field is purely additive to a derived-cache file).
    #[serde(default)]
    pub applied_seq: u64,
    /// Unique run identifier (ULID). Validated on read.
    pub run_id: RunId,
    /// Kind of work this run performs.
    pub kind: Kind,
    /// How-run state (autonomous vs interactive), set once at `run create` from
    /// the explicit `--interactive` flag — never transitioned. See [`Lifecycle`].
    pub lifecycle: Lifecycle,
    /// Human-readable run title.
    pub title: String,
    /// Current aggregate run status.
    pub status: Status,
    /// When the run was created.
    pub created_at: DateTime<Utc>,
    /// When the manifest was last modified.
    pub updated_at: DateTime<Utc>,
    /// Source repository the run operates on, if any.
    pub source_repo: Option<String>,
    /// Branch the run was started from, if any.
    pub source_branch: Option<String>,
    /// Root directory under which this run's worktrees live, if any.
    pub worktree_root: Option<String>,
    /// tmux session taskfleet created to host this run's headless windows
    /// (`--headless` / `--tmux-session <name>`), if any. `None` for a foreground
    /// run whose window lives in the user's own session — that session is never
    /// a teardown target. When set, the supervisor kills this session once its
    /// last taskfleet-owned window is torn down and only the synthetic
    /// bootstrap shell window remains, so an empty headless session is not left
    /// behind (issue `headless-tmux-session-not-torn-down`). `#[serde(default)]`
    /// keeps a manifest written before this field existed readable.
    #[serde(default)]
    pub managed_tmux_session: Option<String>,
    /// Create-time terminal-window retention policy. `None` preserves the
    /// historical immediate-window/session cleanup behavior. The policy is
    /// recorded so supervisors and timer maintenance never reinterpret a run
    /// through later config drift.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_retention: Option<Box<TmuxRetentionPolicy>>,
    /// Completion-notification command registered at `run create --notify`,
    /// if any. When the run reaches a terminal state (`done | failed |
    /// cancelled`) the supervisor runs this command (at-least-once, deduped on a
    /// durable `run.notified` marker event — the healthy path fires once, a
    /// crash between firing and recording may re-fire) with `TASKFLEET_RUN_ID` /
    /// `TASKFLEET_STATUS` / `TASKFLEET_SUMMARY` (and `TASKFLEET_RUN_KIND` / `TASKFLEET_RUN_TITLE`)
    /// in its environment, BEFORE teardown removes the worktree/window. This is
    /// how a spawning session learns of completion without polling (issue
    /// `no-completion-notification-to-parent`). `None` for a run created without
    /// `--notify`; `#[serde(default)]` keeps a manifest written before this
    /// field existed readable.
    #[serde(default)]
    pub notify_cmd: Option<String>,
    /// The agent runtime selected for this run's worker
    /// (`claude` | `pi`), resolved at `run create`
    /// via the flag > env > config > default precedence and recorded here as
    /// provenance. This is the *selected* harness — recorded before the worker is
    /// spawned, so it reflects intent even if the spawn later fails. `None` for a
    /// manifest written before this field existed
    /// (`#[serde(default)]`) — such legacy runs predate harness selection and
    /// were all `claude`. Surfaced on `run show` / `run list --json`.
    #[serde(default)]
    pub harness: Option<String>,
    /// Compact create-time profile resolution. `None` keeps manifests written
    /// before profile selection readable without inventing requested/selected
    /// history. Retry and read paths consume this recorded value; they never
    /// re-resolve current configuration.
    #[serde(default)]
    pub agent_selection: Option<AgentSelection>,
    /// Number of nodes created in this run (denormalized counter).
    pub node_count: u32,
    /// Run that spawned this run, if it is itself a child.
    pub parent_run_id: Option<RunId>,
    /// Node in the parent run that spawned this run, if any.
    pub parent_node_id: Option<NodeId>,
}

/// `(child_run_id, child_node_id)` pointer recorded in `Node::children`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChildRef {
    /// Run id of the spawned child. Validated on read.
    pub run_id: RunId,
    /// Node id within the child run. Validated on read.
    pub node_id: NodeId,
}

/// State of durable native worker evidence capture.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    /// Capture has not completed yet.
    Pending,
    /// The latest capture attempt failed and cleanup remains vetoed.
    Failed,
    /// Every durable artifact was synced and recorded.
    Complete,
}

impl std::fmt::Display for EvidenceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Pending => "pending",
            Self::Failed => "failed",
            Self::Complete => "complete",
        })
    }
}

/// Durable native Pi session and terminal evidence for one worker attempt.
///
/// This is folded onto the node projection from append-only events. The live
/// source is state-root-relative; completed artifact paths are run-relative.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerEvidence {
    /// Worker attempt this evidence belongs to.
    pub attempt: u32,
    /// Exact Pi session identifier assigned before the candidate starts.
    pub session_id: String,
    /// Original cwd stored in the native transcript header.
    pub original_cwd: String,
    /// State-root-relative live native transcript path.
    pub live_session_path: String,
    /// Typed capture state.
    pub status: EvidenceStatus,
    /// Run-relative byte-identical archived transcript, once complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    /// Run-relative resume copy whose header names a surviving cwd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_path: Option<String>,
    /// Run-relative final tmux pane snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_path: Option<String>,
    /// Run-relative exact terminal report JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_path: Option<String>,
    /// SHA-256 of the archived original transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_sha256: Option<String>,
    /// Explicit capture failure detail. Never present on complete evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Opt-in policy recorded on a run placed in a persistent tmux session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TmuxRetentionPolicy {
    /// Keep the selected session after the last worker completes.
    pub persistent: bool,
    /// Maximum age of a completed inert display.
    pub completed_window_ttl_secs: u64,
    /// Maximum completed inert displays retained in this session.
    pub completed_window_max: u32,
}

/// Durable identity of one inert completed-worker display.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetainedDisplay {
    /// Worker attempt represented by this display.
    pub attempt: u32,
    /// Exact socket path observed at worker spawn.
    pub socket: String,
    /// Exact session name observed at worker spawn.
    pub session: String,
    /// Window created for the inert display.
    pub window_id: String,
    /// Its sole dead pane.
    pub pane_id: String,
    /// tmux server PID observed at worker spawn and retention creation.
    pub server_pid: u32,
    /// OS process start identity for `server_pid`, guarding PID/socket reuse.
    pub server_pid_start_secs: u64,
    /// Runtime-owned server marker observed at spawn (empty when unset).
    pub server_marker: String,
    /// Exact Taskfleet ownership marker stored as a tmux window option.
    pub ownership_marker: String,
    /// Time the terminal display became durable.
    pub retained_at: DateTime<Utc>,
    /// Policy-derived expiry time.
    pub expires_at: DateTime<Utc>,
    /// Set after maintenance removed the exact owned display. Durable archives
    /// and run events are intentionally untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expired_at: Option<DateTime<Utc>>,
}

/// `nodes/<node-id>.json` (design.md §1.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// State-schema version this file was written with.
    pub schema_version: u32,
    /// Unique node identifier within its run (e.g. `n-0001`). Validated on
    /// read; this is the projection's filename key, so it can never name a
    /// path outside `nodes/`.
    pub node_id: NodeId,
    /// Run this node belongs to. Validated on read.
    pub run_id: RunId,
    /// Parent node within the same run, if this is a sub-node.
    pub parent_node_id: Option<NodeId>,
    /// Kind of work this node performs.
    pub kind: Kind,
    /// Current node status.
    pub status: Status,
    /// Task description / prompt driving the node, if recorded.
    pub task: Option<String>,
    /// Filesystem path of the node's git worktree, if created.
    pub worktree_path: Option<String>,
    /// Git branch the node works on, if any.
    pub branch: Option<String>,
    /// The commit SHA the node's branch/worktree was forked from at spawn
    /// (the branch tip the moment `create.sh` materialized the worktree). It
    /// is the fixed reference point that lets the supervisor tell "this branch
    /// produced work that merged into source" from "this branch never diverged
    /// from its fork point": a branch still at `base_sha` is trivially an
    /// ancestor of its source branch but has merged nothing, so it must NOT be
    /// reconciled to success or torn down (that would drop a live agent's
    /// uncommitted work). Only a branch whose tip has moved past `base_sha`
    /// *and* is now an ancestor of the run's `source_branch` is a confirmed
    /// merge (issues `false-failed-after-merge` /
    /// `supervisor-stuck-pending-after-self-merge`). `#[serde(default)]` keeps a
    /// node written before this field existed readable (`None` → the
    /// git-reconcile fallback simply does not fire for it).
    #[serde(default)]
    pub base_sha: Option<String>,
    /// tmux window hosting the node's agent, if interactive. This is the
    /// human-readable window *name* — not unique across sessions and blind to
    /// non-default sockets. Kept for display and as the legacy liveness key;
    /// prefer [`Node::tmux_identity`] when present.
    pub tmux_window: Option<String>,
    /// Fully-qualified tmux identity (`session:window_id` + socket path)
    /// captured at spawn time. `None` for nodes registered before create.sh
    /// emitted the qualified fields — those fall back to bare-name matching on
    /// [`Node::tmux_window`]. New spawns always populate this when create.sh
    /// returns it.
    #[serde(default)]
    pub tmux_identity: Option<Box<TmuxIdentity>>,
    /// Native worker evidence, when this attempt uses Pi. Initialized before
    /// publication and advanced only by locked evidence events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<WorkerEvidence>,
    /// Inert completed-window identity, when this run opted into retention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_display: Option<Box<RetainedDisplay>>,
    /// Permanent reason why a configured terminal display could not exist
    /// (for example the recorded tmux server generation was lost).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_unavailable: Option<String>,
    /// PID of the running agent process, if live.
    pub agent_pid: Option<i32>,
    /// Start time of the agent process, used to detect PID reuse.
    pub agent_pid_start_time: Option<DateTime<Utc>>,
    /// PID of the supervisor watching this node, if live.
    pub supervisor_pid: Option<i32>,
    /// Children this node has spawned.
    #[serde(default)]
    pub children: Vec<ChildRef>,
    /// When the node started executing, if it has.
    pub started_at: Option<DateTime<Utc>>,
    /// When the node file was last modified.
    pub updated_at: DateTime<Utc>,
    /// The `node.report` payload that drove this node to its terminal status.
    /// Set only by the report that actually transitions the node (Done /
    /// Failed / Cancelled). Once the node is terminal it is frozen: a late
    /// report against an already-settled node is dropped without overwriting
    /// this field (see `reducer::apply_node_report`). So for a node cancelled
    /// by `run cancel`, this holds the synthesized cancel report, not a
    /// later-arriving agent report — that payload remains only in
    /// `events.jsonl`.
    pub last_report: Option<Value>,
    /// Highest report `seq` consumed per child run id, for idempotent
    /// report processing across supervisor restarts.
    #[serde(default)]
    pub last_processed_report_seq_by_child: Map<String, Value>,
    /// Number of times the supervisor has auto-retried this node after an
    /// empty-handed `agent-died` (issue `autoretry-agent-died-worker`). The
    /// DURABLE, restart-safe bound on the bounded-retry loop: each `node.retry`
    /// event increments it, and the watchdog terminalizes the run `failed` once
    /// it reaches `RETRY_MAX_ATTEMPTS`. `#[serde(default)]` keeps a node written
    /// before this field existed readable (`0` — never retried).
    #[serde(default)]
    pub retry_attempts: u32,
    /// The **told** exit status of the node's worker process, recorded durably by
    /// the `run-worker` launcher shim (`crates/taskfleet/src/run_worker.rs`) when
    /// it `wait()`s on the agent it wrapped. This is a *fact*, not an inference:
    /// the supervisor consumes it via the typed outcome table instead of guessing
    /// completion from pid/pane/activity proxies (design.md §2.1, issue
    /// `thin-exit-status-launcher`). A non-zero code or a terminating signal is a
    /// `failed` worker; `code == 0` with no `explicit-merge` transition is the
    /// *finished-but-unmerged* case that must stay non-terminal (attention-
    /// required), NOT be auto-failed. `None` until the shim records an exit — or
    /// forever, for a worker never launched through the shim (the crash backstop
    /// still covers that path). `#[serde(default)]` keeps a node written before
    /// this field existed readable.
    #[serde(default)]
    pub worker_exit: Option<WorkerExit>,
    /// The in-flight `run merge` transaction for this node, if one has been
    /// STARTED but not yet completed. `run merge` records a `merge.started`
    /// event (setting this field) BEFORE it mutates git, because the merge spans
    /// two durability domains — git refs and the event log — and is not atomic
    /// across them (design.md §2.1b / A2, issue `merge-transaction-recovery`). A
    /// crash after the git merge but before the terminal `explicit-merge`
    /// `node.report` would otherwise strand the work *merged in source* with *no
    /// merge event* → a false `failed`.
    ///
    /// This field is the durable op-log record that lets recovery finish or
    /// reject that ONE known transaction deterministically, by OID — never a
    /// general branch-content heuristic. It is set by [`crate::MergeTxn`]-carrying
    /// `merge.started`, and cleared when the transaction resolves: a terminal
    /// `node.report` (the merge completed) or a `merge.aborted` (recovery found
    /// the git mutation never landed). `#[serde(default)]` keeps a node written
    /// before this field existed readable (`None` — no in-flight merge).
    ///
    /// Boxed so the (rare) in-flight transaction does not inflate every `Node` /
    /// `ProjectionOp` by the full [`MergeTxn`] footprint.
    #[serde(default)]
    pub pending_merge: Option<Box<MergeTxn>>,
    /// The durable, monotonic timestamp of the FIRST tick on which the supervisor
    /// observed this node's worker process confirmed-dead with no told
    /// `worker.exited` and no merge — the anchor for the residual crash
    /// backstop's fixed post-death grace (design.md §2.1a, issue
    /// `typed-supervisor-outcomes`).
    ///
    /// The backstop is the ONLY place pid liveness still governs an outcome
    /// (pid liveness is a pure crash backstop now, never a primary signal). When
    /// the launcher shim's exit fact is lost — a hard kill of the shim, host
    /// death — the supervisor never sees a `worker.exited` event, so it falls
    /// back to "process confirmed gone → `failed`". The grace exists only to let
    /// an in-flight `worker.exited` / merge append land before that fires: on the
    /// first confirmed death the supervisor records this timestamp (via a
    /// `node.death_observed` event) and DEFERS; it terminalizes `failed` only on a
    /// later tick once a fixed short window has elapsed AND an exclusive-lock
    /// re-read confirms no exit/merge landed in the race window.
    ///
    /// Persisted (not in-memory) so the grace survives a supervisor restart in the
    /// window — a restart re-reads it rather than restarting the clock. First-write
    /// -wins in the reducer, so the anchor is monotonic. `None` until the first
    /// confirmed-death observation, or forever for a worker that exits cleanly
    /// (the shim records `worker.exited` and the backstop never engages).
    /// `#[serde(default)]` keeps a node written before this field existed readable.
    #[serde(default)]
    pub first_death_at: Option<DateTime<Utc>>,
    /// An open, agent-authored request for a human decision. The worker records
    /// this through `node.awaiting_input` instead of blocking on interactive
    /// stdin. It remains non-terminal and is cleared by `node.input_resolved`, a
    /// terminal `node.report`, or `node.retry`.
    ///
    /// `opened_at` is stamped from the event envelope and is therefore a durable,
    /// monotonic grace-window anchor that survives supervisor restarts. The
    /// original discussion objects are retained verbatim so read surfaces and
    /// notification hooks can carry the question, options, and recommended
    /// default without inventing a second advisory schema.
    #[serde(default)]
    pub awaiting_input: Option<Box<AwaitingInput>>,
}

/// Durable open-discussion state projected from `node.awaiting_input`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AwaitingInput {
    /// Timestamp of the first open signal in the current unresolved generation.
    pub opened_at: DateTime<Utc>,
    /// Event sequence that opened this generation, used to deduplicate its
    /// delayed parent notification independently from later generations.
    pub event_seq: u64,
    /// Validated report-shaped discussion objects. Each carries `topic`,
    /// `options`, and `recommended_default`.
    pub discussion_items: Vec<Value>,
}

/// A durable, in-flight `run merge` transaction recorded by `merge.started`
/// BEFORE the git mutation, and the sole input to deterministic merge-crash
/// recovery (design.md §2.1b / A2, issue `merge-transaction-recovery`).
///
/// `run merge` spans git refs and the event log and is not atomic across them.
/// Recording the transaction — the exact source ref it will move, the OID it
/// expects that ref to be at (`expected_source_oid`, the compare half of the
/// compare-and-swap), and the worker's tip — lets the supervisor (or a retried
/// `run merge`) resolve the ONE recorded transaction by OID after a crash:
///
/// - source ref still at `expected_source_oid` → the mutation never landed →
///   REJECT (`merge.aborted`), preserving the worker's branch + work.
/// - source ref moved off `expected_source_oid` AND the worker's content is
///   integrated (rebase-robust content verification) → COMPLETE (append the
///   `explicit-merge` `node.report` the crash prevented).
/// - source ref moved unexpectedly but the worker's content is not integrated →
///   fail closed (REJECT), preserving the work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeTxn {
    /// Opaque unique id for this merge attempt. A fresh id per `run merge`
    /// invocation (each attempt re-reads `expected_source_oid`), so recovery can
    /// name exactly which transaction it resolved in the `merge.aborted` audit.
    pub op_id: String,
    /// The source/target ref this merge moves — `manifest.source_branch`
    /// (`main`, or an integration branch). Recovery reads this ref's current OID
    /// to decide the transaction's fate.
    pub source_branch: String,
    /// The worker branch whose commits are being merged (`node.branch`). Its
    /// content is what recovery verifies is integrated into `source_branch`.
    pub worker_branch: String,
    /// The OID `source_branch` was at when the transaction was recorded — the
    /// compare half of the compare-and-swap. If the ref is still here at recovery
    /// time, the git mutation never landed.
    pub expected_source_oid: String,
    /// The worker branch tip at record time. Retained for the audit trail and as
    /// a secondary landing signal; the authoritative completion check is
    /// content-based (rebase-robust) against `source_branch`.
    pub worker_oid: String,
    /// The worker branch's fork point (`node.base_sha`), used to bound the
    /// content check to the worker's own commits. `None` when unrecorded.
    #[serde(default)]
    pub base_sha: Option<String>,
    /// PID of the `run merge` process driving the transaction, so recovery can
    /// tell a still-in-progress merge (driver alive — leave it) from a crashed
    /// one (driver gone — resolve it), never racing a live merge. `None` when
    /// unrecorded.
    #[serde(default)]
    pub driver_pid: Option<i32>,
    /// Start time of `driver_pid` in Unix seconds (the same representation the
    /// pid-file liveness check records), guarding against PID reuse the way the
    /// agent/supervisor liveness checks do — a recycled PID must not look alive.
    /// `None` when the platform could not read it.
    #[serde(default)]
    pub driver_pid_start_secs: Option<u64>,
    /// When the transaction was recorded.
    pub started_at: DateTime<Utc>,
}

/// The observed exit status of a node's worker process, recorded by the
/// `run-worker` launcher shim under the run lock (design.md §2.1 / A1).
///
/// Exactly one of `code` / `signal` is meaningful: a worker that returned
/// normally carries `code = Some(n)` (and `signal = None`); a worker killed by a
/// signal carries `signal = Some(s)` (and, on Unix, `code = None`). A recorded
/// exit is a durable *told fact* — the supervisor reads it rather than inferring
/// completion from liveness proxies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerExit {
    /// Normal-exit status code, if the worker was not killed by a signal.
    #[serde(default)]
    pub code: Option<i32>,
    /// Terminating signal number, if the worker was killed by a signal.
    #[serde(default)]
    pub signal: Option<i32>,
    /// When the shim observed the worker's exit.
    pub at: DateTime<Utc>,
}

impl WorkerExit {
    /// A clean exit: not signalled, and a zero return code. This is the *only*
    /// success-shaped worker exit — but a clean exit alone is NOT a completed
    /// unit (the worker may have finished-but-skipped `run merge`); merge is the
    /// only success truth (design.md §2.6). Callers pair this with a merge check.
    pub fn is_clean(self) -> bool {
        self.signal.is_none() && self.code == Some(0)
    }

    /// A failed worker: killed by a signal, or a non-zero return code. Mutually
    /// exclusive with [`WorkerExit::is_clean`].
    pub fn is_failure(self) -> bool {
        !self.is_clean()
    }
}

/// A fully-qualified tmux window identity recorded at spawn time.
///
/// `tmux_window` (the human name) is not unique across sessions, and a bare
/// `tmux list-windows -a` cannot see windows on a non-default socket. This
/// triple pins the exact window the agent runs in — `session:window_id` is
/// unique per server, `window_id` (the `@NNNN` form) survives renames, and
/// `socket` disambiguates multiple tmux servers. The watchdog matches on this
/// when present (design.md §8.1).
///
/// `pane_id` (the `%NN` form) pins the agent's *specific* pane within that
/// window, recorded at spawn. Window-owning operations (`kill-window` teardown —
/// the supervisor owns the whole window per the cleanup invariants) key off
/// `window_id`; only per-pane operations that must not follow the window's
/// *active* pane — chiefly `pipe-pane` agent-log capture — use `pane_id`. It is
/// `None` for a run spawned before create.sh emitted the field; capture then
/// falls back to `window_id` (issue `capture-agent-pane-by-pane-id`).
///
/// The watchdog's liveness probe still keys off `window_id` (correct for the
/// single-pane autonomous path). A pane-aware liveness probe — needed so a split
/// interactive window whose agent pane dies while a user shell pane survives is
/// still seen as dead — is a follow-up (`watchdog-pane-aware-liveness`), not this
/// change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxIdentity {
    /// Server socket path (`#{socket_path}`). `None` if create.sh could not
    /// read it; the watchdog then queries tmux on its default socket.
    #[serde(default)]
    pub socket: Option<String>,
    /// Session that owns the window (`#{session_name}`).
    pub session: String,
    /// Stable window id in `@NNNN` form (`#{window_id}`). Survives renames and
    /// is unique within the server.
    pub window_id: String,
    /// Stable pane id in `%NN` form (`#{pane_id}`), recorded at spawn — the
    /// agent's own pane. `None` for a run whose create.sh predates the field
    /// (back-compat: old state deserializes with `pane_id: None`). Prefer
    /// [`TmuxIdentity::capture_target`] over reading this directly.
    #[serde(default)]
    pub pane_id: Option<String>,
    /// tmux server PID and its OS start identity at spawn. New persistent
    /// sessions require both; legacy nodes deserialize them as absent.
    #[serde(default)]
    pub server_pid: Option<u32>,
    /// Unix-second process start identity for the recorded tmux server PID.
    #[serde(default)]
    pub server_pid_start_secs: Option<u64>,
    /// Runtime-owned server marker (for example Homebase's owner marker).
    #[serde(default)]
    pub server_marker: Option<String>,
}

impl TmuxIdentity {
    /// The tmux target for a per-pane operation that must hit the agent's own
    /// pane, not the window's *active* pane: the recorded `pane_id` when
    /// present, else the `window_id` (which resolves to the active pane).
    ///
    /// Used by agent-log capture (`pipe-pane`). Window-level operations
    /// (`kill-window`, liveness) must NOT use this — they key off `window_id`
    /// directly so they act on the whole window.
    ///
    /// A recorded `pane_id` is preferred only when non-empty; an empty string
    /// (a directly-deserialized/corrupt state that the reducer/spawn normalizers
    /// never produce) is treated as absent so capture never targets `-t ""`.
    pub fn capture_target(&self) -> &str {
        self.pane_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .unwrap_or(&self.window_id)
    }
}

/// One event-log line (design.md §1.4).
///
/// `run_id` / `node_id` are the typed id newtypes, so deserializing an
/// `events.jsonl` line validates the whole envelope on read: a malformed
/// `run_id` or `node_id` fails the `serde` parse at the read boundary (the
/// id newtypes' validating `Deserialize`) rather than being carried as an
/// unvalidated `String` until some later path helper. The parse failure
/// surfaces as whatever error the reader maps a bad line to — e.g. a
/// newline-terminated bad line is [`Error::CorruptEventLog`] from both
/// [`read_all_events`] and [`find_prior_with_key`], which share one physical
/// reader and torn-tail policy. The reducer still performs its own per-event
/// checks (envelope `run_id` matches the run it is folded into; `data`-borne
/// ids parse), but the envelope ids can no longer be the unvalidated party.
///
/// [`read_all_events`]: crate::events::read_all_events
/// [`find_prior_with_key`]: crate::events
/// [`Error::CorruptEventLog`]: crate::Error::CorruptEventLog
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Wall-clock timestamp the event was appended.
    pub ts: DateTime<Utc>,
    /// Monotonic per-run sequence number (recovered on append).
    pub seq: u64,
    /// Event kind discriminator (e.g. `node.created`, `discussion.opened`).
    pub kind: String,
    /// Run the event belongs to. Validated on read.
    pub run_id: RunId,
    /// Node the event concerns, when applicable. Validated on read.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub node_id: Option<NodeId>,
    /// Caller-supplied key used to dedupe retried appends.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub idempotency_key: Option<String>,
    /// Kind-specific payload applied by the reducer.
    #[serde(default)]
    pub data: Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_terminal_status_is_the_three_way_rule() {
        use Status::{Blocked, Cancelled, Done, Failed, Pending, Running};
        // Empty set → not complete.
        assert_eq!(aggregate_terminal_status([]), None);
        // Any live node → not complete.
        for live in [Pending, Running, Blocked] {
            assert_eq!(aggregate_terminal_status([Done, live]), None);
        }
        // All done → Done.
        assert_eq!(aggregate_terminal_status([Done, Done]), Some(Done));
        // Any failure dominates.
        assert_eq!(aggregate_terminal_status([Done, Failed]), Some(Failed));
        assert_eq!(aggregate_terminal_status([Failed, Cancelled]), Some(Failed));
        // Cancelled (no failure) — pure or mixed with done.
        assert_eq!(
            aggregate_terminal_status([Cancelled, Cancelled]),
            Some(Cancelled)
        );
        assert_eq!(
            aggregate_terminal_status([Done, Cancelled]),
            Some(Cancelled)
        );
    }

    /// `Kind::wire_name` (and thus `Kind::WIRE_NAMES`) must stay identical
    /// to what serde actually (de)serializes. If the `rename_all` routing
    /// or a variant name ever diverges from `wire_name`, this fails — which
    /// is what keeps the report validator's `expected` hint honest.
    #[test]
    fn wire_names_match_serde_round_trip() {
        for &name in Kind::WIRE_NAMES {
            let kind: Kind = serde_json::from_value(Value::String(name.to_string()))
                .unwrap_or_else(|_| panic!("WIRE_NAMES entry {name:?} is not a valid Kind"));
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                Value::String(name.to_string()),
                "serde round-trip diverged from wire_name for {name:?}",
            );
        }
    }

    /// The bounded auto-retry eligibility gate (issue `autoretry-agent-died-worker`)
    /// must include exactly the autonomous single-node worker kinds and exclude
    /// the fan-out driver (and the read-only `Unknown` catch-all).
    #[test]
    fn autonomous_single_node_worker_set_is_exact() {
        for k in [Kind::Spinoff, Kind::Research, Kind::TechnicalDecision] {
            assert!(
                k.is_autonomous_single_node_worker(),
                "{k:?} should be retry-eligible"
            );
            assert_eq!(k.lifecycle(), Lifecycle::Autonomous);
        }
        for k in [
            Kind::FanOut,  // multi-unit driver
            Kind::Unknown, // legacy on-disk run — never freshly supervised
        ] {
            assert!(
                !k.is_autonomous_single_node_worker(),
                "{k:?} must NOT be retry-eligible"
            );
        }
    }

    /// A legacy run recorded under a since-removed kind must still deserialize
    /// to the read-only [`Kind::Unknown`] catch-all rather than faulting the
    /// read — the ADR §D7 "report, never delete" contract for the on-disk
    /// evidence corpus. Every creatable kind still round-trips to itself.
    #[test]
    fn removed_kinds_deserialize_to_unknown() {
        for removed in [
            "code",
            "orchestrate",
            "orchestrated",
            "bugfix",
            "make-skill",
        ] {
            let kind: Kind = serde_json::from_value(Value::String(removed.to_string()))
                .expect("a removed kind must still deserialize, not fault");
            assert_eq!(kind, Kind::Unknown, "{removed:?} should map to Unknown");
        }
        // A wholly unknown value maps there too (forward-compat).
        assert_eq!(
            serde_json::from_value::<Kind>(Value::String("future-kind".into())).unwrap(),
            Kind::Unknown
        );
        // The surviving kinds are unaffected.
        for &name in Kind::WIRE_NAMES {
            let kind: Kind = serde_json::from_value(Value::String(name.to_string())).unwrap();
            assert_ne!(kind, Kind::Unknown, "{name:?} must not fold to Unknown");
        }
    }

    /// Back-compat acceptance criterion (issue `capture-agent-pane-by-pane-id`):
    /// a `TmuxIdentity` persisted before `pane_id` existed — with the field
    /// entirely absent, or written as an explicit `null` — must still
    /// deserialize, yielding `pane_id: None` and a `window_id` capture target.
    #[test]
    fn tmux_identity_deserializes_legacy_state_without_pane_id() {
        // Field entirely absent (a state file written by an older binary).
        let absent: TmuxIdentity = serde_json::from_value(serde_json::json!({
            "socket": null,
            "session": "taskfleet",
            "window_id": "@42",
        }))
        .expect("legacy identity without pane_id must deserialize");
        assert_eq!(absent.pane_id, None);
        assert_eq!(absent.capture_target(), "@42");

        // Field present but explicitly null.
        let null: TmuxIdentity = serde_json::from_value(serde_json::json!({
            "socket": null,
            "session": "taskfleet",
            "window_id": "@42",
            "pane_id": null,
        }))
        .expect("identity with explicit null pane_id must deserialize");
        assert_eq!(null.pane_id, None);
        assert_eq!(null.capture_target(), "@42");
    }

    /// `capture_target` prefers a recorded `pane_id` (`%NN`) over the window id,
    /// but treats an empty `pane_id` as absent (never targets `-t ""`).
    #[test]
    fn capture_target_prefers_nonempty_pane_id() {
        let with_pane = TmuxIdentity {
            socket: None,
            session: "taskfleet".into(),
            window_id: "@42".into(),
            pane_id: Some("%7".into()),
            server_pid: None,
            server_pid_start_secs: None,
            server_marker: None,
        };
        assert_eq!(with_pane.capture_target(), "%7");

        let empty_pane = TmuxIdentity {
            pane_id: Some(String::new()),
            ..with_pane.clone()
        };
        assert_eq!(empty_pane.capture_target(), "@42");
    }
}

#[cfg(test)]
mod id_tests {
    use super::*;

    /// Inputs every id type must reject — the path-traversal vectors plus the
    /// generic malformed cases called out in the issue's success criteria.
    const TRAVERSAL_VECTORS: &[&str] = &[
        "..",
        "../etc",
        "a/b",
        "a/../b",
        ".hidden",
        "./x",
        "foo/bar.json",
        "n-0001/../../etc",
        "",
    ];

    #[test]
    fn run_id_accepts_generator_output_and_rejects_malformed() {
        let id = crate::new_run_id();
        assert!(
            RunId::parse_str(&id).is_ok(),
            "generator must validate: {id}"
        );
        for bad in [
            "tooshort",
            "01jxsnap0000000000000000000", // 27 chars
            "01JXSNAP000000000000000000",  // uppercase
            "01jxiiiiiiiiiiiiiiiiiiiiii",  // `i` not in Crockford
            "80000000000000000000000000",  // first char exceeds ULID range
            "n-0001",                      // wrong shape entirely
        ] {
            assert!(RunId::parse_str(bad).is_err(), "expected reject: {bad:?}");
        }
        for bad in TRAVERSAL_VECTORS {
            assert!(
                RunId::parse_str(bad).is_err(),
                "traversal not rejected: {bad:?}"
            );
        }
    }

    #[test]
    fn node_id_accepts_canonical_and_rejects_malformed() {
        for ok in ["n-0001", "n-0010", "n-123456"] {
            assert!(NodeId::parse_str(ok).is_ok(), "expected accept: {ok}");
        }
        // Wrong prefix is its own error variant.
        assert!(matches!(
            NodeId::parse_str("d-0001"),
            Err(IdValidationError::WrongPrefix { .. })
        ));
        assert!(matches!(
            NodeId::parse_str("0001"),
            Err(IdValidationError::WrongPrefix { .. })
        ));
        for bad in [
            "n-1",           // too few digits
            "n-abcd",        // non-digit body
            "n-",            // empty body
            "n-00a1",        // mixed
            "n-00000000000", // 11 digits — over the 10-digit ceiling
        ] {
            assert!(
                matches!(
                    NodeId::parse_str(bad),
                    Err(IdValidationError::InvalidFormat { .. })
                ),
                "expected InvalidFormat: {bad:?}",
            );
        }
        for bad in TRAVERSAL_VECTORS {
            assert!(
                NodeId::parse_str(bad).is_err(),
                "traversal not rejected: {bad:?}"
            );
        }
    }

    #[test]
    fn deserialize_rejects_malformed_ids() {
        // The validating Deserialize impl is the on-read guard: a tampered
        // projection file whose key no longer validates must fail to parse.
        assert!(serde_json::from_str::<NodeId>("\"n-0001\"").is_ok());
        assert!(serde_json::from_str::<NodeId>("\"../../etc\"").is_err());
        assert!(serde_json::from_str::<NodeId>("\"n-../escape\"").is_err());
    }

    #[test]
    fn serialize_round_trips_as_bare_string() {
        let nid = NodeId::parse_str("n-0042").unwrap();
        let json = serde_json::to_string(&nid).unwrap();
        assert_eq!(json, "\"n-0042\"");
        let back: NodeId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, nid);
        assert_eq!(nid.as_str(), "n-0042");
        assert_eq!(nid.to_string(), "n-0042");
    }

    #[test]
    fn error_exposes_kind_and_expected() {
        let err = NodeId::parse_str("n-x").unwrap_err();
        assert_eq!(err.kind(), "node");
        assert_eq!(err.expected(), "n-NNNN (n- followed by 4-10 ASCII digits)");
    }

    #[test]
    fn event_deserialize_validates_envelope_ids() {
        // The whole `events.jsonl` envelope is now validated on read: the
        // typed `run_id` / `node_id` fields parse through the id newtypes, so
        // a malformed envelope id fails the deserialize rather than being
        // carried downstream as an unchecked string.
        let ok = r#"{"ts":"2026-06-12T00:00:00Z","seq":1,"kind":"node.created","run_id":"01jxsnap000000000000000000","node_id":"n-0001","data":{}}"#;
        assert!(serde_json::from_str::<Event>(ok).is_ok());

        // Invalid `run_id` (not a 26-char ULID) fails the parse.
        let bad_run = r#"{"ts":"2026-06-12T00:00:00Z","seq":1,"kind":"run.status","run_id":"not-a-ulid","data":{}}"#;
        assert!(serde_json::from_str::<Event>(bad_run).is_err());

        // Invalid top-level `node_id` (too few digits) also fails the parse.
        let bad_node = r#"{"ts":"2026-06-12T00:00:00Z","seq":1,"kind":"node.status","run_id":"01jxsnap000000000000000000","node_id":"n-1","data":{}}"#;
        assert!(serde_json::from_str::<Event>(bad_node).is_err());
    }

    #[test]
    fn from_str_and_ord_delegate_to_inner() {
        use std::str::FromStr;
        // `FromStr` mirrors `parse_str`, so the `str::parse` ecosystem works.
        assert!(RunId::from_str("01jxsnap000000000000000000").is_ok());
        assert!("n-0001".parse::<NodeId>().is_ok());
        assert!("n-x".parse::<NodeId>().is_err());

        // `Ord` is lexicographic over the inner string; for ULIDs that is the
        // natural time-encoded order.
        let a = RunId::parse_str("01jxsnap000000000000000000").unwrap();
        let b = RunId::parse_str("02jxsnap000000000000000000").unwrap();
        assert!(a < b);
        let mut v = vec![b.clone(), a.clone()];
        v.sort();
        assert_eq!(v, vec![a, b]);
    }
}

#[cfg(test)]
mod agent_selection_validation_tests {
    use super::*;

    fn valid() -> AgentSelection {
        AgentSelection {
            schema_version: 1,
            profile: "capable".into(),
            selection_source: "cli".into(),
            interaction: "autonomous".into(),
            capability: "capable".into(),
            residency: "remote".into(),
            requested_harness: None,
            selected: SelectedAgentCandidate {
                candidate_index: 1,
                harness: "pi".into(),
                command: vec!["pi".into()],
                telemetry: Some("worker-v1".into()),
            },
            fallback: vec![SkippedAgentCandidate {
                candidate_index: 0,
                harness: "claude".into(),
                reason: "autonomous_harness_unsupported".into(),
            }],
        }
    }

    #[test]
    fn durable_selection_rejects_impossible_state() {
        assert!(valid().validate().is_ok());
        let mut invalid = valid();
        invalid.selected.candidate_index = 200;
        assert!(invalid.validate().is_err());
        let mut invalid = valid();
        invalid.fallback[0].reason = "made_up".into();
        assert!(invalid.validate().is_err());
        let mut invalid = valid();
        invalid.selected.harness = "claude".into();
        assert!(invalid.validate().is_err());
    }
}
