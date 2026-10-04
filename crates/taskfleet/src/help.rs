//! Structured (`--output json|jsonl`) rendering of the `--help` surface.
//!
//! clap's derive ships only a *text* `--help`. AGENTS-AI-FIRST-CLI §14
//! additionally requires every `<tool> ... --help` to emit a
//! machine-readable description of its command surface — subcommands,
//! flags, positionals, defaults, env-var mappings, accepted-value enums,
//! and deprecation status — carrying its own `schema_version`.
//!
//! This module walks the built [`clap::Command`] tree (clap exposes the
//! full metadata via `get_*` / `is_*_set` getters) and projects it onto
//! the serde structs below. The caller in [`crate::cli`] feeds the result
//! through the standard [`crate::output::emit_envelope`] so the help
//! payload rides the same `{schema_version, data}` envelope, format
//! selection (`json` pretty vs `jsonl` single-line), and file routing as
//! every other command.
//!
//! Stability contract (snapshot-tested): flags are sorted by `name` (the
//! clap arg id), positionals by index, subcommands by name; the metadata
//! lists (`long_aliases`, `short_aliases`, `conflicts_with`, `requires`,
//! `required_unless_present`) are sorted. Field renames/removals are breaking
//! changes — bump `SCHEMA_VERSION_HELP`.

use clap::builder::ValueHint;
use clap::parser::ValueSource;
use clap::{Arg, ArgAction, Command};
use serde::Serialize;

use crate::output::{OutputFormat, OutputSpec};

/// Schema version of the help payload itself (independent of the
/// envelope's `schema_version` and of the state-schema version). Bump on
/// any breaking change to the shapes in this module.
///
/// - v1: initial structured-help projection.
/// - v2: `name`/optional `long`, alias lists, `is_global`, `conflicts_with`,
///   `requires`, `required_unless_present`, `arity`, `help_heading`,
///   `accepts_file_paths`, custom-parser `accepted_values`, positional
///   `env`/`defaults`, and the `deprecated`/`deprecation_note` convention on
///   every node. (`requires` / `required_unless_present` were added after the
///   initial v2 cut; they are strictly additive — default `[]` — so they ride
///   v2 rather than forcing a bump, per the convention above.)
/// - v3: depth-bounded rendering. The default response no longer recurses the
///   whole command tree — only the requested node is rendered fully, and its
///   immediate children appear as lightweight [`SubcommandSummary`]s. Pass
///   `--depth N` (N≥1) to expand N more levels fully or `--depth tree` to
///   restore the v2 unbounded shape. A subcommand inside `subcommands` is
///   therefore either a full [`CommandNode`] (when the depth budget allowed
///   recursion) or a [`SubcommandSummary`] (when it would otherwise be the
///   first truncated level); agents distinguish the two by checking for the
///   `flags` field, which only the full variant carries.
pub const SCHEMA_VERSION_HELP: u32 = 3;

/// The `data` body of a `--help --output json|jsonl` response.
#[derive(Debug, Serialize)]
pub struct HelpData {
    /// Version of the help payload schema (this module's shapes).
    pub schema_version_help: u32,
    /// The command surface, starting at the resolved command.
    #[serde(flatten)]
    pub command: CommandNode,
}

/// One node in the command tree — recursive: each subcommand is rendered
/// as a full `CommandNode` so the whole surface is queryable in one call.
#[derive(Debug, Serialize)]
pub struct CommandNode {
    /// Full invocation path, e.g. `taskfleet run create`.
    pub command: String,
    /// One-line description (`about`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    /// Long description (`long_about`), when distinct from `about`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_about: Option<String>,
    /// Visible aliases for this command.
    pub aliases: Vec<String>,
    /// Whether the command is hidden from the human text help. Hidden
    /// commands are still listed (agents may invoke them) but flagged.
    pub hidden: bool,
    /// Whether the command is deprecated (per the `[deprecated]` help-text
    /// convention — see [`parse_deprecation`]).
    pub deprecated: bool,
    /// Optional deprecation note from a `[deprecated: <note>]` prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecation_note: Option<String>,
    /// Command version, when one is set (typically only the root).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Named flags, sorted by `name` (the clap arg id).
    pub flags: Vec<FlagInfo>,
    /// Positional arguments, sorted by position.
    pub positionals: Vec<PositionalInfo>,
    /// Child subcommands, sorted by name. Each entry is either a full
    /// [`CommandNode`] (when the depth budget allowed recursion past this
    /// level) or a [`SubcommandSummary`] (when it is the first truncated
    /// level). The two are distinguishable by the presence of the `flags`
    /// field — only the full variant carries it.
    pub subcommands: Vec<SubcommandEntry>,
}

/// One entry under [`CommandNode::subcommands`]: either a full child node
/// (when the depth budget recursed past it) or a lightweight summary (when
/// it is at the depth cutoff).
///
/// Serialized untagged so the JSON shape is the natural union of
/// [`CommandNode`]'s and [`SubcommandSummary`]'s fields — agents read it as a
/// plain object and branch on field presence (`flags` exists only on Full).
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum SubcommandEntry {
    /// Full node — the depth budget had room to recurse past this level.
    Full(CommandNode),
    /// Summary — this child is at the truncation boundary. Drill in with
    /// `<command> --help --output json` (or pass `--depth N+1` /
    /// `--depth tree` to expand more here).
    Summary(SubcommandSummary),
}

/// Lightweight description emitted in place of a full [`CommandNode`] when
/// the depth budget would have truncated past it. Carries only the fields a
/// caller needs to *decide* whether to drill into this command for full
/// detail.
#[derive(Debug, Serialize)]
pub struct SubcommandSummary {
    /// Full invocation path (same shape as [`CommandNode::command`]).
    pub command: String,
    /// One-line description (`about`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    /// Visible aliases for this command.
    pub aliases: Vec<String>,
    /// Hidden from human help (still listed so agents can find it).
    pub hidden: bool,
    /// Marked deprecated (per the `[deprecated]` help-text convention).
    pub deprecated: bool,
    /// Optional deprecation note from a `[deprecated: <note>]` prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecation_note: Option<String>,
    /// `true` when this command itself has subcommands — drilling further
    /// (via `--depth N+1` or invoking `<command> --help --json`) would
    /// expose them.
    pub has_subcommands: bool,
}

/// Depth budget for [`build_help`].
///
/// `Bounded(N)` expands `N` levels fully past the requested node (children
/// at level `N+1` become [`SubcommandSummary`]s); `Tree` recurses without
/// bound (the v2 shape).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpDepth {
    /// Expand this many full levels past the requested node.
    Bounded(usize),
    /// No bound — every descendant is a full [`CommandNode`].
    Tree,
}

impl Default for HelpDepth {
    /// The v3 default: render the requested node fully and its immediate
    /// children as summaries. Drill in with `<command> --help --json` or
    /// `--depth 2`/`--depth tree` for more.
    fn default() -> Self {
        Self::Bounded(1)
    }
}

/// Parse the user-facing `--depth` value: a positive integer (`1`, `2`, …)
/// or the sentinel `tree` / `full`. Returns a human-readable error string
/// suitable for clap's `value_parser`.
///
/// # Errors
///
/// Returns an error string when the input is neither a positive integer nor
/// the literal `tree`/`full`.
pub fn parse_help_depth(s: &str) -> Result<HelpDepth, String> {
    match s {
        "tree" | "full" => Ok(HelpDepth::Tree),
        _ => match s.parse::<usize>() {
            Ok(n) if n >= 1 => Ok(HelpDepth::Bounded(n)),
            Ok(_) => Err(format!(
                "--depth expects a positive integer (>=1) or 'tree'/'full'; got '{s}'"
            )),
            Err(_) => Err(format!(
                "--depth expects a positive integer or 'tree'/'full'; got '{s}'"
            )),
        },
    }
}

