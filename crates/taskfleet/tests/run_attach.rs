//! Attachment's production CLI path against a strict tmux double and no ambient PATH.
#![cfg(unix)]

use std::fs::File;
use std::os::fd::FromRawFd;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};
use taskfleet_core::{append_and_apply_event, NodeId, RunPaths};
use tempfile::TempDir;

const RUN: &str = "01m08c08v5jxzfqf3r36n0sgzd";
const SESSION: &str = "agents ' $; odd";

struct Fixture {
    dir: TempDir,
    home: std::path::PathBuf,
    paths: RunPaths,
    socket: String,
    pid: u32,
    start: u64,
}

impl Fixture {
    fn new(private: bool) -> Self {
        let dir = TempDir::new().unwrap();
        let home = dir.path().join("home ' $; space");
        let paths = RunPaths::new(home.join("runs").join(RUN), RUN).unwrap();
        append_and_apply_event(
            &paths,
            "run.created",
            None,
            None,
            json!({
                "kind":"spinoff", "lifecycle":"autonomous", "title":"display"
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
        let socket = if private {
            dir.path().join("private ' $; socket")
        } else {
            dir.path().join("ordinary default socket")
        }
        .to_string_lossy()
        .into_owned();
        let fixture = Self {
            dir,
            home,
            paths,
            socket,
            pid,
            start,
        };
        fixture.stub();
        fixture
    }

    fn node(&self, id: &str, identity: bool) {
        let mut data = json!({"kind":"spinoff", "attempt":0});
        if identity {
            data["tmux_socket"] = json!(self.socket);
            data["tmux_session"] = json!(SESSION);
            data["tmux_window_id"] = json!("@12");
            data["tmux_pane_id"] = json!("%23");
            data["tmux_server_pid"] = json!(self.pid);
            data["tmux_server_pid_start_secs"] = json!(self.start);
            data["tmux_server_marker"] = json!("fixture");
        }
        append_and_apply_event(
            &self.paths,
            "node.created",
            Some(&NodeId::parse_str(id).unwrap()),
            None,
            data,
        )
        .unwrap();
    }

    fn retained(&self) {
        self.terminal();
        append_and_apply_event(&self.paths, "worker.display.retained", Some(&NodeId::parse_str("n-0001").unwrap()), None, json!({
            "attempt":0, "socket":self.socket, "session":SESSION, "window_id":"@12", "pane_id":"%23",
            "server_pid":self.pid, "server_pid_start_secs":self.start, "server_marker":"fixture",
            "ownership_marker":format!("taskfleet:{RUN}:n-0001:0"),
            "retained_at":"2020-01-01T00:00:00Z", "expires_at":"2099-01-01T00:00:00Z"
        })).unwrap();
    }

    fn terminal(&self) {
        append_and_apply_event(
            &self.paths,
            "node.report",
            Some(&NodeId::parse_str("n-0001").unwrap()),
            None,
            json!({"success":false,"summary":"retained work"}),
        )
        .unwrap();
        append_and_apply_event(
            &self.paths,
            "run.status",
            None,
            None,
            json!({"status":"failed"}),
        )
        .unwrap();
    }

    fn stub(&self) {
        let script = r#"#!/bin/sh
printf '%s\n' "$@" >> "$CALLS"
[ "$1" = -N ] || exit 80
[ "$2" = -S ] && [ "$3" = "$SOCKET" ] || exit 81
shift 3
case "$1" in
list-sessions) printf '%s\n' "$SERVER_ROW";;
list-windows) [ "$2" = -t ] && [ "$3" = "=$SESSION_NAME" ] || exit 82; printf '%s\n' "$WINDOWS";;
list-panes) [ "$2" = -t ] && [ "$3" = @12 ] || exit 83; printf '%s\n' "$PANE_ROW";;
if-shell)
  [ -z "${TMUX+x}" ] || exit 84
  [ "$2" = -F ] && [ "$3" = -t ] && [ "$4" = %23 ] || exit 85
  [ "$5" = "$GUARD" ] || exit 86
  rest=${6#"$VIEW_PREFIX"}
  viewer=${rest%% *}
  hex=${viewer#taskfleet-view-}
  [ "${#hex}" = 32 ] || exit 87
  case "$hex" in *[!0-9a-f]*) exit 87;; esac
  [ "$6" = "$VIEW_PREFIX$viewer -f read-only,ignore-size,active-pane '' ; set-option -t $viewer destroy-unattached on ; link-window -k -s @12 -t =$viewer: ; select-pane -Z -t %23 ; wait-for -S $viewer" ] || exit 87
  [ "$7" = "$FAILURE_CMD" ] || exit 88
  [ "$#" = 7 ] || exit 89;;
wait-for) [ "$#" = 2 ] || exit 91; exit "${READY_EXIT:-0}";;
*) exit 90;;
esac
"#;
        let stub = self.dir.path().join("tmux");
        std::fs::write(&stub, script).unwrap();
        std::fs::set_permissions(stub, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn cmd(&self, verb: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
        cmd.env("TASKFLEET_HOME", &self.home).env("HOME", &self.home)
            .env("PATH", self.dir.path().join("empty-path"))
            .env("TMUX_BIN", self.dir.path().join("tmux")).env_remove("TMUX")
            .env("CALLS", self.dir.path().join("calls"))
            .env("SOCKET", &self.socket).env("SESSION_NAME", SESSION)
            .env("SERVER_ROW", format!("{}\t{SESSION}\t{}\tfixture", self.socket, self.pid))
            .env("VIEW_PREFIX", "new-session -E -s ")
            .env("FAILURE_CMD", "display-message -p 'Taskfleet display changed or target hidden by zoom'; has-session -t '$4294967296'")
            .env("WINDOWS", "@12")
            .env("PANE_ROW", format!("{}\t{SESSION}\t@12\t%23\t{}\tfixture", self.socket, self.pid))
            .env("GUARD", format!("#{{&&:#{{==:#{{pid}},{}}},#{{&&:#{{==:#{{window_id}},@12}},#{{&&:#{{==:#{{pane_id}},%23}},#{{||:#{{==:#{{window_zoomed_flag}},0}},#{{==:#{{pane_active}},1}}}}}}}}}}", self.pid))
            .args(["run", verb, RUN]);
        cmd
    }

    fn json(&self) -> Value {
        data(self.cmd("attach").arg("--json").output().unwrap())
    }
}

