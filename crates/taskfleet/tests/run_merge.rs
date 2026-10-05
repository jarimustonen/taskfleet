//! Git-backed integration tests for `taskfleet run merge`. All repositories and
//! worktrees are disposable; no workmux or personal tmux is involved.

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
    let gitroot = TempDir::new().unwrap();
    let (repo, worktree) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&worktree);
    let run_id = create_run(&home, "spinoff", "merge-ok");
    forge_worker_node(&home, &run_id, "spinoff", &worktree, "wt/foo");

    let v = run_ok(bin(&home).args(["--output", "json", "run", "merge", &run_id]));
    assert_eq!(v["data"]["merged"], true);
    assert_eq!(v["data"]["branch"], "wt/foo");
    assert_eq!(v["data"]["source"], "main");

    // The backend was invoked with the resolved target and branch.
    assert_eq!(oid(&repo, "main"), oid(&worktree, "HEAD"));

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
    let gitroot = TempDir::new().unwrap();
    let (_repo, worktree) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&worktree);
    let run_id = create_run(&home, "research", "merge-rich");
    forge_worker_node(&home, &run_id, "research", &worktree, "wt/foo");

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

    run_ok(bin(&home).args([
        "--output",
        "json",
        "run",
        "merge",
        &run_id,
        "--source",
        "main",
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
    let gitroot = TempDir::new().unwrap();
    let (repo, worktree) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&worktree);
    let run_id = create_run(&home, "spinoff", "merge-lenient");
    forge_worker_node(&home, &run_id, "spinoff", &worktree, "wt/foo");

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

    let v = run_ok(bin(&home).args([
        "--output",
        "json",
        "run",
        "merge",
        &run_id,
        "--source",
        "main",
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
    assert_eq!(oid(&repo, "main"), oid(&worktree, "HEAD"));
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
    let gitroot = TempDir::new().unwrap();
    let (repo, worktree) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&worktree);

    for body in [
        r#"{"success": false, "summary": "blocked"}"#,
        r#"{"success": true, "cancelled": true, "summary": "cancelled"}"#,
    ] {
        let run_id = create_run(&home, "spinoff", "reject-nonsuccess");
        forge_worker_node(&home, &run_id, "spinoff", &worktree, "wt/foo");
        let report = scratch.path().join("bad-report.json");
        std::fs::write(&report, body).unwrap();
        let out = bin(&home)
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
        assert_ne!(oid(&repo, "main"), oid(&worktree, "HEAD"));
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
    let gitroot = TempDir::new().unwrap();
    let (repo, worktree) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&worktree);
    let run_id = create_run(&home, "spinoff", "merge-badreport");
    forge_worker_node(&home, &run_id, "spinoff", &worktree, "wt/foo");

    // Missing the required `success` field.
    let report = scratch.path().join("bad.json");
    std::fs::write(&report, r#"{"summary": "no success field"}"#).unwrap();

    let out = bin(&home)
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
    assert_ne!(oid(&repo, "main"), oid(&worktree, "HEAD"));
    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert_eq!(node_reports(&events).len(), 0);
}

/// A merge failure (conflict / dirty tree / lock timeout): the backend exits
/// non-zero, `run merge` surfaces `merge_failed`, and NO terminal report is
/// appended — the node stays live for the agent to recover and retry.
#[test]
fn failed_merge_surfaces_error_and_writes_no_report() {
    let home = TestHome::new();
    let gitroot = TempDir::new().unwrap();
    let (_repo, worktree) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&worktree);
    let run_id = create_run(&home, "spinoff", "merge-fail");
    forge_worker_node(&home, &run_id, "spinoff", &worktree, "wt/foo");
    std::fs::write(worktree.join("UNCOMMITTED"), "dirty").unwrap();

    let out = bin(&home)
        .args([
            "--output", "json", "run", "merge", &run_id, "--source", "main",
        ])
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
    let gitroot = TempDir::new().unwrap();
    let (_repo, worktree) = init_repo_with_worktree(gitroot.path());
    let run_id = create_run(&home, "spinoff", "merge-dry");
    forge_worker_node(&home, &run_id, "spinoff", &worktree, "wt/foo");

    let v = run_ok(bin(&home).args(["--output", "json", "run", "merge", &run_id, "--dry-run"]));
    assert_eq!(v["data"]["dry_run"], true);
    assert_eq!(v["data"]["branch"], "wt/foo");

    // The backend was never invoked and no report was written.
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

/// Assert the terminal status is classified before attempting a Git merge.
fn assert_refused_terminal(out: std::process::Output) {
    assert!(!out.status.success(), "the merge must be refused");
    let err: Value = serde_json::from_slice(&out.stderr).expect("stderr is JSON envelope");
    assert_eq!(err["error"]["code"], "run_already_terminal", "{err}");
}

/// Re-merging an already-finished run fails with the clear `run_already_terminal`
/// error, NOT the misleading `merge_spawn_failed` (issue
/// `merge-terminal-misleading`). Repro: a spinoff self-merges; the supervisor
/// then rolls the manifest to `done` AND tears the worktree down (invariant #5);
/// a second `run merge` on the same id must refuse up front — no Rust merge driver spawn.
#[test]
fn second_merge_on_terminal_run_is_run_already_terminal() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let gitroot = TempDir::new().unwrap();
    let (repo, worktree) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&worktree);
    let run_id = create_run(&home, "spinoff", "double-merge");
    forge_worker_node(&home, &run_id, "spinoff", &worktree, "wt/foo");

    // First merge: succeeds and appends the explicit-merge report.
    let v = run_ok(bin(&home).args([
        "--output", "json", "run", "merge", &run_id, "--source", "main",
    ]));
    assert_eq!(v["data"]["merged"], true);

    // Reproduce the real post-teardown state the supervisor leaves: the run
    // rolled up terminal and its worktree was removed.
    set_run_status(&home, &run_id, scratch.path(), "done");
    git(
        &repo,
        &["worktree", "remove", "--force", worktree.to_str().unwrap()],
    );

    // Second merge: refused up front with the clear terminal error, no spawn.
    let out = bin(&home)
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
    // No second report or ref mutation occurred.
    assert_refused_terminal(out);
}

/// A `cancelled` run is refused regardless of its worktree: cancellation is a
/// deliberate teardown the reducer never adopts a merge against, so `run merge`
/// must never spawn the backend for it. Here the worktree still EXISTS, proving
/// the refusal is on status alone (issue `merge-terminal-misleading`).
#[test]
fn merge_on_cancelled_run_is_refused() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let gitroot = TempDir::new().unwrap();
    let (_repo, worktree) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&worktree);
    let run_id = create_run(&home, "spinoff", "cancelled-merge");
    forge_worker_node(&home, &run_id, "spinoff", &worktree, "wt/foo");
    set_run_status(&home, &run_id, scratch.path(), "cancelled");

    let out = bin(&home)
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
    assert_refused_terminal(out);
}

