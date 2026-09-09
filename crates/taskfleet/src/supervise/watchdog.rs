//! Agent liveness watchdog (design.md §7.5).
//!
//! Dual polling: PID liveness via `kill(pid, 0)` + start-time identity
//! defense (so a recycled PID is not mistaken for the original agent) +
//! tmux window presence via `tmux list-windows`.
//!
//! Start-time is queried via the cross-platform `sysinfo` crate. The
//! crate exposes `Process::start_time()` as a Unix timestamp on every
//! supported platform; this avoids the macOS/Linux `sysctl` /
//! `/proc/<pid>/stat` split that the design originally specified. If
//! `sysinfo` ever drops `start_time` we'll switch to direct `libc`
//! probes; tracked in `validation.md`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::process::Command;
use std::time::Duration;

use sysinfo::{Pid, System};
use taskfleet_core::schema::TmuxIdentity;

use crate::supervise::pid_file;

/// Liveness verdict for an agent process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Alive,
    /// PID has exited (or never existed).
    Dead,
    /// `kill(pid, 0)` succeeded but `start_time` disagrees with the
    /// captured value — PID has been recycled by an unrelated process.
    Recycled,
    /// PID looks alive but the tmux window is gone (e.g. user
    /// `tmux kill-window`). Half-state; design.md §7.5 commits to
    /// treating this as terminal after a short retry.
    TmuxGone,
}

impl Liveness {
    pub fn reason(self) -> &'static str {
        match self {
            Liveness::Alive => "alive",
            Liveness::Dead => "agent-died",
            Liveness::Recycled => "agent-pid-recycled",
            Liveness::TmuxGone => "agent-tmux-window-gone",
        }
    }
}

/// Read the process `start_time` (Unix seconds) for `pid`. Returns `None`
/// if the process does not exist or the platform sysinfo backend
/// declines to populate the value.
pub fn pid_start_time(pid: u32) -> Option<u64> {
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[Pid::from_u32(pid)]),
        true,
        sysinfo::ProcessRefreshKind::new(),
    );
    sys.process(Pid::from_u32(pid))
        .map(sysinfo::Process::start_time)
}

/// Outcome of a tmux window probe.
///
/// The distinction between [`Absent`](TmuxProbe::Absent) and
/// [`Unknown`](TmuxProbe::Unknown) is load-bearing for liveness: only `Absent`
/// (the server answered and the window is genuinely not there) may flip a node
/// to `TmuxGone`. `Unknown` (tmux binary missing, server down on the recorded
/// socket, spawn error — we could not get a definitive answer) must NOT, or a
/// transient/operational hiccup would falsely reap a live agent. Process
/// liveness still governs in the `Unknown` case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmuxProbe {
    /// The server answered and the target window is present.
    Present,
    /// The server answered and the target window is definitively gone.
    Absent,
    /// Could not get a definitive answer (tmux missing, server down, error).
    Unknown,
}

/// Default bound on a single `tmux list-windows` probe. A wedged tmux server
/// (hung socket, unresponsive server) must not stall the whole watchdog tick,
/// so the probe is killed after this and its socket is treated as
/// [`SocketWindows::Unreachable`] — PID liveness still governs, so a timeout
/// never reaps a live agent (`watchdog-tmux-probe-timeout`).
#[cfg(not(test))]
const TMUX_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
// A saturated parallel unit-test process can leave an otherwise instant fake
// subprocess unscheduled for several seconds. Keep the shipped bound exact,
// but do not turn scheduler starvation into a false `Unknown` test verdict.
#[cfg(test)]
const TMUX_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Emit the "tmux probe timed out" warning at most once per this many timeout
/// events across the whole process. A persistently wedged server then logs
/// ~once per this many ticks instead of flooding every tick.
const TIMEOUT_WARN_EVERY: u64 = 30;

/// Windows observed on one tmux server (socket) in a single tick.
#[derive(Debug, Clone)]
enum SocketWindows {
    /// The server answered (exit 0); these are the live `window_id`s and
    /// `window_name`s. A `window_id`/`window_name` absent here is a
    /// *definitive* absence on this socket.
    Reachable {
        ids: HashSet<String>,
        names: HashSet<String>,
    },
    /// The server could not be reached, errored, or the probe timed out — no
    /// definitive answer for any node on this socket (maps to
    /// [`TmuxProbe::Unknown`]).
    Unreachable,
}

/// One-tick snapshot of tmux window presence, grouped by server socket.
///
/// The watchdog runs ONE `tmux [-S <socket>] list-windows -a` per *distinct
/// socket* per tick (`watchdog-batch-tmux-probe`) — previously one subprocess
/// per tracked node per tick — then evaluates every node by an O(1) set lookup
/// against this snapshot. The common case (all agents on one server) collapses
/// to a single tmux invocation per tick regardless of node count.
///
/// `None` keys the default socket. A single global `tmux list-windows -a` only
/// enumerates the *connected* server, so a node on a custom `-S` socket needs
/// its own query; grouping by socket (rather than one truly-global query)
/// keeps the tri-state honest — a window missing from socket A is never read as
/// an absence for a node on socket B.
#[derive(Debug, Clone, Default)]
pub struct WatchdogTmuxSnapshot {
    sockets: HashMap<Option<String>, SocketWindows>,
}

impl WatchdogTmuxSnapshot {
    /// Collect a fresh snapshot for `sockets` — one timed probe each — using
    /// the `TMUX_BIN` override (mostly for tests) or `tmux`, honoring
    /// [`TMUX_PROBE_TIMEOUT`].
    pub fn collect(sockets: &BTreeSet<Option<String>>) -> Self {
        let bin = std::env::var("TMUX_BIN").unwrap_or_else(|_| "tmux".to_string());
        Self::collect_with(sockets, &bin, TMUX_PROBE_TIMEOUT)
    }

    /// Testable core of [`collect`](Self::collect): explicit binary and timeout.
    fn collect_with(sockets: &BTreeSet<Option<String>>, bin: &str, timeout: Duration) -> Self {
        let mut map = HashMap::with_capacity(sockets.len());
        for socket in sockets {
            map.insert(
                socket.clone(),
                probe_socket(bin, socket.as_deref(), timeout),
            );
        }
        Self { sockets: map }
    }

