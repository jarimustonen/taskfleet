//! Integration tests for `taskfleet run salvage` (design.md §2.2 / A3,
//! issue `run-salvage-command`).
//!
//! The fenced manual finish: for an `attention-required` run (a worker that
//! exited cleanly but skipped `run merge`) or a `failed`/stuck single-worker
//! run, salvage verifies the prior worker's state, fences a live one, and drives
//! `run merge` from the preserved worktree. These tests seed run state directly
//! via the core append path (no live supervisor, deterministic) and stub the
//! merge backend via `TASKFLEET_MERGE_SH`, exactly like `run_merge.rs`.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};
use taskfleet_core::{append_and_apply_event, ensure_root, NodeId, RunPaths};
use tempfile::TempDir;

fn bin(home: &TempDir) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    c.env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path());
    c
}

fn node_id() -> NodeId {
    NodeId::parse_str("n-0001").unwrap()
}

/// Seed a run dir with a `run.created` (autonomous spinoff) and return its
/// `RunPaths`. `run_id` is a fresh ULID so tests never collide.
fn seed_run(home: &Path, run_id: &str) -> RunPaths {
    ensure_root(home).unwrap();
    let dir = home.join("runs").join(run_id);
    std::fs::create_dir_all(&dir).unwrap();
    let paths = RunPaths::new(dir, run_id).unwrap();
    append_and_apply_event(
        &paths,
        "run.created",
        None,
        None,
        json!({ "kind": "spinoff", "lifecycle": "autonomous", "title": "salvage-test" }),
    )
    .unwrap();
    paths
}

/// Add a worker node `n-0001` carrying `worktree_path` + `branch` (and any extra
/// `node.created` data fields via `extra`).
fn add_worker_node(paths: &RunPaths, worktree: Option<&Path>, branch: Option<&str>, extra: Value) {
    let mut data = json!({ "kind": "spinoff" });
    let obj = data.as_object_mut().unwrap();
    if let Some(wt) = worktree {
        obj.insert("worktree_path".into(), json!(wt.display().to_string()));
    }
    if let Some(b) = branch {
        obj.insert("branch".into(), json!(b));
    }
    if let Some(e) = extra.as_object() {
        for (k, v) in e {
            obj.insert(k.clone(), v.clone());
        }
    }
    append_and_apply_event(paths, "node.created", Some(&node_id()), None, data).unwrap();
}

/// Record a clean `worker.exited` (exit 0) — the attention-required signature.
fn record_clean_exit(paths: &RunPaths) {
    append_and_apply_event(
        paths,
        "worker.exited",
        Some(&node_id()),
        None,
        json!({ "exit_code": 0 }),
    )
    .unwrap();
}

/// Write an executable fake merge backend that exits `code`.
fn fake_merge_sh(dir: &Path, code: i32) -> std::path::PathBuf {
    let p = dir.join("fake-merge.sh");
    std::fs::write(&p, format!("#!/bin/bash\nexit {code}\n")).unwrap();
    let mut perms = std::fs::metadata(&p).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&p, perms).unwrap();
    p
}

fn read_events(paths: &RunPaths) -> Vec<Value> {
    std::fs::read_to_string(paths.events())
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .collect()
}

fn fresh_run_id() -> String {
    taskfleet_core::new_run_id()
}

fn git(cwd: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// Create a real repository with `main` as the target worktree and `wt/dry` as
/// the worker. Dry-run target checks intentionally exercise only declared Git,
/// not ambient tmux/workmux/taskfleet installations.
fn repo_with_worker(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let repo = root.join("repo");
    let worker = root.join("worker");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.invalid"]);
    git(&repo, &["config", "user.name", "Taskfleet Test"]);
    std::fs::write(repo.join("tracked.txt"), "base\n").unwrap();
    git(&repo, &["add", "tracked.txt"]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt/dry",
            worker.to_str().unwrap(),
        ],
    );
    (repo, worker)
}

