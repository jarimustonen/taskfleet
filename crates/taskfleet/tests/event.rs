//! Integration tests for the `event` subcommand family — `tail` (read)
//! and `create` (sanctioned write path).
//!
//! Each test uses a fresh `TempDir` via `TASKFLEET_HOME`.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;

fn bin(home: &TempDir) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    c.env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path());
    c.env("TASKFLEET_TEST_SKIP_MATERIALIZE", "1");
    c
}

fn run_ok_output(cmd: &mut Command) -> (String, String) {
    let out = cmd.output().expect("spawn");
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    assert!(
        out.status.success(),
        "exit={:?}\nstderr={}\nstdout={}",
        out.status,
        stderr,
        stdout
    );
    (stdout, stderr)
}

fn create_run(home: &TempDir) -> String {
    let out = bin(home)
        .args([
            "--output",
            "json",
            "run",
            "create",
            "--kind",
            "spinoff",
            "--title",
            "tail-test",
        ])
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    v["data"]["run_id"].as_str().expect("run_id").to_string()
}

fn events_path(home: &TempDir, run_id: &str) -> PathBuf {
    home.path().join("runs").join(run_id).join("events.jsonl")
}

fn append_event(path: &Path, run_id: &str, seq: u64, kind: &str) {
    let line = serde_json::json!({
        "ts": "2026-06-12T10:00:00Z",
        "seq": seq,
        "kind": kind,
        "run_id": run_id,
        "node_id": null,
        "idempotency_key": null,
        "data": {"status": "ok"},
    });
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open events.jsonl");
    let mut s = line.to_string();
    s.push('\n');
    f.write_all(s.as_bytes()).expect("write");
    f.sync_all().expect("sync");
}

#[test]
fn tail_reads_existing_events_and_terminal_envelope() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let evp = events_path(&home, &run_id);
    // `run create` already wrote one `run.created` event.
    append_event(&evp, &run_id, 2, "node.created");
    append_event(&evp, &run_id, 3, "node.report");

    let (stdout, _) =
        run_ok_output(bin(&home).args(["--output", "jsonl", "event", "tail", &run_id]));
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 4, "stdout was: {stdout}");
    let last: Value = serde_json::from_str(lines.last().unwrap()).expect("terminal json");
    assert_eq!(last["event"], "result");
    assert_eq!(last["status"], "ok");
    assert_eq!(last["schema_version"], 1);
    assert_eq!(last["last_seq"], 3);
    let first: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["seq"], 1);
    let second: Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(second["kind"], "node.created");
}

#[test]
fn tail_from_seq_skips_earlier_events() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let evp = events_path(&home, &run_id);
    append_event(&evp, &run_id, 2, "a");
    append_event(&evp, &run_id, 3, "b");

    let (stdout, _) = run_ok_output(bin(&home).args([
        "--output",
        "jsonl",
        "event",
        "tail",
        &run_id,
        "--from-seq",
        "3",
    ]));
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "stdout was: {stdout}");
    let only: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(only["seq"], 3);
    let terminal: Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(terminal["event"], "result");
    // Only seq=3 was emitted, but last_seq reflects every event we
    // saw (seq=1, 2, 3) so the dedup state is correct on follow.
    assert_eq!(terminal["last_seq"], 3);
}

#[test]
fn tail_from_seq_zero_includes_seq_one() {
    // Regression: a previous implementation used `saturating_sub(1)` and
    // would have skipped seq=0 events. We don't currently emit seq=0,
    // but verify the default --from-seq=0 includes the first event.
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let (stdout, _) = run_ok_output(bin(&home).args([
        "--output",
        "jsonl",
        "event",
        "tail",
        &run_id,
        "--from-seq",
        "0",
    ]));
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "stdout was: {stdout}");
    let first: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["seq"], 1);
}

#[test]
fn tail_text_format_no_terminal_envelope() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let (stdout, _) =
        run_ok_output(bin(&home).args(["--output", "text", "event", "tail", &run_id]));
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "stdout was: {stdout}");
    assert!(lines[0].contains("run.created"), "line: {}", lines[0]);
    assert!(lines[0].starts_with('['), "line: {}", lines[0]);
}

