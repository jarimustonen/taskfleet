#!/usr/bin/env bash
# Deterministic regression tests for the release bump hook's file allowlist.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/shipshape-bump-hook-test.XXXXXX")"
cleanup() { rm -rf "$tmp"; }
trap cleanup EXIT
mkdir -p "$tmp/home" "$tmp/system-bin"
ln -s "$(command -v git)" "$tmp/system-bin/git"

snap_dir="crates/taskfleet/tests/snapshots"
snapshots=(
  envelope_snapshots__version_json.snap
  envelope_snapshots__version_jsonl.snap
  envelope_snapshots__version_text.snap
  envelope_snapshots__skill_list_json.snap
  envelope_snapshots__skill_list_jsonl.snap
)

write_snapshots() {
  local fixture="$1" text_version="${2:-0.4.1}"
  cat >"$fixture/$snap_dir/envelope_snapshots__version_json.snap" <<'EOF'
---
source: fixture
---
{
  "version": "0.4.1",
  "skills": [{"cli_version": "0.4.1"}]
}
EOF
  cat >"$fixture/$snap_dir/envelope_snapshots__version_jsonl.snap" <<'EOF'
---
source: fixture
---
{"version":"0.4.1","skills":[{"cli_version":"0.4.1"}]}
EOF
  cat >"$fixture/$snap_dir/envelope_snapshots__version_text.snap" <<EOF
---
source: fixture
---
taskfleet $text_version
EOF
  cat >"$fixture/$snap_dir/envelope_snapshots__skill_list_json.snap" <<'EOF'
---
source: fixture
---
{"skills":[{"cli_version":"0.4.1"}]}
EOF
  cat >"$fixture/$snap_dir/envelope_snapshots__skill_list_jsonl.snap" <<'EOF'
---
source: fixture
---
{"skills":[{"cli_version":"0.4.1"}]}
EOF
  if [[ "$text_version" == 0.5.0 ]]; then
    sed -i.bak 's/0\.4\.1/0.5.0/g' "$fixture/$snap_dir/"envelope_snapshots__skill_list_*.snap
    rm "$fixture/$snap_dir/"*.bak
  fi
}

make_fixture() {
  local fixture="$1" mode="$2"
  mkdir -p "$fixture/scripts" "$fixture/$snap_dir" "$fixture/bin" "$fixture/toolchain/bin"
  cp "$repo_root/scripts/shipshape-bump-hook.sh" "$fixture/scripts/"
  cp "$repo_root/scripts/check-version-snapshots.sh" "$fixture/scripts/"
  if [[ "$mode" == unchanged-valid ]]; then
    write_snapshots "$fixture" 0.5.0
  else
    write_snapshots "$fixture"
  fi
  cat >"$fixture/Cargo.toml" <<'EOF'
[workspace]
members = []

[workspace.package]
version = "0.5.0"
EOF
  printf 'unchanged\n' >"$fixture/tracked.txt"
  # Ambient Cargo deliberately fails: the hook must use stable's local tools,
  # not a host offload shim that can drop INSTA_UPDATE and snapshot writes.
  cat >"$fixture/bin/cargo" <<'STUB'
#!/usr/bin/env bash
echo 'ambient cargo wrapper must not run' >&2
exit 94
STUB
  cat >"$fixture/bin/rustup" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
[[ "$*" == 'which --toolchain stable cargo' ]] || exit 95
printf '%s/toolchain/bin/cargo\n' "$PWD"
STUB
  cat >"$fixture/toolchain/bin/cargo" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
[[ "${RUSTUP_TOOLCHAIN:-}" == stable ]] || exit 96
[[ "$*" == "test --locked --release -p taskfleet --test envelope_snapshots" ]] || {
  echo "cargo stub: unexpected arguments: $*" >&2
  exit 90
}
[[ "${INSTA_UPDATE:-}" == always ]] || {
  echo "cargo stub: INSTA_UPDATE must be always" >&2
  exit 92
}
snap_dir="crates/taskfleet/tests/snapshots"
update() {
  grep -q '0\.4\.1' "$1" || return 0
  sed 's/0\.4\.1/0.5.0/g' "$1" >"$1.tmp"
  mv "$1.tmp" "$1"
}
case "$HOOK_TEST_MODE" in
  success|unchanged-valid)
    update "$snap_dir/envelope_snapshots__version_json.snap"
    update "$snap_dir/envelope_snapshots__version_jsonl.snap"
    update "$snap_dir/envelope_snapshots__version_text.snap"
    update "$snap_dir/envelope_snapshots__skill_list_json.snap"
    update "$snap_dir/envelope_snapshots__skill_list_jsonl.snap"
    ;;
  snap-new)
    HOOK_TEST_MODE=success "$0" "$@"
    printf 'pending\n' >"$snap_dir/envelope_snapshots__version_text.snap.new"
    ;;
  tracked)
    HOOK_TEST_MODE=success "$0" "$@"
    printf 'side effect\n' >>tracked.txt
    ;;
  untracked)
    HOOK_TEST_MODE=success "$0" "$@"
    printf 'side effect\n' >unrelated.tmp
    ;;
  missing)
    HOOK_TEST_MODE=success "$0" "$@"
    rm "$snap_dir/envelope_snapshots__version_text.snap"
    ;;
  malformed)
    HOOK_TEST_MODE=success "$0" "$@"
    printf '%s\n' '---' 'source: malformed' '---' '{"version":"0.5.0"}' >"$snap_dir/envelope_snapshots__version_json.snap"
    ;;
  partial)
    update "$snap_dir/envelope_snapshots__version_json.snap"
    update "$snap_dir/envelope_snapshots__version_jsonl.snap"
    ;;
  cargo-failure)
    update "$snap_dir/envelope_snapshots__version_json.snap"
    echo "cargo stub: simulated snapshot failure" >&2
    exit 93
    ;;
  *) echo "cargo stub: unknown HOOK_TEST_MODE=$HOOK_TEST_MODE" >&2; exit 91 ;;
