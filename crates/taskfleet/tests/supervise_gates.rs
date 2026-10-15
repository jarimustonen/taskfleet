//! Validation-gate integration tests (V2/V3/V7/V8/V9) for the
//! `supervisor-process` issue.
//!
//! Every test points the binary at a fresh `TempDir` via
//! `TASKFLEET_HOME` so the user's real `~/.taskfleet/` is
//! never touched. tmux/external-process probes are stubbed via
//! `TMUX_BIN` redirection or by skipping the probe entirely.

use std::io::BufRead as _;
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
use std::path::Path;
use std::process::{Child, Command, ExitStatus};
use std::time::Duration;

use serde_json::Value;
use serial_test::file_serial;
use tempfile::TempDir;

mod common;
use common::TestHome;

fn bin(home: &TempDir) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    c.env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path());
    c.env("TASKFLEET_TEST_SKIP_MATERIALIZE", "1");
    // Make tmux probes deterministically "no window" by pointing
    // TMUX_BIN at a binary that prints nothing.
    c.env("TMUX_BIN", "/usr/bin/true");
    c
}

fn run_ok(cmd: &mut Command) -> Value {
    let out = cmd.output().expect("spawn");
    assert!(
        out.status.success(),
        "exit={:?} stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout is valid JSON")
}

fn read_events(events: &Path) -> Vec<Value> {
    std::fs::read_to_string(events)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .collect()
}

fn count_kind(events: &Path, kind: &str) -> usize {
    read_events(events)
        .into_iter()
        .filter(|v| v["kind"] == kind)
        .count()
}

/// Lenient sibling of [`count_kind`]: parses each line individually and
/// *skips* any that fails (`filter_map(.. .ok())`) instead of panicking.
///
/// Used ONLY inside readiness polling ([`wait_for_kind`]): a detached
/// supervisor may be mid-append when a poll reads the file, leaving a torn
/// trailing line whose strict parse would turn a transient state into a test
/// failure. Final assertions keep the strict [`count_kind`]/[`read_events`] so
/// a genuinely corrupt log still fails the test.
fn count_kind_lenient(events: &Path, kind: &str) -> usize {
    std::fs::read_to_string(events)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["kind"] == kind)
        .count()
}

/// How long readiness polls wait before giving up. Generous on purpose: the
/// suite runs many process-spawning supervisor tests in parallel, so under
/// contention a detached spawn+boot+write can take several seconds (observed
/// 0–1000ms isolated, far longer on a saturated/handbrake CI runner).
const POLL_DEADLINE: Duration = Duration::from_secs(30);

/// Poll `predicate` (re-evaluated every 50ms) until it returns `true`, or until
/// `deadline` elapses. Returns `true` if the condition was met in time, `false`
/// on timeout.
///
/// Replaces fixed `sleep`s when waiting on a *detached* supervisor process:
/// its boot+tick+write latency varies, so a single short sleep is inherently
/// flaky, while the assertion we care about is simply "the condition eventually
/// holds". The poll returns as soon as the predicate is satisfied.
///
/// `predicate` is `FnMut` so callers can capture the last observation (see
/// `wait_for_kind`). Timing uses `Instant::elapsed()` rather than a precomputed
/// `Instant + Duration` (which can panic on overflow) and sleeps for at most the
/// remaining budget, so the predicate still gets one final evaluation right at
/// the deadline instead of being skipped after a budget-consuming sleep.
fn poll_until<F: FnMut() -> bool>(deadline: Duration, mut predicate: F) -> bool {
    let start = std::time::Instant::now();
    loop {
        if predicate() {
            return true;
        }
        match deadline.checked_sub(start.elapsed()) {
            Some(remaining) if !remaining.is_zero() => {
                std::thread::sleep(remaining.min(Duration::from_millis(50)));
            }
            _ => return false,
        }
    }
}

/// Poll until `events` contains at least `want` lines of `kind`, or time out.
/// Returns the count observed at the moment the predicate last ran, so callers
/// can assert on it directly (a timeout simply yields the last sub-`want` count).
fn wait_for_kind(events: &Path, kind: &str, want: usize) -> usize {
    let mut seen = 0;
    poll_until(POLL_DEADLINE, || {
        // Lenient parse: tolerate a torn trailing line the supervisor may be
        // mid-append on (see `count_kind_lenient`).
        seen = count_kind_lenient(events, kind);
        seen >= want
    });
    seen
}

/// Direct child supervisor whose Drop guard always reaps the exact process this
/// test spawned. This is deliberately not pid-file based: the run directory is
/// removed by these tests, so durable ownership metadata may no longer exist.
struct OwnedSupervisor {
    child: Child,
}

impl OwnedSupervisor {
    fn wait_for_exit(&mut self) -> ExitStatus {
        let exited = poll_until(POLL_DEADLINE, || {
            self.child
                .try_wait()
                .expect("poll supervisor exit")
                .is_some()
        });
        assert!(
            exited,
            "supervisor pid {} did not self-terminate after run removal",
            self.child.id()
        );
        self.child.wait().expect("reap self-terminated supervisor")
    }
}

impl Drop for OwnedSupervisor {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// Spawn a supervisor and synchronize on its production readiness-pipe signal.
/// A pid-file existence poll is insufficient: the supervisor claims that file
/// before appending `supervisor.started`. Removing the manifest in that boot
/// window races the append's projection rebuild, which can restore the manifest
/// and make a test wait forever for an orphan condition that no longer exists.
fn spawn_ready_supervisor(home: &TempDir, run_id: &str) -> OwnedSupervisor {
    let mut fds = [0; 2];
    // SAFETY: `pipe` initializes exactly two owned descriptors on success.
    assert_eq!(
        unsafe { libc::pipe(fds.as_mut_ptr()) },
        0,
        "create readiness pipe"
    );
    // SAFETY: both descriptors were freshly returned by the successful pipe.
    let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };

    let child = bin(home)
        .env("TASKFLEET_READINESS_FD", write_fd.as_raw_fd().to_string())
        // Hold the process after pid claim to make the old pid-file race a
        // deterministic regression: only the readiness frame may release the
        // test to remove the run state.
        .env("TASKFLEET_TEST_SLOW_BOOT", "250")
        .args(["supervise", run_id])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn supervisor");
    let supervisor = OwnedSupervisor { child };
    // Only the child may retain the write end; closing ours gives EOF if it
    // dies before reporting readiness.
    drop(write_fd);

    let ready = poll_until(POLL_DEADLINE, || {
        let mut pfd = libc::pollfd {
            fd: read_fd.as_raw_fd(),
            events: libc::POLLIN | libc::POLLHUP,
            revents: 0,
        };
        // SAFETY: `pfd` points to one valid pollfd for this zero-timeout probe.
        (unsafe { libc::poll(&raw mut pfd, 1, 0) }) > 0
    });
    assert!(ready, "supervisor did not report readiness");

    let mut line = String::new();
    let mut reader = std::io::BufReader::new(std::fs::File::from(read_fd));
    reader
        .read_line(&mut line)
        .expect("read supervisor readiness frame");
    assert_eq!(
        line,
        format!("R{}\n", supervisor.child.id()),
        "supervisor readiness must prove exact child ownership"
    );

    supervisor
}

fn create_run(home: &TempDir, kind: &str, title: &str) -> String {
    let v = run_ok(bin(home).args([
        "--output", "json", "run", "create", "--kind", kind, "--title", title,
    ]));
    v["data"]["run_id"].as_str().unwrap().to_string()
}

fn run_dir(home: &TempDir, run_id: &str) -> std::path::PathBuf {
    home.path().join("runs").join(run_id)
}

/// Write an executable shell script `name` under `dir` that records each
/// invocation's full argv (space-joined, one line per call) to `<dir>/<log>`
/// and runs `extra` (raw bash, e.g. to emit canned stdout) before `exit 0`.
/// Used to stub `tmux` / `git` so the cleanup path's external commands are
/// asserted on argv rather than really run.
fn fake_recorder(dir: &Path, name: &str, log: &str, extra: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join(name);
    let log_path = dir.join(log);
    // Single-quote the log path for the shell (tempdir paths carry no quotes).
    let log = log_path.to_str().unwrap();
    let body = format!(
        "#!/bin/bash\nprintf '%s ' \"$@\" >> '{log}'\nprintf '\\n' >> '{log}'\n{extra}\nexit 0\n",
    );
    std::fs::write(&p, body).unwrap();
    let mut perms = std::fs::metadata(&p).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&p, perms).unwrap();
    p
}

/// A fresh fake `tmux` recording to `<dir>/tmux.log`.
fn fake_tmux_recorder(dir: &Path) -> std::path::PathBuf {
    fake_recorder(dir, "fake-tmux.sh", "tmux.log", "")
}

/// A fresh fake `git` recording to `<dir>/git.log`. `worktree list` invocations
/// return a canned main-worktree path so the cleanup's `main_worktree_of` probe
/// resolves without a real repo.
fn fake_git_recorder(dir: &Path) -> std::path::PathBuf {
    fake_recorder(
        dir,
        "fake-git.sh",
        "git.log",
        "case \"$*\" in *'worktree list'*) echo 'worktree /fake/main';; esac",
    )
}

