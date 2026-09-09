//! Integration tests for `taskfleet run merge` (issue
//! `bundle-worktree-merge`). The merge backend is stubbed via `TASKFLEET_MERGE_SH`
//! so the tests exercise taskfleet's integration — node resolution,
//! source resolution, terminal-report submission, failure handling — without
//! a real git worktree, workmux, or tmux.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

#[cfg(target_os = "linux")]
use serde_json::json;
use serde_json::Value;
#[cfg(target_os = "linux")]
use taskfleet_core::{append_and_apply_event, NodeId, RunPaths};
use tempfile::TempDir;

mod common;
use common::TestHome;

fn bin(home: &TempDir) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    c.env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path());
    c.env("TASKFLEET_TEST_SKIP_MATERIALIZE", "1");
    c.env("TMUX_BIN", "/usr/bin/true");
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
    serde_json::from_slice(&out.stdout).expect("stdout is valid JSON")
}

fn create_run(home: &TempDir, kind: &str, title: &str) -> String {
    run_ok(bin(home).args([
        "--output", "json", "run", "create", "--kind", kind, "--title", title,
    ]))["data"]["run_id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn run_dir(home: &TempDir, run_id: &str) -> std::path::PathBuf {
    home.path().join("runs").join(run_id)
}

/// Forge a `node.created` for `n-0001` carrying a real (existing) worktree
/// path + branch so `run merge` can `cd` into it and resolve the branch.
fn forge_worker_node(home: &TempDir, run_id: &str, kind: &str, worktree: &Path, branch: &str) {
    let node = home.path().join(format!("node-{run_id}.json"));
    std::fs::write(
        &node,
        format!(
            r#"{{"kind":"{kind}","task":"x","worktree_path":"{}","branch":"{branch}","tmux_session":"taskfleet","tmux_window_id":"@42"}}"#,
            worktree.display()
        ),
    )
    .unwrap();
    run_ok(bin(home).args([
        "--output",
        "json",
        "event",
        "create",
        run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        node.to_str().unwrap(),
    ]));
}

/// Write an executable fake merge backend that records its argv (one line) to
/// `<dir>/merge.log` and exits `code`.
fn fake_merge_sh(dir: &Path, code: i32, stderr: &str) -> std::path::PathBuf {
    let p = dir.join("fake-merge.sh");
    let log = dir.join("merge.log");
    let body = format!(
        "#!/bin/bash\nprintf '%s ' \"$@\" >> '{}'\nprintf '\\n' >> '{}'\n{}\nexit {code}\n",
        log.display(),
        log.display(),
        if stderr.is_empty() {
            String::new()
        } else {
            format!("echo '{stderr}' >&2")
        },
    );
    std::fs::write(&p, body).unwrap();
    let mut perms = std::fs::metadata(&p).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&p, perms).unwrap();
    p
}

fn read_events(events: &Path) -> Vec<Value> {
    std::fs::read_to_string(events)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .collect()
}

fn node_reports(events: &Path) -> Vec<Value> {
    read_events(events)
        .into_iter()
        .filter(|v| v["kind"] == "node.report")
        .collect()
}

/// A clean merge: the backend exits 0, and `run merge` appends a terminal
/// `node.report` carrying `via: "explicit-merge"`.
#[test]
fn successful_merge_submits_explicit_merge_report() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let worktree = TempDir::new().unwrap();
    let run_id = create_run(&home, "spinoff", "merge-ok");
    forge_worker_node(&home, &run_id, "spinoff", worktree.path(), "wt/test-x");

    let merge_sh = fake_merge_sh(scratch.path(), 0, "");
    let v = run_ok(bin(&home).env("TASKFLEET_MERGE_SH", &merge_sh).args([
        "--output", "json", "run", "merge", &run_id, "--source", "main",
    ]));
    assert_eq!(v["data"]["merged"], true);
    assert_eq!(v["data"]["branch"], "wt/test-x");
    assert_eq!(v["data"]["source"], "main");

    // The backend was invoked with the resolved target and branch.
    let argv = std::fs::read_to_string(scratch.path().join("merge.log")).unwrap();
    assert!(
        argv.contains("--target main") && argv.contains("wt/test-x"),
        "merge backend argv was {argv:?}"
    );

    // Exactly one terminal report, stamped with the explicit-merge marker.
    let events = run_dir(&home, &run_id).join("events.jsonl");
    let reports = node_reports(&events);
    assert_eq!(reports.len(), 1, "expected one terminal node.report");
    assert_eq!(reports[0]["data"]["success"], true);
    assert_eq!(reports[0]["data"]["via"], "explicit-merge");
}

/// `--report-file` carries a rich §7.3 payload (`discussion_items`,
/// `spinoff_proposals`) so an autonomous kind merges AND delivers its
/// structured report in one call. `run merge` stamps `via: "explicit-merge"`
/// and submits the agent's payload verbatim otherwise.
#[test]
fn report_file_payload_is_submitted_with_marker() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let worktree = TempDir::new().unwrap();
    let run_id = create_run(&home, "research", "merge-rich");
    forge_worker_node(&home, &run_id, "research", worktree.path(), "wt/test-x");

    let report = scratch.path().join("report.json");
    std::fs::write(
        &report,
        r#"{
            "success": true,
            "summary": "research delivered",
            "discussion_items": [{"topic": "scope creep", "severity": "discuss"}],
            "spinoff_proposals": [{"proposed_title": "follow-up", "proposed_kind": "research"}],
            "wrap_up_recommendations": ["read sources/"]
        }"#,
    )
    .unwrap();

    let merge_sh = fake_merge_sh(scratch.path(), 0, "");
    run_ok(bin(&home).env("TASKFLEET_MERGE_SH", &merge_sh).args([
        "--output",
        "json",
        "run",
        "merge",
        &run_id,
        "--report-file",
        report.to_str().unwrap(),
    ]));

    let events = run_dir(&home, &run_id).join("events.jsonl");
    let reports = node_reports(&events);
    assert_eq!(reports.len(), 1);
    let data = &reports[0]["data"];
    assert_eq!(data["via"], "explicit-merge");
    assert_eq!(data["summary"], "research delivered");
    assert_eq!(data["discussion_items"][0]["topic"], "scope creep");
    assert_eq!(data["spinoff_proposals"][0]["proposed_title"], "follow-up");
    assert_eq!(data["wrap_up_recommendations"][0], "read sources/");
}