#[test]
fn tail_jsonl_format_canonical_one_line() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let (stdout, _) =
        run_ok_output(bin(&home).args(["--output", "jsonl", "event", "tail", &run_id]));
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2);
    let ev: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(ev["kind"], "run.created");
}

#[test]
fn tail_rejects_pretty_json_format() {
    // Pretty single-document JSON is not a valid stream — neither one
    // JSON document (open-ended) nor JSONL (one object per line). The
    // streaming verb refuses the format up front per AGENTS §12.
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let out = bin(&home)
        .args(["--output", "json", "event", "tail", &run_id])
        .output()
        .expect("spawn");
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).expect("utf8");
    let last = stderr.lines().last().expect("error envelope");
    let v: Value = serde_json::from_str(last).expect("json");
    assert_eq!(v["error"]["code"], "unsupported_format");
}

#[test]
fn tail_output_file_truncate_without_follow() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let out_path = home.path().join("captured.jsonl");
    std::fs::write(&out_path, b"GARBAGE\n").unwrap();

    let _ = run_ok_output(bin(&home).args([
        "--output",
        "jsonl",
        "event",
        "tail",
        &run_id,
        "--to-file",
        out_path.to_str().unwrap(),
    ]));
    let body = std::fs::read_to_string(&out_path).unwrap();
    assert!(!body.contains("GARBAGE"), "file not truncated: {body}");
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 2);
    let terminal: Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    assert_eq!(terminal["event"], "result");
}

#[test]
fn tail_output_aliasing_events_file_is_rejected() {
    // Without the guard, --output <events.jsonl> would truncate the
    // canonical event log (no-follow) — silent data loss.
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let evp = events_path(&home, &run_id);

    let pre = std::fs::read_to_string(&evp).expect("events file exists");
    assert!(pre.contains("run.created"));

    let out = bin(&home)
        .args([
            "--output",
            "jsonl",
            "event",
            "tail",
            &run_id,
            "--to-file",
            evp.to_str().unwrap(),
        ])
        .output()
        .expect("spawn");
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).expect("utf8");
    let last = stderr.lines().last().expect("error envelope");
    let v: Value = serde_json::from_str(last).expect("json");
    assert_eq!(v["error"]["code"], "invalid_output");

    // The canonical log must still contain the original event.
    let post = std::fs::read_to_string(&evp).expect("events file still readable");
    assert_eq!(pre, post, "events.jsonl was modified despite alias guard");
}

#[test]
fn tail_unknown_run_fails_with_run_not_found() {
    let home = TempDir::new().unwrap();
    let out = bin(&home)
        .args([
            "--output",
            "jsonl",
            "event",
            "tail",
            "01jzabsent0000000000000000",
        ])
        .output()
        .expect("spawn");
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).expect("utf8");
    let last = stderr.lines().last().expect("error envelope");
    let v: Value = serde_json::from_str(last).expect("json");
    assert_eq!(v["error"]["code"], "run_not_found");
}

#[test]
fn tail_invalid_run_id_rejected() {
    let home = TempDir::new().unwrap();
    let out = bin(&home)
        .args(["--output", "jsonl", "event", "tail", "../etc"])
        .output()
        .expect("spawn");
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).expect("utf8");
    let last = stderr.lines().last().expect("error envelope");
    let v: Value = serde_json::from_str(last).expect("json");
    assert_eq!(v["error"]["code"], "invalid_run_id");
}

