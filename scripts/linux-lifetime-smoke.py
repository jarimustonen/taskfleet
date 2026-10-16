#!/usr/bin/env python3
"""Bounded REAL Linux cgroup smoke. Requires a running user manager, tmux,
workmux and git; never starts/stops another user's or existing tmux service.
Uses a sleep worker (no model, credentials or installed Taskfleet changes).
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import uuid


def run(*args, env=None, cwd=None, check=True):
    return subprocess.run(args, env=env, cwd=cwd, check=check, text=True,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=140)


def wait(test, label, seconds=30):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = test()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError("timeout: " + label)


def main():
    if sys.platform != "linux":
        sys.exit("Linux only")
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/release/taskfleet").resolve()
    workmux = Path(sys.argv[2] if len(sys.argv) > 2 else shutil.which("workmux")).resolve()
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.geteuid()}"))
    manager_env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime),
                       DBUS_SESSION_BUS_ADDRESS=f"unix:path={runtime}/bus")
    run("systemctl", "--user", "show-environment", env=manager_env)
    foreground = len(sys.argv) > 3 and sys.argv[3] == "foreground"
    root = Path(tempfile.mkdtemp(prefix="taskfleet-lifetime-", dir="/var/tmp"))
    user = root / "user"
    user.mkdir()
    state = user / ".taskfleet"
    state.mkdir()
    tools = root / "bin"
    tools.mkdir()
    for name in ("git", "tmux"):
        (tools / name).symlink_to(shutil.which(name))
    (tools / "workmux").symlink_to(workmux)
    # No runtime or bus variables are given to create, reattach or merge.
    minimal = {"HOME": str(user), "USER": os.environ["USER"],
               "PATH": str(tools) + ":" + os.environ["PATH"]}
    worker = root / "worker"
    worker.write_text("#!/bin/sh\nexec /bin/sleep 240\n")
    worker.chmod(0o700)
    (state / "config.toml").write_text(
        '[profiles.smoke]\ndescription="bounded cgroup smoke"\ncapability="fast"\n'
        'residency="local"\nagents=[{harness="claude",command=[' + json.dumps(str(worker)) + ']}]\n'
        '[profile]\ndefault="smoke"\n')
    repo = root / "repo"
    run("git", "init", "-q", "-b", "main", str(repo))
    run("git", "-C", str(repo), "config", "user.name", "Lifetime fixture")
    run("git", "-C", str(repo), "config", "user.email", "fixture@example.invalid")
    run("git", "-C", str(repo), "commit", "--allow-empty", "-qm", "base")
    creator_socket = root / "creator.sock"
    if foreground:
        run(str(tools / "tmux"), "-f", "/dev/null", "-S", str(creator_socket), "new-session", "-d", "-s", "creator", env=minimal)
        pid = run(str(tools / "tmux"), "-S", str(creator_socket), "display-message", "-p", "#{pid}", env=minimal).stdout.strip()
        minimal["TMUX"] = f"{creator_socket},{pid},0"
    token = uuid.uuid4().hex[:12]
    parent_unit = "taskfleet-test-parent-" + token
    tmux_unit = "taskfleet-tmux-" + hashlib.sha256(os.fsencode(state)).hexdigest()[:20]
    socket = runtime / "taskfleet" / (tmux_unit.removeprefix("taskfleet-") + ".sock")
    # socket uses tmux-<home digest>, not the service's taskfleet- prefix.
    result = root / "create.json"
    caller = root / "caller.py"
    caller.write_text(
        "import json, subprocess, time, os\n"
        + "out = subprocess.run(" + repr([str(binary), "run", "create", "--kind", "spinoff",
          "--title", "lifetime fixture", "--task", "sleep fixture",
          "--no-hooks", "--idempotency-key", token, "--notify",
          "sleep 1; printf delivered > " + str(root / "hook-done"), "--json"] + ([] if foreground else ["--headless"])) + ", env=" + repr(minimal)
        + ", cwd=" + repr(str(repo)) + ", capture_output=True, text=True)\n"
        + "open(" + repr(str(root / "caller.stderr")) + ", 'w').write(out.stderr)\n"
        + "assert out.returncode == 0, out.stderr\n"
        + "open(" + repr(str(result)) + ", 'w').write(out.stdout)\n"
        + "time.sleep(240)\n")
    run_id = None
    owned_pids = {}
    try:
        run("systemd-run", "--user", "--collect", "--quiet", "--service-type=exec",
            "--unit=" + parent_unit, "--property=PrivateTmp=yes", "--property=KillMode=mixed",
            "--property=TimeoutStopSec=1", "--property=RuntimeMaxSec=240",
            sys.executable, str(caller), env=manager_env)
        wait(lambda: result.exists(), "first create")
        first = json.loads(result.read_text())["data"]
        run_id = first["run_id"]
        run_dir = state / "runs" / run_id
        node = json.loads((run_dir / "nodes" / "n-0001.json").read_text())
        socket = Path(node["tmux_identity"]["socket"])
        session = node["tmux_identity"]["session"]
        assert json.loads((run_dir / "manifest.json").read_text())["managed_tmux_session"] == session
        if foreground:
            run(str(tools / "tmux"), "-S", str(creator_socket), "kill-server", env=minimal)
        server_pid = node["tmux_identity"]["server_pid"]
        worker_pid = node["agent_pid"]
        supervisor_pid = int((run_dir / "supervisor.pid").read_text().split()[0])
        pids = [server_pid, worker_pid, supervisor_pid]
        assert all(pids), node
        owned_pids = {pid: Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19] for pid in pids}
        cgroups = [Path(f"/proc/{pid}/cgroup").read_text() for pid in pids]
        assert all(parent_unit not in group for group in cgroups), cgroups
        assert "taskfleet-tmux-" in cgroups[0], cgroups
        # New tmux versions place panes in their own user scopes. Older
        # versions leave them in the independently owned server service.
        assert "taskfleet-tmux-" in cgroups[1] or "tmux-spawn-" in cgroups[1], cgroups
        assert "taskfleet-supervise-" in cgroups[2], cgroups
        # Reach the recorded socket from outside PrivateTmp (SSH-equivalent).
        assert socket.is_relative_to(runtime / "taskfleet"), socket
        assert socket.parent.stat().st_mode & 0o077 == 0
        run("tmux", "-S", str(socket), "has-session", "-t", session, env=minimal)
        for _ in range(2):
            result.unlink()
            run("systemctl", "--user", "restart", parent_unit, env=manager_env)
            wait(lambda: result.exists(), "idempotent create after restart")
            assert json.loads(result.read_text())["data"]["run_id"] == run_id
            for pid in pids:
                os.kill(pid, 0)
            assert [Path(f"/proc/{pid}/cgroup").read_text() for pid in pids] == cgroups
            run("tmux", "-S", str(socket), "has-session", "-t", session, env=minimal)
        # Reattach only this supervisor; its independently owned server/worker
        # must not die with the old supervisor unit.
        own_units = run("systemctl", "--user", "list-units", "--all", "--plain", "--no-legend",
                        f"taskfleet-supervise-{run_id}-*", env=manager_env).stdout.splitlines()
        for line in own_units:
            run("systemctl", "--user", "stop", line.split()[0], env=manager_env)
        run(str(binary), "run", "reattach", run_id, "--json", env=minimal)
        for pid in [server_pid, worker_pid]:
            os.kill(pid, 0)
        assert int((run_dir / "supervisor.pid").read_text().split()[0]) != supervisor_pid
        # A real explicit merge exercises the unchanged cleanup authorization.
        checkout = Path(node["worktree_path"])
        (checkout / "WORK").write_text("bounded lifetime fixture\n")
        run("git", "-C", str(checkout), "add", "-A")
        run("git", "-C", str(checkout), "commit", "-qm", "fixture work")
        run(str(binary), "run", "merge", run_id, "--json", env=minimal, cwd=checkout)
        wait(lambda: not checkout.exists(), "worktree cleanup")
        wait(lambda: run("tmux", "-S", str(socket), "has-session", check=False).returncode != 0,
             "last session/server cleanup")
        wait(lambda: not run("systemctl", "--user", "list-units", "--plain", "--no-legend",
                             "--state=active,activating,deactivating", tmux_unit + "-*", env=manager_env).stdout.strip(), "tmux service collection")
        wait(lambda: (root / "hook-done").exists(), "delayed notification completes after supervisor exit", seconds=5)
        wait(lambda: not run("systemctl", "--user", "list-units", "--plain", "--no-legend",
                             "--state=active,activating,deactivating", f"taskfleet-supervise-{run_id}-*", env=manager_env).stdout.strip(), "supervisor service collection")
        print(json.dumps({"real_user_manager": True, "private_tmp": True,
                          "minimal_environment": sorted(minimal), "parent_restarts": 2,
                          "idempotent_create": True, "worker_and_server_survived": True,
                          "reattach": True, "explicit_merge_cleanup": True, "foreground": foreground, "delayed_notify": True}))
    except Exception as error:
        if isinstance(error, subprocess.CalledProcessError):
            print(error.stderr, file=sys.stderr)
            print(error.stdout, file=sys.stderr)
        if (root / "caller.stderr").exists():
            print((root / "caller.stderr").read_text(), file=sys.stderr)
        raise
    finally:
        # Only services whose unique names derive from this disposable home/run.
        run("systemctl", "--user", "stop", parent_unit, env=manager_env, check=False)
        units = run("systemctl", "--user", "list-units", "--all", "--plain", "--no-legend", tmux_unit + "-*", env=manager_env).stdout.splitlines()
        for line in units:
            run("systemctl", "--user", "stop", line.split()[0], env=manager_env, check=False)
        if foreground:
            run(str(tools / "tmux"), "-S", str(creator_socket), "kill-server", env=minimal, check=False)
        if run_id:
            units = run("systemctl", "--user", "list-units", "--all", "--plain", "--no-legend",
                        f"taskfleet-supervise-{run_id}-*", env=manager_env).stdout.splitlines()
            for line in units:
                run("systemctl", "--user", "stop", line.split()[0], env=manager_env, check=False)
        for pid, start in owned_pids.items():
            try:
                current = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
                if current[19] == start and current[0] != "Z":
                    os.kill(pid, 15)
            except ProcessLookupError:
                pass
            except FileNotFoundError:
                pass
        shutil.rmtree(root)


if __name__ == "__main__":
    main()
