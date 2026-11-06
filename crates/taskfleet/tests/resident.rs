//! Named resident workers (ADR 0003) through the production materializer,
//! launchers, handshake, run-worker shim, and supervisor: round report, the
//! told exit that ends a round, `run wait` settling at once, `run land`,
//! rebase-before-resume, resume from the same native session (Pi `--session`,
//! Claude `--resume`), idle expiry to attention or dormancy, and revival.
//!
//! The worker is a stub that records each attempt's argv and exits when the
//! test touches `exit-<attempt>`; the test itself plays the worker's part
//! (commit, `node round-report`) through the public CLI.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

mod common;
use common::{NativeSpawnTools, TestHome};

struct Fixture {
    home: TestHome,
    tools: NativeSpawnTools,
    ctrl: TempDir,
    worktree: PathBuf,
}

impl Fixture {
    fn new(harness: &str) -> Self {
        let home = TestHome::new();
        let tools = NativeSpawnTools::new();
        let ctrl = TempDir::new().unwrap();
        for (k, v) in [
            ("user.name", "Fixture"),
            ("user.email", "fixture@example.invalid"),
        ] {
            git(tools.repo_path(), &["config", k, v]);
        }
        let worker = ctrl.path().join("worker.sh");
        std::fs::write(
            &worker,
            format!(
                "#!/bin/sh\nctrl='{}'\nn=$(ls \"$ctrl\" | grep -c '^argv-')\nprintf '%s\\0' \"$@\" > \"$ctrl/argv-$n\"\nwhile [ ! -f \"$ctrl/exit-$n\" ]; do /bin/sleep 0.05; done\nexit \"$(cat \"$ctrl/exit-$n\")\"\n",
                ctrl.path().display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o755)).unwrap();
        let telemetry = if harness == "pi" {
            r#",telemetry="worker-v1""#
        } else {
            ""
        };
        std::fs::write(
            home.path().join("config.toml"),
            format!(
                "[profiles.res]\ndescription=\"res\"\ncapability=\"fast\"\nresidency=\"local\"\nagents=[{{harness=\"{harness}\",command=[\"{}\"]{telemetry}}}]\n[profile]\ndefault=\"res\"\n",
                worker.display()
            ),
        )
        .unwrap();
        let worktree = tools.worktree("resident");
        Self {
            home,
            tools,
            ctrl,
            worktree,
        }
    }

    fn cmd(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
        command
            .env("TASKFLEET_HOME", self.home.path())
            .env("HOME", self.home.path());
        self.tools
            .configure(&mut command, &self.worktree, "headless");
        command
    }