/// A `--report-file` whose ADVISORY section has a field-name typo
/// (`title`/`detail` instead of `proposed_title`/`proposed_kind`) no longer
/// blocks the merge (issue `merge-report-schema-lenience`). The clean, committed
/// code merges; the malformed advisory proposal is dropped and surfaced as a
/// machine-readable warning. This is the exact glasspad-stint foot-gun.
#[test]
fn typoed_advisory_field_merges_with_warning() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let worktree = TempDir::new().unwrap();
    let run_id = create_run(&home, "spinoff", "merge-lenient");
    forge_worker_node(&home, &run_id, "spinoff", worktree.path(), "wt/test-x");

    let report = scratch.path().join("report.json");
    std::fs::write(
        &report,
        r#"{
            "success": true,
            "summary": "green, reviewed, committed",
            "spinoff_proposals": [
                {"proposed_title": "real follow-up", "proposed_kind": "spinoff"},
                {"title": "typoed follow-up", "detail": "wrong field names"}
            ],
            "wrap_up_recommendations": ["rebase", 42]
        }"#,
    )
    .unwrap();

    let merge_sh = fake_merge_sh(scratch.path(), 0, "");
    let v = run_ok(bin(&home).env("TASKFLEET_MERGE_SH", &merge_sh).args([
        "--output",
        "json",
        "run",
        "merge",
        &run_id,
        "--report-file",
        report.to_str().unwrap(),
    ]));

    // The merge landed (the whole point of the fix).
    assert_eq!(v["data"]["merged"], true);

    // The two malformed advisory entries are reported structurally.
    let adv = v["data"]["report_advisory_warnings"]
        .as_array()
        .expect("report_advisory_warnings present");
    assert_eq!(adv.len(), 2, "one bad proposal + one bad wrap-up element");
    let fields: Vec<&str> = adv.iter().map(|w| w["field"].as_str().unwrap()).collect();
    assert!(fields.contains(&"spinoff_proposals"));
    assert!(fields.contains(&"wrap_up_recommendations"));

    // And human-readable warnings ride the envelope too.
    let warns = v["warnings"].as_array().expect("warnings array");
    assert!(
        warns.iter().any(|w| w
            .as_str()
            .is_some_and(|s| s.contains("dropped spinoff_proposals[1]"))),
        "expected a dropped-proposal warning: {warns:?}"
    );

    // The persisted report keeps the VALID proposal and drops the typoed one.
    let events = run_dir(&home, &run_id).join("events.jsonl");
    let reports = node_reports(&events);
    assert_eq!(reports.len(), 1);
    let data = &reports[0]["data"];
    assert_eq!(data["via"], "explicit-merge");
    let kept = data["spinoff_proposals"].as_array().unwrap();
    assert_eq!(kept.len(), 1, "only the well-formed proposal survives");
    assert_eq!(kept[0]["proposed_title"], "real follow-up");
    assert_eq!(
        data["wrap_up_recommendations"],
        serde_json::json!(["rebase"])
    );
    // The merge backend DID run — leniency lets the merge proceed.
    assert!(scratch.path().join("merge.log").exists());
}

/// A `--report-file` that contradicts the merge (`success: false` or
/// `cancelled: true`) is rejected BEFORE the merge runs. A clean merge is a
/// success; such a report — stamped explicit-merge — would either mis-terminalize
/// a live node or fail the reducer's confirmed-merge adoption gate and strand
/// teardown (4-model review of `reducer-adopt-explicit-merge`).
#[test]
fn non_success_report_file_is_rejected() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let worktree = TempDir::new().unwrap();

    for body in [
        r#"{"success": false, "summary": "blocked"}"#,
        r#"{"success": true, "cancelled": true, "summary": "cancelled"}"#,
    ] {
        let run_id = create_run(&home, "spinoff", "reject-nonsuccess");
        forge_worker_node(&home, &run_id, "spinoff", worktree.path(), "wt/foo");
        let report = scratch.path().join("bad-report.json");
        std::fs::write(&report, body).unwrap();
        let merge_sh = fake_merge_sh(scratch.path(), 0, "");
        let out = bin(&home)
            .env("TASKFLEET_MERGE_SH", &merge_sh)
            .args([
                "--output",
                "json",
                "run",
                "merge",
                &run_id,
                "--source",
                "main",
                // Confirm the interactive merge so the report-shape gate — not
                // the interactive-confirmation gate — is what rejects the body.
                "--report-file",
                report.to_str().unwrap(),
            ])
            .output()
            .expect("spawn");
        assert!(!out.status.success(), "must reject: {body}");
        let err: Value = serde_json::from_slice(&out.stderr).expect("stderr is JSON envelope");
        assert_eq!(err["error"]["code"], "invalid_merge_report", "body: {body}");
        // The merge backend must NOT have run (rejection is pre-merge).
        assert!(
            !scratch.path().join("merge.log").exists(),
            "merge backend must not run when the report is rejected: {body}"
        );
        // No terminal report was appended.
        let events = run_dir(&home, &run_id).join("events.jsonl");
        assert_eq!(node_reports(&events).len(), 0, "no report appended: {body}");
    }
}

/// A malformed `--report-file` is rejected BEFORE the merge runs — the backend
/// is never invoked and no event is appended.
#[test]
fn bad_report_file_rejected_before_merge() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let worktree = TempDir::new().unwrap();
    let run_id = create_run(&home, "spinoff", "merge-badreport");
    forge_worker_node(&home, &run_id, "spinoff", worktree.path(), "wt/test-x");

    // Missing the required `success` field.
    let report = scratch.path().join("bad.json");
    std::fs::write(&report, r#"{"summary": "no success field"}"#).unwrap();

    let merge_sh = fake_merge_sh(scratch.path(), 0, "");
    let out = bin(&home)
        .env("TASKFLEET_MERGE_SH", &merge_sh)
        .args([
            "--output",
            "json",
            "run",
            "merge",
            &run_id,
            "--report-file",
            report.to_str().unwrap(),
        ])
        .output()
        .expect("spawn");
    assert!(!out.status.success());
    let err: Value = serde_json::from_slice(&out.stderr).expect("stderr JSON");
    assert_eq!(err["error"]["code"], "schema_violation");
    // The structured `expected` hint survives the merge boundary (parity with
    // `node report`) — a malformed REQUIRED field stays strict AND self-describing.
    assert_eq!(
        err["error"]["expected"],
        serde_json::json!({"field": "success", "type": "boolean"})
    );
    assert!(
        !scratch.path().join("merge.log").exists(),
        "merge must not run when the report file is invalid"
    );
    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert_eq!(node_reports(&events).len(), 0);
}