fn data(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice::<Value>(&output.stdout).unwrap()["data"].clone()
}

fn error(output: Output, code: &str) {
    assert!(!output.status.success());
    assert_eq!(output.stdout, [] as [u8; 0]);
    let v: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(v["error"]["code"], code, "{v}");
}

#[test]
fn exact_private_and_non_linux_style_socket_discovery_and_shell_command() {
    for private in [true, false] {
        let f = Fixture::new(private);
        f.node("n-0001", true);
        let events = std::fs::read(f.paths.events()).unwrap();
        let show = data(f.cmd("show").arg("--json").output().unwrap());
        assert_eq!(show["attach"]["state"], "recorded");
        assert_eq!(show["attach"]["identity"]["socket"], f.socket);
        assert_eq!(show["attach"]["identity"]["session"], SESSION);
        assert_eq!(show["attach"]["identity"]["pane_id"], "%23");
        assert!(
            !f.dir.path().join("calls").exists(),
            "discovery must not call tmux"
        );
        let validated = f.json();
        assert_eq!(validated["state"], "validated");
        assert_eq!(validated["display"], "worker");
        // Run the actual copy/paste command with --json. State home has quotes,
        // spaces and shell metacharacters; validation must use the same home.
        let mut shell = f.cmd("attach");
        let env: Vec<_> = shell
            .get_envs()
            .map(|(k, v)| (k.to_owned(), v.map(std::ffi::OsStr::to_owned)))
            .collect();
        let mut copied = Command::new("/bin/sh");
        for (key, value) in env {
            if let Some(value) = value {
                copied.env(key, value);
            } else {
                copied.env_remove(key);
            }
        }
        let value = data(
            copied
                .args([
                    "-c",
                    &format!("{} --json", validated["command"].as_str().unwrap()),
                ])
                .output()
                .unwrap(),
        );
        assert_eq!(value["identity"], validated["identity"]);
        // JSON is deliberately safe even when called inside a foreign server.
        data(
            shell
                .arg("--json")
                .env("TMUX", "/unrelated,999,0")
                .output()
                .unwrap(),
        );
        error(
            f.cmd("attach")
                .env("TMUX", "/unrelated,999,0")
                .output()
                .unwrap(),
            "attach_inside_tmux",
        );
        error(
            f.cmd("attach").output().unwrap(),
            "attach_requires_terminal",
        );
        assert_eq!(std::fs::read(f.paths.events()).unwrap(), events);
    }
}

