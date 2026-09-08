//! Hermetic canonical and explicit state-home selection tests. These tests
//! never inspect or modify the user's home.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

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
fn populated_sibling_cannot_change_the_canonical_default() {
    let account = TempDir::new().unwrap();
    let alternate = account.path().join("preserved-state");
    write_file(alternate.join("config.toml"), b"preserved-byte\n");
    write_file(
        alternate.join("runs/01m00000000000000000000000/manifest.json"),
        b"preserved-run-byte\n",
    );

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
    assert_eq!(
        std::fs::read(alternate.join("config.toml")).unwrap(),
        b"preserved-byte\n"
    );
    assert!(!account.path().join(".taskfleet").exists());
}

#[test]
fn explicit_taskfleet_home_selects_a_neutral_nondefault_path() {
    let account = TempDir::new().unwrap();
    let canonical = account.path().join(".taskfleet");
    let alternate = account.path().join("explicit-state");
    write_file(canonical.join("config.toml"), b"canonical-byte\n");
    write_file(alternate.join("config.toml"), b"explicit-byte\n");

    let output = bin(account.path())
        .env("TASKFLEET_HOME", &alternate)
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
        alternate.join("config.toml").display().to_string()
    );
    assert_eq!(
        std::fs::read(canonical.join("config.toml")).unwrap(),
        b"canonical-byte\n"
    );
}

#[test]
fn read_only_explicit_home_does_not_create_the_selected_root() {
    let account = TempDir::new().unwrap();
    let alternate = account.path().join("absent-explicit-state");

    let output = bin(account.path())
        .env("TASKFLEET_HOME", &alternate)
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
        alternate.join("config.toml").display().to_string()
    );
    assert!(!alternate.exists());
}

#[test]
fn selected_default_root_must_be_a_directory() {
    let account = TempDir::new().unwrap();
    std::fs::write(account.path().join(".taskfleet"), b"not-a-directory\n").unwrap();

    let output = config_path(account.path());
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "invalid_home");
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
