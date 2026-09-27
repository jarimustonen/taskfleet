#!/usr/bin/env bash
# Fail-closed prerequisite checks for the full local release gate.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/local-release-validation.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin"

for tool in bash cargo cargo-nextest dirname git jq pwd python3 rustc rustup shipshape; do
  path="$(command -v "$tool")" || { echo "test prerequisite missing: $tool" >&2; exit 1; }
  ln -s "$path" "$tmp/bin/$tool"
done

run_missing() {
  local expected="$1"
  shift
  set +e
  env -i HOME="$tmp" PATH="$tmp/bin" "$@" \
    "$repo_root/scripts/validate-local-release.sh" >"$tmp/stdout" 2>"$tmp/stderr"
  status=$?
  set -e
  [[ "$status" -eq 2 ]] || {
    echo "missing prerequisite did not fail closed (status=$status)" >&2
    cat "$tmp/stderr" >&2
    exit 1
  }
  grep -F "$expected" "$tmp/stderr" >/dev/null || {
    echo "missing prerequisite lacked diagnostic: $expected" >&2
    cat "$tmp/stderr" >&2
    exit 1
  }
}

run_missing 'local release validation prerequisite missing: cargo-deny'
ln -s "$(command -v cargo-deny)" "$tmp/bin/cargo-deny"
run_missing 'local release validation prerequisite missing: cargo-dist 0.33.0'

grep -F 'rustup run stable cargo --version' "$repo_root/scripts/validate-local-release.sh" >/dev/null
grep -F 'export RUSTUP_TOOLCHAIN=stable' "$repo_root/scripts/validate-local-release.sh" >/dev/null
if grep -Eq 'rustup run [0-9]' "$repo_root/scripts/validate-local-release.sh"; then
  echo 'local release validation still selects a frozen Rust toolchain' >&2
  exit 1
fi

# Intercept only the protocol fixture's first allocation, before it clones or
# compiles anything. Assert the actual mktemp argument for both placement modes.
cat >"$tmp/bin/mktemp" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >"$SCRATCH_CAPTURE"
exit 91
STUB
chmod +x "$tmp/bin/mktemp"
check_fixture_parent() {
  local expected="$1"
  shift
  set +e
  env "$@" SCRATCH_CAPTURE="$tmp/scratch-argument" PATH="$tmp/bin:$PATH" \
    "$repo_root/scripts/test-shipshape-release-current-protocol.sh" >"$tmp/stdout" 2>"$tmp/stderr"
  status=$?
  set -e
  [[ "$status" -eq 91 && "$(cat "$tmp/scratch-argument")" == "-d $expected/shipshape-current-protocol.XXXXXX" ]] || {
    echo "protocol fixture scratch placement mismatch (expected $expected, status=$status)" >&2
    cat "$tmp/stderr" >&2
    exit 1
  }
}
check_fixture_parent /var/tmp -u TMPDIR
mkdir -p "$tmp/fixture-parent"
check_fixture_parent "$tmp/fixture-parent" TMPDIR="$tmp/fixture-parent"

echo 'local release validation prerequisite and fixture scratch tests passed'
