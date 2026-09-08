#!/bin/sh
# Checked, bounded real-pi smoke test for Taskfleet's native materializer.
# Everything mutable lives in one disposable root and one private tmux socket.
set -eu

binary=${TASKFLEET_SMOKE_BIN:-"$(pwd)/target/release/taskfleet"}
case "$binary" in /*) ;; *) binary="$(pwd)/$binary" ;; esac
[ -x "$binary" ] || { echo "build first: cargo build --release -p taskfleet" >&2; exit 2; }
command -v git >/dev/null
command -v tmux >/dev/null
command -v workmux >/dev/null
command -v pi >/dev/null
command -v python3 >/dev/null

pi_agent_dir=${PI_CODING_AGENT_DIR:-"$HOME/.pi/agent"}
[ -d "$pi_agent_dir" ] || { echo "Pi agent directory unavailable: $pi_agent_dir" >&2; exit 2; }
source_repo=$(git rev-parse --show-toplevel)
root=$(mktemp -d "${TMPDIR:-/tmp}/taskfleet-native-smoke.XXXXXXXX")
token=$(basename "$root" | tr -cd '[:alnum:]')
socket="taskfleet-smoke-$token"
session="smoke-$token"
run_id=
cleaned=0

inventory() {
  out=$1
  {
    echo '# source worktrees'
    git -C "$source_repo" worktree list --porcelain
    echo '# source fixture refs'
    git -C "$source_repo" for-each-ref --format='%(refname)' 'refs/heads/wt/*'
    echo '# default tmux windows rooted in this source checkout'
    tmux list-windows -a -F '#{socket_path}\t#{session_name}\t#{window_id}\t#{pane_current_path}' 2>/dev/null |
      awk -v repo="$source_repo" 'index($0, repo)' || true
  } > "$out"
}

cleanup() {
  status=$?
  trap - EXIT HUP INT TERM
  if [ -n "$run_id" ] && [ -d "$root/home/runs/$run_id" ]; then
    HOME="$root/user" TASKFLEET_HOME="$root/home" TMUX_BIN="$root/bin/tmux" \
      "$binary" --output json run cancel "$run_id" >/dev/null 2>&1 || true
  fi
  # The private server is the final process/window containment boundary. tmux
  # can leave a stale socket inode after kill-server, so remove that exact
  # token-bound socket only after the private server has stopped.
  tmux -L "$socket" kill-server >/dev/null 2>&1 || true
  if [ -n "${socket_path:-}" ]; then
    case "$socket_path" in
      */tmux-$(id -u)/taskfleet-smoke-$token)
        tmux -S "$socket_path" has-session >/dev/null 2>&1 || rm -f "$socket_path"
        ;;
      *) echo "refusing unexpected smoke socket path: $socket_path" >&2; status=1 ;;
    esac
  fi
  if [ -d "$root/home/runs" ]; then
    find "$root/home/runs" -name supervisor.pid -type f -exec sh -c '
      for f do
        p=$(sed -n "s/ .*//p" "$f")
        if [ -n "$p" ]; then
          kill "$p" 2>/dev/null || true
          i=0
          while kill -0 "$p" 2>/dev/null && [ "$i" -lt 50 ]; do i=$((i + 1)); sleep 0.02; done
        fi
      done
    ' sh {} +
  fi
  if [ -n "${socket_path:-}" ] && [ -e "$socket_path" ]; then
    echo "ERROR: private smoke socket survived cleanup: $socket_path" >&2
    status=1
  fi
  inventory "$root/after"
  if ! cmp -s "$root/before" "$root/after"; then
    echo 'ERROR: native smoke changed resources outside its sandbox:' >&2
    diff -u "$root/before" "$root/after" >&2 || true
    status=1
  fi
  cleaned=1
  rm -rf "$root"
  exit "$status"
}
trap cleanup EXIT HUP INT TERM

inventory "$root/before"
mkdir -p "$root/bin" "$root/home" "$root/user"
git init -q -b main "$root/repo"
git -C "$root/repo" -c user.name='Taskfleet Smoke' -c user.email=smoke@example.invalid \
  commit --allow-empty -qm base

