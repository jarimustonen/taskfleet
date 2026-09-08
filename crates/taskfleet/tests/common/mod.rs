//! Shared integration-test fixtures.
//!
//! [`TestHome`] is a `TempDir`-backed `TASKFLEET_HOME` that reaps every
//! supervisor process spawned beneath it when it drops, so the test suite
//! never leaks `taskfleet supervise` processes
//! (issue: supervise-test-teardown-leak).
//!
//! `#![allow(dead_code)]` because each integration-test binary compiles this
//! module independently and uses only the subset it needs.
#![allow(dead_code)]

use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// Native-spawn fake executables for integration tests. Unlike the removed
/// `TASKFLEET_CREATE_SH` seam, these exercise Taskfleet's production materializer:
/// typed git/workmux/tmux argv, generated launcher, PID handshake, and cleanup.
pub struct NativeSpawnTools {
    dir: TempDir,
    repo: TempDir,
    worktrees: Mutex<Vec<PathBuf>>,
}

impl NativeSpawnTools {
    pub fn new() -> Self {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = TempDir::new().expect("native spawn tools tempdir");
        let repo = TempDir::new().expect("native spawn disposable repository");
        let init = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(repo.path())
            .status()
            .expect("start git for disposable repository");
        assert!(
            init.success(),
            "initialize disposable native-spawn repository"
        );
        let commit = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=Taskfleet Test",
                "-c",
                "user.email=taskfleet@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "fixture base",
            ])
            .current_dir(repo.path())
            .status()
            .expect("start git commit for disposable repository");
        assert!(commit.success(), "commit disposable native-spawn base");
        let write = |name: &str, body: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        write(
            "git",
            r#"#!/bin/sh
if [ "$1" = "-C" ]; then shift 2; fi
case "$1" in
  check-ref-format) exit 0 ;;
  show-ref) exit 1 ;;
  rev-parse) echo 0123456789012345678901234567890123456789; exit 0 ;;
  reflog) exit 1 ;;
  worktree)
    if [ "$2" = remove ]; then for last do :; done; /bin/rm -rf "$last"; fi
    exit 0 ;;
  branch) exit 0 ;;
esac
exit 1
"#,
        );
        write(
            "tmux",
            r#"#!/bin/sh
# This is a complete private fake server: its socket and inventory live only
# under NativeSpawnTools' cryptographically unique TempDir.
if [ "$1" = "-S" ]; then shift 2; fi
case "$1" in
  new-session|has-session) : > "$NATIVE_TEST_TMUX_STATE"; exit 0 ;;
  rename-window) exit 97 ;; # workmux, not Taskfleet, owns display naming
  kill-window|kill-session) /bin/rm -f "$NATIVE_TEST_TMUX_STATE"; exit 0 ;;
  display-message)
    case " $* " in
      *" -t "*) printf '%s\t%s\t@77\t%s\n' "$NATIVE_TEST_TMUX_SOCKET" "${NATIVE_TEST_SESSION:-headless}" "$(/bin/cat "$NATIVE_TEST_WINDOW_NAME_STATE")" ;;
      *) printf '%s\n' "${NATIVE_TEST_SESSION:-fixture}" ;;
    esac
    exit 0 ;;
  list-windows) [ -f "$NATIVE_TEST_TMUX_STATE" ] && printf '@77\n'; exit 0 ;;
  pipe-pane) exit 0 ;;
  capture-pane)
    printf 'fixture final pane\n'
    exit 0 ;;