#[test]
fn tail_partial_line_is_held_until_newline_arrives() {
    // Append the first half of a line (no \n), start tail with --follow,
    // verify it doesn't crash or emit garbage, then complete the line
    // and verify the event is emitted exactly once.
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let evp = events_path(&home, &run_id);

    // Write a partial JSON line.
    let partial = format!(
        "{{\"ts\":\"2026-06-12T10:00:00Z\",\"seq\":2,\"kind\":\"partial.test\",\"run_id\":\"{run_id}\",\"node_id\":null,\"idempotency_key\":null,\"data\":"
    );
    let mut f = OpenOptions::new().append(true).open(&evp).expect("open");
    f.write_all(partial.as_bytes()).unwrap();
    f.sync_all().unwrap();
    drop(f);

    let mut child = bin(&home)
        .args(["--output", "jsonl", "event", "tail", &run_id, "--follow"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn follow");

    let collected = read_stdout_in_background(&mut child);

    // Wait one poll cycle and a bit — the partial must NOT have been emitted.
    thread::sleep(Duration::from_millis(800));

    // Complete the line.
    let completion = "{\"status\":\"ok\"}}\n";
    let mut f = OpenOptions::new().append(true).open(&evp).expect("open");
    f.write_all(completion.as_bytes()).unwrap();
    f.sync_all().unwrap();
    drop(f);

    let saw = wait_for_substring(&collected, "partial.test", Duration::from_secs(2));
    let _ = send_sigint(&child);
    let _ = child.wait();

    let body = collected.lock().unwrap().clone();
    assert!(saw, "partial.test never emitted; stdout was: {body}");
    // Make sure only one occurrence of the completed event.
    assert_eq!(
        body.matches("\"partial.test\"").count(),
        1,
        "stdout: {body}"
    );
}

#[test]
fn tail_follow_picks_up_appended_event_within_one_second() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let evp = events_path(&home, &run_id);

    let mut child = bin(&home)
        .args(["--output", "jsonl", "event", "tail", &run_id, "--follow"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn follow");

    let collected = read_stdout_in_background(&mut child);

    // Wait until the initial drain has emitted run.created so we know
    // the child is in the poll loop (vs racing startup).
    assert!(
        wait_for_substring(&collected, "run.created", Duration::from_secs(2)),
        "initial drain didn't emit"
    );
    append_event(&evp, &run_id, 2, "node.created");
    let saw = wait_for_substring(&collected, "node.created", Duration::from_millis(1500));

    let _ = send_sigint(&child);
    let _ = child.wait();
    assert!(
        saw,
        "follow did not pick up appended event in time; stdout: {}",
        collected.lock().unwrap()
    );
}

#[cfg(unix)]
#[test]
fn tail_follow_emits_cancelled_envelope_on_sigint() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);

    let mut child = bin(&home)
        .args(["--output", "jsonl", "event", "tail", &run_id, "--follow"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn follow");

    let collected = read_stdout_in_background(&mut child);
    assert!(wait_for_substring(
        &collected,
        "run.created",
        Duration::from_secs(2)
    ));

    send_sigint(&child).expect("send SIGINT");
    let status = child.wait().expect("wait");
    assert_eq!(
        status.code(),
        Some(130),
        "expected exit 130, got {:?}; stdout: {}",
        status,
        collected.lock().unwrap()
    );

    let body = collected.lock().unwrap().clone();
    let last_line = body
        .lines()
        .rfind(|l| !l.is_empty())
        .expect("at least one stdout line");
    let v: Value = serde_json::from_str(last_line).expect("terminal json");
    assert_eq!(v["event"], "cancelled");
    assert_eq!(v["schema_version"], 1);
}

#[cfg(unix)]
#[test]
fn tail_signal_exit_flushes_own_diagnostic_logs() {
    // End-to-end coverage for issues/log-guard-flush-on-process-exit:
    // `event tail`'s signal exit goes through `flush_and_exit` ->
    // `std::process::exit`, which bypasses the WorkerGuard's `Drop`. The
    // exit path emits a diagnostic line and calls `flush_logs()` to drain
    // the non-blocking appender to disk first — assert that line actually
    // lands in the real log file via the real binary.
    //
    // This exercises the wiring (flush reachable from tail, log path, line
    // emitted); it is not the deterministic regression guard. The single
    // pre-exit line races with the worker thread, which usually drains it
    // even without the flush. The guaranteed-to-fail-without-the-fix check
    // lives in `cli::tests::drain_cell_blocks_until_buffered_line_is_written`.
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);

    let mut child = bin(&home)
        .args(["--output", "jsonl", "event", "tail", &run_id, "--follow"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn follow");

    let collected = read_stdout_in_background(&mut child);
    // Wait until the initial drain has emitted run.created so the child is
    // in the poll loop (not racing startup) before we signal it.
    assert!(
        wait_for_substring(&collected, "run.created", Duration::from_secs(2)),
        "initial drain didn't emit"
    );

    send_sigint(&child).expect("send SIGINT");
    let status = child.wait().expect("wait");
    assert_eq!(
        status.code(),
        Some(130),
        "expected exit 130, got {status:?}"
    );

    let log_path = home.path().join("logs").join("taskfleet.log.jsonl");
    let contents = std::fs::read_to_string(&log_path)
        .unwrap_or_else(|e| panic!("read log file {}: {e}", log_path.display()));

    // The diagnostic line is emitted immediately before the flush; with the
    // fix it is guaranteed on disk, proving the exit path drains the
    // appender end-to-end.
    let mut saw_exit_line = false;
    let mut saw_dispatch = false;
    for line in contents.lines().filter(|l| !l.trim().is_empty()) {
        let v: Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("non-JSON log line {line:?}: {e}"));
        match v["fields"]["message"].as_str() {
            Some("event tail exiting via signal") => saw_exit_line = true,
            Some("command dispatched") => saw_dispatch = true,
            _ => {}
        }
    }
    assert!(
        saw_dispatch,
        "command-dispatched log line missing — log file:\n{contents}"
    );
    assert!(
        saw_exit_line,
        "tail's pre-exit diagnostic was lost — guard not flushed before process::exit. log file:\n{contents}"
    );
}

#[cfg(unix)]
#[test]
fn tail_follow_detects_log_truncation() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let evp = events_path(&home, &run_id);
    append_event(&evp, &run_id, 2, "x");

    let mut child = bin(&home)
        .args(["--output", "jsonl", "event", "tail", &run_id, "--follow"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn follow");

    let collected = read_stdout_in_background(&mut child);
    assert!(wait_for_substring(
        &collected,
        "\"seq\":2",
        Duration::from_secs(2)
    ));

    // Truncate the file we share with the child's fd. `set_len(0)` on
    // a separately-opened handle still shrinks the inode the child is
    // reading from.
    let f = OpenOptions::new().write(true).open(&evp).expect("open rw");
    f.set_len(0).expect("truncate");
    drop(f);

    let stderr = child.stderr.take().expect("stderr");
    let stderr_str = read_to_string(stderr);
    let status = child.wait().expect("wait");
    assert!(!status.success(), "expected failure exit");
    let last = stderr_str
        .lines()
        .rfind(|l| !l.is_empty())
        .expect("error envelope");
    let v: Value = serde_json::from_str(last).expect("error envelope json");
    assert_eq!(v["error"]["code"], "events_log_truncated");
}

// -- helpers ---------------------------------------------------------------

fn read_stdout_in_background(child: &mut Child) -> std::sync::Arc<std::sync::Mutex<String>> {
    let stdout = child.stdout.take().expect("stdout");
    let collected = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let writer = collected.clone();
    thread::spawn(move || {
        use std::io::Read;
        let mut buf = [0u8; 4096];
        let mut s = stdout;
        while let Ok(n) = s.read(&mut buf) {
            if n == 0 {
                break;
            }
            writer
                .lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    });
    collected
}

fn read_to_string<R: std::io::Read + Send + 'static>(mut r: R) -> String {
    let mut s = String::new();
    let _ = r.read_to_string(&mut s);
    s
}

fn wait_for_substring(
    collected: &std::sync::Arc<std::sync::Mutex<String>>,
    needle: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if collected.lock().unwrap().contains(needle) {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    false
}

#[cfg(unix)]
fn send_sigint(child: &Child) -> std::io::Result<()> {
    let pid = child.id() as i32;
    let rc = unsafe { libc::kill(pid, libc::SIGINT) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn send_sigint(child: &Child) -> std::io::Result<()> {
    // On non-Unix, fall back to kill() (SIGKILL-equivalent). The
    // cancelled-envelope assertion will not hold, so the signal test is
    // gated to `cfg(unix)` above.
    let mut c = unsafe { std::ptr::read(child as *const Child) };
    c.kill()
}

// ---- helpers shared by the `event create` tests below ----

fn run_ok(cmd: &mut Command) -> Value {
    let out = cmd.output().expect("spawn");
    assert!(
        out.status.success(),
        "exit={:?} stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout is valid JSON")
}

fn run_fail(cmd: &mut Command) -> (i32, Value) {
    let out = cmd.output().expect("spawn");
    assert!(!out.status.success(), "expected failure");
    let code = out.status.code().expect("exit code");
    let stderr = String::from_utf8(out.stderr).expect("utf8");
    let last = stderr.lines().last().expect("stderr has at least one line");
    let v: Value = serde_json::from_str(last).expect("error envelope JSON");
    (code, v)
}

fn write_json(home: &TempDir, name: &str, v: Value) -> std::path::PathBuf {
    let p = home.path().join(name);
    std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    p
}

#[test]
fn node_created_then_node_status_updates_projection() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);

    // Append node.created via event create.
    let created_data = write_json(
        &home,
        "node_created.json",
        json!({"kind": "spinoff", "task": "demo"}),
    );
    let v = run_ok(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        created_data.to_str().unwrap(),
    ]));
    assert_eq!(v["data"]["kind"], "node.created");
    assert_eq!(v["data"]["seq"].as_u64().unwrap(), 2); // 1 = run.created

    // nodes/n-0001.json must exist with status pending.
    let node_path = home
        .path()
        .join("runs")
        .join(&run_id)
        .join("nodes")
        .join("n-0001.json");
    let node: Value = serde_json::from_slice(&std::fs::read(&node_path).unwrap()).unwrap();
    assert_eq!(node["status"], "pending");

    // node.status running.
    let status_data = write_json(&home, "node_status.json", json!({"status": "running"}));
    let v = run_ok(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.status",
        "--node-id",
        "n-0001",
        "--from-file",
        status_data.to_str().unwrap(),
    ]));
    assert_eq!(v["data"]["seq"].as_u64().unwrap(), 3);

    let node: Value = serde_json::from_slice(&std::fs::read(&node_path).unwrap()).unwrap();
    assert_eq!(node["status"], "running");
}

#[test]
fn orchestrator_decision_and_discuss_critical_are_appended_and_visible_in_tail() {
    // /orchestrate's decision log (`orchestrator.decision`) and pakkopysäytys
    // (`discuss.critical`) are append-only audit records on the driver run.
    // Both must be accepted by `event create`, appear in `event tail`, and
    // leave projections untouched (they carry no projection of their own).
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home); // seq 1 = run.created

    let decision = write_json(
        &home,
        "decision.json",
        json!({
            "id": "d-001",
            "decision": "merge integration branch eagerly",
            "because": "downstream features block on it",
            "scope": "feature-b, feature-c",
            "reversibility": "medium"
        }),
    );
    let v = run_ok(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "orchestrator.decision",
        "--from-file",
        decision.to_str().unwrap(),
        "--idempotency-key",
        "d-001",
    ]));
    assert_eq!(v["data"]["kind"], "orchestrator.decision");
    assert_eq!(v["data"]["seq"].as_u64().unwrap(), 2);
    // Append-only: the event touches no projection file.
    assert!(
        v["data"]["projections"].as_array().unwrap().is_empty(),
        "audit kind must not project: {}",
        v["data"]
    );

    let discuss = write_json(
        &home,
        "discuss.json",
        json!({
            "summary": "two features need the same schema migration",
            "trigger": "cross_cutting",
            "options": ["serialize", "split the migration"],
            "recommended": "serialize",
            "affected_features": ["feature-b", "feature-c"]
        }),
    );
    let v = run_ok(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "discuss.critical",
        "--from-file",
        discuss.to_str().unwrap(),
        "--idempotency-key",
        "disc-001",
    ]));
    assert_eq!(v["data"]["kind"], "discuss.critical");
    assert_eq!(v["data"]["seq"].as_u64().unwrap(), 3);
    assert_eq!(
        v["data"]["projections"].as_array().unwrap().as_slice(),
        [] as [Value; 0]
    );

    // Both events are durable and visible in `event tail`.
    let (stdout, _) =
        run_ok_output(bin(&home).args(["--output", "jsonl", "event", "tail", &run_id]));
    let lines: Vec<&str> = stdout.lines().collect();
    let kinds: Vec<String> = lines
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| v["kind"].as_str().map(str::to_string))
        .collect();
    assert!(
        kinds.iter().any(|k| k == "orchestrator.decision"),
        "{kinds:?}"
    );
    assert!(kinds.iter().any(|k| k == "discuss.critical"), "{kinds:?}");

    // The run manifest is still `pending` — these audit kinds never roll the
    // run toward a terminal state (only `node.report` does, via the supervisor).
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(home.path().join("runs").join(&run_id).join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["status"], "pending");
}

