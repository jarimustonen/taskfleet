//! `run cancel` — thin wrapper over [`taskfleet_core::cancel_run`] (whole run) and
//! [`taskfleet_core::cancel_node`] (a single live node, `--node <id>`).
//!
//! All the cancel semantics (single-lock transaction, terminal-run refusal,
//! convergent re-cancel, honest node accounting, log-authoritative node
//! resolution) live in core. This file resolves the run, dispatches to the
//! whole-run or per-node path, and translates the core outcome / errors into
//! the CLI's envelope.

use serde::Serialize;
use serde_json::json;

use taskfleet_core::{
    cancel_node, cancel_run, read_manifest_opt, CancelOutcome, NodeCancelOutcome, NodeId,
};

use crate::error::CliError;
use crate::output::{self, OutputFormat, OutputSpec};
use crate::run::{from_core, parse_node_id, run_paths_from_cli_arg, status_kebab};

#[derive(Serialize)]
struct CancelPayload {
    run_id: String,
    cancelled_nodes: Vec<String>,
    nodes_already_terminal: Vec<String>,
    already_cancelled: bool,
}

/// Per-node (`--node`) cancel payload — distinct from the whole-run shape so an
/// AI caller can tell them apart without inferring intent from the flag it sent.
#[derive(Serialize)]
struct NodeCancelPayload {
    run_id: String,
    node: String,
    /// True when this call settled the node terminally cancelled (fresh append
    /// or crash-convergence). False on an idempotent already-terminal no-op.
    cancelled: bool,
    /// True when the node was already terminal on entry — a clean idempotent
    /// no-op, not a fresh cancel.
    already_terminal: bool,
    /// The terminal status the run was rolled up to when this cancel settled the
    /// LAST live node (`cancelled` / `failed`), or `null` when siblings remain
    /// live (the run stays live). Lets an AI caller detect terminalization
    /// without a follow-up `run show`.
    run_rolled_up_to: Option<String>,
}

pub fn run(
    run_id: &str,
    node_arg: Option<&str>,
    note: Option<&str>,
    spec: &OutputSpec,
    warnings: &[String],
) -> Result<(), CliError> {
    // Validate a `--node` argument's format up front (before any lock/side
    // effect), so a malformed id is a loud `invalid_id` rather than a silent
    // no-match. The run's node SET is still resolved log-authoritatively in core.
    let node_id = match node_arg {
        Some(n) => Some(parse_node_id(n)?),
        None => None,
    };

    let root = crate::home::root_dir()?;
    let paths = run_paths_from_cli_arg(&root, run_id)?;
    // `run_id` may have been an unambiguous prefix; from here on report the full
    // resolved id so payloads/messages never echo a partial id back to the user.
    let run_id = paths.run_id.as_str();

    // Friendly `run_not_found` (exit 1) for a missing manifest, BEFORE taking
    // the lock — and, importantly, before core calls `RunLock::acquire`, which
    // would create `<run-dir>/.lock` (and its parent) as a side effect for a
    // bogus run-id. This pre-check is best-effort, not authoritative: if the run
    // is deleted in the window between here and the lock, core's own manifest
    // read fails with a NotFound I/O error, which the matches below map back to
    // the same `run_not_found` envelope so the race can't leak an `io_error`.
    match read_manifest_opt(&paths).map_err(from_core)? {
        None => return Err(run_not_found(run_id)),
        // A run recorded under a removed kind is read-only (ADR §D7) — refuse
        // before core appends its cancel events and rewrites the manifest
        // (which would overwrite the legacy kind with `"unknown"`).
        Some(m) => {
            crate::run::reject_legacy_kind(m.kind, paths.run_id.as_str())?;
            crate::run::require_writer_fence(&m)?;
        }
    }

    if let Some(node_id) = node_id {
        let outcome = match cancel_node(&paths, &node_id, note) {
            Ok(o) => o,
            // A Done/Failed run can't be cancelled node-by-node either — mirror
            // the whole-run refusal so the CLI never claims a transition the
            // reducer would drop.
            Err(taskfleet_core::Error::RunAlreadyTerminal { status }) => {
                let s = status_kebab(status);
                return Err(CliError::user(
                    "run_already_terminal",
                    format!("run is {s}, cannot cancel"),
                )
                .with_invalid_value(s)
                .with_expected(json!(["running", "pending", "blocked"])));
            }
            // The named node isn't in this run's log — a caller error, mapped to
            // the same `node_not_found` code `run merge` uses.
            Err(taskfleet_core::Error::NodeNotFound { node_id }) => {
                return Err(CliError::user(
                    "node_not_found",
                    format!("no node {node_id} in run {run_id}"),
                )
                .with_invalid_value(node_id));
            }
            Err(taskfleet_core::Error::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                return Err(run_not_found(run_id));
            }
            Err(e) => return Err(from_core(e)),
        };
        return emit_node(run_id, &outcome, spec, warnings);
    }

    let outcome = match cancel_run(&paths, note) {
        Ok(o) => o,
        // A Done/Failed run can't be cancelled: refusing here is honest where
        // the old path appended events the reducer then dropped while still
        // printing success.
        Err(taskfleet_core::Error::RunAlreadyTerminal { status }) => {
            let s = status_kebab(status);
            return Err(CliError::user(
                "run_already_terminal",
                format!("run is {s}, cannot cancel"),
            )
            .with_invalid_value(s)
            .with_expected(json!(["running", "pending", "blocked"])));
        }
        // The run vanished between the pre-check and the lock: report it as the
        // same missing-run condition rather than a generic system I/O error.
        Err(taskfleet_core::Error::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            return Err(run_not_found(run_id));
        }
        Err(e) => return Err(from_core(e)),
    };

    emit(run_id, &outcome, spec, warnings)
}

