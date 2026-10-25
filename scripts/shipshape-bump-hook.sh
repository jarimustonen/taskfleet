#!/usr/bin/env bash
# Deterministic project hook for shipshape's engine-owned version bump.
# Runs inside shipshape's clean release checkout after manifest/pin/lock/changelog edits.
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

snapshot_dir="crates/taskfleet/tests/snapshots"
snapshots=(
  "$snapshot_dir/envelope_snapshots__version_json.snap"
  "$snapshot_dir/envelope_snapshots__version_jsonl.snap"
  "$snapshot_dir/envelope_snapshots__version_text.snap"
  "$snapshot_dir/envelope_snapshots__skill_list_json.snap"
  "$snapshot_dir/envelope_snapshots__skill_list_jsonl.snap"
)

state_dir="$(mktemp -d "${TMPDIR:-/tmp}/shipshape-bump-hook.XXXXXX")"
cleanup() { rm -rf "$state_dir"; }
trap cleanup EXIT

# Keep the pre-regeneration bytes as proof that every expected fixture was
# actually rewritten by this invocation, rather than merely being different
# from HEAD because of an earlier/manual edit.
for index in "${!snapshots[@]}"; do
  snapshot="${snapshots[$index]}"
  test -f "$snapshot" || { echo "missing version snapshot: $snapshot" >&2; exit 1; }
  cp "$snapshot" "$state_dir/before-snapshot-$index"
done

# Preserve the complete non-version-bearing-snapshot state across the test. This catches
# tracked, staged, and untracked side effects while allowing shipshape's pre-hook
# manifest/lock/changelog edits to remain exactly as they were.
exclude_args=()
for snapshot in "${snapshots[@]}"; do
  exclude_args+=(":(exclude)$snapshot")
done
git diff --binary HEAD -- . "${exclude_args[@]}" >"$state_dir/before.diff"
git ls-files --others --exclude-standard -z >"$state_dir/before.untracked"

# Snapshot updates must execute in this checkout. An ambient Cargo wrapper may
# offload tests without INSTA_UPDATE or return writes only in a remote sandbox.
# Select stable's real tools, as the exact-commit release gate does, rather than
# relying on wrapper-specific environment forwarding or artifact copy-back.
(
  stable_cargo="$(rustup which --toolchain stable cargo)"
  export PATH="$(dirname "$stable_cargo"):$PATH" RUSTUP_TOOLCHAIN=stable
  INSTA_UPDATE=always cargo test --locked --release -p taskfleet --test envelope_snapshots
)

snap_new="$(find . \( -path ./target -o -path ./.git \) -prune -o -name '*.snap.new' -print -quit)"
if [ -n "$snap_new" ]; then
  echo "bump hook left unreviewed .snap.new files" >&2
  find . \( -path ./target -o -path ./.git \) -prune -o -name '*.snap.new' -print >&2
  exit 1
fi

git diff --binary HEAD -- . "${exclude_args[@]}" >"$state_dir/after.diff"
git ls-files --others --exclude-standard -z >"$state_dir/after.untracked"
cmp -s "$state_dir/before.diff" "$state_dir/after.diff" || {
  echo "bump hook changed a file outside the version-bearing snapshots" >&2
  exit 1
}
cmp -s "$state_dir/before.untracked" "$state_dir/after.untracked" || {
  echo "bump hook created or removed an unrelated untracked file" >&2
  exit 1
}

./scripts/check-version-snapshots.sh

for index in "${!snapshots[@]}"; do
  snapshot="${snapshots[$index]}"
  test -f "$snapshot" || { echo "missing version snapshot: $snapshot" >&2; exit 1; }

  if cmp -s "$state_dir/before-snapshot-$index" "$snapshot"; then
    compare_status=0
  else
    compare_status=$?
  fi
  case "$compare_status" in
    0)
      echo "version bump did not regenerate expected snapshot: $snapshot" >&2
      exit 1
      ;;
    1) ;;
    *)
      echo "failed to compare regenerated snapshot (status $compare_status): $snapshot" >&2
      exit 1
      ;;
  esac

  # Only status 1 proves a diff. The old trailing `git diff --quiet && { ...; }`
  # returned 1 in the successful changed-snapshot case; because the loop was the
  # script's final command, that status became the hook's silent exit 1.
  if git diff --quiet HEAD -- "$snapshot"; then
    diff_status=0
  else
    diff_status=$?
  fi
  case "$diff_status" in
    0)
      echo "version snapshot is unchanged from HEAD: $snapshot" >&2
      exit 1
      ;;
    1) ;;
    *)
      echo "failed to compare version snapshot with HEAD (status $diff_status): $snapshot" >&2
      exit 1
      ;;
  esac
done

echo "shipshape bump hook regenerated and validated ${#snapshots[@]} version snapshots"
exit 0
