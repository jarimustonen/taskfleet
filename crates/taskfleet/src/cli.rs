//! Clap-based CLI dispatch.
//!
//! MVP only ships a placeholder `version` subcommand. The full subcommand
//! tree (per `design.md` §2) lands in subsequent issues.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, OnceLock};

use clap::{ColorChoice, CommandFactory, FromArgMatches, Parser, Subcommand};
use serde::Serialize;
use tracing::info;
use tracing_appender::non_blocking::{ErrorCounter, NonBlockingBuilder, WorkerGuard};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use crate::error::{CliError, ExitKind};
use crate::output::{self, OutputFormat, OutputSpec};

const GIT_COMMIT: &str = env!("TASKFLEET_GIT_COMMIT");
const CARGO_VERSION: &str = env!("CARGO_PKG_VERSION");
const SUPPORTED_ENVELOPE_SCHEMAS: &[u32] = &[taskfleet_core::SCHEMA_VERSION];

#[derive(Parser, Debug)]
#[command(
    name = "taskfleet",
    version = CARGO_VERSION,
    about = "Orchestrate AI-agent workflows: worktrees, fan-out, orchestrate, llm-skills.",
    disable_help_subcommand = true,
    disable_version_flag = true,
    color = ColorChoice::Never,
)]
struct Cli {
    /// Output format. `jsonl` (default) emits one compact JSON envelope
    /// per line on stdout — AI-first. `json` emits a single pretty
    /// document. `text` emits a human-readable summary. A path-shaped
    /// value (`./out.json`, `./out.jsonl`) routes the machine envelope
    /// to that file (the format is inferred from the extension).
    #[arg(
        long,
        global = true,
        value_name = "FMT|PATH",
        value_parser = parse_output_arg,
    )]
    output: Option<OutputSpec>,

    /// Emit one pretty JSON document to stdout. Shorthand for `--output json` to stdout.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Show binary, commit, and state-schema versions.
    Version,
    /// List, show, or install companion AI-skills shipped with this binary.
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
    /// Create, list, show, wait on, cancel, merge, salvage, or reattach a run.
    Run {
        #[command(subcommand)]
        action: crate::run::RunAction,
    },
    /// Read (`tail`) or append (`create`) events on a run's event log.
    Event {
        #[command(subcommand)]
        action: crate::event::EventAction,
    },
    /// List/show nodes, update advisory telemetry, or submit a terminal report.
    Node {
        #[command(subcommand)]
        action: crate::node::NodeAction,
    },
    /// Long-lived per-run supervisor: tail-follow events, watchdog
    /// agents, consume child `node.report` events with deterministic-
    /// ID dedup. Re-enters the same binary; `run reattach` invokes it.
    Supervise(crate::supervise::SuperviseArgs),
    /// Thin launcher shim: `run-worker <run> <node> -- <cmd> …` wraps an
    /// autonomous worker, waits on it, records its true exit status as a
    /// durable `worker.exited` event, and exits with the worker's own code.
    /// Internal — invoked by the worker-launch path, not by AI callers.
    #[command(name = "run-worker", hide = true)]
    RunWorker(crate::run_worker::RunWorkerArgs),
    /// Private one-shot PID/start-identity publisher invoked only by a generated
    /// worker launcher immediately before it execs the recorded candidate.
    #[command(name = "worker-handshake", hide = true)]
    WorkerHandshake(crate::worker_handshake::WorkerHandshakeArgs),
    /// Read-only self-diagnostic: validate schema, skill-sync, deps,
    /// config, and data integrity. `--fix` applies the safe subset.
    Doctor(crate::doctor::DoctorArgs),
    /// Inspect configuration: print the config file path (`config path`)
    /// or the effective resolved config with per-key source (`config show`).
    Config {
        #[command(subcommand)]
        action: crate::config::ConfigAction,
    },
}

#[derive(Subcommand, Debug)]
enum SkillAction {
    /// List skills embedded in this binary.
    List,
    /// Print a skill's SKILL.md to stdout.
    Show {
        /// Skill name (see `skill list`).
        name: String,
    },
    /// Stream a skill's SKILL.md (frontmatter + body) byte-identically
    /// to stdout. Read-only twin of `install` (AGENTS-AI-FIRST-CLI §16).
    Print {
        /// Skill name (see `skill list`).
        name: String,
    },
    /// Copy a skill's SKILL.md to the agent's skill directory. Installs
    /// every embedded skill when no name is given (per §15).
    Install {
        /// Skill name (see `skill list`). Omit to install every skill.
        name: Option<String>,
        /// Which agent runtime to install for. Defaults to all runtimes.
        #[arg(
            long,
            value_enum,
            default_value_t = SkillAgentArg::All,
            default_value_if("dest", clap::builder::ArgPredicate::IsPresent, "claude")
        )]
        agent: SkillAgentArg,
        /// Override the install base while preserving each agent's native layout.
        #[arg(long, conflicts_with = "dest")]
        target: Option<PathBuf>,
        /// Compatibility override for one exact output file. Requires one skill;
        /// when `--agent` is omitted, the legacy Claude form is retained.
        #[arg(long, conflicts_with = "target")]
        dest: Option<PathBuf>,
        /// Validate and print the complete install plan without writing files.
        #[arg(long)]
        dry_run: bool,
        /// Overwrite existing files at the destination(s).
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum SkillAgentArg {
    Claude,
    Pi,
    Codex,
    All,
}

impl From<SkillAgentArg> for crate::skill::AgentTarget {
    fn from(v: SkillAgentArg) -> Self {
        match v {
            SkillAgentArg::Claude => Self::Claude,
            SkillAgentArg::Pi => Self::Pi,
            SkillAgentArg::Codex => Self::Codex,
            SkillAgentArg::All => Self::All,
        }
    }
}

