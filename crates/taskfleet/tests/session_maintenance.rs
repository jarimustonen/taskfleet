mod common;

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn maintenance_is_repo_cwd_and_tmux_environment_independent_and_idempotent() {
    let home = common::TestHome::new();
    std::fs::write(
        home.path().join("config.toml"),
        "[tmux]\ndefault_session='agents'\npersistent=true\ncompleted_window_ttl='24h'\ncompleted_window_max=20\n",
    )
    .unwrap();
    for _ in 0..2 {
        let mut command = Command::cargo_bin("taskfleet").unwrap();
        command
            .env("TASKFLEET_HOME", home.path())
            .env_remove("TMUX")
            .current_dir("/")
            .args(["session", "maintain", "--output", "json"])
            .assert()
            .success()
            .stdout(predicate::str::contains("\"examined\": 0"))
            .stdout(predicate::str::contains("\"expired\": 0"));
    }
}

#[test]
fn malformed_current_config_does_not_disable_recorded_policy_maintenance() {
    let home = common::TestHome::new();
    std::fs::write(
        home.path().join("config.toml"),
        "[tmux]\ndefault_session='agents'\npersistent=true\ncompleted_window_ttl='bogus'\ncompleted_window_max=20\n",
    )
    .unwrap();
    let mut command = Command::cargo_bin("taskfleet").unwrap();
    command
        .env("TASKFLEET_HOME", home.path())
        .env_remove("TMUX")
        .current_dir("/")
        .args(["session", "maintain", "--output", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"examined\": 0"));
}
