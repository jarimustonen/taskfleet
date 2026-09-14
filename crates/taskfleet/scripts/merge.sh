#!/bin/bash
# merge.sh — merge a worktree branch back to its target branch.
#
# BUNDLED COPY. This script is embedded into the `taskfleet` binary
# (see crates/taskfleet/src/run/merge.rs) and materialized to a temp file at
# runtime by `taskfleet run merge`. It is the v1 merge-mechanics
# backend: it owns the rebase, the portable mkdir lock that serializes
# concurrent merges, the workmux merge, and the post-merge teardown of the
# worktree + tmux window + branch. `run merge` wraps it, then submits the terminal
# `node.report` so the supervisor can wind the run down.
#
# It descends from the now-sunset external worktree-merge skill.
# Keep the cleanup subshell: it is the proven path that handles
# the lingering-cwd race (a shell still parked inside the worktree) via
# detach + sleep + `--force`. The per-run supervisor also runs a best-effort
# cleanup on the terminal report; the two race and either finishing first
# leaves the other a clean no-op (see supervise/cleanup.rs).
#
# Usage: merge.sh [--target <branch>] [branch-name]
#   If no branch name given, uses the current branch.
#   If no --target given, the target is the main/master worktree.

set -euo pipefail

TARGET_BRANCH=""
# The OID the caller (`run merge`) recorded for the target branch BEFORE the
# merge — the compare half of the compare-and-swap (design.md §2.1b / A2). When
# set, we verify the target branch is still exactly this OID after taking the
# merge lock and refuse (exit 76) if it moved, so the source ref only advances
# when the recorded transaction still applies.
EXPECTED_SOURCE_OID=""
# PID of the `run merge` driver process that spawned us. We are its child but can
# be orphaned if it is killed (SIGKILL/OOM/tmux teardown) mid-merge. Recovery
# gates on the driver being dead, so if the driver died we must NOT fast-forward
# the source ref (recovery would already have classified this as "not landed").
# Checked immediately before the mutation (design.md §2.1b / A2).
DRIVER_PID=""
POSITIONAL=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --target)
            TARGET_BRANCH="${2:-}"
            shift 2 || { echo "Error: --target requires a value" >&2; exit 1; }
            ;;
        --target=*)
            TARGET_BRANCH="${1#--target=}"
            shift
            ;;
        --expected-source-oid)
            EXPECTED_SOURCE_OID="${2:-}"
            shift 2 || { echo "Error: --expected-source-oid requires a value" >&2; exit 1; }
            ;;
        --expected-source-oid=*)
            EXPECTED_SOURCE_OID="${1#--expected-source-oid=}"
            shift
            ;;
        --driver-pid)
            DRIVER_PID="${2:-}"
            shift 2 || { echo "Error: --driver-pid requires a value" >&2; exit 1; }
            ;;
        --driver-pid=*)
            DRIVER_PID="${1#--driver-pid=}"
            shift
            ;;
        *)
            POSITIONAL+=("$1")
            shift
            ;;
    esac
done

# Get branch name (argument or current)
if [[ -n "${POSITIONAL[0]:-}" ]]; then
    BRANCH="${POSITIONAL[0]}"
else
    BRANCH=$(git branch --show-current)
fi

echo "Merge worktree: $BRANCH"

# Check not on main
if [[ "$BRANCH" == "main" ]] || [[ "$BRANCH" == "master" ]]; then
    echo "Error: Cannot merge from main/master branch" >&2
    exit 1
fi

# Resolve the target branch + the worktree it is checked out in.
if [[ -n "$TARGET_BRANCH" ]]; then
    # Explicit target: find the worktree that has this branch checked out.
    TARGET_PATH=$(git worktree list | grep -F "[$TARGET_BRANCH]" | awk '{print $1}' | head -n1)
    if [[ -z "$TARGET_PATH" ]]; then
        echo "Error: target branch '$TARGET_BRANCH' is not checked out in any worktree" >&2
        echo "       /orchestrate keeps its own worktree alive as the merge parent; ensure it still exists." >&2
        exit 1
    fi
else
    # Legacy: detect the main/master worktree.
    TARGET_PATH=$(git worktree list | grep -E '\[main\]|\[master\]' | awk '{print $1}' | head -n1)
    if [[ -z "$TARGET_PATH" ]]; then
        echo "Error: Could not find main worktree" >&2
        exit 1
    fi
    TARGET_BRANCH=$(git -C "$TARGET_PATH" branch --show-current)
