//! Exact worker-worktree ownership discovery (`run show --current`).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use taskfleet_core::{append_and_apply_event, ensure_root, NodeId, RunPaths};
use tempfile::TempDir;

const RUN_A: &str = "01m08c08v5jxzfqf3r36n0sgzd";
const RUN_B: &str = "01m08c08v5422jae649kmwewy9";
const OLD_PREFIX: &str = "01m08c08v5";

fn bin(home: &TempDir, cwd: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    command
        .env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path())
        .current_dir(cwd);
    command
}

fn worktree(root: &Path, name: &str, branch: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::create_dir_all(path.join(".git")).unwrap();
    std::fs::write(
        path.join(".git/HEAD"),
        format!("ref: refs/heads/{branch}\n"),
    )
    .unwrap();
    path
}

fn git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", cwd)
        .status()
        .expect("git is a declared test dependency");
    assert!(status.success(), "git {args:?} failed");
}

/// Create the production linked-worktree shape: each checkout has a `.git`
/// marker file pointing at `<main>/.git/worktrees/<name>`.
fn linked_worktrees(root: &Path, branch_a: &str, branch_b: &str) -> (PathBuf, PathBuf) {
    let main = root.join("main");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q"]);
    git(&main, &["config", "user.email", "tests@example.invalid"]);
    git(&main, &["config", "user.name", "Ownership Test"]);
    std::fs::write(main.join("seed"), "seed\n").unwrap();
    git(&main, &["add", "seed"]);
    git(&main, &["commit", "-q", "-m", "seed"]);
    git(&main, &["branch", branch_a]);
    git(&main, &["branch", branch_b]);
    let a = root.join("alpha");
    let b = root.join("beta");
    git(
        &main,
        &["worktree", "add", "-q", a.to_str().unwrap(), branch_a],
    );
    git(
        &main,
        &["worktree", "add", "-q", b.to_str().unwrap(), branch_b],
    );
    assert!(a.join(".git").is_file());
    assert!(b.join(".git").is_file());
    (a, b)
}

fn seed_run(home: &Path, run_id: &str, worktree: &Path, branch: &str) {
    ensure_root(home).unwrap();
    let dir = home.join("runs").join(run_id);
    std::fs::create_dir_all(&dir).unwrap();
    let paths = RunPaths::new(dir, run_id).unwrap();
    append_and_apply_event(
        &paths,
        "run.created",
        None,
        None,
        json!({ "kind": "spinoff", "lifecycle": "autonomous", "title": branch }),
    )
    .unwrap();
    append_and_apply_event(
        &paths,
        "node.created",
        Some(&NodeId::parse_str("n-0001").unwrap()),
        None,
        json!({
            "kind": "spinoff",
            "branch": branch,
            "worktree_path": worktree,
        }),
    )
    .unwrap();
}

fn show_current(home: &TempDir, cwd: &Path) -> std::process::Output {
    bin(home, cwd)
        .args(["run", "show", "--current", "--output", "json"])
        .output()
        .unwrap()
}

fn error_code(output: &std::process::Output) -> String {
    let value: Value = serde_json::from_slice(&output.stderr).unwrap();
    value["error"]["code"].as_str().unwrap().to_string()
}

#[test]
fn each_worktree_resolves_only_its_full_run_id_when_old_prefix_collides() {
    assert_eq!(&RUN_A[..10], OLD_PREFIX);
    assert_eq!(&RUN_B[..10], OLD_PREFIX);

    let home = TempDir::new().unwrap();
    let repos = TempDir::new().unwrap();
    let branch_a = format!("wt/{OLD_PREFIX}-alpha");
    let branch_b = format!("wt/{OLD_PREFIX}-beta");
    let (worktree_a, worktree_b) = linked_worktrees(repos.path(), &branch_a, &branch_b);
    seed_run(home.path(), RUN_A, &worktree_a, &branch_a);
    seed_run(home.path(), RUN_B, &worktree_b, &branch_b);

    for (cwd, expected, other) in [(&worktree_a, RUN_A, RUN_B), (&worktree_b, RUN_B, RUN_A)] {
        let output = show_current(&home, cwd);
        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["data"]["run_id"], expected);
        assert_ne!(value["data"]["run_id"], other);
    }
}

