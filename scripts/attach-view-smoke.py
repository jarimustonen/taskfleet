#!/usr/bin/env python3
"""Opt-in Linux tmux-semantics smoke; never connects to an existing server.

Run after cargo build --release:
  python3 scripts/attach-view-smoke.py --taskfleet ./target/release/taskfleet --tmux /usr/bin/tmux
Requires explicit binaries; not part of bare-CI tests. Seeds a skeleton through
Taskfleet's event API (no agent/supervisor), then exercises the real native viewer
in a pty. All state, shells and the server are contained by one private socket.
"""
import argparse
import json
import os
import pathlib
import pty
import select
import signal
import subprocess
import sys
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--taskfleet", type=lambda p: str(pathlib.Path(p).resolve()), required=True)
    parser.add_argument("--tmux", type=lambda p: str(pathlib.Path(p).resolve()), required=True)
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("this opt-in smoke uses Linux /proc start identities")
    with tempfile.TemporaryDirectory(prefix="taskfleet-view-", dir="/var/tmp") as temp:
        root = pathlib.Path(temp)
        socket = str(root / "socket ' $;")
        home = root / "home ' $;"
        env = dict({k: v for k, v in os.environ.items() if not k.startswith("TASKFLEET_")}, HOME=str(root), TASKFLEET_HOME=str(home),
                   TMUX_BIN=args.tmux, TASKFLEET_TEST_SKIP_MATERIALIZE="1",
                   TERM="xterm-256color")
        env.pop("TMUX", None)
        session = "agents ' $; odd"
        token = "TASKFLEET_VIEW_READY"
        child = None
        master = None

        def tmux(*argv):
            return subprocess.check_output([args.tmux, "-N", "-S", socket, *argv], env=env).decode().strip()

        def taskfleet(*argv):
            return json.loads(subprocess.check_output([args.taskfleet, *argv, "--json"], env=env))["data"]

        def inventory():
            return tmux("list-windows", "-t", "=" + session, "-F", "#{window_id}:#{window_active}:#{pane_id}:#{window_zoomed_flag}")

        try:
            # This is the sole explicit server-start operation, on our fresh socket.
            subprocess.check_call([args.tmux, "-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", session, "/bin/sleep 120"], env=env)
            window = tmux("new-window", "-d", "-P", "-F", "#{window_id}", "-t", "=" + session, "/bin/sleep 120")
            pane = tmux("split-window", "-d", "-P", "-F", "#{pane_id}", "-t", window, f"printf '{token}\\n'; exec /bin/sleep 120")
            pid = int(tmux("display-message", "-p", "-t", pane, "#{pid}"))
            stat = pathlib.Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
            boot = next(int(line.split()[1]) for line in pathlib.Path("/proc/stat").read_text().splitlines() if line.startswith("btime "))
            start = boot + int(stat[19]) // os.sysconf("SC_CLK_TCK")
            run = taskfleet("run", "create", "--kind", "spinoff", "--title", "isolated viewer smoke")["run_id"]
            record = root / "node.json"
            record.write_text(json.dumps({"kind": "spinoff", "tmux_socket": socket,
                "tmux_session": session, "tmux_window_id": window, "tmux_pane_id": pane,
                "tmux_server_pid": pid, "tmux_server_pid_start_secs": start,
                "tmux_server_marker": tmux("display-message", "-p", "-t", pane, "#{HOMEBASE_TMUX_OWNER}")}))
            taskfleet("event", "create", run, "--kind", "node.created", "--node-id", "n-0001", "--from-file", str(record))
            assert taskfleet("run", "attach", run)["state"] == "validated"
            # Grouping would make keep-last destroy this unattached source.
            tmux("set-option", "-t", session, "destroy-unattached", "keep-last")
            for mode in ["normal", "hidden-by-zoom", "visible-zoomed-target"]:
                if mode == "hidden-by-zoom":
                    tmux("resize-pane", "-Z", "-t", window)
                elif mode == "visible-zoomed-target":
                    # This is deliberate setup in our own fixture, never product behavior.
                    tmux("select-pane", "-t", pane)
                    tmux("resize-pane", "-Z", "-t", pane)
                before = inventory()
                child, master = pty.fork()
                if child == 0:
                    os.execve(args.taskfleet, [args.taskfleet, "run", "attach", run], env)
                output = b""
                deadline = time.monotonic() + 15
                while token.encode() not in output or mode == "hidden-by-zoom":
                    remaining = deadline - time.monotonic()
                    ready, _, _ = select.select([master], [], [], max(0, remaining))
                    assert ready and remaining > 0, ("viewer readiness/exit timeout", output)
                    try:
                        chunk = os.read(master, 65536)
                    except OSError:
                        break  # Linux pty EIO after client exits.
                    if not chunk:
                        break
                    output += chunk
                if mode != "hidden-by-zoom":
                    clients = tmux("list-clients", "-F", "#{session_name}\t#{window_id}\t#{client_tty}\t#{client_flags}").split("\t")
                    assert clients[0].startswith("taskfleet-view-"), clients
                    assert clients[1] == window, clients
                    assert all(flag in clients[3].split(",") for flag in ["read-only", "ignore-size", "active-pane"]), clients
                    assert inventory() == before, "viewer changed source window/pane/zoom"
                    tmux("detach-client", "-t", clients[2])
                _, status = os.waitpid(child, 0)
                child = None
                os.close(master)
                master = None
                assert os.waitstatus_to_exitcode(status) == (1 if mode == "hidden-by-zoom" else 0), (status, output)
                if mode == "hidden-by-zoom":
                    assert b"target hidden by zoom" in output, output
                assert tmux("list-sessions", "-F", "#{session_name}") == session, "viewer session leaked or source destroyed"
                assert inventory() == before, "source selection, zoom or windows changed after viewer exit"
                print(f"PASS: {mode}; read-only independent viewer, no source changes, viewer expires")
        finally:
            if child:
                os.kill(child, signal.SIGTERM)
                os.waitpid(child, 0)
            if master is not None:
                os.close(master)
            if pathlib.Path(socket).exists():
                tmux("kill-server")


if __name__ == "__main__":
    main()
