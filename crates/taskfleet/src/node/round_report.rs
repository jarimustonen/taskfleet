//! `node round-report` — a resident worker's told end of a round (ADR 0003).
//!
//! Appends `node.round_reported` (round number, the reporting worker attempt,
//! the bounded summary) under the run lock. The supervisor then reads the exit
//! that follows as the end of a round (`waiting`), not as a crash or as
//! attention, and `run wait` settles immediately. For Pi the command also
//! drops the attempt's round marker, which the bundled adapter reads at its
//! next settled boundary to shut Pi down; Claude's `--print` process ends with
//! its turn.
//!
//! A second report from the same attempt amends the open round. A report from
//! an attempt that already exited is refused: only the live worker of the
//! current attempt can end its round.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::json;
use taskfleet_core::reducer::MAX_ROUND_SUMMARY_BYTES;
use taskfleet_core::{append_and_apply_unlocked, read_manifest_opt, read_node_opt, RunLock};

use crate::error::CliError;
use crate::output::{self, OutputFormat, OutputSpec};
use crate::run::{from_core, parse_node_id, run_paths_from_cli_arg};

pub struct Args<'a> {
    pub run_id: String,
    pub node_id: String,
    pub summary: Option<String>,
    pub summary_file: Option<PathBuf>,
    pub dry_run: bool,
    pub spec: &'a OutputSpec,
    pub warnings: &'a [String],
}

#[derive(Serialize)]
struct Payload<'a> {
    run_id: &'a str,
    node_id: &'a str,
    resident_name: &'a str,
    round: u32,
    attempt: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_seq: Option<u64>,
    /// What the worker does next: stop.
    next: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    dry_run: Option<bool>,
}

const NEXT: &str = "stop now: the round is reported and the next message resumes this session";

fn read_summary(text: Option<String>, file: Option<&Path>) -> Result<String, CliError> {
    let summary = match (text, file) {
        (Some(text), None) => text,
        (None, Some(path)) => {
            let f = std::fs::File::open(path).map_err(|e| {
                CliError::user(
                    "invalid_summary_file",
                    format!("open {}: {e}", path.display()),
                )
                .with_invalid_value(path.display().to_string())
            })?;
            let mut bytes = Vec::new();
            f.take(MAX_ROUND_SUMMARY_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| {
                    CliError::user(
                        "invalid_summary_file",
                        format!("read {}: {e}", path.display()),
                    )
                })?;
            String::from_utf8(bytes).map_err(|e| {
                CliError::user(
                    "invalid_summary_file",
                    format!("{} is not UTF-8: {e}", path.display()),
                )
            })?
        }
        _ => {
            return Err(CliError::user(
                "invalid_summary",
                "provide --summary-file <file> or --summary <text>",
            ))
        }
    };
    if summary.trim().is_empty()
        || summary.len() > MAX_ROUND_SUMMARY_BYTES
        || summary.contains('\0')
    {
        return Err(CliError::user(
            "invalid_summary",
            format!(
                "round summary must be nonblank UTF-8, at most {MAX_ROUND_SUMMARY_BYTES} bytes, without NUL"
            ),
        ));
    }
    Ok(summary)
}

pub fn run(args: Args<'_>) -> Result<(), CliError> {
    let summary = read_summary(args.summary, args.summary_file.as_deref())?;
    let node_id = parse_node_id(&args.node_id)?;
    let root = crate::home::root_dir()?;
    let paths = run_paths_from_cli_arg(&root, &args.run_id)?;
    let guard = RunLock::acquire_existing(&paths.lock()).map_err(from_core)?;
    let manifest = read_manifest_opt(&paths)
        .map_err(from_core)?
        .ok_or_else(|| {
            CliError::user("run_not_found", format!("run {} not found", paths.run_id))
        })?;
    let policy = manifest.resident.clone().ok_or_else(|| {
        CliError::user(
            "not_resident",
            format!(
                "run {} is not a resident run; end an ordinary run with `run merge` or a terminal `node report`",
                paths.run_id
            ),
        )
        .with_invalid_value(paths.run_id.to_string())
    })?;
    let node = read_node_opt(&paths, &node_id)
        .map_err(from_core)?
        .ok_or_else(|| {
            CliError::user("node_not_found", format!("node {node_id} not found"))
                .with_invalid_value(node_id.as_str())
        })?;
    if manifest.status.is_terminal() || node.status.is_terminal() {
        return Err(CliError::user(
            "worker_not_active",
            format!("node {node_id} of run {} is terminal", paths.run_id),
        ));
    }
    if node.worker_exit.is_some() {
        return Err(CliError::user(
            "worker_not_active",
            format!(
                "the current worker attempt of node {node_id} already exited; only the live worker ends its round"
            ),
        ));
    }
    let attempt = node.retry_attempts;
    let state = node.resident.as_deref();
    let amending = state
        .and_then(|s| s.open_round())
        .is_some_and(|r| r.attempt == attempt);
    let reported = state.map_or(0, |s| s.rounds_reported);
    let round = if amending { reported } else { reported + 1 };
    let mut event_seq = None;
    if !args.dry_run {
        let seq = append_and_apply_unlocked(
            &guard.witness(),
            &paths,
            "node.round_reported",
            Some(&node_id),
            None,
            json!({"attempt": attempt, "summary": summary}),
        )
        .map_err(from_core)?;
        event_seq = Some(seq);
    }
    drop(guard);
    let mut warnings = args.warnings.to_vec();
    if !args.dry_run {
        // The told fact is the event above. The marker only lets the bundled
        // Pi adapter end the process at its next settled boundary; without it
        // a Pi worker stays up and the parent's next message waits for it.
        let marker = crate::run::resident::round_marker_path(&root, paths.run_id.as_str(), attempt);
        if let Err(e) = std::fs::write(&marker, b"") {
            warnings.push(format!(
                "round recorded, but the Pi stop marker {} could not be written ({e}); exit the worker yourself",
                marker.display()
            ));
        }
    }
    let payload = Payload {
        run_id: paths.run_id.as_str(),
        node_id: node_id.as_str(),
        resident_name: &policy.name,
        round,
        attempt,
        event_seq,
        next: NEXT,
        dry_run: args.dry_run.then_some(true),
    };
    match args.spec.format {
        OutputFormat::Text => {
            println!(
                "round {round} reported for {} ({}); {NEXT}",
                policy.name, paths.run_id
            );
            output::emit_text_warnings(&warnings);
            Ok(())
        }
        _ => output::emit_envelope(&payload, args.spec, &warnings),
    }
}