    fn ok(&self, args: &[&str]) -> Value {
        let output = self.cmd().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn fail(&self, args: &[&str]) -> Value {
        let output = self.cmd().args(args).output().unwrap();
        assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
        // Error envelopes go to stderr.
        serde_json::from_slice(&output.stderr).unwrap_or_else(|e| {
            panic!("{args:?}: {e}: {}", String::from_utf8_lossy(&output.stderr))
        })
    }

    fn create(&self, name: &str, extra: &[&str]) -> String {
        let repo = self.tools.repo_path().to_str().unwrap().to_string();
        let mut args = vec![
            "--output",
            "json",
            "run",
            "create",
            "--kind",
            "spinoff",
            "--headless",
            "--title",
            "resident fixture",
            "--task",
            "prototype round one",
            "--source-branch",
            "main",
            "--source-repo",
            &repo,
            "--resident",
            "--name",
            name,
        ];
        args.extend_from_slice(extra);
        let created = self.ok(&args);
        let id = created["data"]["run_id"].as_str().unwrap().to_string();
        wait_event_count(&self.events(&id), "supervisor.started", 1);
        id
    }

    fn events(&self, id: &str) -> PathBuf {
        self.home.path().join("runs").join(id).join("events.jsonl")
    }

    fn argv(&self, attempt: u32) -> Vec<String> {
        let path = self.ctrl.path().join(format!("argv-{attempt}"));
        let deadline = Instant::now() + Duration::from_secs(20);
        while !path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::fs::read_to_string(path)
            .unwrap()
            .split_terminator('\0')
            .map(str::to_string)
            .collect()
    }

    fn end_attempt(&self, attempt: u32, code: i32) {
        std::fs::write(
            self.ctrl.path().join(format!("exit-{attempt}")),
            code.to_string(),
        )
        .unwrap();
    }

    fn commit(&self, file: &str, body: &str) {
        std::fs::write(self.worktree.join(file), body).unwrap();
        // Like a real worker: everything, including the materializer's handoff note.
        git(&self.worktree, &["add", "-A"]);
        git(&self.worktree, &["commit", "-qm", file]);
    }

    fn advance_main(&self, file: &str) -> String {
        std::fs::write(self.tools.repo_path().join(file), file).unwrap();
        git(self.tools.repo_path(), &["add", file]);
        git(self.tools.repo_path(), &["commit", "-qm", file]);
        git(self.tools.repo_path(), &["rev-parse", "main"])
    }

    fn report_round(&self, id: &str, summary: &str) -> Value {
        self.ok(&["node", "round-report", id, "--summary", summary, "--json"])
    }

    fn show(&self, run: &str) -> Value {
        self.ok(&["run", "show", run, "--json"])["data"].clone()
    }

    fn node(&self, id: &str) -> Value {
        let path = self
            .home
            .path()
            .join("runs")
            .join(id)
            .join("nodes/n-0001.json");
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    fn wait_exit_recorded(&self, id: &str, attempt: u32) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            let node = self.node(id);
            if node["worker_exit"].is_object() && node["retry_attempts"] == attempt {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("attempt {attempt} exit never recorded: {}", self.node(id));
    }
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn kinds(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|e| e["kind"].as_str().map(str::to_string))
        .collect()
}

fn wait_event_count(path: &Path, kind: &str, wanted: usize) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if kinds(path).iter().filter(|k| *k == kind).count() >= wanted {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let failures: Vec<String> = std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains("evidence.failed"))
        .map(str::to_string)
        .collect();
    panic!(
        "timed out waiting for {wanted} {kind}: {:?} {failures:?}",
        kinds(path)
    );
}

fn flag_value<'a>(argv: &'a [String], flag: &str) -> &'a str {
    let at = argv
        .iter()
        .position(|a| a == flag)
        .unwrap_or_else(|| panic!("{flag} missing from {argv:?}"));
    &argv[at + 1]
}

