#!/bin/sh
# Checked, bounded real-agent smoke of a named resident worker (ADR 0003):
# round 1, told round report, the worker's own exit, `run land`, a source
# advance, `run message` (rebase + resume of the same native session), round 2,
# and the parent's final `run merge`. Continuity is proven by a code word the
# worker only ever saw in round 1's prompt.
#
# Usage: scripts/resident-smoke.sh pi|claude
# Optional: RESIDENT_SMOKE_CLAUDE_MODEL (default claude-haiku-4-5-20251001),
#           RESIDENT_SMOKE_PI_ARGS (extra Pi argv words, e.g. "--model x").
#
# Everything mutable lives in one disposable root under ${TMPDIR:-/var/tmp}: the
# repository, the Taskfleet home, and therefore its state-keyed tmux service.
# The release binary is placed first on the worker's PATH as `taskfleet`; the
# user's installed taskfleet and skills are never touched. The agents run with
# the user's own credentials; the session-store entries they create for this
# root under ~/.claude/projects are removed on success.
set -eu

harness=${1:-}
case "$harness" in pi|claude) ;; *) echo "usage: $0 pi|claude" >&2; exit 2 ;; esac
binary=${TASKFLEET_SMOKE_BIN:-"$(pwd)/target/release/taskfleet"}
case "$binary" in /*) ;; *) binary="$(pwd)/$binary" ;; esac
[ -x "$binary" ] || { echo "build first: cargo build --release -p taskfleet" >&2; exit 2; }
for tool in git tmux workmux python3 systemctl "$harness"; do
  command -v "$tool" >/dev/null || { echo "missing prerequisite: $tool" >&2; exit 2; }
done

root=$(mktemp -d "${TMPDIR:-/var/tmp}/tf-resident-smoke.XXXXXXXX")
home="$root/home"
repo="$root/repo"
run_id=
name="smoke-$harness"
key=

tf() { (cd "$repo" && TASKFLEET_HOME="$home" PATH="$root/bin:$PATH" "$binary" "$@"); }
field() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1], {'d': d}))" "$1"; }
fail() { echo "resident smoke ($harness): $*" >&2; exit 1; }

cleanup() {
  status=$?
  trap - EXIT HUP INT TERM
  if [ -n "$run_id" ] && [ -d "$home/runs/$run_id" ]; then
    tf --output json run cancel "$run_id" >/dev/null 2>&1 || true
    sleep 2
  fi
  if [ -d "$home/runs" ]; then
    find "$home/runs" -name supervisor.pid -type f -exec sh -c '
      for f do
        p=$(sed -n "s/ .*//p" "$f")
        [ -n "$p" ] && kill "$p" 2>/dev/null || true
      done' sh {} +
  fi
  if [ -n "$key" ]; then
    for unit in $(systemctl --user list-units --plain --no-legend --all "taskfleet-tmux-$key-*" | awk '{print $1}'); do
      systemctl --user stop "$unit" >/dev/null 2>&1 || true
    done
  fi
  if [ "$status" -eq 0 ]; then
    # Claude (and Pi's Claude bridge) record sessions under ~/.claude/projects
    # keyed by the escaped working directory; remove exactly this root's entries.
    python3 - "$root" <<'PY'
import os, shutil, sys
prefix = sys.argv[1].replace("/", "-").replace(".", "-")
base = os.path.expanduser("~/.claude/projects")
for name in (os.listdir(base) if os.path.isdir(base) else []):
    if name.startswith(prefix + "-"):
        shutil.rmtree(os.path.join(base, name))
PY
    rm -rf "$root"
  else
    echo "resident smoke ($harness) failed; evidence kept in $root" >&2
  fi
  exit "$status"
}
trap cleanup EXIT HUP INT TERM

mkdir -p "$root/bin" "$home"
ln -s "$binary" "$root/bin/taskfleet"
git init -q -b main "$repo"
git -C "$repo" config user.name 'Resident Smoke'
git -C "$repo" config user.email smoke@example.invalid
printf 'history/\n' > "$repo/.gitignore"
git -C "$repo" add .gitignore
git -C "$repo" commit -qm base
key=$(printf '%s' "$home" | python3 -c 'import hashlib,sys; print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest()[:20])')

if [ "$harness" = pi ]; then
  agent="\"$(command -v pi)\"${RESIDENT_SMOKE_PI_ARGS:+, $(printf '%s' "$RESIDENT_SMOKE_PI_ARGS" | python3 -c 'import json,sys; print(", ".join(json.dumps(w) for w in sys.stdin.read().split()))')}"
  agents="[{ harness = \"pi\", command = [$agent], telemetry = \"worker-v1\" }]"