#[test]
fn legacy_and_entropy_branch_formats_both_resolve_by_exact_ownership() {
    let home = TempDir::new().unwrap();
    let repos = TempDir::new().unwrap();
    let legacy_branch = format!("wt/{OLD_PREFIX}-legacy");
    let entropy_id = &RUN_B[RUN_B.len() - 10..];
    let entropy_branch = format!("wt/{entropy_id}-entropy");
    assert!(!RUN_B.starts_with(entropy_id));

    let (legacy_worktree, entropy_worktree) =
        linked_worktrees(repos.path(), &legacy_branch, &entropy_branch);
    seed_run(home.path(), RUN_A, &legacy_worktree, &legacy_branch);
    seed_run(home.path(), RUN_B, &entropy_worktree, &entropy_branch);

    for (cwd, expected) in [(&legacy_worktree, RUN_A), (&entropy_worktree, RUN_B)] {
        let output = show_current(&home, cwd);
        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["data"]["run_id"], expected);
    }
}

#[test]
fn duplicate_exact_path_claims_fail_closed() {
    let home = TempDir::new().unwrap();
    let repos = TempDir::new().unwrap();
    let branch = format!("wt/{OLD_PREFIX}-alpha");
    let cwd = worktree(repos.path(), "alpha", &branch);
    seed_run(home.path(), RUN_A, &cwd, &branch);
    seed_run(home.path(), RUN_B, &cwd, &branch);

    let output = show_current(&home, &cwd);
    assert!(!output.status.success());
    assert_eq!(error_code(&output), "run_owner_ambiguous");
}

#[test]
fn stale_branch_evidence_fails_closed() {
    let home = TempDir::new().unwrap();
    let repos = TempDir::new().unwrap();
    let current_branch = format!("wt/{OLD_PREFIX}-alpha");
    let cwd = worktree(repos.path(), "alpha", &current_branch);
    seed_run(home.path(), RUN_A, &cwd, &format!("wt/{OLD_PREFIX}-stale"));

    let output = show_current(&home, &cwd);
    assert!(!output.status.success());
    assert_eq!(error_code(&output), "run_owner_stale");
}

#[test]
fn missing_and_malformed_evidence_are_distinct_errors() {
    let home = TempDir::new().unwrap();
    let repos = TempDir::new().unwrap();
    let branch = format!("wt/{OLD_PREFIX}-alpha");
    let cwd = worktree(repos.path(), "alpha", &branch);

    let missing = show_current(&home, &cwd);
    assert!(!missing.status.success());
    assert_eq!(error_code(&missing), "run_owner_not_found");

    seed_run(home.path(), RUN_A, &cwd, &branch);
    std::fs::write(
        home.path()
            .join("runs")
            .join(RUN_A)
            .join("nodes/n-0001.json"),
        b"{not-json",
    )
    .unwrap();
    let malformed = show_current(&home, &cwd);
    assert!(!malformed.status.success());
    assert_eq!(error_code(&malformed), "run_owner_malformed");
    assert_eq!(malformed.status.code(), Some(1));
}

#[test]
fn show_current_clap_requires_exactly_one_selector() {
    let home = TempDir::new().unwrap();
    let repos = TempDir::new().unwrap();
    let branch = format!("wt/{OLD_PREFIX}-alpha");
    let cwd = worktree(repos.path(), "alpha", &branch);

    for (args, expected) in [
        (vec!["run", "show", "--output", "json"], "missing_argument"),
        (
            vec!["run", "show", RUN_A, "--current", "--output", "json"],
            "invalid_arguments",
        ),
    ] {
        let output = bin(&home, &cwd).args(args).output().unwrap();
        assert!(!output.status.success());
        assert_eq!(error_code(&output), expected);
    }
}

