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
        &[
            "run",
            "upgrade-lock",
            "--recover",
            "--confirm-quiescent",
            "--",
            "/bin/true",
        ],
    );
    assert!(String::from_utf8_lossy(&refused.stderr).contains("upgrade_state_unverifiable"));
    assert!(home.path().join(".worker-admission-inhibited").exists());
    std::fs::remove_dir_all(staging.join("orphan")).unwrap();
    let stale = call(
        &home,
        &[
            "run",
            "upgrade-lock",
            "--recover",
            "--confirm-quiescent",
            "--",
            "/bin/true",
        ],
    );
    assert!(stale.status.success());
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
    let blocked = call(
        &home,
        &[
            "run",
            "create",
            "--kind",
            "spinoff",
            "--title",
            "blocked",
            "--skip-materialize",
        ],
    );
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("upgrade_inhibited"));
    let ok = call(
        &home,
        &[
            "run",
            "upgrade-lock",
            "--recover",
            "--confirm-quiescent",
            "--wait-secs",
            "0",
            "--",
            "/bin/true",
        ],
    );
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
}

#[test]
fn detached_writer_survives_timeout_but_cannot_admit_until_verified_recovery() {
    let home = TempDir::new().unwrap();
    let live = home.path().join("live");
    let done = home.path().join("done");
    // setsid creates a new session; the child closes the inherited lock FD
    // before the parent times out. Its delayed write is real, not inferred
    // from a PID or process-group lookup.
    let script = format!(
        "import os,time; p=os.fork();\nif p==0:\n os.setsid(); os.closerange(3,4096); open({:?},'w').close(); time.sleep(3); open({:?},'w').close(); os._exit(0)\ntime.sleep(10)",
        live.display().to_string(), done.display().to_string()
    );
    let status = Command::new(env!("CARGO_BIN_EXE_taskfleet"))
        .env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path())
        .args([
            "run",
            "upgrade-lock",
            "--command-timeout-secs",
            "1",
            "--",
            "python3",
            "-c",
            &script,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
    wait_for(&live);
    let create = || {
        call(
            &home,
            &[
                "run",
                "create",
                "--kind",
                "spinoff",
                "--title",
                "blocked",
                "--skip-materialize",
            ],
        )
    };
    assert!(String::from_utf8_lossy(&create().stderr).contains("upgrade_inhibited"));
    let recovery = || {
        call(
            &home,
            &[
                "run",
                "upgrade-lock",
                "--recover",
                "--confirm-quiescent",
                "--",
                "/bin/sh",
                "-c",
                &format!("test -f {}", done.display()),
            ],
        )
    };
    assert!(
        !recovery().status.success(),
        "reconciliation must fail while writer is live"
    );
    assert!(String::from_utf8_lossy(&create().stderr).contains("upgrade_inhibited"));
    wait_for(&done);
    assert!(recovery().status.success());
    assert!(create().status.success());
}

#[test]
fn hard_kill_and_compromised_marker_fail_closed() {
    let home = TempDir::new().unwrap();
    let ready = home.path().join("ready");
    let mut gate = Command::new(env!("CARGO_BIN_EXE_taskfleet"))
        .env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path())
        .args(["run", "upgrade-lock", "--", "/bin/sh", "-c"])
        .arg(format!("touch {}; sleep 2", ready.display()))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for(&ready);
    gate.kill().unwrap();
    gate.wait().unwrap();
    // Even after inherited flock holders exit, the durable marker remains.
    let marker = home.path().join(".worker-admission-inhibited");
    wait_for(&marker);
    let blocked = call(
        &home,
        &[
            "run",
            "create",
            "--kind",
            "spinoff",
            "--title",
            "blocked",
            "--skip-materialize",
        ],
    );
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("upgrade_inhibited"));
    std::fs::remove_file(&marker).unwrap();
    std::os::unix::fs::symlink("/dev/null", &marker).unwrap();
    let blocked = call(
        &home,
        &[
            "run",
            "create",
            "--kind",
            "spinoff",
            "--title",
            "blocked",
            "--skip-materialize",
        ],
    );
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("upgrade_inhibited"));
    let recover = call(
        &home,
        &[
            "run",
            "upgrade-lock",
            "--recover",
            "--confirm-quiescent",
            "--",
            "/bin/true",
        ],
    );
    assert!(String::from_utf8_lossy(&recover.stderr).contains("upgrade_state_unverifiable"));
}

#[test]
fn failed_recovery_and_timeout_remain_inhibited_until_synchronous_success() {
    let home = TempDir::new().unwrap();
    let failed = call(&home, &["run", "upgrade-lock", "--", "/bin/false"]);
    assert!(String::from_utf8_lossy(&failed.stderr).contains("upgrade_command_failed"));
    let no_confirm = call(
        &home,
        &["run", "upgrade-lock", "--recover", "--", "/bin/true"],
    );
    assert!(!no_confirm.status.success());
    let timeout = call(
        &home,
        &[
            "run",
            "upgrade-lock",
            "--recover",
            "--confirm-quiescent",
            "--command-timeout-secs",
            "1",
            "--",
            "/bin/sleep",
            "5",
        ],
    );
    assert!(String::from_utf8_lossy(&timeout.stderr).contains("upgrade_command_timeout"));
    let invalid = call(
        &home,
        &[
            "run",
            "upgrade-lock",
            "--recover",
            "--confirm-quiescent",
            "--command-timeout-secs",
            "0",
            "--",
            "/bin/true",
        ],
    );
    assert!(!invalid.status.success());
    assert!(home.path().join(".worker-admission-inhibited").exists());
    let recovered = call(
        &home,
        &[
            "run",
            "upgrade-lock",
            "--recover",
            "--confirm-quiescent",
            "--",
            "/bin/true",
        ],
    );
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert!(!home.path().join(".worker-admission-inhibited").exists());
}
