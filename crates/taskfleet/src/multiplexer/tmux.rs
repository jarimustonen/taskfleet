//! Typed tmux operations for the supervisor's teardown + window-lookup paths.
//!
//! ## Provenance
//!
//! Adapted from workmux (raine/workmux), `src/multiplexer/tmux.rs`
//! (`TmuxBackend`), MIT-licensed, at upstream commit
//! `6dd33d4d7e8929a4ee8e812fa16c6acceb8082e3`. Only the kill / lookup / create
//! surface is copied; the split-pane / handshake / status-bar machinery and the
//! non-tmux backends are omitted. The copy is fork-and-owned — we do not track
//! upstream drift — and adapted for the supervisor: every operation takes an
//! optional server socket (`-S <socket>`, threaded before the subcommand so a
//! headless run's non-default server is targeted), the kill operations are
//! best-effort/lenient (a vanished window/session is a clean no-op, never an
//! error), and window lookup matches the pane cwd *exactly* to preserve the
//! supervisor's cross-session-kill safety invariant (root CLAUDE.md §5).
//!
//! ## License & drift
//!
//! The upstream MIT license (Copyright (c) 2025 workmux contributors) permits
//! this vendoring; its copyright notice is preserved above. **Drift policy:
//! fork-and-own** — we do not track upstream fixes. The vendored surface is a
//! narrow, stable slice (tmux kill/lookup/create), so divergence is low-risk;
//! if a tmux bug is found upstream, port the fix by hand and bump the provenance
//! commit above. The kitty/wezterm/zellij backends and the full `Multiplexer`
//! trait are intentionally not vendored.

use std::process::{Command, Stdio};

use tracing::{info, warn};

/// The tmux binary name, honoring the `TMUX_BIN` override (tests, non-default
/// installs). The supervisor's watchdog resolves its probe binary the same way.
pub fn tmux_bin() -> String {
    std::env::var("TMUX_BIN").unwrap_or_else(|_| "tmux".to_string())
}

/// Scope for a `list-windows` query: one session (`-t <session>`) or every
/// window on the server (`-a`).
pub enum WindowScope<'a> {
    /// `list-windows -t <session>` — a single session's windows.
    Session(&'a str),
    /// `list-windows -a` — every window on the server.
    All,
}

/// Parameters for a detached (`-d`, "headless") `new-session`.
pub struct NewSession<'a> {
    /// Session name (`-s`).
    pub session: &'a str,
    /// Start directory for the initial pane (`-c`).
    pub cwd: &'a str,
    /// Optional name for the initial window (`-n`). When set, automatic-rename
    /// is disabled so the name sticks (matches upstream `create_session`).
    pub window_name: Option<&'a str>,
    /// Optional server socket (`-S`) to create the session on.
    pub socket: Option<&'a str>,
}

/// Failure from a non-lenient tmux operation ([`Tmux::new_session`]).
#[derive(Debug, thiserror::Error)]
pub enum TmuxError {
    /// The tmux binary could not be spawned at all.
    #[error("tmux {op} failed to spawn: {source}")]
    Spawn {
        op: &'static str,
        #[source]
        source: std::io::Error,
    },
    /// tmux ran but exited non-zero.
    #[error("tmux {op} exited {code:?}: {stderr}")]
    NonZero {
        op: &'static str,
        code: Option<i32>,
        stderr: String,
    },
    /// tmux exited zero but printed no value where one was required (e.g. an
    /// empty pane id from `new-session`).
    #[error("tmux {op} produced no output")]
    EmptyOutput { op: &'static str },
}

/// Typed tmux backend. Construct with [`Tmux::new`] (honors `TMUX_BIN`) or
/// [`Tmux::with_bin`] to pin a specific binary (the supervisor threads its
/// already-resolved binary name through so tests can inject a fake tmux).
pub struct Tmux {
    bin: String,
    #[cfg(test)]
    script: Option<String>,
}

impl Default for Tmux {
    fn default() -> Self {
        Self::new()
    }
}

impl Tmux {
    /// A backend using the `TMUX_BIN`-resolved binary.
    pub fn new() -> Self {
        Self::with_bin(tmux_bin())
    }