#[test]
fn relative_path_and_missing_branch_are_refused_as_unsafe_evidence() {
    let home = TempDir::new().unwrap();
    let repos = TempDir::new().unwrap();
    let branch = format!("wt/{OLD_PREFIX}-alpha");
    let cwd = worktree(repos.path(), "alpha", &branch);
    seed_run(home.path(), RUN_A, &cwd, &branch);
    let node_path = home
        .path()
        .join("runs")
        .join(RUN_A)
        .join("nodes/n-0001.json");

    let mut node: Value = serde_json::from_slice(&std::fs::read(&node_path).unwrap()).unwrap();
    node["worktree_path"] = json!(".");
    std::fs::write(&node_path, serde_json::to_vec(&node).unwrap()).unwrap();
    let relative = show_current(&home, &cwd);
    assert!(!relative.status.success());
    assert_eq!(error_code(&relative), "run_owner_malformed");

    node["worktree_path"] = json!(cwd);
    node["branch"] = Value::Null;
    std::fs::write(&node_path, serde_json::to_vec(&node).unwrap()).unwrap();
    let branchless = show_current(&home, &cwd);
    assert!(!branchless.status.success());
    assert_eq!(error_code(&branchless), "run_owner_stale");
}

fn create_skeleton(home: &TempDir, cwd: &Path, extra: &[&str]) -> std::process::Output {
    bin(home, cwd)
        .env("TASKFLEET_TEST_SKIP_MATERIALIZE", "1")
        .args([
            "run", "create", "--kind", "spinoff", "--title", "helper", "--output", "json",
        ])
        .args(extra)
        .output()
        .unwrap()
}

fn created_run(home: &TempDir, output: &std::process::Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let id = value["data"]["run_id"].as_str().unwrap().to_string();
    assert!(home.path().join("runs").join(&id).exists());
    id
}

