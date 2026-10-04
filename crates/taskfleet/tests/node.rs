//! Integration tests for `taskfleet node {list,show,report}`.

use std::path::PathBuf;
use std::process::Command;

use serde_json::{json, Value};
use tempfile::TempDir;

fn bin(home: &TempDir) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    c.env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path());
    c.env("TASKFLEET_TEST_SKIP_MATERIALIZE", "1");
    c
}

fn run_ok(cmd: &mut Command) -> Value {
    let out = cmd.output().expect("spawn");
    assert!(
        out.status.success(),
        "exit={:?} stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout JSON")
}

fn run_fail(cmd: &mut Command) -> (i32, Value) {
    let out = cmd.output().expect("spawn");
    assert!(!out.status.success(), "expected failure");
    let code = out.status.code().expect("exit code");
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    let last = stderr.lines().last().expect("error envelope");
    let v: Value = serde_json::from_str(last).expect("envelope JSON");
    (code, v)
}

fn create_run(home: &TempDir) -> String {
    let v = run_ok(bin(home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "node-test",
    ]));
    v["data"]["run_id"].as_str().unwrap().to_string()
}

fn create_node(home: &TempDir, run_id: &str, node_id: &str) {
    let p = write_json(
        home,
        &format!("nc-{node_id}.json"),
        json!({"kind": "spinoff"}),
    );
    run_ok(bin(home).args([
        "--output",
        "json",
        "event",
        "create",
        run_id,
        "--kind",
        "node.created",
        "--node-id",
        node_id,
        "--from-file",
        p.to_str().unwrap(),
    ]));
}

fn write_json(home: &TempDir, name: &str, v: Value) -> PathBuf {
    let p = home.path().join(name);
    std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    p
}

#[test]
fn list_empty_run_returns_no_nodes() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let v = run_ok(bin(&home).args(["--output", "json", "node", "list", &run_id]));
    assert_eq!(v["data"]["nodes"].as_array().unwrap().len(), 0);
    assert_eq!(v["data"]["run_id"], run_id);
}

#[test]
fn list_returns_created_nodes_sorted() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0002");
    create_node(&home, &run_id, "n-0001");
    let v = run_ok(bin(&home).args(["--output", "json", "node", "list", &run_id]));
    let nodes = v["data"]["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0]["node_id"], "n-0001");
    assert_eq!(nodes[1]["node_id"], "n-0002");
    assert_eq!(nodes[0]["status"], "pending");
}

#[test]
fn list_status_filter() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0001");
    create_node(&home, &run_id, "n-0002");
    // Flip one node to running.
    let p = write_json(&home, "st.json", json!({"status": "running"}));
    run_ok(bin(&home).args([
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
        p.to_str().unwrap(),
    ]));
    let v = run_ok(bin(&home).args([
        "--output", "json", "node", "list", &run_id, "--status", "running",
    ]));
    let nodes = v["data"]["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["node_id"], "n-0001");
}

#[test]
fn list_unknown_run_rejected() {
    let home = TempDir::new().unwrap();
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "node",
        "list",
        "01jzabsent0000000000000000",
    ]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "run_not_found");
}

#[test]
fn list_invalid_run_id_rejected() {
    let home = TempDir::new().unwrap();
    let (code, err) = run_fail(bin(&home).args(["--output", "json", "node", "list", "../etc"]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "invalid_run_id");
}

#[test]
fn show_returns_full_node_json() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0001");
    let v = run_ok(bin(&home).args(["--output", "json", "node", "show", &run_id, "n-0001"]));
    assert_eq!(v["data"]["node_id"], "n-0001");
    assert_eq!(v["data"]["run_id"], run_id);
    assert_eq!(v["data"]["status"], "pending");
    assert_eq!(v["data"]["schema_version"], 1);
}

#[test]
fn show_unknown_node_rejected() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let (code, err) =
        run_fail(bin(&home).args(["--output", "json", "node", "show", &run_id, "n-9999"]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "node_not_found");
}

