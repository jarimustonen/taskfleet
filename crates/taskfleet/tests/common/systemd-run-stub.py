#!/usr/bin/python3
"""CI-only user-manager stub. No systemd or developer tmux dependency."""
import os
import socket
import subprocess
import sys
import tempfile
import time

args = sys.argv[1:]
assert "--user" in args and "--collect" in args, "must use a collected USER service"
env = os.environ.copy()
log = os.devnull
kind = None
cwd = None
while args:
    arg = args.pop(0)
    if arg == "--":
        break
    if arg == "--service-type":
        kind = args.pop(0)
    elif arg == "--unit":
        args.pop(0)
    elif arg.startswith("--working-directory="):
        cwd = arg.split("=", 1)[1]
    elif arg.startswith("--setenv="):
        key, value = arg[len("--setenv="):].split("=", 1)
        env[key] = value
    elif arg.startswith("--property=StandardError=append:"):
        log = arg.split("append:", 1)[1].replace("%%", "%")
if kind != "notify":
    sys.exit(0)  # fake tmux owns no server process
with tempfile.TemporaryDirectory(prefix="notify-", dir=env["XDG_RUNTIME_DIR"]) as tmp:
    address = tmp + "/n"
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
    listener.bind(address)
    listener.settimeout(0.02)
    env["NOTIFY_SOCKET"] = address
    with open(log, "ab") as output:
        child = subprocess.Popen(args, env=env, stdin=subprocess.DEVNULL,
                                 stdout=output, stderr=output, start_new_session=True, cwd=cwd)
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        try:
            if listener.recv(4096) == b"READY=1":
                sys.exit(0)
        except socket.timeout:
            pass
        if child.poll() is not None:
            print("stub user service exited during boot", file=sys.stderr)
            sys.exit(1)
    child.kill()
    child.wait()
    print("stub user service boot timed out", file=sys.stderr)
    sys.exit(1)