fn log_contents(dir: &Path, log: &str) -> String {
    std::fs::read_to_string(dir.join(log)).unwrap_or_default()
}

/// A live `sleep` process forged as a run's worker agent; killed on drop so a
/// panicking assertion never leaks it.
struct AgentGuard {
    child: std::process::Child,
}
impl Drop for AgentGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Forge a NON-terminal `node.created` whose `agent_pid` is a freshly spawned,
/// live `sleep`, so the run is a normal single-worker run the supervisor keeps
/// supervising (rather than a node-less run the no-worker guard would fail).
/// Returns the guard that reaps the agent on drop.
fn forge_live_worker_node(home: &TempDir, run_id: &str) -> AgentGuard {
    let child = Command::new("sleep")
        .arg("120")
        .spawn()
        .expect("spawn agent");
    let agent_pid = child.id();
    let node = home.path().join(format!("node-live-{run_id}.json"));
    std::fs::write(
        &node,
        format!(
            r#"{{"kind":"spinoff","task":"x","worktree_path":"/fake/wt","branch":"wt/test-x","tmux_session":"taskfleet","tmux_window_id":"@42","agent_pid":{agent_pid}}}"#
        ),
    )
    .unwrap();
    run_ok(bin(home).args([
        "--output",
        "json",
        "event",
        "create",
        run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        node.to_str().unwrap(),
    ]));
    AgentGuard { child }
}

/// Forge a `node.created` carrying the worktree/branch/tmux fields the cleanup
/// path consumes, then a terminal `node.report` so the node is settled before
/// the supervisor ticks (keeps the watchdog out of it).
fn forge_terminal_worker_node(home: &TempDir, run_id: &str, kind: &str, report: &str) {
    let node = home.path().join(format!("node-{run_id}.json"));
    std::fs::write(
        &node,
        format!(
            r#"{{"kind":"{kind}","task":"x","worktree_path":"/fake/wt","branch":"wt/test-x","tmux_session":"taskfleet","tmux_window_id":"@42"}}"#
        ),
    )
    .unwrap();
    run_ok(bin(home).args([
        "--output",
        "json",
        "event",
        "create",
        run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        node.to_str().unwrap(),
    ]));
    let report_file = home.path().join(format!("report-{run_id}.json"));
    std::fs::write(&report_file, report).unwrap();
    run_ok(bin(home).args([
        "--output",
        "json",
        "node",
        "report",
        run_id,
        "n-0001",
        "--from-file",
        report_file.to_str().unwrap(),
    ]));
}

/// Forge a `node.created` for a specific `node_id` carrying a distinct
/// worktree/branch, without settling it — so a later `run cancel --node` drives
/// the terminal cancel through the production path. Used to build a multi-node
/// fan-out for the per-node-cancel rollup test.
fn forge_live_fanout_node(home: &TempDir, run_id: &str, node_id: &str, branch: &str) {
    let node = home.path().join(format!("node-{run_id}-{node_id}.json"));
    std::fs::write(
        &node,
        format!(
            r#"{{"kind":"fan-out","task":"x","worktree_path":"/fake/wt-{node_id}","branch":"{branch}","tmux_session":"taskfleet","tmux_window_id":"@42"}}"#
        ),
    )
    .unwrap();
    run_ok(bin(home).args([
        "--output",
        "json",
        "event",
        "create",
        run_id,
        "--kind",
        "node.created",
        "--node-id",
        node_id,
        "--from-file",
        node.to_str().unwrap(),
    ]));
}

/// Read the latest `run.status` value recorded in the event log, if any.
fn latest_run_status(events: &Path) -> Option<String> {
    read_events(events)
        .into_iter()
        .filter(|v| v["kind"] == "run.status")
        .filter_map(|v| v["data"]["status"].as_str().map(str::to_string))
        .next_back()
}

/// supervisor-complete-run-on-terminal-report + supervisor-close-tmux-on-terminal:
/// an autonomous run whose node submits a successful terminal `node.report` must
/// (1) be rolled up to `run.status: done` by the supervisor — no `run cancel`
/// needed — and (2) have its tmux window closed, worktree removed, and branch
/// deleted on the same terminal transition. The branch delete uses the safe
/// `git branch -d` (not the force `-D`): a plain success report is not the
/// confirmed-merge path (`via: "explicit-merge"`), so only a branch actually
/// merged into its source is dropped (issue `blocked-report-deletes-branch`).
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn terminal_report_rolls_run_to_done_and_cleans_up() {
    let home = TestHome::new();
    let dir = TempDir::new().unwrap();
    let run_id = create_run(&home, "spinoff", "rollup-done");
    forge_terminal_worker_node(
        &home,
        &run_id,
        "spinoff",
        r#"{"success": true, "summary": "ok", "discussion_items": [], "spinoff_proposals": [], "wrap_up_recommendations": []}"#,
    );

    run_ok(
        bin(&home)
            .env("TMUX_BIN", fake_tmux_recorder(dir.path()))
            .env("GIT_BIN", fake_git_recorder(dir.path()))
            .args(["--output", "json", "supervise", &run_id, "--once"]),
    );

    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert_eq!(
        latest_run_status(&events).as_deref(),
        Some("done"),
        "supervisor must roll the run up to done"
    );

    let tmux = log_contents(dir.path(), "tmux.log");
    assert!(
        tmux.contains("kill-window -t @42"),
        "tmux window not closed: {tmux:?}"
    );
    let git = log_contents(dir.path(), "git.log");
    // NON-force removal on the non-merge (plain-success) path: git's own atomic
    // dirty-check is the TOCTOU safety net; `--force` is reserved for a confirmed
    // `run merge` (issue `non-merge-teardown-dirty-worktree`).
    assert!(
        git.contains("worktree remove /fake/wt") && !git.contains("worktree remove --force"),
        "worktree not removed with non-force on the non-merge path: {git:?}"
    );
    assert!(
        git.contains("branch -d -- wt/test-x"),
        "branch not deleted with the safe -d on the non-merge path: {git:?}"
    );
}

/// The BLOCKED path (`blocked-report-deletes-branch`): a `node.report
/// {success:false}` (no `run merge`) is the documented "needs a human" handoff.
/// The run still rolls up to `run.status: failed` and the tmux window may close
/// (winding the run down is fine), but the supervisor MUST NOT tear down the
/// branch or worktree — that committed, unmerged work must survive for the human
/// to pick up. A `cleanup.branch_preserved` audit event records the handoff.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn blocked_report_rolls_run_to_failed_but_preserves_branch() {
    let home = TestHome::new();
    let dir = TempDir::new().unwrap();
    let run_id = create_run(&home, "spinoff", "rollup-blocked");
    forge_terminal_worker_node(
        &home,
        &run_id,
        "spinoff",
        r#"{"success": false, "summary": "boom", "discussion_items": [{"topic": "blocked"}]}"#,
    );

    run_ok(
        bin(&home)
            .env("TMUX_BIN", fake_tmux_recorder(dir.path()))
            .env("GIT_BIN", fake_git_recorder(dir.path()))
            .args(["--output", "json", "supervise", &run_id, "--once"]),
    );

    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert_eq!(latest_run_status(&events).as_deref(), Some("failed"));
    // The tmux window may still close (the run is winding down).
    assert!(log_contents(dir.path(), "tmux.log").contains("kill-window -t @42"));
    // But NEITHER the worktree removal NOR the branch delete may run — the
    // blocked path preserves both for the human.
    let git = log_contents(dir.path(), "git.log");
    assert!(
        !git.contains("worktree remove"),
        "blocked path must not remove the worktree: {git:?}"
    );
    assert!(
        !git.contains("branch -d") && !git.contains("branch -D"),
        "blocked path must not delete the branch: {git:?}"
    );
    // The preservation is auditable.
    let preserved = read_events(&events)
        .into_iter()
        .any(|v| v["kind"] == "cleanup.branch_preserved" && v["data"]["branch"] == "wt/test-x");
    assert!(
        preserved,
        "expected a cleanup.branch_preserved audit event; events: {:?}",
        read_events(&events)
            .into_iter()
            .filter_map(|v| v["kind"].as_str().map(str::to_string))
            .collect::<Vec<_>>()
    );
}

/// A failed native worker with no pending evidence must not keep its supervisor
/// alive after recording the branch-preserved handoff. In contrast, incomplete
/// evidence remains pending and is retried before cleanup can settle.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn failed_worker_supervisor_exits_after_preserving_work() {
    let home = TestHome::new();
    let dir = TempDir::new().unwrap();
    let run_id = create_run(&home, "spinoff", "failed-worker-exit");
    forge_terminal_worker_node(
        &home,
        &run_id,
        "spinoff",
        r#"{"success":false,"summary":"writer failed"}"#,
    );
    run_ok(
        bin(&home)
            .env("TMUX_BIN", fake_tmux_recorder(dir.path()))
            .env("GIT_BIN", fake_git_recorder(dir.path()))
            .args(["--output", "json", "supervise", &run_id, "--max-iter", "12"]),
    );
    let events = read_events(&run_dir(&home, &run_id).join("events.jsonl"));
    assert!(events
        .iter()
        .any(|e| e["kind"] == "run.status" && e["data"]["status"] == "failed"));
    assert!(events
        .iter()
        .any(|e| e["kind"] == "cleanup.branch_preserved"));
    assert!(events
        .iter()
        .any(|e| e["kind"] == "supervisor.exited" && e["data"]["reason"] == "work-complete"));
    let git = log_contents(dir.path(), "git.log");
    assert!(
        !git.contains("worktree remove")
            && !git.contains("branch -d")
            && !git.contains("branch -D")
    );
}