#[test]
fn unknown_kind_is_rejected_with_expected_list() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let p = write_json(&home, "x.json", json!({}));
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.bogus",
        "--from-file",
        p.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "unknown_event_kind");
    let expected = err["error"]["expected"].as_array().expect("expected list");
    assert!(expected.iter().any(|v| v.as_str() == Some("node.created")));
}

#[test]
fn missing_node_id_for_node_scoped_kind_rejected() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let p = write_json(&home, "ns.json", json!({"status": "running"}));
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.status",
        "--from-file",
        p.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "missing_required_flag");
}

#[test]
fn dry_run_does_not_touch_filesystem() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let events_path = home.path().join("runs").join(&run_id).join("events.jsonl");
    let before = std::fs::read(&events_path).unwrap();

    let p = write_json(&home, "nc.json", json!({"kind": "spinoff"}));
    let v = run_ok(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
        "--dry-run",
    ]));
    assert_eq!(v["data"]["dry_run"], true);
    let projections = v["data"]["projections"].as_array().expect("projections");
    assert!(projections
        .iter()
        .any(|p| p.as_str() == Some("nodes/n-0001.json")));

    let after = std::fs::read(&events_path).unwrap();
    assert_eq!(before, after, "dry-run must not append to events.jsonl");
    assert!(!home
        .path()
        .join("runs")
        .join(&run_id)
        .join("nodes")
        .join("n-0001.json")
        .exists());
}

