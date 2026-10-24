//! CLI eligibility and attempt-bound acknowledged delivery, without ambient Pi/tmux.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Output};

use serde_json::{json, Value};
use taskfleet_core::{append_and_apply_event, ensure_root, new_run_id, NodeId, RunPaths};
use tempfile::TempDir;

const SESSION: &str = "12345678-1234-1234-1234-123456789abc";

struct Fixture {
    home: TempDir,
    paths: RunPaths,
    node: NodeId,
}

impl Fixture {
    fn new(owner: &str, harness: &str) -> Self {
        let home = TempDir::new().unwrap();
        ensure_root(home.path()).unwrap();
        let id = new_run_id();
        let paths = RunPaths::new(home.path().join("runs").join(&id), &id).unwrap();
        std::fs::create_dir_all(&paths.root).unwrap();
        append_and_apply_event(
            &paths,
            "run.created",
            None,
            None,
            json!({
                "kind":"spinoff", "lifecycle":"autonomous", "title":"message fixture",
                "agent_owner":owner,
                "agent_selection": {
                    "schema_version":1, "profile":"fixture", "selection_source":"cli",
                    "interaction":"autonomous", "capability":"capable", "residency":"local",
                    "selected":{"candidate_index":0,"harness":harness,"command":[harness],
                        "telemetry": if harness == "pi" { Some("worker-v1") } else { None }},
                    "fallback":[]
                }
            }),
        )
        .unwrap();
        let node = NodeId::parse_str("n-0001").unwrap();
        append_and_apply_event(
            &paths,
            "node.created",
            Some(&node),
            None,
            json!({
                "kind":"spinoff", "pi_session_id":SESSION, "pi_session_cwd":home.path(),
                "pi_session_path":format!(".creating/pi-sessions/{id}/pi-session-{SESSION}.jsonl"),
                "attempt":2
            }),
        )
        .unwrap();
        std::fs::create_dir_all(home.path().join(".creating")).unwrap();
        std::fs::write(
            home.path().join(format!(".creating/message-{id}.ts")),
            "fixture",
        )
        .unwrap();
        Self { home, paths, node }
    }

    fn event(&self, kind: &str, data: Value) {
        append_and_apply_event(&self.paths, kind, Some(&self.node), None, data).unwrap();
    }

    fn socket(&self, attempt: u32) -> std::path::PathBuf {
        self.home.path().join(format!(
            ".creating/message-{}-{attempt}.sock",
            self.paths.run_id
        ))
    }

    fn message(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_taskfleet"))
            .env("HOME", self.home.path())
            .env("TASKFLEET_HOME", self.home.path())
            .args([
                "--output",
                "json",
                "run",
                "message",
                self.paths.run_id.as_str(),
                "private steering",
            ])
            .output()
            .unwrap()
    }

    fn refused(&self, code: &str, reason: &str) {
        let out = self.message();
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        let error: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(error["error"]["code"], code, "{error}");
        assert!(
            error["error"]["message"].as_str().unwrap().contains(reason),
            "{error}"
        );
        assert!(!std::fs::read_to_string(self.paths.events())
            .unwrap()
            .contains("worker.message.attempted"));
    }
}

#[test]
fn pending_and_running_workers_reach_matching_acknowledged_adapter() {
    for status in ["pending", "running"] {
        let f = Fixture::new("taskfleet", "pi");
        if status == "running" {
            f.event("node.status", json!({"status":status}));
        }
        let listener = UnixListener::bind(f.socket(2)).unwrap();
        // Bound the fixture too: a regression must fail, never hang on accept.
        listener.set_nonblocking(true).unwrap();
        let worker = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "adapter never contacted"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(e) => panic!("accept: {e}"),
                }
            };
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut line = String::new();
            BufReader::new(&socket).read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["session"], SESSION);
            assert_eq!(request["message"], "private steering");
            writeln!(
                socket,
                "{}",
                json!({"id":request["id"], "session":SESSION, "accepted":true})
            )
            .unwrap();
        });
        let out = f.message();
        worker.join().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let receipt: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(receipt["data"]["delivery"], "accepted");
        assert_eq!(receipt["data"]["read_or_acted"], "unconfirmed");
        let events = std::fs::read_to_string(f.paths.events()).unwrap();
        assert!(events.contains("worker.message.attempted"));
        assert!(events.contains("worker.message.result"));
        assert!(!events.contains("private steering"));
    }
}

#[test]
fn terminal_and_exited_workers_are_refused_before_delivery() {
    for status in ["done", "failed", "cancelled"] {
        let f = Fixture::new("taskfleet", "pi");
        f.event("run.status", json!({"status":status}));
        f.refused("worker_not_active", "terminal");
        let f = Fixture::new("taskfleet", "pi");
        f.event("node.status", json!({"status":status}));
        f.refused("worker_not_active", "terminal");
    }
    let f = Fixture::new("taskfleet", "pi");
    f.event("worker.exited", json!({"exit_code":0}));
    f.refused("worker_not_active", "recorded worker exit");
}

#[test]
fn caller_owned_and_non_pi_workers_remain_unsupported() {
    for (owner, harness) in [("caller", "pi"), ("taskfleet", "claude")] {
        Fixture::new(owner, harness).refused("message_unsupported", "Taskfleet-launched Pi");
    }
}

#[test]
fn missing_and_wrong_attempt_adapters_are_unsupported() {
    let f = Fixture::new("taskfleet", "pi");
    f.refused("message_unsupported", "no reachable acknowledged");
    let _old_attempt = UnixListener::bind(f.socket(1)).unwrap();
    f.refused("message_unsupported", "no reachable acknowledged");
}