/// A terminal run torn down WITHOUT ever being explicitly merged — a genuine
/// autonomous `failed` whose worktree the supervisor removed — also refuses with
/// the clear terminal error, not `merge_spawn_failed`. This is the case a
/// marker-only guard would have missed (it has no explicit-merge report).
#[test]
fn terminal_failed_torn_down_is_run_already_terminal() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let gitroot = TempDir::new().unwrap();
    let (repo, worktree) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&worktree);
    let run_id = create_run(&home, "spinoff", "failed-torn-down");
    forge_worker_node(&home, &run_id, "spinoff", &worktree, "wt/foo");
    set_run_status(&home, &run_id, scratch.path(), "failed");
    git(
        &repo,
        &["worktree", "remove", "--force", worktree.to_str().unwrap()],
    );

    let out = bin(&home)
        .args([
            "--output", "json", "run", "merge", &run_id, "--source", "main",
        ])
        .output()
        .expect("spawn");
    assert_refused_terminal(out);
}

/// A NON-terminal run whose worktree has vanished surfaces the distinct
/// `worktree_missing` error (not `run_already_terminal`, not the misleading
/// `merge_spawn_failed`) — the worktree was removed out from under a live run.
#[test]
fn nonterminal_missing_worktree_is_worktree_missing() {
    let home = TestHome::new();
    let gitroot = TempDir::new().unwrap();
    let (repo, worktree) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&worktree);
    let run_id = create_run(&home, "spinoff", "live-no-worktree");
    forge_worker_node(&home, &run_id, "spinoff", &worktree, "wt/foo");
    // No terminal status — the run is still live; just remove its worktree.
    git(
        &repo,
        &["worktree", "remove", "--force", worktree.to_str().unwrap()],
    );

    let out = bin(&home)
        .args([
            "--output", "json", "run", "merge", &run_id, "--source", "main",
        ])
        .output()
        .expect("spawn");
    assert!(!out.status.success());
    let err: Value = serde_json::from_slice(&out.stderr).expect("stderr is JSON envelope");
    assert_eq!(err["error"]["code"], "worktree_missing", "{err}");
    assert_eq!(
        node_reports(&run_dir(&home, &run_id).join("events.jsonl")).len(),
        0
    );
}