/// A named flag.
///
/// The many `bool` fields each mirror an independent piece of clap metadata
/// (takes-value / multiple / required / global / hidden / deprecated /
/// accepts-file-paths); collapsing them into an enum would lose the
/// orthogonality and the stable JSON field names agents read, so the
/// pedantic `struct_excessive_bools` lint is allowed here.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Serialize)]
pub struct FlagInfo {
    /// Stable clap arg id. Always present (even for short-only flags), and
    /// the identifier `conflicts_with` entries refer to.
    pub name: String,
    /// Long name without the leading `--`, when the flag has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long: Option<String>,
    /// Short name without the leading `-`, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short: Option<String>,
    /// Additional long spellings (visible and hidden), sorted.
    pub long_aliases: Vec<String>,
    /// Additional short spellings (visible and hidden), sorted.
    pub short_aliases: Vec<String>,
    /// One-line help text, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Value-name placeholders shown in usage (empty for boolean flags).
    pub value_names: Vec<String>,
    /// Whether the flag consumes a value.
    pub takes_value: bool,
    /// Whether the flag may be repeated / accept multiple values.
    pub multiple: bool,
    /// Whether the flag is required.
    pub required: bool,
    /// Whether the flag is global (propagated to every subcommand).
    pub is_global: bool,
    /// Whether the flag is hidden from the human text help.
    pub hidden: bool,
    /// Whether the flag is deprecated. clap 4.6 exposes no first-class
    /// deprecation getter, so this is driven by the `[deprecated]`
    /// help-text convention (see [`parse_deprecation`]). Emitted
    /// unconditionally so agents can rely on its presence.
    pub deprecated: bool,
    /// Optional deprecation note from a `[deprecated: <note>]` prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecation_note: Option<String>,
    /// Default value(s) applied when the flag is omitted.
    pub defaults: Vec<String>,
    /// Accepted values (the enum) when the flag is value-restricted.
    pub accepted_values: Vec<String>,
    /// Whether the flag accepts a filesystem path as its value (§13) — by
    /// clap value-hint, or registered for a custom parser (`--output`).
    pub accepts_file_paths: bool,
    /// Ids of flags this one is mutually exclusive with (clap `conflicts_with`),
    /// sorted. Reflects declared conflicts; may be one-directional.
    pub conflicts_with: Vec<String>,
    /// Ids of args this one unconditionally pulls in (clap `requires` /
    /// `requires_all`), sorted. Reflects declared requirements; may be
    /// one-directional. Conditional `requires_if` edges are excluded (see
    /// [`requirement_edges`]).
    pub requires: Vec<String>,
    /// Ids of args whose presence makes this one no longer required (clap
    /// `required_unless_present` / `_any`), sorted. The all-of variant
    /// (`required_unless_present_all`) is not represented — see
    /// [`requirement_edges`].
    pub required_unless_present: Vec<String>,
    /// The help section heading this flag is grouped under, when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help_heading: Option<String>,
    /// Value-count / repetition shape.
    pub arity: Arity,
    /// Environment variable that supplies this flag (per §8), when mapped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
}

/// Value-count and repetition shape of a value-taking arg.
#[derive(Debug, Serialize)]
pub struct Arity {
    /// Minimum number of values per occurrence (0 for boolean flags).
    pub min: usize,
    /// Maximum number of values per occurrence. `null` means unbounded —
    /// clap represents that as `usize::MAX`, which is not safely
    /// JSON-representable (exceeds `Number.MAX_SAFE_INTEGER`), so it is
    /// projected as `null` rather than a 2^64-1 literal.
    pub max: Option<usize>,
    /// Whether the arg may appear multiple times (`Append` / `Count`).
    pub repeated: bool,
    /// Whether a single occurrence may carry more than one value.
    pub multi_value: bool,
    /// Delimiter splitting a single value into many, when configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_delimiter: Option<String>,
    /// Whether the value must be attached with `=` (`--flag=value`).
    pub require_equals: bool,
}

/// A positional argument.
///
/// Several orthogonal `bool` markers (required / multiple / accepts-file-paths
/// / deprecated) earn the same `struct_excessive_bools` allowance as
/// [`FlagInfo`] — each is an independent, stably-named JSON field.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Serialize)]
pub struct PositionalInfo {
    /// Argument id.
    pub name: String,
    /// One-line help text, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Value-name placeholders shown in usage.
    pub value_names: Vec<String>,
    /// 1-based position.
    pub index: usize,
    /// Whether the argument is required.
    pub required: bool,
    /// Whether the argument accepts multiple values.
    pub multiple: bool,
    /// Accepted values (the enum) when the argument is value-restricted.
    pub accepted_values: Vec<String>,
    /// Whether the argument accepts a filesystem path as its value (§13).
    pub accepts_file_paths: bool,
    /// Default value(s) applied when the argument is omitted.
    pub defaults: Vec<String>,
    /// Environment variable that supplies this argument (per §8), when mapped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// Whether the positional is deprecated (per the `[deprecated]`
    /// help-text convention — see [`parse_deprecation`]).
    pub deprecated: bool,
    /// Optional deprecation note from a `[deprecated: <note>]` prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecation_note: Option<String>,
}

/// Synthetic id for the global help flag injected into the lenient-parse
/// clone (see [`resolve_help_request`]). Double-underscore prefix keeps it
/// clear of any real arg id.
const HELP_FLAG_ID: &str = "__taskfleet_help_request";

/// Id of the global `--output` arg, read back from the lenient parse and
/// used to register its custom-parser metadata. Kept `pub(crate)` so a test
/// can assert it still matches the real `Cli` arg.
pub(crate) const OUTPUT_ARG_ID: &str = "output";

/// Id of the global `--json` shorthand. It is a real format selector, so
/// `--help --json` must take the same structured-help path as
/// `--help --output json`.
pub(crate) const JSON_ARG_ID: &str = "json";

/// Whether `arg` is *the* global `--output` flag — matched on id, long name,
/// and global-ness together so a future subcommand-local arg that merely
/// reuses the id `output` cannot inherit its custom metadata.
fn is_output_arg(arg: &Arg) -> bool {
    arg.get_id().as_str() == OUTPUT_ARG_ID
        && arg.get_long() == Some(OUTPUT_ARG_ID)
        && arg.is_global_set()
}

/// Outcome of inspecting raw argv for a structured-help request.
#[derive(Debug)]
pub enum HelpRequest {
    /// Not a JSON help request — the caller falls through to clap's normal
    /// dispatch. Covers no `--help`, a bare `--help` (no explicit
    /// `--output`), and `--output text`, all of which keep clap's text
    /// rendering (§14: bare `--help` is unchanged).
    None,
    /// Structured help requested for the resolved subcommand path
    /// (canonical subcommand names, root excluded). `depth` is the rendering
    /// budget — see [`HelpDepth`].
    Render {
        /// Output format / file routing (parsed from `--output`).
        spec: OutputSpec,
        /// Canonical subcommand path from root.
        path: Vec<String>,
        /// How deep to expand the subcommand tree before switching to
        /// [`SubcommandSummary`] entries.
        depth: HelpDepth,
    },
    /// Structured help requested, but a token in subcommand position is not
    /// a known subcommand. The caller emits an error envelope (exit 1)
    /// rather than falling back to root help.
    UnknownSubcommand { token: String },
    /// Structured help requested, but `--depth` carried an unparseable value
    /// (not a positive integer and not `tree`/`full`). The caller emits an
    /// `invalid_arguments` envelope rather than rendering with a silent
    /// default — agents must learn about the bad input.
    InvalidDepth {
        /// The literal value as the user typed it.
        value: String,
    },
    /// Both global output selectors were supplied. This is the same
    /// caller error normal command dispatch reports.
    ConflictingOutputFlags,
    /// `--output` appeared in argv but clap's lenient parse could not
    /// produce an output specification. Never let `--json` hide malformed
    /// or valueless `--output` input.
    InvalidOutput,
}