/// A merge failure (conflict / dirty tree / lock timeout): the backend exits
/// non-zero, `run merge` surfaces `merge_failed`, and NO terminal report is
/// appended — the node stays live for the agent to recover and retry.
#[test]
fn failed_merge_surfaces_error_and_writes_no_report() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let worktree = TempDir::new().unwrap();
    let run_id = create_run(&home, "spinoff", "merge-fail");
    forge_worker_node(&home, &run_id, "spinoff", worktree.path(), "wt/test-x");

    let merge_sh = fake_merge_sh(scratch.path(), 1, "Error: rebase conflict");
    let out = bin(&home)
        .env("TASKFLEET_MERGE_SH", &merge_sh)
        .args(["--output", "json", "run", "merge", &run_id])
        .output()
        .expect("spawn");
    assert!(!out.status.success(), "merge failure must exit non-zero");
    let err: Value = serde_json::from_slice(&out.stderr).expect("stderr is JSON envelope");
    assert_eq!(err["error"]["code"], "merge_failed");

    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert_eq!(
        node_reports(&events).len(),
        0,
        "a failed merge must not submit a terminal report"
    );
}

/// `--dry-run` resolves inputs and reports the planned merge without invoking
/// the backend or appending any event — a read-only preview with no merge and
/// no report.
#[test]
fn dry_run_resolves_without_side_effects() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let worktree = TempDir::new().unwrap();
    let run_id = create_run(&home, "spinoff", "merge-dry");
    forge_worker_node(&home, &run_id, "spinoff", worktree.path(), "wt/test-x");

    let merge_sh = fake_merge_sh(scratch.path(), 1, "should never run");
    let v = run_ok(bin(&home).env("TASKFLEET_MERGE_SH", &merge_sh).args([
        "--output",
        "json",
        "run",
        "merge",
        &run_id,
        "--dry-run",
    ]));
    assert_eq!(v["data"]["dry_run"], true);
    assert_eq!(v["data"]["branch"], "wt/test-x");

    // The backend was never invoked and no report was written.
    assert!(
        !scratch.path().join("merge.log").exists(),
        "dry-run must not invoke the merge backend"
    );
    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert_eq!(node_reports(&events).len(), 0);
}

/// Roll the run's manifest to a terminal `status` by appending a `run.status`
/// event — the supervisor's own rollup, driven directly so a test needn't spawn
/// a real supervisor.
fn set_run_status(home: &TempDir, run_id: &str, scratch: &Path, status: &str) {
    let f = scratch.join(format!("run-status-{status}.json"));
    std::fs::write(&f, format!(r#"{{"status":"{status}"}}"#)).unwrap();
    run_ok(bin(home).args([
        "--output",
        "json",
        "event",
        "create",
        run_id,
        "--kind",
        "run.status",
        "--from-file",
        f.to_str().unwrap(),
    ]));
}

/// Assert `run merge` fails with `run_already_terminal` and never spawned the
/// merge backend a second time. `expected_backend_lines` is how many argv lines
/// the shared `merge.log` should hold (the count from any earlier merges).
fn assert_refused_terminal(
    out: std::process::Output,
    scratch: &Path,
    expected_backend_lines: usize,
) {
    assert!(!out.status.success(), "the merge must be refused");
    let err: Value = serde_json::from_slice(&out.stderr).expect("stderr is JSON envelope");
    assert_eq!(
        err["error"]["code"], "run_already_terminal",
        "a terminal run must surface run_already_terminal, not merge_spawn_failed: {err}"
    );
    let log = scratch.join("merge.log");
    let lines = std::fs::read_to_string(&log).map_or(0, |s| s.lines().count());
    assert_eq!(
        lines, expected_backend_lines,
        "the refused merge must NOT invoke the merge backend"
    );
}

/// Re-merging an already-finished run fails with the clear `run_already_terminal`
/// error, NOT the misleading `merge_spawn_failed` (issue
/// `merge-terminal-misleading`). Repro: a spinoff self-merges; the supervisor
/// then rolls the manifest to `done` AND tears the worktree down (invariant #5);
/// a second `run merge` on the same id must refuse up front — no merge.sh spawn.
#[test]
fn second_merge_on_terminal_run_is_run_already_terminal() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let worktree = TempDir::new().unwrap();
    let run_id = create_run(&home, "spinoff", "double-merge");
    forge_worker_node(&home, &run_id, "spinoff", worktree.path(), "wt/test-x");

    // First merge: succeeds and appends the explicit-merge report.
    let merge_sh = fake_merge_sh(scratch.path(), 0, "");
    let v = run_ok(bin(&home).env("TASKFLEET_MERGE_SH", &merge_sh).args([
        "--output", "json", "run", "merge", &run_id, "--source", "main",
    ]));
    assert_eq!(v["data"]["merged"], true);

    // Reproduce the real post-teardown state the supervisor leaves: the run
    // rolled up terminal and its worktree was removed.
    set_run_status(&home, &run_id, scratch.path(), "done");
    std::fs::remove_dir_all(worktree.path()).unwrap();

    // Second merge: refused up front with the clear terminal error, no spawn.
    let out = bin(&home)
        .env("TASKFLEET_MERGE_SH", &merge_sh)
        .args([
            "--output", "json", "run", "merge", &run_id, "--source", "main",
        ])
        .output()
        .expect("spawn");
    let err: Value = serde_json::from_slice(&out.stderr).expect("stderr is JSON envelope");
    let msg = err["error"]["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("no worktree left to merge"),
        "the message must explain there is nothing to merge: {msg}"
    );
    // merge.log holds exactly the ONE line from the first merge.
    assert_refused_terminal(out, scratch.path(), 1);
}

/// A `cancelled` run is refused regardless of its worktree: cancellation is a
/// deliberate teardown the reducer never adopts a merge against, so `run merge`
/// must never spawn the backend for it. Here the worktree still EXISTS, proving
/// the refusal is on status alone (issue `merge-terminal-misleading`).
#[test]
fn merge_on_cancelled_run_is_refused() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let worktree = TempDir::new().unwrap();
    let run_id = create_run(&home, "spinoff", "cancelled-merge");
    forge_worker_node(&home, &run_id, "spinoff", worktree.path(), "wt/test-x");
    set_run_status(&home, &run_id, scratch.path(), "cancelled");

    let merge_sh = fake_merge_sh(scratch.path(), 0, "");
    let out = bin(&home)
        .env("TASKFLEET_MERGE_SH", &merge_sh)
        .args([
            "--output", "json", "run", "merge", &run_id, "--source", "main",
        ])
        .output()
        .expect("spawn");
    let err: Value = serde_json::from_slice(&out.stderr).expect("stderr is JSON envelope");
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("cancelled"),
        "the message must name the cancellation: {err}"
    );
    assert_refused_terminal(out, scratch.path(), 0);
}