pub(crate) fn run() -> ExitCode {
    // Capture arguments once and parse them through the one canonical identity.
    let raw_args_os: Vec<OsString> = std::env::args_os().skip(1).collect();
    // Structured-help recognition only compares flag/subcommand spellings. A
    // lossy view is safe there, while clap receives the original OsStrings so
    // path-valued arguments retain arbitrary platform bytes.
    let help_args_are_utf8 = raw_args_os.iter().all(|arg| arg.to_str().is_some());
    let raw_args: Vec<String> = raw_args_os
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let command = command_for();

    // Structured `--help --output json|jsonl` (AGENTS-AI-FIRST-CLI §14):
    // clap's `--help` only renders text, so intercept the request before
    // anything else and project the command surface to JSON instead. A bare
    // `--help` (no explicit `--output`) or `--output text` returns `None`
    // here and falls through to clap's default text help.
    //
    // This runs *before* `init_logging`: structured help is pure metadata
    // that never touches run state, so it must not depend on (or be
    // perturbed by) the log file's writability — keeping the payload
    // deterministic regardless of `$HOME`/`$TASKFLEET_HOME`.
    match crate::help::resolve_help_request(&command, &raw_args) {
        crate::help::HelpRequest::None => {}
        crate::help::HelpRequest::Render { spec, path, depth } => {
            // The help resolver also parses an output-file path. Never use its
            // lossy recognition view to write to a replacement-character path.
            if !help_args_are_utf8 {
                let err = CliError::user(
                    "invalid_arguments",
                    "structured help arguments (including --output paths) must be valid UTF-8",
                );
                err.emit();
                return ExitCode::from(ExitKind::User as u8);
            }
            return emit_json_help(&path, depth, &spec);
        }
        crate::help::HelpRequest::UnknownSubcommand { token } => {
            // §14 tightening: an unknown subcommand under structured help is
            // an error, not a silent fall-back to root help.
            let err = CliError::user(
                "unknown_subcommand",
                format!("unknown subcommand '{token}'"),
            );
            err.emit();
            return ExitCode::from(ExitKind::User as u8);
        }
        crate::help::HelpRequest::InvalidDepth { value } => {
            // Bad `--depth` value under a JSON help request: structured
            // error, not a silent fall-through to the default depth, so
            // an agent learns immediately that its input was wrong
            // (issue: help-json-depth-control).
            let err = CliError::user(
                "invalid_arguments",
                format!("--depth expects a positive integer or 'tree'/'full'; got '{value}'"),
            )
            .with_invalid_value(value);
            err.emit();
            return ExitCode::from(ExitKind::User as u8);
        }
        crate::help::HelpRequest::ConflictingOutputFlags => {
            let err = CliError::user(
                "conflicting_output_flags",
                "--json cannot be used with --output; use at most one output selector",
            );
            err.emit();
            return ExitCode::from(ExitKind::User as u8);
        }
        crate::help::HelpRequest::InvalidOutput => {
            let err = CliError::user(
                "invalid_arguments",
                "--output requires a valid FMT|PATH value",
            );
            err.emit();
            return ExitCode::from(ExitKind::User as u8);
        }
    }

    // Parse before resolving public inputs or opening logs. Besides keeping
    // all help paths filesystem-pure, this ensures malformed invocations do
    // not create state and lets repository-config preflight inspect the
    // selected `run create --source-repo` before the first write.
    let mut matches = match command
        .try_get_matches_from(std::iter::once(OsString::from("taskfleet")).chain(raw_args_os))
    {
        Ok(matches) => matches,
        Err(e) => return handle_clap_error(e, &[]),
    };
    let cli = match Cli::from_arg_matches_mut(&mut matches) {
        Ok(cli) => cli,
        Err(e) => {
            let mut command = command_for();
            return handle_clap_error(e.format(&mut command), &[]);
        }
    };

    // Resolve post-Clap global-flag semantics before any filesystem work.
    let output = match (cli.output.clone(), cli.json) {
        (Some(_), true) => {
            let err = CliError::user(
                "conflicting_output_flags",
                "--json cannot be used with --output; use at most one output selector",
            );
            err.emit();
            return ExitCode::from(ExitKind::User as u8);
        }
        (Some(output), false) => output,
        (None, true) => OutputSpec {
            format: OutputFormat::Json,
            file: None,
        },
        (None, false) => OutputSpec::default(),
    };

    // The private launcher handshake runs before the worker run is published
    // and must not resolve/create the public state root or initialize logging.
    if matches!(&cli.command, Command::WorkerHandshake(_)) {
        let Command::WorkerHandshake(args) = cli.command else {
            unreachable!("variant checked above")
        };
        return match crate::worker_handshake::dispatch(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                error.emit();
                ExitCode::from(error.kind as u8)
            }
        };
    }

    let internal_worker_root = matches!(&cli.command, Command::RunWorker(_))
        .then(|| std::env::var_os("TASKFLEET_INTERNAL_WORKER_STATE_ROOT").map(PathBuf::from))
        .flatten();
    let repository_source = match &cli.command {
        Command::Run {
            action: crate::run::RunAction::Create { source_repo, .. },
        } => crate::home::RepositorySource::RunCreate(source_repo.as_deref()),
        _ => crate::home::RepositorySource::NotApplicable,
    };
    if let Err(err) = crate::home::initialize(internal_worker_root, repository_source) {
        err.emit();
        return ExitCode::from(err.kind as u8);
    }

    // `_log_guard` owns the non-blocking writer's worker thread. Resolution
    // above is deliberately complete before this first filesystem write. A
    // mutating command may establish a fresh selected root here so its first
    // invocation remains logged; read-only commands never do so merely for
    // diagnostics.
    let LoggingInit {
        warnings: logging_warnings,
        guard: _log_guard,
    } = init_logging(command_writes_state(&cli.command));

    info!(
        target: "taskfleet::cli",
        output_format = ?output.format,
        output_file = ?output.file,
        command = ?cli.command,
        "command dispatched"
    );

    let output = &output;
    let result = match cli.command {
        Command::Version => cmd_version(output, &logging_warnings),
        Command::Skill { action } => match action {
            SkillAction::List => crate::skill::cmd_list(output, &logging_warnings),
            SkillAction::Show { name } => crate::skill::cmd_show(&name, output, &logging_warnings),
            SkillAction::Print { name } => {
                crate::skill::cmd_print(&name, output, &logging_warnings)
            }
            SkillAction::Install {
                name,
                agent,
                target,
                dest,
                dry_run,
                force,
            } => crate::skill::cmd_install(
                name.as_deref(),
                agent.into(),
                crate::skill::InstallOptions {
                    target,
                    dest,
                    dry_run,
                    force,
                },
                output,
                &logging_warnings,
            ),
        },
        Command::Run { action } => crate::run::dispatch(action, output, &logging_warnings),
        Command::Event { action } => crate::event::dispatch(action, output, &logging_warnings),
        Command::Node { action } => crate::node::dispatch(action, output, &logging_warnings),
        Command::Supervise(args) => crate::supervise::dispatch(args, output, &logging_warnings),
        Command::RunWorker(args) => crate::run_worker::dispatch(args),
        Command::WorkerHandshake(_) => unreachable!("handshake returns before home resolution"),
        // `doctor` owns its exit code directly: §18 requires exit 1 on any
        // `fail` *without* an error envelope (the report on stdout is the
        // answer), which does not map onto the shared `Result` path below.
        Command::Doctor(args) => return crate::doctor::run(&args, output, &logging_warnings),
        Command::Config { action } => crate::config::dispatch(action, output, &logging_warnings),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            e.emit();
            ExitCode::from(e.kind as u8)
        }
    }
}

