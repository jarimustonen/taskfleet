//! `node` subcommand — read (`list`, `show`) + agent self-report (`report`).
//!
//! Mirrors the noun-module pattern set by `run/`: one file per verb,
//! shared types here, single `dispatch` entry point called from
//! `cli.rs`. The `report` verb is a domain verb (design.md §2.0, §2.2)
//! — it's the agent's structured-report sink and accepts the §7.3
//! payload schema directly rather than going through `event create`.

pub mod dto;
pub mod list;
pub mod report;
pub mod round_report;
pub mod show;
pub mod telemetry;

use std::path::PathBuf;

use clap::Subcommand;

use crate::error::CliError;
use crate::output::OutputSpec;

#[derive(Subcommand, Debug)]
pub enum NodeAction {
    /// List nodes belonging to a run.
    List {
        run_id: String,
        /// Filter by status (e.g. `running`, `done`).
        #[arg(long)]
        status: Option<String>,
    },
    /// Print one node's JSON projection.
    Show { run_id: String, node_id: String },
    /// Update one bounded, advisory last-told worker telemetry sample.
    Telemetry {
        #[command(subcommand)]
        action: TelemetryAction,
    },
    /// Agent self-submission of a structured terminal report (§7.3).
    Report {
        run_id: String,
        node_id: String,
        /// JSON file containing the §7.3 report payload.
        #[arg(long)]
        from_file: PathBuf,
        /// Dedup token — a repeat call with the same key returns the
        /// existing event's `seq` instead of appending again.
        #[arg(long)]
        idempotency_key: Option<String>,
        /// Validate the payload and exit 0 without writing anything
        /// to the run's events.jsonl or projection files.
        #[arg(long)]
        dry_run: bool,
    },
    /// End a resident worker's round (ADR 0003): record the round summary as a
    /// told `node.round_reported`, then stop. The exit that follows is the end
    /// of the round, `run wait` settles at once, and the next `run message`
    /// resumes the same session in the same worktree. Resident runs only.
    /// Example: taskfleet node round-report <run-id> --summary-file /tmp/round.md
    RoundReport {
        /// Exact run id from the worker's run context.
        run_id: String,
        /// Reporting node id.
        #[arg(long, default_value = "n-0001")]
        node: String,
        /// UTF-8 file with the round summary (at most 16 KiB): what was done,
        /// what is recommended, what is needed next.
        #[arg(long, conflicts_with = "summary")]
        summary_file: Option<PathBuf>,
        /// Inline round summary (alternative to --summary-file).
        #[arg(long)]
        summary: Option<String>,
        /// Validate and report the round number without appending anything.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum TelemetryAction {
    /// Replace the current attempt's advisory telemetry sample.
    Update {
        /// Exact run id. Required unless --input-file is used.
        #[arg(long, required_unless_present = "input_file")]
        run_id: Option<String>,
        /// Exact node id. Required unless --input-file is used.
        #[arg(long, required_unless_present = "input_file")]
        node_id: Option<String>,
        /// Absolute current attempt. Required unless --input-file is used.
        #[arg(long, required_unless_present = "input_file")]
        attempt: Option<u32>,
        /// Last-told activity. Required unless --input-file is used.
        #[arg(long, value_enum, required_unless_present = "input_file")]
        state: Option<telemetry::StateArg>,
        /// Bounded active tool count (1-32), only for `tool_running`.
        #[arg(long, conflicts_with = "input_file")]
        active_tool_count: Option<u8>,
        /// Sanitized tool name, only when active-tool-count is 1.
        #[arg(long, conflicts_with = "input_file")]
        tool_name: Option<String>,
        /// Strict v1 JSON request path, or '-' for stdin. Maximum raw size: 4096 bytes.
        #[arg(
            long,
            value_name = "PATH|-",
            conflicts_with_all = ["run_id", "node_id", "attempt", "state", "active_tool_count", "tool_name"]
        )]
        input_file: Option<PathBuf>,
    },
}

pub fn dispatch(
    action: NodeAction,
    spec: &OutputSpec,
    warnings: &[String],
) -> Result<(), CliError> {
    match action {
        NodeAction::List { run_id, status } => list::run(list::Args {
            run_id,
            status,
            spec,
            warnings,
        }),
        NodeAction::Show { run_id, node_id } => show::run(&run_id, &node_id, spec, warnings),
        NodeAction::Telemetry { action } => match action {
            TelemetryAction::Update {
                run_id,
                node_id,
                attempt,
                state,
                active_tool_count,
                tool_name,
                input_file,
            } => telemetry::update(telemetry::Args {
                run_id,
                node_id,
                attempt,
                state,
                active_tool_count,
                tool_name,
                input_file,
                spec,
                warnings,
            }),
        },
        NodeAction::Report {
            run_id,
            node_id,
            from_file,
            idempotency_key,
            dry_run,
        } => report::run(report::Args {
            run_id,
            node_id,
            from_file,
            idempotency_key,
            dry_run,
            spec,
            warnings,
        }),
        NodeAction::RoundReport {
            run_id,
            node,
            summary_file,
            summary,
            dry_run,
        } => round_report::run(round_report::Args {
            run_id,
            node_id: node,
            summary,
            summary_file,
            dry_run,
            spec,
            warnings,
        }),
    }
}
