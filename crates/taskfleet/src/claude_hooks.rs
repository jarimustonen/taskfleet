//! Invocation-local Claude command hooks: harness facts, never pane inference.
//!
//! The helper is a harness subprocess, so a permission modal does not prevent
//! it running. Commands pin run/node/attempt/session and the re-entry binary;
//! payloads cannot select a run or execute a decision. No stdout is emitted:
//! in particular we never return a Claude permission decision or a blocking code.
//! Awaiting-input ingestion uses the existing event store and grace/notify path.
//! Bare idle notifications are ignored: background work can leave Claude idle
//! without requiring a human decision. No background-state inference is needed.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use clap::Args;
use serde_json::{json, Value};
use taskfleet_core::{
    append_and_apply_unlocked, read_manifest_opt, read_node_opt, NodeId, RunLock, RunPaths,
};

use crate::error::CliError;
use crate::run::from_core;

const SOURCE: &str = "claude-notification-v1";
// Native progress hook JSON includes tool inputs/results, not just metadata.
// Keep ingestion bounded without rejecting ordinary large Read/Write payloads.
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const RECOVERY: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "UserPromptSubmit",
    "Stop",
];

#[derive(Debug, Args)]
pub struct HookArgs {
    #[arg(long)]
    pub state_root: PathBuf,
    #[arg(long)]
    pub run_id: String,
    #[arg(long)]
    pub node_id: String,
    #[arg(long)]
    pub attempt: u32,
    #[arg(long)]
    pub session_id: String,
}

/// Inline settings avoid both staging-directory rename races and writes to
/// Claude's global/project configuration. Shell quoting is shared with launchers.
pub fn settings(
    exe: &Path,
    root: &Path,
    run: &str,
    node: &str,
    attempt: u32,
    session: &str,
) -> Value {
    let command = [
        exe.to_string_lossy().into_owned(),
        "claude-hook".into(),
        "--state-root".into(),
        root.to_string_lossy().into_owned(),
        "--run-id".into(),
        run.into(),
        "--node-id".into(),
        node.into(),
        "--attempt".into(),
        attempt.to_string(),
        "--session-id".into(),
        session.into(),
    ]
    .iter()
    .map(|s| crate::run::spawn::shell_literal(s))
    .collect::<Vec<_>>()
    .join(" ");
    let hook = json!({"type": "command", "command": command, "timeout": 5});
    let mut hooks = serde_json::Map::new();
    hooks.insert(
        "Notification".into(),
        json!([{
            "matcher": "permission_prompt", "hooks": [hook.clone()]
        }]),
    );
    for event in RECOVERY {
        hooks.insert((*event).into(), json!([{"hooks": [hook.clone()]}]));
    }
    json!({"hooks": hooks})
}

#[derive(Debug, PartialEq, Eq)]
enum Signal {
    Permission,
    Progress { turn_boundary: bool },
    Ignore,
}

fn parse(bytes: &[u8], session: &str) -> Result<Signal, CliError> {
    if bytes.len() as u64 > MAX_BYTES {
        // Oversized data is unavailable evidence, not a harness error or a
        // permission decision. Never splash tool-response contents into stderr.
        return Ok(Signal::Ignore);
    }
    let v: Value = serde_json::from_slice(bytes)
        .map_err(|e| CliError::user("claude_hook_invalid", format!("invalid hook JSON: {e}")))?;
    if v.get("session_id").and_then(Value::as_str) != Some(session) || v.get("agent_id").is_some() {
        return Ok(Signal::Ignore);
    }
    let event = v
        .get("hook_event_name")
        .and_then(Value::as_str)
        .ok_or_else(|| CliError::user("claude_hook_invalid", "hook_event_name must be a string"))?;
    Ok(if event == "Notification" {
        match v.get("notification_type").and_then(Value::as_str) {
            Some("permission_prompt") => Signal::Permission,
            _ => Signal::Ignore,
        }
    } else if RECOVERY.contains(&event) {
        Signal::Progress {
            turn_boundary: matches!(event, "UserPromptSubmit" | "Stop"),
        }
    } else {
        Signal::Ignore
    })
}

pub fn dispatch(args: HookArgs) -> Result<(), CliError> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| CliError::system("io_error", format!("read Claude hook: {e}")))?;
    let signal = parse(&bytes, &args.session_id)?;
    ingest(&args, signal)
}