#[test]
fn report_appends_event_and_updates_node() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0001");
    let p = write_json(
        &home,
        "rep.json",
        json!({
            "success": true,
            "summary": "did the thing",
            "discussion_items": [],
            "spinoff_proposals": [],
            "wrap_up_recommendations": [],
        }),
    );
    let v = run_ok(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        &run_id,
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
    ]));
    assert!(v["data"]["event_seq"].as_u64().unwrap() >= 2);

    // Projection reflects done + last_report.
    let node_path = home
        .path()
        .join("runs")
        .join(&run_id)
        .join("nodes")
        .join("n-0001.json");
    let node: Value = serde_json::from_slice(&std::fs::read(&node_path).unwrap()).unwrap();
    assert_eq!(node["status"], "done");
    assert_eq!(node["last_report"]["summary"], "did the thing");

    let shown = run_ok(bin(&home).args(["--output", "json", "node", "show", &run_id, "n-0001"]));
    assert_eq!(shown["data"]["last_report"]["summary"], "did the thing");
    assert_eq!(shown["data"]["report"], shown["data"]["last_report"]);
}

#[test]
fn report_dry_run_does_not_touch_filesystem() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0001");
    let events_path = home.path().join("runs").join(&run_id).join("events.jsonl");
    let before = std::fs::read(&events_path).unwrap();

    let p = write_json(&home, "rep.json", json!({"success": true}));
    let v = run_ok(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        &run_id,
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
        "--dry-run",
    ]));
    assert_eq!(v["data"]["dry_run"], true);
    assert!(v["data"]["event_seq"].is_null());
    let after = std::fs::read(&events_path).unwrap();
    assert_eq!(before, after, "events.jsonl must be untouched on dry-run");
}

#[test]
fn report_invalid_payload_rejected() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0001");
    let p = write_json(&home, "bad.json", json!({"summary": "no success"}));
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        &run_id,
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "schema_violation");
}

#[test]
fn report_unknown_node_rejected() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    let p = write_json(&home, "rep.json", json!({"success": true}));
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        &run_id,
        "n-9999",
        "--from-file",
        p.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "node_not_found");
}

#[test]
fn report_idempotency_key_returns_existing_seq() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0001");
    let p = write_json(&home, "rep.json", json!({"success": true}));
    let v1 = run_ok(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        &run_id,
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
        "--idempotency-key",
        "k1",
    ]));
    let seq1 = v1["data"]["event_seq"].as_u64().unwrap();

    let v2 = run_ok(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        &run_id,
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
        "--idempotency-key",
        "k1",
    ]));
    assert_eq!(v2["data"]["event_seq"].as_u64().unwrap(), seq1);
    assert_eq!(v2["data"]["idempotent_replay"], true);

    let events =
        std::fs::read_to_string(home.path().join("runs").join(&run_id).join("events.jsonl"))
            .unwrap();
    let count = events
        .lines()
        .filter(|l| l.contains("\"kind\":\"node.report\""))
        .count();
    assert_eq!(count, 1);
}

#[test]
fn report_idempotency_conflict_on_payload_mismatch() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0001");
    let p1 = write_json(&home, "r1.json", json!({"success": true, "summary": "a"}));
    run_ok(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        &run_id,
        "n-0001",
        "--from-file",
        p1.to_str().unwrap(),
        "--idempotency-key",
        "k1",
    ]));

    let p2 = write_json(&home, "r2.json", json!({"success": true, "summary": "b"}));
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        &run_id,
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
fn report_unknown_run_rejected() {
    let home = TempDir::new().unwrap();
    let p = write_json(&home, "rep.json", json!({"success": true}));
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        "01jzabsent0000000000000000",
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "run_not_found");
}

#[test]
fn report_idempotency_conflict_on_node_mismatch() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0001");
    create_node(&home, &run_id, "n-0002");
    let p = write_json(&home, "rep.json", json!({"success": true}));
    run_ok(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        &run_id,
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
        "--idempotency-key",
        "k1",
    ]));
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        &run_id,
        "n-0002",
        "--from-file",
        p.to_str().unwrap(),
        "--idempotency-key",
        "k1",
    ]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "idempotency_conflict");
}

#[test]
fn report_success_false_marks_node_failed() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0001");
    let p = write_json(&home, "rep.json", json!({"success": false}));
    run_ok(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        &run_id,
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
    ]));
    let node: Value = serde_json::from_slice(
        &std::fs::read(
            home.path()
                .join("runs")
                .join(&run_id)
                .join("nodes")
                .join("n-0001.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(node["status"], "failed");
}

#[test]
fn report_from_file_too_large_rejected() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0001");
    let big = home.path().join("big.json");
    let mut s = String::from("{\"success\":true,\"summary\":\"");
    s.push_str(&"a".repeat(2 * 1024 * 1024));
    s.push_str("\"}");
    std::fs::write(&big, &s).unwrap();
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        &run_id,
        "n-0001",
        "--from-file",
        big.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "from_file_too_large");
}

