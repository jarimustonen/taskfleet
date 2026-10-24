#!/usr/bin/env python3
"""Bounded real-Pi message proof using current user profiles and private resources.

Run from a checkout after cargo build --release. No installed binaries, skills,
configuration, real repositories, or shared tmux sessions are modified.
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


def wait_for(check, description, seconds=90):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.1)
    raise RuntimeError(f"timed out: {description}")


def main():
    binary = Path(os.environ.get("TASKFLEET_SMOKE_BIN", "target/release/taskfleet")).resolve()
    user_home = Path.home()
    config = Path(os.environ.get("TASKFLEET_HOME", user_home / ".taskfleet")) / "config.toml"
    agent_dir = os.environ.get("PI_CODING_AGENT_DIR", str(user_home / ".pi/agent"))
    # Short private paths also keep Unix-domain socket names below OS limits.
    with tempfile.TemporaryDirectory(prefix="tfm.", dir=os.environ.get("TMPDIR", "/var/tmp")) as tmp:
        root = Path(tmp)
        state, repo = root / "state", root / "repo"
        state.mkdir(mode=0o700)
        repo.mkdir()
        (root / "user").mkdir()
        (root / "bin").mkdir()
        shutil.copyfile(config, state / "config.toml")
        socket = root / "tmux.sock"
        tmux = shutil.which("tmux")
        assert tmux
        wrapper = root / "bin/tmux"
        wrapper.write_text(f'#!/bin/sh\nexec "{tmux}" -S "{socket}" "$@"\n')
        wrapper.chmod(0o700)
        linux = sys.platform == "linux"
        if linux:
            # The production Linux launcher owns its manager-backed server,
            # keyed by this disposable state home. Do not bypass that lifetime.
            runtime = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.geteuid()}"))
            key = hashlib.sha256(os.fsencode(state)).hexdigest()[:20]
            socket = runtime / "taskfleet" / f"tmux-{key}.sock"
        env = os.environ.copy()
        env.update(HOME=str(root / "user"), TASKFLEET_HOME=str(state),
                   PI_CODING_AGENT_DIR=agent_dir, TMUX_BIN=tmux if linux else str(wrapper))
        native_workmux = user_home / ".local/bin/workmux"
        if linux and "WORKMUX_BIN" not in env and native_workmux.is_file():
            # A seat-routing shell wrapper may resolve relative to HOME; use
            # the already installed native executable with our disposable HOME.
            env["WORKMUX_BIN"] = str(native_workmux)
        # Isolate Taskfleet's selection from ambient caller overrides; use the
        # copied installed config's default profile unchanged.
        for key in ("TASKFLEET_PROFILE", "TASKFLEET_HARNESS", "TMUX", "TMUX_PANE"):
            env.pop(key, None)

        def cmd(argv, **kwargs):
            result = subprocess.run(argv, env=env, cwd=repo, text=True,
                                    capture_output=True, timeout=120, **kwargs)
            if result.returncode:
                raise RuntimeError(f"command failed ({result.returncode}): {result.stderr}")
            return result

        def tf(*args):
            return json.loads(cmd([str(binary), "--output", "json", *args]).stdout)["data"]

        cmd(["git", "init", "-q", "-b", "main"])
        cmd(["git", "-c", "user.name=Message Smoke", "-c", "user.email=smoke@example.invalid",
             "commit", "--allow-empty", "-qm", "base"])
        if not linux:
            cmd([str(wrapper), "new-session", "-d", "-s", "message-smoke", "-c", str(repo)])
        run_id = None
        worktree = None
        try:
            created = tf("run", "create", "--kind", "spinoff", "--tmux-session", "message-smoke",
                         "--title", "disposable-message-proof", "--task",
                         "This is a disposable live-message smoke test. Do not merge, report, commit, "
                         "or modify files. Reply exactly MESSAGE_READY and finish this turn. "
                         "Remain available at your prompt for a later steering message.")
            run_id = created["run_id"]
            worktree = Path(created["worktree_path"])
            run_dir = state / "runs" / run_id
            node_path = run_dir / "nodes/n-0001.json"
            node = json.loads(node_path.read_text())
            manifest = json.loads((run_dir / "manifest.json").read_text())
            print(json.dumps({"run_id": run_id, "status": node["status"],
                              "selection": manifest["agent_selection"]}), flush=True)
            assert node["status"] == "pending", node["status"]
            session_path = state / node["evidence"]["live_session_path"]
            adapter = state / f".creating/message-{run_id}-{node['evidence']['attempt']}.sock"
            wait_for(lambda: adapter.exists(), "bundled extension listening")
            def ready():
                if not session_path.exists():
                    return False
                for line in session_path.read_text().splitlines():
                    try:
                        message = json.loads(line).get("message", {})
                    except json.JSONDecodeError:
                        continue  # The writer may still be appending its last line.
                    if message.get("role") == "assistant" and any(
                        part.get("type") == "text" and part.get("text", "").strip() == "MESSAGE_READY"
                        for part in message.get("content", []) if isinstance(part, dict)
                    ):
                        return True
                return False

            wait_for(ready, "initial assistant MESSAGE_READY turn")
            # Proof of action is separate from the adapter acceptance receipt.
            receipt = tf("run", "message", run_id,
                         "Live-message smoke instruction: use the bash tool to write exactly "
                         "MESSAGE_ACTED to the file message-acted.txt in your current worktree. "
                         "Do not merge, report, commit, or exit. Then finish your turn.")
            print(json.dumps({"receipt": receipt}), flush=True)
            assert receipt["delivery"] == "accepted"
            acted = worktree / "message-acted.txt"
            wait_for(lambda: acted.exists() and acted.read_text().strip() == "MESSAGE_ACTED",
                     "worker acts on delivered instruction")
            print("worker action verified: message-acted.txt == MESSAGE_ACTED", flush=True)
            events = [json.loads(line) for line in (run_dir / "events.jsonl").read_text().splitlines()]
            print(json.dumps({"message_events": [e for e in events if e["kind"].startswith("worker.message.")]}), flush=True)
        finally:
            try:
                if run_id:
                    tf("run", "cancel", run_id)
                    wait_for(lambda: '"kind":"supervisor.exited"' in
                             (state / "runs" / run_id / "events.jsonl").read_text(), "cancel teardown")
                    discarded = tf("run", "discard", run_id, "--force", "--reason",
                                   "dispose isolated live-message smoke files")
                    print(json.dumps({"discard": discarded}), flush=True)
                    assert worktree is not None and not worktree.exists()
                    print("cancel/discard verified: disposable worktree removed", flush=True)
            finally:
                # Exact state-home-derived Linux socket, or exact private Mac
                # socket: never a default server or another run's manager unit.
                subprocess.run([tmux, "-N", "-S", str(socket), "kill-server"],
                               env=env, capture_output=True, timeout=10)
                socket.unlink(missing_ok=True)
                if linux:
                    socket.with_suffix(".lock").unlink(missing_ok=True)


if __name__ == "__main__":
    main()