fi

if [[ "$BRANCH" == "$TARGET_BRANCH" ]]; then
    echo "Error: refusing to merge '$BRANCH' into itself" >&2
    exit 1
fi

echo "Merge target: $TARGET_BRANCH ($TARGET_PATH)"
echo ""

# Check for uncommitted changes in OUR OWN worktree. This is safe to test
# before taking the merge lock: it is the agent's own source worktree, not the
# shared target, so it is never touched by a concurrent merge.
#
# Capture the status into a variable rather than testing $(...) inline: a
# `git status` FAILURE (e.g. the worktree vanished) inside `[[ -n "$(...)" ]]`
# yields an empty string and would be silently read as "clean" — `set -e` does
# not fire on a substitution nested in a test. Fail loud instead.
if ! SOURCE_STATUS=$(git status --porcelain --untracked-files=all); then
    echo "Error: could not inspect worktree status" >&2
    exit 1
fi
if [[ -n "$SOURCE_STATUS" ]]; then
    echo "Error: Uncommitted changes in worktree" >&2
    echo "Please commit first using /git-commit" >&2
    exit 1
fi

echo "Worktree status: clean"
echo ""

# Serialize concurrent merge runs against the same repo BEFORE inspecting the
# target worktree — the target-cleanliness check MUST live inside this critical
# section. While another merge holds this lock it is mid-rebase and the target
# worktree is transiently dirty; checking BEFORE the lock let a concurrent merge
# observe that in-flight state and fail with a spurious "uncommitted changes in
# target" (issue concurrent-self-merge-race). Inside the lock the only merge
# touching the target is ours, so a dirty target there is genuine user work that
# must still block.
#
# Without the lock, two parallel rebases would also race on the FF step and one
# would fail. Use the shared common git dir so the lock works from linked
# worktrees too (a linked worktree's .git is a file, not a directory).
#
# The mutex is an atomic `mkdir` lock, NOT `flock`: `flock(1)` ships with
# util-linux and is ABSENT on stock macOS (present only via a keg-only homebrew
# util-linux), and macOS is the primary platform. `mkdir` is atomic on
# POSIX/HFS+/APFS — exactly one racer wins the create; the losers spin with a
# short sleep until they win it or the timeout elapses (issue
# merge-lock-flock-not-portable-macos). No external binary required.
GIT_COMMON_DIR=$(git rev-parse --git-common-dir)
LOCK_DIR="$GIT_COMMON_DIR/worktree-merge.lock"
LOCK_TIMEOUT="${MERGE_LOCK_TIMEOUT:-600}"
# Validate the timeout: a non-numeric / non-positive value would make the
# acquire loop below behave nonsensically and could be misread downstream as a
# lock-contention failure (`merge_in_progress`). The upper bound guards the
# `SECONDS + LOCK_TIMEOUT` deadline arithmetic below from overflowing bash's
# signed integers (a huge value would wrap negative and fire the timeout
# immediately). 86400s (a full day) is far past any legitimate merge wait.
# Reject a bad value up front with a plain error instead.
# The `{1,5}` digit cap (max 99999) keeps the value well inside signed-integer
# range so neither the comparisons here nor the deadline arithmetic can overflow.
if ! [[ "$LOCK_TIMEOUT" =~ ^[0-9]{1,5}$ ]] || [[ "$LOCK_TIMEOUT" -lt 1 ]] || [[ "$LOCK_TIMEOUT" -gt 86400 ]]; then
    echo "Error: MERGE_LOCK_TIMEOUT must be an integer between 1 and 86400 (got '$LOCK_TIMEOUT')" >&2
    exit 1
fi

# Release the lock on ANY exit once we own it. The lock is the directory itself,
# so removal releases it — no fd to leak into workmux descendants (the old
# `flock` fd-inheritance deadlock is gone). Guarded by LOCK_ACQUIRED so a
# timeout exit (which never owns the dir) can't remove a peer's live lock. The
# INT/TERM/HUP traps route a signal death through the same cleanup (each re-exits
# so the EXIT trap fires), so a Ctrl-C or `kill` between acquire and release does
# not strand the lock dir — important because there is deliberately NO automatic
# stale-lock reclaim (see the acquire loop). Only an uncatchable SIGKILL / crash
# can leave a stale dir behind, bounded by the timeout below.
LOCK_ACQUIRED=0
release_merge_lock() {
    if [[ "$LOCK_ACQUIRED" -eq 1 ]]; then
        rm -rf "$LOCK_DIR" 2>/dev/null || true
    fi
}
trap release_merge_lock EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