    /// Verdict for a qualified identity: match its stable `window_id` on its
    /// recorded socket. `session` is deliberately NOT part of the match —
    /// `window_id` is unique within a server, and matching on session would
    /// re-introduce the `rename-session` / window-move fragility the qualified
    /// identity work (P6.1) removed.
    fn lookup_qualified(&self, identity: &TmuxIdentity) -> TmuxProbe {
        match self.sockets.get(&identity.socket) {
            Some(SocketWindows::Reachable { ids, .. }) => {
                if ids.contains(&identity.window_id) {
                    TmuxProbe::Present
                } else {
                    TmuxProbe::Absent
                }
            }
            // `Unreachable` is no verdict; `None` means the socket was not
            // probed this tick (should not happen — the tick gathers every
            // node's socket first). Both defer to PID liveness rather than risk
            // a false absence.
            Some(SocketWindows::Unreachable) | None => TmuxProbe::Unknown,
        }
    }

    /// The tmux window verdict for `probe` — the same qualified-identity /
    /// legacy-bare-name selection [`check_liveness`] uses, exposed so the
    /// interactive liveness policy can consult the window *independently* of the
    /// PID check (which short-circuits on a dead PID before ever probing tmux).
    ///
    /// Returns `None` when there is no tmux check to run at all — either
    /// `skip_tmux_check` is set, or the node carries neither a qualified identity
    /// nor a legacy window name. Unlike [`check_liveness`], this does NOT emit the
    /// once-per-name legacy warning: that warning belongs to the primary liveness
    /// path, and this accessor may be a second consult on the same tick.
    pub fn probe_verdict(&self, probe: &AgentProbe) -> Option<TmuxProbe> {
        if probe.skip_tmux_check {
            return None;
        }
        match probe.tmux_identity.as_ref() {
            Some(identity) => Some(self.lookup_qualified(identity)),
            None => probe
                .tmux_window
                .as_deref()
                .map(|name| self.lookup_by_name(name)),
        }
    }

    /// Legacy bare-name verdict on the default socket — the ambiguous match
    /// kept for nodes registered before native materializer emitted the qualified
    /// identity.
    fn lookup_by_name(&self, window_name: &str) -> TmuxProbe {
        match self.sockets.get(&None) {
            Some(SocketWindows::Reachable { names, .. }) => {
                if names.contains(window_name) {
                    TmuxProbe::Present
                } else {
                    TmuxProbe::Absent
                }
            }
            Some(SocketWindows::Unreachable) | None => TmuxProbe::Unknown,
        }
    }
}

/// Run one timed `tmux [-S socket] list-windows -a -F '#{window_id}|#{window_name}'`
/// and parse it into a [`SocketWindows`]. A single query carries both fields so
/// the qualified (`window_id`) and legacy (`window_name`) paths share one
/// subprocess. `window_id` (`@NNNN`) never contains `|`, so splitting each line
/// on the FIRST `|` cleanly separates the id from a name that may contain `|`.
fn probe_socket(bin: &str, socket: Option<&str>, timeout: Duration) -> SocketWindows {
    let mut cmd = Command::new(bin);
    if let Some(s) = socket {
        cmd.args(["-S", s]);
    }
    cmd.args(["list-windows", "-a", "-F", "#{window_id}|#{window_name}"]);
    match run_timed(cmd, timeout) {
        ProbeOutcome::Ok(stdout) => {
            let mut ids = HashSet::new();
            let mut names = HashSet::new();
            // Trim each line so a stray `\r` (anomalous terminals) does not
            // defeat the exact match — mirrors the prior per-node classifier.
            for line in stdout.split(|b| *b == b'\n') {
                let line = trim_ascii(line);
                if line.is_empty() {
                    continue;
                }
                match line.iter().position(|b| *b == b'|') {
                    Some(i) => {
                        let id = trim_ascii(&line[..i]);
                        let name = trim_ascii(&line[i + 1..]);
                        if !id.is_empty() {
                            ids.insert(String::from_utf8_lossy(id).into_owned());
                        }
                        names.insert(String::from_utf8_lossy(name).into_owned());
                    }
                    // No delimiter (unexpected) — treat the whole line as an id.
                    None => {
                        ids.insert(String::from_utf8_lossy(line).into_owned());
                    }
                }
            }
            SocketWindows::Reachable { ids, names }
        }
        // Non-zero exit (no server on this socket, permission error) and spawn
        // failure (tmux missing) are both "no verdict" — not an absence.
        ProbeOutcome::NonZero | ProbeOutcome::SpawnErr => SocketWindows::Unreachable,
        ProbeOutcome::TimedOut => {
            warn_timeout_rate_limited(socket);
            SocketWindows::Unreachable
        }
    }
}

/// Result of a timed subprocess wait.
enum ProbeOutcome {
    /// Exited 0 — stdout captured.
    Ok(Vec<u8>),
    /// Exited non-zero.
    NonZero,
    /// Could not spawn (or an unexpected wait error).
    SpawnErr,
    /// Exceeded the deadline and was killed.
    TimedOut,
}

/// Generous cap on a single tmux `list-windows` capture. The output is tiny in
/// practice (one line per window); this is a memory backstop, not a real limit.
const TMUX_OUTPUT_CAP: usize = 1 << 20; // 1 MiB

/// Spawn `cmd` and wait at most `timeout`, mapping the shared bounded-subprocess
/// outcome ([`crate::proc::run_with_timeout`]) into a tmux-probe verdict. The
/// timeout + process-group-kill + drained-capture machinery lives in
/// [`crate::proc`] so the watchdog and pane-output capture share one
/// implementation; here we only care about exit success and captured stdout.
fn run_timed(cmd: Command, timeout: Duration) -> ProbeOutcome {
    match crate::proc::run_with_timeout(cmd, timeout, TMUX_OUTPUT_CAP) {
        crate::proc::TimedOutcome::Exited { status, stdout, .. } => {
            if status.success() {
                ProbeOutcome::Ok(stdout.bytes)
            } else {
                ProbeOutcome::NonZero
            }
        }
        crate::proc::TimedOutcome::TimedOut => ProbeOutcome::TimedOut,
        crate::proc::TimedOutcome::SpawnErr(_) => ProbeOutcome::SpawnErr,
    }
}