esac
exit 1
"#,
        );
        write(
            "workmux",
            r#"#!/bin/sh
case "$1" in
  add)
    /bin/pwd -P > "$NATIVE_TEST_WORKMUX_CWD"
    printf '%s' "$NATIVE_TEST_WORKMUX_CONFIGURED_WINDOW_NAME" > "$NATIVE_TEST_WINDOW_NAME_STATE"
    shift; branch=$1; shift; agent=; prompt=
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -a) agent=$2; shift 2 ;;
        -P) prompt=$2; shift 2 ;;
        *) shift ;;
      esac
    done
    /bin/mkdir -p "$NATIVE_TEST_WORKTREE"
    text=$(/bin/cat "$prompt")
    TMUX_PANE=%77 "$agent" -- "$text" </dev/null >"$NATIVE_TEST_AGENT_STDOUT" 2>"$NATIVE_TEST_AGENT_STDERR" &
    pid=$!
    echo "$pid" > "$NATIVE_TEST_AGENT_PID"
    /bin/ps -o lstart= -p "$pid" > "$NATIVE_TEST_AGENT_START"
    exit 0 ;;
  path) printf '%s\n' "$NATIVE_TEST_WORKTREE"; exit 0 ;;
  remove)
    if [ -f "$NATIVE_TEST_AGENT_PID" ]; then kill "$(/bin/cat "$NATIVE_TEST_AGENT_PID")" 2>/dev/null || true; fi
    /bin/rm -rf "$NATIVE_TEST_WORKTREE"
    exit 0 ;;
esac
exit 1
"#,
        );
        Self {
            dir,
            repo,
            worktrees: Mutex::new(Vec::new()),
        }
    }

    /// Return an unambiguously fixture-owned worktree path. Cleanup is allowed
    /// to force-remove only paths constructed through this method.
    pub fn worktree(&self, name: &str) -> PathBuf {
        assert!(
            !name.is_empty() && !name.contains('/') && name != "." && name != "..",
            "native fixture worktree name must be one safe path component"
        );
        self.dir.path().join("worktrees").join(name)
    }

    pub fn repo_path(&self) -> &Path {
        self.repo.path()
    }

    pub fn workmux_cwd_path(&self) -> PathBuf {
        self.dir.path().join("workmux.cwd")
    }

    pub fn configure(&self, command: &mut std::process::Command, worktree: &Path, session: &str) {
        let owned_root = self.dir.path().join("worktrees");
        assert!(
            worktree.starts_with(&owned_root),
            "native fixture refuses non-owned worktree path {} (root {})",
            worktree.display(),
            owned_root.display()
        );
        self.worktrees.lock().unwrap().push(worktree.to_path_buf());
        // Every production-path test that did not deliberately select another
        // disposable cwd runs from this fixture's real throwaway repository.
        // The explicit dependency fakes cannot touch the developer's repository
        // or default tmux server.
        if command.get_current_dir().is_none() {
            command.current_dir(self.repo.path());
        }
        command
            // Materialized tests must never inherit placement from the developer
            // shell or CI runner. Each create call must declare its isolated
            // `--headless` / `--tmux-session` placement through the public CLI.
            // Stripping TMUX here makes an omitted flag fail deterministically.
            .env_remove("TMUX")
            .env("GIT_BIN", self.dir.path().join("git"))
            .env("TMUX_BIN", self.dir.path().join("tmux"))
            .env("WORKMUX_BIN", self.dir.path().join("workmux"))
            .env("NATIVE_TEST_WORKTREE", worktree)
            .env("NATIVE_TEST_SESSION", session)
            .env("NATIVE_TEST_AGENT_PID", self.dir.path().join("agent.pid"))
            .env(
                "NATIVE_TEST_AGENT_START",
                self.dir.path().join("agent.start"),
            )
            .env("NATIVE_TEST_TMUX_SOCKET", self.dir.path().join("tmux.sock"))
            .env("NATIVE_TEST_WORKMUX_CWD", self.workmux_cwd_path())
            // The fake workmux resolves this configured name and publishes it
            // through the fake tmux server. It is intentionally not a real
            // project's configured prefix.
            .env(
                "NATIVE_TEST_WORKMUX_CONFIGURED_WINDOW_NAME",
                "🧪 wm-owned-window",
            )
            .env(
                "NATIVE_TEST_WINDOW_NAME_STATE",
                self.dir.path().join("window-name.state"),
            )
            .env("NATIVE_TEST_TMUX_STATE", self.dir.path().join("tmux.state"))
            .env(
                "NATIVE_TEST_AGENT_STDOUT",
                self.dir.path().join("agent.stdout"),
            )
            .env(
                "NATIVE_TEST_AGENT_STDERR",
                self.dir.path().join("agent.stderr"),
            );
    }

    pub fn agent_stderr(&self) -> std::path::PathBuf {
        self.dir.path().join("agent.stderr")
    }
}