#[test]
fn pi_resident_reports_lands_rebases_and_resumes_the_same_session() {
    let f = Fixture::new("pi");
    let id = f.create("proto", &[]);
    let first = f.argv(0);
    let session = flag_value(&first, "--session").to_string();
    assert!(first
        .iter()
        .any(|a| a.contains("Named resident worker `proto`")));
    assert!(first
        .iter()
        .any(|a| a.contains(&format!("taskfleet node round-report {id}"))));

    // A second live run cannot take the same name in this repository.
    let repo = f.tools.repo_path().to_str().unwrap().to_string();
    let dup = f.fail(&[
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--headless",
        "--title",
        "dup",
        "--task",
        "x",
        "--source-branch",
        "main",
        "--source-repo",
        &repo,
        "--resident",
        "--name",
        "proto",
    ]);
    assert_eq!(dup["error"]["code"], "resident_name_in_use");

    // Round 1: the worker commits, reports, and its process ends.
    f.commit("ROUND1", "first round");
    let reported = f.report_round(&id, "round one: built the prototype");
    assert_eq!(reported["data"]["round"], 1);
    assert_eq!(reported["data"]["attempt"], 0);
    let marker = f
        .home
        .path()
        .join(".creating")
        .join(format!("round-{id}-0.reported"));
    assert!(marker.exists(), "the Pi adapter's stop marker is written");
    // Wait settles at once on the round report (no awaiting-input grace).
    let started = Instant::now();
    let waited = f.ok(&["run", "wait", "proto", "--timeout", "30s", "--json"]);
    assert!(started.elapsed() < Duration::from_secs(10));
    let outcome = &waited["data"]["runs"][0];
    assert_eq!(outcome["round_reported"], true);
    assert_eq!(outcome["status"], "pending");
    assert_eq!(
        outcome["resident"]["last_round"]["summary"],
        "round one: built the prototype"
    );
    f.end_attempt(0, 0);
    f.wait_exit_recorded(&id, 0);

    // The told exit after the round report is a round end: waiting, not attention.
    std::thread::sleep(Duration::from_millis(1500));
    let shown = f.show("proto");
    assert_eq!(shown["status"], "pending");
    assert_eq!(shown["attention_required"], false);
    assert_eq!(shown["resident"]["round_state"], "waiting");
    assert_eq!(shown["resident"]["worker_running"], false);
    assert_eq!(shown["resident"]["rounds_reported"], 1);

    // Land: rebase onto the advanced source and fast-forward it; nothing ends.
    f.advance_main("SOURCE1");
    let landed = f.ok(&["run", "land", "proto", "--json"]);
    assert_eq!(landed["data"]["landed"], true);
    assert_eq!(landed["data"]["commits"], 1);
    let main_tip = git(f.tools.repo_path(), &["rev-parse", "main"]);
    assert_eq!(landed["data"]["integrated_oid"], main_tip);
    assert!(
        f.tools.repo_path().join("ROUND1").exists(),
        "round work is on main"
    );
    assert!(f.worktree.exists(), "worktree stays");
    let again = f.ok(&["run", "land", "proto", "--json"]);
    assert_eq!(again["data"]["landed"], false);
    assert_eq!(again["data"]["reason"], "nothing-to-land");
    let shown = f.show(&id);
    assert_eq!(shown["status"], "pending");
    assert_eq!(
        shown["resident"]["last_landing"]["integrated_oid"],
        main_tip
    );

    // Message: main moved again, so the worktree is rebased before resuming.
    let source2 = f.advance_main("SOURCE2");
    let msg = f.ok(&[
        "run",
        "message",
        "proto",
        "Jari chose A1; riff on it",
        "--json",
    ]);
    assert_eq!(msg["data"]["delivery"], "resumed");
    assert_eq!(msg["data"]["resumed"]["attempt"], 1);
    assert_eq!(msg["data"]["resumed"]["round"], 2);
    assert_eq!(msg["data"]["resumed"]["rebase"]["state"], "rebased");
    assert_eq!(msg["data"]["resumed"]["rebase"]["source_oid"], source2);
    assert!(
        f.worktree.join("SOURCE2").exists(),
        "main was rebased into the worker tree"
    );
    let second = f.argv(1);
    assert_eq!(
        flag_value(&second, "--session"),
        session,
        "same native session"
    );
    let input = second.last().unwrap();
    assert!(input.contains("Taskfleet round 2"), "{input}");
    assert!(input.contains("Jari chose A1; riff on it"));
    assert!(input.contains("rebased your branch"));
    let node = f.node(&id);
    assert_eq!(node["retry_attempts"], 1);
    assert!(node["worker_exit"].is_null());
    assert_eq!(node["evidence"]["attempt"], 1);
    assert_eq!(
        node["evidence"]["live_session_path"],
        format!(
            ".creating/pi-sessions/{id}/{}",
            Path::new(&session).file_name().unwrap().to_str().unwrap()
        )
    );
    assert!(node["resident"]["last_round"]["closed_by"].is_string());
    // The message text is the worker's input, never an event-log entry.
    let log = std::fs::read_to_string(f.events(&id)).unwrap();
    assert!(!log.contains("riff on it"));
    let shown = f.show("proto");
    assert_eq!(shown["resident"]["round_state"], "working");

    // Mid-round, a land is refused: a worker process is writing the tree.
    let busy = f.fail(&["run", "land", "proto", "--wait-exit", "0s", "--json"]);
    assert_eq!(busy["error"]["code"], "worker_running");

    // Finish: the parent merges the final round; the run ends and tears down.
    f.report_round(&id, "round two");
    f.end_attempt(1, 0);
    f.wait_exit_recorded(&id, 1);
    let merged = f.ok(&["run", "merge", "proto", "--json"]);
    assert_eq!(merged["data"]["merged"], true);
    wait_event_count(&f.events(&id), "supervisor.exited", 1);
    assert_eq!(f.show(&id)["status"], "done");
}