/// Warn — rate-limited to ~once per [`TIMEOUT_WARN_EVERY`] timeout events
/// process-wide — that a tmux probe timed out and its socket is being treated
/// as unreachable for this tick. The watchdog probes every tick, so an
/// un-throttled `warn!` would flood the log while a server stays wedged.
fn warn_timeout_rate_limited(socket: Option<&str>) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static TIMEOUTS: AtomicU64 = AtomicU64::new(0);
    let n = TIMEOUTS.fetch_add(1, Ordering::Relaxed);
    if n % TIMEOUT_WARN_EVERY == 0 {
        tracing::warn!(
            socket = socket.unwrap_or("<default>"),
            timeout_secs = TMUX_PROBE_TIMEOUT.as_secs(),
            warn_every = TIMEOUT_WARN_EVERY,
            "tmux list-windows probe timed out; treating socket as unreachable \
             and deferring to PID liveness for its nodes this tick (warning \
             rate-limited)"
        );
    }
}

fn trim_ascii(b: &[u8]) -> &[u8] {
    let start = b.iter().position(|c| !c.is_ascii_whitespace());
    let Some(start) = start else { return &[] };
    let end = b.iter().rposition(|c| !c.is_ascii_whitespace()).unwrap();
    &b[start..=end]
}

/// Snapshot of what we know about an agent at spawn time. Compared
/// against live probes by [`check_liveness`].
#[derive(Debug, Clone)]
pub struct AgentProbe {
    pub pid: u32,
    /// Start-time captured at spawn (Unix seconds). `None` means the
    /// supervisor could not read `start_time` on this platform — we then
    /// fall back to PID-only liveness and accept the tiny risk of a
    /// reused PID being mistaken for the agent.
    pub start_time: Option<u64>,
    pub tmux_window: Option<String>,
    /// Fully-qualified tmux identity captured at spawn. When `Some`, the tmux
    /// probe matches the stable `window_id` on the recorded socket (precise);
    /// when `None` (a node registered before native materializer emitted the qualified
    /// fields), it falls back to a bare-name match on [`AgentProbe::tmux_window`].
    pub tmux_identity: Option<TmuxIdentity>,
    /// When `true`, skip the tmux probe. Used when tmux is not the
    /// host (e.g. fake-spawn test fixtures) or tmux unavailability is
    /// not a failure signal.
    pub skip_tmux_check: bool,
}

impl AgentProbe {
    /// The tmux socket this probe will consult this tick (for the watchdog to
    /// union across nodes into the set of sockets to snapshot once per tick), or
    /// `None` when this probe runs no tmux check at all (`skip_tmux_check`).
    ///
    /// A returned socket of `None` means the default server — either a qualified
    /// identity with no recorded socket, or the legacy bare-name fallback (which
    /// always uses the default socket). To keep that distinction in one value
    /// without an `Option<Option<_>>`, the "no probe" case is signalled by the
    /// `bool` in the tuple: `(false, _)` = no probe; `(true, socket)` = probe
    /// `socket`.
    pub fn probe_socket(&self) -> (bool, Option<String>) {
        if self.skip_tmux_check {
            return (false, None);
        }
        match &self.tmux_identity {
            Some(id) => (true, id.socket.clone()),
            None => (self.tmux_window.is_some(), None),
        }
    }
}

/// Cap on distinct window names retained for warn-once dedup. Bounds the
/// `WARNED` set for a long-running supervisor: once the cap is hit we stop
/// tracking new names (and warn once that we're capping), accepting that a few
/// later legacy windows may re-warn rather than letting the set grow unbounded.
const MAX_LEGACY_WARN_KEYS: usize = 1024;

/// Warn — at most once per window name per process — that a node lacks a
/// qualified tmux identity and is using the ambiguous bare-name fallback. The
/// watchdog probes every tick, so an un-deduplicated `warn!` would flood the
/// log for every legacy node; this surfaces the condition once and stays quiet
/// thereafter. New spawns get the loud warning at the spawn boundary instead
/// (`run::spawn::run_create_sh`).
fn warn_legacy_bare_name_once(window_name: &str) {
    use std::sync::{Mutex, OnceLock};
    static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let mutex = WARNED.get_or_init(|| Mutex::new(HashSet::new()));
    // A poisoned mutex must not abort liveness checks — this is only log dedup,
    // so recover the guard and carry on. Decide-then-release: never hold the
    // lock across the `tracing::warn!` emit.
    let mut guard = mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cap_hit = guard.len() >= MAX_LEGACY_WARN_KEYS;
    let is_new = !cap_hit && guard.insert(window_name.to_string());
    drop(guard);
    if is_new {
        tracing::warn!(
            tmux_window = window_name,
            "node has no qualified tmux identity; falling back to bare \
             window-name liveness matching (registered before native materializer \
             emitted the qualified fields) — this is ambiguous across sessions"
        );
    }
}

