//! Hermetic default-state-root selection across the canonical Taskfleet root
//! and an adopted pre-rename root. These tests never inspect the user's homes.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};
use taskfleet_core::{append_and_apply_event, ensure_root, NodeId, RunPaths};
use tempfile::TempDir;

const RUN_ID: &str = "01m1x32k7vwwdmgtd000000001";

fn bin(account_home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    command
        .env("HOME", account_home)
        .env_remove("TASKFLEET_HOME")
        .env_remove("TASKFLEET_INTERNAL_WORKER_STATE_ROOT")
        .env_remove("TASKFLEET_LOG")
        .env_remove("TASKFLEET_PROFILE")
        .env_remove("TASKFLEET_HARNESS");
    command
}

fn config_path(account_home: &Path) -> Output {
    bin(account_home)
        .args(["config", "path", "--output", "json"])
        .output()
        .unwrap()
}

fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

fn write_file(path: impl AsRef<Path>, bytes: &[u8]) {
    let path = path.as_ref();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn seed_incidental_canonical(account_home: &Path) -> PathBuf {
    let root = account_home.join(".taskfleet");
    write_file(
        root.join("logs/taskfleet.log.jsonl"),
        b"existing-log-byte\n",
    );
    write_file(
        root.join("state/pi-installed-skills.json"),
        b"existing-provenance-byte\n",
    );
    root
}

fn seed_meaningful(root: &Path, marker: &[u8]) {
    write_file(root.join("config.toml"), marker);
}

#[test]
fn fresh_account_selects_canonical_without_read_side_population() {
    let account = TempDir::new().unwrap();
    let output = config_path(account.path());
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        json_stdout(&output)["data"]["path"],
        account
            .path()
            .join(".taskfleet/config.toml")
            .display()
            .to_string()
    );
    assert!(
        !account.path().join(".taskfleet").exists(),
        "config path must not establish the state root merely to log its read"
    );
}

#[test]
fn fresh_meaningful_canonical_root_is_selected() {
    let account = TempDir::new().unwrap();
    let canonical = account.path().join(".taskfleet");
    seed_meaningful(&canonical, b"canonical-config-byte\n");

    let output = config_path(account.path());
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        json_stdout(&output)["data"]["path"],
        canonical.join("config.toml").display().to_string()
    );
    assert!(!account.path().join(".orchestratectl").exists());
}

#[test]
fn incidental_only_canonical_root_remains_the_default() {
    let account = TempDir::new().unwrap();
    let canonical = seed_incidental_canonical(account.path());

    let output = config_path(account.path());
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        json_stdout(&output)["data"]["path"],
        canonical.join("config.toml").display().to_string()
    );
}

#[test]
fn sole_meaningful_legacy_root_is_adopted_in_place() {
    let account = TempDir::new().unwrap();
    let legacy = account.path().join(".orchestratectl");
    seed_meaningful(&legacy, b"legacy-config-byte\n");

    let output = config_path(account.path());
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        json_stdout(&output)["data"]["path"],
        legacy.join("config.toml").display().to_string()
    );
    assert!(!account.path().join(".taskfleet").exists());
    assert_eq!(
        std::fs::read(legacy.join("config.toml")).unwrap(),
        b"legacy-config-byte\n"
    );
}

#[test]
fn incidental_canonical_runtime_content_cannot_displace_legacy_state() {
    let account = TempDir::new().unwrap();
    let canonical = seed_incidental_canonical(account.path());
    let legacy = account.path().join(".orchestratectl");
    seed_meaningful(&legacy, b"legacy-config-byte\n");
    let log_before = std::fs::read(canonical.join("logs/taskfleet.log.jsonl")).unwrap();
    let state_before = std::fs::read(canonical.join("state/pi-installed-skills.json")).unwrap();

    let output = config_path(account.path());
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        json_stdout(&output)["data"]["path"],
        legacy.join("config.toml").display().to_string()
    );
    assert_eq!(
        std::fs::read(canonical.join("logs/taskfleet.log.jsonl")).unwrap(),
        log_before
    );
    assert_eq!(
        std::fs::read(canonical.join("state/pi-installed-skills.json")).unwrap(),
        state_before
    );
}

#[test]
fn genuinely_meaningful_dual_roots_fail_closed_without_changing_bytes() {
    let account = TempDir::new().unwrap();
    let canonical = account.path().join(".taskfleet");
    let legacy = account.path().join(".orchestratectl");
    seed_meaningful(&canonical, b"canonical-byte\n");
    seed_meaningful(&legacy, b"legacy-byte\n");

    let output = config_path(account.path());
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "conflicting_state_homes");
    assert_eq!(
        std::fs::read(canonical.join("config.toml")).unwrap(),
        b"canonical-byte\n"
    );
    assert_eq!(
        std::fs::read(legacy.join("config.toml")).unwrap(),
        b"legacy-byte\n"
    );
}

