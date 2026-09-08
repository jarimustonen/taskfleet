//! Integration tests for `taskfleet run wait`.
//!
//! These synthesize terminal manifests directly through the sanctioned write
//! path (`run create --skip-materialize` + `node report` + `event create
//! run.status`) rather than spawning real supervisors — the wait loop is a
//! read-only poll of `manifest.status`, so a live supervisor adds nothing but
//! latency and PTY pressure. No `#[file_serial]` gate is needed (no supervisor
//! is spawned); the `TestHome` fixture still reaps any stray process on drop.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};
use tempfile::TempDir;

mod common;
use common::TestHome;

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
    serde_json::from_slice(&out.stdout).expect("stdout is valid JSON")
}

/// Run a command expecting a specific non-zero exit code, returning the parsed
/// stdout JSON (the data envelope — `run wait` emits it even on exit 2/3).
fn run_exit(cmd: &mut Command, want: i32) -> Value {
    let out = cmd.output().expect("spawn");
    let code = out.status.code().expect("exit code");
    assert_eq!(
        code,
        want,
        "want exit {want}, got {code}; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout is valid JSON")
}

/// Run a command expecting an error envelope on stderr at `want` exit.
fn run_err(cmd: &mut Command, want: i32) -> Value {
    let out = cmd.output().expect("spawn");
    let code = out.status.code().expect("exit code");
    assert_eq!(code, want, "want exit {want}, got {code}");
    let stderr = String::from_utf8(out.stderr).expect("utf8");
    let last = stderr.lines().last().expect("stderr has at least one line");
    serde_json::from_str(last).expect("error envelope JSON")
}

fn create(home: &TempDir, kind: &str, title: &str) -> String {
    let v = run_ok(bin(home).args([
        "--output", "json", "run", "create", "--kind", kind, "--title", title,
    ]));
    v["data"]["run_id"]
        .as_str()
        .expect("run_id is string")
        .to_string()
}

/// Append one event via `event create`, writing `data` to a temp file.
fn event_create(home: &TempDir, run_id: &str, kind: &str, node_id: Option<&str>, data: Value) {
    let keep = TempDir::new().unwrap();
    let f = keep.path().join("data.json");
    std::fs::write(&f, serde_json::to_vec(&data).unwrap()).unwrap();
    let mut cmd = bin(home);
    cmd.args([
        "--output", "json", "event", "create", run_id, "--kind", kind,
    ]);
    if let Some(n) = node_id {
        cmd.args(["--node-id", n]);
    }
    cmd.args(["--from-file", f.to_str().unwrap()]);
    run_ok(&mut cmd);
}

fn add_node(home: &TempDir, run_id: &str, node_id: &str) {
    event_create(
        home,
        run_id,
        "node.created",
        Some(node_id),
        json!({ "kind": "spinoff" }),
    );
}

fn node_report(home: &TempDir, run_id: &str, node_id: &str, data: Value) {
    let keep = TempDir::new().unwrap();
    let f = keep.path().join("report.json");
    std::fs::write(&f, serde_json::to_vec(&data).unwrap()).unwrap();
    run_ok(bin(home).args([
        "--output",
        "json",
        "node",
        "report",
        run_id,
        node_id,
        "--from-file",
        f.to_str().unwrap(),
    ]));
}

/// Drive a fresh run to a terminal `manifest.status`, folding the given report
/// into node `n-0001`. `status` is the terminal `run.status` to stamp.
fn settle_run(home: &TempDir, title: &str, status: &str, report: Value) -> String {
    let run_id = create(home, "spinoff", title);
    add_node(home, &run_id, "n-0001");
    node_report(home, &run_id, "n-0001", report);
    event_create(
        home,
        &run_id,
        "run.status",
        None,
        json!({ "status": status }),
    );
    run_id
}