/// Probe the agent against a per-tick tmux [`WatchdogTmuxSnapshot`] and return
/// a [`Liveness`] verdict. The snapshot is collected once per tick (one tmux
/// invocation per distinct socket); the per-node tmux check here is an O(1) set
/// lookup with no subprocess of its own.
pub fn check_liveness(probe: &AgentProbe, tmux: &WatchdogTmuxSnapshot) -> Liveness {
    if !pid_file::pid_alive(probe.pid) {
        return Liveness::Dead;
    }
    if let Some(expected) = probe.start_time {
        if let Some(actual) = pid_start_time(probe.pid) {
            // Allow a 1-second tolerance: some platforms round
            // start_time to whole seconds while sysinfo's first probe
            // may report fractional ticks differently than the second.
            if expected.abs_diff(actual) > 1 {
                return Liveness::Recycled;
            }
        }
    }
    if !probe.skip_tmux_check {
        let probe_result = match probe.tmux_identity.as_ref() {
            // Precise path: match the stable window_id on the recorded socket.
            Some(identity) => Some(tmux.lookup_qualified(identity)),
            // Legacy path: node registered before native materializer emitted the
            // qualified fields. Fall back to the ambiguous bare-name match.
            None => probe.tmux_window.as_deref().map(|name| {
                warn_legacy_bare_name_once(name);
                tmux.lookup_by_name(name)
            }),
        };
        // Only a *definitive* absence (server answered, window not there) flips
        // the node to TmuxGone. `Unknown` (server unreachable / tmux missing)
        // and "nothing to probe" leave liveness to the PID check above — a
        // transient or operational tmux failure must not reap a live agent.
        match probe_result {
            Some(TmuxProbe::Absent) => return Liveness::TmuxGone,
            Some(TmuxProbe::Unknown) => {
                tracing::debug!(
                    "tmux liveness probe inconclusive (server unreachable or \
                     tmux unavailable); deferring to PID liveness"
                );
            }
            Some(TmuxProbe::Present) | None => {}
        }
    }
    Liveness::Alive
}

