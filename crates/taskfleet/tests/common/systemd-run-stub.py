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
assert "--property=UnsetEnvironment=TASKFLEET_CREATOR_SESSION" in args
assert not any(a.startswith("--setenv=TASKFLEET_CREATOR_SESSION=") for a in args)
env = os.environ.copy()
# A user manager can retain values unrelated to the current caller.
env["TASKFLEET_CREATOR_SESSION"] = "pi:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
unset = []
log = os.devnull
assert "notify" not in args or "--property=ExitType=cgroup" in args, "notify hooks must survive main PID exit"
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
    elif arg.startswith("--property=UnsetEnvironment="):
        unset.extend(arg.split("=", 2)[2].split())
    elif arg.startswith("--property=StandardError=append:"):
        log = arg.split("append:", 1)[1].replace("%%", "%")
for key in unset:
    env.pop(key, None)
if kind != "notify":
    sys.exit(subprocess.run(args, env=env, cwd=cwd, check=False).returncode)  # fixture foreground server
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