impl Drop for NativeSpawnTools {
    fn drop(&mut self) {
        // Assertion panics and failed/timeout paths still arrive here. Only an
        // identity-matching process recorded inside our private root may be
        // signalled; a recycled PID is left alone.
        let pid_path = self.dir.path().join("agent.pid");
        let start_path = self.dir.path().join("agent.start");
        if let (Some(pid), Ok(expected)) = (
            read_first_token_pid(&pid_path),
            std::fs::read_to_string(&start_path),
        ) {
            let actual = std::process::Command::new("/bin/ps")
                .args(["-o", "lstart=", "-p", &pid.to_string()])
                .output()
                .ok()
                .map(|out| String::from_utf8_lossy(&out.stdout).into_owned());
            if actual.as_deref() == Some(expected.as_str()) {
                unsafe { libc::kill(pid, libc::SIGTERM) };
            }
        }

        // These paths came from this fixture, not production state. Record
        // diagnostics before the TempDirs force-remove any failed-test residue.
        for path in self.worktrees.get_mut().unwrap().iter() {
            if path.exists() {
                eprintln!(
                    "native-spawn fixture cleanup: removing owned worktree {}",
                    path.display()
                );
                let _ = std::fs::remove_dir_all(path);
            }
        }
        let _ = std::fs::remove_file(self.dir.path().join("tmux.state"));
    }
}

/// Grace period between the polite SIGTERM and the SIGKILL escalation for a
/// supervisor that does not exit promptly.
const REAP_GRACE: Duration = Duration::from_secs(2);

/// A `TempDir` used as `TASKFLEET_HOME` that reaps the supervisor
/// processes spawned beneath it on drop.
///
/// `run create` (and `run reattach`) spawn a top-level supervisor that
/// double-forks and `setsid`s into its own session, reparenting to init — it
/// is therefore *not* a child of the test process (so `waitpid` cannot reap
/// it) and lives outside the test's own process group (so a harness-wide
/// `killpg` can never reach it). The authoritative handle is the PID each
/// supervisor writes into `<run-dir>/supervisor.pid`; on drop we scan every
/// run dir under the home and reap each still-live supervisor.
///
/// Derefs to the inner [`TempDir`] so existing helpers that take `&TempDir`
/// (and `home.path()`) keep working unchanged. The reap happens in
/// [`Drop::drop`] *before* the inner `TempDir` field is dropped, so the run
/// dirs — and their pid files — still exist when we read them.
pub struct TestHome {
    dir: TempDir,
}

impl TestHome {
    /// Create a fresh temp home. Panics on failure (a test cannot proceed
    /// without an isolated home), mirroring the previous
    /// `TempDir::new().unwrap()` call sites.
    pub fn new() -> Self {
        Self {
            dir: TempDir::new().expect("create temp TASKFLEET_HOME"),
        }
    }
}