/// Forge a `node.created` for `n-0001` carrying a real worktree path + branch so
/// a stubbed `run merge` can resolve the branch. Mirrors `run_merge.rs`.
fn forge_worker_node(home: &TempDir, run_id: &str, worktree: &Path, branch: &str) {
    let node = home.path().join(format!("node-{run_id}.json"));
    std::fs::write(
        &node,
        serde_json::to_vec(&json!({
            "kind": "spinoff",
            "task": "x",
            "worktree_path": worktree.display().to_string(),
            "branch": branch,
            "tmux_session": "taskfleet",
            "tmux_window_id": "@42",
        }))
        .unwrap(),
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

/// Write an executable no-op merge backend that exits 0. Mirrors `run_merge.rs`.
fn fake_merge_sh(dir: &Path) -> std::path::PathBuf {
    let p = dir.join("fake-merge.sh");
    std::fs::write(&p, "#!/bin/bash\nexit 0\n").unwrap();
    let mut perms = std::fs::metadata(&p).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&p, perms).unwrap();
    p
}

/// Drive a fresh run through a STUBBED `run merge` so it settles `done` with a
/// GENUINE `RunMerge`-origin terminal report (issue `retire-via-string`) — the
/// real merge authority, not a forged `via: "explicit-merge"` string pushed
/// through `node report` (which now strips `via` and stamps an `Agent` origin, so
/// it could never fabricate a merge). `summary` rides `--report-file`. No source
/// repo/branch is wired and the throwaway worktree is dropped before `run wait`
/// runs, so git verification yields nothing and `landed` falls back to the durable
/// merge marker (`report-marker`).
fn settle_merged_run(home: &TempDir, title: &str, summary: &str) -> String {
    let run_id = create(home, "spinoff", title);
    let worktree = TempDir::new().unwrap();
    forge_worker_node(home, &run_id, worktree.path(), "wt/test-merge");
    let scratch = TempDir::new().unwrap();
    let report = scratch.path().join("report.json");
    std::fs::write(
        &report,
        serde_json::to_vec(&json!({ "success": true, "summary": summary })).unwrap(),
    )
    .unwrap();
    let merge_sh = fake_merge_sh(scratch.path());
    run_ok(bin(home).env("TASKFLEET_MERGE_SH", &merge_sh).args([
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
    // Guard the typed merge-authority path: assert the terminal report `run merge`
    // wrote actually carries a `RunMerge` origin, not merely a legacy `via` string.
    // Without this, a regression to via-only stamping would still pass (the helper
    // honors the legacy fallback) and silently defeat the point of this issue.
    let events = home.path().join("runs").join(&run_id).join("events.jsonl");
    let report = std::fs::read_to_string(&events)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|v| v["kind"] == "node.report")
        .expect("a terminal node.report was appended by run merge");
    assert_eq!(
        report["data"]["origin"]["kind"], "run-merge",
        "run merge must stamp a typed RunMerge origin: {report}"
    );

    // `run merge` terminalizes the node but does not roll the run manifest up
    // (no supervisor in this test); stamp the terminal `run.status` explicitly,
    // matching `settle_run`.
    event_create(
        home,
        &run_id,
        "run.status",
        None,
        json!({ "status": "done" }),
    );
    run_id
}

/// A run left at `pending` — created with no node, never settled.
fn pending_run(home: &TempDir, title: &str) -> String {
    create(home, "spinoff", title)
}

#[test]
fn all_happy_path_two_done_runs_exit_zero() {
    let home = TestHome::new();
    // A genuine merge (stubbed `run merge` → `RunMerge` origin), NOT a forged
    // `via` string through `node report` (issue `retire-via-string`).
    let a = settle_merged_run(&home, "a", "did A");
    let b = settle_merged_run(&home, "b", "did B");

    // Default condition is --all; both runs are already terminal.
    let v = run_ok(bin(&home).args(["--output", "json", "run", "wait", &a, &b]));
    assert_eq!(v["data"]["condition"], "all");
    assert_eq!(v["data"]["outcome"], "condition-met");
    let runs = v["data"]["runs"].as_array().expect("runs array");
    assert_eq!(runs.len(), 2);
    for r in runs {
        assert_eq!(r["status"], "done");
        assert_eq!(r["merged"], true);
        // With no source repo/branch to git-verify against, the `landed` signal
        // falls back to the durable merge marker (a `RunMerge`-origin report).
        assert_eq!(r["landed"], true);
        assert_eq!(r["landed_method"], "report-marker");
        assert!(r["summary"].as_str().unwrap().starts_with("did "));
        assert!(r.get("error").is_none(), "done run has no error: {r}");
    }
    assert!(v["data"]["waited_ms"].is_number());
}

#[test]
fn any_returns_when_one_of_two_is_terminal() {
    let home = TestHome::new();
    let done = settle_run(
        &home,
        "done",
        "done",
        json!({ "success": true, "summary": "ok" }),
    );
    // The second run never settles; --any must still return promptly.
    let pending = pending_run(&home, "pending");

    let v = run_ok(bin(&home).args(["--output", "json", "run", "wait", &done, &pending, "--any"]));
    assert_eq!(v["data"]["condition"], "any");
    assert_eq!(v["data"]["outcome"], "condition-met");
    let runs = v["data"]["runs"].as_array().expect("runs array");
    assert_eq!(runs.len(), 2);
    // The first-listed terminal run is reported done; the other is still pending.
    let by_id = |id: &str| runs.iter().find(|r| r["run_id"] == id).unwrap();
    assert_eq!(by_id(&done)["status"], "done");
    assert_eq!(by_id(&pending)["status"], "pending");
}

#[test]
fn timeout_without_terminal_run_exits_two() {
    let home = TestHome::new();
    let pending = pending_run(&home, "pending");
    // Give it a worker node so it reads as genuinely "still working" rather than
    // stillborn (a 0-node run with no supervisor now settles promptly as
    // stalled; this test wants a run that legitimately never settles).
    add_node(&home, &pending, "n-0001");

    // No run will ever settle within the budget → exit 2, with waited_ms ≈ 500.
    let v = run_exit(
        bin(&home).args([
            "--output",
            "json",
            "run",
            "wait",
            &pending,
            "--timeout",
            "500ms",
        ]),
        2,
    );
    assert_eq!(v["data"]["condition"], "all");
    assert_eq!(
        v["data"]["outcome"], "timed-out",
        "the JSON result must carry the loop's timeout decision without relying on exit status"
    );
    let waited = v["data"]["waited_ms"].as_u64().expect("waited_ms u64");
    assert!(
        (400..=2000).contains(&waited),
        "waited_ms {waited} should be ~500ms (the timeout budget)"
    );
    assert_eq!(v["data"]["runs"][0]["status"], "pending");
}

#[test]
fn fresh_tool_telemetry_does_not_satisfy_run_wait() {
    let home = TestHome::new();
    let pending = pending_run(&home, "telemetry-is-not-settlement");
    add_node(&home, &pending, "n-0001");
    run_ok(bin(&home).args([
        "--output",
        "json",
        "node",
        "telemetry",
        "update",
        "--run-id",
        &pending,
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

    let value = run_exit(
        bin(&home).args([
            "--output",
            "json",
            "run",
            "wait",
            &pending,
            "--timeout",
            "500ms",
        ]),
        2,
    );
    assert_eq!(value["data"]["condition"], "all");
    assert_eq!(value["data"]["runs"][0]["status"], "pending");
    assert!(value["data"]["waited_ms"].as_u64().unwrap() >= 400);
}

#[test]
fn fail_on_error_with_failed_run_exits_three() {
    let home = TestHome::new();
    let failed = settle_run(
        &home,
        "failed",
        "failed",
        json!({ "success": false, "summary": "blew up" }),
    );

    // Condition is met (the run is terminal) but it failed → exit 3.
    let v = run_exit(
        bin(&home).args([
            "--output",
            "json",
            "run",
            "wait",
            &failed,
            "--fail-on-error",
        ]),
        3,
    );
    assert_eq!(v["data"]["runs"][0]["status"], "failed");

    // Without --fail-on-error the same terminal state is a plain success (exit 0).
    let v = run_ok(bin(&home).args(["--output", "json", "run", "wait", &failed]));
    assert_eq!(v["data"]["runs"][0]["status"], "failed");
}

#[test]
fn unknown_run_id_exits_one() {
    let home = TestHome::new();
    // Well-formed ULID that names no run on disk.
    let ghost = "01arz3ndektsv4rrffq69g5fav";
    let v = run_err(
        bin(&home).args(["--output", "json", "run", "wait", ghost]),
        1,
    );
    assert_eq!(v["error"]["code"], "unknown_run");
    assert_eq!(v["error"]["invalid_value"], ghost);
}

#[test]
fn malformed_timeout_is_rejected_up_front() {
    let home = TestHome::new();
    let pending = pending_run(&home, "pending");
    let v = run_err(
        bin(&home).args([
            "--output",
            "json",
            "run",
            "wait",
            &pending,
            "--timeout",
            "soon",
        ]),
        1,
    );
    assert_eq!(v["error"]["code"], "invalid_arguments");
}

/// End-to-end proof of the issue's fix: `run wait` reports `landed: true`
/// (git-verified) even after the caller rebases local `main` so the worker's
/// merge is replayed under a new hash — the exact case where
/// `git merge-base --is-ancestor <worker-branch> main` lies.
#[test]
fn wait_reports_landed_git_verified_after_caller_rebase() {
    use std::process::Command;

    fn git(repo: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    // Build a repo whose worker branch merged into main, then rebase local main
    // (replaying the merge under a new hash; the worker branch ref stays put).
    let repo_dir = TempDir::new().unwrap();
    let repo = repo_dir.path();
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["config", "user.email", "t@t"]);
    git(repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("f"), "base\n").unwrap();
    git(repo, &["add", "f"]);
    git(repo, &["commit", "-qm", "base"]);
    let base = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["checkout", "-q", "-b", "wt/worker"]);
    std::fs::write(repo.join("f"), "base\nwork\n").unwrap();
    git(repo, &["commit", "-qam", "worker change"]);
    git(repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("g"), "other\n").unwrap();
    git(repo, &["add", "g"]);
    git(repo, &["commit", "-qm", "other session"]);
    let worker_tip = git(repo, &["rev-parse", "wt/worker"]);
    git(repo, &["checkout", "-q", "-b", "replay", &worker_tip]);
    git(repo, &["rebase", "-q", "main"]);
    git(repo, &["checkout", "-q", "main"]);
    git(repo, &["merge", "-q", "--ff-only", "replay"]);
    git(repo, &["branch", "-q", "-D", "replay"]);
    git(repo, &["checkout", "-q", "-b", "tmp", &base]);
    std::fs::write(repo.join("h"), "upstream\n").unwrap();
    git(repo, &["add", "h"]);
    git(repo, &["commit", "-qm", "origin moved"]);
    git(repo, &["checkout", "-q", "main"]);
    git(repo, &["rebase", "-q", "tmp"]);

    // Precondition: the ancestry check the old skill guidance used now lies.
    let is_ancestor = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--is-ancestor", "wt/worker", "main"])
        .status()
        .unwrap()
        .success();
    assert!(
        !is_ancestor,
        "the rebase-replay case must make --is-ancestor lie"
    );

    // Wire the run to that repo: source_repo + source_branch on the manifest,
    // branch + base_sha on the node. No explicit-merge marker on the report, so
    // `landed` can only come from git verification.
    let home = TestHome::new();
    let run_id = run_ok(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "rebase-landed",
        "--source-repo",
        repo.to_str().unwrap(),
        "--source-branch",
        "main",
    ]))["data"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    event_create(
        &home,
        &run_id,
        "node.created",
        Some("n-0001"),
        json!({
            "kind": "spinoff",
            "branch": "wt/worker",
            "base_sha": base,
            "worktree_path": repo.to_str().unwrap(),
        }),
    );
    node_report(
        &home,
        &run_id,
        "n-0001",
        json!({ "success": true, "summary": "landed via rebase-replay" }),
    );
    event_create(
        &home,
        &run_id,
        "run.status",
        None,
        json!({ "status": "done" }),
    );

    let v = run_ok(bin(&home).args(["--output", "json", "run", "wait", &run_id]));
    let r = &v["data"]["runs"][0];
    assert_eq!(r["status"], "done");
    assert_eq!(
        r["landed"], true,
        "content is merged → landed must be true despite the rebase: {r}"
    );
    assert_eq!(
        r["landed_method"], "git-verified",
        "the landing is confirmed by patch-id, not the report marker: {r}"
    );
    // No explicit-merge marker → the report-based `merged` flag stays false,
    // proving `landed` is independently git-derived.
    assert_eq!(r["merged"], false);
}

/// A stillborn run — created, but its supervisor died before spawning any node
/// (no supervisor pid, 0 nodes, `updated_at == created_at`) — settles the wait
/// promptly instead of blocking the whole timeout, and reports `stalled: true`.
/// This is the core of issue `run-wait-stillborn-run-not-detected` (a real
/// incident blocked `run wait` for ~6h).
#[test]
fn stillborn_run_settles_promptly_as_stalled() {
    let home = TestHome::new();
    // `run create` under TASKFLEET_TEST_SKIP_MATERIALIZE spawns no supervisor, so the
    // fresh run has the exact stillborn shape (pending, 0 nodes, no supervisor,
    // updated_at == created_at).
    let born = pending_run(&home, "stillborn");

    // A generous timeout the wait must NOT consume: a stillborn run is detected
    // on the first poll, so it returns far sooner than the budget.
    let start = std::time::Instant::now();
    let v = run_ok(bin(&home).args(["--output", "json", "run", "wait", &born, "--timeout", "30s"]));
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "stillborn run must settle promptly, took {elapsed:?}"
    );
    let r = &v["data"]["runs"][0];
    // Status is still `pending` (the run never started) — the `stalled` flag is
    // what tells the caller it is dead, not slow.
    assert_eq!(r["status"], "pending");
    assert_eq!(r["stalled"], true, "stillborn run must be stalled: {r}");
    // A structured reason lets a JSON grader tell "supervisor never started"
    // from "worker failed" without re-deriving it.
    assert_eq!(
        r["error"], "supervisor died before creating any worker node",
        "stillborn outcome carries a structured reason: {r}"
    );
    let waited = v["data"]["waited_ms"].as_u64().expect("waited_ms u64");
    assert!(
        waited < 5000,
        "waited_ms {waited} should be well under the 30s budget"
    );
}

/// Under `--fail-on-error`, a stillborn run grades as a failure (exit 3) even
/// though its status is still `pending` — a caller that relies on the exit code
/// must not mistake a dead run for a clean completion. Without the flag the same
/// shape settles as a plain success (exit 0), carrying `stalled: true` for a
/// caller that inspects the envelope.
#[test]
fn stillborn_run_fail_on_error_exits_three() {
    let home = TestHome::new();
    let born = pending_run(&home, "stillborn-fail");

    let v = run_exit(
        bin(&home).args([
            "--output",
            "json",
            "run",
            "wait",
            &born,
            "--fail-on-error",
            "--timeout",
            "30s",
        ]),
        3,
    );
    assert_eq!(v["data"]["runs"][0]["stalled"], true);
    assert_eq!(v["data"]["runs"][0]["status"], "pending");
    assert_eq!(
        v["data"]["runs"][0]["error"],
        "supervisor died before creating any worker node"
    );

    // Same run, no --fail-on-error → exit 0, but still flagged stalled.
    let v = run_ok(bin(&home).args(["--output", "json", "run", "wait", &born, "--timeout", "30s"]));
    assert_eq!(v["data"]["runs"][0]["stalled"], true);
}

/// Rewrite a run manifest's `updated_at` to `minutes_ago` before now, leaving
/// every other field intact. Used to age a `node_count > 0` run past the orphan
/// grace window without waiting real time — a legitimate synthesis (the wait
/// loop only *reads* the manifest under a shared lock; no reducer replay runs on
/// a read, so the backdated clock is what the poll observes).
fn backdate_manifest_updated_at(home: &TempDir, run_id: &str, minutes_ago: i64) {
    let path = home.path().join("runs").join(run_id).join("manifest.json");
    let mut m: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read manifest")).expect("parse");
    let old = chrono::Utc::now() - chrono::Duration::minutes(minutes_ago);
    // Match the manifest's RFC3339 serialization (`chrono` `DateTime<Utc>`).
    m["updated_at"] = json!(old.to_rfc3339_opts(chrono::SecondsFormat::Micros, true));
    std::fs::write(&path, serde_json::to_vec(&m).expect("serialize")).expect("write manifest");
}

/// An *orphaned* run — its supervisor created a worker node (`node_count > 0`)
/// then died mid-run, leaving the run `pending` with a dead supervisor and a
/// manifest clock idle past the grace window — settles the wait promptly as
/// `stalled` instead of blocking the whole timeout, and carries the
/// orphaned-specific reason. This is the core of issue `run-wait-still`, the
/// sibling the stillborn fix (`node_count == 0`) scoped out.
#[test]
fn orphaned_run_settles_promptly_as_stalled() {
    let home = TestHome::new();
    // Create a run and give it a worker node so `node_count > 0` (not stillborn).
    // Under SKIP_MATERIALIZE no supervisor is spawned, so it reads as dead.
    let run = pending_run(&home, "orphaned");
    add_node(&home, &run, "n-0001");
    // Age the manifest clock well past the 15-minute orphan grace so the poll
    // sees a genuinely stranded run rather than a transiently-idle one.
    backdate_manifest_updated_at(&home, &run, 30);

    let start = std::time::Instant::now();
    let v = run_ok(bin(&home).args(["--output", "json", "run", "wait", &run, "--timeout", "30s"]));
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "orphaned run must settle promptly, took {elapsed:?}"
    );
    let r = &v["data"]["runs"][0];
    // Status is still `pending` (the run never reached a terminal state) — the
    // `stalled` flag is what tells the caller it is stranded, not still working.
    assert_eq!(r["status"], "pending");
    assert_eq!(r["stalled"], true, "orphaned run must be stalled: {r}");
    assert_eq!(
        r["error"], "supervisor died mid-run; work is stranded and cannot be rolled up",
        "orphaned outcome carries the mid-run reason, distinct from stillborn: {r}"
    );
    let waited = v["data"]["waited_ms"].as_u64().expect("waited_ms u64");
    assert!(
        waited < 5000,
        "waited_ms {waited} well under the 30s budget"
    );
}

/// The grace-window guard, end-to-end: a `node_count > 0` run with a dead
/// supervisor but a FRESH manifest clock (no backdating) is NOT treated as
/// orphaned — it must block the full budget and time out, because a supervisor
/// caught mid-reattach/restart would present this exact shape. (Complements the
/// unit-level boundary tests in `run::stalled`.) This mirrors — and pins the
/// grace-guard reason for — `timeout_without_terminal_run_exits_two`.
#[test]
fn recently_active_dead_supervisor_run_does_not_settle_early() {
    let home = TestHome::new();
    let run = pending_run(&home, "fresh-orphan-candidate");
    add_node(&home, &run, "n-0001"); // node_count > 0, clock ≈ now

    let v = run_exit(
        bin(&home).args([
            "--output",
            "json",
            "run",
            "wait",
            &run,
            "--timeout",
            "500ms",
        ]),
        2,
    );
    assert_eq!(v["data"]["runs"][0]["status"], "pending");
    assert_eq!(
        v["data"]["runs"][0]["stalled"], false,
        "within the grace window a dead-supervisor run must NOT be flagged stalled"
    );
    let waited = v["data"]["waited_ms"].as_u64().expect("waited_ms u64");
    assert!(
        (400..=2000).contains(&waited),
        "waited_ms {waited} should be ~500ms (the timeout budget, not an early exit)"
    );
}

#[test]
fn all_and_any_are_mutually_exclusive() {
    let home = TestHome::new();
    let pending = pending_run(&home, "pending");
    let out = bin(&home)
        .args(["run", "wait", &pending, "--all", "--any"])
        .output()
        .expect("spawn");
    assert_eq!(
        out.status.code(),
        Some(1),
        "conflicting flags are a usage error"
    );
}

/// Stamp a clean (`code: 0`) `worker_exit` fact onto `n-0001`'s projection — the
/// durable shape the launcher shim's `worker.exited` fold produces for a worker
/// that finished normally. Leaves the node (and the run) non-terminal: no
/// `node.report`, no terminal `run.status`. Patched directly on the projection
/// file (the `worker.exited` event kind is shim-only and not routable through
/// `event create`), mirroring `backdate_manifest_updated_at`; the wait loop only
/// *reads* the node under a shared lock, so no reducer replay clobbers the patch.
fn stamp_clean_worker_exit(home: &TempDir, run_id: &str) {
    let path = home
        .path()
        .join("runs")
        .join(run_id)
        .join("nodes")
        .join("n-0001.json");
    let mut n: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read node")).expect("parse node");
    n.as_object_mut().expect("node object").insert(
        "worker_exit".into(),
        json!({ "code": 0, "signal": null, "at": "2026-08-15T10:00:00Z" }),
    );
    std::fs::write(&path, serde_json::to_vec(&n).expect("serialize node")).expect("write node");
}

/// An *attention-required* run — its worker exited cleanly but skipped
/// `run merge`, so the node stays non-terminal — settles the wait promptly with
/// `attention_required: true` (design.md §2.5 / A5) instead of blocking the whole
/// timeout, and NEVER mutates the run to a terminal status.
#[test]
fn attention_required_run_settles_promptly_without_terminalizing() {
    let home = TestHome::new();
    let run = pending_run(&home, "attention");
    add_node(&home, &run, "n-0001");
    stamp_clean_worker_exit(&home, &run);

    let start = std::time::Instant::now();
    let v = run_ok(bin(&home).args(["--output", "json", "run", "wait", &run, "--timeout", "30s"]));
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "attention-required run must settle promptly, took {elapsed:?}"
    );
    let r = &v["data"]["runs"][0];
    // Non-terminal: the run is still `pending`. The `attention_required` flag is
    // what tells the caller the worker finished but skipped `run merge`.
    assert_eq!(r["status"], "pending", "run must NOT be terminalized: {r}");
    assert_eq!(
        r["attention_required"], true,
        "clean-exit-no-merge run must be attention_required: {r}"
    );
    assert_eq!(
        r["stalled"], false,
        "attention-required is distinct from a supervisor-death stall: {r}"
    );
    assert_eq!(
        r["error"], "worker exited cleanly without running `run merge`",
        "attention outcome carries its own reason, distinct from a stall: {r}"
    );

    // The run status on disk is untouched — the wait mutated nothing.
    let show = run_ok(bin(&home).args(["--output", "json", "run", "show", &run]));
    assert_eq!(
        show["data"]["status"], "pending",
        "run wait must not have mutated the run terminal"
    );
}

/// An explicit awaiting-input marker is visible on show/list immediately and,
/// once its grace elapses, settles `run wait` without terminalizing the run.
#[test]
fn awaiting_input_surfaces_and_settles_after_grace() {
    let home = TestHome::new();
    let run = pending_run(&home, "awaiting-input");
    add_node(&home, &run, "n-0001");
    event_create(
        &home,
        &run,
        "node.awaiting_input",
        Some("n-0001"),
        json!({ "discussion_items": [{
            "topic": "Choose scope",
            "options": ["small", "large"],
            "recommended_default": "small"
        }] }),
    );

    let show = run_ok(bin(&home).args(["--output", "json", "run", "show", &run]));
    assert_eq!(show["data"]["awaiting_input"], true);
    assert_eq!(show["data"]["open_discussion_count"], 1);
    assert_eq!(
        show["data"]["awaiting_input_detail"]["discussion_items"][0]["recommended_default"],
        "small"
    );
    let list = run_ok(bin(&home).args(["--output", "json", "run", "list"]));
    let row = list["data"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["run_id"] == run)
        .unwrap();
    assert_eq!(row["awaiting_input"], true);
    assert_eq!(row["open_discussion_count"], 1);

    let out = run_ok(
        bin(&home)
            .env("TASKFLEET_AWAITING_INPUT_GRACE_SECS", "0")
            .args(["--output", "json", "run", "wait", &run, "--timeout", "2s"]),
    );
    let waited = &out["data"]["runs"][0];
    assert_eq!(waited["status"], "pending");
    assert_eq!(waited["awaiting_input"], true);
    assert_eq!(waited["awaiting_input_detail"]["open_discussion_count"], 1);
}

/// Under `--fail-on-error`, an attention-required run grades as a failure
/// (exit 3) even though its status is still `pending` — a caller that treats a
/// met-but-not-`done` wait as failure notices the skipped merge.
#[test]
fn attention_required_run_fail_on_error_exits_three() {
    let home = TestHome::new();
    let run = pending_run(&home, "attention-fail");
    add_node(&home, &run, "n-0001");
    stamp_clean_worker_exit(&home, &run);

    let v = run_exit(
        bin(&home).args([
            "--output",
            "json",
            "run",
            "wait",
            &run,
            "--timeout",
            "30s",
            "--fail-on-error",
        ]),
        3,
    );
    let r = &v["data"]["runs"][0];
    assert_eq!(r["status"], "pending");
    assert_eq!(r["attention_required"], true);
}

/// Precedence: a run whose worker exited cleanly AND whose supervisor died
/// mid-run (the orphaned shape — dead supervisor, node, idle clock) is reported
/// `attention_required`, NOT `stalled`. The told clean-exit fact is the more
/// specific truth and the correct remediation is the manual finish (`run merge`),
/// not `run reattach`.
#[test]
fn attention_wins_over_orphaned_stall() {
    let home = TestHome::new();
    let run = pending_run(&home, "attention-vs-orphan");
    add_node(&home, &run, "n-0001");
    stamp_clean_worker_exit(&home, &run);
    // Age the clock past the orphan grace so, absent the clean exit, this would
    // classify as an orphaned stall.
    backdate_manifest_updated_at(&home, &run, 30);

    let v = run_ok(bin(&home).args(["--output", "json", "run", "wait", &run, "--timeout", "30s"]));
    let r = &v["data"]["runs"][0];
    assert_eq!(
        r["attention_required"], true,
        "clean exit wins over the orphaned-stall shape: {r}"
    );
    assert_eq!(
        r["stalled"], false,
        "attention-required must suppress the stall verdict: {r}"
    );
}