/// Terminal-via-cancel: a `run cancel` already drove the run to `cancelled`
/// (the existing path). The supervisor must still perform the autonomous
/// teardown when it next ticks over the now-terminal run.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn terminal_via_cancel_still_cleans_up() {
    let home = TestHome::new();
    let dir = TempDir::new().unwrap();
    let run_id = create_run(&home, "spinoff", "cancel-clean");
    // Forge the worker node (with worktree/tmux) but DO NOT report — cancel
    // synthesizes the terminal node.report itself.
    let node = home.path().join("cancel-node.json");
    std::fs::write(
        &node,
        r#"{"kind":"spinoff","task":"x","worktree_path":"/fake/wt","branch":"wt/test-x","tmux_session":"taskfleet","tmux_window_id":"@42"}"#,
    )
    .unwrap();
    run_ok(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        node.to_str().unwrap(),
    ]));
    run_ok(bin(&home).args(["--output", "json", "run", "cancel", &run_id]));

    run_ok(
        bin(&home)
            .env("TMUX_BIN", fake_tmux_recorder(dir.path()))
            .env("GIT_BIN", fake_git_recorder(dir.path()))
            .args(["--output", "json", "supervise", &run_id, "--once"]),
    );

    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert_eq!(latest_run_status(&events).as_deref(), Some("cancelled"));
    assert!(
        log_contents(dir.path(), "tmux.log").contains("kill-window -t @42"),
        "cancel path must still close the tmux window"
    );
    // Cancel is a source-relative (non-merge) teardown → NON-force removal
    // (issue `non-merge-teardown-dirty-worktree`).
    let git = log_contents(dir.path(), "git.log");
    assert!(
        git.contains("worktree remove /fake/wt") && !git.contains("worktree remove --force"),
        "cancel path must remove the worktree with non-force: {git:?}"
    );
}

/// Per-node fan-out cancel + last-node roll-up + branch-preserving teardown
/// (`per-node-run`): cancelling each child of a multi-node fan-out with `run
/// cancel --node <id>` leaves the run non-terminal until the LAST child settles,
/// then that cancel rolls the batch up to `cancelled` in the same transaction
/// (llm-review C1 — not deferred to a possibly-dead supervisor). When the
/// supervisor next ticks over the now-terminal run it tears each cancelled node
/// down with the source-relative (non-merge) path — never a force removal — so
/// branch-preserving work is safe.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn per_node_cancel_of_every_child_rolls_fanout_up_to_cancelled() {
    let home = TestHome::new();
    let dir = TempDir::new().unwrap();
    let run_id = create_run(&home, "fan-out", "per-node-cancel");
    forge_live_fanout_node(&home, &run_id, "n-0001", "wt/child-a");
    forge_live_fanout_node(&home, &run_id, "n-0002", "wt/child-b");

    // Cancel the first child: the run stays live while n-0002 runs.
    let v = run_ok(bin(&home).args([
        "--output", "json", "run", "cancel", &run_id, "--node", "n-0001",
    ]));
    assert!(v["data"]["run_rolled_up_to"].is_null());
    let show = run_ok(bin(&home).args(["--output", "json", "run", "show", &run_id]));
    assert_eq!(
        show["data"]["manifest"]["status"], "pending",
        "run must not terminalize while n-0002 is live"
    );

    // Cancel the last child: this rolls the all-cancelled batch up to `cancelled`
    // right here, under the same lock.
    let v = run_ok(bin(&home).args([
        "--output", "json", "run", "cancel", &run_id, "--node", "n-0002",
    ]));
    assert_eq!(v["data"]["run_rolled_up_to"], "cancelled");
    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert_eq!(
        latest_run_status(&events).as_deref(),
        Some("cancelled"),
        "an all-cancelled fan-out rolls up to cancelled, not failed"
    );

    // The supervisor ticks over the now-terminal run and tears the children
    // down. Cancel is source-relative (non-merge) → NEVER a force removal.
    run_ok(
        bin(&home)
            .env("TMUX_BIN", fake_tmux_recorder(dir.path()))
            .env("GIT_BIN", fake_git_recorder(dir.path()))
            .args(["--output", "json", "supervise", &run_id, "--once"]),
    );
    let git = log_contents(dir.path(), "git.log");
    assert!(
        !git.contains("worktree remove --force"),
        "cancel must never force-remove a child's worktree: {git:?}"
    );
    assert!(
        !git.contains("branch -D"),
        "cancel must never force-delete a child's branch: {git:?}"
    );
}

/// worktree-merge-orphans-tmux-window: when the recorded tmux window cannot be
/// located during cleanup (the orphan case — a manually-resolved rebase renamed
/// the window so the spawn-time id/name no longer matches and no pane is parked
/// in the worktree), the supervisor must record a non-fatal
/// `cleanup.window_missing` audit event and STILL roll the run up to `done`. The
/// run must not fail just because a window was already gone.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn missing_window_records_event_without_failing_run() {
    let home = TestHome::new();
    let dir = TempDir::new().unwrap();
    let run_id = create_run(&home, "spinoff", "orphan-window");
    forge_terminal_worker_node(
        &home,
        &run_id,
        "spinoff",
        r#"{"success": true, "summary": "ok"}"#,
    );

    // Fake tmux where `kill-window` reports the target missing (exit 1) and
    // `list-windows` shows no pane in the worktree → no path recovery.
    let tmux = fake_recorder(
        dir.path(),
        "fake-tmux.sh",
        "tmux.log",
        "case \"$*\" in *kill-window*) exit 1;; *list-windows*) exit 0;; esac",
    );

    run_ok(
        bin(&home)
            .env("TMUX_BIN", &tmux)
            .env("GIT_BIN", fake_git_recorder(dir.path()))
            .args(["--output", "json", "supervise", &run_id, "--once"]),
    );

    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert_eq!(
        latest_run_status(&events).as_deref(),
        Some("done"),
        "an orphaned window must not fail the run"
    );
    assert_eq!(
        count_kind(&events, "cleanup.window_missing"),
        1,
        "the orphaned window must be recorded once: {:?}",
        read_events(&events)
            .into_iter()
            .map(|v| v["kind"].clone())
            .collect::<Vec<_>>()
    );
    // The kill was still attempted against the recorded id before falling back.
    assert!(log_contents(dir.path(), "tmux.log").contains("kill-window -t @42"));
}

/// V2: `agent_pid` discovery and PID-based liveness probe.
///
/// Real tmux-pane PID re-discovery requires a live tmux server, which
/// CI cannot rely on. This in-process simulation exercises the same
/// code path: the watchdog reads `nodes/n-0001.json`, probes the
/// recorded PID via `kill(pid, 0)`, and treats an exited PID as
/// terminal — synthesizing a `node.report {failed: true,
/// reason: "agent-died"}` event for the supervisor's reducer to fold.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn v2_agent_pid_discovery_via_liveness_probe() {
    let home = TestHome::new();
    let run_id = create_run(&home, "spinoff", "v2-pid");
    // Forge a node.created carrying an agent_pid that points at our
    // own PID (definitely alive, so the watchdog must NOT synthesize a
    // failed report on the first --once tick).
    let our_pid = std::process::id();
    let report = home.path().join("v2-node.json");
    std::fs::write(
        &report,
        format!(
            r#"{{"kind":"spinoff","task":"x","agent_pid":{our_pid},"tmux_window":"never-existed"}}"#
        ),
    )
    .unwrap();
    run_ok(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        report.to_str().unwrap(),
    ]));

    // Tick the supervisor once. Since tmux_window is set and TMUX_BIN
    // is /usr/bin/true (no output), the watchdog will see no matching
    // window and may synthesize TmuxGone. To keep V2 about PID
    // semantics, blank the tmux_window field by overwriting the node
    // JSON's tmux_window to null and re-running.
    let node_p = run_dir(&home, &run_id).join("nodes").join("n-0001.json");
    let mut n: Value = serde_json::from_slice(&std::fs::read(&node_p).unwrap()).unwrap();
    n["tmux_window"] = Value::Null;
    std::fs::write(&node_p, serde_json::to_vec_pretty(&n).unwrap()).unwrap();

    // Now supervise --once: alive PID + no tmux probe → no synthesis. Disable
    // the spawn grace (this test is about PID liveness, not freshness); the
    // grace itself is covered by the dedicated `*_within_grace` regressions.
    run_ok(
        bin(&home)
            .env("TASKFLEET_WATCHDOG_GRACE_SECS", "0")
            .env("TASKFLEET_DEATH_GRACE_SECS", "0")
            .args(["--output", "json", "supervise", &run_id, "--once"]),
    );
    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert_eq!(
        count_kind(&events, "node.report"),
        0,
        "alive PID must not synthesize a failed node.report"
    );

    // Swap the PID to one that is guaranteed dead (PID 1 may be
    // privileged-owned but not "dead"; use a never-allocated high PID
    // like 0x7FFF_FFFE). On macOS, signal to PID 999_999_999 returns
    // ESRCH unless someone forked an unreasonable number of times.
    let mut n: Value = serde_json::from_slice(&std::fs::read(&node_p).unwrap()).unwrap();
    n["agent_pid"] = Value::from(0x3FFF_FFFE_i64);
    std::fs::write(&node_p, serde_json::to_vec_pretty(&n).unwrap()).unwrap();
    run_ok(
        bin(&home)
            .env("TASKFLEET_WATCHDOG_GRACE_SECS", "0")
            .env("TASKFLEET_DEATH_GRACE_SECS", "0")
            .args(["--output", "json", "supervise", &run_id, "--once"]),
    );
    assert!(
        count_kind(&events, "node.report") >= 1,
        "dead PID must synthesize a failed node.report. events={:?}",
        read_events(&events)
            .into_iter()
            .map(|v| v["kind"].clone())
            .collect::<Vec<_>>()
    );
}