fn run_not_found(run_id: &str) -> CliError {
    CliError::user("run_not_found", format!("no run with id {run_id}")).with_invalid_value(run_id)
}

fn emit_node(
    run_id: &str,
    outcome: &NodeCancelOutcome,
    spec: &OutputSpec,
    warnings: &[String],
) -> Result<(), CliError> {
    let run_rolled_up_to = outcome.rolled_up.map(status_kebab).map(str::to_string);
    let payload = NodeCancelPayload {
        run_id: run_id.to_string(),
        node: outcome.node_id.as_str().to_string(),
        cancelled: outcome.cancelled,
        already_terminal: outcome.already_terminal,
        run_rolled_up_to,
    };
    match spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => {
            output::emit_envelope(&payload, spec, warnings)?;
        }
        OutputFormat::Text => {
            let action = if payload.already_terminal {
                format!(
                    "no-op: node {} of run {} was already terminal",
                    payload.node, payload.run_id,
                )
            } else {
                format!(
                    "cancelled node {} of run {} (branch + worktree preserved)",
                    payload.node, payload.run_id,
                )
            };
            match &payload.run_rolled_up_to {
                // This settled the last live node — the run terminalized here,
                // under the same lock, so the operator isn't left waiting on a
                // rollup that (with a dead supervisor) might never come.
                Some(status) => println!("{action}; run rolled up to {status}"),
                None => println!("{action}; run stays live until all nodes settle"),
            }
            output::emit_text_warnings(warnings);
        }
    }
    Ok(())
}

fn emit(
    run_id: &str,
    outcome: &CancelOutcome,
    spec: &OutputSpec,
    warnings: &[String],
) -> Result<(), CliError> {
    let payload = CancelPayload {
        run_id: run_id.to_string(),
        cancelled_nodes: outcome.nodes_cancelled.iter().map(node_str).collect(),
        nodes_already_terminal: outcome
            .nodes_already_terminal
            .iter()
            .map(node_str)
            .collect(),
        already_cancelled: outcome.run_was_already_cancelled,
    };
    match spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => {
            output::emit_envelope(&payload, spec, warnings)?;
        }
        OutputFormat::Text => {
            let cancelled = payload.cancelled_nodes.len();
            let already = payload.nodes_already_terminal.len();
            if payload.already_cancelled {
                println!(
                    "no-op: run {} was already cancelled, converged {cancelled} additional node(s) ({already} already terminal)",
                    payload.run_id,
                );
            } else {
                println!(
                    "cancelled run {} ({cancelled} node(s) cancelled, {already} already terminal)",
                    payload.run_id,
                );
            }
            output::emit_text_warnings(warnings);
        }
    }
    Ok(())
}

fn node_str(id: &NodeId) -> String {
    id.as_str().to_string()
}
