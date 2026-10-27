//! Operator overrides of `run upgrade-lock`: what each accepts, what it
//! records, and the refusals no flag lifts. All state under a disposable home.
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
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

fn error_of(out: &std::process::Output) -> Value {
    assert!(!out.status.success(), "expected refusal");
    let body: Value = serde_json::from_slice(&out.stderr)
        .unwrap_or_else(|_| panic!("stderr: {}", String::from_utf8_lossy(&out.stderr)));
    body["error"].clone()
}

fn data_of(out: &std::process::Output) -> Value {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body: Value = serde_json::from_slice(&out.stdout).unwrap();
    body["data"].clone()
}

fn create(home: &TempDir) -> String {
    let out = call(
        home,
        &[
            "run",
            "create",
            "--kind",
            "spinoff",
            "--title",
            "live",
            "--skip-materialize",
        ],
    );
    data_of(&out)["run_id"].as_str().unwrap().to_string()
}

fn marker(home: &TempDir) -> PathBuf {
    home.path().join(".worker-admission-inhibited")
}

/// Activation that succeeds only while admission is excluded by the marker.
fn checks_marker(home: &TempDir) -> String {
    format!("test -f {}", marker(home).display())
}

fn upgrade(home: &TempDir, flags: &[&str], script: &str) -> std::process::Output {
    let mut args = vec!["run", "upgrade-lock", "--wait-secs", "0"];
    args.extend_from_slice(flags);
    args.extend_from_slice(&["--", "/bin/sh", "-c", script]);
    call(home, &args)
}

/// Hold an exclusive flock on `path` in another process until killed.
fn hold_lock(path: &Path, ready: &Path) -> Child {
    let child = Command::new("python3")
        .args([
            "-c",
            "import fcntl,sys,time\nf=open(sys.argv[1],'a')\nfcntl.flock(f,fcntl.LOCK_EX)\nopen(sys.argv[2],'w').close()\ntime.sleep(60)",
        ])
        .arg(path)
        .arg(ready)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    wait_for(ready);
    child
}

/// Listen on a Unix socket at `path` until killed.
fn listen(path: &Path, ready: &Path) -> Child {
    let child = Command::new("python3")
        .args([
            "-c",
            "import socket,sys,time\ns=socket.socket(socket.AF_UNIX)\ns.bind(sys.argv[1])\ns.listen(1)\nopen(sys.argv[2],'w').close()\ntime.sleep(60)",
        ])
        .arg(path)
        .arg(ready)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    wait_for(ready);
    child
}

/// Listen with a full accept queue and never accept, until killed.
fn fill_backlog(path: &Path, ready: &Path) -> Child {
    let _ = std::fs::remove_file(path);
    let child = Command::new("python3")
        .args([
            "-c",
            "import socket,sys,time\ns=socket.socket(socket.AF_UNIX)\ns.bind(sys.argv[1])\ns.listen(0)\nc=[]\nfor _ in range(8):\n  x=socket.socket(socket.AF_UNIX); x.setblocking(False)\n  try: x.connect(sys.argv[1])\n  except OSError: pass\n  c.append(x)\nopen(sys.argv[2],'w').close()\ntime.sleep(60)",
        ])
        .arg(path)
        .arg(ready)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    wait_for(ready);
    child
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(path.exists(), "helper never signalled readiness");
}