fn command_writes_state(command: &Command) -> bool {
    match command {
        Command::Skill { action } => matches!(
            action,
            SkillAction::Install {
                agent: SkillAgentArg::Pi | SkillAgentArg::All,
                target: None,
                dest: None,
                dry_run: false,
                ..
            }
        ),
        Command::Run { action } => match action {
            crate::run::RunAction::Create { dry_run, .. }
            | crate::run::RunAction::Merge { dry_run, .. }
            | crate::run::RunAction::Salvage { dry_run, .. }
            | crate::run::RunAction::Discard { dry_run, .. } => !dry_run,
            crate::run::RunAction::Cancel { .. } | crate::run::RunAction::Reattach { .. } => true,
            crate::run::RunAction::List { .. }
            | crate::run::RunAction::Show { .. }
            | crate::run::RunAction::Wait { .. } => false,
        },
        Command::Event { action } => match action {
            crate::event::EventAction::Create { dry_run, .. } => !dry_run,
            crate::event::EventAction::Tail { .. } => false,
        },
        Command::Node { action } => match action {
            crate::node::NodeAction::Telemetry { .. } => true,
            crate::node::NodeAction::Report { dry_run, .. } => !dry_run,
            crate::node::NodeAction::List { .. } | crate::node::NodeAction::Show { .. } => false,
        },
        Command::Supervise(_) | Command::RunWorker(_) => true,
        Command::Version | Command::Config { .. } | Command::WorkerHandshake(_) => false,
        Command::Doctor(args) => args.fix && !args.dry_run,
    }
}

fn command_for() -> clap::Command {
    Cli::command().name("taskfleet")
}

/// Render the structured help payload for the resolved `subcommand_path`
/// (canonical subcommand names from [`crate::help::resolve_help_request`])
/// and emit it through the standard success envelope. No `warnings` parameter:
/// help renders before logging and is pure command metadata.
fn emit_json_help(
    subcommand_path: &[String],
    depth: crate::help::HelpDepth,
    spec: &OutputSpec,
) -> ExitCode {
    let mut root = command_for();
    // Propagate global args (e.g. `--output`) and the implicit `--help`
    // into every subcommand so each node's flag list is accurate.
    root.build();
    let (target, path) = crate::help::navigate_path(&root, subcommand_path);
    let data = crate::help::build_help(target, &path, depth);
    match output::emit_envelope(&data, spec, &[]) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            e.emit();
            ExitCode::from(e.kind as u8)
        }
    }
}

fn handle_clap_error(e: clap::Error, logging_warnings: &[String]) -> ExitCode {
    use clap::error::ErrorKind;
    // Help is not a failure; let clap print and exit 0. `--version` is
    // disabled at the clap level (`disable_version_flag`), so it never
    // surfaces here — agents must use the `version` subcommand.
    if matches!(
        e.kind(),
        ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    ) {
        let _ = e.print();
        return ExitCode::SUCCESS;
    }

    // Preserve the full clap error context (allowed values, usage hints).
    // §4 of AGENTS-AI-FIRST-CLI requires the expected format to reach the
    // caller; taking only `.lines().next()` strips that. We trim the
    // trailing "For more information, try '--help'." line because it
    // depends on TTY and is noise for the JSON envelope.
    let message = e
        .to_string()
        .lines()
        .filter(|l| !l.trim_start().starts_with("For more information"))
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    let message = if message.is_empty() {
        "invalid arguments".to_string()
    } else {
        message
    };

    let code = match e.kind() {
        ErrorKind::InvalidSubcommand | ErrorKind::UnknownArgument => "unknown_subcommand_or_flag",
        ErrorKind::MissingRequiredArgument | ErrorKind::MissingSubcommand => "missing_argument",
        ErrorKind::InvalidValue => "invalid_value",
        _ => "invalid_arguments",
    };
    let err = CliError {
        kind: ExitKind::User,
        code: code.to_string(),
        message,
        invalid_value: None,
        expected: None,
        details: None,
    };
    err.emit();
    crate::output::emit_text_warnings(logging_warnings);
    ExitCode::from(ExitKind::User as u8)
}

