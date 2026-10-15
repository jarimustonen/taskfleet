//! Acknowledged, metadata-only worker steering. Only the bundled Pi extension
//! can accept a message; neither a live PID nor tmux send-keys proves receipt.
//! The audit records intent and outcome, not message text. Acceptance is not a
//! read receipt: Pi may still stop before presenting the queued steering turn.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use taskfleet_core::{
    append_and_apply_unlocked, read_manifest_opt, read_node_opt, AgentOwner, RunLock, Status,
};

use crate::error::CliError;
use crate::output::{self, OutputFormat, OutputSpec};
use crate::run::{from_core, parse_node_id, run_paths_from_cli_arg};

const MAX_MESSAGE: usize = 32 * 1024;

#[derive(Serialize)]
struct Receipt<'a> {
    run_id: &'a str,
    node: &'a str,
    message_id: &'a str,
    delivery: &'static str,
    read_or_acted: &'static str,
    attempt_seq: u64,
}

fn check_text(text: &str) -> Result<(), CliError> {
    if text.trim().is_empty() || text.len() > MAX_MESSAGE || text.contains('\0') {
        return Err(CliError::user(
            "invalid_message",
            "message must be nonblank UTF-8, at most 32768 bytes, without NUL",
        ));
    }
    Ok(())
}

/// Strict request/response boundary, injectable without a real harness in tests.
fn exchange(
    stream: &mut UnixStream,
    id: &str,
    session: &str,
    text: &str,
) -> Result<bool, CliError> {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(io)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(io)?;
    let request = json!({"id":id,"session":session,"message":text});
    serde_json::to_writer(&mut *stream, &request)
        .map_err(|e| CliError::system("message_protocol", e.to_string()))?;
    stream.write_all(b"\n").map_err(io)?;
    let mut reply = Vec::new();
    let reader = BufReader::new(stream);
    reader
        .take(4096)
        .read_until(b'\n', &mut reply)
        .map_err(io)?;
    if reply.last() != Some(&b'\n') {
        return Err(CliError::system(
            "message_ack_unknown",
            "adapter did not return a complete acknowledgement; delivery may have happened",
        ));
    }
    let value: Value = serde_json::from_slice(&reply).map_err(|_| {
        CliError::system(
            "message_ack_unknown",
            "invalid adapter acknowledgement; delivery may have happened",
        )
    })?;
    if value.get("id").and_then(Value::as_str) != Some(id)
        || value.get("session").and_then(Value::as_str) != Some(session)
    {
        return Err(CliError::system(
            "message_ack_unknown",
            "adapter identity mismatch; delivery may have happened",
        ));
    }
    value
        .get("accepted")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            CliError::system(
                "message_ack_unknown",
                "adapter acknowledgement lacks accepted flag; delivery may have happened",
            )
        })
}

fn io(error: std::io::Error) -> CliError {
    CliError::system(
        "message_ack_unknown",
        format!("adapter communication failed; delivery may have happened: {error}"),
    )
}