/// V3: `kill(pid, 0)` + `start_time` identity defense.
///
/// Verifies that (1) `start_time` is stable across reads, (2) supplying
/// a wildly wrong `start_time` forces `Recycled` rather than `Alive` for
/// the watchdog's own PID, and (3) the watchdog's verdict on a dead
/// PID is `Dead`. Cross-platform via the `sysinfo` crate path.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn v3_kill_and_start_time_identity() {
    // Stability: two reads of our own start_time differ by ≤ 1s.
    // (Already covered by a unit test on watchdog.rs; here we
    // additionally drive it through the public CLI by running
    // `supervise --once` repeatedly and confirming the supervisor does
    // not falsely declare itself dead.)
    let home = TestHome::new();
    let run_id = create_run(&home, "spinoff", "v3-st");
    let our_pid = std::process::id();
    let report = home.path().join("v3.json");
    std::fs::write(
        &report,
        format!(r#"{{"kind":"spinoff","task":"x","agent_pid":{our_pid}}}"#),
    )
    .unwrap();
    run_ok(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        report.to_str().unwrap(),
    ]));
    // Record a deliberately wrong start_time so the watchdog flags Recycled.
    let node_p = run_dir(&home, &run_id).join("nodes").join("n-0001.json");
    let mut n: Value = serde_json::from_slice(&std::fs::read(&node_p).unwrap()).unwrap();
    n["agent_pid_start_time"] = Value::String("1970-01-01T00:00:00Z".into());
    n["tmux_window"] = Value::Null;
    std::fs::write(&node_p, serde_json::to_vec_pretty(&n).unwrap()).unwrap();

    // Grace disabled: this test asserts the recycled-PID verdict, not freshness.
    run_ok(
        bin(&home)
            .env("TASKFLEET_WATCHDOG_GRACE_SECS", "0")
            .env("TASKFLEET_DEATH_GRACE_SECS", "0")
            .args(["--output", "json", "supervise", &run_id, "--once"]),
    );
    let events = run_dir(&home, &run_id).join("events.jsonl");
    let reports = read_events(&events)
        .into_iter()
        .filter(|v| v["kind"] == "node.report")
        .collect::<Vec<_>>();
    assert_eq!(reports.len(), 1, "recycled PID must synthesize one report");
    assert_eq!(reports[0]["data"]["reason"], "agent-pid-recycled");
}

/// Forge a `node.created` for `n-0001` carrying `agent_pid`, then null its
/// `tmux_window` so the watchdog's verdict turns purely on PID liveness (no
/// tmux probe). Returns the node projection path so the caller can mutate it
/// further (e.g. backdate `started_at` to age the node past the spawn grace).
fn forge_pid_node(home: &TempDir, run_id: &str, agent_pid: i64) -> std::path::PathBuf {
    let node = home.path().join(format!("wd-node-{run_id}.json"));
    std::fs::write(
        &node,
        format!(r#"{{"kind":"spinoff","task":"x","agent_pid":{agent_pid}}}"#),
    )
    .unwrap();
    run_ok(bin(home).args([
        "--output",
        "json",
        "event",
        "create",
        run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        node.to_str().unwrap(),
    ]));
    let node_p = run_dir(home, run_id).join("nodes").join("n-0001.json");
    let mut n: Value = serde_json::from_slice(&std::fs::read(&node_p).unwrap()).unwrap();
    n["tmux_window"] = Value::Null;
    std::fs::write(&node_p, serde_json::to_vec_pretty(&n).unwrap()).unwrap();
    node_p
}

/// supervisor-watchdog-misfire-on-fresh-spawn (regression): a node whose
/// recorded `agent_pid` reads *dead* but which was created only moments ago
/// (well within the default spawn grace) must NOT be terminalized. This is the
/// destructive bug — the watchdog used to synthesize `agent-died` within
/// milliseconds of `node.created`, before the real agent had a chance to
/// checkpoint that it is alive, and auto-cleanup would then tear down the live
/// agent's worktree. The default grace (no `TASKFLEET_WATCHDOG_GRACE_SECS` override)
/// must suppress the synthesis. The test runs far inside the 5s window, so no
/// `sleep` is needed.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn fresh_spawn_dead_pid_suppressed_within_grace() {
    let home = TestHome::new();
    let run_id = create_run(&home, "spinoff", "fresh-dead-grace");
    // Guaranteed-dead PID + a node created just now (started_at ≈ now).
    forge_pid_node(&home, &run_id, 0x3FFF_FFFE_i64);

    run_ok(bin(&home).args(["--output", "json", "supervise", &run_id, "--once"]));

    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert_eq!(
        count_kind(&events, "node.report"),
        0,
        "a fresh node within the spawn grace must not be terminalized even \
         though its PID reads dead, events={:?}",
        read_events(&events)
            .into_iter()
            .map(|v| v["kind"].clone())
            .collect::<Vec<_>>()
    );
}

/// Companion to the regression above: a fresh node whose agent PID is genuinely
/// alive (our own) is likewise left alone — the grace never *causes* a false
/// positive, it only suppresses one. Covers criterion 4's "fresh spawn with
/// alive agent does not trigger watchdog".
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn fresh_spawn_alive_pid_no_synthesis() {
    let home = TestHome::new();
    let run_id = create_run(&home, "spinoff", "fresh-alive");
    forge_pid_node(&home, &run_id, i64::from(std::process::id()));

    run_ok(bin(&home).args(["--output", "json", "supervise", &run_id, "--once"]));

    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert_eq!(
        count_kind(&events, "node.report"),
        0,
        "an alive fresh node must not be terminalized"
    );
}

/// The other half of the contract: once a node has aged past the spawn grace,
/// a dead agent PID DOES synthesize a terminal `agent-died` report — the grace
/// delays the verdict, it does not disable it. Mirrors "spawn then immediately
/// kill -9 the agent" by backdating `started_at` so the node is comfortably
/// older than the grace. Covers criterion 4's "fresh spawn with dead agent does
/// trigger watchdog (after grace)" and criterion 2.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn dead_pid_synthesizes_after_grace() {
    let home = TestHome::new();
    let run_id = create_run(&home, "spinoff", "dead-after-grace");
    let node_p = forge_pid_node(&home, &run_id, 0x3FFF_FFFE_i64);

    // Age the node well past the 5s grace without sleeping: rewrite started_at.
    let mut n: Value = serde_json::from_slice(&std::fs::read(&node_p).unwrap()).unwrap();
    n["started_at"] = Value::String("2020-01-01T00:00:00Z".into());
    std::fs::write(&node_p, serde_json::to_vec_pretty(&n).unwrap()).unwrap();

    // Zero the residual crash backstop's post-death grace so the single `--once`
    // tick fires immediately (design.md §2.1a); the persisted defer-then-fire
    // behavior at a non-zero grace is covered by
    // `dead_pid_deferred_within_death_grace_then_fires`.
    run_ok(bin(&home).env("TASKFLEET_DEATH_GRACE_SECS", "0").args([
        "--output",
        "json",
        "supervise",
        &run_id,
        "--once",
    ]));

    let events = run_dir(&home, &run_id).join("events.jsonl");
    let reports = read_events(&events)
        .into_iter()
        .filter(|v| v["kind"] == "node.report")
        .collect::<Vec<_>>();
    assert_eq!(
        reports.len(),
        1,
        "a node past the spawn grace with a dead PID must synthesize one \
         terminal report"
    );
    assert_eq!(reports[0]["data"]["reason"], "agent-died");
}