/// Run `run salvage` and return the parsed error envelope (asserting failure).
fn salvage_err(cmd: &mut Command) -> Value {
    let out = cmd.output().expect("spawn");
    assert!(
        !out.status.success(),
        "expected failure, got success; stdout={}",
        String::from_utf8_lossy(&out.stdout)
    );
    serde_json::from_slice(&out.stderr).expect("stderr is a JSON error envelope")
}

/// Happy path: an attention-required run (worker exited cleanly, node still
/// non-terminal) is finished by salvage — no fence, a real merge, a terminal
/// explicit-merge report.
#[test]
fn attention_required_run_is_finished() {
    let home = TempDir::new().unwrap();
    let scratch = TempDir::new().unwrap();
    let gitroot = TempDir::new().unwrap();
    let (_repo, worktree) = repo_with_worker(gitroot.path());
    let run_id = fresh_run_id();
    let paths = seed_run(home.path(), &run_id);
    add_worker_node(&paths, Some(&worktree), Some("wt/dry"), json!({}));
    record_clean_exit(&paths);

    let merge_sh = fake_merge_sh(scratch.path(), 0);
    let out = bin(&home)
        .env("TASKFLEET_MERGE_SH", &merge_sh)
        .args([
            "--output", "json", "run", "salvage", &run_id, "--source", "main",
        ])
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["data"]["worker_state"], "exited");
    assert_eq!(v["data"]["fenced"], false);
    assert_eq!(v["data"]["merge"]["merged"], true);
    assert_eq!(v["data"]["merge"]["branch"], "wt/dry");

    // Exactly one terminal report, stamped explicit-merge.
    let reports: Vec<Value> = read_events(&paths)
        .into_iter()
        .filter(|e| e["kind"] == "node.report")
        .collect();
    assert_eq!(reports.len(), 1, "one terminal node.report");
    assert_eq!(reports[0]["data"]["via"], "explicit-merge");
    assert_eq!(reports[0]["data"]["success"], true);
}

/// `--dry-run` reports the plan (worker state, no fence, planned merge) and
/// appends NOTHING — no merge, no report.
#[test]
fn dry_run_previews_without_mutating() {
    let home = TempDir::new().unwrap();
    let gitroot = TempDir::new().unwrap();
    let (repo, worktree) = repo_with_worker(gitroot.path());
    let run_id = fresh_run_id();
    let paths = seed_run(home.path(), &run_id);
    add_worker_node(&paths, Some(&worktree), Some("wt/dry"), json!({}));
    record_clean_exit(&paths);

    let events_before = std::fs::read(paths.events()).unwrap();
    let refs_before = git(&repo, &["show-ref"]);
    let target_status_before = git(&repo, &["status", "--porcelain", "--untracked-files=all"]);
    let worker_status_before = git(
        &worktree,
        &["status", "--porcelain", "--untracked-files=all"],
    );
    let out = bin(&home)
        .args([
            "--output",
            "json",
            "run",
            "salvage",
            &run_id,
            "--source",
            "main",
            "--dry-run",
        ])
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["data"]["dry_run"], true);
    assert_eq!(v["data"]["merge"]["merged"], false);
    assert_eq!(std::fs::read(paths.events()).unwrap(), events_before);
    assert_eq!(git(&repo, &["show-ref"]), refs_before);
    assert_eq!(
        git(&repo, &["status", "--porcelain", "--untracked-files=all"]),
        target_status_before
    );
    assert_eq!(
        git(
            &worktree,
            &["status", "--porcelain", "--untracked-files=all"],
        ),
        worker_status_before
    );
}