/// A terminal run torn down WITHOUT ever being explicitly merged — a genuine
/// autonomous `failed` whose worktree the supervisor removed — also refuses with
/// the clear terminal error, not `merge_spawn_failed`. This is the case a
/// marker-only guard would have missed (it has no explicit-merge report).
#[test]
fn terminal_failed_torn_down_is_run_already_terminal() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let worktree = TempDir::new().unwrap();
    let run_id = create_run(&home, "spinoff", "failed-torn-down");
    forge_worker_node(&home, &run_id, "spinoff", worktree.path(), "wt/test-x");
    set_run_status(&home, &run_id, scratch.path(), "failed");
    std::fs::remove_dir_all(worktree.path()).unwrap();

    let merge_sh = fake_merge_sh(scratch.path(), 0, "");
    let out = bin(&home)
        .env("TASKFLEET_MERGE_SH", &merge_sh)
        .args([
            "--output", "json", "run", "merge", &run_id, "--source", "main",
        ])
        .output()
        .expect("spawn");
    assert_refused_terminal(out, scratch.path(), 0);
}

/// A NON-terminal run whose worktree has vanished surfaces the distinct
/// `worktree_missing` error (not `run_already_terminal`, not the misleading
/// `merge_spawn_failed`) — the worktree was removed out from under a live run.
#[test]
fn nonterminal_missing_worktree_is_worktree_missing() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let worktree = TempDir::new().unwrap();
    let run_id = create_run(&home, "spinoff", "live-no-worktree");
    forge_worker_node(&home, &run_id, "spinoff", worktree.path(), "wt/test-x");
    // No terminal status — the run is still live; just remove its worktree.
    std::fs::remove_dir_all(worktree.path()).unwrap();

    let merge_sh = fake_merge_sh(scratch.path(), 0, "");
    let out = bin(&home)
        .env("TASKFLEET_MERGE_SH", &merge_sh)
        .args([
            "--output", "json", "run", "merge", &run_id, "--source", "main",
        ])
        .output()
        .expect("spawn");
    assert!(!out.status.success());
    let err: Value = serde_json::from_slice(&out.stderr).expect("stderr is JSON envelope");
    assert_eq!(err["error"]["code"], "worktree_missing", "{err}");
    assert!(
        !scratch.path().join("merge.log").exists(),
        "the merge backend must not run when the worktree is missing"
    );
}

/// A `NotFound` from the merge-backend spawn is only re-attributed to a missing
/// worktree when the worktree is ACTUALLY gone. With a present worktree but a
/// bad `TASKFLEET_MERGE_SH` override (nonexistent backend), the error must remain the
/// generic `merge_spawn_failed` — not a spurious `worktree_missing` (round-2
/// review: the `NotFound` remap must not misattribute a missing backend).
#[test]
fn missing_backend_with_live_worktree_is_merge_spawn_failed() {
    let home = TestHome::new();
    let worktree = TempDir::new().unwrap();
    let run_id = create_run(&home, "spinoff", "bad-backend");
    forge_worker_node(&home, &run_id, "spinoff", worktree.path(), "wt/test-x");

    // Worktree present, but the backend path does not exist.
    let out = bin(&home)
        .env("TASKFLEET_MERGE_SH", "/no/such/merge-backend.sh")
        .args([
            "--output", "json", "run", "merge", &run_id, "--source", "main",
        ])
        .output()
        .expect("spawn");
    assert!(!out.status.success());
    let err: Value = serde_json::from_slice(&out.stderr).expect("stderr is JSON envelope");
    assert_eq!(
        err["error"]["code"], "merge_spawn_failed",
        "a missing backend (worktree present) must not be misread as worktree_missing: {err}"
    );
}

/// The guard does NOT block a terminal run whose worktree still EXISTS. This is
/// the load-bearing crash-safety / adoption path (issues
/// `reducer-adopt-explicit-merge`, `merge-skips-teardown`): a watchdog
/// `agent-died` false positive terminalizes the run to `failed` while the
/// still-alive agent's worktree survives (a blocked handoff preserves it), and
/// a merge that appended its report then crashed before teardown also leaves the
/// worktree in place. Either way `run merge` must fall through and complete —
/// worktree existence, not the merge marker, is the discriminator.
#[test]
fn terminal_but_unmerged_run_still_merges() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let worktree = TempDir::new().unwrap();
    let run_id = create_run(&home, "spinoff", "swallowed-then-merge");
    forge_worker_node(&home, &run_id, "spinoff", worktree.path(), "wt/test-x");

    // Watchdog false positive: the node is terminalized as agent-died, and the
    // run is rolled up to `failed` — but the worktree still exists.
    append_node_report(
        &home,
        &run_id,
        scratch.path(),
        r#"{"success": false, "failed": true, "reason": "agent-died"}"#,
    );
    set_run_status(&home, &run_id, scratch.path(), "failed");

    // The still-alive agent's `run merge` must PROCEED (worktree exists → the
    // guard falls through, so the reducer can adopt the merge).
    let merge_sh = fake_merge_sh(scratch.path(), 0, "");
    let v = run_ok(bin(&home).env("TASKFLEET_MERGE_SH", &merge_sh).args([
        "--output", "json", "run", "merge", &run_id, "--source", "main",
    ]));
    assert_eq!(
        v["data"]["merged"], true,
        "a terminal run with a surviving worktree must still accept run merge: {}",
        v["data"]
    );
}

/// A run id that names no run surfaces `run_not_found` (not a backend spawn).
#[test]
fn missing_run_is_run_not_found() {
    let home = TestHome::new();
    let out = bin(&home)
        .args([
            "--output",
            "json",
            "run",
            "merge",
            "01jxsnap000000000000000000",
        ])
        .output()
        .expect("spawn");
    assert!(!out.status.success());
    let err: Value = serde_json::from_slice(&out.stderr).expect("stderr is JSON envelope");
    assert_eq!(err["error"]["code"], "run_not_found");
}

/// Run `git <args>` in `cwd`, asserting success.
fn git(cwd: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("spawn git")
        .status
        .success();
    assert!(ok, "git {args:?} failed in {}", cwd.display());
}

/// True when local branch `branch` exists in `repo`.
fn branch_exists(repo: &Path, branch: &str) -> bool {
    Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "--verify", "--quiet", branch])
        .output()
        .expect("spawn git")
        .status
        .success()
}

/// Init a real repo on `main` with a linked worktree on `wt/foo`, returning
/// `(repo, worktree)` — enough for a full `git worktree remove` + `branch -D`
/// round-trip through `run merge`'s synchronous teardown.
fn init_repo_with_worktree(tmp: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let repo = tmp.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("README"), "x").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "init"]);
    let wt = tmp.join("wt");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt/foo",
            wt.to_str().unwrap(),
        ],
    );
    (repo, wt)
}