/// Synthetic id for the global `--depth` arg injected into the lenient-parse
/// clone (see [`resolve_help_request`]).
const DEPTH_ARG_ID: &str = "__taskfleet_help_depth";

/// Resolve a structured-help request from raw argv (excluding argv\[0\])
/// via a clap *lenient* parse.
///
/// This replaces the former hand-rolled argv scan. `root` is the real
/// (canonical) command tree; it is cloned and reconfigured so a single
/// tolerant parse recovers the subcommand path **and** the `--output`
/// value exactly as clap would — robust against value-taking flags at any
/// level, short-flag clusters (`-vh`), non-canonical aliases, and `--`
/// (handled by clap for free).
///
/// Returns [`HelpRequest::Render`] only when **both** an explicit help flag
/// (`--help`/`-h`) and an explicit machine-output selector are present:
/// `--json`, or non-text `--output`. A bare `--help` or `--output text`
/// returns [`HelpRequest::None`]. An unknown subcommand returns
/// [`HelpRequest::UnknownSubcommand`].
#[must_use]
pub fn resolve_help_request(root: &Command, args: &[String]) -> HelpRequest {
    let mut lenient = root
        .clone()
        // Tolerate unknown flags/values so the path and `--output` resolve
        // even when `--help` sits where clap would normally reject it.
        .ignore_errors(true)
        // Suppress clap's built-in `--help` (its action short-circuits to
        // text); we detect help via the injected flag below instead.
        .disable_help_flag(true)
        // Surface an unknown token in subcommand position as an external
        // subcommand in the matches rather than silently dropping it (so we
        // can error on it).
        .allow_external_subcommands(true)
        .arg(
            Arg::new(HELP_FLAG_ID)
                .long("help")
                .short('h')
                .action(ArgAction::SetTrue)
                .global(true),
        )
        // `--depth` is meaningful only under a JSON help request, but the
        // lenient parse is the cheapest place to recover it. Accept the raw
        // string here; the parser runs after the help-request check below.
        .arg(
            Arg::new(DEPTH_ARG_ID)
                .long("depth")
                .num_args(1)
                .action(ArgAction::Set)
                .global(true),
        );
    // `allow_external_subcommands` is a *local* setting — clap does not
    // propagate it — so also apply it to every descendant, otherwise a stray
    // token under a valid noun (`--output json --help run bogus`) is swallowed
    // and the noun resolves cleanly, defeating the unknown-subcommand check.
    enable_external_subcommands_recursively(&mut lenient);

    // clap expects argv[0] to be the program name.
    let with_prog = std::iter::once(root.get_name().to_string()).chain(args.iter().cloned());
    let Ok(matches) = lenient.try_get_matches_from_mut(with_prog) else {
        // `ignore_errors` makes a hard error practically unreachable; if one
        // slips through we simply decline the request.
        return HelpRequest::None;
    };

    // A help request requires an explicit `--help`/`-h`.
    if !matches.get_flag(HELP_FLAG_ID) {
        return HelpRequest::None;
    }

    // ...and an explicit machine-output selector. The global `--json`
    // shorthand is equivalent to `--output json` for this early path too;
    // otherwise `taskfleet --help --json` would fall through to clap's
    // text renderer before normal output selection gets a chance to run.
    // The `jsonl` default does not count (`value_source` distinguishes it),
    // so a bare `--help` keeps clap's text rendering.
    let output_spec = match matches.value_source(OUTPUT_ARG_ID) {
        Some(ValueSource::CommandLine) => matches.get_one::<OutputSpec>(OUTPUT_ARG_ID).cloned(),
        _ => None,
    };
    // `ignore_errors(true)` intentionally tolerates malformed values while
    // resolving a help path. Do not let that tolerance turn an explicit but
    // invalid `--output` into successful `--json` help.
    if output_was_supplied(args) && output_spec.is_none() {
        return HelpRequest::InvalidOutput;
    }
    // `try_get_one` makes the minimal synthetic command trees used by unit
    // tests safe too: asking a real `ArgMatches` for an absent id otherwise
    // panics. The real CLI's `--json` remains global by declaration.
    let json_shorthand = matches
        .try_get_one::<bool>(JSON_ARG_ID)
        .ok()
        .flatten()
        .is_some_and(|value| *value)
        && matches.value_source(JSON_ARG_ID) == Some(ValueSource::CommandLine);
    let spec = match (output_spec, json_shorthand) {
        // Match normal command dispatch: a caller must choose exactly one
        // output selector, including on the early structured-help path.
        (Some(_), true) => return HelpRequest::ConflictingOutputFlags,
        (Some(spec), false) if spec.format != OutputFormat::Text => spec,
        (None, true) => OutputSpec {
            format: OutputFormat::Json,
            file: None,
        },
        _ => return HelpRequest::None,
    };

    // Walk the resolved subcommand path, validating each name against the
    // real tree. With `allow_external_subcommands`, an unknown token in
    // subcommand position surfaces here as a subcommand whose name the real
    // tree does not know — that is the unknown-subcommand signal. We record
    // the *canonical* `child.get_name()` (not the matched token, which clap
    // may return as a non-canonical alias) so `path` is canonical as documented.
    let mut cur = root;
    let mut path = Vec::new();
    let mut node = &matches;
    while let Some((name, sub)) = node.subcommand() {
        match cur.find_subcommand(name) {
            Some(child) => {
                cur = child;
                path.push(child.get_name().to_string());
                node = sub;
            }
            None => {
                return HelpRequest::UnknownSubcommand {
                    token: name.to_string(),
                }
            }
        }
    }

    // Resolve `--depth`. Absent / value-source-default → the v3 default
    // (Bounded(1)). Present from the command line → parse; bad input is a
    // structured error, never a silent fallback.
    let depth = match matches.value_source(DEPTH_ARG_ID) {
        Some(ValueSource::CommandLine) => {
            let raw = matches
                .get_one::<String>(DEPTH_ARG_ID)
                .cloned()
                .unwrap_or_default();
            match parse_help_depth(&raw) {
                Ok(d) => d,
                Err(_) => return HelpRequest::InvalidDepth { value: raw },
            }
        }
        _ => HelpDepth::default(),
    };

    HelpRequest::Render { spec, path, depth }
}

/// Set `allow_external_subcommands(true)` on `cmd` and every descendant.
/// clap's builder methods consume `self`, so each subcommand is swapped out
/// of the mutable borrow, transformed, and swapped back (the placeholder is
/// overwritten before it can be observed).
fn enable_external_subcommands_recursively(cmd: &mut Command) {
    for sub in cmd.get_subcommands_mut() {
        let owned = std::mem::replace(sub, Command::new("")).allow_external_subcommands(true);
        *sub = owned;
        enable_external_subcommands_recursively(sub);
    }
}