pub fn run(
    run_id: &str,
    node: &str,
    text: Option<&str>,
    body_file: Option<&Path>,
    spec: &OutputSpec,
    warnings: &[String],
) -> Result<(), CliError> {
    let text = match (text, body_file) {
        (Some(text), None) => text.to_owned(),
        (None, Some(path)) => std::fs::read_to_string(path).map_err(|e| {
            CliError::user(
                "invalid_message_file",
                format!("read {}: {e}", path.display()),
            )
        })?,
        _ => {
            return Err(CliError::user(
                "invalid_message",
                "provide text or --body-file",
            ))
        }
    };
    check_text(&text)?;
    let node_id = parse_node_id(node)?;
    let root = crate::home::root_dir()?;
    let paths = run_paths_from_cli_arg(&root, run_id)?;
    if read_manifest_opt(&paths).map_err(from_core)?.is_none() {
        return Err(CliError::user(
            "run_not_found",
            format!("run {} not found", paths.run_id),
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let (session, attempt, seq) = {
        let guard = RunLock::acquire_existing(&paths.lock()).map_err(from_core)?;
        let witness = guard.witness();
        let manifest = read_manifest_opt(&paths)
            .map_err(from_core)?
            .ok_or_else(|| CliError::user("run_not_found", "run disappeared"))?;
        let node_data = read_node_opt(&paths, &node_id)
            .map_err(from_core)?
            .ok_or_else(|| CliError::user("node_not_found", format!("node {node} not found")))?;
        if manifest.agent_owner != AgentOwner::Taskfleet
            || manifest
                .agent_selection
                .as_ref()
                .map(|a| a.selected.harness.as_str())
                != Some("pi")
        {
            return Err(CliError::user("message_unsupported", "only Taskfleet-launched Pi workers with the bundled message adapter support delivery"));
        }
        if manifest.status.is_terminal()
            || node_data.status != Status::Running
            || node_data.worker_exit.is_some()
        {
            return Err(CliError::user(
                "worker_not_active",
                format!("node {node} is not an active worker"),
            ));
        }
        let evidence = node_data.evidence.as_ref().ok_or_else(|| {
            CliError::user(
                "message_unsupported",
                "worker has no native Pi session identity",
            )
        })?;
        let socket = root.join(".creating").join(format!(
            "message-{}-{}.sock",
            paths.run_id, evidence.attempt
        ));
        let extension = root
            .join(".creating")
            .join(format!("message-{}.ts", paths.run_id));
        if !extension.is_file() || !socket.exists() {
            return Err(CliError::user("message_unsupported", "this worker has no reachable acknowledged Pi message adapter (older runs are unsupported)"));
        }
        let seq = append_and_apply_unlocked(&witness, &paths, "worker.message.attempted", Some(&node_id), Some(&id), json!({"message_id":id,"sender":whoami(),"adapter":"pi-extension-v1","bytes":text.len(),"attempt":evidence.attempt})).map_err(from_core)?;
        (evidence.session_id.clone(), evidence.attempt, seq)
    };
    let socket = root
        .join(".creating")
        .join(format!("message-{}-{attempt}.sock", paths.run_id));
    let result = UnixStream::connect(&socket)
        .map_err(io)
        .and_then(|mut stream| exchange(&mut stream, &id, &session, &text));
    // Even a timeout is recorded as unknown, never silently retried: Pi could
    // have accepted it before the connection failed. No sensitive text in events.
    let status = match result {
        Ok(true) => "accepted",
        Ok(false) => "rejected",
        Err(_) => "unknown",
    };
    let guard = RunLock::acquire_existing(&paths.lock()).map_err(from_core)?;
    append_and_apply_unlocked(
        &guard.witness(),
        &paths,
        "worker.message.result",
        Some(&node_id),
        Some(&id),
        json!({"message_id":id,"attempt_seq":seq,"delivery":status,"read_or_acted":"unconfirmed"}),
    )
    .map_err(from_core)?;
    let payload = Receipt {
        run_id: paths.run_id.as_str(),
        node,
        message_id: &id,
        delivery: status,
        read_or_acted: "unconfirmed",
        attempt_seq: seq,
    };
    if let Err(error) = result {
        return Err(
            error.with_details(json!({"message_id":id,"delivery":"unknown","attempt_seq":seq}))
        );
    }
    if status == "rejected" {
        return Err(
            CliError::user("message_rejected", "Pi adapter rejected the message")
                .with_details(json!({"message_id":id,"delivery":status})),
        );
    }
    match spec.format {
        OutputFormat::Text => {
            println!("message {id}: accepted by Pi (read/acted unconfirmed)");
            output::emit_text_warnings(warnings);
            Ok(())
        }
        _ => output::emit_envelope(&payload, spec, warnings),
    }
}

fn whoami() -> String {
    std::env::var("USER").unwrap_or_else(|_| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    #[test]
    fn stub_ack_requires_matching_identity_and_does_not_echo_body() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("socket");
        let listener = UnixListener::bind(path.clone()).unwrap();
        let worker = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&socket).read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["message"], "private instruction");
            (&socket)
                .write_all(b"{\"id\":\"m1\",\"session\":\"s1\",\"accepted\":true}\n")
                .unwrap();
        });
        let mut stream = UnixStream::connect(path).unwrap();
        assert!(exchange(&mut stream, "m1", "s1", "private instruction").unwrap());
        worker.join().unwrap();
        assert!(check_text("  ").is_err());
    }
    #[test]
    fn wrong_ack_is_not_receipt() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        std::thread::spawn(move || {
            let _ = server.write_all(b"{\"id\":\"wrong\",\"session\":\"s\",\"accepted\":true}\n");
        });
        assert!(exchange(&mut client, "m", "s", "text").is_err());
    }
}
