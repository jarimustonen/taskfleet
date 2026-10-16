//! Linux user-manager boundaries with explicit stubs, safe on bare CI.
#![cfg(target_os = "linux")]

use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use tempfile::TempDir;

mod common;
use common::{NativeSpawnTools, TestHome};

#[test]
fn invalid_runtime_directory_is_actionable_and_has_no_worktree_side_effects() {
    for mode in [0o755, 0o777] {
        let home = TestHome::new();
        let tools = NativeSpawnTools::new();
        let worktree = tools.worktree("invalid-runtime");
        let runtime = TempDir::new().unwrap();
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(mode)).unwrap();
        // A usable profile gets us as far as runtime validation, without ever
        // launching the candidate or talking to a real manager.
        std::fs::write(home.path().join("config.toml"), "[profiles.test]\ndescription='test'\ncapability='fast'\nresidency='local'\nagents=[{harness='claude',command=['/bin/sleep','30']}]\n[profile]\ndefault='test'\n").unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
        cmd.env("HOME", home.path())
            .env("TASKFLEET_HOME", home.path());
        tools.configure(&mut cmd, &worktree, "headless");
        let out = cmd
            .env("XDG_RUNTIME_DIR", runtime.path())
            .args([
                "run",
                "create",
                "--kind",
                "spinoff",
                "--title",
                "bad runtime",
                "--task",
                "noop",
                "--headless",
                "--json",
            ])
            .output()
            .unwrap();
        assert!(!out.status.success());
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(error.contains("linux_runtime_dir_invalid"), "{error}");
        assert!(error.contains("0700"), "{error}");
        assert!(!worktree.exists());
    }
}

#[test]
fn missing_manager_does_not_fall_back_to_native_detach() {
    let home = TestHome::new();
    let runtime = TempDir::new().unwrap();
    let mut create = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    let out = create
        .env("HOME", home.path())
        .env("TASKFLEET_HOME", home.path())
        .args([
            "run",
            "create",
            "--kind",
            "spinoff",
            "--title",
            "manager absent",
            "--skip-materialize",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let data: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let id = data["data"]["run_id"].as_str().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_taskfleet"))
        .env("HOME", home.path())
        .env("TASKFLEET_HOME", home.path())
        .env("XDG_RUNTIME_DIR", runtime.path())
        .env("SYSTEMD_RUN_BIN", "/bin/false")
        .args(["run", "reattach", id, "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(error.contains("linux_user_unit_failed"), "{error}");
    assert!(error.contains("enable-linger"), "{error}");
    assert!(!home
        .path()
        .join("runs")
        .join(id)
        .join("supervisor.pid")
        .exists());
}

#[test]
fn missing_manager_prevents_worker_materialization() {
    let home = TestHome::new();
    let tools = NativeSpawnTools::new();
    let worktree = tools.worktree("no-manager");
    std::fs::write(home.path().join("config.toml"), "[profiles.test]\ndescription='test'\ncapability='fast'\nresidency='local'\nagents=[{harness='claude',command=['/bin/sleep','30']}]\n[profile]\ndefault='test'\n").unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    cmd.env("HOME", home.path())
        .env("TASKFLEET_HOME", home.path());
    tools.configure(&mut cmd, &worktree, "headless");
    let out = cmd
        .env("TMUX_BIN", "/bin/false")
        .env("SYSTEMD_RUN_BIN", "/bin/false")
        .args([
            "run",
            "create",
            "--kind",
            "spinoff",
            "--title",
            "no manager",
            "--task",
            "noop",
            "--headless",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(error.contains("linux_user_unit_failed"), "{error}");
    assert!(!worktree.exists());
}