# Exit 75 (EX_TEMPFAIL) is RESERVED for the lock-timeout case below and mapped
# to `merge_in_progress` by `run merge`. It is the sole producer of that status
# (the `workmux` invocation later normalizes its exit so a downstream 75 can't
# leak).
#
# A `mkdir` that fails because the dir already exists is genuine contention — a
# peer holds the lock — so we wait. A `mkdir` that fails for ANY OTHER reason
# (permission denied, read-only `.git`, a non-directory sitting at the path) is a
# hard error that must fail as `merge_failed`, NOT spin for the whole timeout and
# masquerade as `merge_in_progress`; we detect that as "mkdir failed AND the dir
# is still absent" and exit 1 immediately.
#
# There is deliberately NO pid-based stale-lock reclaim: a shell has no atomic
# check-and-delete for a directory, so any "read holder pid → decide dead →
# rename aside" sequence races a successor that acquires the path in between and
# can rip a LIVE lock out from under it, letting two merges run concurrently.
# Mutual exclusion matters more than crash auto-recovery here, so a crashed
# holder's lock is instead bounded by the timeout, with the operator told how to
# clear it. The signal traps above keep such orphans to SIGKILL/crash only.
LOCK_DEADLINE=$(( SECONDS + LOCK_TIMEOUT ))
while true; do
    if mkdir "$LOCK_DIR" 2>/dev/null; then
        LOCK_ACQUIRED=1
        break
    fi
    if [[ ! -d "$LOCK_DIR" ]]; then
        # Not an "already exists" failure. Retry once in case the holder released
        # between our mkdir and this test; if the dir still isn't there, mkdir
        # failed for a real reason — fail hard rather than loop to the timeout.
        if mkdir "$LOCK_DIR" 2>/dev/null; then
            LOCK_ACQUIRED=1
            break
        fi
        if [[ ! -d "$LOCK_DIR" ]]; then
            echo "Error: could not create the merge lock directory at $LOCK_DIR" >&2
            exit 1
        fi
    fi
    if (( SECONDS >= LOCK_DEADLINE )); then
        # A concurrent merge into this target held the lock longer than we
        # waited. This is a serialization timeout, NOT a dirty tree: exit 75 so
        # `run merge` surfaces a distinct, retryable `merge_in_progress` error
        # rather than the misleading dirty-target failure this race used to
        # produce.
        echo "Error: another merge is holding the target branch '$TARGET_BRANCH'; could not acquire the merge lock at $LOCK_DIR within ${LOCK_TIMEOUT}s (if no merge is running, the lock may be stale from a crashed merge — remove $LOCK_DIR and retry)" >&2
        exit 75
    fi
    sleep 0.1
done
echo "Acquired merge lock"
echo ""

# Now that we hold the lock, no COOPERATING merge (one that also takes this lock)
# is touching the target, so the transient mid-rebase dirt of a concurrent
# self-merge can no longer be observed here. A dirty target at this point is
# therefore genuine uncommitted work — whether a human's edit or a non-merge
# writer — and must still block (this is the real safety check, replacing the
# racy pre-lock one that produced the false positive). As above, capture the
# status so a `git status` failure can't be misread as "clean".
if ! TARGET_STATUS=$(git -C "$TARGET_PATH" status --porcelain --untracked-files=all); then
    echo "Error: could not inspect target worktree status ($TARGET_PATH)" >&2
    exit 1
fi
if [[ -n "$TARGET_STATUS" ]]; then
    echo "Error: Uncommitted changes in target worktree ($TARGET_PATH)" >&2
    echo "Please commit or stash changes in the target before merging" >&2
    exit 1
fi

echo "Target status: clean"
echo ""

