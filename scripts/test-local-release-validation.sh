#!/usr/bin/env bash
# Fail-closed prerequisite checks for the full local release gate.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/local-release-validation.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin"

for tool in bash cargo cargo-nextest dirname git jq pwd rustup shipshape; do
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
run_missing 'local release validation prerequisite missing: cargo-dist 0.28.2'

echo 'local release validation prerequisite tests passed'
