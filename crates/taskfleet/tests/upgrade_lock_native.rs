//! Native materialization cannot bypass an exclusive fleet gate.
mod common;
use common::{NativeSpawnTools, TestHome};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn native_create_is_excluded_then_materializes_in_private_fixture() {
    let home = TestHome::new();
    let tools = NativeSpawnTools::new();
    let worker = home.path().join("worker.sh");
    std::fs::write(&worker, "#!/bin/sh\nexec /bin/sleep 120\n").unwrap();
    std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(home.path().join("config.toml"), format!(
        "[profiles.test]\ndescription=\"test\"\ncapability=\"fast\"\nresidency=\"local\"\nagents=[{{harness=\"pi\",command=[\"{}\"],telemetry=\"worker-v1\"}}]\n[profile]\ndefault=\"test\"\n", worker.display()
    )).unwrap();
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
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ready.exists());
    let worktree = tools.worktree("native-admission");
    let make = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
        command
            .env("TASKFLEET_HOME", home.path())
            .env("HOME", home.path());
        tools.configure(&mut command, &worktree, "headless");
        command.args([
            "--json",
            "run",
            "create",
            "--kind",
            "spinoff",
            "--headless",
            "--title",
            "native admission",
            "--task",
            "work",
        ]);
        command
    };
    let blocked = make().output().unwrap();
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("admission_busy"));
    assert!(!worktree.exists());
    assert!(gate.wait().unwrap().success());
    let created = make().output().unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    assert!(worktree.exists());
}