esac
STUB
  chmod +x "$fixture/bin/"* "$fixture/toolchain/bin/cargo" "$fixture/scripts/"*.sh
  (
    cd "$fixture"
    export HOME="$tmp/home" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null
    git -c init.defaultBranch=main init -q
    git add -A
    git -c user.email=test@example.invalid -c user.name=bump-hook-test \
      -c commit.gpgsign=false commit -qm fixture
    test -z "$(git status --porcelain=v1 --untracked-files=all)"
  )
}

run_case() {
  local mode="$1" expected_status="$2" expected_diagnostic="${3:-}"
  local fixture="$tmp/fixture-$mode" stdout="$tmp/$mode.stdout" stderr="$tmp/$mode.stderr"
  make_fixture "$fixture" "$mode"
  set +e
  (
    cd "$fixture"
    env -i HOME="$tmp/home" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
      TMPDIR="$tmp" HOOK_TEST_MODE="$mode" PATH="$fixture/bin:$tmp/system-bin:/usr/bin:/bin" \
      ./scripts/shipshape-bump-hook.sh
  ) >"$stdout" 2>"$stderr"
  local status=$?
  set -e
  [[ "$status" -eq "$expected_status" ]] || {
    echo "$mode: expected status $expected_status, got $status" >&2
    cat "$stderr" >&2
    exit 1
  }
  if [[ -n "$expected_diagnostic" ]]; then
    grep -F "$expected_diagnostic" "$stderr" >/dev/null || {
      echo "$mode: missing diagnostic: $expected_diagnostic" >&2
      cat "$stderr" >&2
      exit 1
    }
  fi
}

run_case success 0
run_case snap-new 1 "bump hook left unreviewed .snap.new files"
run_case tracked 1 "bump hook changed a file outside the version-bearing snapshots"
run_case untracked 1 "bump hook created or removed an unrelated untracked file"
run_case missing 1 "check-version-snapshots: missing snapshot"
run_case malformed 1 "is missing an expected version field"
run_case partial 1 "encodes version(s) 0.4.1, expected 0.5.0"
run_case unchanged-valid 1 "version bump did not regenerate expected snapshot"
run_case cargo-failure 93 "cargo stub: simulated snapshot failure"

echo "shipshape bump hook tests passed"