    /// A backend pinned to `bin` (the supervisor's already-resolved binary; also
    /// the seam tests inject a fake tmux through).
    pub fn with_bin(bin: impl Into<String>) -> Self {
        Self {
            bin: bin.into(),
            #[cfg(test)]
            script: None,
        }
    }

    /// Run a test script as data through a stable interpreter instead of execing
    /// the freshly written file. Parallel libtest subprocesses can inherit an
    /// open writer for the script and make direct exec fail with ETXTBSY.
    #[cfg(test)]
    pub(crate) fn with_script(script: impl Into<String>) -> Self {
        Self {
            bin: "/bin/sh".into(),
            script: Some(script.into()),
        }
    }

    /// A `Command` for `self.bin`, with the optional server socket threaded in
    /// *before* the subcommand (`tmux -S <socket> <verb> …`), matching how tmux
    /// parses server options.
    fn base(&self, socket: Option<&str>) -> Command {
        let mut cmd = Command::new(&self.bin);
        #[cfg(test)]
        if let Some(script) = &self.script {
            cmd.arg(script);
        }
        if let Some(s) = socket {
            cmd.args(["-S", s]);
        }
        cmd
    }

    /// Issue `tmux [-S <socket>] kill-window -t <target>` leniently; returns
    /// `true` when tmux reported success (window found and killed). A non-zero
    /// exit (typically "can't find window") returns `false` so the caller can
    /// fall back to a path-based lookup rather than leak the window.
    pub fn kill_window(&self, socket: Option<&str>, target: &str) -> bool {
        let mut cmd = self.base(socket);
        cmd.args(["kill-window", "-t", target]);
        run_lenient(cmd, &format!("tmux kill-window -t {target}"))
    }

    /// Issue `tmux [-S <socket>] kill-session -t <session>` leniently; returns
    /// `true` when tmux reported success. A non-zero exit (the session vanished
    /// in a race) returns `false` so no audit event is recorded for a no-op.
    pub fn kill_session(&self, socket: Option<&str>, session: &str) -> bool {
        let mut cmd = self.base(socket);
        cmd.args(["kill-session", "-t", session]);
        run_lenient(cmd, &format!("tmux kill-session -t {session}"))
    }

    /// Find the `window_id` of a window whose active pane's cwd is **exactly**
    /// `worktree_path`, scoped to a single session when possible.
    ///
    /// This is the rename-proof handle the supervisor's orphan-recovery path
    /// keys off — a manually-resolved rebase mutates the branch/window name but
    /// not the pane's cwd. Two safety constraints (root CLAUDE.md §5, issue
    /// `find-window-by-path-cross-session-kill`):
    ///
    /// 1. **Session-scoped.** When `session` is `Some`, query `list-windows -t
    ///    <session>`; otherwise fall back to `-a`. Without this scope, an
    ///    unrelated pane in a different session that happened to cd into the
    ///    worktree would match and get killed.
    /// 2. **Exact-match cwd.** Match only `path == worktree_path`, never a
    ///    sub-path — a sibling pane one level deeper (`worktree/src/foo`) would
    ///    otherwise match and die.
    ///
    /// `None` if tmux is unavailable, the server errors, or no pane matches.
    pub fn find_window_by_path(
        &self,
        socket: Option<&str>,
        session: Option<&str>,
        worktree_path: &str,
    ) -> Option<String> {
        let scope = match session {
            Some(name) => WindowScope::Session(name),
            None => WindowScope::All,
        };
        let out = self.list_windows_raw(socket, scope, "#{window_id}\t#{pane_current_path}")?;
        out.lines().find_map(|line| {
            let (wid, path) = line.split_once('\t')?;
            if path.trim_end() != worktree_path {
                return None;
            }
            let wid = wid.trim();
            (!wid.is_empty()).then(|| wid.to_string())
        })
    }