#[derive(Debug, Serialize)]
struct VersionPayload {
    version: &'static str,
    commit: &'static str,
    /// Bundled skill catalog (AGENTS-AI-FIRST-CLI §17). Each entry's
    /// `cli_version` is sourced from the embedded SKILL.md frontmatter,
    /// so an agent can audit "is the skill I loaded matching the binary
    /// I am about to call?" in one call.
    skills: Vec<crate::skill::SkillCatalogEntry>,
    // Duplicated from the success envelope intentionally. §10 of
    // AGENTS-AI-FIRST-CLI requires `version --json` to return
    // `{version, commit, schema_version, supported_schemas}` at the
    // payload level. Agents that unwrap `.data` must still see the
    // contract; omitting this field would make `.data.schema_version`
    // null while `.data.state_schema_version` is present — an
    // asymmetric foot-gun (review history/review-version-subcommand.md
    // §1).
    schema_version: u32,
    /// The envelope schema versions understood by this binary, as required by
    /// §10. The named companion below distinguishes independent schemas that
    /// happen to share a numeric version.
    supported_schemas: &'static [u32],
    /// Independent payload schemas keyed by their public wire surface. This
    /// is additive to `supported_schemas`, whose array shape is already part
    /// of the v1 version-payload API.
    supported_schemas_by_name: SupportedSchemas,
    state_schema_version: u32,
    supported_state_schemas: &'static [u32],
}

#[derive(Debug, Serialize)]
struct SupportedSchemas {
    envelope: &'static [u32],
    state: &'static [u32],
    config: &'static [u32],
    help: &'static [u32],
    skill: &'static [u32],
}

fn cmd_version(spec: &OutputSpec, warnings: &[String]) -> Result<(), CliError> {
    let payload = VersionPayload {
        version: CARGO_VERSION,
        commit: GIT_COMMIT,
        skills: crate::skill::catalog(),
        schema_version: taskfleet_core::SCHEMA_VERSION,
        supported_schemas: SUPPORTED_ENVELOPE_SCHEMAS,
        supported_schemas_by_name: SupportedSchemas {
            envelope: SUPPORTED_ENVELOPE_SCHEMAS,
            state: taskfleet_core::SUPPORTED_STATE_SCHEMAS,
            config: &[crate::config::CONFIG_SCHEMA_VERSION],
            help: &[crate::help::SCHEMA_VERSION_HELP],
            skill: &[crate::skill::SKILL_SCHEMA_VERSION],
        },
        state_schema_version: taskfleet_core::STATE_SCHEMA_VERSION,
        supported_state_schemas: taskfleet_core::SUPPORTED_STATE_SCHEMAS,
    };
    match spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => {
            output::emit_envelope(&payload, spec, warnings)?;
        }
        OutputFormat::Text => {
            println!("taskfleet {}", payload.version);
            println!("commit:                  {}", payload.commit);
            println!("envelope schema:         {}", payload.schema_version);
            println!(
                "supported envelopes:     {}",
                format_u32_list(payload.supported_schemas)
            );
            println!(
                "config schemas:          {}",
                format_u32_list(payload.supported_schemas_by_name.config)
            );
            println!(
                "help schemas:            {}",
                format_u32_list(payload.supported_schemas_by_name.help)
            );
            println!(
                "skill schemas:           {}",
                format_u32_list(payload.supported_schemas_by_name.skill)
            );
            println!("state schema version:    {}", payload.state_schema_version);
            println!(
                "supported state schemas: {}",
                format_u32_list(payload.supported_state_schemas)
            );
            output::emit_text_warnings(warnings);
        }
    }
    Ok(())
}

fn parse_output_arg(s: &str) -> Result<OutputSpec, String> {
    output::parse_output_value(s)
}