/// A per-file write fingerprint: `(inode, mtime_secs, mtime_nanos, size)`.
///
/// Inode alone is NOT a reliable write signal under load. An atomic projection
/// write is temp-file + rename, which frees the old file's inode; a busy
/// filesystem (CI's high test parallelism) can immediately RECYCLE that inode
/// number onto the very next file created in the same directory — so a genuine
/// rewrite can land the *same* inode number it had before, and an inode-only
/// diff then misses the write. This was the `dry-run-projection-parity-flake`:
/// a real `manifest.json` rewrite went undetected only under CI parallelism.
///
/// Pairing the inode with the file's mtime (nanosecond) and size defeats reuse:
/// a rewrite always stamps a fresh mtime (the two writes come from separate CLI
/// process invocations, seconds apart — never the same nanosecond), so even a
/// recycled inode reads as a change. An *unchanged* file keeps all four fields,
/// and a byte-identical rewrite (e.g. a manifest timestamp refresh) still shows
/// a new inode/mtime — preserving the "detect every fsync" semantic.
#[cfg(unix)]
type WriteFingerprint = (u64, i64, i64, u64);

/// Snapshot every projection file under a run dir to a run-root-relative
/// `path → fingerprint` map (see [`WriteFingerprint`] for why inode alone is
/// not enough).
#[cfg(unix)]
fn projection_inodes(run_dir: &Path) -> std::collections::BTreeMap<String, WriteFingerprint> {
    use std::os::unix::fs::MetadataExt;
    let mut consider = vec![run_dir.join("manifest.json")];
    for sub in ["nodes"] {
        if let Ok(rd) = std::fs::read_dir(run_dir.join(sub)) {
            for ent in rd.flatten() {
                let p = ent.path();
                if p.extension().and_then(|s| s.to_str()) == Some("json") {
                    consider.push(p);
                }
            }
        }
    }
    let mut map = std::collections::BTreeMap::new();
    for p in consider {
        if let Ok(md) = std::fs::symlink_metadata(&p) {
            if md.file_type().is_file() {
                let rel = p
                    .strip_prefix(run_dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                map.insert(rel, (md.ino(), md.mtime(), md.mtime_nsec(), md.size()));
            }
        }
    }
    map
}

/// The `event create --dry-run` `projections` list must equal the files a real
/// apply of the same event actually fsyncs — the end-to-end parity the
/// `projected-paths-into-reducer` fix guarantees (the CLI now reads the
/// reducer's own plan rather than a hand-maintained mirror).
#[cfg(unix)]
#[test]
fn dry_run_projections_match_real_apply_writes() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let run_dir = home.path().join("runs").join(&run_id);

    // Seed a first node so the run has real projection state.
    let nc = write_json(&home, "nc.json", json!({"kind": "spinoff"}));
    run_ok(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        nc.to_str().unwrap(),
    ]));

    // A second `node.created` plans BOTH the new node projection AND the manifest
    // (node_count is a derived manifest counter), so the plan and the real apply
    // — which also rewrites the manifest to advance the applied-seq watermark —
    // touch the same file set.
    let nc2 = write_json(&home, "nc2.json", json!({"kind": "spinoff"}));

    // 1. Dry-run reports the projections the reducer plans.
    let v = run_ok(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0002",
        "--from-file",
        nc2.to_str().unwrap(),
        "--dry-run",
    ]));
    let mut planned: Vec<String> = v["data"]["projections"]
        .as_array()
        .expect("projections")
        .iter()
        .map(|p| p.as_str().unwrap().to_string())
        .collect();
    planned.sort();
    assert!(!planned.is_empty(), "node.created must plan writes");

    // 2. Real apply — diff projection inodes to learn what was actually written.
    let before = projection_inodes(&run_dir);
    run_ok(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0002",
        "--from-file",
        nc2.to_str().unwrap(),
    ]));
    let after = projection_inodes(&run_dir);
    let mut touched: Vec<String> = after
        .iter()
        .filter(|(rel, ino)| before.get(*rel) != Some(*ino))
        .map(|(rel, _)| rel.clone())
        .collect();
    touched.sort();

    assert_eq!(
        planned, touched,
        "dry-run projections must equal the files a real apply fsyncs"
    );
}

