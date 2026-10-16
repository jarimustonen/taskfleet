# Linux independent lifetime: validation

## Delivered boundary

Taskfleet-owned Linux spinoffs launch their supervisor and tmux server through
independent transient systemd user services. The user manager must be systemd
250+; no process-group/double-fork fallback is used. Sockets are recorded under
the validated, caller-owned private runtime directory. The selected state home,
PATH and caller environment cross the manager boundary without modifying the
manager's global environment. Non-Linux retains native launch.

`ExitType=cgroup` preserves asynchronous notification hooks after the supervisor
main PID exits. Normal teardown still belongs only to the supervisor and the
existing typed outcome policy. No merge, preservation or event-reduction rules
were changed.

## Real user-manager evidence

On Linux with systemd 259 and tmux 3.6, built the local release binary with
`TMPDIR=/var/tmp/taskfleet-linux-build cargo build --release -p taskfleet` and ran:

```sh
python3 scripts/linux-lifetime-smoke.py ./target/release/taskfleet /home/jari/.local/bin/workmux
python3 scripts/linux-lifetime-smoke.py ./target/release/taskfleet /home/jari/.local/bin/workmux foreground
```

Both passed. Each used an independent disposable parent user service with
`PrivateTmp=yes`, `KillMode=mixed`, an inert sleep worker, and its own home,
repository, socket and uniquely scoped service names. Assertions covered:

- Supervisor and tmux server belong to Taskfleet user services outside the
  parent's cgroup. This tmux version gives workers their own `tmux-spawn` user
  scope, also outside the parent. Neither worker nor server depended on the
  supervisor service's lifetime.
- Two parent restarts per scenario leave the same worker/server PIDs and cgroups
  alive. Keyed creation after each restart returns the original full run id.
- Headless creation receives exactly PATH, USER and HOME: runtime-dir and bus
  discovery work without widening that environment.
- Foreground creation additionally receives TMUX. Its derived session is
  recorded as managed. The originating disposable tmux server is killed after
  initial creation; idempotent replay, supervisor reattach and cleanup still
  work without that original server.
- An external same-user process outside PrivateTmp reaches the recorded socket.
  Its private runtime parent has no group/other access bits.
- Stopping only the recorded supervisor service and reattaching changes the
  supervisor PID while preserving the worker and server.
- A real explicit merge removes the disposable checkout and managed session,
  ends the tmux server, and collects its service. A delayed notification writes
  its marker after supervisor exit; the supervisor service is then collected.

Logs on this machine: `/var/tmp/taskfleet-linux-build/real-headless-hooks.log`
and `real-foreground-hooks.log`. The script cleans only its own services and
resources. No installed Taskfleet binary, installed skills, household service,
or existing tmux session was modified.

## Bare-CI / stub evidence

The focused release-mode suite passed: `caller_create` (20),
`creation_reliability` (4), `e2e_spinoff` (4), `linux_lifetime` (5), and `run`
(52). Clippy with warnings denied passed. All Rust commands used the disk-backed
TMPDIR above.

These fixtures explicitly stub the user manager and tmux, including stripped
PATH caller-owned tests. They check typed missing-manager errors without native
fallback, invalid runtime modes with no worktree side effects, wrong owner,
symlink/missing runtime rejection, service/readiness argv, idempotency, reattach,
merge and preservation paths. A deterministic server-disappearance fixture
checks one manager-owned bootstrap retry; a typed boot-error fixture checks that
systemd startup failure does not erase the supervisor's error code/message.
They are **not cgroup-survival evidence**.

## Independent review and corrections

One focused independent review by `claude-fable-5-1`, no panel/repeated review.
All three findings were confirmed against source and fixed in this issue:

1. Foreground Linux sessions were created on the private server but not recorded
   as managed, leaving a bootstrap shell/server after merge. Session resolution
   now happens before `run.created`, and the foreground real smoke proves final
   cleanup after the original server is gone.
2. A sibling's teardown could end the server after the initial creator probe.
   Under the existing creator lock, a failed session creation now re-probes and
   bootstraps once through the manager. Generation-specific service names avoid
   colliding with the old service's collection. `-N` still forbids replacement
   inside the caller's cgroup. The forced-race fixture exercises this path.
3. Structured boot errors were written only to the native readiness pipe.
   Linux now records the same error frame in the private receipt and parses it
   even when service startup fails. The typed-error fixture checks the message.

Host-side checking additionally reproduced default systemd main-PID exit
killing a delayed notification hook. `ExitType=cgroup` fixes that without
changing hook ordering or blocking the supervisor; both real smokes now assert
hook delivery and eventual unit collection. No findings remain deferred and no
follow-up issues were filed.

## Gate and limitations

The required command is `scripts/validate-local-release.sh`, with the same
TMPDIR. Its exact successful commit/result is recorded in the issue closure
note and worker report. Earlier gate attempts exposed the missing caller-test
manager fixture; that fixture was fixed. A subsequent in-progress gate was
stopped before tests to incorporate the confirmed review/lifetime corrections.

The real evidence is a disposable **user-service** parent, not a restart of
Open Habitat's actual production daemon. No household service was restarted.
Missing-manager and wrong-owner cases are deterministic stub/unit tests, not a
shutdown of the live manager or a cross-user login. Other-user exclusion follows
from validated uid ownership and mode-0700 directory traversal. Non-Linux
runtime behavior was not executed on this Linux host; its native implementation
is conditionally preserved. No release/tag/publication is part of this worker.