#[test]
fn claude_resident_resumes_by_session_id_and_refuses_mid_round_messages() {
    let f = Fixture::new("claude");
    let id = f.create("review-fix", &[]);
    let first = f.argv(0);
    let session = flag_value(&first, "--session-id").to_string();
    assert!(first.iter().any(|a| a == "--print"));
    assert_eq!(f.node(&id)["claude_session_id"], session);

    let busy = f.fail(&["run", "message", "review-fix", "hello", "--json"]);
    assert_eq!(busy["error"]["code"], "worker_running");

    f.report_round(&id, "findings listed");
    // Even a failing exit after the round report ends the round, not the run.
    f.end_attempt(0, 3);
    f.wait_exit_recorded(&id, 0);
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(f.show(&id)["status"], "pending");

    let msg = f.ok(&["run", "message", &id, "check the fix", "--json"]);
    assert_eq!(msg["data"]["resumed"]["rebase"]["state"], "up-to-date");
    let second = f.argv(1);
    assert_eq!(flag_value(&second, "--resume"), session);
    assert!(!second.iter().any(|a| a == "--session-id"));
    assert!(second.iter().any(|a| a == "--settings"));
    assert!(second.iter().any(|a| a == "--print"));
    assert!(second.last().unwrap().contains("check the fix"));
    assert_eq!(f.node(&id)["claude_session_id"], session);

    let cancelled = f.ok(&["run", "cancel", "review-fix", "--json"]);
    assert!(cancelled["ok"].as_bool().unwrap_or(true));
    f.end_attempt(1, 0);
}

#[test]
fn idle_expiry_preserves_unlanded_work_then_goes_dormant_and_revives() {
    let f = Fixture::new("pi");
    let id = f.create("researcher", &["--idle-expiry", "1s"]);
    f.commit("FINDINGS", "unlanded");
    f.report_round(&id, "findings");
    f.end_attempt(0, 0);
    f.wait_exit_recorded(&id, 0);

    // Unlanded work: preserved, attention raised, never merged.
    wait_event_count(&f.events(&id), "node.resident_attention", 1);
    let shown = f.show("researcher");
    assert_eq!(shown["attention_required"], true);
    assert!(shown["attention"]["reason"]
        .as_str()
        .unwrap()
        .contains("unlanded"));
    assert!(f.worktree.join("FINDINGS").exists());
    assert!(!f.tools.repo_path().join("FINDINGS").exists());
    let waited = f.ok(&["run", "wait", "researcher", "--timeout", "10s", "--json"]);
    assert_eq!(waited["data"]["runs"][0]["attention_required"], true);

    // Landing clears the attention; the next expiry finds everything landed and
    // removes only the worktree (branch and session stay).
    f.ok(&["run", "land", "researcher", "--json"]);
    wait_event_count(&f.events(&id), "node.resident_dormant", 1);
    assert!(!f.worktree.exists());
    let branch = f.node(&id)["branch"].as_str().unwrap().to_string();
    git(f.tools.repo_path(), &["rev-parse", "--verify", &branch]);
    let shown = f.show(&id);
    assert_eq!(shown["resident"]["round_state"], "dormant");
    assert_eq!(shown["attention_required"], false);
    let land = f.ok(&["run", "land", "researcher", "--json"]);
    assert_eq!(land["data"]["reason"], "dormant");

    // The next message recreates the worktree at the same path and resumes.
    let msg = f.ok(&[
        "run",
        "message",
        "researcher",
        "one follow-up question",
        "--json",
    ]);
    assert_eq!(msg["data"]["resumed"]["worktree_recreated"], true);
    assert!(f.worktree.join("FINDINGS").exists());
    let second = f.argv(1);
    assert_eq!(
        flag_value(&second, "--session"),
        flag_value(&f.argv(0), "--session")
    );
    assert_eq!(f.show(&id)["resident"]["round_state"], "working");
    f.ok(&["run", "cancel", &id, "--json"]);
    f.end_attempt(1, 0);
}