/// Walk a canonical subcommand-name path (root excluded) through a built
/// command tree, returning the deepest command and its full invocation
/// path. The names come from [`resolve_help_request`]'s clap parse, so
/// every lookup succeeds; an unknown name simply stops the walk (defensive,
/// not expected).
#[must_use]
pub fn navigate_path<'a>(root: &'a Command, names: &[String]) -> (&'a Command, String) {
    let mut cur = root;
    let mut path = vec![root.get_name().to_string()];
    for name in names {
        let Some(sc) = cur.find_subcommand(name) else {
            break;
        };
        cur = sc;
        path.push(sc.get_name().to_string());
    }
    (cur, path.join(" "))
}

/// Project a built [`Command`] (and its descendants) onto [`HelpData`].
///
/// `command_path` is the full invocation path of `cmd` (from
/// [`navigate_path`]); child paths are derived by appending the child name.
/// `depth` is the user-facing budget: with the default `HelpDepth::Bounded(1)`,
/// the requested node is rendered fully and its immediate children appear as
/// [`SubcommandSummary`]s. `HelpDepth::Tree` reproduces the unbounded v2 shape.
#[must_use]
pub fn build_help(cmd: &Command, command_path: &str, depth: HelpDepth) -> HelpData {
    HelpData {
        schema_version_help: SCHEMA_VERSION_HELP,
        command: build_node(cmd, command_path, depth),
    }
}

fn build_node(cmd: &Command, command_path: &str, depth: HelpDepth) -> CommandNode {
    let mut flags: Vec<FlagInfo> = cmd
        .get_arguments()
        .filter(|a| !a.is_positional())
        .map(|a| build_flag(cmd, a))
        .collect();
    flags.sort_by(|a, b| a.name.cmp(&b.name));

    let mut positionals: Vec<PositionalInfo> =
        cmd.get_positionals().map(build_positional).collect();
    positionals.sort_by_key(|p| p.index);

    // Subcommand recursion is gated on `depth`:
    //   - `Tree`         → kids are Full, recursing with `Tree`.
    //   - `Bounded(1)`   → kids are Summary (we've spent our last level on
    //                      this node; immediate children get the cheap form).
    //   - `Bounded(n>1)` → kids are Full, recursing with `Bounded(n - 1)`
    //                      (one less full level remaining below them).
    //   - `Bounded(0)`   → not reached in normal use (parse_help_depth
    //                      requires n>=1); render as Summary to stay safe.
    let mut subcommands: Vec<SubcommandEntry> = cmd
        .get_subcommands()
        .map(|sc| {
            let child_path = format!("{command_path} {}", sc.get_name());
            match depth {
                HelpDepth::Tree => {
                    SubcommandEntry::Full(build_node(sc, &child_path, HelpDepth::Tree))
                }
                HelpDepth::Bounded(n) if n >= 2 => {
                    SubcommandEntry::Full(build_node(sc, &child_path, HelpDepth::Bounded(n - 1)))
                }
                HelpDepth::Bounded(_) => SubcommandEntry::Summary(build_summary(sc, &child_path)),
            }
        })
        .collect();
    subcommands.sort_by(|a, b| subcommand_entry_path(a).cmp(subcommand_entry_path(b)));

    // A command's deprecation marker may sit on either `about` (the canonical
    // one-liner — the recommended place) or `long_about`; the prefix is
    // stripped from both so it never leaks into the rendered text, and the
    // command is deprecated if either carries it (note: `about` wins).
    let about_dep = parse_deprecation(cmd.get_about().map(ToString::to_string));
    let long_dep = parse_deprecation(cmd.get_long_about().map(ToString::to_string));
    let about = about_dep.text;
    // Only surface `long_about` when it adds something over `about`; clap
    // returns the same text for both when only `about` was set.
    let long_about = long_dep.text.filter(|l| Some(l) != about.as_ref());

    CommandNode {
        command: command_path.to_string(),
        about,
        long_about,
        aliases: cmd.get_visible_aliases().map(ToString::to_string).collect(),
        hidden: cmd.is_hide_set(),
        deprecated: about_dep.deprecated || long_dep.deprecated,
        deprecation_note: about_dep.note.or(long_dep.note),
        version: cmd.get_version().map(ToString::to_string),
        flags,
        positionals,
        subcommands,
    }
}

/// Project a [`Command`] onto a lightweight [`SubcommandSummary`]. Used at
/// the depth cutoff so an agent can decide whether to drill in without
/// receiving every grandchild's flags + positionals.
fn build_summary(cmd: &Command, command_path: &str) -> SubcommandSummary {
    let about_dep = parse_deprecation(cmd.get_about().map(ToString::to_string));
    let long_dep = parse_deprecation(cmd.get_long_about().map(ToString::to_string));
    SubcommandSummary {
        command: command_path.to_string(),
        about: about_dep.text,
        aliases: cmd.get_visible_aliases().map(ToString::to_string).collect(),
        hidden: cmd.is_hide_set(),
        deprecated: about_dep.deprecated || long_dep.deprecated,
        deprecation_note: about_dep.note.or(long_dep.note),
        has_subcommands: cmd.get_subcommands().next().is_some(),
    }
}

/// Sort key for [`SubcommandEntry`]: the `command` field, which is the
/// canonical invocation path on both variants.
fn subcommand_entry_path(entry: &SubcommandEntry) -> &str {
    match entry {
        SubcommandEntry::Full(n) => n.command.as_str(),
        SubcommandEntry::Summary(s) => s.command.as_str(),
    }
}

/// Build a [`FlagInfo`]. Every non-positional arg has a long or short name,
/// so (unlike v1) short-only flags are included with `name` falling back to
/// the clap id. `cmd` is the owning command, needed for `conflicts_with`.
fn build_flag(cmd: &Command, arg: &Arg) -> FlagInfo {
    let action = arg.get_action();
    let takes_value = takes_value(action);
    let dep = parse_deprecation(arg.get_help().map(ToString::to_string));
    let (requires, required_unless_present) = requirement_edges(arg);
    FlagInfo {
        name: arg.get_id().as_str().to_string(),
        long: arg.get_long().map(ToString::to_string),
        short: arg.get_short().map(|c| c.to_string()),
        long_aliases: long_aliases(arg),
        short_aliases: short_aliases(arg),
        help: dep.text,
        // Boolean flags carry a derived value-name placeholder in clap;
        // suppress it so agents don't read a value where none is taken.
        value_names: if takes_value {
            value_names(arg)
        } else {
            Vec::new()
        },
        takes_value,
        multiple: multiple(arg, action),
        required: arg.is_required_set(),
        is_global: arg.is_global_set(),
        hidden: arg.is_hide_set(),
        deprecated: dep.deprecated,
        deprecation_note: dep.note,
        defaults: default_values(arg),
        accepted_values: accepted_values(arg),
        accepts_file_paths: accepts_file_paths(arg),
        conflicts_with: conflicts_with(cmd, arg),
        requires,
        required_unless_present,
        help_heading: arg.get_help_heading().map(ToString::to_string),
        arity: arity(arg, action),
        env: arg.get_env().map(|e| e.to_string_lossy().into_owned()),
    }
}

fn build_positional(arg: &Arg) -> PositionalInfo {
    let dep = parse_deprecation(arg.get_help().map(ToString::to_string));
    PositionalInfo {
        name: arg.get_id().as_str().to_string(),
        help: dep.text,
        value_names: value_names(arg),
        // `build()` assigns every positional a 1-based index before we
        // walk, so this is always `Some`. Emitting a bogus `0` instead
        // would silently mis-sort and lie about the position.
        index: arg
            .get_index()
            .expect("positional has an index after Command::build()"),
        required: arg.is_required_set(),
        multiple: multiple(arg, arg.get_action()),
        accepted_values: accepted_values(arg),
        accepts_file_paths: accepts_file_paths(arg),
        defaults: default_values(arg),
        env: arg.get_env().map(|e| e.to_string_lossy().into_owned()),
        deprecated: dep.deprecated,
        deprecation_note: dep.note,
    }
}