/// Liveness for a node, accounting for how its agent process behaves over the
/// run's lifetime.
///
/// AUTONOMOUS runs: the recorded `agent_pid` is a single fire-and-forget process
/// that runs to completion and self-merges; its death is authoritative, so the
/// PID governs and this defers straight to [`check_liveness`] (a lost-but-merged
/// branch is separately rescued by the tick's git-reconcile probe).
///
/// INTERACTIVE runs: a human drives the tmux window and may quit and restart the
/// agent process (claude) — or it re-execs itself on update / context compaction
/// — while the window, and their uncommitted work, live on for hours or days.
/// The recorded pid then goes stale and a bare PID probe fires a false
/// `agent-died` on a still-live session (issue
/// `agent-died-merge-no-teardown-interactive`: a ~1.5-day run whose agent issued
/// `run merge` 18 min AFTER the watchdog declared it dead). So for an interactive
/// node the tmux WINDOW is the authoritative liveness signal, exactly as it
/// already is when the pid happens to be alive: a dead/recycled pid with the
/// window still present (or an inconclusive probe) is NOT terminal — only a
/// definitive window absence is (and that is still streak-gated by the caller,
/// same as any [`Liveness::TmuxGone`]). This holds regardless of how long the
/// run has idled, because the window outlives every agent-process restart.
///
/// The deliberate cost of this bias: a genuinely-dead interactive agent whose
/// WINDOW survives — a pane whose root shell outlives the crashed `claude`
/// child, a `remain-on-exit` zombie pane, or a tmux server that is permanently
/// unreachable while the supervisor keeps ticking — is NOT auto-reaped and its
/// node stays non-terminal until the human finalizes it with `run merge` /
/// `run cancel`. Preserving a human's uncommitted work is worth more than a
/// prompt auto-teardown on a liveness guess; the false-positive this fixes was
/// the destructive direction.
///
/// When there is no window to probe at all (`skip_tmux_check`, or no identity /
/// window name recorded) the interactive path has no better signal, so it falls
/// back to the PID verdict — the degenerate case, since interactive runs always
/// own a window.
pub fn check_liveness_for_lifecycle(
    probe: &AgentProbe,
    tmux: &WatchdogTmuxSnapshot,
    interactive: bool,
) -> Liveness {
    let verdict = check_liveness(probe, tmux);
    if !interactive || probe.skip_tmux_check {
        return verdict;
    }
    match verdict {
        // A stale/dead pid is EXPECTED across an interactive agent restart. Re-base
        // the verdict on the window alone.
        Liveness::Dead | Liveness::Recycled => match tmux.probe_verdict(probe) {
            // Window really gone → the session ended; hand the caller the
            // streak-gated half-state (a transient false-`Absent` must not reap).
            Some(TmuxProbe::Absent) => Liveness::TmuxGone,
            // Window present or the probe was inconclusive (server unreachable /
            // tmux missing) → the human's session lives on despite the stale pid.
            // Inconclusive deliberately errs toward Alive: a transient/operational
            // tmux failure must never reap a live human session (matching the PID
            // path's tri-state). The known cost is that a genuinely-dead
            // interactive agent whose window persists — a surviving pane shell, a
            // `remain-on-exit` zombie pane, or a permanently-unreachable tmux
            // server while the supervisor keeps ticking — is NOT auto-reaped; the
            // human finalizes it with `run merge` / `run cancel`. That is the
            // intended bias for human-owned work (never destroy uncommitted work
            // on a liveness guess).
            Some(TmuxProbe::Present | TmuxProbe::Unknown) => Liveness::Alive,
            // Nothing to probe (no identity AND no window name, yet the caller did
            // not set `skip_tmux_check`): there is no window signal to re-base on,
            // so keep the authoritative PID verdict rather than masking a dead pid
            // as Alive. The production tick never builds this shape (it sets
            // `skip_tmux_check` iff both are absent), but `check_liveness_for_lifecycle`
            // is `pub` and must honor its own contract on any `AgentProbe`.
            None => verdict,
        },
        // `Alive` and `TmuxGone` already reflect the window correctly.
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Empty snapshot for the PID-only tests (no tmux probe is consulted when
    /// `skip_tmux_check` is set).
    fn empty_snapshot() -> WatchdogTmuxSnapshot {
        WatchdogTmuxSnapshot::default()
    }

    #[test]
    fn own_pid_has_start_time_and_is_alive() {
        let pid = std::process::id();
        let st = pid_start_time(pid).expect("self start_time");
        let probe = AgentProbe {
            pid,
            start_time: Some(st),
            tmux_window: None,
            tmux_identity: None,
            skip_tmux_check: true,
        };
        assert_eq!(check_liveness(&probe, &empty_snapshot()), Liveness::Alive);
    }

    #[test]
    fn dead_pid_is_dead() {
        // PID 0 is never a real process; treat as dead.
        let probe = AgentProbe {
            pid: 0,
            start_time: None,
            tmux_window: None,
            tmux_identity: None,
            skip_tmux_check: true,
        };
        assert_eq!(check_liveness(&probe, &empty_snapshot()), Liveness::Dead);
    }

    #[test]
    fn mismatched_start_time_detects_recycled_pid() {
        let pid = std::process::id();
        let probe = AgentProbe {
            pid,
            start_time: Some(1), // 1970-ish: cannot match a live process
            tmux_window: None,
            tmux_identity: None,
            skip_tmux_check: true,
        };
        assert_eq!(
            check_liveness(&probe, &empty_snapshot()),
            Liveness::Recycled
        );
    }

    #[test]
    fn start_time_is_stable_across_reads() {
        let pid = std::process::id();
        let a = pid_start_time(pid).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let b = pid_start_time(pid).unwrap();
        assert!(a.abs_diff(b) <= 1, "start_time drifted: {a} vs {b}");
    }

    // ---- Qualified-identity tmux matching --------------------------------
    // These tests mock `tmux` via TMUX_BIN. `std::env::set_var` is process-global
    // and unsafe to call concurrently with ANY other env access, so serialize
    // them behind the crate-wide env lock — the SAME lock the harness adapter
    // tests use (aider/pi set DEEPSEEK_API_KEY/GIT_BIN). A watchdog-local lock
    // would only exclude other watchdog tests, letting a concurrent harness test
    // corrupt the environ block mid-tick (issue
    // `idempotency-key-allowed-duplicate-run` surfaced this). The shared lock is
    // poison-tolerant, so one test's panic doesn't cascade into `PoisonError`s.
    use std::path::{Path, PathBuf};

    use crate::harness::support::test_env;

    /// RAII guard for a process-global env var: restores the prior value (or
    /// unsets) on drop, so a panicking assertion cannot leak `TMUX_BIN` into
    /// another test.
    struct EnvGuard {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }
    impl EnvGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let old = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, old }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.old {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn chmod_exec(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(p).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(p, perms).unwrap();
    }

    /// Write an executable fake `tmux` that records its argv (one arg per line)
    /// to `<dir>/args`, appends the invocation's full argv (space-joined) as one
    /// line to `<dir>/invocations` per call (so tests can count how many times the
    /// watchdog shelled out AND filter those calls by a per-test nonce carried in
    /// the argv, e.g. a unique `-S <socket>` — see
    /// [`invocations_matching`]), and replays the lines in `<dir>/stdout`, exiting
    /// `code`. Passing a non-zero `code` simulates "server unreachable" for the
    /// `Unknown` path. `stdout_lines` is written to a file (not interpolated into
    /// the script) so arbitrary content — including quotes — is safe.
    fn fake_tmux(dir: &Path, stdout_lines: &[&str], code: i32) -> PathBuf {
        let out = dir.join("stdout");
        std::fs::write(&out, stdout_lines.join("\n")).unwrap();
        let p = dir.join("fake-tmux.sh");
        let body = format!(
            "#!/bin/bash\nprintf '%s\\n' \"$@\" > {args:?}\necho \"$*\" >> {inv:?}\ncat {out:?}\nexit {code}\n",
            args = dir.join("args"),
            inv = dir.join("invocations"),
            out = out,
            code = code,
        );
        std::fs::write(&p, body).unwrap();
        chmod_exec(&p);
        p
    }

    /// Write an executable fake `tmux` that sleeps `secs` before exiting — used
    /// to drive the probe-timeout path. It never prints output; the watchdog is
    /// expected to kill it at the deadline.
    fn fake_tmux_slow(dir: &Path, secs: u32) -> PathBuf {
        let p = dir.join("fake-tmux-slow.sh");
        std::fs::write(&p, format!("#!/bin/bash\nsleep {secs}\nexit 0\n")).unwrap();
        chmod_exec(&p);
        p
    }

    fn args_lines(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("args"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// How many times the fake `tmux` was invoked (lines in `invocations`).
    fn invocation_count(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("invocations")).map_or(0, |s| s.lines().count())
    }

    /// How many recorded fake-`tmux` invocations carry `needle` in their argv line.
    /// Filtering on a per-test nonce (a unique `-S <socket>` this test alone
    /// passes) makes the count immune to a stray invocation another test appended
    /// to our counter by leaking our `TMUX_BIN` and running this same fake script:
    /// that foreign call uses a different socket (usually the default, no `-S`), so
    /// it never matches the nonce (issue `immoderately-irate-north`).
    fn invocations_matching(dir: &Path, needle: &str) -> usize {
        std::fs::read_to_string(dir.join("invocations"))
            .map_or(0, |s| s.lines().filter(|l| l.contains(needle)).count())
    }

    fn id(socket: Option<&str>, session: &str, window_id: &str) -> TmuxIdentity {
        TmuxIdentity {
            socket: socket.map(str::to_string),
            session: session.to_string(),
            window_id: window_id.to_string(),
            // Watchdog liveness keys off session:window_id, not the pane.
            pane_id: None,
            server_pid: None,
            server_pid_start_secs: None,
            server_marker: None,
        }
    }

    /// Build a one-tick snapshot for `sockets` via `WatchdogTmuxSnapshot::collect`
    /// (reads the `TMUX_BIN` fake the caller installed).
    fn snapshot(sockets: &[Option<&str>]) -> WatchdogTmuxSnapshot {
        let set: BTreeSet<Option<String>> = sockets.iter().map(|s| s.map(str::to_string)).collect();
        WatchdogTmuxSnapshot::collect(&set)
    }

    #[test]
    fn snapshot_parses_list_windows_fixture() {
        let _g = test_env::lock();
        let dir = tempfile::TempDir::new().unwrap();
        // A realistic `list-windows -a -F '#{window_id}|#{window_name}'` dump:
        // ids in column one, names (which may themselves contain `|`, and a
        // trailing `\r`) in column two; a blank trailing line to ignore.
        let _e = EnvGuard::set(
            "TMUX_BIN",
            fake_tmux(
                dir.path(),
                &["@7|main", "@42|🚀 wt/x", "@99|other|piped\r", ""],
                0,
            ),
        );
        let snap = snapshot(&[None]);

        // window_id matching (qualified path), split on the FIRST `|`.
        assert_eq!(
            snap.lookup_qualified(&id(None, "taskfleet", "@42")),
            TmuxProbe::Present
        );
        assert_eq!(
            snap.lookup_qualified(&id(None, "taskfleet", "@99")),
            TmuxProbe::Present
        );
        assert_eq!(
            snap.lookup_qualified(&id(None, "taskfleet", "@1")),
            TmuxProbe::Absent
        );
        // window_name matching (legacy path): the name keeps everything after
        // the first `|`, and the trailing `\r` is trimmed.
        assert_eq!(snap.lookup_by_name("🚀 wt/x"), TmuxProbe::Present);
        assert_eq!(snap.lookup_by_name("other|piped"), TmuxProbe::Present);
        assert_eq!(snap.lookup_by_name("nope"), TmuxProbe::Absent);

        // ONE invocation for the whole snapshot, listing ALL windows (-a) and
        // never session-scoping (-t).
        assert_eq!(invocation_count(dir.path()), 1);
        let args = args_lines(dir.path());
        assert!(args.iter().any(|a| a == "-a"), "expected -a: {args:?}");
        assert!(
            !args.iter().any(|a| a == "-t"),
            "must not session-scope: {args:?}"
        );
    }

    #[test]
    fn snapshot_passes_socket_when_recorded() {
        let _g = test_env::lock();
        let dir = tempfile::TempDir::new().unwrap();
        let _e = EnvGuard::set("TMUX_BIN", fake_tmux(dir.path(), &["@42|win"], 0));
        let snap = snapshot(&[Some("/tmp/sock")]);
        assert_eq!(
            snap.lookup_qualified(&id(Some("/tmp/sock"), "taskfleet", "@42")),
            TmuxProbe::Present
        );
        // The probe targeted the recorded socket via `-S <socket>`.
        let args = args_lines(dir.path());
        assert!(
            args.windows(2)
                .any(|w| w == ["-S".to_string(), "/tmp/sock".to_string()]),
            "socket not passed: {args:?}"
        );
    }

    #[test]
    fn snapshot_no_socket_flag_for_default() {
        let _g = test_env::lock();
        let dir = tempfile::TempDir::new().unwrap();
        let _e = EnvGuard::set("TMUX_BIN", fake_tmux(dir.path(), &["@7|a", "@99|b"], 0));
        let snap = snapshot(&[None]);
        // @42 is genuinely absent on a server that answered → definitive Absent.
        assert_eq!(
            snap.lookup_qualified(&id(None, "taskfleet", "@42")),
            TmuxProbe::Absent
        );
        assert!(
            !args_lines(dir.path()).iter().any(|a| a == "-S"),
            "unexpected -S with default socket"
        );
    }

    #[test]
    fn snapshot_unreachable_when_server_down() {
        let _g = test_env::lock();
        let dir = tempfile::TempDir::new().unwrap();
        // Non-zero exit == "no server running on <socket>" / error → not a
        // verdict; every lookup on this socket is Unknown.
        let _e = EnvGuard::set("TMUX_BIN", fake_tmux(dir.path(), &["no server running"], 1));
        let snap = snapshot(&[Some("/tmp/dead-sock")]);
        assert_eq!(
            snap.lookup_qualified(&id(Some("/tmp/dead-sock"), "taskfleet", "@42")),
            TmuxProbe::Unknown
        );
    }

    #[test]
    fn check_liveness_uses_qualified_identity_for_tmux_gone() {
        let _g = test_env::lock();
        let dir = tempfile::TempDir::new().unwrap();
        // Live PID (self) + start_time skipped, so the verdict turns purely on
        // the tmux probe. The fake server lists @99, not our @42 → TmuxGone.
        let _e = EnvGuard::set("TMUX_BIN", fake_tmux(dir.path(), &["@99|other"], 0));
        let probe = AgentProbe {
            pid: std::process::id(),
            start_time: None,
            tmux_window: Some("🚀 wt/x".to_string()),
            tmux_identity: Some(id(Some("/tmp/sock"), "taskfleet", "@42")),
            skip_tmux_check: false,
        };
        let snap = snapshot(&[Some("/tmp/sock")]);
        assert_eq!(check_liveness(&probe, &snap), Liveness::TmuxGone);
    }

    #[test]
    fn check_liveness_alive_when_qualified_window_present() {
        let _g = test_env::lock();
        let dir = tempfile::TempDir::new().unwrap();
        let _e = EnvGuard::set("TMUX_BIN", fake_tmux(dir.path(), &["@42|🚀 wt/x"], 0));
        let probe = AgentProbe {
            pid: std::process::id(),
            start_time: None,
            tmux_window: Some("🚀 wt/x".to_string()),
            tmux_identity: Some(id(None, "taskfleet", "@42")),
            skip_tmux_check: false,
        };
        let snap = snapshot(&[None]);
        assert_eq!(check_liveness(&probe, &snap), Liveness::Alive);
    }

    #[test]
    fn check_liveness_stays_alive_when_probe_unknown() {
        let _g = test_env::lock();
        let dir = tempfile::TempDir::new().unwrap();
        // Recorded socket's server is down (non-zero exit). The agent PID (self)
        // is alive, so an inconclusive tmux probe must NOT reap it — this is the
        // regression the tri-state fixes (wrong/dead socket → false TmuxGone).
        let _e = EnvGuard::set("TMUX_BIN", fake_tmux(dir.path(), &["no server"], 1));
        let probe = AgentProbe {
            pid: std::process::id(),
            start_time: None,
            tmux_window: Some("🚀 wt/x".to_string()),
            tmux_identity: Some(id(Some("/tmp/dead-sock"), "taskfleet", "@42")),
            skip_tmux_check: false,
        };
        let snap = snapshot(&[Some("/tmp/dead-sock")]);
        assert_eq!(check_liveness(&probe, &snap), Liveness::Alive);
    }

    #[test]
    fn check_liveness_falls_back_to_bare_name_without_identity() {
        let _g = test_env::lock();
        let dir = tempfile::TempDir::new().unwrap();
        // Legacy node: no identity. The fallback matches by window NAME on the
        // default socket.
        let _e = EnvGuard::set("TMUX_BIN", fake_tmux(dir.path(), &["@3|legacy-win"], 0));
        let probe = AgentProbe {
            pid: std::process::id(),
            start_time: None,
            tmux_window: Some("legacy-win".to_string()),
            tmux_identity: None,
            skip_tmux_check: false,
        };
        let snap = snapshot(&[None]);
        assert_eq!(check_liveness(&probe, &snap), Liveness::Alive);
    }

    #[test]
    fn snapshot_is_one_invocation_per_socket_regardless_of_node_count() {
        let _g = test_env::lock();
        let dir = tempfile::TempDir::new().unwrap();
        // A per-test nonce socket, unique to this TempDir. Probing it makes the
        // watchdog pass `-S <nonce>` to the fake, so the recorded invocation argv
        // carries the nonce — letting `invocations_matching` count ONLY this tick's
        // own tmux call and ignore any stray invocation a lock-forgetting test
        // might append to our counter by leaking our `TMUX_BIN` and running this
        // fake (that call would use the default socket, no `-S`, and never match).
        // This is what makes the count deterministic under full-parallel test runs
        // (issue `immoderately-irate-north`).
        let sock_buf = dir.path().join("wd.sock");
        let sock = sock_buf.to_str().unwrap();
        // Five nodes all live on the same (nonce) socket.
        let _e = EnvGuard::set(
            "TMUX_BIN",
            fake_tmux(dir.path(), &["@1|a", "@2|b", "@3|c", "@4|d", "@5|e"], 0),
        );
        // One snapshot for the tick — the watchdog gathers the distinct sockets
        // (here just our nonce socket) and probes each once.
        let snap = snapshot(&[Some(sock)]);
        // Evaluate all five nodes against it: pure set lookups, no extra forks.
        for n in 1..=5 {
            let probe = AgentProbe {
                pid: std::process::id(),
                start_time: None,
                tmux_window: None,
                tmux_identity: Some(id(Some(sock), "taskfleet", &format!("@{n}"))),
                skip_tmux_check: false,
            };
            assert_eq!(check_liveness(&probe, &snap), Liveness::Alive);
        }
        // The whole tick shelled out to tmux exactly once for our socket (was
        // once-per-node). Counting only OUR socket's invocations makes this immune
        // to cross-test counter pollution.
        assert_eq!(
            invocations_matching(dir.path(), sock),
            1,
            "expected a single tmux invocation for the tick"
        );
    }

    #[test]
    fn probe_timeout_yields_unreachable_and_pid_only_liveness() {
        let _g = test_env::lock();
        let dir = tempfile::TempDir::new().unwrap();
        // A wedged server: the fake sleeps far past the deadline. The probe must
        // be killed and the socket treated as Unreachable (→ Unknown lookups),
        // so a live PID is NOT reaped — PID liveness governs.
        let bin = fake_tmux_slow(dir.path(), 30);
        let sockets: BTreeSet<Option<String>> = [Some("/tmp/wedged".to_string())].into();
        let snap = WatchdogTmuxSnapshot::collect_with(
            &sockets,
            bin.to_str().unwrap(),
            Duration::from_millis(150),
        );
        assert_eq!(
            snap.lookup_qualified(&id(Some("/tmp/wedged"), "taskfleet", "@42")),
            TmuxProbe::Unknown,
            "a timed-out probe must be Unknown, never a false Absent"
        );

        let probe = AgentProbe {
            pid: std::process::id(),
            start_time: None,
            tmux_window: Some("🚀 wt/x".to_string()),
            tmux_identity: Some(id(Some("/tmp/wedged"), "taskfleet", "@42")),
            skip_tmux_check: false,
        };
        // Live PID + inconclusive (timed-out) tmux probe → stays Alive.
        assert_eq!(check_liveness(&probe, &snap), Liveness::Alive);
    }

    // ---- Lifecycle-aware liveness (interactive long-idle false positive) ----
    // Regression suite for `agent-died-merge-no-teardown-interactive`: on a
    // long-lived interactive run the recorded agent_pid goes stale (human quits
    // and restarts claude / it re-execs) while the tmux window lives on. A bare
    // PID probe fires a false `agent-died`; the window is the real liveness
    // signal for interactive runs. PID 0 is a deterministically-dead pid.
    const DEAD_PID: u32 = 0;

    /// THE fix: interactive node, recorded pid is dead, but the tmux window is
    /// still present → ALIVE, not the false `agent-died`. This is the exact
    /// long-idle window from the filed trace.
    #[test]
    fn interactive_dead_pid_with_live_window_is_alive() {
        let _g = test_env::lock();
        let dir = tempfile::TempDir::new().unwrap();
        let _e = EnvGuard::set("TMUX_BIN", fake_tmux(dir.path(), &["@42|🚀 wt/x"], 0));
        let probe = AgentProbe {
            pid: DEAD_PID,
            start_time: None,
            tmux_window: Some("🚀 wt/x".to_string()),
            tmux_identity: Some(id(None, "taskfleet", "@42")),
            skip_tmux_check: false,
        };
        let snap = snapshot(&[None]);
        // The bare PID verdict IS `Dead` — the false positive we're curing.
        assert_eq!(check_liveness(&probe, &snap), Liveness::Dead);
        // Interactive re-bases on the (present) window → Alive.
        assert_eq!(
            check_liveness_for_lifecycle(&probe, &snap, true),
            Liveness::Alive,
            "interactive: a stale pid with a live window must not read agent-died"
        );
        // Autonomous keeps the authoritative PID verdict → Dead.
        assert_eq!(
            check_liveness_for_lifecycle(&probe, &snap, false),
            Liveness::Dead,
            "autonomous: a dead fire-and-forget agent is genuinely dead"
        );
    }

    /// Interactive node, dead pid AND the window is genuinely gone → the
    /// streak-gated `TmuxGone` half-state (a truly-ended session still
    /// terminalizes; the caller's streak guards a transient false-Absent).
    #[test]
    fn interactive_dead_pid_with_absent_window_is_tmux_gone() {
        let _g = test_env::lock();
        let dir = tempfile::TempDir::new().unwrap();
        // Server answers but our @42 is not among its windows → definitive Absent.
        let _e = EnvGuard::set("TMUX_BIN", fake_tmux(dir.path(), &["@99|other"], 0));
        let probe = AgentProbe {
            pid: DEAD_PID,
            start_time: None,
            tmux_window: Some("🚀 wt/x".to_string()),
            tmux_identity: Some(id(None, "taskfleet", "@42")),
            skip_tmux_check: false,
        };
        let snap = snapshot(&[None]);
        assert_eq!(
            check_liveness_for_lifecycle(&probe, &snap, true),
            Liveness::TmuxGone,
            "interactive: dead pid + genuinely-absent window terminalizes (streak-gated)"
        );
    }

    /// Interactive node, dead pid, but the tmux probe is INCONCLUSIVE (server
    /// unreachable). A transient/operational tmux hiccup must never reap a live
    /// human session → stays Alive.
    #[test]
    fn interactive_dead_pid_with_unknown_window_stays_alive() {
        let _g = test_env::lock();
        let dir = tempfile::TempDir::new().unwrap();
        // Non-zero exit → server unreachable → Unknown.
        let _e = EnvGuard::set("TMUX_BIN", fake_tmux(dir.path(), &["no server"], 1));
        let probe = AgentProbe {
            pid: DEAD_PID,
            start_time: None,
            tmux_window: Some("🚀 wt/x".to_string()),
            tmux_identity: Some(id(Some("/tmp/dead-sock"), "taskfleet", "@42")),
            skip_tmux_check: false,
        };
        let snap = snapshot(&[Some("/tmp/dead-sock")]);
        assert_eq!(
            check_liveness_for_lifecycle(&probe, &snap, true),
            Liveness::Alive,
            "interactive: an inconclusive tmux probe must not reap a live session"
        );
    }

    /// Degenerate interactive node with NO window to probe (`skip_tmux_check`):
    /// no better signal exists, so the interactive path falls back to the PID
    /// verdict (dead pid → Dead). Guards against a stuck non-terminal node when
    /// there is genuinely nothing else to consult.
    #[test]
    fn interactive_without_window_falls_back_to_pid() {
        let probe = AgentProbe {
            pid: DEAD_PID,
            start_time: None,
            tmux_window: None,
            tmux_identity: None,
            skip_tmux_check: true,
        };
        let snap = WatchdogTmuxSnapshot::default();
        assert_eq!(
            check_liveness_for_lifecycle(&probe, &snap, true),
            Liveness::Dead,
            "interactive with no window to probe falls back to the PID verdict"
        );
    }

    /// The pub-API footgun guard: an interactive probe with `skip_tmux_check ==
    /// false` but NO identity and NO window name has nothing to re-base on, so a
    /// dead pid must stay `Dead` — never be masked as `Alive`. The production
    /// tick never builds this shape (it sets `skip_tmux_check` iff both are
    /// absent), but the function is `pub` and must honor its own contract.
    #[test]
    fn interactive_dead_pid_no_window_signal_keeps_pid_verdict() {
        let probe = AgentProbe {
            pid: DEAD_PID,
            start_time: None,
            tmux_window: None,
            tmux_identity: None,
            // Deliberately inconsistent shape: says "check tmux" but gives nothing.
            skip_tmux_check: false,
        };
        let snap = WatchdogTmuxSnapshot::default();
        assert_eq!(
            check_liveness_for_lifecycle(&probe, &snap, true),
            Liveness::Dead,
            "no window signal to re-base on → keep the authoritative PID verdict, \
             never mask a dead pid as Alive"
        );
    }

    /// The `Recycled` verdict (pid alive but `start_time` disagrees — the recorded
    /// agent exited and its pid was reused) re-bases on the window exactly like
    /// `Dead`: a present window means the human's session lives on. Uses this
    /// process's own live pid with a bogus recorded `start_time` to force
    /// `Recycled` out of `check_liveness`.
    #[test]
    fn interactive_recycled_pid_with_live_window_is_alive() {
        let _g = test_env::lock();
        let dir = tempfile::TempDir::new().unwrap();
        let _e = EnvGuard::set("TMUX_BIN", fake_tmux(dir.path(), &["@42|🚀 wt/x"], 0));
        let probe = AgentProbe {
            pid: std::process::id(),
            start_time: Some(1), // 1970: cannot match a live process → Recycled
            tmux_window: Some("🚀 wt/x".to_string()),
            tmux_identity: Some(id(None, "taskfleet", "@42")),
            skip_tmux_check: false,
        };
        let snap = snapshot(&[None]);
        assert_eq!(check_liveness(&probe, &snap), Liveness::Recycled);
        assert_eq!(
            check_liveness_for_lifecycle(&probe, &snap, true),
            Liveness::Alive,
            "interactive: a recycled pid with a live window must not terminalize"
        );
        assert_eq!(
            check_liveness_for_lifecycle(&probe, &snap, false),
            Liveness::Recycled,
            "autonomous: a recycled pid is authoritative"
        );
    }

    /// A genuinely-alive interactive agent stays Alive on both paths — the fix
    /// does not change the healthy case.
    #[test]
    fn interactive_live_pid_and_window_is_alive() {
        let _g = test_env::lock();
        let dir = tempfile::TempDir::new().unwrap();
        let _e = EnvGuard::set("TMUX_BIN", fake_tmux(dir.path(), &["@42|🚀 wt/x"], 0));
        let probe = AgentProbe {
            pid: std::process::id(),
            start_time: None,
            tmux_window: Some("🚀 wt/x".to_string()),
            tmux_identity: Some(id(None, "taskfleet", "@42")),
            skip_tmux_check: false,
        };
        let snap = snapshot(&[None]);
        assert_eq!(
            check_liveness_for_lifecycle(&probe, &snap, true),
            Liveness::Alive
        );
    }
}