fn stop(mut child: Child) {
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn live_runs_are_accepted_only_within_this_state_schema_and_recorded() {
    let home = TempDir::new().unwrap();
    let run_id = create(&home);
    let script = checks_marker(&home);
    // A live published Pi run keeps its adapter files under `.creating`; they
    // belong to the run, not to an unpublished creation.
    std::fs::write(
        home.path()
            .join(".creating")
            .join(format!("message-{run_id}.ts")),
        "adapter",
    )
    .unwrap();

    // The run's launcher calls the release that created it.
    let old_exe = home.path().join("old-taskfleet");
    std::fs::write(&old_exe, "").unwrap();
    std::fs::write(
        home.path()
            .join("runs")
            .join(&run_id)
            .join("candidate-launch-n-0001-attempt-0.sh"),
        format!(
            "#!/bin/sh\nset -eu\n'{}' 'worker-handshake' '--pid' \"$$\"\nexec 'pi'\n",
            old_exe.display()
        ),
    )
    .unwrap();

    let refused = error_of(&upgrade(&home, &[], &script));
    assert_eq!(refused["code"], "upgrade_not_quiescent");
    assert_eq!(refused["details"]["run_ids"][0], run_id.as_str());
    assert_eq!(refused["details"]["runs"][0]["reasons"][0], "not_terminal");

    // Each override names what it accepts: no implicit schema.
    let usage = upgrade(&home, &["--allow-live-runs"], &script);
    assert!(!usage.status.success());
    assert!(!marker(&home).exists());

    let other_schema = error_of(&upgrade(
        &home,
        &["--allow-live-runs", "--target-state-schema", "2"],
        &script,
    ));
    assert_eq!(other_schema["code"], "upgrade_state_schema_incompatible");
    assert_eq!(other_schema["details"]["run_ids"][0], run_id.as_str());
    assert!(
        !marker(&home).exists(),
        "a refusal never inhibits admission"
    );
    assert!(!home.path().join("upgrade-receipts").exists());

    let data = data_of(&upgrade(
        &home,
        &["--allow-live-runs", "--target-state-schema", "1"],
        &script,
    ));
    assert_eq!(data["quiescent"], false);
    assert_eq!(data["state_schema_version"], 1);
    assert_eq!(data["accepted_live_runs"][0]["run_id"], run_id.as_str());
    assert_eq!(
        data["predecessor_executables"][0]["path"],
        old_exe.display().to_string()
    );
    assert_eq!(
        data["predecessor_executables"][0]["run_ids"][0],
        run_id.as_str()
    );
    assert_eq!(
        data["predecessor_executables"][0]["present_after_activation"],
        true
    );
    let receipt: Value =
        serde_json::from_slice(&std::fs::read(data["receipt"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(receipt["outcome"], "completed");
    assert_eq!(receipt["target_state_schema"], 1);
    assert_eq!(receipt["accepted_live_runs"][0]["run_id"], run_id.as_str());
    assert!(!marker(&home).exists());
    // The run itself is untouched and still live.
    assert!(home.path().join("runs").join(&run_id).is_dir());

    // A failed activation keeps admission inhibited and still names the
    // accepted run in its receipt and error details.
    let failed = error_of(&upgrade(
        &home,
        &["--allow-live-runs", "--target-state-schema", "1"],
        "exit 3",
    ));
    assert_eq!(failed["code"], "upgrade_command_failed");
    assert_eq!(failed["details"]["accepted_live_runs"][0], run_id.as_str());
    let receipt: Value = serde_json::from_slice(
        &std::fs::read(failed["details"]["receipt"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["outcome"], "failed");
    assert!(marker(&home).exists());
    let recovered = data_of(&call(
        &home,
        &[
            "run",
            "upgrade-lock",
            "--recover",
            "--confirm-quiescent",
            "--allow-live-runs",
            "--target-state-schema",
            "1",
            "--",
            "/bin/true",
        ],
    ));
    assert_eq!(recovered["recovered"], true);
    assert_eq!(
        recovered["accepted_live_runs"][0]["run_id"],
        run_id.as_str()
    );
    assert!(!marker(&home).exists());

    // An activation that removes the accepted run's executable is reported.
    let removing = data_of(&upgrade(
        &home,
        &["--allow-live-runs", "--target-state-schema", "1"],
        &format!("rm {}", old_exe.display()),
    ));
    assert_eq!(
        removing["predecessor_executables"][0]["present_after_activation"],
        false
    );
}

#[test]
fn unclassifiable_or_foreign_schema_runs_refuse_even_with_the_override() {
    let home = TempDir::new().unwrap();
    let run_id = create(&home);
    let run_dir = home.path().join("runs").join(&run_id);
    let flags = ["--allow-live-runs", "--target-state-schema", "1"];

    std::fs::write(run_dir.join("supervisor.pid"), "not a pid record").unwrap();
    let pid = error_of(&upgrade(&home, &flags, "true"));
    assert_eq!(pid["code"], "upgrade_state_unverifiable");
    assert_eq!(pid["details"]["run_ids"][0], run_id.as_str());
    // Without the override the same run is simply not quiescent.
    let plain = error_of(&upgrade(&home, &[], "true"));
    assert_eq!(plain["code"], "upgrade_not_quiescent");
    std::fs::remove_file(run_dir.join("supervisor.pid")).unwrap();

    let manifest = run_dir.join("manifest.json");
    let mut body: Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    body["schema_version"] = 99.into();
    std::fs::write(&manifest, serde_json::to_vec(&body).unwrap()).unwrap();
    let foreign = error_of(&upgrade(&home, &flags, "true"));
    assert_eq!(foreign["code"], "corrupt_state", "{foreign}");
    assert!(!marker(&home).exists());
}

/// Simulate a creator killed before publication: the staged run tree plus its
/// Pi adapter file under `.creating`.
fn stale_creation(home: &TempDir) -> String {
    let run_id = create(home);
    let staging = home.path().join(".creating").join("runs");
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::rename(
        home.path().join("runs").join(&run_id),
        staging.join(&run_id),
    )
    .unwrap();
    std::fs::write(
        home.path()
            .join(".creating")
            .join(format!("message-{run_id}.ts")),
        "adapter",
    )
    .unwrap();
    run_id
}

#[test]
fn stale_creation_moves_aside_only_when_its_creator_is_provably_gone() {
    let home = TempDir::new().unwrap();
    let id = stale_creation(&home);
    let staged = home.path().join(".creating/runs").join(&id);
    let script = checks_marker(&home);
    let flag = ["--abandon-stale-creations"];

    let refused = error_of(&upgrade(&home, &[], &script));
    assert_eq!(refused["code"], "upgrade_state_unverifiable");
    assert_eq!(refused["details"]["creation_ids"][0], id.as_str());

    // Held staging lock: something may still write it.
    let holder = hold_lock(&staged.join(".lock"), &home.path().join("ready-1"));
    let live = error_of(&upgrade(&home, &flag, &script));
    stop(holder);
    assert_eq!(live["code"], "upgrade_creation_live", "{live}");
    assert!(staged.is_dir(), "a refusal moves nothing");
    assert!(!marker(&home).exists());

    // Held keyed materializer lease.
    let leases = home.path().join("idempotency/materializers");
    std::fs::create_dir_all(&leases).unwrap();
    let lease = leases.join(format!("{id}.lease"));
    let holder = hold_lock(&lease, &home.path().join("ready-2"));
    let live = error_of(&upgrade(&home, &flag, &script));
    stop(holder);
    assert_eq!(live["code"], "upgrade_creation_live", "{live}");

    // A Pi adapter still accepting connections.
    let socket = home
        .path()
        .join(".creating")
        .join(format!("message-{id}-0.sock"));
    let listener = listen(&socket, &home.path().join("ready-3"));
    let live = error_of(&upgrade(&home, &flag, &script));
    stop(listener);
    assert_eq!(live["code"], "upgrade_creation_live", "{live}");
    assert!(staged.is_dir());
    // A listener that never accepts, with its backlog already full, must not
    // wedge the gate: the probe is nonblocking and counts it as live.
    let started = Instant::now();
    let stuck = fill_backlog(&socket, &home.path().join("ready-4"));
    let live = error_of(&upgrade(&home, &flag, &script));
    stop(stuck);
    assert_eq!(live["code"], "upgrade_creation_live", "{live}");
    assert!(started.elapsed() < Duration::from_secs(20));

    // The listener is gone; its socket file now refuses connections.

    let data = data_of(&upgrade(&home, &flag, &script));
    assert_eq!(data["quiescent"], false);
    let abandoned = &data["abandoned_creations"][0];
    assert_eq!(abandoned["creation_id"], id.as_str());
    let dest = PathBuf::from(abandoned["moved_to"].as_str().unwrap());
    assert_eq!(dest, home.path().join("abandoned-creations").join(&id));
    assert!(
        dest.join("run/manifest.json").is_file(),
        "moved, not deleted"
    );
    assert!(dest.join(format!("message-{id}.ts")).is_file());
    assert!(dest.join(format!("message-{id}-0.sock")).exists());
    let record: Value =
        serde_json::from_slice(&std::fs::read(dest.join("abandoned.json")).unwrap()).unwrap();
    assert_eq!(record["creation_id"], id.as_str());
    let evidence = record["evidence"].to_string();
    for fact in [
        "admission_exclusive_held",
        "staging_lock_free",
        "materializer_lease_released",
        "message_socket_refused",
    ] {
        assert!(evidence.contains(fact), "{fact} missing from {evidence}");
    }
    let receipt: Value =
        serde_json::from_slice(&std::fs::read(data["receipt"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(
        receipt["abandoned_creations"][0]["creation_id"],
        id.as_str()
    );
    assert_eq!(receipt["outcome"], "completed");
    assert!(!staged.exists());
    assert!(!marker(&home).exists());

    // Nothing left to abandon: a plain upgrade is quiescent again.
    let clean = data_of(&upgrade(&home, &[], &script));
    assert_eq!(clean["quiescent"], true);
    assert!(clean["receipt"].is_null());
}

#[test]
fn abandonment_refuses_live_recorded_pids_unknown_entries_and_reuse() {
    let home = TempDir::new().unwrap();
    let id = stale_creation(&home);
    let staged = home.path().join(".creating/runs").join(&id);
    let flag = ["--abandon-stale-creations"];

    // A handshake naming a live process (this test) proves a live worker.
    let path = staged.join("worker-handshake-n-0001-attempt-0.json");
    #[cfg(target_os = "linux")]
    {
        let me = std::process::id();
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
        let ticks = stat
            .rsplit_once(") ")
            .unwrap()
            .1
            .split_whitespace()
            .nth(19)
            .unwrap();
        let handshake = serde_json::json!({
            "schema_version": 1, "run_id": id, "node_id": "n-0001", "attempt": 0,
            "token": "t", "pid": me, "start_time": 0,
            "start_identity": format!("linux:{ticks}"), "tmux_pane_id": "%0",
        });
        std::fs::write(&path, serde_json::to_vec(&handshake).unwrap()).unwrap();
        let live = error_of(&upgrade(&home, &flag, "true"));
        assert_eq!(live["code"], "upgrade_creation_live", "{live}");
    }
    std::fs::write(&path, "{").unwrap();
    let unreadable = error_of(&upgrade(&home, &flag, "true"));
    assert_eq!(unreadable["code"], "upgrade_state_unverifiable");
    std::fs::remove_file(&path).unwrap();

    // An unrecognized `.creating` entry is never treated as a creation.
    let orphan = home.path().join(".creating/orphan");
    std::fs::create_dir(&orphan).unwrap();
    let unknown = error_of(&upgrade(&home, &flag, "true"));
    assert_eq!(unknown["code"], "upgrade_state_unverifiable");
    std::fs::remove_dir(&orphan).unwrap();

    // An earlier abandonment of the same id is never overwritten.
    let dest = home.path().join("abandoned-creations").join(&id);
    std::fs::create_dir_all(&dest).unwrap();
    let reused = error_of(&upgrade(&home, &flag, "true"));
    assert_eq!(reused["code"], "upgrade_state_unverifiable");
    assert!(staged.is_dir());
    assert!(!marker(&home).exists());
}
