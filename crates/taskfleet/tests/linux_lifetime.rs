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

fn sleep_profile(home: &TestHome, path: &std::path::Path) {
    std::fs::write(path, "#!/bin/sh\nexec /bin/sleep 30\n").unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(home.path().join("config.toml"), format!("[profiles.test]\ndescription='test'\ncapability='fast'\nresidency='local'\nagents=[{{harness='claude',command=['{}']}}]\n[profile]\ndefault='test'\n", path.display())).unwrap();
}

#[test]
fn typed_supervisor_boot_error_survives_manager_failure() {
    let home = TestHome::new();
    let tools = NativeSpawnTools::new();
    let script = home.path().join("boot-error");
    std::fs::write(&script, "#!/bin/sh\nprintf 'Esupervisor_manifest_unreadable\\tfixture typed error\\n' > \"$TASKFLEET_READINESS_FILE\"\nexit 1\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    sleep_profile(&home, &home.path().join("worker"));
    let worktree = tools.worktree("boot-error");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    cmd.env("HOME", home.path())
        .env("TASKFLEET_HOME", home.path());
    tools.configure(&mut cmd, &worktree, "headless");
    let out = cmd
        .env("TASKFLEET_SUPERVISE_BIN", script)
        .args([
            "run",
            "create",
            "--kind",
            "spinoff",
            "--title",
            "typed boot failure",
            "--task",
            "noop",
            "--headless",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(
        error.contains("supervisor_manifest_unreadable: fixture typed error"),
        "{error}"
    );
    assert!(!error.contains("enable-linger"), "{error}");
}

#[test]
fn server_disappearing_after_probe_is_bootstrapped_once_through_manager() {
    let home = TestHome::new();
    let tools = NativeSpawnTools::new();
    sleep_profile(&home, &home.path().join("worker"));
    let worktree = tools.worktree("server-race");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    cmd.env("HOME", home.path())
        .env("TASKFLEET_HOME", home.path());
    tools.configure(&mut cmd, &worktree, "headless");
    let original = cmd
        .get_envs()
        .find(|(key, _)| *key == "TMUX_BIN")
        .unwrap()
        .1
        .unwrap()
        .to_owned();
    let tmux = home.path().join("racing-tmux");
    let booted = home.path().join("server-booted");
    let gone = home.path().join("server-gone");
    std::fs::write(
        &tmux,
        format!(
            r#"#!/bin/sh
if [ "$1" = "-N" ]; then shift; fi
if [ "$1" = "-S" ]; then shift 2; fi
if [ "$1" = "-D" ]; then echo boot >> '{}'; exit 0; fi
if [ ! -f '{}' ]; then
  case "$1" in
    show-options) [ ! -f '{}' ]; exit $? ;;
    has-session|new-session) echo gone > '{}'; exit 1 ;;
  esac
fi
exec '{}' "$@"
"#,
            booted.display(),
            booted.display(),
            gone.display(),
            gone.display(),
            original.to_string_lossy()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&tmux, std::fs::Permissions::from_mode(0o700)).unwrap();
    let out = cmd
        .env("TMUX_BIN", tmux)
        .args([
            "run",
            "create",
            "--kind",
            "spinoff",
            "--title",
            "server race",
            "--task",
            "noop",
            "--headless",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(gone.exists());
    assert_eq!(std::fs::read_to_string(booted).unwrap(), "boot\n");
}
