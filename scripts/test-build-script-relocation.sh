#!/usr/bin/env bash
# Regression: a cached build-script executable must use Cargo's current runtime paths.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
build_rs="${BUILD_RS_PATH:-$repo_root/crates/taskfleet/build.rs}"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/taskfleet-build-script-relocation.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

stale_manifest="$tmp/removed-compile-checkout/crates/taskfleet"
build_script="$tmp/taskfleet-build-script"
mkdir -p "$stale_manifest"

# Cargo exposes both values while compiling build.rs. CARGO_MANIFEST_DIR is
# intentionally made stale before the resulting executable is run.
CARGO_MANIFEST_DIR="$stale_manifest" \
CARGO_PKG_VERSION="9.8.7-relocation-test" \
  rustc --edition=2021 "$build_rs" -o "$build_script"
rm -rf "$tmp/removed-compile-checkout"

for fixture in first second; do
  manifest="$tmp/runtime-$fixture"
  out_dir="$tmp/out-$fixture"
  skill_dir="$manifest/skills/example-$fixture"
  mkdir -p "$skill_dir" "$out_dir"
  printf 'fixture=%s version={{CLI_VERSION}}\n' "$fixture" >"$skill_dir/SKILL.template.md"

  stdout="$tmp/stdout-$fixture"
  CARGO_MANIFEST_DIR="$manifest" OUT_DIR="$out_dir" "$build_script" >"$stdout"

  expected="fixture=$fixture version=9.8.7-relocation-test"
  [[ "$(cat "$out_dir/skills/example-$fixture/SKILL.md")" == "$expected" ]] || {
    echo "build-script relocation rendered the wrong skill for $fixture" >&2
    exit 1
  }
  grep -Fx "cargo:rerun-if-changed=$manifest/skills" "$stdout" >/dev/null || {
    echo "build-script relocation did not watch the current skills directory for $fixture" >&2
    exit 1
  }
  grep -Fx "cargo:rerun-if-changed=$skill_dir/SKILL.template.md" "$stdout" >/dev/null || {
    echo "build-script relocation did not watch the current template for $fixture" >&2
    exit 1
  }
  if grep -F "$stale_manifest" "$stdout" >/dev/null; then
    echo "build-script relocation leaked its removed compile-time manifest path" >&2
    exit 1
  fi
done

echo 'build-script relocation regression passed'