else
  model=${RESIDENT_SMOKE_CLAUDE_MODEL:-claude-haiku-4-5-20251001}
  agents="[{ harness = \"claude\", command = [\"$(command -v claude)\", \"--model\", \"$model\", \"--dangerously-skip-permissions\"] }]"
fi
cat > "$home/config.toml" <<EOF
[profiles.smoke]
description = "bounded resident-worker smoke"
capability = "fast"
residency = "local"
agents = $agents
[profile]
default = "smoke"
EOF

cat > "$root/round1.md" <<'EOF'
This is a bounded smoke test in a disposable repository. Keep every answer short.
Remember this code word: PELICAN-42. Keep it out of every file, commit, and
this round's summary; a later round will ask you to state it in that round's
summary, and then you should.

ROUND.txt is this task's only artifact: create no other file in the
repository (write round summaries under /tmp).

Round 1: create the file ROUND.txt containing the single line `one`, commit it
with the message `round one`, then end the round exactly as your run context
describes, with the summary `round one done`. Then stop.
EOF

created=$(tf --output json run create --kind spinoff --headless --resident --name "$name" \
  --title "resident smoke $harness" --prompt-file "$root/round1.md" --source-branch main)
run_id=$(printf '%s' "$created" | field 'd["data"]["run_id"]')
events="$home/runs/$run_id/events.jsonl"
echo "resident smoke ($harness): run $run_id"

wait_round() {
  out=$(tf --output json run wait "$name" --timeout 8m) || fail "run wait failed: $out"
  [ "$(printf '%s' "$out" | field 'd["data"]["runs"][0].get("round_reported", False)')" = True ] ||
    fail "round not reported: $out"
  printf '%s' "$out" | field 'd["data"]["runs"][0]["resident"]["last_round"]["summary"]'
}

await_exit() {
  i=0
  while [ "$i" -lt 600 ]; do
    count=$(grep -c '"kind":"worker.exited"' "$events" || true)
    [ "$count" -ge "$1" ] && return 0
    i=$((i + 1)); sleep 0.5
  done
  fail "worker attempt did not exit after its round report"
}

summary1=$(wait_round)
echo "round 1 summary: $summary1"
await_exit 1
case "$summary1" in *PELICAN*) fail "code word leaked into round 1" ;; esac

landed=$(tf --output json run land "$name")
[ "$(printf '%s' "$landed" | field 'd["data"]["landed"]')" = True ] || fail "land: $landed"
[ "$(git -C "$repo" show main:ROUND.txt)" = one ] || fail "round 1 work is not on main"

printf 'source advanced while the worker slept\n' > "$repo/SOURCE.txt"
git -C "$repo" add SOURCE.txt
git -C "$repo" commit -qm "source advance"

msg=$(tf --output json run message "$name" "Round 2: append the line two to ROUND.txt and commit it. In this round's summary state the code word I gave you in round 1 and the exact content of SOURCE.txt. Then end the round as before and stop.")
[ "$(printf '%s' "$msg" | field 'd["data"]["delivery"]')" = resumed ] || fail "message: $msg"
[ "$(printf '%s' "$msg" | field 'd["data"]["resumed"]["rebase"]["state"]')" = rebased ] || fail "no rebase: $msg"
summary2=$(wait_round)
echo "round 2 summary: $summary2"
await_exit 2
case "$summary2" in *PELICAN-42*) ;; *) fail "session continuity not shown (no code word)" ;; esac
case "$summary2" in *"source advanced"*) ;; *) fail "rebased source not seen by the worker" ;; esac

if [ "$harness" = pi ]; then
  node="$home/runs/$run_id/nodes/n-0001.json"
  python3 - "$node" <<'PY'
import json,sys
n=json.load(open(sys.argv[1]))
assert n["retry_attempts"] == 1 and n["evidence"]["attempt"] == 1, n
PY
fi

merged=$(tf --output json run merge "$name")
[ "$(printf '%s' "$merged" | field 'd["data"]["merged"]')" = True ] || fail "merge: $merged"
i=0
while [ "$i" -lt 120 ]; do
  grep -q '"kind":"supervisor.exited"' "$events" && break
  i=$((i + 1)); sleep 0.5
done
shown=$(tf --output json run show "$run_id")
[ "$(printf '%s' "$shown" | field 'd["data"]["status"]')" = done ] || fail "not done: $shown"
[ "$(git -C "$repo" show main:ROUND.txt | tr -s '\n' ' ' | sed 's/ *$//')" = "one two" ] ||
  fail "round 2 work is not on main"
if [ "$harness" = pi ]; then
  [ "$(printf '%s' "$shown" | field 'd["data"]["evidence"]["status"]')" = complete ] || fail "evidence: $shown"
fi
run_id=
echo "resident smoke ($harness): OK"