/// The residual crash backstop's PERSISTED post-death grace (design.md §2.1a):
/// with a non-zero grace, the FIRST tick that observes the dead PID only records
/// the durable `node.death_observed` anchor and DEFERS — it does not synthesize a
/// terminal report. A later tick past the grace fires it. This is what lets an
/// in-flight `worker.exited` / merge append win the race.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn dead_pid_deferred_within_death_grace_then_fires() {
    let home = TestHome::new();
    let run_id = create_run(&home, "spinoff", "death-grace-defer");
    let node_p = forge_pid_node(&home, &run_id, 0x3FFF_FFFE_i64);

    // Past the spawn grace, so only the death grace governs.
    let mut n: Value = serde_json::from_slice(&std::fs::read(&node_p).unwrap()).unwrap();
    n["started_at"] = Value::String("2020-01-01T00:00:00Z".into());
    std::fs::write(&node_p, serde_json::to_vec_pretty(&n).unwrap()).unwrap();

    // A generous death grace: the first tick must DEFER, recording the anchor.
    run_ok(bin(&home).env("TASKFLEET_DEATH_GRACE_SECS", "3600").args([
        "--output",
        "json",
        "supervise",
        &run_id,
        "--once",
    ]));
    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert_eq!(
        count_kind(&events, "node.report"),
        0,
        "the first tick within the death grace must NOT terminalize"
    );
    assert_eq!(
        count_kind(&events, "node.death_observed"),
        1,
        "the first tick records the durable first-death anchor"
    );

    // A later tick with a zero grace: the anchor is now older than the grace, so
    // the backstop fires.
    run_ok(bin(&home).env("TASKFLEET_DEATH_GRACE_SECS", "0").args([
        "--output",
        "json",
        "supervise",
        &run_id,
        "--once",
    ]));
    let reports = read_events(&events)
        .into_iter()
        .filter(|v| v["kind"] == "node.report")
        .collect::<Vec<_>>();
    assert_eq!(
        reports.len(),
        1,
        "the backstop fires once the grace elapses"
    );
    assert_eq!(reports[0]["data"]["reason"], "agent-died");
}

/// Wedge a newline-terminated garbage line into the MIDDLE of `events`
/// (a valid record before AND after it), so the supervisor's own
/// backward-scanning appends still land on a parseable last record. Returns
/// the rewritten contents written to disk.
fn wedge_corrupt_middle_line(events: &Path) {
    let original = std::fs::read_to_string(events).unwrap();
    let mut trailing: Value = serde_json::from_str(original.lines().next().unwrap()).unwrap();
    trailing["seq"] = Value::from(900);
    let trailing = serde_json::to_string(&trailing).unwrap();
    let mut rewritten = String::new();
    rewritten.push_str(original.trim_end_matches('\n'));
    rewritten.push('\n');
    rewritten.push_str("{this is not valid json at all\n");
    rewritten.push_str(&trailing);
    rewritten.push('\n');
    std::fs::write(events, rewritten).unwrap();
}

/// corrupt-line-quarantine: by default the supervisor *heals* a poisoned
/// own-run `events.jsonl` — the corrupt line is renamed aside to a
/// `.corrupt-<ts>.bak` backup, a recovered log is written in its place, and a
/// single `supervisor.event_log_quarantined` event is emitted. After the heal
/// the log replays strictly (no poison bytes left for a future reader).
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn corrupt_tail_line_is_quarantined_and_log_heals() {
    let home = TestHome::new();
    let run_id = create_run(&home, "spinoff", "corrupt-tail");
    let events = run_dir(&home, &run_id).join("events.jsonl");
    wedge_corrupt_middle_line(&events);

    run_ok(bin(&home).args(["--output", "json", "supervise", &run_id, "--max-iter", "4"]));

    // The healed log now parses strictly end-to-end (no corrupt line left),
    // and carries exactly one quarantine event — never a skip event.
    let evs = read_events(&events);
    let quarantined: Vec<&Value> = evs
        .iter()
        .filter(|v| v["kind"] == "supervisor.event_log_quarantined")
        .collect();
    assert_eq!(
        quarantined.len(),
        1,
        "expected exactly one quarantine event"
    );
    assert_eq!(
        count_kind(&events, "supervisor.event_log_skipped_line"),
        0,
        "quarantine replaces the in-memory skip diagnostic"
    );
    assert!(
        !std::fs::read_to_string(&events)
            .unwrap()
            .contains("not valid json"),
        "the corrupt line must be excised from the recovered log"
    );

    // The quarantine event names a backup file that exists and holds the
    // original (still-poisoned) bytes.
    let backup = quarantined[0]["data"]["backup_path"].as_str().unwrap();
    let backup = Path::new(backup);
    assert!(
        backup.exists(),
        "backup file must exist: {}",
        backup.display()
    );
    assert!(std::fs::read_to_string(backup)
        .unwrap()
        .contains("not valid json"));
    assert!(quarantined[0]["data"]["removed_byte_offsets"]
        .as_array()
        .is_some_and(|a| !a.is_empty()));
}

/// F17 (opt-out path): with `--no-quarantine-corrupt-lines` the supervisor
/// keeps the P2 behavior — a corrupt line is reported once via
/// `supervisor.event_log_skipped_line` and skipped in memory, the bytes left
/// on disk, never re-erroring on the same offset every tick.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn corrupt_tail_line_is_skipped_once_without_looping() {
    let home = TestHome::new();
    let run_id = create_run(&home, "spinoff", "corrupt-tail");
    let events = run_dir(&home, &run_id).join("events.jsonl");
    wedge_corrupt_middle_line(&events);

    run_ok(bin(&home).args([
        "--output",
        "json",
        "supervise",
        &run_id,
        "--max-iter",
        "4",
        "--no-quarantine-corrupt-lines",
    ]));

    // Tolerant read: the corrupt line is intentionally still on disk (we skip
    // in memory, never mutate the file), so `read_events`' strict parse can't
    // be used here.
    let skipped: Vec<Value> = std::fs::read_to_string(&events)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["kind"] == "supervisor.event_log_skipped_line")
        .collect();
    assert_eq!(
        skipped.len(),
        1,
        "expected exactly one skip event, got {}",
        skipped.len()
    );
    assert!(
        skipped[0]["data"]["byte_offset"].is_number(),
        "skip event carries the byte offset"
    );
    assert!(
        skipped[0]["data"]["line_excerpt"]
            .as_str()
            .unwrap_or_default()
            .contains("not valid json"),
        "skip event carries a line excerpt: {:?}",
        skipped[0]["data"]
    );
    // The poison bytes are intentionally left on disk in the opt-out path.
    assert!(std::fs::read_to_string(&events)
        .unwrap()
        .contains("not valid json"));
}

/// supervise-gates-jsonl-poll-tolerance: readiness polling must tolerate a
/// torn trailing JSONL line — the half-written record a detached supervisor is
/// mid-append on when a poll happens to read the file. The strict `read_events`
/// would panic on it; the lenient `count_kind_lenient` (used by `wait_for_kind`)
/// skips it and still counts the intact records before it.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn lenient_poll_skips_torn_trailing_line() {
    let home = TestHome::new();
    let run_id = create_run(&home, "spinoff", "torn-tail");
    let events = run_dir(&home, &run_id).join("events.jsonl");

    // Append one intact record of a known kind, then a torn trailing line
    // (truncated mid-object, no closing brace) as if a write were caught
    // in flight.
    let mut contents = std::fs::read_to_string(&events).unwrap();
    if !contents.ends_with('\n') {
        contents.push('\n');
    }
    contents.push_str(r#"{"kind":"torn.marker","seq":99}"#);
    contents.push('\n');
    contents.push_str(r#"{"kind":"torn.marker","seq":100"#); // torn: no closing brace / newline
    std::fs::write(&events, contents).unwrap();

    // Strict parse would panic on the torn tail; the lenient counter skips it
    // and counts only the intact marker.
    assert_eq!(count_kind_lenient(&events, "torn.marker"), 1);
    // And `wait_for_kind`, which polls via the lenient counter, resolves
    // immediately instead of hanging until the deadline.
    assert_eq!(wait_for_kind(&events, "torn.marker", 1), 1);
}

/// §7.8: SIGINT exits 130, SIGTERM exits 143, and the `supervisor.exited`
/// event records `reason:"signal"` + the specific `signal` name. Regression
/// guard for the supervisor-process review FIX (F3) — `ctrlc` could not
/// surface which signal fired, so the old code exited 0 with no signal field.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn signal_exit_codes_and_payload() {
    use std::io::Read;
    for (sig, code, name) in [("TERM", 143, "SIGTERM"), ("INT", 130, "SIGINT")] {
        let home = TestHome::new();
        let run_id = create_run(&home, "spinoff", "sig");
        // Long-lived supervisor (no --once): spawn, let it enter the loop,
        // then deliver the signal and assert the exit code + event.
        let mut child = bin(&home)
            .args(["supervise", &run_id])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn supervisor");
        // Poll for readiness (the PID file appears once the supervisor has
        // installed its signal handlers and entered the loop) rather than a
        // fixed sleep, so the kill never races startup even on a loaded CI.
        let pid_file = run_dir(&home, &run_id).join("supervisor.pid");
        assert!(
            poll_until(POLL_DEADLINE, || pid_file.exists()),
            "supervisor did not start in time: {}",
            pid_file.display()
        );
        // Check the kill(2) return: a dropped signal (ESRCH/EPERM) would
        // otherwise manifest only as the unbounded-wait hang below.
        let rc = unsafe { libc::kill(child.id() as i32, sig_num(sig)) };
        assert_eq!(
            rc,
            0,
            "{name}: kill({sig}) failed: {}",
            std::io::Error::last_os_error()
        );
        // Bound the wait: on a saturated runner the exit can lag the signal by
        // several seconds, and a dropped signal / hung handler must not hang
        // the test forever. Poll try_wait (WNOHANG-equivalent) every 50ms, and
        // kill+reap on timeout. The exit-code assertion happens AT THE END,
        // once we have a real status — never mid-wait.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(s) = child.try_wait().expect("try_wait") {
                break s;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{name}: supervisor did not exit within 10s of {sig}");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert_eq!(
            status.code(),
            Some(code),
            "{name} must exit {code}, got {status:?}"
        );
        // §7.8: a cleanly-signalled supervisor removes its own PID file.
        assert!(
            !pid_file.exists(),
            "{name}: supervisor.pid must be removed on signal exit"
        );
        let events = run_dir(&home, &run_id).join("events.jsonl");
        let mut s = String::new();
        std::fs::File::open(&events)
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        let exited = s
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .find(|v| v["kind"] == "supervisor.exited")
            .expect("supervisor.exited present");
        assert_eq!(exited["data"]["reason"], "signal");
        assert_eq!(exited["data"]["signal"], name);
    }
}