#[test]
fn event_create_cannot_bypass_node_report_validation() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0001");
    let p = write_json(&home, "bad.json", json!({"summary": "no success"}));
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.report",
        "--node-id",
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "kind_not_routable");
}

#[test]
fn list_emits_kebab_case_kind_and_status() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    // Use a multi-word kind variant so kebab-case matters.
    let p = write_json(&home, "tn.json", json!({"kind": "technical-decision"}));
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
    ]));
    let v = run_ok(bin(&home).args(["--output", "json", "node", "list", &run_id]));
    let nodes = v["data"]["nodes"].as_array().unwrap();
    assert_eq!(nodes[0]["kind"], "technical-decision");
    assert_eq!(nodes[0]["status"], "pending");
}

#[test]
fn report_invalid_run_id_rejected() {
    let home = TempDir::new().unwrap();
    let p = write_json(&home, "rep.json", json!({"success": true}));
    let (code, err) = run_fail(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        "../etc",
        "n-0001",
        "--from-file",
        p.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "invalid_run_id");
}

#[test]
fn telemetry_update_flags_are_bounded_and_do_not_change_run_truth() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0001");
    let run_dir = home.path().join("runs").join(&run_id);
    let events_before = std::fs::read(run_dir.join("events.jsonl")).unwrap();
    let manifest_before = std::fs::read(run_dir.join("manifest.json")).unwrap();
    let node_before = std::fs::read(run_dir.join("nodes/n-0001.json")).unwrap();

    let accepted = run_ok(bin(&home).args([
        "--output",
        "json",
        "node",
        "telemetry",
        "update",
        "--run-id",
        &run_id,
        "--node-id",
        "n-0001",
        "--attempt",
        "0",
        "--state",
        "tool_running",
        "--active-tool-count",
        "1",
        "--tool-name",
        "functions.bash",
    ]));
    assert_eq!(accepted["data"]["accepted"], true);
    assert_eq!(accepted["data"]["run_id"], run_id);
    assert_eq!(accepted["data"]["node_id"], "n-0001");
    assert_eq!(accepted["data"]["attempt"], 0);
    assert!(accepted["data"]["received_at"].is_string());
    assert!(accepted["data"]["expires_at"].is_string());

    assert_eq!(
        std::fs::read(run_dir.join("events.jsonl")).unwrap(),
        events_before
    );
    assert_eq!(
        std::fs::read(run_dir.join("manifest.json")).unwrap(),
        manifest_before
    );
    assert_eq!(
        std::fs::read(run_dir.join("nodes/n-0001.json")).unwrap(),
        node_before
    );
}