#[test]
fn stale_and_absent_targets_never_fall_back_or_start_a_viewer() {
    let f = Fixture::new(true);
    f.node("n-0001", true);
    for (key, value) in [
        ("SERVER_ROW", ""),
        ("SERVER_ROW", "/foreign\tother\t1\tfixture"),
        ("WINDOWS", "@99"),
        ("PANE_ROW", ""),
        ("PANE_ROW", "/foreign\tother\t@12\t%23\t1\tfixture"),
    ] {
        error(
            f.cmd("attach")
                .arg("--json")
                .env(key, value)
                .output()
                .unwrap(),
            "display_stale",
        );
    }
    let calls = std::fs::read_to_string(f.dir.path().join("calls")).unwrap();
    assert!(!calls.contains("if-shell"));
    assert!(!calls.contains("new-session"));
    // PID-generation mismatch is rejected before touching the socket.
    let node = NodeId::parse_str("n-0001").unwrap();
    append_and_apply_event(&f.paths, "node.retry", Some(&node), None, json!({
        "tmux_socket":f.socket, "tmux_session":SESSION, "tmux_window_id":"@12", "tmux_pane_id":"%23",
        "tmux_server_pid":f.pid,"tmux_server_pid_start_secs":f.start + 1
    })).unwrap();
    error(
        f.cmd("attach").arg("--json").output().unwrap(),
        "display_stale",
    );
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("calls")).unwrap(),
        calls
    );
}

#[test]
fn no_display_multi_node_and_full_id_only() {
    let f = Fixture::new(true);
    error(
        f.cmd("attach").arg("--json").output().unwrap(),
        "display_unavailable",
    );
    f.node("n-0001", false);
    let show = data(f.cmd("show").arg("--json").output().unwrap());
    assert_eq!(show["attach"]["reason"], "display-not-recorded");
    f.node("n-0002", true);
    let show = data(f.cmd("show").arg("--json").output().unwrap());
    assert_eq!(show["attach"]["reason"], "no-unique-node-display");
    assert!(show["attach"]["command"].is_null());
    let mut short = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    error(
        short
            .env("TASKFLEET_HOME", &f.home)
            .args(["run", "attach", &RUN[..12], "--json"])
            .output()
            .unwrap(),
        "invalid_id",
    );
    assert!(!f.dir.path().join("calls").exists());
}

