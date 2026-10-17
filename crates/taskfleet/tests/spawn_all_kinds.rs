//! Integration coverage for Taskfleet's native materialization path.
//! These tests stub the explicit git/workmux/tmux CLI dependencies, never a
//! private create script, so the generated launcher and PID handshake execute.

use std::os::unix::fs::PermissionsExt as _;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

mod common;
use common::{NativeSpawnTools, TestHome};

const KINDS: &[&str] = &["spinoff", "research", "technical-decision", "fan-out"];

fn profile(home: &TestHome, scratch: &TempDir) {
    let worker = scratch.path().join("worker.sh");
    std::fs::write(&worker, "#!/bin/sh\n/bin/sleep 2\n").unwrap();
    std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            r#"[profiles.test]
description="native test"
capability="fast"
residency="local"
agents=[{{harness="pi",command=["{}"],telemetry="worker-v1"}}]
[profile]
default="test"
"#,
            worker.display()
        ),
    )
    .unwrap();
}

fn command(
    home: &TestHome,
    tools: &NativeSpawnTools,
    worktree: &std::path::Path,
    session: &str,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    command
        .env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path());
    tools.configure(&mut command, worktree, session);
    command
}

fn run_ok(command: &mut Command) -> Value {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn native_spawn_without_declared_placement_rejects_bare_context() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let tools = NativeSpawnTools::new();
    profile(&home, &scratch);
    let worktree = tools.worktree("worktree");

    // Deliberately omit both placement flags. NativeSpawnTools removes ambient
    // TMUX, so production must reject this before invoking tmux or workmux.
    let output = command(&home, &tools, &worktree, "fixture")
        .args([
            "--output",
            "json",
            "run",
            "create",
            "--kind",
            "spinoff",
            "--title",
            "missing placement",
            "--task",
            "work",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no_tmux_session"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!worktree.exists());
    assert!(std::fs::read_dir(home.path().join("runs"))
        .map_or(true, |mut entries| entries.next().is_none()));
}

#[test]
fn each_kind_native_spawn_publishes_a_live_handshaken_node() {
    for kind in KINDS {
        let home = TestHome::new();
        let scratch = TempDir::new().unwrap();
        let tools = NativeSpawnTools::new();
        profile(&home, &scratch);
        let worktree = tools.worktree("worktree");
        let created = run_ok(command(&home, &tools, &worktree, "fixture").args([
            "--output",
            "json",
            "run",
            "create",
            "--kind",
            kind,
            "--tmux-session",
            "fixture",
            "--title",
            "native smoke",
            "--task",
            "do work",
        ]));
        assert_eq!(created["data"]["kind"], *kind);
        assert_eq!(created["data"]["node_id"], "n-0001");
        assert_eq!(
            created["data"]["worktree_path"],
            worktree.display().to_string()
        );
        let run_id = created["data"]["run_id"].as_str().unwrap();
        let node: Value = serde_json::from_slice(
            &std::fs::read(
                home.path()
                    .join("runs")
                    .join(run_id)
                    .join("nodes/n-0001.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(node["agent_pid"].as_u64().is_some_and(|pid| pid > 0));
        assert_eq!(node["tmux_identity"]["pane_id"], "%77");
        assert!(home
            .path()
            .join("runs")
            .join(run_id)
            .join("worker-handshake-n-0001-attempt-0.json")
            .is_file());
    }
}

#[test]
fn headless_native_spawn_preserves_and_records_workmux_window_name() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let tools = NativeSpawnTools::new();
    profile(&home, &scratch);
    let worktree = tools.worktree("worktree");
    let created = run_ok(command(&home, &tools, &worktree, "isolated").args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--tmux-session",
        "isolated",
        "--title",
        "headless",
        "--task",
        "work",
    ]));
    let run_id = created["data"]["run_id"].as_str().unwrap();
    let node: Value = serde_json::from_slice(
        &std::fs::read(
            home.path()
                .join("runs")
                .join(run_id)
                .join("nodes/n-0001.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(node["tmux_identity"]["session"], "isolated");
    assert_eq!(node["tmux_identity"]["window_id"], "@77");
    assert_eq!(node["tmux_window"], "🧪 wm-owned-window");
    assert_eq!(created["data"]["tmux_window"], "🧪 wm-owned-window");
    assert_eq!(
        std::fs::read_to_string(tools.workmux_cwd_path())
            .unwrap()
            .trim(),
        tools.repo_path().canonicalize().unwrap().to_str().unwrap(),
        "workmux must execute from the source repository chosen by run create"
    );
}

#[test]
fn named_source_branch_is_preserved_by_native_spawn() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let tools = NativeSpawnTools::new();
    let branch = Command::new(common::fixture_git_binary())
        .arg("-C")
        .arg(tools.repo_path())
        .args(["branch", "integration"])
        .output()
        .unwrap();
    assert!(
        branch.status.success(),
        "{}",
        String::from_utf8_lossy(&branch.stderr)
    );
    profile(&home, &scratch);
    let worktree = tools.worktree("worktree");
    let created = run_ok(command(&home, &tools, &worktree, "fixture").args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--source-branch",
        "integration",
        "--tmux-session",
        "fixture",
        "--title",
        "base",
        "--task",
        "work",
    ]));
    let run_id = created["data"]["run_id"].as_str().unwrap();
    let shown = run_ok(
        command(&home, &tools, &worktree, "fixture")
            .args(["--output", "json", "run", "show", run_id]),
    );
    assert_eq!(shown["data"]["source_branch"], "integration");
}

#[test]
fn node_backed_and_claude_compatible_recorded_candidates_materialize() {
    for (name, harness, interactive, argv) in [
        ("node", "pi", false, vec!["node", "worker.js", "--exact"]),
        (
            "claude",
            "claude",
            true,
            vec!["claude-compatible", "--model", "test"],
        ),
    ] {
        let home = TestHome::new();
        let scratch = TempDir::new().unwrap();
        let tools = NativeSpawnTools::new();
        let candidate = scratch.path().join(argv[0]);
        std::fs::write(&candidate, "#!/bin/sh\nexec /bin/sleep 2\n").unwrap();
        std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o755)).unwrap();
        let command_argv: Vec<String> = std::iter::once(candidate.display().to_string())
            .chain(argv.iter().skip(1).map(|arg| (*arg).to_owned()))
            .collect();
        std::fs::write(
            home.path().join("config.toml"),
            format!(
                "[profiles.test]\ndescription=\"test\"\ncapability=\"fast\"\nresidency=\"local\"\nagents=[{{harness=\"{harness}\",command={}{} }}]\n[profile]\ndefault=\"test\"\n",
                serde_json::to_string(&command_argv).unwrap(),
                if harness == "pi" { ",telemetry=\"worker-v1\"" } else { "" }
            ),
        )
        .unwrap();
        let worktree = tools.worktree("worktree");
        let mut cmd = command(&home, &tools, &worktree, "fixture");
        cmd.args([
            "--output",
            "json",
            "run",
            "create",
            "--kind",
            "spinoff",
            "--tmux-session",
            "fixture",
        ]);
        if interactive {
            cmd.arg("--interactive");
        }
        let created = run_ok(cmd.args(["--title", name, "--task", "work"]));
        assert_eq!(
            created["data"]["selection"]["selected"]["command"],
            serde_json::json!(command_argv)
        );
    }
}