fn format_u32_list(values: &[u32]) -> String {
    values
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Buffered-message capacity for the non-blocking log channel. Set
/// explicitly rather than relying on the crate default so the memory
/// ceiling (and the worst-case drain time on shutdown) is a deliberate,
/// visible choice. At the supervisor's 500ms × ~100-node cadence this is
/// far more headroom than a healthy disk needs.
const LOG_BUFFERED_LINES: usize = 128_000;

/// Shared cell holding the non-blocking appender's [`WorkerGuard`]. The
/// guard lives behind `Arc<Mutex<…>>` so the same underlying guard is
/// reachable from two places: the [`LogGuard`] bound in [`run`] (whose
/// `Drop` drains on normal unwinding) and the process-global [`LOG_FLUSH`]
/// cell (drained by [`flush_logs`] on a `process::exit` path that bypasses
/// `Drop`). `None` whenever the subscriber was never installed.
type LogCell = Arc<Mutex<Option<WorkerGuard>>>;

/// Process-global handle to the log [`LogCell`], populated by
/// [`init_logging`]. Lets any subcommand that exits via `std::process::exit`
/// — which skips the [`LogGuard`]'s `Drop` — drain this process's own
/// buffered tracing events to disk first, via [`flush_logs`]. Empty until
/// `init_logging` runs (e.g. the structured-help path returns before it).
static LOG_FLUSH: OnceLock<LogCell> = OnceLock::new();

/// Process-global handle to the non-blocking appender's dropped-event
/// counter. In **lossy** mode (see [`init_logging`]) a sustained burst the
/// disk cannot keep up with overflows the bounded channel and new events —
/// including `error!`/`warn!` — are discarded; this counter records how
/// many. Populated by [`init_logging`] (via [`finish_logging`]) and read by
/// [`dropped_log_events`].
///
/// It is a *clone* of the same `Arc<AtomicUsize>` the live writer increments
/// (tracing-appender's [`NonBlocking::error_counter`] hands out a shared
/// handle), so reads always see the current count without holding the log
/// flush lock. Both readers that need it — the success-envelope warning
/// injected in [`output::emit_envelope`] (rendered *inside* a subcommand)
/// and the supervisor's periodic warn — run where the [`LogGuard`] is not in
/// scope, so a process-global accessor is the only thing that reaches them.
///
/// [`NonBlocking::error_counter`]: tracing_appender::non_blocking::NonBlocking::error_counter
static LOG_DROPPED: OnceLock<ErrorCounter> = OnceLock::new();

/// Number of log events this process has dropped due to lossy back-pressure
/// (bounded-channel overflow). `0` before [`init_logging`] runs, when the
/// subscriber was never installed, or — the steady state — whenever the disk
/// has kept up. Reads the shared counter lock-free. See [`LOG_DROPPED`].
pub(crate) fn dropped_log_events() -> u64 {
    LOG_DROPPED.get().map_or(0, |c| c.dropped_lines() as u64)
}

/// Drain the non-blocking appender's channel by dropping the [`WorkerGuard`].
/// tracing-appender 0.2 exposes no manual flush (`NonBlocking::flush` is a
/// no-op); the worker drains the channel and calls `Write::flush` on the
/// file only when it receives the `Shutdown` message that `WorkerGuard::drop`
/// sends. Note this is a userspace flush, not an `fsync` — records reach the
/// OS/page cache, not guaranteed physical media.
///
/// Dropping the guard blocks until the worker acknowledges, but only up to
/// tracing-appender's built-in shutdown budget (~100ms to enqueue the
/// `Shutdown` + ~1s waiting for the worker). Under a deep backlog on a slow
/// disk that budget can elapse before the channel is fully drained, so this
/// is a best-effort flush, not an unconditional guarantee.
///
/// Idempotent: takes the guard out of the shared cell, so the first caller
/// (whether [`LogGuard::drop`] or [`flush_logs`]) flushes and every later
/// call is a no-op. After the first flush this process's logging is dead —
/// any further events are silently discarded — so only flush right before
/// exit. Poison handling is defensive: the only code holding this lock is
/// `take()`/assignment (neither can panic mid-hold), but if the lock were
/// ever poisoned we still recover the guard rather than strand the flush.
fn drain_cell(cell: &Mutex<Option<WorkerGuard>>) {
    let taken = match cell.lock() {
        Ok(mut g) => g.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    };
    // Drop *after* releasing the lock: `WorkerGuard::drop` blocks (up to the
    // shutdown budget above) waiting for the worker to drain, and there is no
    // reason to hold the mutex across it.
    drop(taken);
}

/// Drain this process's buffered tracing events to disk, then shut logging
/// down. This is **terminal**, not a periodic flush: it drops the
/// `WorkerGuard`, so every `tracing` event emitted afterwards is silently
/// discarded. Call it only as the last step before `std::process::exit`
/// (e.g. `event tail`'s signal exit) — the path that bypasses the
/// [`LogGuard`]'s `Drop`. A no-op if logging was never initialised. See
/// `issues/log-guard-flush-on-process-exit`.
pub(crate) fn flush_logs() {
    if let Some(cell) = LOG_FLUSH.get() {
        drain_cell(cell);
    }
}

/// RAII owner of the non-blocking log writer. Holding it keeps the
/// background writer thread alive; its `Drop` drains the channel to disk on
/// normal stack unwinding (the common exit path). For exits that bypass
/// `Drop` (`std::process::exit`), call [`flush_logs`] explicitly first.
///
/// `#[must_use]`: binding it for the process lifetime is the whole point —
/// dropping it early shuts the writer thread down, after which all further
/// log events are silently discarded.
#[must_use = "the log writer thread is shut down when the guard is dropped — bind it for the process lifetime"]
struct LogGuard {
    cell: LogCell,
}

impl Drop for LogGuard {
    fn drop(&mut self) {
        drain_cell(&self.cell);
    }
}

/// Result of [`init_logging`]: any non-fatal warnings to surface to the
/// caller, plus the [`LogGuard`] whose lifetime keeps the background log
/// writer alive.
#[must_use = "the log writer thread is shut down when the guard is dropped — bind it for the process lifetime"]
struct LoggingInit {
    warnings: Vec<String>,
    guard: LogGuard,
}

/// Build a [`LoggingInit`] from the collected `warnings` and an optional
/// [`WorkerGuard`], storing the guard into the *single* process-global
/// [`LOG_FLUSH`] cell so [`flush_logs`] and the returned [`LogGuard`] always
/// drain the same one. `init_logging`'s every return point funnels through
/// here so the global is always registered (even when no guard exists — then
/// flushing is a harmless no-op).
///
/// There is exactly one cell per process: `get_or_init` creates it on the
/// first call and every later call reuses it. The live guard is installed
/// only into an empty slot — a second `init_logging` (tests, re-entry) keeps
/// the original guard and drops its own. This keeps the global cell and the
/// stack [`LogGuard`] pointing at the same `Option`, which is the whole
/// correctness argument: whichever drains first takes the guard, the other
/// is a no-op. (In the real binary `init_logging` runs once; a second call
/// would in any case get `guard: None`, since `try_init` refuses to install
/// a second subscriber.)
///
/// `dropped` is the live writer's [`ErrorCounter`] on the success path (and
/// `None` on every early-return / re-entry path where no writer was
/// installed). The first non-`None` value wins the set-once [`LOG_DROPPED`]
/// slot, matching the guard's "first init owns the live handle" rule.
fn finish_logging(
    warnings: Vec<String>,
    guard: Option<WorkerGuard>,
    dropped: Option<ErrorCounter>,
) -> LoggingInit {
    if let Some(counter) = dropped {
        // Set-once is correct here, not a race: `dropped` is `Some` only on
        // the path where `try_init` *installed* the subscriber, and `try_init`
        // installs at most one per process. A second `init_logging` (tests,
        // re-entry) fails `try_init`, so it reaches here with `dropped: None`
        // and never contends for the slot. Thus the live counter — the one the
        // installed writer actually increments — always wins. (Mirrors the
        // WorkerGuard "first init owns the live handle" rule below.)
        let _ = LOG_DROPPED.set(counter);
    }
    let cell = LOG_FLUSH.get_or_init(|| Arc::new(Mutex::new(None))).clone();
    {
        let mut slot = match cell.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if slot.is_none() {
            *slot = guard;
        }
        // else: an earlier init already owns the live guard; `guard` (None on
        // the re-entry path) is dropped here.
    }
    LoggingInit {
        warnings,
        guard: LogGuard { cell },
    }
}

/// Test-only writer wrapper that sleeps `delay` before delegating each
/// `write` to `inner`, throttling the non-blocking log worker so buffered
/// events provably linger until an explicit flush. Installed only when
/// `TASKFLEET_TEST_SLOW_LOG_WRITES` is set (see [`slow_log_write_delay`]); never on
/// a normal run.
struct SlowLogWriter<W: std::io::Write> {
    inner: W,
    delay: std::time::Duration,
}

impl<W: std::io::Write> std::io::Write for SlowLogWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        std::thread::sleep(self.delay);
        // `write_all`, not `write`: a short delegated write would let the
        // non-blocking worker emit a truncated JSONL line. Report the full
        // length so the caller never retries (and re-sleeps).
        self.inner.write_all(buf)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Per-write delay for the [`SlowLogWriter`] test hook, parsed from
/// `TASKFLEET_TEST_SLOW_LOG_WRITES` (milliseconds).
///
/// **Debug builds only.** In a release build this always returns `None`, so
/// the env var has zero effect and the plain file is used directly — the hook
/// can never throttle a shipped binary's logging (a reviewer-flagged footgun:
/// a stray exported var would otherwise stall every log write). Integration
/// tests run the debug binary, where the hook is live.
#[cfg(debug_assertions)]
fn slow_log_write_delay() -> Option<std::time::Duration> {
    std::env::var("TASKFLEET_TEST_SLOW_LOG_WRITES")
        .ok()?
        .parse::<u64>()
        .ok()
        .map(std::time::Duration::from_millis)
}

/// Release-build stub: the slow-write test hook is compiled out, so the env
/// var is inert. See the debug variant above.
#[cfg(not(debug_assertions))]
fn slow_log_write_delay() -> Option<std::time::Duration> {
    None
}

/// Initialise the JSONL log subscriber. Logs go to
/// `<resolved Taskfleet home>/logs/taskfleet.log.jsonl`. Best-effort: if the
/// log file cannot be opened, the process still runs and the caller sees
/// the failure in the success-envelope `warnings` array or on stderr.
///
/// Returns the collected warnings plus the [`LogGuard`] owning the
/// non-blocking writer. The guard owns the background writer thread; the
/// caller MUST keep it alive until the process is done logging, otherwise
/// buffered events are dropped on drop. The guard wraps `None` whenever the
/// subscriber was not installed (no log path, IO error, or an
/// already-initialised global subscriber).
///
/// Delivery semantics: the writer runs in **lossy** mode — if the channel
/// fills (a sustained burst the disk cannot keep up with) new events are
/// dropped rather than blocking the caller. This matches the MVP decision
/// to favour supervisor responsiveness over strict log completeness. Drops
/// are no longer silent: the count is surfaced via [`dropped_log_events`] —
/// rendered into the success-envelope `warnings` by [`output::emit_envelope`]
/// and periodically `warn!`-ed by the long-lived supervisor. Logs are still
/// lost on `panic = "abort"`. A `std::process::exit` that bypasses the
/// guard's `Drop` must call [`flush_logs`] first (see `event tail`).
fn init_logging(command_writes_state: bool) -> LoggingInit {
    let mut warnings = Vec::new();
    let log_path = if let Some(p) = log_path() {
        p
    } else {
        warnings.push("log path unavailable: TASKFLEET_HOME and HOME are unset".to_string());
        return finish_logging(warnings, None, None);
    };

    if let Some(parent) = log_path.parent() {
        // Logging must not establish the selected state root for a read-only
        // command. A command that will mutate state may create it here so the
        // first state-establishing invocation retains its diagnostics.
        if command_writes_state {
            if let Err(e) = fs::create_dir_all(parent) {
                warnings.push(format!(
                    "could not create log directory {}: {}",
                    parent.display(),
                    e
                ));
                return finish_logging(warnings, None, None);
            }
        } else {
            let Some(root) = parent.parent() else {
                warnings.push(format!(
                    "could not derive state root from log path {}",
                    log_path.display()
                ));
                return finish_logging(warnings, None, None);
            };
            match fs::metadata(root) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => {
                    warnings.push(format!(
                        "could not create log directory {}: state root {} is not a directory",
                        parent.display(),
                        root.display()
                    ));
                    return finish_logging(warnings, None, None);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return finish_logging(warnings, None, None);
                }
                Err(e) => {
                    warnings.push(format!(
                        "could not inspect state root {} for logging: {}",
                        root.display(),
                        e
                    ));
                    return finish_logging(warnings, None, None);
                }
            }
            if let Err(e) = fs::create_dir(parent) {
                if e.kind() != std::io::ErrorKind::AlreadyExists {
                    warnings.push(format!(
                        "could not create log directory {}: {}",
                        parent.display(),
                        e
                    ));
                    return finish_logging(warnings, None, None);
                }
            }
        }
    }

    let file = match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(f) => f,
        Err(e) => {
            warnings.push(format!(
                "could not open log file {}: {}",
                log_path.display(),
                e
            ));
            return finish_logging(warnings, None, None);
        }
    };

    let filter = match crate::home::log_filter() {
        Ok(Some(value)) => match EnvFilter::try_new(&value) {
            Ok(filter) => filter,
            Err(error) => {
                warnings.push(format!(
                    "invalid {} filter; using info: {error}",
                    crate::home::LOG_ENV
                ));
                EnvFilter::new("info")
            }
        },
        Ok(None) => EnvFilter::new("info"),
        Err(error) => {
            warnings.push(format!(
                "log filter resolution failed; using info: {}",
                error.message
            ));
            EnvFilter::new("info")
        }
    };

    // Hand the file to a background writer thread. The supervisor polls at
    // 500ms across ~100 nodes; doing the `write(2)` synchronously on the
    // tracing call path would serialise that hot loop on disk IO. The
    // single worker also serialises every record this process emits, so
    // no JSONL line is split or interleaved with another from *this*
    // process (cross-process appenders are still only protected at the
    // kernel's per-`write(2)` O_APPEND granularity). `lossy(true)` keeps
    // the tracing call path non-blocking under back-pressure; see the
    // delivery-semantics note above.
    let builder = NonBlockingBuilder::default()
        .lossy(true)
        .buffered_lines_limit(LOG_BUFFERED_LINES);
    // Test-only: `TASKFLEET_TEST_SLOW_LOG_WRITES=<ms>` wraps the file so each
    // `write(2)` sleeps, forcing the background worker to fall behind. That
    // makes the flush-on-exit contract observable end-to-end: events stay
    // buffered (not yet on disk) at exit, so only an explicit drain
    // ([`flush_logs`] / [`LogGuard::drop`]) gets them there. Off (no wrapper,
    // zero overhead) unless the env var is set by a test.
    let (writer, guard) = match slow_log_write_delay() {
        Some(delay) => builder.finish(SlowLogWriter { inner: file, delay }),
        None => builder.finish(file),
    };
    // Capture the dropped-event counter *before* `writer` is moved into the
    // layer below. It is a cheap `Arc` clone sharing the writer's atomic, so
    // it keeps reflecting live drops; registered into `LOG_DROPPED` via
    // `finish_logging` on the success path only.
    let dropped = writer.error_counter();
    let layer = fmt::layer()
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(writer);

    if let Err(e) = tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init()
    {
        // Subscriber already installed (or some other install failure):
        // the layer we just built is unused, so let the guard drop here
        // (flushing and joining the idle worker) rather than handing back
        // a guard for a writer nobody reads.
        warnings.push(format!("tracing subscriber not installed: {e}"));
        return finish_logging(warnings, None, None);
    }

    finish_logging(warnings, Some(guard), Some(dropped))
}