tmux -L "$socket" new-session -d -s "$session" -c "$root/repo"
server_pid=$(tmux -L "$socket" display-message -p -t "$session" '#{pid}')
socket_path=$(tmux -L "$socket" display-message -p -t "$session" '#{socket_path}')
cat > "$root/bin/tmux" <<EOF
#!/bin/sh
exec tmux -L '$socket' "\$@"
EOF
chmod 755 "$root/bin/tmux"

pi_path=$(command -v pi)
cat > "$root/home/config.toml" <<EOF
[profiles.smoke]
description = "bounded real-pi native spawn smoke"
capability = "fast"
residency = "local"
agents = [{ harness = "pi", command = ["$pi_path", "-p", "--no-tools"], telemetry = "worker-v1" }]
[profile]
default = "smoke"
EOF
cat > "$root/prompt.md" <<'EOF'
This is a bounded smoke test in a disposable repository. Do not invoke tools or
modify files. Reply with exactly SMOKE_OK, then exit.
EOF

created=$(cd "$root/repo" && \
  HOME="$root/user" PI_CODING_AGENT_DIR="$pi_agent_dir" \
  TASKFLEET_HOME="$root/home" TMUX="$socket_path,$server_pid,0" \
  TMUX_BIN="$root/bin/tmux" \
  "$binary" --output json run create --kind spinoff --tmux-session "$session" \
    --title native-pi-smoke --prompt-file "$root/prompt.md" --agent-startup-timeout 30)
run_id=$(printf '%s' "$created" | python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["run_id"])')
worktree=$(printf '%s' "$created" | python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["worktree_path"])')
branch=$(printf '%s' "$created" | python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["branch"])')

# Native materialization deliberately archives its generated prompt as an
# untracked worktree diagnostic. In this disposable smoke only, prove that it is
# the sole change and remove that exact fixture-owned file before cancellation;
# production cleanup must continue preserving arbitrary dirty work.
archive="history/.worktree/$branch.md"
status=$(git -C "$worktree" status --porcelain --untracked-files=all)
[ "$status" = "?? $archive" ] || {
  echo "unexpected real-pi smoke worktree changes: $status" >&2
  exit 1
}
rm -f "$worktree/$archive"

# Let native Pi finish and let the launcher durably record writer quiescence.
# A terminal report can otherwise race Pi's final transcript append.
i=0
while [ "$i" -lt 600 ]; do
  grep -q '"kind":"worker.exited"' "$root/home/runs/$run_id/events.jsonl" && break
  i=$((i + 1))
  sleep 0.1
done
[ "$i" -lt 600 ] || { echo 'native Pi did not exit within 60 seconds' >&2; exit 1; }
worker_code=$(python3 - "$root/home/runs/$run_id/events.jsonl" <<'PY'
import json,sys
exits=[json.loads(line)["data"] for line in open(sys.argv[1])
       if json.loads(line).get("kind") == "worker.exited"]
assert exits, "worker exit event absent"
print(exits[-1].get("exit_code", ""))
PY
)
[ "$worker_code" = "0" ] || { echo "native Pi exited unsuccessfully: $worker_code" >&2; exit 1; }

# Creation plus the private PID/pane identity is the smoke assertion. Cancellation
# deliberately exercises non-merge cleanup of a now-clean disposable worktree.
HOME="$root/user" TASKFLEET_HOME="$root/home" TMUX_BIN="$root/bin/tmux" \
  "$binary" --output json run cancel "$run_id" >/dev/null

# Give the supervisor a bounded opportunity to capture exact terminal evidence
# and finish its own cleanup. The trap remains the backstop on timeout, child
# failure, interruption, and assertion.
i=0
while [ "$i" -lt 100 ]; do
  live=0
  if [ -f "$root/home/runs/$run_id/supervisor.pid" ]; then
    pid=$(sed -n 's/ .*//p' "$root/home/runs/$run_id/supervisor.pid")
    kill -0 "$pid" 2>/dev/null && live=1
  fi
  [ "$live" -eq 0 ] && [ ! -e "$worktree" ] && break
  i=$((i + 1))
  sleep 0.05
done
if [ "$i" -ge 100 ]; then
  echo 'smoke resources did not clean up within 5 seconds' >&2
  HOME="$root/user" TASKFLEET_HOME="$root/home" "$binary" --output json run show "$run_id" >&2 || true
  git -C "$worktree" status --porcelain --untracked-files=all >&2 2>/dev/null || true
  exit 1