/// log-delivery-hardening: a supervisor signalled with SIGTERM must FLUSH its
/// buffered tracing events to the JSONL log before exiting. The signal path
/// exits via `process::exit(143)`, which bypasses the `LogGuard`'s `Drop`, so
/// `dispatch` calls `flush_logs()` first (the same flush-on-exit contract
/// `event tail`'s signal path uses). Without it, the supervisor's boot/loop
/// log events are silently lost on shutdown.
///
/// To make this a *real* regression guard (an idle supervisor's worker would
/// otherwise drain naturally before the signal, hiding a missing flush), the
/// `TASKFLEET_TEST_SLOW_LOG_WRITES` hook throttles each log `write(2)` so the
/// buffered "supervisor started" event is provably still in flight at signal
/// time — only the explicit `flush_logs()` drain gets it to disk before exit.
// The slow-writer injection is deliberately unavailable in production
// binaries, so this deterministic flush test only applies to debug builds.
#[cfg(debug_assertions)]
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn sigterm_flushes_buffered_supervisor_logs() {
    let home = TestHome::new();
    let run_id = create_run(&home, "spinoff", "sigterm-flush");

    // Long-lived supervisor (no --once) so it is mid-loop when signalled.
    // Slow log writes (250ms each) keep the worker behind the shutdown path,
    // so the flush — not luck — is what lands the event on disk.
    let mut child = bin(&home)
        .env("TASKFLEET_TEST_SLOW_LOG_WRITES", "250")
        .args(["supervise", &run_id])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn supervisor");

    // Wait until it has booted (PID file ⇒ handlers installed, loop entered,
    // and the "supervisor started" event already emitted into the appender).
    let pid_file = run_dir(&home, &run_id).join("supervisor.pid");
    assert!(
        poll_until(POLL_DEADLINE, || pid_file.exists()),
        "supervisor did not start in time: {}",
        pid_file.display()
    );

    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let status = child.wait().expect("wait");
    assert_eq!(status.code(), Some(143), "SIGTERM must exit 143");

    // The supervisor emits a "received termination signal" breadcrumb into the
    // shared JSONL log (init_logging) immediately before the flush, so under
    // the slow-write throttle it is provably still buffered at exit. Only the
    // explicit flush_logs() drain on the signal path lands it on disk; without
    // that flush, process::exit cuts the worker off mid-write and it is lost.
    let log = home.path().join("logs").join("taskfleet.log.jsonl");
    let contents = std::fs::read_to_string(&log).unwrap_or_default();
    let saw_shutdown_log = contents
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .any(|v| {
            v["target"] == "taskfleet::supervise"
                && v["fields"]["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("received termination signal"))
        });
    assert!(
        saw_shutdown_log,
        "supervisor's buffered shutdown log line was not flushed on SIGTERM; \
         log contents:\n{contents}"
    );
}

/// `signal-exit-143-regression`: a SIGTERM/SIGINT delivered DURING boot — after
/// the pid file is claimed (the readiness signal a test/parent polls on) but
/// before the supervisor confirms readiness — must still honor §7.8 (exit
/// 143/130 with a `supervisor.exited{reason:"signal"}` event), NOT bail out via
/// the old `terminated_during_boot` error path (which returned
/// `ExitKind::System` = exit 2). This was the intermittent CI failure: under
/// `--release` load the boot window widened enough that the SIGTERM landed
/// mid-boot and the supervisor exited 2 (`unix_wait_status(512)`) instead of 143.
///
/// `TASKFLEET_TEST_SLOW_BOOT` is a bounded barrier that holds boot right after the pid
/// claim UNTIL a termination signal is observed, so the signal is *proven* to
/// land in the boot-window branch — a plain sleep could expire under a
/// descheduled test and let the signal hit the loop instead, passing even
/// against the buggy code.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn signal_during_boot_exits_143() {
    use std::io::Read;
    for (sig, code, name) in [("TERM", 143, "SIGTERM"), ("INT", 130, "SIGINT")] {
        let home = TestHome::new();
        let run_id = create_run(&home, "spinoff", "sig-boot");
        // The barrier parks boot in the pid-claimed-but-not-yet-ready window;
        // the 5s cap is a safety bound (the signal below releases it far sooner).
        let mut child = bin(&home)
            .env("TASKFLEET_TEST_SLOW_BOOT", "5000")
            .args(["supervise", &run_id])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn supervisor");
        let pid_file = run_dir(&home, &run_id).join("supervisor.pid");
        assert!(
            poll_until(POLL_DEADLINE, || pid_file.exists()),
            "{name}: supervisor did not claim pid file in time: {}",
            pid_file.display()
        );
        let rc = unsafe { libc::kill(child.id() as i32, sig_num(sig)) };
        assert_eq!(
            rc,
            0,
            "{name}: kill({sig}) failed: {}",
            std::io::Error::last_os_error()
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(s) = child.try_wait().expect("try_wait") {
                break s;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{name}: supervisor did not exit within 10s of a boot-window {sig}");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert_eq!(
            status.code(),
            Some(code),
            "boot-window {name} must exit {code}, got {status:?}"
        );
        assert!(
            !pid_file.exists(),
            "{name}: supervisor.pid must be removed on a boot-window signal exit"
        );
        let events = run_dir(&home, &run_id).join("events.jsonl");
        let mut s = String::new();
        std::fs::File::open(&events)
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        let exited = s
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .find(|v| v["kind"] == "supervisor.exited")
            .expect("supervisor.exited present even on a boot-window signal");
        assert_eq!(exited["data"]["reason"], "signal");
        assert_eq!(exited["data"]["signal"], name);
    }
}

/// F15 lock-aware watchdog: when a node already carries a real
/// `last_report`, the watchdog must DEFER to it and not synthesize a second
/// terminal `node.report`, even though the agent PID is dead. Regression for
/// the duplicate-terminal-report race — the watchdog re-reads `last_report`
/// (now under the run lock) before committing a synthetic report.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn watchdog_defers_when_report_already_present() {
    let home = TestHome::new();
    let run_id = create_run(&home, "spinoff", "wd-defer");
    let our_pid = std::process::id();
    let report = home.path().join("wd-node.json");
    std::fs::write(
        &report,
        format!(r#"{{"kind":"spinoff","task":"x","agent_pid":{our_pid}}}"#),
    )
    .unwrap();
    run_ok(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        report.to_str().unwrap(),
    ]));

    // Forge the projection into the watchdog's danger zone: a NON-terminal
    // node whose agent PID is dead (so the watchdog wants to synthesize) but
    // which ALREADY carries a real `last_report` (so it must defer). tmux is
    // nulled so the probe is pure-PID.
    let node_p = run_dir(&home, &run_id).join("nodes").join("n-0001.json");
    let mut n: Value = serde_json::from_slice(&std::fs::read(&node_p).unwrap()).unwrap();
    n["agent_pid"] = Value::from(0x3FFF_FFFE_i64); // guaranteed-dead pid
    n["tmux_window"] = Value::Null;
    n["last_report"] = serde_json::json!({"success": true, "summary": "real report"});
    std::fs::write(&node_p, serde_json::to_vec_pretty(&n).unwrap()).unwrap();

    // Grace disabled so the deferral (present last_report) — not freshness — is
    // what blocks synthesis; otherwise this could pass for the wrong reason.
    run_ok(
        bin(&home)
            .env("TASKFLEET_WATCHDOG_GRACE_SECS", "0")
            .env("TASKFLEET_DEATH_GRACE_SECS", "0")
            .args(["--output", "json", "supervise", &run_id, "--once"]),
    );

    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert_eq!(
        count_kind(&events, "node.report"),
        0,
        "watchdog must defer to the present last_report and synthesize nothing, events={:?}",
        read_events(&events)
            .into_iter()
            .map(|v| v["kind"].clone())
            .collect::<Vec<_>>()
    );
}