#[test]
fn idempotency_key_returns_existing_seq() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);

    let p = write_json(&home, "nc.json", json!({"kind": "spinoff"}));
    let v1 = run_ok(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
        "--idempotency-key",
        "k1",
    ]));
    let seq1 = v1["data"]["seq"].as_u64().unwrap();

    // Same key with the same payload returns the original seq.
    let v2 = run_ok(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
        "--idempotency-key",
        "k1",
    ]));
    assert_eq!(v2["data"]["seq"].as_u64().unwrap(), seq1);
    assert_eq!(v2["data"]["idempotent_replay"], true);

    // events.jsonl must have exactly one node.created.
    let events =
        std::fs::read_to_string(home.path().join("runs").join(&run_id).join("events.jsonl"))
            .unwrap();
    let count = events
        .lines()
        .filter(|l| l.contains("\"kind\":\"node.created\""))
        .count();
    assert_eq!(count, 1);
}

#[test]
fn idempotency_key_conflict_on_payload_mismatch() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);

    let p = write_json(&home, "nc.json", json!({"kind": "spinoff"}));
    run_ok(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
        "--idempotency-key",
        "k1",
    ]));

    // Same key, different payload → conflict (not a silent replay).
    let p2 = write_json(&home, "nc2.json", json!({"kind": "code"}));
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        p2.to_str().unwrap(),
        "--idempotency-key",
        "k1",
    ]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "idempotency_conflict");
}

