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

/// Exercise the public maintenance command with no ambient executables. The
/// strict tmux double only accepts identity reads and an exact owned-pane kill.
#[test]
fn display_expiry_keeps_durable_history_and_evidence() {
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt as _;
    use taskfleet_core::{append_and_apply_event, NodeId, RunPaths};

    for mode in ["owned", "active", "foreign", "shared", "last"] {
        let home = common::TestHome::new();
        let id = "01m08c08v5jxzfqf3r36n0sgzd";
        let paths = RunPaths::new(home.path().join("runs").join(id), id).unwrap();
        let nid = NodeId::parse_str("n-0001").unwrap();
        append_and_apply_event(&paths, "run.created", None, None, json!({
            "kind":"spinoff", "lifecycle":"autonomous", "title":"history",
            "tmux_retention":{"persistent":true,"completed_window_ttl_secs":1,"completed_window_max":20}
        })).unwrap();
        append_and_apply_event(
            &paths,
            "node.created",
            Some(&nid),
            None,
            json!({"kind":"spinoff", "attempt":0, "pi_session_id":"018f5f64-b137-7d44-b2b4-4f02c3f646e8", "pi_session_cwd":"/original", "pi_session_path":format!(".creating/pi-sessions/{id}/pi-session-018f5f64-b137-7d44-b2b4-4f02c3f646e8.jsonl")}),
        )
        .unwrap();
        let report = json!({"success":true,"via":"explicit-merge","summary":"retained result"});
        append_and_apply_event(&paths, "node.report", Some(&nid), None, report.clone()).unwrap();
        append_and_apply_event(&paths, "run.status", None, None, json!({"status":"done"})).unwrap();
        let archive = paths.root.join("evidence/n-0001");
        std::fs::create_dir_all(&archive).unwrap();
        // Deliberately distinct original/resume headers: expiry must not rewrite
        // or substitute either artifact, nor delete the terminal report/pane.
        let artifacts = [
            (
                "pi-session.original.jsonl",
                b"{\"type\":\"session\",\"cwd\":\"/original\"}\n".to_vec(),
            ),
            (
                "pi-session.resume.jsonl",
                b"{\"type\":\"session\",\"cwd\":\"/source\"}\n".to_vec(),
            ),
            ("terminal-report.json", serde_json::to_vec(&report).unwrap()),
            ("final-pane.log", b"retained pane\n".to_vec()),
        ];
        for (name, bytes) in &artifacts {
            std::fs::write(archive.join(name), bytes).unwrap();
        }
        append_and_apply_event(
            &paths,
            "worker.evidence.archived",
            Some(&nid),
            None,
            json!({
                "attempt":0,"session_id":"018f5f64-b137-7d44-b2b4-4f02c3f646e8",
                "transcript_path":"evidence/n-0001/pi-session.original.jsonl",
                "resume_path":"evidence/n-0001/pi-session.resume.jsonl",
                "pane_path":"evidence/n-0001/final-pane.log",
                "report_path":"evidence/n-0001/terminal-report.json",
                "transcript_sha256":"0".repeat(64)
            }),
        )
        .unwrap();
        let pid = std::process::id();
        let mut system = sysinfo::System::new();
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
            true,
            sysinfo::ProcessRefreshKind::new(),
        );
        let start = system
            .process(sysinfo::Pid::from_u32(pid))
            .unwrap()
            .start_time();
        append_and_apply_event(&paths, "worker.display.retained", Some(&nid), None, json!({
            "attempt":0,"socket":"/fixture/socket","session":"agents","window_id":"@1","pane_id":"%1",
            "server_pid":pid,"server_pid_start_secs":start,"server_marker":"fixture",
            "ownership_marker":"owned","retained_at":"2020-01-01T00:00:00Z","expires_at":"2020-01-02T00:00:00Z"
        })).unwrap();
        let manifest_before = taskfleet_core::read_manifest_opt(&paths).unwrap().unwrap();
        let events_before = std::fs::read(paths.root.join("events.jsonl")).unwrap();
        let stub = home.path().join("tmux");
        let log = home.path().join("tmux.log");
        let dead = if mode == "active" { "0" } else { "1" };
        let marker = if mode == "foreign" {
            "foreign"
        } else {
            "owned"
        };
        let windows = if mode == "last" { "@1" } else { "@1\\n@2" };
        let row =
            format!("/fixture/socket\\tagents\\t@1\\t%1\\t{dead}\\t{marker}\\t{pid}\\tfixture\\n");
        let rows = if mode == "shared" {
            format!("{row}{row}")
        } else {
            row
        };
        std::fs::write(
            &stub,
            format!(
                r#"#!/bin/sh
[ "$1" = -N ] || exit 89
shift
[ "$1" = -S ] && [ "$2" = /fixture/socket ] || exit 90
shift 2
case "$1" in
list-sessions) printf '/fixture/socket\tagents\t{pid}\tfixture\n';;
list-windows) printf '{windows}\n';;
list-panes) printf '%b' '{rows}';;
kill-pane) [ "$2" = -t ] && [ "$3" = %1 ] || exit 91; printf '%s\n' "$*" >> '{}';;
*) exit 92;;
esac
"#,
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).unwrap();
        for _ in 0..2 {
            let output = std::process::Command::new(env!("CARGO_BIN_EXE_taskfleet"))
                .env("TASKFLEET_HOME", home.path())
                .env("HOME", home.path())
                .env("PATH", home.path())
                .env("TMUX_BIN", &stub)
                .env_remove("TMUX")
                .current_dir("/")
                .args(["session", "maintain", "--json"])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                value["data"]["preserved"],
                i32::from(mode != "owned"),
                "mode={mode}: {value}"
            );
        }
        let manifest_after = taskfleet_core::read_manifest_opt(&paths).unwrap().unwrap();
        assert_eq!(manifest_after.status, manifest_before.status);
        assert_eq!(manifest_after.run_id, manifest_before.run_id);
        assert!(manifest_after.applied_seq >= manifest_before.applied_seq);
        let events_after = std::fs::read(paths.root.join("events.jsonl")).unwrap();
        assert!(
            events_after.starts_with(&events_before),
            "history is append-only"
        );
        let events = String::from_utf8(events_after).unwrap();
        assert_eq!(
            events
                .lines()
                .filter(
                    |line| serde_json::from_str::<serde_json::Value>(line).unwrap()["kind"]
                        == "worker.display.expired"
                )
                .count(),
            usize::from(mode == "owned")
        );
        for (name, bytes) in &artifacts {
            assert_eq!(&std::fs::read(archive.join(name)).unwrap(), bytes);
        }
        assert_eq!(
            std::fs::read_to_string(log).unwrap_or_default(),
            if mode == "owned" {
                "kill-pane -t %1\n"
            } else {
                ""
            }
        );
        let node = taskfleet_core::read_node_opt(&paths, &nid)
            .unwrap()
            .unwrap();
        assert_eq!(node.last_report, Some(report));
    }
}
