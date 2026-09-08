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