#[test]
fn idempotency_scan_over_corrupt_interior_line_is_corrupt_state() {
    // A newline-terminated garbage line in events.jsonl is interior
    // corruption. The `--idempotency-key` dedup scan must surface it as a
    // non-retryable `corrupt_state` user error (exit 1), not silently skip it
    // (which could hide a matching key and double-append) and not collapse it
    // into the generic `io_error` system class (exit 2).
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let evp = events_path(&home, &run_id);

    // Inject a malformed, newline-terminated line after the bootstrap event.
    let mut f = OpenOptions::new().append(true).open(&evp).expect("open");
    f.write_all(b"{not a valid event line\n").unwrap();
    f.sync_all().unwrap();
    drop(f);

    let p = write_json(&home, "nc.json", json!({"kind": "spinoff"}));
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
        "--idempotency-key",
        "k1",
    ]));
    assert_eq!(code, 1, "expected exit 1; envelope: {err}");
    assert_eq!(err["error"]["code"], "corrupt_state");
}

#[test]
fn malformed_state_file_json_is_corrupt_state_not_io_error() {
    // A malformed `manifest.json` on disk is a non-retryable data-integrity
    // fault, not transient I/O: it must surface as the `corrupt_state` user
    // error (exit 1) so an AI caller's retry loop doesn't hammer a file that
    // will never parse — never the generic `io_error` system class (exit 2).
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let manifest = home.path().join("runs").join(&run_id).join("manifest.json");
    std::fs::write(&manifest, b"{ not valid json").unwrap();

    let (code, err) = run_fail(bin(&home).args(["--output", "json", "run", "show", &run_id]));
    assert_eq!(code, 1, "expected exit 1; envelope: {err}");
    assert_eq!(err["error"]["code"], "corrupt_state");
}