fn log_path() -> Option<PathBuf> {
    crate::home::root_dir()
        .ok()
        .map(|root| root.join("logs").join(crate::home::LOG_FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Duration;

    #[test]
    fn command_has_canonical_identity() {
        let taskfleet = command_for();
        assert_eq!(taskfleet.get_name(), "taskfleet");

        let structured = crate::help::build_help(
            &taskfleet,
            taskfleet.get_name(),
            crate::help::HelpDepth::Bounded(1),
        );
        assert_eq!(structured.command.command, "taskfleet");
        assert!(structured
            .command
            .subcommands
            .iter()
            .map(|entry| serde_json::to_value(entry).unwrap())
            .all(|entry| entry["command"]
                .as_str()
                .is_some_and(|command| command.starts_with("taskfleet "))));
    }

    /// `help::OUTPUT_ARG_ID` keys the structured-help projection's custom
    /// `--output` metadata (accepted values, file-path acceptance). If the
    /// `Cli` field is renamed, the id must move with it — this asserts the
    /// coupling holds against the real command tree.
    #[test]
    fn output_arg_id_matches_real_cli_tree() {
        let cmd = Cli::command();
        let output = cmd
            .get_arguments()
            .find(|a| a.get_id().as_str() == crate::help::OUTPUT_ARG_ID)
            .expect("an arg with the OUTPUT_ARG_ID id exists on the root");
        assert_eq!(output.get_long(), Some("output"));
        assert!(output.is_global_set(), "--output must be global");
    }

    /// Keep the structured-help resolver's id tied to the real global
    /// shorthand: a field/id rename must fail a test rather than quietly
    /// falling back to text help.
    #[test]
    fn json_arg_id_matches_real_cli_tree() {
        let cmd = Cli::command();
        let json = cmd
            .get_arguments()
            .find(|a| a.get_id().as_str() == crate::help::JSON_ARG_ID)
            .expect("an arg with the JSON_ARG_ID id exists on the root");
        assert_eq!(json.get_long(), Some("json"));
        assert!(json.is_global_set(), "--json must be global");
    }

    /// Underlying log writer that sleeps before each `write`, so the
    /// non-blocking worker thread cannot drain instantly. This makes the
    /// "did the flush actually wait for the drain?" question deterministic:
    /// without a flush the buffered line is provably still in flight; with
    /// one it is on disk.
    struct SlowSink {
        out: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for SlowSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            std::thread::sleep(Duration::from_millis(200));
            self.out.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// `drain_cell` must block until the worker has written every buffered
    /// line — this is the whole point of the flush hook on the
    /// `process::exit` path. Regression for
    /// `issues/log-guard-flush-on-process-exit`: with the slow sink, a line
    /// enqueued just before the flush is NOT yet on disk, and only the
    /// blocking drain (dropping the `WorkerGuard`) gets it there.
    #[test]
    fn drain_cell_blocks_until_buffered_line_is_written() {
        let out = Arc::new(Mutex::new(Vec::new()));
        let (mut writer, guard) = NonBlockingBuilder::default()
            .lossy(true)
            .finish(SlowSink { out: out.clone() });

        // Enqueue one line. The worker picks it up but stalls inside the
        // 200ms sleep, so nothing has reached the sink yet.
        writer.write_all(b"buffered-line\n").unwrap();
        assert!(
            out.lock().unwrap().is_empty(),
            "line reached the sink before the flush — the sink wasn't slow enough"
        );

        // Draining drops the guard, which blocks until the worker finishes
        // the in-flight write and drains the channel.
        let cell: LogCell = Arc::new(Mutex::new(Some(guard)));
        drain_cell(&cell);
        assert_eq!(
            &*out.lock().unwrap(),
            b"buffered-line\n",
            "flush did not drain the buffered line to disk"
        );

        // Idempotent: the guard is gone, so a second drain is a harmless
        // no-op (and must not panic on the empty cell).
        drain_cell(&cell);
        assert_eq!(&*out.lock().unwrap(), b"buffered-line\n");
    }

    /// `drain_cell` over a cell that never held a guard (the
    /// logging-uninitialised case) is a no-op, mirroring `flush_logs` when
    /// `LOG_FLUSH` is empty.
    #[test]
    fn drain_cell_on_absent_guard_is_noop() {
        let cell: LogCell = Arc::new(Mutex::new(None));
        drain_cell(&cell); // must not panic
    }

    /// The normal-exit path: dropping a [`LogGuard`] must drain its cell —
    /// this is the RAII guarantee `run()` relies on. Uses the slow sink so
    /// the line is provably still in flight when the guard drops, making the
    /// flush (not luck) responsible for it reaching disk. Guards against a
    /// regression where `LogGuard::drop` stops calling `drain_cell` (e.g. an
    /// extra `Arc` clone keeping the inner `WorkerGuard` alive past drop).
    #[test]
    fn log_guard_drop_drains_buffered_line() {
        let out = Arc::new(Mutex::new(Vec::new()));
        let (mut writer, guard) = NonBlockingBuilder::default()
            .lossy(true)
            .finish(SlowSink { out: out.clone() });
        writer.write_all(b"on-drop-line\n").unwrap();

        let cell: LogCell = Arc::new(Mutex::new(Some(guard)));
        // A second Arc to the same cell (as `LOG_FLUSH` holds in production)
        // must NOT keep the inner guard alive: `LogGuard::drop` takes it out
        // of the `Option` and drops it regardless of the strong count.
        let log_guard = LogGuard { cell: cell.clone() };
        assert!(
            out.lock().unwrap().is_empty(),
            "line reached the sink before drop — the sink wasn't slow enough"
        );

        drop(log_guard);
        assert_eq!(
            &*out.lock().unwrap(),
            b"on-drop-line\n",
            "LogGuard::drop did not drain the buffered line"
        );
        // The surviving Arc's slot is now empty — flushing it is a no-op.
        assert!(cell.lock().unwrap().is_none());
    }

    /// The lossy non-blocking appender MUST drop *and count* events when the
    /// channel saturates under back-pressure. This is the counter-intuitive
    /// guard the issue calls out: if the buffer never overflows in any test,
    /// the dropped-event warning system ([`dropped_log_events`] →
    /// `emit_envelope` / supervisor warn) is untested dead code. Uses the
    /// same `lossy(true)` builder config as [`init_logging`], a tiny channel,
    /// and the `SlowSink` so the single worker thread provably cannot drain
    /// fast enough — then bursts ~10x the buffer and asserts the count rose.
    #[test]
    fn lossy_appender_counts_dropped_events_on_overflow() {
        let out = Arc::new(Mutex::new(Vec::new()));
        let (mut writer, _guard) = NonBlockingBuilder::default()
            .lossy(true)
            .buffered_lines_limit(1)
            .finish(SlowSink { out });
        // `error_counter()` is the exact handle `init_logging` captures and
        // `dropped_log_events()` reads — a clone sharing the writer's atomic.
        let counter = writer.error_counter();
        assert_eq!(counter.dropped_lines(), 0, "no drops before the burst");

        // Tight burst far exceeding the 1-line channel. The worker is stuck
        // in the SlowSink's 200ms sleep on the first line, so the channel
        // fills and every excess line is dropped (lossy mode never blocks
        // the caller). The synchronous burst completes in microseconds, well
        // inside that sleep, so the worker drains nothing more meanwhile.
        for _ in 0..50 {
            let _ = writer.write_all(b"overflow-line\n");
        }

        // Assert a *substantial* fraction dropped, not merely > 0: with a
        // 1-line channel and a stalled worker, ~48 of the 50 must overflow.
        // A loose `> 0` would pass even if the lossy path barely engaged.
        assert!(
            counter.dropped_lines() >= 40,
            "lossy overflow must drop the bulk of the burst, got {}",
            counter.dropped_lines()
        );
    }
}