# Compare-and-swap guard (design.md §2.1b / A2). Now that we hold the merge lock
# (so no cooperating merge can move the target between this check and the FF
# below), verify the target branch is still exactly the OID `run merge` recorded
# before it opened the transaction. If it moved, the recorded transaction no
# longer applies — a concurrent/non-cooperating writer advanced the target — so
# refuse the merge (exit 76) rather than fast-forwarding onto an unexpected base.
# The agent recovers by rebasing and re-running `run merge` (which records a fresh
# expected OID). Exit 76 is distinct from the dirty-tree/conflict failure (1) and
# the lock-timeout retry (75) so `run merge` can classify it precisely.
if [[ -n "$EXPECTED_SOURCE_OID" ]]; then
    # `--end-of-options` so a target branch legitimately starting with `-` is never
    # misparsed as a flag (mirrors the Rust `read_oid` hardening; /llm-review).
    if ! ACTUAL_SOURCE_OID=$(git -C "$TARGET_PATH" rev-parse --verify --end-of-options "$TARGET_BRANCH"); then
        echo "Error: could not resolve target branch '$TARGET_BRANCH' for the compare-and-swap check" >&2
        exit 1
    fi
    if [[ "$ACTUAL_SOURCE_OID" != "$EXPECTED_SOURCE_OID" ]]; then
        echo "Error: target branch '$TARGET_BRANCH' moved from the expected $EXPECTED_SOURCE_OID to $ACTUAL_SOURCE_OID since the merge started; refusing to fast-forward onto an unexpected base — rebase and re-run the merge" >&2
        exit 76
    fi
    echo "Compare-and-swap: target at expected $EXPECTED_SOURCE_OID"
    echo ""
fi

# Gather commits
COMMITS=$(git log --oneline "${TARGET_BRANCH}..HEAD")
COMMIT_COUNT=$(echo "$COMMITS" | grep -c . || echo "0")

echo "Commits to merge ($COMMIT_COUNT):"
echo "$COMMITS"
echo ""

# Orphan-race guard (design.md §2.1b / A2, /llm-review). We are the driver's
# child; if the driver was killed we may have been orphaned. Recovery gates on the
# driver being dead — so if the driver is gone, it will (or already did) classify
# this transaction against the CURRENT source ref. Fast-forwarding now, after the
# driver died, would move the source out from under a recovery that already read it
# unmoved and rejected the transaction — stranding the merged work with no report.
# So bail before the mutation if the driver is no longer alive. This shrinks the
# orphan window from the whole merge to the gap between this check and the ref
# update; a durable operation lease (0.2.1) would close it entirely.
if [[ -n "$DRIVER_PID" ]] && ! kill -0 "$DRIVER_PID" 2>/dev/null; then
    echo "Error: the run merge driver process ($DRIVER_PID) is no longer alive; aborting before mutating the source ref so crash recovery stays deterministic" >&2
    exit 1
fi

echo "Running workmux merge --rebase --into $TARGET_BRANCH..."

# Use --keep so workmux doesn't try (and fail) to kill our own window.
# --into pins the merge target (defaults to config main_branch when omitted).
#
# The lock is a directory removed by our EXIT trap, so there is no lock fd for a
# workmux descendant to inherit and hold past our exit — the old `flock`
# fd-inheritance deadlock cannot occur.
#
# Capture the status instead of letting `set -e` propagate it: a downstream exit
# 75 (from workmux or git) would otherwise leak out as the script's status and
# be misclassified as the lock-timeout `merge_in_progress`. Any workmux failure
# is a genuine merge failure — normalize it to exit 1 (`merge_failed`).
merge_rc=0
workmux merge --rebase --keep --into "$TARGET_BRANCH" || merge_rc=$?
if [[ "$merge_rc" -ne 0 ]]; then
    echo "Error: workmux merge failed (exit $merge_rc)" >&2
    exit 1
fi

echo ""
echo "Merge complete!"
echo "  Branch: $BRANCH"
echo "  Target: $TARGET_BRANCH"
echo "  Commits merged: $COMMIT_COUNT"

# Tmux window teardown is now the supervisor's responsibility — it can find
# the window rename-proof via worktree-path (session-scoped, exact match),
# while merge.sh only saw $TMUX_PANE and would target the WRONG window when
# a retry came from a different pane after a manual rebase resolution
# (issue: merge-sh-tmux-pane-recovery). The supervisor's cleanup also
# handles `git worktree remove --force` and `git branch -D`, so merge.sh
# no longer races it on those either — the merge call here advances the
# run to terminal, and the supervisor's next tick tears down everything.