/// supervisor-child-detach-reap: a supervisor spawned through the detached
/// path (`run reattach` → `spawn_and_reap`, which `setsid`s into its own
/// session) must SURVIVE a `SIGHUP` delivered to its spawner's process group
/// — the exact signal a closing terminal sends. Without `setsid` the
/// supervisor would share the spawner's group and die.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn spawned_supervisor_survives_sighup_to_spawner_group() {
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::time::Instant;

    let home = TestHome::new();
    let run_id = create_run(&home, "spinoff", "sighup-survive");
    // Give the run a live worker node so the supervisor keeps supervising it —
    // otherwise the no-worker guard terminalizes a node-less run within a few
    // ticks (that guard is itself under test in
    // `no_worker_node_run_terminalizes_failed`). This mirrors a real supervised
    // run, which is what the SIGHUP-survival property is about.
    let _agent = forge_live_worker_node(&home, &run_id);

    // The "spawner": a shell, made a process-group LEADER via setpgid(0,0),
    // that runs `run reattach` (which forks the detached supervisor) and then
    // sleeps to keep its process group alive until we signal it. A bare
    // (no `--once`) reattach yields a long-lived supervisor that loops idle
    // over the live-worker run, so it stays alive for us to signal.
    let bin_path = env!("CARGO_BIN_EXE_taskfleet");
    let script =
        format!("{bin_path} --output json run reattach {run_id} >/dev/null 2>&1; sleep 30");
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(script);
    cmd.env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path());
    cmd.env("TASKFLEET_TEST_SKIP_MATERIALIZE", "1");
    cmd.env("TMUX_BIN", "/usr/bin/true");
    // Keep the watchdog from synthesizing an agent-death for the forged node
    // during the test window, so the supervisor stays alive to receive SIGHUP.
    cmd.env("TASKFLEET_WATCHDOG_GRACE_SECS", "60");
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    // SAFETY: setpgid(0,0) is async-signal-safe; it makes the shell its own
    // process-group leader (pgid == shell pid) so we can signal that group.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut spawner = cmd.spawn().expect("spawn shell");
    let spawner_pgid = spawner.id() as i32; // == pid because of setpgid(0,0)

    let deadline = Instant::now() + Duration::from_secs(30);
    let pid_file = run_dir(&home, &run_id).join("supervisor.pid");
    let sup_pid = loop {
        if let Some(p) = read_first_token_pid(&pid_file) {
            if pid_alive(p) {
                break p;
            }
        }
        if Instant::now() >= deadline {
            let _ = kill_group(spawner_pgid);
            let _ = spawner.wait();
            panic!("supervisor did not write a live pid file in time");
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    // The supervisor must NOT be in the spawner's process group (it setsid'd).
    // Sending SIGHUP to the spawner group must therefore leave it running.
    unsafe {
        libc::kill(-spawner_pgid, libc::SIGHUP);
    }
    let _ = spawner.wait(); // reap the (now-HUP'd) shell

    // Give any errant SIGHUP propagation a moment: poll_until returns true
    // only if the supervisor DIED within the window, so survival means it
    // returned false (timed out waiting for death).
    assert!(
        !poll_until(Duration::from_secs(3), || !pid_alive(sup_pid)),
        "supervisor (pid {sup_pid}) must survive SIGHUP to the spawner's group"
    );
    assert!(
        pid_alive(sup_pid),
        "supervisor (pid {sup_pid}) must still be alive after spawner-group SIGHUP"
    );

    // Cleanup: stop the long-lived supervisor so it does not leak.
    unsafe {
        libc::kill(sup_pid as i32, libc::SIGTERM);
    }
    // Best-effort reap wait so it is fully gone (it is not our direct child —
    // it reparented to init — so we poll instead of wait()).
    poll_until(Duration::from_secs(5), || !pid_alive(sup_pid));
}

/// Read the first whitespace-delimited token of a pid file as a u32 (the
/// `"<pid> <start_time>"` or legacy `"<pid>"` format), mirroring the
/// supervisor's own reader without depending on crate internals.
fn read_first_token_pid(path: &Path) -> Option<u32> {
    let s = std::fs::read_to_string(path).ok()?;
    s.split_whitespace().next()?.parse::<u32>().ok()
}

/// `kill(pid, 0)` liveness probe (no signal sent). True iff the process
/// exists and we may signal it (alive, possibly foreign-owned).
fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// SIGTERM a whole process group (best-effort cleanup helper).
fn kill_group(pgid: i32) -> std::io::Result<()> {
    let rc = unsafe { libc::kill(-pgid, libc::SIGTERM) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn sig_num(sig: &str) -> libc::c_int {
    match sig {
        "TERM" => libc::SIGTERM,
        "INT" => libc::SIGINT,
        _ => unreachable!(),
    }
}

/// V8: `run reattach` end-to-end.
///
/// Start a run, fork a one-shot supervisor via `run reattach --once`,
/// confirm the events.jsonl picks up the supervisor.reattached marker.
/// Reattach again: the previous supervisor is dead, so the new one
/// boots cleanly. Demonstrates the stale-PID detection path.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn v8_reattach_end_to_end() {
    let home = TestHome::new();
    let run_id = create_run(&home, "spinoff", "v8");

    run_ok(bin(&home).args(["--output", "json", "run", "reattach", &run_id, "--once"]));
    // Wait for the spawned --once supervisor to boot, tick, and write its
    // supervisor.exited event. Polling (not a fixed sleep) because the
    // detached process's latency varies and a single 500ms sleep is flaky.
    let events = run_dir(&home, &run_id).join("events.jsonl");
    assert!(wait_for_kind(&events, "supervisor.exited", 1) >= 1);
    assert!(count_kind(&events, "supervisor.reattach-requested") >= 1);
    assert!(count_kind(&events, "supervisor.reattached") >= 1);

    // The `--once` supervisor writes `supervisor.exited` just *before* it
    // removes its pid file and exits. Wait for the pid file to disappear so
    // the second reattach sees a genuinely stale (dead) prior supervisor
    // rather than racing the still-dying one (which would refuse).
    let pid_file = run_dir(&home, &run_id).join("supervisor.pid");
    // Fatal on timeout: if the pid file never disappears the second reattach
    // would race the still-dying supervisor (the exact condition this wait
    // guards against) and fail later with a misleading reattach-count error, so
    // assert the precondition here with a useful message instead.
    assert!(
        poll_until(POLL_DEADLINE, || !pid_file.exists()),
        "prior --once supervisor did not remove its pid file in time: {}",
        pid_file.display()
    );

    // Second reattach: prior PID is stale.
    run_ok(bin(&home).args(["--output", "json", "run", "reattach", &run_id, "--once"]));
    assert!(wait_for_kind(&events, "supervisor.reattach-requested", 2) >= 2);
}

/// V9: `run cancel` synthesized-report propagation.
///
/// A child run with a non-terminal node receives `run cancel`. The
/// cancel verb synthesizes a terminal `node.report {cancelled: true}`.
/// A parent supervisor that tails the child sees the cancelled report
/// and must advance `last_processed_report_seq_by_child` so a replay is a
/// no-op. (The 0.2 cut removed the mid-run discussion/spinoff derivation, so no
/// discussions/ or spinoffs/ projection dirs are ever created — asserted here as
/// a regression guard that the machinery is truly gone.)
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn v9_cancel_synthesizes_report_no_spinoffs() {
    let home = TestHome::new();
    let parent = create_run(&home, "fan-out", "v9-parent");
    let p_node = home.path().join("v9-pn.json");
    std::fs::write(&p_node, r#"{"kind":"fan-out","task":"x"}"#).unwrap();
    run_ok(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &parent,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        p_node.to_str().unwrap(),
    ]));

    let child = run_ok(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "v9-c",
        "--parent-run-id",
        &parent,
        "--parent-node-id",
        "n-0001",
    ]))["data"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let c_node = home.path().join("v9-cn.json");
    std::fs::write(&c_node, r#"{"kind":"spinoff","task":"x"}"#).unwrap();
    run_ok(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &child,
        "--kind",
        "node.created",
        "--node-id",
        "n-0001",
        "--from-file",
        c_node.to_str().unwrap(),
    ]));

    // Cancel the child — synthesizes a node.report {cancelled: true}.
    run_ok(bin(&home).args(["--output", "json", "run", "cancel", &child]));

    // Pre-seed parent to skip child-supervisor fork.
    std::fs::write(
        run_dir(&home, &parent).join("supervisor.state.json"),
        format!(r#"{{"schema_version":1,"spawned_children":{{"{child}":1}}}}"#),
    )
    .unwrap();
    run_ok(bin(&home).args(["--output", "json", "supervise", &parent, "--once"]));

    // Parent must have zero spinoffs/discussions derived from the
    // cancelled child report.
    let n_spin = std::fs::read_dir(run_dir(&home, &parent).join("spinoffs"))
        .map_or(0, std::iter::Iterator::count);
    let n_disc = std::fs::read_dir(run_dir(&home, &parent).join("discussions"))
        .map_or(0, std::iter::Iterator::count);
    assert_eq!(n_spin, 0, "cancelled child must not propagate spinoffs");
    assert_eq!(n_disc, 0, "cancelled child must not propagate discussions");

    // Parent's spawning-node JSON should record the child cursor so a
    // replay is a no-op.
    let parent_node: Value = serde_json::from_slice(
        &std::fs::read(run_dir(&home, &parent).join("nodes").join("n-0001.json")).unwrap(),
    )
    .unwrap();
    assert!(
        parent_node["last_processed_report_seq_by_child"]
            .get(&child)
            .is_some(),
        "cursor not advanced: {parent_node:?}"
    );

    // The cursor is now event-sourced: a `supervisor.cursor_advanced` event
    // must back the projection update so a from-scratch rebuild reproduces it
    // (issue `supervisor-state-not-event-sourced`).
    let parent_events =
        std::fs::read_to_string(run_dir(&home, &parent).join("events.jsonl")).unwrap();
    let cursor_evs: Vec<Value> = parent_events
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["kind"] == "supervisor.cursor_advanced")
        .collect();
    assert_eq!(
        cursor_evs.len(),
        1,
        "exactly one cursor_advanced event must back the projection: {parent_events}"
    );
    assert_eq!(
        cursor_evs[0]["data"]["child_run_id"],
        serde_json::json!(child)
    );
    assert_eq!(cursor_evs[0]["node_id"], serde_json::json!("n-0001"));
}