/// Submit a terminal `node.report` for `n-0001` via the agent self-report path,
/// so a test can pre-terminalize a node the way the watchdog's synthesized
/// report does — before `run merge` runs.
fn append_node_report(home: &TempDir, run_id: &str, scratch: &Path, data: &str) {
    let f = scratch.join("pre-report.json");
    std::fs::write(&f, data).unwrap();
    run_ok(bin(home).args([
        "--output",
        "json",
        "node",
        "report",
        run_id,
        "n-0001",
        "--from-file",
        f.to_str().unwrap(),
    ]));
}

/// THE `merge-skips-teardown` / `agent-died-merge-no-teardown-interactive` fix
/// (issue `reducer-adopt-explicit-merge`): a long-lived interactive node the
/// watchdog falsely declared `agent-died` is already terminal when the still-alive
/// agent runs `run merge`. The taskfleet-core reducer now ADOPTS the late
/// `via: "explicit-merge"` report even against that terminal node — overwriting
/// `last_report` and reconciling status to `Done` — so `any_node_merged_explicitly`
/// sees the merge and the SUPERVISOR (invariant #5) warrants teardown. `run merge`
/// no longer reclaims inline.
///
/// This run was never supervised (`--skip-materialize` skeleton), so there is no
/// live/restartable supervisor and the worktree/branch survive THIS call
/// (`supervisor: NotSupervised`) — real teardown is driven by a reattached
/// supervisor, proven end-to-end under a real detached supervisor in
/// `e2e_spinoff::swallowed_agent_died_then_merge_reattaches_and_tears_down`. Here
/// we assert the load-bearing projection change: the report is adopted.
#[test]
fn merge_adopts_swallowed_report_and_defers_teardown() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let gitroot = TempDir::new().unwrap();
    let (repo, wt) = init_repo_with_worktree(gitroot.path());
    let run_id = create_run(&home, "spinoff", "swallowed-merge");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");

    // Watchdog false positive: the node is terminalized as agent-died BEFORE the
    // merge. Pre-fix the reducer would swallow the explicit-merge report; now it
    // adopts it.
    append_node_report(
        &home,
        &run_id,
        scratch.path(),
        r#"{"success": false, "failed": true, "reason": "agent-died"}"#,
    );

    let merge_sh = fake_merge_sh(scratch.path(), 0, "");
    let v = run_ok(bin(&home).env("TASKFLEET_MERGE_SH", &merge_sh).args([
        "--output", "json", "run", "merge", &run_id, "--source", "main",
    ]));

    assert_eq!(v["data"]["merged"], true);
    // Never supervised → no teardown actor to (re)start; the supervisor owns
    // teardown, so this call leaves the resources for it.
    assert_eq!(
        v["data"]["supervisor"]["state"], "not-supervised",
        "a never-supervised run has no teardown actor: {}",
        v["data"]
    );
    assert!(
        wt.exists(),
        "run merge no longer reclaims inline; the supervisor owns teardown"
    );
    assert!(
        branch_exists(&repo, "wt/foo"),
        "the branch is left for the supervisor"
    );

    // THE fix: the reducer ADOPTED the explicit-merge report onto the projection,
    // reconciling the watchdog-FAILED node to Done, so a supervisor can now warrant
    // teardown (contrast the pre-fix behavior, where last_report stayed agent-died).
    let node_show =
        run_ok(bin(&home).args(["--output", "json", "node", "show", &run_id, "n-0001"]));
    assert_eq!(node_show["data"]["last_report"]["via"], "explicit-merge");
    assert_eq!(node_show["data"]["status"], "done");
}

/// The healthy interactive path is unchanged: when the node is LIVE at merge
/// time the reducer adopts the `explicit-merge` report, so `run merge` leaves
/// teardown to the supervisor (invariant #5) and does NOT reclaim inline — the
/// worktree/branch survive this call (a real supervisor, absent in this test,
/// would tear them down). Guards against the fix over-reaching into the path
/// that already works.
#[test]
fn merge_defers_to_supervisor_when_report_adopted() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let gitroot = TempDir::new().unwrap();
    let (repo, wt) = init_repo_with_worktree(gitroot.path());
    let run_id = create_run(&home, "spinoff", "adopted-merge");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");

    // No pre-terminalization: the node is live, so the explicit-merge report is
    // adopted and a supervisor owns teardown.
    let merge_sh = fake_merge_sh(scratch.path(), 0, "");
    let v = run_ok(bin(&home).env("TASKFLEET_MERGE_SH", &merge_sh).args([
        "--output", "json", "run", "merge", &run_id, "--source", "main",
    ]));

    assert_eq!(v["data"]["merged"], true);
    assert!(
        wt.exists(),
        "adopted path must NOT reclaim inline — the supervisor is the teardown actor"
    );
    assert!(
        branch_exists(&repo, "wt/foo"),
        "adopted path must leave the branch for the supervisor"
    );
    // The report was adopted onto the projection.
    let node_show =
        run_ok(bin(&home).args(["--output", "json", "node", "show", &run_id, "n-0001"]));
    assert_eq!(node_show["data"]["last_report"]["via"], "explicit-merge");
}

/// A FAILED merge (backend exits non-zero) on an already-terminal node must NOT
/// adopt or tear down anything — the worktree + branch survive and `run merge`
/// surfaces `merge_failed`. Guards the ordering: the terminal report is appended
/// (and thus the reducer's adoption + the supervisor's teardown are reachable)
/// ONLY AFTER `run_merge_sh` confirms the merge landed, so a failed merge can
/// never mark a branch merged or warrant its deletion.
#[test]
fn failed_merge_on_preterminal_node_reclaims_nothing() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let gitroot = TempDir::new().unwrap();
    let (repo, wt) = init_repo_with_worktree(gitroot.path());
    let run_id = create_run(&home, "spinoff", "swallowed-merge-fail");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");

    // Pre-terminalize the node so its report would be swallowed on a *successful*
    // merge — but here the merge itself fails.
    append_node_report(
        &home,
        &run_id,
        scratch.path(),
        r#"{"success": false, "failed": true, "reason": "agent-died"}"#,
    );

    let merge_sh = fake_merge_sh(scratch.path(), 1, "Error: rebase conflict");
    let out = bin(&home)
        .env("TASKFLEET_MERGE_SH", &merge_sh)
        .args([
            "--output", "json", "run", "merge", &run_id, "--source", "main",
        ])
        .output()
        .expect("spawn");

    assert!(!out.status.success(), "a failed merge must exit non-zero");
    let err: Value = serde_json::from_slice(&out.stderr).expect("stderr is JSON envelope");
    assert_eq!(err["error"]["code"], "merge_failed");
    assert!(wt.exists(), "a failed merge must not reclaim the worktree");
    assert!(
        branch_exists(&repo, "wt/foo"),
        "a failed merge must not reclaim the branch"
    );
}