    /// List a session's windows as `(any_attached, window_names)` via a single
    /// `tmux list-windows -t <session> -F '#{session_attached}\t#{window_name}'`.
    /// `None` when the session is gone (non-zero exit) or tmux could not run, so
    /// the caller treats an already-torn-down session as a clean no-op.
    /// `any_attached` is true when ANY line reports a non-zero
    /// `#{session_attached}` (a human is in the session).
    pub fn list_session_windows(
        &self,
        socket: Option<&str>,
        session: &str,
    ) -> Option<(bool, Vec<String>)> {
        let out = self.list_windows_raw(
            socket,
            WindowScope::Session(session),
            "#{session_attached}\t#{window_name}",
        )?;
        let mut attached = false;
        let mut names = Vec::new();
        for line in out.lines() {
            let Some((att, name)) = line.split_once('\t') else {
                continue;
            };
            if att.trim() != "0" {
                attached = true;
            }
            let name = name.trim_end();
            if !name.is_empty() {
                names.push(name.to_string());
            }
        }
        Some((attached, names))
    }

    /// Run one `tmux [-S <socket>] list-windows <scope> -F <format>` and return
    /// its raw stdout. `None` on a non-zero exit (session/server gone) or a
    /// spawn failure (tmux unavailable) — both are "no answer", which every
    /// lookup caller treats as an already-torn-down no-op. stderr is discarded;
    /// a probe failure is expected during teardown races and never logged.
    fn list_windows_raw(
        &self,
        socket: Option<&str>,
        scope: WindowScope<'_>,
        format: &str,
    ) -> Option<String> {
        let mut cmd = self.base(socket);
        match scope {
            WindowScope::Session(name) => cmd.args(["list-windows", "-t", name]),
            WindowScope::All => cmd.args(["list-windows", "-a"]),
        };
        cmd.args(["-F", format]);
        cmd.stderr(Stdio::null());
        let out = cmd.output().ok()?;
        if !out.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Create a detached ("headless") session and return the pane id of its
    /// initial window.
    ///
    /// `tmux [-S <socket>] new-session -d -s <session> -c <cwd> [-n <window>]
    /// -P -F '#{pane_id}'`. When a window name is given, automatic-rename is
    /// disabled so the name sticks (matches upstream `create_session`).
    ///
    /// Retained as the typed session primitive used by supervisor-side code and
    /// tests. Native run creation owns its stricter create-or-confirm sequence in
    /// `run::spawn`, alongside the workmux transaction rollback.
    #[allow(dead_code)]
    pub fn new_session(&self, params: &NewSession<'_>) -> Result<String, TmuxError> {
        let mut cmd = self.base(params.socket);
        cmd.args(["new-session", "-d", "-s", params.session, "-c", params.cwd]);
        if let Some(name) = params.window_name {
            cmd.args(["-n", name]);
        }
        cmd.args(["-P", "-F", "#{pane_id}"]);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let out = cmd.output().map_err(|source| TmuxError::Spawn {
            op: "new-session",
            source,
        })?;
        if !out.status.success() {
            return Err(TmuxError::NonZero {
                op: "new-session",
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        let pane_id = String::from_utf8_lossy(&out.stdout).trim().to_string();
        // A zero-exit `new-session` that printed no pane id would leave every
        // downstream target (`-t ""`) ambiguous, so treat empty output as a
        // protocol failure rather than returning `Ok("")`.
        if pane_id.is_empty() {
            return Err(TmuxError::EmptyOutput { op: "new-session" });
        }

        // Keep a named initial window from being auto-renamed by shell activity.
        // Best-effort and silent (matches upstream): this runs on the spawn path,
        // not teardown, so it must not emit the cleanup-flavored audit line, and a
        // failure only means the name may drift — not a spawn failure.
        if params.window_name.is_some() {
            let _ = self
                .base(params.socket)
                .args([
                    "set-window-option",
                    "-w",
                    "-t",
                    &pane_id,
                    "automatic-rename",
                    "off",
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }

        Ok(pane_id)
    }
}

/// Run a best-effort tmux command, logging its outcome to both `tracing` and
/// stderr (captured to `supervisor.stderr.log`) so the teardown is auditable.
/// Returns `true` only on success; a non-zero exit or spawn error is logged at
/// `warn`, swallowed, and reported as `false` so a caller can fall back rather
/// than leak. Mirrors the git-side lenient runner in `supervise::cleanup`;
/// tmux ops live here so the multiplexer module owns its own execution.
fn run_lenient(mut cmd: Command, label: &str) -> bool {
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    match cmd.output() {
        Ok(out) if out.status.success() => {
            info!(target: "taskfleet::supervise", step = label, "cleanup step ok");
            eprintln!("supervisor cleanup: {label}: ok");
            true
        }
        Ok(out) => {
            let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
            warn!(
                target: "taskfleet::supervise",
                step = label,
                code = out.status.code(),
                detail = %detail,
                "cleanup step non-zero (treated as already-done/refused; continuing)"
            );
            eprintln!("supervisor cleanup: {label}: non-zero exit (continuing): {detail}");
            false
        }
        Err(e) => {
            warn!(
                target: "taskfleet::supervise",
                step = label,
                error = %e,
                "cleanup step could not spawn (continuing)"
            );
            eprintln!("supervisor cleanup: {label}: spawn failed (continuing): {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Write a fake `tmux` script that records its argv to `<dir>/tmux.log` and
    /// runs `body` (a shell snippet with `$@`/`$*` available).
    fn fake_tmux(dir: &std::path::Path, body: &str) -> Tmux {
        let path = dir.join("tmux");
        let log = dir.join("tmux.log");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(
            format!(
                "#!/bin/sh\necho \"$@\" >> {log}\n{body}\n",
                log = log.display(),
            )
            .as_bytes(),
        )
        .unwrap();
        file.sync_all().unwrap();
        drop(file);
        Tmux::with_script(path.to_string_lossy())
    }

    fn log_of(dir: &std::path::Path) -> String {
        std::fs::read_to_string(dir.join("tmux.log")).unwrap_or_default()
    }

    #[test]
    fn kill_window_threads_socket_before_verb() {
        let tmp = tempfile::tempdir().unwrap();
        let tmux = fake_tmux(tmp.path(), "");
        assert!(tmux.kill_window(Some("/tmp/sock-7"), "@42"));
        let log = log_of(tmp.path());
        assert!(
            log.contains("-S /tmp/sock-7 kill-window -t @42"),
            "socket must precede the verb: {log:?}"
        );
    }

    #[test]
    fn kill_window_reports_false_on_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let tmux = fake_tmux(tmp.path(), "exit 1");
        assert!(!tmux.kill_window(None, "@99"));
    }

    #[test]
    fn kill_session_no_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let tmux = fake_tmux(tmp.path(), "");
        assert!(tmux.kill_session(None, "headless"));
        assert!(log_of(tmp.path()).contains("kill-session -t headless"));
    }

    #[test]
    fn find_window_by_path_requires_exact_cwd_match() {
        let tmp = tempfile::tempdir().unwrap();
        // A sibling pane one level deeper must NOT match; only the exact cwd.
        let tmux = fake_tmux(
            tmp.path(),
            r#"case "$*" in *list-windows*) printf '@7\t/wt/foo/src\n@9\t/wt/foo\n';; esac"#,
        );
        assert_eq!(
            tmux.find_window_by_path(None, Some("headless"), "/wt/foo"),
            Some("@9".to_string())
        );
        assert!(log_of(tmp.path()).contains("list-windows -t headless"));
    }

    #[test]
    fn find_window_by_path_scopes_to_all_when_no_session() {
        let tmp = tempfile::tempdir().unwrap();
        let tmux = fake_tmux(
            tmp.path(),
            r#"case "$*" in *list-windows*) printf '@9\t/wt/foo\n';; esac"#,
        );
        assert_eq!(
            tmux.find_window_by_path(None, None, "/wt/foo"),
            Some("@9".to_string())
        );
        assert!(log_of(tmp.path()).contains("list-windows -a"));
    }

    #[test]
    fn find_window_by_path_none_on_nonzero() {
        let tmp = tempfile::tempdir().unwrap();
        let tmux = fake_tmux(tmp.path(), "exit 1");
        assert_eq!(tmux.find_window_by_path(None, Some("s"), "/wt/foo"), None);
    }

    #[test]
    fn list_session_windows_parses_attached_and_names() {
        let tmp = tempfile::tempdir().unwrap();
        let tmux = fake_tmux(
            tmp.path(),
            r#"case "$*" in *list-windows*) printf '0\tzsh\n1\t🎬 wt/x\n';; esac"#,
        );
        let (attached, names) = tmux.list_session_windows(None, "headless").unwrap();
        assert!(
            attached,
            "a non-zero session_attached means a human is in it"
        );
        assert_eq!(names, vec!["zsh".to_string(), "🎬 wt/x".to_string()]);
    }

    #[test]
    fn list_session_windows_none_when_session_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let tmux = fake_tmux(tmp.path(), "exit 1");
        assert!(tmux.list_session_windows(None, "gone").is_none());
    }

    #[test]
    fn new_session_headless_returns_pane_id_and_disables_rename() {
        let tmp = tempfile::tempdir().unwrap();
        // Echo a pane id for the new-session query; record everything.
        let tmux = fake_tmux(
            tmp.path(),
            r#"case "$*" in *new-session*) echo '%3';; esac"#,
        );
        let pane = tmux
            .new_session(&NewSession {
                session: "headless",
                cwd: "/tmp",
                window_name: Some("wt/x"),
                socket: Some("/tmp/sock"),
            })
            .unwrap();
        assert_eq!(pane, "%3");
        let log = log_of(tmp.path());
        assert!(
            log.contains(
                "-S /tmp/sock new-session -d -s headless -c /tmp -n wt/x -P -F #{pane_id}"
            ),
            "new-session argv: {log:?}"
        );
        assert!(
            log.contains("set-window-option -w -t %3 automatic-rename off"),
            "named window must disable automatic-rename: {log:?}"
        );
    }

    #[test]
    fn new_session_surfaces_nonzero() {
        let tmp = tempfile::tempdir().unwrap();
        let tmux = fake_tmux(
            tmp.path(),
            r#"case "$*" in *new-session*) echo boom >&2; exit 1;; esac"#,
        );
        let err = tmux
            .new_session(&NewSession {
                session: "headless",
                cwd: "/tmp",
                window_name: None,
                socket: None,
            })
            .unwrap_err();
        match err {
            TmuxError::NonZero { stderr, .. } => assert_eq!(stderr, "boom"),
            other => panic!("expected NonZero, got {other:?}"),
        }
    }

    #[test]
    fn new_session_without_window_name_skips_rename_off() {
        let tmp = tempfile::tempdir().unwrap();
        let tmux = fake_tmux(
            tmp.path(),
            r#"case "$*" in *new-session*) echo '%1';; esac"#,
        );
        let pane = tmux
            .new_session(&NewSession {
                session: "headless",
                cwd: "/tmp",
                window_name: None,
                socket: None,
            })
            .unwrap();
        assert_eq!(pane, "%1");
        assert!(
            !log_of(tmp.path()).contains("set-window-option"),
            "no window name means no automatic-rename toggle"
        );
    }

    #[test]
    fn new_session_empty_pane_id_is_a_protocol_error() {
        let tmp = tempfile::tempdir().unwrap();
        // Zero exit, no stdout — must not return Ok("").
        let tmux = fake_tmux(tmp.path(), "");
        let err = tmux
            .new_session(&NewSession {
                session: "headless",
                cwd: "/tmp",
                window_name: Some("wt/x"),
                socket: None,
            })
            .unwrap_err();
        assert!(matches!(err, TmuxError::EmptyOutput { .. }), "{err:?}");
        assert!(
            !log_of(tmp.path()).contains("set-window-option"),
            "must not target an empty pane id"
        );
    }
}