fi

shown=$(HOME="$root/user" TASKFLEET_HOME="$root/home" TMUX_BIN="$root/bin/tmux" \
  "$binary" --output json run show "$run_id")
evidence_values=$(printf '%s' "$shown" | python3 -c '
import json,sys
v=json.load(sys.stdin)["data"]["evidence"]
assert v["status"] == "complete", v
assert v["session_id"] and v["original_cwd"]
print(v["transcript_path"])
print(v["resume_path"])
print(v["pane_path"])
print(v["report_path"])
print(v["transcript_sha256"])
print(v["live_session_path"])
')
transcript_rel=$(printf '%s\n' "$evidence_values" | sed -n '1p')
resume_rel=$(printf '%s\n' "$evidence_values" | sed -n '2p')
pane_rel=$(printf '%s\n' "$evidence_values" | sed -n '3p')
report_rel=$(printf '%s\n' "$evidence_values" | sed -n '4p')
recorded_sha=$(printf '%s\n' "$evidence_values" | sed -n '5p')
live_rel=$(printf '%s\n' "$evidence_values" | sed -n '6p')
run_dir="$root/home/runs/$run_id"
[ ! -e "$root/home/$live_rel" ] || { echo 'private live transcript survived archive' >&2; exit 1; }
if tmux -L "$socket" list-panes -a -F '#{pane_current_path}' 2>/dev/null | grep -Fx "$worktree" >/dev/null; then
  echo 'worker pane survived evidence cleanup' >&2
  exit 1
fi
for rel in "$transcript_rel" "$resume_rel" "$pane_rel" "$report_rel"; do
  [ -f "$run_dir/$rel" ] || { echo "missing durable evidence artifact: $rel" >&2; exit 1; }
done
actual_sha=$(shasum -a 256 "$run_dir/$transcript_rel" | awk '{print $1}')
[ "$actual_sha" = "$recorded_sha" ] || { echo 'transcript digest mismatch' >&2; exit 1; }
python3 - "$run_dir/$transcript_rel" "$run_dir/$resume_rel" "$worktree" "$root/repo" <<'PY'
import json,os,sys
original=open(sys.argv[1],"rb").read()
resume=open(sys.argv[2],"rb").read()
ohead,otail=original.split(b"\n",1)
rhead,rtail=resume.split(b"\n",1)
assert otail == rtail, "resume copy changed transcript history"
assert os.path.realpath(json.loads(ohead)["cwd"]) == os.path.realpath(sys.argv[3])
assert os.path.realpath(json.loads(rhead)["cwd"]) == os.path.realpath(sys.argv[4])
PY

# Open the archived conversation through Pi's supported explicit-session CLI.
# The second harmless, no-tools request proves the rewritten cwd remains usable
# after the worker worktree has been removed. It mutates only the separate
# resume copy; the byte-identical original archive must remain unchanged.
resume_out=$(python3 - "$pi_path" "$run_dir/$resume_rel" "$root" "$pi_agent_dir" <<'PY'
import os,subprocess,sys
env=os.environ.copy()
env["HOME"]=sys.argv[3]+"/user"
env["PI_CODING_AGENT_DIR"]=sys.argv[4]
try:
    result=subprocess.run(
        [sys.argv[1],"--session",sys.argv[2],"--no-tools","-p",
         "Use no tools. Reply with exactly the token from your immediately preceding assistant message, and nothing else."],
        cwd=sys.argv[3],env=env,text=True,capture_output=True,timeout=60)
except (subprocess.SubprocessError,OSError) as error:
    raise SystemExit(f"Pi resume canary could not execute: {error}")
if result.returncode:
    raise SystemExit(
        f"Pi resume canary exited {result.returncode}; stdout={result.stdout!r}; stderr={result.stderr!r}")
print(result.stdout,end="")
PY
)
printf '%s' "$resume_out" | grep -q 'SMOKE_OK' || {
  echo "Pi resume canary returned unexpected output: $resume_out" >&2
  exit 1
}
post_resume_sha=$(shasum -a 256 "$run_dir/$transcript_rel" | awk '{print $1}')
[ "$post_resume_sha" = "$recorded_sha" ] || { echo 'resume mutated original archive' >&2; exit 1; }

echo "native real-pi evidence/resume smoke passed in private socket $socket"