#[test]
fn explicit_taskfleet_home_still_overrides_a_dual_root_conflict() {
    let account = TempDir::new().unwrap();
    let canonical = account.path().join(".taskfleet");
    let legacy = account.path().join(".orchestratectl");
    seed_meaningful(&canonical, b"canonical-byte\n");
    seed_meaningful(&legacy, b"legacy-byte\n");

    let output = bin(account.path())
        .env("TASKFLEET_HOME", &legacy)
        .args(["config", "path", "--output", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        json_stdout(&output)["data"]["path"],
        legacy.join("config.toml").display().to_string()
    );

    let canonical_output = bin(account.path())
        .env("TASKFLEET_HOME", &canonical)
        .args(["config", "path", "--output", "json"])
        .output()
        .unwrap();
    assert!(canonical_output.status.success());
    assert_eq!(
        json_stdout(&canonical_output)["data"]["path"],
        canonical.join("config.toml").display().to_string()
    );
}

#[test]
fn run_show_and_current_use_the_same_adopted_root() {
    let account = TempDir::new().unwrap();
    seed_incidental_canonical(account.path());
    let legacy = account.path().join(".orchestratectl");
    let repo = account.path().join("worker");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "wt/legacy-worker"]);
    git(&repo, &["config", "user.email", "tests@example.invalid"]);
    git(&repo, &["config", "user.name", "State Root Test"]);
    std::fs::write(repo.join("seed"), "seed\n").unwrap();
    git(&repo, &["add", "seed"]);
    git(&repo, &["commit", "-q", "-m", "seed"]);
    seed_run(&legacy, &repo, "wt/legacy-worker");
    let canonical_log = account.path().join(".taskfleet/logs/taskfleet.log.jsonl");
    let canonical_state = account
        .path()
        .join(".taskfleet/state/pi-installed-skills.json");
    let log_before = std::fs::read(&canonical_log).unwrap();
    let state_before = std::fs::read(&canonical_state).unwrap();

    for args in [
        vec!["run", "show", RUN_ID, "--output", "json"],
        vec!["run", "show", "--current", "--output", "json"],
    ] {
        let output = bin(account.path())
            .current_dir(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(json_stdout(&output)["data"]["run_id"], RUN_ID);
    }
    assert_eq!(std::fs::read(canonical_log).unwrap(), log_before);
    assert_eq!(std::fs::read(canonical_state).unwrap(), state_before);
}

#[test]
fn supervisor_and_worker_terminal_command_share_the_adopted_root() {
    let account = TempDir::new().unwrap();
    let canonical = seed_incidental_canonical(account.path());
    let legacy = account.path().join(".orchestratectl");
    let repo = account.path().join("worker-terminal");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "wt/terminal"]);
    git(&repo, &["config", "user.email", "tests@example.invalid"]);
    git(&repo, &["config", "user.name", "State Root Test"]);
    std::fs::write(repo.join("seed"), "seed\n").unwrap();
    git(&repo, &["add", "seed"]);
    git(&repo, &["commit", "-q", "-m", "seed"]);
    seed_run(&legacy, &repo, "wt/terminal");
    let canonical_log = std::fs::read(canonical.join("logs/taskfleet.log.jsonl")).unwrap();
    let canonical_state = std::fs::read(canonical.join("state/pi-installed-skills.json")).unwrap();

    let supervised = bin(account.path())
        .current_dir(&repo)
        .args(["supervise", RUN_ID, "--once", "--output", "json"])
        .output()
        .unwrap();
    assert!(
        supervised.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&supervised.stderr)
    );

    let report = account.path().join("report.json");
    std::fs::write(&report, r#"{"success":true,"summary":"terminal"}"#).unwrap();
    let terminal = bin(account.path())
        .current_dir(&repo)
        .args([
            "node",
            "report",
            RUN_ID,
            "n-0001",
            "--from-file",
            report.to_str().unwrap(),
            "--output",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        terminal.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&terminal.stderr)
    );

    let events =
        std::fs::read_to_string(legacy.join("runs").join(RUN_ID).join("events.jsonl")).unwrap();
    assert!(events.contains("\"kind\":\"supervisor.started\""));
    assert!(events.contains("\"kind\":\"node.report\""));
    assert_eq!(
        std::fs::read(canonical.join("logs/taskfleet.log.jsonl")).unwrap(),
        canonical_log
    );
    assert_eq!(
        std::fs::read(canonical.join("state/pi-installed-skills.json")).unwrap(),
        canonical_state
    );
}

#[test]
fn first_state_creating_command_retains_file_logging() {
    let account = TempDir::new().unwrap();
    let repo = account.path().join("source");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "tests@example.invalid"]);
    git(&repo, &["config", "user.name", "State Root Test"]);
    std::fs::write(repo.join("seed"), "seed\n").unwrap();
    git(&repo, &["add", "seed"]);
    git(&repo, &["commit", "-q", "-m", "seed"]);

    let output = bin(account.path())
        .current_dir(&repo)
        .env("TASKFLEET_TEST_SKIP_MATERIALIZE", "1")
        .args([
            "run",
            "create",
            "--kind",
            "spinoff",
            "--title",
            "first run",
            "--task",
            "test",
            "--output",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = std::fs::read_to_string(account.path().join(".taskfleet/logs/taskfleet.log.jsonl"))
        .unwrap();
    assert!(log.contains("command dispatched"));
}

fn seed_run(home: &Path, worktree: &Path, branch: &str) {
    ensure_root(home).unwrap();
    let dir = home.join("runs").join(RUN_ID);
    std::fs::create_dir_all(&dir).unwrap();
    let paths = RunPaths::new(dir, RUN_ID).unwrap();
    append_and_apply_event(
        &paths,
        "run.created",
        None,
        None,
        json!({ "kind": "spinoff", "lifecycle": "autonomous", "title": "legacy" }),
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
            "worktree_path": worktree.canonicalize().unwrap(),
        }),
    )
    .unwrap();
}

fn git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", cwd)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}
