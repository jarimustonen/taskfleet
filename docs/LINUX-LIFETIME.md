# Linux worker lifetime

On Linux, Taskfleet-owned workers require a running **systemd user manager**
(systemd 250 or newer).
Homebase provisions lingering for browser users; Taskfleet does not enable it,
restart the manager, or fall back to `setsid`, `nohup` or double-forking. Those
techniques leave descendants in the calling daemon's cgroup and cannot survive
its restart.

## Ownership

- Each supervisor starts in a transient `taskfleet-supervise-<full-run-id>-<generation>.service`.
  The manager starts the exact currently running Taskfleet executable. `Type=notify`
  confirms the existing PID-claim/initialization readiness point; a private
  per-launch receipt carries its PID back across the manager boundary. Startup
  failures remain actionable failures, not successful unconfirmed launches.
  `ExitType=cgroup` lets asynchronous notification hooks finish after the main
  supervisor exits; the unit is collected when its final child exits. A slow
  hook never blocks the supervisor tick.
- A shared `taskfleet-tmux-<state-home-digest>-<generation>.service` runs `tmux -D` in the
  foreground. Workers are children of this server, **not** of the creating
  daemon or supervisor. New tmux versions give panes their own manager-owned
  `tmux-spawn-*.scope` units. Reattaching a supervisor does not stop the workers.
  Different state homes get different servers and bootstrap locks.
- The server starts only from materialization. Its first session is created
  before enabling `exit-empty`. Closing the last managed session through normal
  supervisor cleanup ends the server and collects its service. Retained displays,
  another live run, or an attached/user-modified session keep it alive under the
  existing retention and preservation rules. Neither cancellation nor reattach
  stops a shared server unit directly.
- Ordinary foreground placement uses a session with the current session's name
  on this dedicated server. This new session is recorded as managed, so normal
  cleanup removes its unused bootstrap shell too, and retries do not need the
  original caller's server to remain alive. Explicit `--headless` and `--tmux-session` still
  choose the session name. Outside tmux, one of those flags remains required.

The socket is `$XDG_RUNTIME_DIR/taskfleet/tmux-<state-home-digest>.sock`.
Use the **recorded** node `tmux_identity.socket`, not a guessed name, to inspect
it from SSH:

```sh
tmux -S <recorded-socket> attach -t <recorded-session>
```

This socket is outside a daemon's `PrivateTmp` namespace. The runtime directory
must be an absolute, non-symlink directory owned by the effective uid, with no
group/other permissions. Taskfleet creates and validates its own mode-0700
subdirectory as well. Other Unix users cannot traverse either boundary.

## Minimal environment and errors

The creating process may have **only PATH, USER and HOME**. If `XDG_RUNTIME_DIR`
is unset, Taskfleet uses `/run/user/<effective-uid>` only after validating it.
An explicitly empty, missing, insecure or wrong-owner directory fails with
`linux_runtime_dir_invalid`; it does not fall back to `/tmp`. The derived runtime
and same-user bus address are supplied to `systemd-run --user` and its service.
The caller's environment is forwarded per service; Taskfleet never imports it
into the shared manager. Service identity/readiness variables are not forwarded
from a parent service.

A missing/unreachable manager or a failed service start yields
`linux_user_unit_failed` (or `supervisor_spawn_failed` with that cause at create).
Check `systemctl --user status`, the run's `supervisor.stderr.log` and
`journalctl --user -u 'taskfleet-*'`. An administrator can provision lingering
with `loginctl enable-linger <user>`. Taskfleet never does so automatically.

Non-Linux systems retain native tmux placement and the existing readiness-pipe,
setsid/double-fork supervisor launch. No systemd executable or runtime-directory
validation is needed there. Caller-owned runs do not launch Taskfleet workers
and are outside this contract.

## Validation

The regular release suite uses an explicit user-manager stub in disposable
runtime directories. It checks service argv, sd_notify, errors, native creation,
idempotency, reattach and cleanup without requiring systemd or touching the
real user's server. `SYSTEMD_RUN_BIN`, like `TMUX_BIN`/`WORKMUX_BIN`, is an
executable override for such isolated fixtures; it is not a native-lifetime
fallback. These tests are **not evidence of cgroup survival**.

For real lifetime evidence on a Linux host with a running user manager:

```sh
mkdir -p /var/tmp/taskfleet-build
TMPDIR=/var/tmp/taskfleet-build cargo build --release -p taskfleet
python3 scripts/linux-lifetime-smoke.py ./target/release/taskfleet /absolute/path/to/native/workmux
python3 scripts/linux-lifetime-smoke.py ./target/release/taskfleet /absolute/path/to/native/workmux foreground
```

The bounded smoke uses a disposable home/repository, an inert sleep worker,
and uniquely named user services. Its parent service has `PrivateTmp=yes` and
`KillMode=mixed`. It creates with the three-variable environment, restarts the
parent twice, checks worker **and tmux server** cgroups/liveness and external
socket discovery, retries keyed creation, reattaches the supervisor, then
explicitly merges and verifies worktree/session/server/service teardown. It also
checks delayed notification delivery and supervisor service collection.
The foreground variant creates a private originating server, records the derived
managed session, then removes the originating server before retry/reattach and
cleanup. That variant additionally needs `TMUX`; the headless variant exercises
the three-variable caller environment. The smoke
cleans only services derived from its own disposable home and full run id.
It requires real Git, tmux, workmux, Python and a manager capable of user-service
`PrivateTmp`; absence is a missing prerequisite, never a stub success.