#[test]
fn incomplete_or_malformed_recorded_identities_never_reach_tmux() {
    let f = Fixture::new(true);
    f.node("n-0001", false);
    for (field, value) in [
        ("tmux_socket", json!(null)),
        ("tmux_socket", json!("relative-socket")),
        ("tmux_pane_id", json!(null)),
        ("tmux_pane_id", json!("%23; kill-server")),
        ("tmux_window_id", json!("@12; kill-server")),
        ("tmux_server_pid", json!(null)),
        ("tmux_server_pid_start_secs", json!(null)),
    ] {
        let mut record = json!({"tmux_socket":f.socket,"tmux_session":SESSION,
            "tmux_window_id":"@12","tmux_pane_id":"%23",
            "tmux_server_pid":f.pid,"tmux_server_pid_start_secs":f.start});
        record[field] = value;
        append_and_apply_event(
            &f.paths,
            "node.retry",
            Some(&NodeId::parse_str("n-0001").unwrap()),
            None,
            record,
        )
        .unwrap();
        let show = data(f.cmd("show").arg("--json").output().unwrap());
        assert_eq!(show["attach"]["reason"], "incomplete-display-identity");
        error(
            f.cmd("attach").arg("--json").output().unwrap(),
            "display_unavailable",
        );
    }
    assert!(!f.dir.path().join("calls").exists());
}

#[test]
fn terminal_retained_cleaned_and_removed_worker_displays() {
    let f = Fixture::new(true);
    f.node("n-0001", true);
    f.retained();
    let row = format!(
        "{}\t{SESSION}\t@12\t%23\t1\ttaskfleet:{RUN}:n-0001:0\t{}\tfixture",
        f.socket, f.pid
    );
    let got = data(
        f.cmd("attach")
            .arg("--json")
            .env("PANE_ROW", &row)
            .output()
            .unwrap(),
    );
    assert_eq!(got["display"], "retained");
    error(
        f.cmd("attach")
            .arg("--json")
            .env(
                "PANE_ROW",
                row.replace("\t1\ttaskfleet:", "\t0\ttaskfleet:"),
            )
            .output()
            .unwrap(),
        "display_stale",
    );
    append_and_apply_event(
        &f.paths,
        "worker.display.expired",
        Some(&NodeId::parse_str("n-0001").unwrap()),
        None,
        json!({"attempt":0,"ownership_marker":format!("taskfleet:{RUN}:n-0001:0")}),
    )
    .unwrap();
    let show = data(f.cmd("show").arg("--json").output().unwrap());
    assert_eq!(show["attach"]["reason"], "display-cleaned");
    error(
        f.cmd("attach").arg("--json").output().unwrap(),
        "display_unavailable",
    );
    let f = Fixture::new(false);
    f.node("n-0001", true);
    f.terminal();
    // A terminal result itself is NOT a reason to suppress a still-live display.
    assert_eq!(f.json()["display"], "worker");
    error(
        f.cmd("attach")
            .arg("--json")
            .env("WINDOWS", "@99")
            .output()
            .unwrap(),
        "display_stale",
    );
}

fn pty() -> (File, File) {
    let mut master = -1;
    let mut slave = -1;
    // SAFETY: openpty initializes both descriptors; null pointers request defaults.
    assert_eq!(
        unsafe {
            libc::openpty(
                &raw mut master,
                &raw mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        0
    );
    // SAFETY: successful openpty returned two owned descriptors.
    unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) }
}

#[test]
fn native_outside_tmux_launches_only_guarded_read_only_exact_pane_client() {
    let f = Fixture::new(true);
    f.node("n-0001", true);
    let (_master, slave) = pty();
    let status = f
        .cmd("attach")
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave))
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let calls = std::fs::read_to_string(f.dir.path().join("calls")).unwrap();
    assert!(calls.contains("if-shell\n-F\n-t\n%23\n"));
    assert!(!calls.contains("send-keys"));
    assert!(!calls.contains("switch-client"));
}

#[test]
fn native_setup_must_be_explicitly_acknowledged_not_just_exit_zero() {
    let f = Fixture::new(true);
    f.node("n-0001", true);
    let (_master, slave) = pty();
    let output = f
        .cmd("attach")
        .env("READY_EXIT", "1")
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave))
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "tmux_attach_failed");
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("setup was not confirmed"));
}

#[test]
fn attach_help_explains_json_and_native_behavior() {
    let output = Command::new(env!("CARGO_BIN_EXE_taskfleet"))
        .args(["run", "attach", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("outside tmux"));
    assert!(text.contains("descriptor without attaching"));
}