fn value_names(arg: &Arg) -> Vec<String> {
    arg.get_value_names()
        .unwrap_or_default()
        .iter()
        .map(ToString::to_string)
        .collect()
}

fn default_values(arg: &Arg) -> Vec<String> {
    // `--output`'s default is applied by the shared output resolver rather
    // than clap (the field is an Option so `--json` can remain a distinct
    // shorthand). Project that real effective default, not an empty list.
    if is_output_arg(arg) {
        return vec!["jsonl".to_string()];
    }
    arg.get_default_values()
        .iter()
        .map(|v| v.to_string_lossy().into_owned())
        .collect()
}

/// Whether an explicit `--output` spelling occurs before `--` in raw argv.
/// The lenient clap clone can discard a bad/missing value, so `ArgMatches`
/// alone cannot distinguish that caller error from an absent selector.
fn output_was_supplied(args: &[String]) -> bool {
    args.iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| arg == "--output" || arg.starts_with("--output="))
}

/// Accepted (enum) values, excluding any the author hid from help. Order is
/// the author's declaration order (semantically meaningful, deterministic),
/// so this list is *not* re-sorted. Falls back to a registered set for
/// custom value-parsers (see [`custom_accepted_values`]).
fn accepted_values(arg: &Arg) -> Vec<String> {
    let from_parser: Vec<String> = arg
        .get_possible_values()
        .into_iter()
        .filter(|p| !p.is_hide_set())
        .map(|p| p.get_name().to_string())
        .collect();
    if from_parser.is_empty() {
        if let Some(custom) = custom_accepted_values(arg) {
            return custom;
        }
    }
    from_parser
}

/// Accepted-value set for args whose custom `value_parser` hides its enum
/// from clap (so [`Arg::get_possible_values`] is empty). Registered by id;
/// the sole entry today is the global `--output` (§13). Values are sorted —
/// unlike a derived enum, they carry no declaration order.
fn custom_accepted_values(arg: &Arg) -> Option<Vec<String>> {
    is_output_arg(arg).then(|| vec!["json".to_string(), "jsonl".to_string(), "text".to_string()])
}

/// All additional long spellings (visible and hidden), sorted for stability.
fn long_aliases(arg: &Arg) -> Vec<String> {
    let mut v: Vec<String> = arg
        .get_all_aliases()
        .unwrap_or_default()
        .into_iter()
        .map(ToString::to_string)
        .collect();
    v.sort();
    v
}

/// All additional short spellings (visible and hidden), sorted.
fn short_aliases(arg: &Arg) -> Vec<String> {
    let mut v: Vec<String> = arg
        .get_all_short_aliases()
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.to_string())
        .collect();
    v.sort();
    v
}

/// Ids of the flags `arg` is mutually exclusive with, sorted and de-duped.
/// Reflects clap's declared conflicts (`conflicts_with`); clap stores them
/// on the declaring side, so the relation may be one-directional.
///
/// Global args are skipped: `Command::get_arg_conflicts_with` **panics** when
/// an arg's conflict target is unknown to the command, and a global flag is
/// projected into every subcommand where its (root-defined) conflict targets
/// may be absent. Non-global args only conflict with siblings present in the
/// same command, so they are safe. Global-flag conflicts are vanishingly rare
/// and not worth risking a panic on the help-render path.
fn conflicts_with(cmd: &Command, arg: &Arg) -> Vec<String> {
    if arg.is_global_set() {
        return Vec::new();
    }
    let mut v: Vec<String> = cmd
        .get_arg_conflicts_with(arg)
        .into_iter()
        .map(|a| a.get_id().as_str().to_string())
        .collect();
    v.sort();
    v.dedup();
    v
}

// ----------------------------------------------------------------------
// Requirement edges via the `Arg` Debug projection
// ----------------------------------------------------------------------
//
// clap 4.6 exposes a public getter for *conflicts*
// (`Command::get_arg_conflicts_with`, used by [`conflicts_with`]) but **none**
// for *requirements*: the data lives in private `Arg` fields with no accessor
// (`requires: Vec<(ArgPredicate, Id)>`, `r_unless: Vec<Id>` — verified against
// `clap_builder-4.6.0/src/builder/arg.rs`). The CLI declares real requirements
// today (`run create` `--parent-run-id` ⇄ `--parent-node-id`), so omitting the
// edges leaves a genuine gap.
//
// The faithful alternative to a hand-maintained side-registry — which would
// silently drift from the `#[arg(requires = ...)]` declarations — is to read
// the *real* private fields back through `Arg`'s `Debug` projection, the only
// drift-free source. The Debug format is not a stability guarantee, so guard
// tests (`requires_edge_is_recovered_from_a_synthetic_arg`,
// `required_unless_present_is_recovered_from_a_synthetic_arg`,
// `requirement_edges_ignore_lookalike_help_text`) pin the recovery — a clap
// upgrade that changes the format fails CI loudly rather than silently emptying
// the field. A real-tree assertion lives in `tests/help_json.rs`.

/// Both requirement-edge lists for `arg`, recovered from a single `Arg` Debug
/// projection: `(requires, required_unless_present)`, each sorted and de-duped.
///
/// - `requires` (clap `requires` / `requires_all`): only the unconditional
///   `IsPresent` predicate is surfaced; a conditional `requires_if`
///   (`Equals(..)`) target is a different relationship and is excluded.
/// - `required_unless_present` (clap `required_unless_present` / `_any`): from
///   the `r_unless` field. The all-of variant `required_unless_present_all`
///   writes a *separate* `r_unless_all` field that `Arg`'s `Debug` does not
///   print, so an all-of requirement is **not** represented (a documented gap —
///   none in the tree today).
///
/// See the module note above on why this reads `Arg`'s `Debug` rather than a
/// getter.
fn requirement_edges(arg: &Arg) -> (Vec<String>, Vec<String>) {
    let debug = format!("{arg:?}");

    // `requires` entries are `(<predicate>, "<id>")`; keep only `IsPresent`.
    let requires = debug_field_list(&debug, "requires").map_or_else(Vec::new, |seg| {
        sorted_dedup(
            seg.match_indices("(IsPresent, \"")
                .filter_map(|(i, m)| {
                    let rest = &seg[i + m.len()..];
                    rest.find('"').map(|end| rest[..end].to_string())
                })
                .collect(),
        )
    });

    // `r_unless` entries are bare `"<id>"`.
    let required_unless_present = debug_field_list(&debug, "r_unless")
        .map_or_else(Vec::new, |seg| sorted_dedup(quoted_tokens(seg)));

    (requires, required_unless_present)
}

/// Return the `[...]` payload of a named field in an `Arg` `Debug` string —
/// the slice *between* the field's brackets (exclusive), bracket-depth-aware so
/// a nested list cannot truncate it. `None` if the field is absent.
///
/// The field name is matched only **outside** any quoted string: clap prints
/// the user-controlled `help` / `long_help` text (quoted) *before*
/// `requires` / `r_unless`, so a help string that happens to contain
/// `requires: [` must not shadow the real field. Quote tracking honors `\\`
/// and `\"` escapes (the derive uses `escape_debug`). Outside quotes the Debug
/// output is pure ASCII, so byte indexing stays on char boundaries.
fn debug_field_list<'a>(debug: &'a str, field: &str) -> Option<&'a str> {
    let needle = format!("{field}: [");
    let bytes = debug.as_bytes();
    let mut i = 0;
    let mut in_quote = false;
    let mut escaped = false;
    while i < bytes.len() {
        if in_quote {
            match bytes[i] {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_quote = false,
                _ => {}
            }
            i += 1;
        } else if bytes[i] == b'"' {
            in_quote = true;
            i += 1;
        } else if debug[i..].starts_with(&needle) {
            return bracket_payload(debug, i + needle.len());
        } else {
            i += 1;
        }
    }
    None
}