#[test]
fn telemetry_update_accepts_strict_file_and_stdin_json() {
    use std::io::Write;
    use std::process::Stdio;

    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0001");
    let request = json!({
        "schema_version": 1, "protocol_version": 1,
        "run_id": run_id, "node_id": "n-0001", "attempt": 0,
        "state": "settled"
    });
    let path = write_json(&home, "telemetry.json", request.clone());
    let from_file = run_ok(bin(&home).args([
        "--output",
        "json",
        "node",
        "telemetry",
        "update",
        "--input-file",
        path.to_str().unwrap(),
    ]));
    assert_eq!(from_file["data"]["accepted"], true);

    let mut child = bin(&home)
        .args([
            "--output",
            "json",
            "node",
            "telemetry",
            "update",
            "--input-file",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["data"]["accepted"], true);
}

#[test]
fn telemetry_update_rejects_mixed_unknown_oversize_and_invalid_metadata() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0001");
    let valid = write_json(
        &home,
        "telemetry.json",
        json!({
            "schema_version": 1, "protocol_version": 1,
            "run_id": run_id, "node_id": "n-0001", "attempt": 0,
            "state": "settled"
        }),
    );
    let (code, error) = run_fail(bin(&home).args([
        "--output",
        "json",
        "node",
        "telemetry",
        "update",
        "--input-file",
        valid.to_str().unwrap(),
        "--run-id",
        &run_id,
    ]));
    assert_eq!(code, 1);
    assert_eq!(error["error"]["code"], "invalid_arguments");

    let unknown = write_json(
        &home,
        "unknown.json",
        json!({
            "schema_version": 1, "protocol_version": 1,
            "run_id": run_id, "node_id": "n-0001", "attempt": 0,
            "state": "settled", "progress": 100
        }),
    );
    let (code, error) = run_fail(bin(&home).args([
        "--output",
        "json",
        "node",
        "telemetry",
        "update",
        "--input-file",
        unknown.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert_eq!(error["error"]["code"], "invalid_telemetry_request");

    let oversized = home.path().join("oversized.json");
    std::fs::write(&oversized, vec![b' '; 4097]).unwrap();
    let (code, error) = run_fail(bin(&home).args([
        "--output",
        "json",
        "node",
        "telemetry",
        "update",
        "--input-file",
        oversized.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert_eq!(error["error"]["code"], "telemetry_input_too_large");
    assert_eq!(error["error"]["expected"]["maximum_bytes"], 4096);

    let (code, error) = run_fail(bin(&home).args([
        "--output",
        "json",
        "node",
        "telemetry",
        "update",
        "--run-id",
        &run_id,
        "--node-id",
        "n-0001",
        "--attempt",
        "0",
        "--state",
        "settled",
        "--active-tool-count",
        "1",
    ]));
    assert_eq!(code, 1);
    assert_eq!(error["error"]["code"], "invalid_telemetry_metadata");
}

#[test]
fn run_show_and_list_surface_only_observational_telemetry() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0001");
    create_node(&home, &run_id, "n-0002");
    run_ok(bin(&home).args([
        "--output",
        "json",
        "node",
        "telemetry",
        "update",
        "--run-id",
        &run_id,
        "--node-id",
        "n-0001",
        "--attempt",
        "0",
        "--state",
        "tool_running",
        "--active-tool-count",
        "1",
        "--tool-name",
        "bash",
    ]));

    let shown = run_ok(bin(&home).args(["--output", "json", "run", "show", &run_id]));
    assert_eq!(
        shown["data"]["status"], "pending",
        "telemetry is not run truth"
    );
    assert_eq!(shown["data"]["telemetry_available"], true);
    let rows = shown["data"]["telemetry"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["node_id"], "n-0001");
    assert_eq!(rows[0]["sample"], "current");
    assert_eq!(rows[0]["state"], "tool_running");
    assert_eq!(rows[0]["attempt"], 0);
    assert_eq!(rows[0]["active_tool_count"], 1);
    assert_eq!(rows[0]["tool_name"], "bash");
    assert_eq!(rows[1]["node_id"], "n-0002");
    assert_eq!(rows[1]["sample"], "absent");
    assert!(rows[1].get("state").is_none());
    assert_eq!(shown["data"]["telemetry_counts"]["current"], 1);
    assert_eq!(shown["data"]["telemetry_counts"]["absent"], 1);
    assert_eq!(rows[0]["requirement"], "required");
    assert_eq!(rows[0]["support"], "configured");
    assert_eq!(rows[1]["requirement"], "required");
    assert_eq!(rows[1]["support"], "configured");

    let listed = run_ok(bin(&home).args(["--output", "json", "run", "list"]));
    let row = listed["data"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["run_id"] == run_id)
        .unwrap();
    assert_eq!(row["status"], "pending");
    assert_eq!(row["telemetry_available"], true);
    assert_eq!(row["telemetry_counts"]["current"], 1);
    assert_eq!(row["telemetry_counts"]["absent"], 1);
    assert!(row.get("telemetry").is_none(), "run list stays bounded");

    let text = bin(&home)
        .args(["--output", "text", "run", "show", &run_id])
        .output()
        .unwrap();
    assert!(text.status.success());
    let stdout = String::from_utf8(text.stdout).unwrap();
    assert!(stdout.contains("last told activity: tool_running"));
    assert!(stdout.contains("run status unchanged"));
    for forbidden in ["healthy", "making progress", "wedged", "stuck detection"] {
        assert!(
            !stdout.contains(forbidden),
            "forbidden inference wording: {stdout}"
        );
    }
}

#[test]
fn run_views_distinguish_freshness_corruption_and_old_attempts() {
    use chrono::{Duration, SecondsFormat, Utc};

    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0001");
    run_ok(bin(&home).args([
        "--output",
        "json",
        "node",
        "telemetry",
        "update",
        "--run-id",
        &run_id,
        "--node-id",
        "n-0001",
        "--attempt",
        "0",
        "--state",
        "settled",
    ]));
    let run_dir = home.path().join("runs").join(&run_id);
    let sample_path = run_dir.join("telemetry/n-0001.json");
    let valid: Value = serde_json::from_slice(&std::fs::read(&sample_path).unwrap()).unwrap();
    let sample_status = || {
        run_ok(bin(&home).args(["--output", "json", "run", "show", &run_id]))["data"]["telemetry"]
            [0]["sample"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(sample_status(), "current");

    let now = Utc::now();
    let stamp = |time: chrono::DateTime<Utc>| time.to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut stale = valid.clone();
    stale["state_since"] = json!(stamp(now - Duration::seconds(120)));
    stale["received_at"] = json!(stamp(now - Duration::seconds(100)));
    stale["expires_at"] = json!(stamp(now - Duration::seconds(10)));
    std::fs::write(&sample_path, serde_json::to_vec(&stale).unwrap()).unwrap();
    assert_eq!(sample_status(), "stale");

    let mut future = valid.clone();
    future["state_since"] = json!(stamp(now + Duration::seconds(10)));
    future["received_at"] = json!(stamp(now + Duration::seconds(10)));
    future["expires_at"] = json!(stamp(now + Duration::seconds(100)));
    std::fs::write(&sample_path, serde_json::to_vec(&future).unwrap()).unwrap();
    assert_eq!(sample_status(), "clock_unreliable");

    std::fs::write(&sample_path, b"{invalid").unwrap();
    assert_eq!(sample_status(), "invalid");

    std::fs::write(&sample_path, serde_json::to_vec(&valid).unwrap()).unwrap();
    let node_path = run_dir.join("nodes/n-0001.json");
    let mut node: Value = serde_json::from_slice(&std::fs::read(&node_path).unwrap()).unwrap();
    node["retry_attempts"] = json!(1);
    std::fs::write(&node_path, serde_json::to_vec(&node).unwrap()).unwrap();
    let shown = run_ok(bin(&home).args(["--output", "json", "run", "show", &run_id]));
    let old = &shown["data"]["telemetry"][0];
    assert_eq!(old["sample"], "absent");
    assert!(
        old.get("state").is_none(),
        "old attempt activity must be hidden: {old}"
    );
    assert!(
        old.get("attempt").is_none(),
        "old attempt number must be hidden: {old}"
    );
    assert_eq!(shown["data"]["telemetry_counts"]["absent"], 1);

    let listed = run_ok(bin(&home).args(["--output", "json", "run", "list"]));
    assert_eq!(listed["data"]["runs"][0]["telemetry_counts"]["absent"], 1);
}

#[test]
fn telemetry_update_pins_states_versions_attempt_and_terminal_errors() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0001");
    for state in ["agent_active", "settled", "shutdown"] {
        let value = run_ok(bin(&home).args([
            "--output",
            "json",
            "node",
            "telemetry",
            "update",
            "--run-id",
            &run_id,
            "--node-id",
            "n-0001",
            "--attempt",
            "0",
            "--state",
            state,
        ]));
        assert_eq!(value["data"]["accepted"], true, "state {state}");
    }

    let (code, error) = run_fail(bin(&home).args([
        "--output",
        "json",
        "node",
        "telemetry",
        "update",
        "--run-id",
        &run_id,
        "--node-id",
        "n-0001",
        "--attempt",
        "1",
        "--state",
        "settled",
    ]));
    assert_eq!(code, 1);
    assert_eq!(error["error"]["code"], "telemetry_attempt_mismatch");
    assert_eq!(error["error"]["expected"], 0);

    let (code, error) = run_fail(bin(&home).args([
        "--output",
        "json",
        "node",
        "telemetry",
        "update",
        "--run-id",
        &run_id,
        "--node-id",
        "n-9999",
        "--attempt",
        "0",
        "--state",
        "settled",
    ]));
    assert_eq!(code, 1);
    assert_eq!(error["error"]["code"], "node_not_found");

    for (field, code) in [
        ("schema_version", "unsupported_telemetry_schema"),
        ("protocol_version", "unsupported_telemetry_protocol"),
    ] {
        let mut request = json!({
            "schema_version": 1, "protocol_version": 1,
            "run_id": run_id, "node_id": "n-0001", "attempt": 0, "state": "settled"
        });
        request[field] = json!(2);
        let path = write_json(&home, &format!("bad-{field}.json"), request);
        let (exit, error) = run_fail(bin(&home).args([
            "--output",
            "json",
            "node",
            "telemetry",
            "update",
            "--input-file",
            path.to_str().unwrap(),
        ]));
        assert_eq!(exit, 1);
        assert_eq!(error["error"]["code"], code);
    }

    let report = write_json(&home, "terminal.json", json!({"success": true}));
    run_ok(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        &run_id,
        "n-0001",
        "--from-file",
        report.to_str().unwrap(),
    ]));
    let (code, error) = run_fail(bin(&home).args([
        "--output",
        "json",
        "node",
        "telemetry",
        "update",
        "--run-id",
        &run_id,
        "--node-id",
        "n-0001",
        "--attempt",
        "0",
        "--state",
        "shutdown",
    ]));
    assert_eq!(code, 1);
    assert_eq!(error["error"]["code"], "telemetry_node_terminal");
}

#[test]
fn telemetry_input_size_boundary_and_missing_file_are_stable() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    create_node(&home, &run_id, "n-0001");
    let mut exact = serde_json::to_vec(&json!({
        "schema_version": 1, "protocol_version": 1,
        "run_id": run_id, "node_id": "n-0001", "attempt": 0, "state": "settled"
    }))
    .unwrap();
    exact.resize(4096, b' ');
    let exact_path = home.path().join("exact.json");
    std::fs::write(&exact_path, &exact).unwrap();
    assert_eq!(
        run_ok(bin(&home).args([
            "--output",
            "json",
            "node",
            "telemetry",
            "update",
            "--input-file",
            exact_path.to_str().unwrap(),
        ]))["data"]["accepted"],
        true
    );

    exact.push(b' ');
    std::fs::write(&exact_path, &exact).unwrap();
    let (code, error) = run_fail(bin(&home).args([
        "--output",
        "json",
        "node",
        "telemetry",
        "update",
        "--input-file",
        exact_path.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert_eq!(error["error"]["code"], "telemetry_input_too_large");

    let missing = home.path().join("missing.json");
    let (code, error) = run_fail(bin(&home).args([
        "--output",
        "json",
        "node",
        "telemetry",
        "update",
        "--input-file",
        missing.to_str().unwrap(),
    ]));
    assert_eq!(code, 1);
    assert_eq!(error["error"]["code"], "telemetry_input_unreadable");
}

#[test]
fn advisory_read_failure_does_not_hide_canonical_run_rows() {
    let home = TempDir::new().unwrap();
    let broken = create_run(&home);
    create_node(&home, &broken, "n-0001");
    run_ok(bin(&home).args([
        "--output",
        "json",
        "node",
        "telemetry",
        "update",
        "--run-id",
        &broken,
        "--node-id",
        "n-0001",
        "--attempt",
        "0",
        "--state",
        "settled",
    ]));
    let sample = home
        .path()
        .join("runs")
        .join(&broken)
        .join("telemetry/n-0001.json");
    std::fs::remove_file(&sample).unwrap();
    std::fs::create_dir(&sample).unwrap();

    let other = create_run(&home);
    create_node(&home, &other, "n-0001");
    let listed = run_ok(bin(&home).args(["--output", "json", "run", "list"]));
    let rows = listed["data"]["runs"].as_array().unwrap();
    assert!(rows.iter().any(|row| row["run_id"] == broken));
    assert!(rows.iter().any(|row| row["run_id"] == other));
    let broken_row = rows.iter().find(|row| row["run_id"] == broken).unwrap();
    assert_eq!(broken_row["status"], "pending");
    assert_eq!(broken_row["telemetry_available"], false);
    assert_eq!(broken_row["telemetry_counts"]["invalid"], 0);
    assert!(listed["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| {
            warning.as_str().is_some_and(|text| {
                text.contains("telemetry unavailable") && text.contains(&broken)
            })
        }));

    let shown = run_ok(bin(&home).args(["--output", "json", "run", "show", &broken]));
    assert_eq!(shown["data"]["status"], "pending");
    assert_eq!(shown["data"]["telemetry_available"], false);
    assert_eq!(shown["data"]["telemetry"].as_array().unwrap().len(), 0);
    assert!(shown["warnings"][0]
        .as_str()
        .unwrap()
        .contains("run status unchanged"));
}