fn ingest(args: &HookArgs, signal: Signal) -> Result<(), CliError> {
    let run = taskfleet_core::RunId::parse_str(&args.run_id)
        .map_err(|e| CliError::user("invalid_run_id", e.to_string()))?;
    let node_id = NodeId::parse_str(&args.node_id)
        .map_err(|e| CliError::user("invalid_node_id", e.to_string()))?;
    if signal == Signal::Ignore {
        return Ok(());
    }
    let paths = RunPaths::new(
        args.state_root.join("runs").join(run.as_str()),
        run.as_str(),
    )
    .map_err(from_core)?;
    // A progress hook can precede publication. No guessed binding or sidecar is
    // created; a later actual notification will report against the live node.
    if !paths.root.is_dir() {
        return Ok(());
    }
    let guard = RunLock::acquire_existing(&paths.lock()).map_err(from_core)?;
    let lock = guard.witness();
    taskfleet_core::replay_unapplied_unlocked(&lock, &paths).map_err(from_core)?;
    let Some(manifest) = read_manifest_opt(&paths).map_err(from_core)? else {
        return Ok(());
    };
    let Some(node) = read_node_opt(&paths, &node_id).map_err(from_core)? else {
        return Ok(());
    };
    if manifest.status.is_terminal()
        || node.status.is_terminal()
        || node.worker_exit.is_some()
        || node.retry_attempts != args.attempt
        || manifest
            .agent_selection
            .as_ref()
            .is_none_or(|s| s.selected.harness != "claude")
    {
        return Ok(());
    }
    if let Some(open) = node.awaiting_input.as_ref() {
        // Never clear a manual request, including one appended to ours. Duplicate
        // notifications also leave the durable first-open timestamp untouched.
        let solely_ours = open.discussion_items.len() == 1
            && open.discussion_items[0]
                .get("source")
                .and_then(Value::as_str)
                == Some(SOURCE)
            && open.discussion_items[0]
                .get("session_id")
                .and_then(Value::as_str)
                == Some(&args.session_id);
        let permission = open
            .discussion_items
            .first()
            .and_then(|item| item.get("notification_type"))
            .and_then(Value::as_str)
            == Some("permission_prompt");
        // Other tools may finish while a parallel tool is still in a modal.
        // Tool progress can clear legacy idle input, but permission needs a turn boundary.
        let resolves = matches!(
            signal,
            Signal::Progress {
                turn_boundary: true
            }
        ) || (!permission && matches!(signal, Signal::Progress { .. }));
        if resolves && solely_ours {
            append_and_apply_unlocked(
                &lock,
                &paths,
                "node.input_resolved",
                Some(&node_id),
                None,
                json!({"event_seq": open.event_seq, "source": SOURCE}),
            )
            .map_err(from_core)?;
        }
        return Ok(());
    }
    let (topic, notification) = match signal {
        Signal::Permission => (
            "Claude reported a permission prompt; inspect the worker before deciding",
            "permission_prompt",
        ),
        Signal::Progress { .. } | Signal::Ignore => return Ok(()),
    };
    append_and_apply_unlocked(
        &lock,
        &paths,
        "node.awaiting_input",
        Some(&node_id),
        None,
        json!({"discussion_items": [{"topic": topic, "options": ["Inspect worker"],
            "recommended_default": "Inspect worker", "source": SOURCE,
            "notification_type": notification, "session_id": args.session_id,
            "attempt": args.attempt}]}),
    )
    .map_err(from_core)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_uses_protocol_not_message_or_pane_text() {
        assert_eq!(parse(br#"{"session_id":"s","hook_event_name":"Notification","notification_type":"permission_prompt"}"#, "s").unwrap(), Signal::Permission);
        assert_eq!(parse(br#"{"session_id":"s","hook_event_name":"Notification","notification_type":"idle_prompt","message":"anything"}"#, "s").unwrap(), Signal::Ignore);
        assert_eq!(
            parse(br#"{"session_id":"s","hook_event_name":"PreToolUse"}"#, "s").unwrap(),
            Signal::Progress {
                turn_boundary: false
            }
        );
        for v in [
            json!({"session_id":"wrong"}),
            json!({"session_id":"s", "agent_id":"sub"}),
            json!({"session_id":"s","hook_event_name":"Notification","notification_type":"other","message":"Do you want to proceed?"}),
        ] {
            assert_eq!(
                parse(&serde_json::to_vec(&v).unwrap(), "s").unwrap(),
                Signal::Ignore
            );
        }
        assert!(parse(b"not json", "s").is_err());
        assert_eq!(
            parse(&vec![b' '; MAX_BYTES as usize + 1], "s").unwrap(),
            Signal::Ignore
        );
        let large = json!({"session_id":"s", "hook_event_name":"PostToolUse", "tool_response":"x".repeat(70 * 1024)});
        assert_eq!(
            parse(&serde_json::to_vec(&large).unwrap(), "s").unwrap(),
            Signal::Progress {
                turn_boundary: false
            }
        );
        assert!(parse(br#"{"session_id":"s","hook_event_name":1}"#, "s").is_err());
    }
}