// --- Concurrent self-merge race (issue `concurrent-self-merge-race`) ---
//
// Several independent spinoffs that self-merge into the SAME source branch within
// seconds must serialize on the merge lock, never observe each other's mid-merge
// (transient-dirty) target state. The bug: merge.sh checked the target worktree
// for cleanliness BEFORE taking the serializing lock, so a concurrent merge that
// was mid-rebase made the checker fail with a spurious "uncommitted changes in
// target". The fix moves that check inside the lock; a lock-acquisition timeout is
// surfaced as a distinct, retryable `merge_in_progress` error. These two tests
// drive the REAL bundled `scripts/merge.sh` (via `TASKFLEET_MERGE_SH`) against a real
// git repo + linked worktree; both exercised paths return before `workmux`, so
// they need neither `workmux` nor a live tmux.

/// Materialize the real bundled merge backend (not the stub) into `dir` with the
/// exec bit set, so these tests exercise the actual locking + cleanliness logic.
/// The checked-in `scripts/merge.sh` is not tracked executable, so it must be
/// copied + chmod'd (mirroring how `run merge` materializes the embedded copy).
fn real_merge_sh(dir: &Path) -> std::path::PathBuf {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/merge.sh");
    let body = std::fs::read(&src).expect("read scripts/merge.sh");
    let dst = dir.join("merge.sh");
    std::fs::write(&dst, body).unwrap();
    let mut perms = std::fs::metadata(&dst).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&dst, perms).unwrap();
    dst
}

/// Kills and reaps a spawned child on drop — panic-safe cleanup for the
/// background merge-lock holder, so a failing assertion can't leave a holder
/// process (and its lock) alive for the rest of its sleep.
struct ChildGuard(std::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Spawn a background holder of the repo's merge lock — the portable mkdir lock
/// merge.sh derives (`<git-common-dir>/worktree-merge.lock`, a directory). It
/// `mkdir`s the lock (aborting via `set -e` if that fails, so it never falsely
/// signals `ready` without holding the lock), touches `ready` once it holds the
/// lock, then holds it via `exec sleep` so the guard's SIGKILL hits the sleep
/// directly and leaves no orphaned process. The returned guard releases the lock
/// (kills the holder) on drop.
fn hold_merge_lock(repo: &Path, ready: &Path) -> ChildGuard {
    let lock = repo.join(".git").join("worktree-merge.lock");
    let child = Command::new("bash")
        .arg("-c")
        .arg(format!(
            "set -e; mkdir '{lock}'; touch '{ready}'; exec sleep 30",
            lock = lock.display(),
            ready = ready.display(),
        ))
        .spawn()
        .expect("spawn merge-lock holder");
    ChildGuard(child)
}

/// Spawn a holder that mimics a concurrent merge's full life: acquire the lock,
/// transiently dirty the target (`dirty`), signal `ready`, hold briefly, then
/// clean the target and release (`rm -rf` the lock dir, as merge.sh's trap does).
/// A merge that blocks on the lock during the dirty window must NOT observe the
/// dirt — it acquires only after the clean.
fn hold_lock_dirty_then_clean(repo: &Path, dirty: &Path, ready: &Path) -> ChildGuard {
    let lock = repo.join(".git").join("worktree-merge.lock");
    let child = Command::new("bash")
        .arg("-c")
        .arg(format!(
            "set -e; mkdir '{lock}'; touch '{dirty}'; touch '{ready}'; \
             sleep 2; rm -f '{dirty}'; rm -rf '{lock}'",
            lock = lock.display(),
            dirty = dirty.display(),
            ready = ready.display(),
        ))
        .spawn()
        .expect("spawn merge-lock holder");
    ChildGuard(child)
}

/// Create a dir holding a fake `workmux` that exits `code`, to prepend to PATH so
/// the real merge.sh can reach (and get past) the merge step without a real
/// workmux/tmux. Returns the dir to prepend.
fn fake_workmux_dir(dir: &Path, code: i32) -> std::path::PathBuf {
    let bindir = dir.join("fakebin");
    std::fs::create_dir_all(&bindir).unwrap();
    let p = bindir.join("workmux");
    std::fs::write(&p, format!("#!/bin/bash\nexit {code}\n")).unwrap();
    let mut perms = std::fs::metadata(&p).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&p, perms).unwrap();
    bindir
}