/// Orphan defense: a long-lived supervisor whose run dir vanishes out
/// from under it must self-terminate cleanly (exit 0) rather than poll a
/// deleted directory forever and keep forking children — the root cause
/// of the `test-harness-leaks-supervisors` orphan accumulation.
///
/// We remove only `manifest.json` (the trigger) while leaving the events
/// log intact, so we can additionally assert the documented
/// `supervisor.self-terminated` marker lands. (When the *whole* run dir
/// is removed — the TempDir-teardown case — there is no log to write to,
/// and the event is correctly skipped; only the clean exit is observable.)
#[test]
fn self_terminate_when_run_dir_vanishes() {
    let home = TestHome::new();
    let run_id = create_run(&home, "spinoff", "self-term");
    let rdir = run_dir(&home, &run_id);
    let mut supervisor = spawn_ready_supervisor(&home, &run_id);
    let pid_file = rdir.join("supervisor.pid");

    // Yank the manifest out from under the fully booted supervisor (leaving
    // events.jsonl intact).
    std::fs::remove_file(rdir.join("manifest.json")).expect("remove manifest");

    let status = supervisor.wait_for_exit();
    assert_eq!(
        status.code(),
        Some(0),
        "self-terminate must be a clean exit 0, got {status:?}"
    );

    // The surviving events log records the documented marker.
    let events = rdir.join("events.jsonl");
    assert!(
        count_kind(&events, "supervisor.self-terminated") >= 1,
        "expected a supervisor.self-terminated event, got {:?}",
        read_events(&events)
            .into_iter()
            .map(|v| v["kind"].clone())
            .collect::<Vec<_>>()
    );
    // The PID file is cleaned up on exit.
    assert!(
        !pid_file.exists(),
        "supervisor.pid must be removed on self-terminate"
    );
}

/// The original leak's exact failure mode: the *entire* run dir is removed
/// (a test `TempDir` teardown). The supervisor must self-terminate cleanly
/// (exit 0) AND must not resurrect the deleted directory — `state::save` /
/// `append_and_apply_event` write through `create_dir_all`, so a sloppy
/// implementation leaves a ghost dir behind after an operator's `rm -rf`.
#[test]
fn self_terminate_when_whole_run_dir_removed() {
    use std::time::Instant;

    let home = TestHome::new();
    let run_id = create_run(&home, "spinoff", "self-term-dir");
    let rdir = run_dir(&home, &run_id);

    let mut supervisor = spawn_ready_supervisor(&home, &run_id);

    // Remove the whole run dir out from under the supervisor. This races
    // the supervisor's per-tick `state.json` write, which can recreate a
    // file mid-walk and yield ENOTEMPTY. The first pass deletes
    // `manifest.json`, after which the orphan defense stops the
    // supervisor writing, so a short retry loop converges. (This is the
    // operator's `rm -rf` racing a live writer — a test concern, not a
    // product one; the product guarantee is "do not *resurrect* the dir",
    // asserted below.)
    let del_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match std::fs::remove_dir_all(&rdir) {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) if Instant::now() < del_deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => panic!("remove run dir: {e}"),
        }
    }

    let status = supervisor.wait_for_exit();
    assert_eq!(
        status.code(),
        Some(0),
        "self-terminate must be a clean exit 0, got {status:?}"
    );
    // The supervisor must NOT have recreated the directory the operator
    // (here, the test) deleted.
    assert!(
        !rdir.exists(),
        "supervisor must not resurrect the deleted run dir: {}",
        std::fs::read_dir(&rdir)
            .map(|d| d
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(", "))
            .unwrap_or_default()
    );
}

/// supervisor-spawn-fails-silently-at-run-create (suggested-fix #5): a
/// supervised run that never got a worker node (`node_count` 0) and has no
/// children can never make progress. Rather than poll `pending` forever — or,
/// for a reattached zombie, falsely exit `work-complete` with zero children —
/// the supervisor must terminalize the run as `failed` with reason
/// `no-worker-node`.
///
/// A `--skip-materialize` spinoff `run create` produces exactly this shape:
/// `run.created` only, no `node.created`, no supervisor. We then run the
/// supervisor against it (with the create-window grace overridden to 0 so the
/// guard's age gate does not defer) and assert it fails the run loudly.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn no_worker_node_run_terminalizes_failed() {
    let home = TestHome::new();
    let run_id = create_run(&home, "spinoff", "no-worker");
    let rdir = run_dir(&home, &run_id);
    let events = rdir.join("events.jsonl");

    // Sanity: this really is a zero-node run before the supervisor ticks.
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(rdir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["node_count"], 0, "precondition: no worker node");

    // Enough ticks (>= NO_WORKER_TICKS) for the guard to fire; the supervisor
    // exits early via all_work_done once it terminalizes. `TASKFLEET_NO_WORKER_GRACE_SECS=0`
    // disables the create-window age gate (production waits 15 min).
    run_ok(bin(&home).env("TASKFLEET_NO_WORKER_GRACE_SECS", "0").args([
        "--output",
        "json",
        "supervise",
        &run_id,
        "--max-iter",
        "12",
    ]));

    // The run is now terminal `failed` — not stuck `pending`.
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(rdir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(
        manifest["status"], "failed",
        "no-worker run must terminalize failed, not stay pending"
    );
    assert_eq!(latest_run_status(&events).as_deref(), Some("failed"));

    // The synthesized run.status carries the diagnostic reason, and the
    // supervisor exited reporting the spawn failure — never `work-complete`.
    let events_v = read_events(&events);
    let failed_reason = events_v
        .iter()
        .filter(|v| v["kind"] == "run.status" && v["data"]["status"] == "failed")
        .find_map(|v| v["data"]["reason"].as_str());
    assert_eq!(failed_reason, Some("no-worker-node"));
    let exit_reason = events_v
        .iter()
        .filter(|v| v["kind"] == "supervisor.exited")
        .find_map(|v| v["data"]["reason"].as_str());
    assert_eq!(
        exit_reason,
        Some("supervisor-spawn-failed"),
        "must not exit work-complete with zero children"
    );
}

/// The no-worker guard must NOT clip a legitimately in-flight creation. A run
/// whose `create.sh` is still materializing the worker sits at `node_count == 0`
/// for the whole create window (up to `--agent-startup-timeout`); a supervisor
/// reattached during that window sees the same shape. With the default
/// (production) create-window grace, a freshly-created zero-node run must stay
/// `pending`, not be terminalized `failed`.
#[test]
#[file_serial(key, path => "/tmp/taskfleet-test-supervise.lock")]
fn no_worker_guard_defers_within_create_window() {
    let home = TestHome::new();
    let run_id = create_run(&home, "spinoff", "young-no-worker");
    let rdir = run_dir(&home, &run_id);

    // No TASKFLEET_NO_WORKER_GRACE_SECS override: the default 15-min grace applies, so
    // this ~0s-old run is well inside the create window and must be left alone.
    run_ok(bin(&home).args(["--output", "json", "supervise", &run_id, "--max-iter", "8"]));

    let manifest: Value =
        serde_json::from_slice(&std::fs::read(rdir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(
        manifest["status"], "pending",
        "a young zero-node run is still materializing; the guard must not fail it"
    );
    let events_v = read_events(&rdir.join("events.jsonl"));
    assert!(
        !events_v
            .iter()
            .any(|v| v["kind"] == "run.status" && v["data"]["status"] == "failed"),
        "no premature no-worker terminalization inside the create window"
    );
}