/// A `NotFound` from the merge-backend spawn is only re-attributed to a missing
/// worktree when the worktree is ACTUALLY gone. With a present worktree but a
/// bad `GIT_BIN` override (nonexistent backend), the error must remain the
/// generic `merge_spawn_failed` — not a spurious `worktree_missing` (round-2
/// review: the `NotFound` remap must not misattribute a missing backend).
#[test]
fn missing_git_with_live_worktree_fails_without_report() {
    let home = TestHome::new();
    let gitroot = TempDir::new().unwrap();
    let (_repo, worktree) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&worktree);
    let run_id = create_run(&home, "spinoff", "bad-backend");
    forge_worker_node(&home, &run_id, "spinoff", &worktree, "wt/foo");

    // Worktree present, but the backend path does not exist.
    let out = bin(&home)
        .env("GIT_BIN", "/no/such/git")
        .args([
            "--output", "json", "run", "merge", &run_id, "--source", "main",
        ])
        .output()
        .expect("spawn");
    assert!(!out.status.success());
    let err: Value = serde_json::from_slice(&out.stderr).expect("stderr is JSON envelope");
    assert_eq!(
        node_reports(&run_dir(&home, &run_id).join("events.jsonl")).len(),
        0
    );
    assert_eq!(
        err["error"]["code"], "merge_failed",
        "a missing git (worktree present) must not be misread as worktree_missing: {err}"
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
    let gitroot = TempDir::new().unwrap();
    let (_repo, worktree) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&worktree);
    let run_id = create_run(&home, "spinoff", "swallowed-then-merge");
    forge_worker_node(&home, &run_id, "spinoff", &worktree, "wt/foo");

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
    let v = run_ok(bin(&home).args([
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
    commit_worker_change(&wt);
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

    let v = run_ok(bin(&home).args([
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
    let gitroot = TempDir::new().unwrap();
    let (repo, wt) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&wt);
    let run_id = create_run(&home, "spinoff", "adopted-merge");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");

    // No pre-terminalization: the node is live, so the explicit-merge report is
    // adopted and a supervisor owns teardown.
    let v = run_ok(bin(&home).args([
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
/// ONLY AFTER `run_git_merge` confirms the merge landed, so a failed merge can
/// never mark a branch merged or warrant its deletion.
#[test]
fn failed_merge_on_preterminal_node_reclaims_nothing() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let gitroot = TempDir::new().unwrap();
    let (repo, wt) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&wt);
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

    std::fs::write(wt.join("UNCOMMITTED"), "dirty").unwrap();
    let out = bin(&home)
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
// (transient-dirty) target state. The bug: Rust merge driver checked the target worktree
// for cleanliness BEFORE taking the serializing lock, so a concurrent merge that
// was mid-rebase made the checker fail with a spurious "uncommitted changes in
// target". The fix moves that check inside the lock; a lock-acquisition timeout is
// surfaced as a distinct, retryable `merge_in_progress` error. These two tests
// drive the REAL bundled `scripts/Rust merge driver` (via `GIT_BIN`) against a real
// git repo + linked worktree; both exercised paths return before `workmux`, so
// they need neither `workmux` nor a live tmux.

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
/// Rust merge driver derives (`<git-common-dir>/worktree-merge.lock`, a directory). It
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
/// clean the target and release (`rm -rf` the lock dir, as Rust merge driver's trap does).
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

/// The production merge requires Git but neither workmux nor ambient tools.
#[cfg(target_os = "linux")]
#[test]
fn git_only_merge_succeeds_with_stripped_path() {
    let home = TestHome::new();
    let gitroot = TempDir::new().unwrap();
    let (repo, wt) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&wt);
    let worker_oid = oid(&wt, "HEAD");
    let run_id = create_run(&home, "spinoff", "embedded-linux-merge");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");
    let git_bin = Command::new("which").arg("git").output().unwrap();
    assert!(git_bin.status.success());
    let git_bin = String::from_utf8(git_bin.stdout)
        .unwrap()
        .trim()
        .to_string();
    let empty_path = TempDir::new().unwrap();
    let v = run_ok(
        bin(&home)
            .env("PATH", empty_path.path())
            .env("GIT_BIN", git_bin)
            .env("WORKMUX_BIN", "/no/such/workmux")
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
}

/// `run salvage` delegates to the same Git-backed merge execution path.
#[cfg(target_os = "linux")]
#[test]
fn salvage_uses_default_embedded_backend() {
    let home = TestHome::new();
    let gitroot = TempDir::new().unwrap();
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

    let v = run_ok(bin(&home).args([
        "--output", "json", "run", "salvage", &run_id, "--source", "main",
    ]));

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
}

/// Git-driver failures retain the established error/report/ref contract and
/// still remove the private script path after the child exits.
#[cfg(target_os = "linux")]
#[test]
fn default_embedded_backend_failure_preserves_state_and_cleans_up() {
    let home = TestHome::new();
    let gitroot = TempDir::new().unwrap();
    let (repo, wt) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&wt);
    std::fs::write(wt.join("UNCOMMITTED"), "dirty").unwrap();
    let source_before = oid(&repo, "main");
    let worker_before = oid(&wt, "HEAD");
    let run_id = create_run(&home, "spinoff", "embedded-linux-failure");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");

    let out = bin(&home)
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
        .contains("Uncommitted changes in worktree"));
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
}

/// THE regression for the race: another merge holds the lock AND the target
/// worktree is (transiently) dirty. Pre-fix, Rust merge driver checked the target BEFORE
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
    commit_worker_change(&wt);
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
/// work, Rust merge driver acquires the lock, finds the target dirty, and blocks with its
/// existing dirty-target message (`merge_failed`). Guards against the fix
/// weakening the real safety check while removing the racy pre-lock one.
#[test]
fn genuine_dirty_target_still_blocks() {
    let home = TestHome::new();
    let gitroot = TempDir::new().unwrap();
    let (repo, wt) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&wt);
    let run_id = create_run(&home, "spinoff", "dirty-target");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");

    // Real uncommitted user work in the target, and NO lock holder — the merge
    // will acquire the lock and must still refuse a dirty target.
    std::fs::write(repo.join("USER-WORK.txt"), "human's uncommitted edit").unwrap();

    let out = bin(&home)
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
/// Rust merge driver reach and pass the merge step without a real workmux/tmux.
#[test]
fn concurrent_self_merge_waits_then_succeeds() {
    let home = TestHome::new();
    let gitroot = TempDir::new().unwrap();
    let (repo, wt) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&wt);
    let run_id = create_run(&home, "spinoff", "race-success");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");

    // A peer holds the lock, dirties the target for ~2s, then cleans + releases.
    let dirty = repo.join("PEER-INFLIGHT.txt");
    let ready = gitroot.path().join("lock-ready");
    let _holder = hold_lock_dirty_then_clean(&repo, &dirty, &ready);
    wait_for(&ready, 5); // peer now holds the lock with the target dirty

    // Launch our merge WHILE the peer holds the lock + target is dirty. It must
    // block on the lock (never seeing the dirt), then land once the peer frees.
    let v = run_ok(bin(&home).env("MERGE_LOCK_TIMEOUT", "30").args([
        "--output", "json", "run", "merge", &run_id, "--source", "main",
    ]));

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
/// `merge_in_progress`. Rust merge driver reserves exit 75 for the lock-timeout branch
/// and normalizes `workmux`'s exit, so a `workmux` that exits 75 (with the lock
/// free and the target clean) surfaces as a plain `merge_failed`.
#[test]
fn downstream_exit_75_is_not_merge_in_progress() {
    let home = TestHome::new();
    let gitroot = TempDir::new().unwrap();
    let (_repo, wt) = init_repo_with_worktree(gitroot.path());
    commit_worker_change(&wt);
    let shim = gitroot.path().join("git-shim");
    std::fs::write(
        &shim,
        r#"#!/bin/sh
case " $* " in *" merge --ff-only "*) exit 75;; esac
exec git "$@"
"#,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    let run_id = create_run(&home, "spinoff", "exit75");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");

    // No lock holder, target clean — the merge reaches workmux, which exits 75.
    let out = bin(&home)
        .env("GIT_BIN", &shim)
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

/// A real rebase conflict leaves both refs and the worker checkout available
/// for a human to resolve; it never emits a successful terminal report.
#[test]
fn rust_driver_conflict_preserves_unmerged_work() {
    let home = TestHome::new();
    let root = TempDir::new().unwrap();
    let (repo, wt) = init_repo_with_worktree(root.path());
    std::fs::write(repo.join("README"), "source\n").unwrap();
    git(&repo, &["commit", "-qam", "source edit"]);
    let before = oid(&repo, "main");
    std::fs::write(wt.join("README"), "worker\n").unwrap();
    git(&wt, &["commit", "-qam", "worker edit"]);
    let run_id = create_run(&home, "spinoff", "conflict");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");
    let out = bin(&home)
        .env("WORKMUX_BIN", "/no/such/workmux")
        .args([
            "--output", "json", "run", "merge", &run_id, "--source", "main",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["error"]["code"], "merge_failed");
    assert_eq!(oid(&repo, "main"), before);
    assert!(wt.exists());
    assert_eq!(
        node_reports(&run_dir(&home, &run_id).join("events.jsonl")).len(),
        0
    );
}

/// The recorded source OID is checked after the merge lock: a writer moving
/// main while this invocation waits cannot be overwritten by the later FF.
#[test]
fn source_movement_while_waiting_on_lock_refuses_merge() {
    let home = TestHome::new();
    let root = TempDir::new().unwrap();
    let (repo, wt) = init_repo_with_worktree(root.path());
    commit_worker_change(&wt);
    let run_id = create_run(&home, "spinoff", "cas-source-move");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");
    let lock = repo.join(".git/worktree-merge.lock");
    std::fs::create_dir(&lock).unwrap();
    let mut child = bin(&home);
    child.args([
        "--output", "json", "run", "merge", &run_id, "--source", "main",
    ]);
    let mut child = child.spawn().unwrap();
    let events = run_dir(&home, &run_id).join("events.jsonl");
    for _ in 0..250 {
        if read_events(&events)
            .iter()
            .any(|event| event["kind"] == "merge.started")
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(read_events(&events)
        .iter()
        .any(|event| event["kind"] == "merge.started"));
    std::fs::write(repo.join("SOURCE"), "other writer").unwrap();
    git(&repo, &["add", "SOURCE"]);
    git(&repo, &["commit", "-qm", "other writer"]);
    let moved = oid(&repo, "main");
    std::fs::remove_dir(&lock).unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success());
    assert_eq!(oid(&repo, "main"), moved);
    assert!(wt.exists());
    assert_eq!(node_reports(&events).len(), 0);
    assert!(read_events(&events)
        .iter()
        .any(|event| event["kind"] == "merge.aborted"));
}

/// A driver killed immediately after Git moves the source leaves a pending
/// transaction; the retry verifies its recorded worker OID and completes the
/// missing report rather than running a second merge.
#[cfg(target_os = "linux")]
#[test]
fn killed_driver_after_ref_move_is_recovered() {
    use std::os::unix::fs::PermissionsExt as _;
    let home = TestHome::new();
    let root = TempDir::new().unwrap();
    let (repo, wt) = init_repo_with_worktree(root.path());
    commit_worker_change(&wt);
    let worker_oid = oid(&wt, "HEAD");
    let run_id = create_run(&home, "spinoff", "crash-after-git");
    forge_worker_node(&home, &run_id, "spinoff", &wt, "wt/foo");

    let git_path = Command::new("which").arg("git").output().unwrap();
    let git_path = String::from_utf8(git_path.stdout)
        .unwrap()
        .trim()
        .to_string();
    let shim = root.path().join("git-crash-shim");
    // No untrusted argv reaches this fixture; it only intercepts the final FF.
    std::fs::write(
        &shim,
        format!(
            r#"#!/bin/sh
case " $* " in
  *" merge --ff-only "*)
    "{git_path}" "$@" || exit $?
    kill -9 "$PPID"
    exit 0 ;;
esac
exec "{git_path}" "$@"
"#
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = bin(&home)
        .env("GIT_BIN", &shim)
        .args([
            "--output", "json", "run", "merge", &run_id, "--source", "main",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success(), "the fixture must kill the driver");
    assert_eq!(oid(&repo, "main"), worker_oid);
    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert!(read_events(&events)
        .iter()
        .any(|e| e["kind"] == "merge.started"));
    assert_eq!(node_reports(&events).len(), 0);
    let recovered = run_ok(bin(&home).args([
        "--output", "json", "run", "merge", &run_id, "--source", "main",
    ]));
    assert_eq!(recovered["data"]["merged"], true);
    assert_eq!(oid(&repo, "main"), worker_oid);
    let reports = node_reports(&events);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["data"]["origin"]["kind"], "run-merge");
    assert!(
        wt.exists(),
        "only the supervisor may tear down the checkout"
    );
}