/// A fixture-only `workmux merge` that performs the source fast-forward the real
/// command owns. It keeps these tests isolated from an installed workmux/tmux.
#[cfg(target_os = "linux")]
fn fake_workmux_fast_forward_dir(dir: &Path) -> std::path::PathBuf {
    let bindir = dir.join("fakebin");
    std::fs::create_dir_all(&bindir).unwrap();
    let p = bindir.join("workmux");
    std::fs::write(
        &p,
        r#"#!/bin/bash
set -euo pipefail
target=""
while [[ $# -gt 0 ]]; do
  if [[ "$1" == "--into" ]]; then target="$2"; shift 2; else shift; fi
done
[[ -n "$target" ]]
branch=$(git symbolic-ref --short HEAD)
target_path=$(git worktree list --porcelain | sed -n '1s/^worktree //p')
actual_target=$(git -C "$target_path" symbolic-ref --short HEAD)
[[ "$actual_target" == "$target" ]]
git -C "$target_path" merge --ff-only "$branch"
"#,
    )
    .unwrap();
    let mut perms = std::fs::metadata(&p).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&p, perms).unwrap();
    bindir
}

#[cfg(target_os = "linux")]
fn commit_worker_change(worktree: &Path) {
    std::fs::write(worktree.join("WORKER.txt"), "worker change\n").unwrap();
    git(worktree, &["add", "WORKER.txt"]);
    git(worktree, &["commit", "-qm", "worker change"]);
}

#[cfg(target_os = "linux")]
fn oid(repo: &Path, rev: &str) -> String {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", rev])
        .output()
        .expect("spawn git rev-parse");
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[cfg(target_os = "linux")]
fn assert_no_embedded_merge_tempfiles(tmp: &Path) {
    let leftovers: Vec<_> = std::fs::read_dir(tmp)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| name.to_string_lossy().starts_with("taskfleet-merge-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "embedded merge-script temp paths must be cleaned up: {leftovers:?}"
    );
}

/// `PATH` with `prepend` in front of the inherited one.
fn path_with(prepend: &Path) -> String {
    format!(
        "{}:{}",
        prepend.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

/// Poll for a path to appear, up to `secs`. Panics if it never does.
fn wait_for(path: &Path, secs: u64) {
    for _ in 0..(secs * 50) {
        if path.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("timed out waiting for {}", path.display());
}

/// Linux regression for the default embedded backend. The old implementation
/// retained `NamedTempFile`'s writable descriptor through `execve`, which fails
/// with `ETXTBSY` on Linux before any script logic runs. No backend override is
/// present here: the embedded bytes are materialized, executed, and cleaned up.
#[cfg(target_os = "linux")]
#[test]
fn default_embedded_backend_executes_and_cleans_up() {
    let home = TestHome::new();
    let gitroot = TempDir::new().unwrap();
    let script_tmp = TempDir::new().unwrap();
    let (repo, wt) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&wt);
    let worker_oid = oid(&wt, "HEAD");
    let run_id = create_run(&home, "spinoff", "embedded-linux-merge");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");
    let fakebin = fake_workmux_fast_forward_dir(gitroot.path());

    let v = run_ok(
        bin(&home)
            .env_remove("TASKFLEET_MERGE_SH")
            .env("TMPDIR", script_tmp.path())
            .env("PATH", path_with(&fakebin))
            .args([
                "--output", "json", "run", "merge", &run_id, "--source", "main",
            ]),
    );

    assert_eq!(v["data"]["merged"], true);
    assert_eq!(oid(&repo, "main"), worker_oid, "source must fast-forward");
    assert!(branch_exists(&repo, "wt/foo"), "supervisor owns teardown");
    let reports = node_reports(&run_dir(&home, &run_id).join("events.jsonl"));
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["data"]["via"], "explicit-merge");
    assert_eq!(reports[0]["data"]["success"], true);
    assert_no_embedded_merge_tempfiles(script_tmp.path());
}

/// `run salvage` delegates to the same merge execution path. Exercise that real
/// path too, without the external backend override that masked Linux `ETXTBSY`.
#[cfg(target_os = "linux")]
#[test]
fn salvage_uses_default_embedded_backend() {
    let home = TestHome::new();
    let gitroot = TempDir::new().unwrap();
    let script_tmp = TempDir::new().unwrap();
    let (repo, wt) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&wt);
    let worker_oid = oid(&wt, "HEAD");
    let run_id = create_run(&home, "spinoff", "embedded-linux-salvage");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");

    let paths = RunPaths::new(run_dir(&home, &run_id), &run_id).unwrap();
    append_and_apply_event(
        &paths,
        "worker.exited",
        Some(&NodeId::parse_str("n-0001").unwrap()),
        None,
        json!({ "exit_code": 0 }),
    )
    .unwrap();

    let fakebin = fake_workmux_fast_forward_dir(gitroot.path());
    let v = run_ok(
        bin(&home)
            .env_remove("TASKFLEET_MERGE_SH")
            .env("TMPDIR", script_tmp.path())
            .env("PATH", path_with(&fakebin))
            .args([
                "--output", "json", "run", "salvage", &run_id, "--source", "main",
            ]),
    );

    assert_eq!(v["data"]["worker_state"], "exited");
    assert_eq!(v["data"]["merge"]["merged"], true);
    assert_eq!(
        oid(&repo, "main"),
        worker_oid,
        "salvage must fast-forward source"
    );
    let reports = node_reports(&run_dir(&home, &run_id).join("events.jsonl"));
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["data"]["via"], "explicit-merge");
    assert_no_embedded_merge_tempfiles(script_tmp.path());
}

/// Embedded-script failures retain the established error/report/ref contract and
/// still remove the private script path after the child exits.
#[cfg(target_os = "linux")]
#[test]
fn default_embedded_backend_failure_preserves_state_and_cleans_up() {
    let home = TestHome::new();
    let gitroot = TempDir::new().unwrap();
    let script_tmp = TempDir::new().unwrap();
    let (repo, wt) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&wt);
    let source_before = oid(&repo, "main");
    let worker_before = oid(&wt, "HEAD");
    let run_id = create_run(&home, "spinoff", "embedded-linux-failure");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");
    let fakebin = fake_workmux_dir(gitroot.path(), 42);

    let out = bin(&home)
        .env_remove("TASKFLEET_MERGE_SH")
        .env("TMPDIR", script_tmp.path())
        .env("PATH", path_with(&fakebin))
        .args([
            "--output", "json", "run", "merge", &run_id, "--source", "main",
        ])
        .output()
        .expect("spawn taskfleet");

    assert!(!out.status.success());
    let err: Value = serde_json::from_slice(&out.stderr).expect("JSON error envelope");
    assert_eq!(err["error"]["code"], "merge_failed");
    assert!(err["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("workmux merge failed (exit 42)"));
    assert_eq!(
        oid(&repo, "main"),
        source_before,
        "source ref must not move"
    );
    assert_eq!(oid(&wt, "HEAD"), worker_before, "worker ref must not move");
    assert!(branch_exists(&repo, "wt/foo"));
    assert_eq!(
        node_reports(&run_dir(&home, &run_id).join("events.jsonl")).len(),
        0,
        "a failed backend must not submit a success report"
    );
    assert_no_embedded_merge_tempfiles(script_tmp.path());
}

/// THE regression for the race: another merge holds the lock AND the target
/// worktree is (transiently) dirty. Pre-fix, merge.sh checked the target BEFORE
/// the lock and failed immediately with the spurious "uncommitted changes in
/// target" (`merge_failed`). Post-fix, the checker lives inside the lock, so this
/// merge serializes: it blocks on the held lock and, when the hold outlasts the
/// timeout, surfaces the DISTINCT, retryable `merge_in_progress` — never the false
/// dirty-target failure. No terminal report is written (the merge never ran).
#[test]
fn concurrent_self_merge_serializes_instead_of_false_dirty() {
    let home = TestHome::new();
    let gitroot = TempDir::new().unwrap();
    let (repo, wt) = init_repo_with_worktree(gitroot.path());
    let run_id = create_run(&home, "spinoff", "race-merge");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");

    // Simulate another merge's mid-rebase transient state: the target worktree is
    // dirty. Pre-fix this alone (checked before the lock) produced the false
    // positive; post-fix it is only inspected once we hold the lock.
    std::fs::write(repo.join("RACE.txt"), "in-flight merge state").unwrap();

    // Another merge holds the serializing lock for the whole test.
    let ready = gitroot.path().join("lock-ready");
    let _holder = hold_merge_lock(&repo, &ready);
    wait_for(&ready, 5);

    // Our merge waits on the lock, then times out (1s) — a serialization
    // conflict, surfaced as the distinct retryable code, NOT a dirty-tree error.
    let out = bin(&home)
        .env("TASKFLEET_MERGE_SH", real_merge_sh(gitroot.path()))
        .env("MERGE_LOCK_TIMEOUT", "1")
        .args([
            "--output", "json", "run", "merge", &run_id, "--source", "main",
        ])
        .output()
        .expect("spawn");

    assert!(
        !out.status.success(),
        "a merge blocked by a concurrent one must not succeed"
    );
    let err: Value = serde_json::from_slice(&out.stderr).expect("stderr is JSON envelope");
    assert_eq!(
        err["error"]["code"], "merge_in_progress",
        "a lock-held concurrent merge must surface the distinct serialization code, \
         not a dirty-tree failure: {err}"
    );
    let msg = err["error"]["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("another merge is holding"),
        "the error must name the serialization conflict, not the transient dirt: {msg}"
    );
    assert!(
        !msg.to_lowercase().contains("uncommitted changes in target"),
        "the false-positive dirty-target error must be gone: {msg}"
    );

    // The merge never ran, so no terminal report was appended.
    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert_eq!(
        node_reports(&events).len(),
        0,
        "a serialized-out merge must not submit a terminal report"
    );
}

/// The genuine dirty-target safety check is preserved: with NO concurrent merge
/// (the lock is free) but the target worktree carrying real uncommitted user
/// work, merge.sh acquires the lock, finds the target dirty, and blocks with its
/// existing dirty-target message (`merge_failed`). Guards against the fix
/// weakening the real safety check while removing the racy pre-lock one.
#[test]
fn genuine_dirty_target_still_blocks() {
    let home = TestHome::new();
    let gitroot = TempDir::new().unwrap();
    let (repo, wt) = init_repo_with_worktree(gitroot.path());
    let run_id = create_run(&home, "spinoff", "dirty-target");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");

    // Real uncommitted user work in the target, and NO lock holder — the merge
    // will acquire the lock and must still refuse a dirty target.
    std::fs::write(repo.join("USER-WORK.txt"), "human's uncommitted edit").unwrap();

    let out = bin(&home)
        .env("TASKFLEET_MERGE_SH", real_merge_sh(gitroot.path()))
        .args([
            "--output", "json", "run", "merge", &run_id, "--source", "main",
        ])
        .output()
        .expect("spawn");

    assert!(
        !out.status.success(),
        "a genuinely dirty target must still block the merge"
    );
    let err: Value = serde_json::from_slice(&out.stderr).expect("stderr is JSON envelope");
    assert_eq!(
        err["error"]["code"], "merge_failed",
        "a genuine dirty target is a hard merge failure, not a serialization retry: {err}"
    );
    let msg = err["error"]["message"].as_str().unwrap_or_default();
    assert!(
        msg.to_lowercase().contains("uncommitted changes in target"),
        "the genuine dirty-target message must survive: {msg}"
    );
    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert_eq!(node_reports(&events).len(), 0);
}

/// The PRIMARY behavior the fix enables: a merge that starts while a concurrent
/// merge holds the lock AND has the target transiently dirty must SERIALIZE —
/// block on the lock, and only proceed once the peer releases and the target is
/// clean again — then SUCCEED. Pre-fix, the pre-lock dirty check made it fail
/// spuriously; post-fix, the check is behind the lock, so the transient dirt is
/// never observed and the merge lands. A fake `workmux` (exit 0) lets the real
/// merge.sh reach and pass the merge step without a real workmux/tmux.
#[test]
fn concurrent_self_merge_waits_then_succeeds() {
    let home = TestHome::new();
    let gitroot = TempDir::new().unwrap();
    let (repo, wt) = init_repo_with_worktree(gitroot.path());
    let run_id = create_run(&home, "spinoff", "race-success");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");

    let fakebin = fake_workmux_dir(gitroot.path(), 0);

    // A peer holds the lock, dirties the target for ~2s, then cleans + releases.
    let dirty = repo.join("PEER-INFLIGHT.txt");
    let ready = gitroot.path().join("lock-ready");
    let _holder = hold_lock_dirty_then_clean(&repo, &dirty, &ready);
    wait_for(&ready, 5); // peer now holds the lock with the target dirty

    // Launch our merge WHILE the peer holds the lock + target is dirty. It must
    // block on the lock (never seeing the dirt), then land once the peer frees.
    let v = run_ok(
        bin(&home)
            .env("TASKFLEET_MERGE_SH", real_merge_sh(gitroot.path()))
            .env("PATH", path_with(&fakebin))
            .env("MERGE_LOCK_TIMEOUT", "30")
            .args([
                "--output", "json", "run", "merge", &run_id, "--source", "main",
            ]),
    );

    assert_eq!(
        v["data"]["merged"], true,
        "a merge that serialized behind a concurrent one must still land: {}",
        v["data"]
    );
    let events = run_dir(&home, &run_id).join("events.jsonl");
    let reports = node_reports(&events);
    assert_eq!(reports.len(), 1, "the serialized merge submits one report");
    assert_eq!(reports[0]["data"]["via"], "explicit-merge");
}

/// A downstream command exiting 75 must NOT masquerade as the lock-timeout
/// `merge_in_progress`. merge.sh reserves exit 75 for the lock-timeout branch
/// and normalizes `workmux`'s exit, so a `workmux` that exits 75 (with the lock
/// free and the target clean) surfaces as a plain `merge_failed`.
#[test]
fn downstream_exit_75_is_not_merge_in_progress() {
    let home = TestHome::new();
    let gitroot = TempDir::new().unwrap();
    let (_repo, wt) = init_repo_with_worktree(gitroot.path());
    let run_id = create_run(&home, "spinoff", "exit75");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");

    // No lock holder, target clean — the merge reaches workmux, which exits 75.
    let fakebin = fake_workmux_dir(gitroot.path(), 75);
    let out = bin(&home)
        .env("TASKFLEET_MERGE_SH", real_merge_sh(gitroot.path()))
        .env("PATH", path_with(&fakebin))
        .args([
            "--output", "json", "run", "merge", &run_id, "--source", "main",
        ])
        .output()
        .expect("spawn");

    assert!(
        !out.status.success(),
        "a workmux failure must fail the merge"
    );
    let err: Value = serde_json::from_slice(&out.stderr).expect("stderr is JSON envelope");
    assert_eq!(
        err["error"]["code"], "merge_failed",
        "a downstream exit 75 must not be misread as a lock-timeout retry: {err}"
    );
    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert_eq!(
        node_reports(&events).len(),
        0,
        "a failed merge writes no report"
    );
}