#[test]
fn native_workmux_failure_rolls_back_without_publication() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let tools = NativeSpawnTools::new();
    profile(&home, &scratch);
    let worktree = tools.worktree("worktree");
    let output = command(&home, &tools, &worktree, "fixture")
        .env("WORKMUX_BIN", "/usr/bin/false")
        .args([
            "--output",
            "json",
            "run",
            "create",
            "--kind",
            "spinoff",
            "--tmux-session",
            "fixture",
            "--title",
            "fail",
            "--task",
            "work",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(!worktree.exists());
    assert!(std::fs::read_dir(home.path().join("runs"))
        .map_or(true, |mut entries| entries.next().is_none()));
}

#[test]
fn missing_task_is_rejected_before_native_dependencies() {
    let home = TestHome::new();
    let output = Command::new(env!("CARGO_BIN_EXE_taskfleet"))
        .env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path())
        .args([
            "--output", "json", "run", "create", "--kind", "spinoff", "--title", "missing",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("missing-task-or-prompt-file"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn workers_clear_browser_attribution_even_from_stale_tmux_and_minimal_path() {
    for (harness, interactive) in [
        ("pi", false),
        ("pi", true),
        ("claude", false),
        ("claude", true),
    ] {
        let home = TestHome::new();
        let scratch = TempDir::new().unwrap();
        let tools = NativeSpawnTools::new();
        let worktree = tools.worktree("nested-session");
        let worker = scratch.path().join("worker");
        let nested = scratch.path().join("nested.json");
        let marker = scratch.path().join("environment");
        // Wait on the precise parent's publication, not a timing assumption.
        // Interactive workers do not pass through the publication-wait shim.
        std::fs::write(
            &worker,
            format!(
                r#"#!/bin/sh
printf '%s' "${{TASKFLEET_CREATOR_SESSION-unset}}" > '{}'
cd '{}' || exit 1
count=0
while [ ! -f "$TASKFLEET_HOME/runs/"*/nodes/n-0001.json ]; do
  count=$((count+1)); [ "$count" -lt 500 ] || exit 90
  /bin/sleep 0.02
done
'{}' run create --kind spinoff --title nested --skip-materialize --json > '{}'
exec /bin/sleep 30
"#,
                marker.display(),
                worktree.display(),
                env!("CARGO_BIN_EXE_taskfleet"),
                nested.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(home.path().join("config.toml"), format!(
            "[profiles.test]\ndescription='test'\ncapability='fast'\nresidency='local'\nagents=[{{harness='{harness}',command=['{}']{}}}]\n[profile]\ndefault='test'\n",
            worker.display(), if harness == "pi" {",telemetry='worker-v1'"} else {""})).unwrap();
        let path = scratch.path().join("bin");
        std::fs::create_dir(&path).unwrap();
        std::os::unix::fs::symlink(common::fixture_git_binary(), path.join("git")).unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
        cmd.env_clear()
            .env("HOME", home.path())
            .env("USER", "fixture")
            .env("TASKFLEET_HOME", home.path())
            .env("TMPDIR", scratch.path());
        tools.configure(&mut cmd, &worktree, "headless");
        cmd.env("PATH", &path)
            .env(
                "TASKFLEET_CREATOR_SESSION",
                "pi:12345678-1234-4234-8234-123456789abc",
            )
            .env(
                "NATIVE_TEST_SERVER_CREATOR_SESSION",
                "pi:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            )
            .args([
                "run",
                "create",
                "--kind",
                "spinoff",
                "--title",
                "session worker",
                "--task",
                "noop",
                "--headless",
                "--json",
            ]);
        if interactive {
            cmd.arg("--interactive");
        }
        let parent = run_ok(&mut cmd);
        let id = parent["data"]["run_id"].as_str().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let child: Value = loop {
            if let Ok(bytes) = std::fs::read(&nested) {
                if let Ok(value) = serde_json::from_slice(&bytes) {
                    break value;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "nested create did not finish: {harness} interactive={interactive}"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "unset");
        let nested_id = child["data"]["run_id"].as_str().unwrap();
        let manifest: Value = serde_json::from_slice(
            &std::fs::read(
                home.path()
                    .join("runs")
                    .join(nested_id)
                    .join("manifest.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["created_by_run_id"], id);
        assert!(manifest["created_by_session"].is_null());
        assert!(manifest["parent_run_id"].is_null());
        let parent_manifest: Value = serde_json::from_slice(
            &std::fs::read(home.path().join("runs").join(id).join("manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            parent_manifest["created_by_session"]["session_id"],
            "12345678-1234-4234-8234-123456789abc"
        );
    }
}

/// A controlled Claude-compatible harness captures the real generated argv.
/// Its Notification command runs independently of the sleeping/modal worker,
/// with no Claude account, telemetry, tmux service, jq, or PATH executable lookup.
#[test]
fn claude_generated_hooks_detect_and_recover_without_worker_telemetry() {
    use serde_json::json;
    use std::io::Write as _;
    use std::process::Stdio;
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let tools = NativeSpawnTools::new();
    let candidate = scratch.path().join("claude fixture.sh");
    let argv_path = scratch.path().join("argv");
    std::fs::write(
        &candidate,
        format!(
            "#!/bin/sh\nprintf '%s\\0' \"$@\" > '{}'\nexec /bin/sleep 2\n",
            argv_path.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(home.path().join("config.toml"), format!(
        "[profiles.test]\ndescription='fixture'\ncapability='fast'\nresidency='local'\nagents=[{{harness='claude',command=[{}]}}]\n[profile]\ndefault='test'\n",
        serde_json::to_string(&candidate.display().to_string()).unwrap())).unwrap();
    let worktree = tools.worktree("worktree");
    let created = run_ok(command(&home, &tools, &worktree, "fixture").args([
        "--json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--interactive",
        "--tmux-session",
        "fixture",
        "--title",
        "hook fixture",
        "--task",
        "no real agent",
    ]));
    let run = created["data"]["run_id"].as_str().unwrap();
    let raw = std::fs::read(&argv_path).unwrap();
    let argv: Vec<_> = raw
        .split(|b| *b == 0)
        .map(|s| String::from_utf8(s.to_vec()).unwrap())
        .collect();
    let get = |flag| argv[argv.iter().position(|s| s == flag).unwrap() + 1].clone();
    let session = get("--session-id");
    let settings: Value = serde_json::from_str(&get("--settings")).unwrap();
    let hook = settings["hooks"]["Notification"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let notify = |kind: &str| json!({"session_id":session,"hook_event_name":"Notification","notification_type":kind});
    let call = |cmd: &str, payload: Value| {
        let mut child = Command::new("/bin/sh")
            .args(["-c", cmd])
            .env("PATH", "")
            .env("TASKFLEET_HOME", scratch.path().join("wrong-home"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&payload).unwrap())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.stdout.is_empty(),
            "hook must never answer a permission prompt"
        );
        assert_eq!(out.stderr, [] as [u8; 0]);
    };
    let show = || {
        run_ok(
            Command::new(env!("CARGO_BIN_EXE_taskfleet"))
                .env("TASKFLEET_HOME", home.path())
                .args(["--json", "run", "show", run]),
        )
    };
    call(
        hook,
        json!({"session_id":session,"hook_event_name":"PostToolUse","tool_response":"x".repeat(70 * 1024)}),
    );
    call(hook, notify("permission_prompt"));
    let first = show();
    assert_eq!(first["data"]["awaiting_input"], true);
    assert_eq!(first["data"]["telemetry_counts"]["absent"], 1);
    let seq = first["data"]["awaiting_input_detail"]["event_seq"].clone();
    call(hook, notify("permission_prompt"));
    assert_eq!(show()["data"]["awaiting_input_detail"]["event_seq"], seq);
    assert_eq!(
        show()["data"]["awaiting_input_detail"]["open_discussion_count"],
        1
    );
    // A wrong session, subagent or stale attempt cannot resolve the request.
    call(
        hook,
        json!({"session_id":"wrong", "hook_event_name":"UserPromptSubmit"}),
    );
    call(
        hook,
        json!({"session_id":session,"agent_id":"child","hook_event_name":"UserPromptSubmit"}),
    );
    let stale = hook.replace("'--attempt' '0'", "'--attempt' '1'");
    assert_ne!(stale, hook);
    call(
        &stale,
        json!({"session_id":session,"hook_event_name":"UserPromptSubmit"}),
    );
    assert_eq!(show()["data"]["awaiting_input"], true);
    let waited = Command::new(env!("CARGO_BIN_EXE_taskfleet"))
        .env("TASKFLEET_HOME", home.path())
        .env("TASKFLEET_AWAITING_INPUT_GRACE_SECS", "0")
        .env("PATH", "")
        .args([
            "--json",
            "run",
            "wait",
            run,
            "--check-in",
            "1s",
            "--timeout",
            "2s",
            "--fail-on-error",
        ])
        .output()
        .unwrap();
    assert_eq!(waited.status.code(), Some(3));
    let v: Value = serde_json::from_slice(&waited.stdout).unwrap();
    assert_eq!(v["data"]["outcome"], "condition-met");
    assert_eq!(
        v["data"]["runs"][0]["progress_evidence"]["prompt_signal"],
        "awaiting-input-reported"
    );
    let recovery = settings["hooks"]["PostToolUseFailure"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    call(
        recovery,
        json!({"session_id":session,"hook_event_name":"PostToolUseFailure"}),
    );
    // A different parallel tool finishing is not proof the permission modal closed.
    assert_eq!(show()["data"]["awaiting_input"], true);
    call(
        recovery,
        json!({"session_id":session,"hook_event_name":"Stop"}),
    );
    assert_eq!(show()["data"]["awaiting_input"], false);
    call(hook, notify("idle_prompt"));
    assert_eq!(show()["data"]["awaiting_input"], true);
    // Coexistence: a manual request appended to the generation must survive.
    let data = scratch.path().join("manual.json");
    std::fs::write(&data, json!({"discussion_items":[{"topic":"Human fork","options":["Wait"],"recommended_default":"Wait"}]}).to_string()).unwrap();
    run_ok(
        Command::new(env!("CARGO_BIN_EXE_taskfleet"))
            .env("TASKFLEET_HOME", home.path())
            .args([
                "--json",
                "event",
                "create",
                run,
                "--kind",
                "node.awaiting_input",
                "--node-id",
                "n-0001",
                "--from-file",
                data.to_str().unwrap(),
            ]),
    );
    call(
        recovery,
        json!({"session_id":session,"hook_event_name":"UserPromptSubmit"}),
    );
    assert_eq!(
        show()["data"]["awaiting_input_detail"]["open_discussion_count"],
        2
    );
    // Explicit cancellation is still the only operator transition here.
    run_ok(
        Command::new(env!("CARGO_BIN_EXE_taskfleet"))
            .env("TASKFLEET_HOME", home.path())
            .args(["--json", "run", "cancel", run]),
    );
    call(hook, notify("permission_prompt"));
    assert_eq!(show()["data"]["status"], "cancelled");
    assert_eq!(show()["data"]["awaiting_input"], false);
    assert!(!scratch.path().join("wrong-home").exists());
}