fn show(home: &TempDir, cwd: &Path, id: &str) -> Value {
    let output = bin(home, cwd)
        .args(["run", "show", id, "--output", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn helper_create_records_exact_creator_without_fan_out_parent_edge() {
    let home = TempDir::new().unwrap();
    let repos = TempDir::new().unwrap();
    let branch = format!("wt/{OLD_PREFIX}-alpha");
    let cwd = worktree(repos.path(), "alpha", &branch);
    seed_run(home.path(), RUN_A, &cwd, &branch);
    // No ambient git/tmux/harness executable is required for attribution.
    let output = bin(&home, &cwd)
        .env("PATH", "/nonexistent")
        .env("TASKFLEET_TEST_SKIP_MATERIALIZE", "1")
        .args([
            "run", "create", "--kind", "spinoff", "--title", "helper", "--output", "json",
        ])
        .output()
        .unwrap();
    let id = created_run(&home, &output);
    let detail = show(&home, &cwd, &id);
    assert_eq!(detail["data"]["created_by_run_id"], RUN_A);
    assert_eq!(detail["data"]["manifest"]["created_by_run_id"], RUN_A);
    assert!(detail["data"]["manifest"]["parent_run_id"].is_null());
    assert!(detail["data"]["manifest"]["parent_node_id"].is_null());
    let events =
        std::fs::read_to_string(home.path().join("runs").join(RUN_A).join("events.jsonl")).unwrap();
    assert!(!events.contains("child.spawned"));
    let listing = bin(&home, &cwd)
        .args(["run", "list", "--output", "json"])
        .output()
        .unwrap();
    assert!(listing.status.success());
    let list: Value = serde_json::from_slice(&listing.stdout).unwrap();
    let row = list["data"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["run_id"] == id)
        .unwrap();
    assert_eq!(row["created_by_run_id"], RUN_A);
}

#[test]
fn outside_run_create_is_unattributed_even_on_a_worker_named_branch() {
    let home = TempDir::new().unwrap();
    let repos = TempDir::new().unwrap();
    let cwd = worktree(
        repos.path(),
        "ordinary",
        &format!("wt/{OLD_PREFIX}-ordinary"),
    );
    let id = created_run(&home, &create_skeleton(&home, &cwd, &[]));
    assert!(show(&home, &cwd, &id)["data"]["created_by_run_id"].is_null());
}

#[test]
fn create_refuses_stale_and_ambiguous_creator_claims_without_publishing() {
    for duplicate in [false, true] {
        let home = TempDir::new().unwrap();
        let repos = TempDir::new().unwrap();
        let branch = format!("wt/{OLD_PREFIX}-alpha");
        let cwd = worktree(repos.path(), "alpha", &branch);
        seed_run(
            home.path(),
            RUN_A,
            &cwd,
            if duplicate { &branch } else { "other" },
        );
        if duplicate {
            seed_run(home.path(), RUN_B, &cwd, &branch);
        }
        let output = create_skeleton(&home, &cwd, &[]);
        assert!(!output.status.success());
        assert_eq!(
            error_code(&output),
            if duplicate {
                "run_owner_ambiguous"
            } else {
                "run_owner_stale"
            }
        );
        assert_eq!(
            std::fs::read_dir(home.path().join("runs")).unwrap().count(),
            if duplicate { 2 } else { 1 }
        );
    }
}

#[test]
fn explicit_fan_out_parent_remains_separate_from_worktree_creator() {
    let home = TempDir::new().unwrap();
    let repos = TempDir::new().unwrap();
    let branch = format!("wt/{OLD_PREFIX}-alpha");
    let cwd = worktree(repos.path(), "alpha", &branch);
    seed_run(home.path(), RUN_A, &cwd, &branch);
    let id = created_run(
        &home,
        &create_skeleton(
            &home,
            &cwd,
            &["--parent-run-id", RUN_A, "--parent-node-id", "n-0001"],
        ),
    );
    let manifest = &show(&home, &cwd, &id)["data"]["manifest"];
    assert_eq!(manifest["created_by_run_id"], RUN_A);
    assert_eq!(manifest["parent_run_id"], RUN_A);
    assert_eq!(manifest["parent_node_id"], "n-0001");
    let events =
        std::fs::read_to_string(home.path().join("runs").join(RUN_A).join("events.jsonl")).unwrap();
    assert!(events.contains("child.spawned"));
}

#[test]
fn idempotent_replay_does_not_reassign_creator_from_retry_cwd() {
    let home = TempDir::new().unwrap();
    let repos = TempDir::new().unwrap();
    let branch = format!("wt/{OLD_PREFIX}-alpha");
    let (cwd, outside) = linked_worktrees(repos.path(), &branch, "outside");
    seed_run(home.path(), RUN_A, &cwd, &branch);
    let source_repo = repos.path().join("main");
    let flags = [
        "--source-repo",
        source_repo.to_str().unwrap(),
        "--source-branch",
        "master",
        "--idempotency-key",
        "once",
    ];
    let id = created_run(&home, &create_skeleton(&home, &cwd, &flags));
    let replay = create_skeleton(&home, &outside, &flags);
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let result: Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(result["data"]["run_id"], id);
    assert_eq!(
        show(&home, &outside, &id)["data"]["created_by_run_id"],
        RUN_A
    );
}

#[test]
fn detached_unowned_release_checkout_can_create_but_detached_owner_cannot() {
    let home = TempDir::new().unwrap();
    let repos = TempDir::new().unwrap();
    let cwd = worktree(repos.path(), "release", "release");
    std::fs::write(cwd.join(".git/HEAD"), format!("{}\n", "a".repeat(40))).unwrap();
    let id = created_run(&home, &create_skeleton(&home, &cwd, &[]));
    assert!(show(&home, &cwd, &id)["data"]["created_by_run_id"].is_null());
    seed_run(home.path(), RUN_A, &cwd, "release");
    let output = create_skeleton(&home, &cwd, &[]);
    assert!(!output.status.success());
    assert_eq!(error_code(&output), "run_owner_stale");
    assert_eq!(error_code(&show_current(&home, &cwd)), "run_owner_stale");
}