#[test]
fn node_heartbeat_kind_is_rejected() {
    // node.heartbeat is design.md §7.5 "future opt-in" and not in the
    // closed MVP set — the reducer has no case for it.
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let p = write_json(&home, "hb.json", json!({}));
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.heartbeat",
        "--node-id",
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "unknown_event_kind");
}

#[test]
fn run_created_kind_is_forbidden_via_event_create() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let p = write_json(
        &home,
        "rc.json",
        json!({"kind": "spinoff", "lifecycle": "autonomous", "title": "x"}),
    );
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "run.created",
        "--from-file",
        p.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "kind_not_routable");
}

#[test]
fn from_file_size_cap_enforced() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    // 2 MiB of JSON whitespace — larger than the 1 MiB cap.
    let big = home.path().join("big.json");
    let mut buf = String::from("{\"pad\":\"");
    buf.push_str(&"a".repeat(2 * 1024 * 1024));
    buf.push_str("\"}");
    std::fs::write(&big, &buf).unwrap();
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        big.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "from_file_too_large");
}

#[test]
fn run_not_found_is_user_error() {
    let home = TempDir::new().unwrap();
    let p = write_json(&home, "x.json", json!({}));
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        "01jzabsent0000000000000000",
        "--kind",
        "node.status",
        "--node-id",
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "run_not_found");
}

#[test]
fn from_file_invalid_json_rejected() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let p = home.path().join("bad.json");
    std::fs::write(&p, b"not json").unwrap();
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "from_file_invalid_json");
}

#[test]
fn node_id_rejected_for_run_status() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let p = write_json(&home, "rs.json", json!({"status": "running"}));
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "run.status",
        "--node-id",
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "unexpected_flag");
}