#[test]
fn resident_flags_and_round_reports_are_validated() {
    let f = Fixture::new("pi");
    let repo = f.tools.repo_path().to_str().unwrap().to_string();
    let base = [
        "--output",
        "json",
        "run",
        "create",
        "--headless",
        "--title",
        "t",
        "--task",
        "x",
        "--source-branch",
        "main",
        "--source-repo",
        repo.as_str(),
    ];
    let fanout = f.fail(
        &[
            &base[..],
            &["--kind", "fan-out", "--resident", "--name", "x1"],
        ]
        .concat(),
    );
    assert_eq!(fanout["error"]["code"], "invalid_arguments");
    let interactive = f.fail(
        &[
            &base[..],
            &[
                "--kind",
                "spinoff",
                "--interactive",
                "--resident",
                "--name",
                "x1",
            ],
        ]
        .concat(),
    );
    assert_eq!(interactive["error"]["code"], "invalid_arguments");
    let bad = f.fail(
        &[
            &base[..],
            &["--kind", "spinoff", "--resident", "--name", "7up"],
        ]
        .concat(),
    );
    assert_eq!(bad["error"]["code"], "invalid_resident_name");

    // An ordinary run has no rounds; a name that matches nothing keeps the id error.
    let plain = f.ok(&[&base[..], &["--kind", "spinoff"]].concat());
    let plain_id = plain["data"]["run_id"].as_str().unwrap();
    let refused = f.fail(&["node", "round-report", plain_id, "--summary", "x", "--json"]);
    assert_eq!(refused["error"]["code"], "not_resident");
    let unknown = f.fail(&["run", "show", "nobody-here", "--json"]);
    assert_eq!(unknown["error"]["code"], "invalid_run_id");
    f.ok(&["run", "cancel", plain_id, "--json"]);
    f.end_attempt(0, 0);
}

#[test]
fn rebase_conflicts_land_nothing_and_become_the_rounds_first_task() {
    let f = Fixture::new("pi");
    let id = f.create("fixer", &[]);
    f.commit("CONFLICT", "worker side\n");
    let worker_tip = git(&f.worktree, &["rev-parse", "HEAD"]);
    f.report_round(&id, "fixed on my side");
    f.end_attempt(0, 0);
    f.wait_exit_recorded(&id, 0);
    std::fs::write(f.tools.repo_path().join("CONFLICT"), "main side\n").unwrap();
    git(f.tools.repo_path(), &["add", "CONFLICT"]);
    git(f.tools.repo_path(), &["commit", "-qm", "main side"]);
    let main_before = git(f.tools.repo_path(), &["rev-parse", "main"]);

    // Land refuses and leaves the worker's tree exactly as it was.
    let refused = f.fail(&["run", "land", "fixer", "--json"]);
    assert_eq!(refused["error"]["code"], "land_conflict");
    assert_eq!(git(&f.worktree, &["rev-parse", "HEAD"]), worker_tip);
    assert_eq!(
        git(f.tools.repo_path(), &["rev-parse", "main"]),
        main_before
    );
    assert_eq!(git(&f.worktree, &["status", "--porcelain"]), "");

    // The message rebases anyway and hands the conflict to the worker.
    let msg = f.ok(&["run", "message", "fixer", "continue", "--json"]);
    let rebase = &msg["data"]["resumed"]["rebase"];
    assert_eq!(rebase["state"], "conflicts");
    assert_eq!(rebase["files"], serde_json::json!(["CONFLICT"]));
    let input = f.argv(1).last().unwrap().clone();
    assert!(input.contains("stopped on conflicts"), "{input}");
    assert!(input.contains("`CONFLICT`"));
    let blocked = f.fail(&["run", "land", "fixer", "--wait-exit", "0s", "--json"]);
    assert_eq!(blocked["error"]["code"], "worker_running");
    f.ok(&["run", "cancel", &id, "--json"]);
    f.end_attempt(1, 0);
}
