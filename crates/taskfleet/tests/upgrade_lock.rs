//! Cross-process admission contract; all state under a disposable home.
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

fn call(home: &TempDir, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_taskfleet"))
        .env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path())
        .args(["--json"])
        .args(args)
        .output()
        .unwrap()
}

fn wait_for(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(path.exists(), "activation never signalled readiness");
}

#[test]
fn exclusive_gate_refuses_direct_create_and_recovers_after_child_exit() {
    let home = TempDir::new().unwrap();
    let ready = home.path().join("ready");
    let mut gate = Command::new(env!("CARGO_BIN_EXE_taskfleet"))
        .env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path())
        .args(["--json", "run", "upgrade-lock", "--", "/bin/sh", "-c"])
        .arg(format!("touch {}; sleep 12", ready.display()))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for(&ready);
    let blocked = call(
        &home,
        &[
            "run",
            "create",
            "--kind",
            "spinoff",
            "--title",
            "direct",
            "--skip-materialize",
        ],
    );
    assert!(!blocked.status.success());
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("admission_busy"));
    assert!(gate.wait().unwrap().success());
    let created = call(
        &home,
        &[
            "run",
            "create",
            "--kind",
            "spinoff",
            "--title",
            "direct",
            "--skip-materialize",
        ],
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let refused = call(
        &home,
        &["run", "upgrade-lock", "--wait-secs", "0", "--", "/bin/true"],
    );
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("upgrade_not_quiescent"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
}

#[test]
fn unpublished_state_refuses_upgrade_and_crashed_lease_is_released() {
    let home = TempDir::new().unwrap();
    let ready = home.path().join("ready");
    let mut gate = Command::new(env!("CARGO_BIN_EXE_taskfleet"))
        .env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path())
        .args(["run", "upgrade-lock", "--", "/bin/sh", "-c"])
        .arg(format!("touch {}; sleep 12", ready.display()))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for(&ready);
    // Child inherited the lease; killing the CLI parent must not admit a creator.
    gate.kill().unwrap();
    gate.wait().unwrap();
    let blocked = call(
        &home,
        &[
            "run",
            "create",
            "--kind",
            "spinoff",
            "--title",
            "direct",
            "--skip-materialize",
        ],
    );
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("admission_busy"));
    std::thread::sleep(Duration::from_secs(12));
    let staging = home.path().join(".creating");
    std::fs::create_dir_all(staging.join("orphan")).unwrap();
    let refused = call(
        &home,
        &["run", "upgrade-lock", "--wait-secs", "0", "--", "/bin/true"],
    );
    assert!(String::from_utf8_lossy(&refused.stderr).contains("upgrade_state_unverifiable"));
    std::fs::remove_dir_all(staging.join("orphan")).unwrap();
    let timed_out = call(
        &home,
        &[
            "run",
            "upgrade-lock",
            "--command-timeout-secs",
            "1",
            "--",
            "/bin/sleep",
            "5",
        ],
    );
    assert!(String::from_utf8_lossy(&timed_out.stderr).contains("upgrade_command_timeout"));
    let ok = call(
        &home,
        &["run", "upgrade-lock", "--wait-secs", "0", "--", "/bin/true"],
    );
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
}