impl Default for TestHome {
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for TestHome {
    type Target = TempDir;
    fn deref(&self) -> &TempDir {
        &self.dir
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        reap_supervisors_under(self.dir.path());
        // `self.dir` (the TempDir) drops *after* this body, removing the tree.
    }
}

/// SIGTERM, then — after [`REAP_GRACE`] — SIGKILL every live supervisor whose
/// pid file lives under `<home>/runs/*/supervisor.pid`.
pub fn reap_supervisors_under(home: &Path) {
    // Only signal pids that are genuinely *our* detached supervisor processes
    // (command line names `taskfleet supervise`). A `supervisor.pid` file
    // can hold an unrelated pid — e.g. `run_error_envelopes` parks the test's
    // own pid to exercise the "supervisor already running" refusal — and a pid
    // can be recycled to a stranger after the supervisor exits; signalling
    // either would be a serious bug (we would SIGTERM the test runner itself).
    let pids: Vec<libc::pid_t> = scan_supervisor_pids(home)
        .into_iter()
        .filter(|&p| is_supervisor_process(p))
        .collect();
    if pids.is_empty() {
        return;
    }
    let our_pgid = unsafe { libc::getpgrp() };
    // Phase 1 — polite signal. A detached supervisor lives in its own session
    // (pgid != ours), so signal the whole process group and take down anything
    // it forked (child supervisors, create.sh helpers). A supervisor that is
    // *not* detached still shares our group, so signal only its pid — never
    // the group, or we would SIGTERM the test runner itself.
    for &pid in &pids {
        if process_gone(pid) {
            continue;
        }
        signal_target(pid, our_pgid, libc::SIGTERM);
    }
    // Phase 2 — escalate to SIGKILL on any that ignored SIGTERM. Identity-safe:
    // the moment `kill(pid, 0)` reports the pid gone or recycled to another
    // owner we stop, so we never SIGKILL a stranger.
    let deadline = Instant::now() + REAP_GRACE;
    for &pid in &pids {
        while !process_gone(pid) {
            if Instant::now() >= deadline {
                signal_target(pid, our_pgid, libc::SIGKILL);
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Signal `pid`'s detached process group when it has one distinct from ours,
/// otherwise just the pid. `getpgid` failing (e.g. the pid just exited) falls
/// back to a per-pid signal.
fn signal_target(pid: libc::pid_t, our_pgid: libc::pid_t, sig: libc::c_int) {
    let group = unsafe { libc::getpgid(pid) };
    if group > 1 && group != our_pgid {
        // Detached session created by the supervisor's `setsid`: the group
        // holds only its own lineage, so a group-wide signal is safe.
        unsafe { libc::kill(-group, sig) };
    } else {
        unsafe { libc::kill(pid, sig) };
    }
}

/// True once `pid` no longer names a process we may signal: `kill(pid, 0)`
/// fails with `ESRCH` (gone) or `EPERM` (recycled to another owner). Either
/// way our supervisor is gone and we must not escalate a signal to whatever
/// now holds the pid.
fn process_gone(pid: libc::pid_t) -> bool {
    unsafe { libc::kill(pid, 0) != 0 }
}

/// True iff `pid` names a live `taskfleet supervise <run-id>` process.
/// Matched via `ps`
/// (portable across macOS and Linux), so a parked test pid or recycled unrelated
/// pid is never mistaken for a supervisor. The `" supervise"` argument also
/// distinguishes the `supervise_gates` test binary from the actual subcommand.
fn is_supervisor_process(pid: libc::pid_t) -> bool {
    let Ok(out) = std::process::Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
    else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let command = String::from_utf8_lossy(&out.stdout);
    command.contains("taskfleet supervise")
}

/// Collect the deduplicated, positive supervisor PIDs recorded under
/// `<home>/runs/*/supervisor.pid`.
fn scan_supervisor_pids(home: &Path) -> Vec<libc::pid_t> {
    let mut pids = Vec::new();
    let Ok(entries) = std::fs::read_dir(home.join("runs")) else {
        return pids;
    };
    for entry in entries.flatten() {
        let pid_file = entry.path().join("supervisor.pid");
        if let Some(pid) = read_first_token_pid(&pid_file) {
            if pid > 0 && !pids.contains(&pid) {
                pids.push(pid);
            }
        }
    }
    pids
}

/// Read the first whitespace-delimited token of a pid file as a `pid_t`. The
/// file holds `"<pid> <start_time>"` (or a legacy bare `"<pid>"`).
fn read_first_token_pid(path: &Path) -> Option<libc::pid_t> {
    let s = std::fs::read_to_string(path).ok()?;
    s.split_whitespace().next()?.parse::<libc::pid_t>().ok()
}