/// Regression: dry-run auto-detects main and rejects unrelated uncommitted work
/// in that target with the same stable error class/message as the real backend,
/// without changing refs, worktree contents, or run events.
#[test]
fn dry_run_refuses_dirty_auto_detected_target_without_mutating() {
    let home = TempDir::new().unwrap();
    let gitroot = TempDir::new().unwrap();
    let (repo, worktree) = repo_with_worker(gitroot.path());
    let run_id = fresh_run_id();
    let paths = seed_run(home.path(), &run_id);
    add_worker_node(&paths, Some(&worktree), Some("wt/dry"), json!({}));
    record_clean_exit(&paths);
    std::fs::write(repo.join("UNRELATED.txt"), "do not merge over me\n").unwrap();

    let events_before = std::fs::read(paths.events()).unwrap();
    let refs_before = git(&repo, &["show-ref"]);
    let target_status_before = git(&repo, &["status", "--porcelain", "--untracked-files=all"]);
    let v =
        salvage_err(bin(&home).args(["--output", "json", "run", "salvage", &run_id, "--dry-run"]));

    assert_eq!(v["error"]["code"], "merge_failed");
    assert_eq!(v["error"]["invalid_value"], "wt/dry");
    let message = v["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("Uncommitted changes in target worktree"));
    assert!(message.contains("Please commit or stash changes in the target before merging"));
    assert_eq!(std::fs::read(paths.events()).unwrap(), events_before);
    assert_eq!(git(&repo, &["show-ref"]), refs_before);
    assert_eq!(
        git(&repo, &["status", "--porcelain", "--untracked-files=all"]),
        target_status_before
    );
}

/// A run that already merged (`done`) has nothing to salvage — refuse.
#[test]
fn refuses_done_run() {
    let home = TempDir::new().unwrap();
    let worktree = TempDir::new().unwrap();
    let run_id = fresh_run_id();
    let paths = seed_run(home.path(), &run_id);
    add_worker_node(&paths, Some(worktree.path()), Some("wt/done"), json!({}));
    append_and_apply_event(
        &paths,
        "run.status",
        None,
        None,
        json!({ "status": "done" }),
    )
    .unwrap();

    let v = salvage_err(bin(&home).args(["--output", "json", "run", "salvage", &run_id]));
    assert_eq!(v["error"]["code"], "run_already_terminal");
}

/// A cancelled run never adopts a merge — refuse.
#[test]
fn refuses_cancelled_run() {
    let home = TempDir::new().unwrap();
    let worktree = TempDir::new().unwrap();
    let run_id = fresh_run_id();
    let paths = seed_run(home.path(), &run_id);
    add_worker_node(&paths, Some(worktree.path()), Some("wt/c"), json!({}));
    append_and_apply_event(
        &paths,
        "run.status",
        None,
        None,
        json!({ "status": "cancelled" }),
    )
    .unwrap();

    let v = salvage_err(bin(&home).args(["--output", "json", "run", "salvage", &run_id]));
    assert_eq!(v["error"]["code"], "run_already_terminal");
}

/// A multi-node (fan-out-shaped) run is ambiguous — refuse (per-node salvage is
/// a follow-up).
#[test]
fn refuses_multi_node_run() {
    let home = TempDir::new().unwrap();
    let worktree = TempDir::new().unwrap();
    let run_id = fresh_run_id();
    let paths = seed_run(home.path(), &run_id);
    add_worker_node(&paths, Some(worktree.path()), Some("wt/a"), json!({}));
    append_and_apply_event(
        &paths,
        "node.created",
        Some(&NodeId::parse_str("n-0002").unwrap()),
        None,
        json!({ "kind": "spinoff", "worktree_path": worktree.path().display().to_string(), "branch": "wt/b" }),
    )
    .unwrap();

    let v = salvage_err(bin(&home).args(["--output", "json", "run", "salvage", &run_id]));
    assert_eq!(v["error"]["code"], "ambiguous_multi_node");
}

/// A node with no preserved worktree cannot be salvaged — refuse.
#[test]
fn refuses_run_without_worktree() {
    let home = TempDir::new().unwrap();
    let run_id = fresh_run_id();
    let paths = seed_run(home.path(), &run_id);
    add_worker_node(&paths, None, Some("wt/no-wt"), json!({}));
    record_clean_exit(&paths);

    let v = salvage_err(bin(&home).args(["--output", "json", "run", "salvage", &run_id]));
    assert_eq!(v["error"]["code"], "no_worktree");
}

/// A recorded worktree that no longer exists on disk — refuse with a distinct,
/// actionable code (not a misleading merge-spawn failure).
#[test]
fn refuses_torn_down_worktree() {
    let home = TempDir::new().unwrap();
    let run_id = fresh_run_id();
    let paths = seed_run(home.path(), &run_id);
    add_worker_node(
        &paths,
        Some(Path::new("/nonexistent/salvage/worktree")),
        Some("wt/gone"),
        json!({}),
    );
    record_clean_exit(&paths);

    let v = salvage_err(bin(&home).args(["--output", "json", "run", "salvage", &run_id]));
    assert_eq!(v["error"]["code"], "worktree_missing");
}

/// A live worker whose identity cannot be verified (a live pid with no recorded
/// start-time) is never fenced — refuse, even though it looks alive. Uses a real
/// long-lived child process so the pid is genuinely alive at check time.
#[test]
fn refuses_unverifiable_live_worker() {
    let home = TempDir::new().unwrap();
    let worktree = TempDir::new().unwrap();
    let run_id = fresh_run_id();
    let paths = seed_run(home.path(), &run_id);

    // Spawn a real child that stays alive for the duration of the check, so the
    // recorded agent_pid is genuinely live. No agent_pid_start_time recorded →
    // identity is unverifiable.
    let mut child = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep");
    let pid = child.id();
    add_worker_node(
        &paths,
        Some(worktree.path()),
        Some("wt/live"),
        json!({ "agent_pid": pid }),
    );

    let v =
        salvage_err(bin(&home).args(["--output", "json", "run", "salvage", &run_id, "--fence"]));
    assert_eq!(v["error"]["code"], "worker_unfenceable");

    let _ = child.kill();
    let _ = child.wait();
}

/// A never-started run (pending, worker never got a pid, no worker exit) has no
/// work to salvage — refuse with a precise reason.
#[test]
fn refuses_never_started_pending_run() {
    let home = TempDir::new().unwrap();
    let worktree = TempDir::new().unwrap();
    let run_id = fresh_run_id();
    let paths = seed_run(home.path(), &run_id);
    // Worktree + branch present, but no agent_pid and no worker.exited → NoPid,
    // and the run is still Pending (default after seed).
    add_worker_node(&paths, Some(worktree.path()), Some("wt/pending"), json!({}));

    let v = salvage_err(bin(&home).args(["--output", "json", "run", "salvage", &run_id]));
    assert_eq!(v["error"]["code"], "run_not_started");
}

/// A `done` run whose worktree still exists (a crash between the merge report and
/// teardown) points the operator at `run reattach`, not a re-merge.
#[test]
fn done_run_with_live_worktree_points_at_reattach() {
    let home = TempDir::new().unwrap();
    let worktree = TempDir::new().unwrap();
    let run_id = fresh_run_id();
    let paths = seed_run(home.path(), &run_id);
    add_worker_node(&paths, Some(worktree.path()), Some("wt/done-wt"), json!({}));
    append_and_apply_event(
        &paths,
        "run.status",
        None,
        None,
        json!({ "status": "done" }),
    )
    .unwrap();

    let v = salvage_err(bin(&home).args(["--output", "json", "run", "salvage", &run_id]));
    assert_eq!(v["error"]["code"], "run_already_terminal");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("run reattach"),
        "done-with-worktree must point at reattach: {}",
        v["error"]["message"]
    );
}

/// An unknown run id is a friendly `run_not_found`, not a system error.
#[test]
fn refuses_unknown_run() {
    let home = TempDir::new().unwrap();
    ensure_root(home.path()).unwrap();
    let run_id = fresh_run_id();
    let v = salvage_err(bin(&home).args(["--output", "json", "run", "salvage", &run_id]));
    assert_eq!(v["error"]["code"], "run_not_found");
}