/// Slice from `start` to the matching close bracket of the list opened just
/// before it, bracket-depth-aware. `start` is the byte after the opening `[`.
fn bracket_payload(debug: &str, start: usize) -> Option<&str> {
    let mut depth = 1usize;
    for (i, c) in debug[start..].char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&debug[start..start + i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Every double-quoted token in a slice (the bare-id form used by `r_unless`).
fn quoted_tokens(seg: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = seg;
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        out.push(after[..close].to_string());
        rest = &after[close + 1..];
    }
    out
}

/// Sort and de-duplicate a list of ids (the stable form every edge list emits).
fn sorted_dedup(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

/// Whether `arg` accepts a filesystem path as its value (§13). Derived from
/// clap's value hint (PathBuf-typed args default to [`ValueHint::AnyPath`]),
/// plus an explicit allowance for the custom-parsed `--output`.
fn accepts_file_paths(arg: &Arg) -> bool {
    if is_output_arg(arg) {
        return true;
    }
    matches!(
        arg.get_value_hint(),
        ValueHint::AnyPath | ValueHint::FilePath | ValueHint::DirPath | ValueHint::ExecutablePath
    )
}

/// Value-count / repetition shape of `arg`. An unbounded maximum
/// (clap's `usize::MAX` sentinel) is projected as `max: None`.
fn arity(arg: &Arg, action: &ArgAction) -> Arity {
    let range = arg.get_num_args();
    let raw_max = range.map_or(0, |r| r.max_values());
    let max = (raw_max != usize::MAX).then_some(raw_max);
    Arity {
        min: range.map_or(0, |r| r.min_values()),
        max,
        repeated: matches!(action, ArgAction::Append | ArgAction::Count),
        // Unbounded (`max == None`) is inherently multi-value.
        multi_value: max.is_none_or(|m| m > 1),
        value_delimiter: arg.get_value_delimiter().map(|c| c.to_string()),
        require_equals: arg.is_require_equals_set(),
    }
}

/// Result of applying the [`parse_deprecation`] help-text convention.
struct Deprecation {
    deprecated: bool,
    note: Option<String>,
    /// The help/about text with any `[deprecated...]` prefix stripped
    /// (`None` if nothing remains).
    text: Option<String>,
}

// ----------------------------------------------------------------------
// Help-text convention: marking deprecation
// ----------------------------------------------------------------------
//
// clap 4.6 exposes NO `Arg`/`Command` deprecation getter, so there is no
// structural source for `deprecated`. Instead, deprecation is declared in
// the help text itself and parsed back out here:
//
//   - `[deprecated]`                → deprecated, no note
//   - `[deprecated: use --foo bar]` → deprecated, note = "use --foo bar"
//
// The token must be a PREFIX of the help/about text (after any leading
// whitespace the derive may inject). The walker strips it from the rendered
// `about`/`long_about`/`help`/positional-help and sets `deprecated: true`
// (with the optional note). Authors opt something into deprecation purely in
// its doc-comment / `#[arg(help = ...)]`:
//
//   /// [deprecated: use `run create --kind`] Spawn a run.
//   Spawn { ... }
//
// A malformed `[deprecated:` with no closing `]` is still treated as
// deprecated (consuming the remainder as the note) rather than silently
// leaking the marker into the rendered text. Applies uniformly to flags,
// positionals, and whole subcommands. There are no deprecations in the tree
// today; this is the forward path.

/// Parse the `[deprecated]` / `[deprecated: <note>]` prefix convention from
/// a piece of help/about text. See the convention block above.
fn parse_deprecation(text: Option<String>) -> Deprecation {
    let Some(text) = text else {
        return Deprecation {
            deprecated: false,
            note: None,
            text: None,
        };
    };
    // Tolerate leading whitespace/newlines the derive may inject.
    let trimmed = text.trim_start();
    // `[deprecated: <note>]` — note runs up to the first closing `]`; a
    // missing `]` consumes the rest as the note (still deprecated).
    if let Some(rest) = trimmed.strip_prefix("[deprecated:") {
        let (note, after) = match rest.find(']') {
            Some(end) => (rest[..end].trim(), rest[end + 1..].trim_start()),
            None => (rest.trim(), ""),
        };
        return Deprecation {
            deprecated: true,
            note: non_empty(note),
            text: non_empty(after),
        };
    }
    // `[deprecated]` — bare marker, no note.
    if let Some(rest) = trimmed.strip_prefix("[deprecated]") {
        return Deprecation {
            deprecated: true,
            note: None,
            text: non_empty(rest.trim_start()),
        };
    }
    Deprecation {
        deprecated: false,
        note: None,
        text: Some(text),
    }
}

/// `Some(trimmed)` unless the string is empty, in which case `None`.
fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// Whether an arg consumes a value, derived from its action. `Help` /
/// `Version` / `SetTrue` / `SetFalse` / `Count` are valueless; `Set` /
/// `Append` carry a value. (Positionals always take a value and are
/// handled via this same mapping — their action is `Set`/`Append`.)
fn takes_value(action: &ArgAction) -> bool {
    matches!(action, ArgAction::Set | ArgAction::Append)
}

/// Whether an arg may repeat / accept multiple values: `Append` and
/// `Count` are inherently repeatable; otherwise consult the value count.
fn multiple(arg: &Arg, action: &ArgAction) -> bool {
    matches!(action, ArgAction::Append | ArgAction::Count)
        || arg.get_num_args().is_some_and(|r| r.max_values() > 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A miniature command tree mirroring the real one's shape: a global
    /// `--output` (custom `OutputSpec` parser, `jsonl` default) plus a noun
    /// (`run`) with a leaf verb (`create`).
    fn test_root() -> Command {
        Command::new("tool")
            .arg(
                Arg::new(OUTPUT_ARG_ID)
                    .long("output")
                    .global(true)
                    .default_value("jsonl")
                    .value_parser(crate::output::parse_output_value),
            )
            .subcommand(Command::new("run").subcommand(Command::new("create")))
    }

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn render_resolves_leaf_path_and_output() {
        let req = resolve_help_request(
            &test_root(),
            &args(&["run", "create", "--help", "--output", "json"]),
        );
        match req {
            HelpRequest::Render { spec, path, depth } => {
                assert_eq!(spec.format, OutputFormat::Json);
                assert_eq!(path, vec!["run".to_string(), "create".to_string()]);
                assert_eq!(depth, HelpDepth::default());
            }
            _ => panic!("expected Render"),
        }
    }

    #[test]
    fn output_can_precede_the_subcommand_path() {
        // A robustness win over the old scan: `--output` resolves wherever
        // it sits, including before any subcommand.
        let req =
            resolve_help_request(&test_root(), &args(&["--output", "jsonl", "run", "--help"]));
        match req {
            HelpRequest::Render { spec, path, depth } => {
                assert_eq!(spec.format, OutputFormat::Jsonl);
                assert_eq!(path, vec!["run".to_string()]);
                assert_eq!(depth, HelpDepth::default());
            }
            _ => panic!("expected Render"),
        }
    }

    #[test]
    fn bare_help_without_output_is_none() {
        // Default `--output jsonl` must NOT count — clap's text help stands.
        assert!(matches!(
            resolve_help_request(&test_root(), &args(&["run", "create", "--help"])),
            HelpRequest::None
        ));
    }

    #[test]
    fn output_text_with_help_is_none() {
        assert!(matches!(
            resolve_help_request(&test_root(), &args(&["run", "--help", "--output", "text"])),
            HelpRequest::None
        ));
    }

    #[test]
    fn no_help_flag_is_none() {
        assert!(matches!(
            resolve_help_request(&test_root(), &args(&["run", "--output", "json"])),
            HelpRequest::None
        ));
    }

    #[test]
    fn double_dash_suppresses_detection() {
        // After `--`, a trailing `--help` is positional data, not a request.
        assert!(matches!(
            resolve_help_request(
                &test_root(),
                &args(&["run", "--", "--help", "--output", "json"])
            ),
            HelpRequest::None
        ));
    }

    #[test]
    fn unknown_subcommand_after_flags_is_flagged() {
        // Flag-first ordering surfaces the bad token as an external
        // subcommand the real tree rejects.
        match resolve_help_request(
            &test_root(),
            &args(&["--help", "--output", "json", "bogus"]),
        ) {
            HelpRequest::UnknownSubcommand { token } => assert_eq!(token, "bogus"),
            other => panic!("expected UnknownSubcommand, got {other:?}"),
        }
    }

    #[test]
    fn deprecation_prefix_is_parsed_and_stripped() {
        // Bare marker.
        let d = parse_deprecation(Some("[deprecated] Old flag.".to_string()));
        assert!(d.deprecated);
        assert_eq!(d.note, None);
        assert_eq!(d.text.as_deref(), Some("Old flag."));

        // Marker with a note.
        let d = parse_deprecation(Some("[deprecated: use --kind] Spawn.".to_string()));
        assert!(d.deprecated);
        assert_eq!(d.note.as_deref(), Some("use --kind"));
        assert_eq!(d.text.as_deref(), Some("Spawn."));

        // Marker with nothing after it collapses the text to None.
        let d = parse_deprecation(Some("[deprecated]".to_string()));
        assert!(d.deprecated);
        assert_eq!(d.text, None);

        // Non-deprecated text is untouched; a non-prefix occurrence does not
        // trigger the convention.
        let d = parse_deprecation(Some("Create a [deprecated] run.".to_string()));
        assert!(!d.deprecated);
        assert_eq!(d.text.as_deref(), Some("Create a [deprecated] run."));
    }

    #[test]
    fn deprecation_surfaces_on_a_synthetic_flag() {
        // End-to-end through the walker: a flag whose help opens with the
        // marker renders `deprecated: true` with the prefix stripped.
        let mut cmd = Command::new("tool").arg(
            Arg::new("legacy")
                .long("legacy")
                .action(ArgAction::SetTrue)
                .help("[deprecated: use --modern] Old toggle."),
        );
        cmd.build();
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_id() == "legacy")
            .unwrap();
        let flag = build_flag(&cmd, arg);
        assert!(flag.deprecated);
        assert_eq!(flag.deprecation_note.as_deref(), Some("use --modern"));
        assert_eq!(flag.help.as_deref(), Some("Old toggle."));
    }

    #[test]
    fn short_only_flag_is_included_with_id_fallback() {
        // v2 reconsideration: short-only flags are no longer skipped — they
        // appear with `name` = clap id and no `long`.
        let mut cmd = Command::new("tool").arg(
            Arg::new("verbose")
                .short('v')
                .action(ArgAction::Count)
                .help("Increase verbosity."),
        );
        cmd.build();
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_id() == "verbose")
            .unwrap();
        let flag = build_flag(&cmd, arg);
        assert_eq!(flag.name, "verbose");
        assert_eq!(flag.long, None);
        assert_eq!(flag.short.as_deref(), Some("v"));
        // `Count` is a repeatable, valueless action.
        assert!(flag.arity.repeated);
        assert!(!flag.takes_value);
    }

    /// Test helper: pluck the `Full` variant by canonical path or panic.
    fn expect_full<'a>(subs: &'a [SubcommandEntry], path: &str) -> &'a CommandNode {
        subs.iter()
            .find_map(|e| match e {
                SubcommandEntry::Full(n) if n.command == path => Some(n),
                _ => None,
            })
            .unwrap_or_else(|| panic!("expected full subcommand {path:?} in {subs:?}"))
    }

    /// Test helper: pluck the `Summary` variant by canonical path or panic.
    fn expect_summary<'a>(subs: &'a [SubcommandEntry], path: &str) -> &'a SubcommandSummary {
        subs.iter()
            .find_map(|e| match e {
                SubcommandEntry::Summary(s) if s.command == path => Some(s),
                _ => None,
            })
            .unwrap_or_else(|| panic!("expected summary subcommand {path:?} in {subs:?}"))
    }

    #[test]
    fn deprecation_surfaces_on_a_synthetic_subcommand() {
        let mut cmd =
            Command::new("tool").subcommand(Command::new("old").about("[deprecated] Legacy verb."));
        cmd.build();
        let node = build_node(&cmd, "tool", HelpDepth::Tree);
        let old = expect_full(&node.subcommands, "tool old");
        assert!(old.deprecated);
        assert_eq!(old.about.as_deref(), Some("Legacy verb."));
    }

    #[test]
    fn command_deprecation_can_come_from_long_about() {
        // The marker on `long_about` alone still flags the command (and is
        // stripped from the rendered long text).
        let mut cmd = Command::new("tool").subcommand(
            Command::new("old")
                .about("Legacy verb.")
                .long_about("[deprecated: gone in 1.0] Legacy verb, more detail."),
        );
        cmd.build();
        let node = build_node(&cmd, "tool", HelpDepth::Tree);
        let old = expect_full(&node.subcommands, "tool old");
        assert!(old.deprecated);
        assert_eq!(old.deprecation_note.as_deref(), Some("gone in 1.0"));
        assert_eq!(old.long_about.as_deref(), Some("Legacy verb, more detail."));
    }

    #[test]
    fn default_depth_summarizes_immediate_children() {
        // `--depth 1` (default): root is full, immediate children are
        // SubcommandSummary entries — they expose name/about/aliases/etc but
        // NOT flags/positionals/grandchildren.
        let mut cmd = Command::new("tool").subcommand(
            Command::new("noun")
                .about("A noun.")
                .subcommand(Command::new("verb").about("A verb.")),
        );
        cmd.build();
        let node = build_node(&cmd, "tool", HelpDepth::Bounded(1));
        let noun = expect_summary(&node.subcommands, "tool noun");
        assert_eq!(noun.about.as_deref(), Some("A noun."));
        assert!(noun.has_subcommands, "noun has a verb under it");
    }

    #[test]
    fn depth_two_expands_one_more_level() {
        // `--depth 2`: root + immediate children full, grandchildren as
        // summaries.
        let mut cmd = Command::new("tool").subcommand(
            Command::new("noun")
                .about("A noun.")
                .subcommand(Command::new("verb").about("A verb.")),
        );
        cmd.build();
        let node = build_node(&cmd, "tool", HelpDepth::Bounded(2));
        let noun = expect_full(&node.subcommands, "tool noun");
        let _ = expect_summary(&noun.subcommands, "tool noun verb");
    }

    #[test]
    fn tree_depth_recurses_unbounded() {
        // `--depth tree`: every descendant is a full CommandNode (the v2
        // shape, retained for callers that need the whole surface in one
        // payload — snapshot tests, doc generation).
        let mut cmd = Command::new("tool").subcommand(
            Command::new("noun")
                .about("A noun.")
                .subcommand(Command::new("verb").about("A verb.")),
        );
        cmd.build();
        let node = build_node(&cmd, "tool", HelpDepth::Tree);
        let noun = expect_full(&node.subcommands, "tool noun");
        let _ = expect_full(&noun.subcommands, "tool noun verb");
    }

    #[test]
    fn parse_help_depth_accepts_positive_int_and_tree_aliases() {
        assert_eq!(parse_help_depth("1").unwrap(), HelpDepth::Bounded(1));
        assert_eq!(parse_help_depth("7").unwrap(), HelpDepth::Bounded(7));
        assert_eq!(parse_help_depth("tree").unwrap(), HelpDepth::Tree);
        assert_eq!(parse_help_depth("full").unwrap(), HelpDepth::Tree);
        assert!(parse_help_depth("0").is_err()); // <1 rejected
        assert!(parse_help_depth("-1").is_err());
        assert!(parse_help_depth("abc").is_err());
        assert!(parse_help_depth("").is_err());
    }

    #[test]
    fn resolve_help_request_carries_depth_from_flag() {
        let req = resolve_help_request(
            &test_root(),
            &args(&["--help", "--output", "json", "--depth", "2"]),
        );
        match req {
            HelpRequest::Render { depth, .. } => assert_eq!(depth, HelpDepth::Bounded(2)),
            _ => panic!("expected Render"),
        }
    }

    #[test]
    fn resolve_help_request_recognises_tree() {
        let req = resolve_help_request(
            &test_root(),
            &args(&["--help", "--output", "json", "--depth", "tree"]),
        );
        match req {
            HelpRequest::Render { depth, .. } => assert_eq!(depth, HelpDepth::Tree),
            _ => panic!("expected Render"),
        }
    }

    #[test]
    fn resolve_help_request_rejects_bad_depth_value() {
        let req = resolve_help_request(
            &test_root(),
            &args(&["--help", "--output", "json", "--depth", "garbage"]),
        );
        match req {
            HelpRequest::InvalidDepth { value } => assert_eq!(value, "garbage"),
            other => panic!("expected InvalidDepth, got {other:?}"),
        }
    }

    #[test]
    fn malformed_deprecation_marker_is_not_leaked() {
        // `[deprecated:` without a closing `]` is still treated as deprecated
        // and never leaks the raw marker into the rendered text.
        let d = parse_deprecation(Some("[deprecated: use --modern".to_string()));
        assert!(d.deprecated);
        assert_eq!(d.note.as_deref(), Some("use --modern"));
        assert_eq!(d.text, None);
    }

    #[test]
    fn nested_unknown_subcommand_after_flags_is_flagged() {
        // Regression for the review finding: a stray token under a *valid*
        // noun must still be rejected (needs recursive
        // allow_external_subcommands), not resolve to the noun's help.
        match resolve_help_request(
            &test_root(),
            &args(&["--output", "json", "--help", "run", "bogus"]),
        ) {
            HelpRequest::UnknownSubcommand { token } => assert_eq!(token, "bogus"),
            other => panic!("expected UnknownSubcommand, got {other:?}"),
        }
    }

    #[test]
    fn requires_edge_is_recovered_from_a_synthetic_arg() {
        // End-to-end through the walker: an arg declaring `requires` surfaces
        // the target id; a sibling with no requirement stays empty (the edge
        // is one-directional, like `conflicts_with`).
        let mut cmd = Command::new("tool")
            .arg(Arg::new("a").long("a").requires("b"))
            .arg(Arg::new("b").long("b"));
        cmd.build();
        let flag = |id: &str| {
            let arg = cmd.get_arguments().find(|a| a.get_id() == id).unwrap();
            build_flag(&cmd, arg)
        };
        assert_eq!(flag("a").requires, vec!["b".to_string()]);
        assert_eq!(flag("b").requires, Vec::<String>::new());
    }

    #[test]
    fn conditional_requires_if_is_excluded() {
        // `requires_if` is a conditional (`Equals`) predicate, not an
        // unconditional edge — it must not leak into `requires`, and the
        // predicate value ("x") must never be mistaken for an arg id.
        let mut cmd = Command::new("tool")
            .arg(Arg::new("a").long("a").requires_if("x", "b").requires("c"))
            .arg(Arg::new("b").long("b"))
            .arg(Arg::new("c").long("c"));
        cmd.build();
        let arg = cmd.get_arguments().find(|a| a.get_id() == "a").unwrap();
        let flag = build_flag(&cmd, arg);
        // Only the unconditional `requires("c")` survives.
        assert_eq!(flag.requires, vec!["c".to_string()]);
    }

    #[test]
    fn required_unless_present_is_recovered_from_a_synthetic_arg() {
        // `required_unless_present` / `_any` land in clap's `r_unless`, which
        // the Debug projection recovers (sorted, de-duped).
        let mut cmd = Command::new("tool")
            .arg(
                Arg::new("a")
                    .long("a")
                    .required_unless_present_any(["c", "b"]),
            )
            .arg(Arg::new("b").long("b"))
            .arg(Arg::new("c").long("c"));
        cmd.build();
        let arg = cmd.get_arguments().find(|a| a.get_id() == "a").unwrap();
        let flag = build_flag(&cmd, arg);
        assert_eq!(
            flag.required_unless_present,
            vec!["b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn requirement_edges_ignore_lookalike_help_text() {
        // `Arg`'s Debug prints the user-controlled `help` text (quoted) BEFORE
        // the real `requires` / `r_unless` fields. A help string containing the
        // field needle must neither shadow the real edge (false negative) nor
        // fabricate one (false positive) — the parse skips quoted text.
        let mut cmd = Command::new("tool")
            .arg(
                Arg::new("a")
                    .long("a")
                    .help("needs requires: [(IsPresent, \"ghost\")] and r_unless: [\"ghost\"]")
                    .requires("realdep")
                    .required_unless_present("alt"),
            )
            .arg(Arg::new("realdep").long("realdep"))
            .arg(Arg::new("alt").long("alt"));
        cmd.build();
        let arg = cmd.get_arguments().find(|a| a.get_id() == "a").unwrap();
        let flag = build_flag(&cmd, arg);
        assert_eq!(flag.requires, vec!["realdep".to_string()]);
        assert_eq!(flag.required_unless_present, vec!["alt".to_string()]);
    }

    #[test]
    fn requirement_edges_default_empty() {
        // A plain flag carries empty edge lists (the additive default that
        // lets the new fields ride schema v2).
        let mut cmd = Command::new("tool").arg(Arg::new("a").long("a"));
        cmd.build();
        let arg = cmd.get_arguments().find(|a| a.get_id() == "a").unwrap();
        let flag = build_flag(&cmd, arg);
        assert_eq!(flag.requires, Vec::<String>::new());
        assert_eq!(flag.required_unless_present, Vec::<String>::new());
    }

    #[test]
    fn unbounded_arity_max_is_null_not_usize_max() {
        let mut cmd = Command::new("tool").arg(
            Arg::new("items")
                .long("items")
                .num_args(1..)
                .action(ArgAction::Append),
        );
        cmd.build();
        let arg = cmd.get_arguments().find(|a| a.get_id() == "items").unwrap();
        let flag = build_flag(&cmd, arg);
        assert_eq!(flag.arity.max, None, "unbounded max must serialize as null");
        assert!(flag.arity.multi_value);
    }
}
