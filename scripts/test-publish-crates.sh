#!/usr/bin/env bash
# Credential-free registry reconciliation tests. Every mutating/network boundary
# is stubbed; the fixture never reaches crates.io or a real cargo publish.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/publish-crates-test.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/repo/scripts" "$tmp/repo/release" "$tmp/repo/crates/taskfleet-core" "$tmp/repo/crates/taskfleet"
fixture_root="$(cd "$tmp/repo" && pwd -P)"
cp "$repo_root/scripts/publish-crates.sh" "$repo_root/scripts/validate-release-topology.sh" "$tmp/repo/scripts/"
cp "$repo_root/release/taskfleet-release.json" "$tmp/repo/release/"
jq '.activation = "ready"' "$tmp/repo/release/taskfleet-release.json" >"$tmp/topology.json"
mv "$tmp/topology.json" "$tmp/repo/release/taskfleet-release.json"
cat >"$tmp/repo/Cargo.toml" <<'EOF'
[workspace]
members = []
[workspace.package]
version = "1.2.3"
EOF
# GNU tar implements -z by resolving gzip through PATH, unlike bsdtar on
# macOS. Declare that external compressor explicitly so this stripped fixture
# exercises the same dependency boundary on both platforms.
for tool in bash jq awk grep mktemp rm mkdir mv tar gzip sha256sum cat cp dirname; do
  path="$(command -v "$tool")" || { echo "missing test prerequisite: $tool" >&2; exit 1; }
  ln -s "$path" "$tmp/bin/$tool"
done
# Keep an executable symlink target with deliberately narrow mode bits in the
# fixture. A blanket chmod of bin/* would dereference this link on Linux and
# change the target from 0700 to 0711.
cat >"$tmp/symlink-mode-probe" <<'STUB'
#!/bin/sh
exit 0
STUB
chmod 700 "$tmp/symlink-mode-probe"
ln -s "$tmp/symlink-mode-probe" "$tmp/bin/symlink-mode-probe"
symlink_target_mode_before="$(LC_ALL=C ls -ld "$tmp/symlink-mode-probe")"
cat >"$tmp/bin/git" <<'STUB'
#!/usr/bin/env bash
case "$*" in
  "rev-parse HEAD") printf '%s\n' 1111111111111111111111111111111111111111 ;;
  "status --porcelain") ;;
  *) exit 90 ;;
esac
STUB
cat >"$tmp/bin/cargo" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$CARGO_LOG"
if [[ "$1" == --version ]]; then echo 'cargo 1.85.0 (fixture)'; exit 0; fi
if [[ "$1" == metadata ]]; then
  target_dir="${CARGO_TARGET_DIR:-$FIXTURE_ROOT/target}"
  jq -n --arg root "$FIXTURE_ROOT" --arg target "$target_dir" '{target_directory:$target,packages:[
    {name:"taskfleet-core",version:"1.2.3",manifest_path:($root+"/crates/taskfleet-core/Cargo.toml"),repository:"https://github.com/jarimustonen/taskfleet",homepage:"https://github.com/jarimustonen/taskfleet",license:"MIT",rust_version:"1.85",description:"core",dependencies:[]},
    {name:"taskfleet",version:"1.2.3",manifest_path:($root+"/crates/taskfleet/Cargo.toml"),repository:"https://github.com/jarimustonen/taskfleet",homepage:"https://github.com/jarimustonen/taskfleet",license:"MIT",rust_version:"1.85",description:"cli",dependencies:[{name:"taskfleet-core",req:"=1.2.3",kind:null,optional:false,target:null,uses_default_features:true,features:[]}]}

  ]}'
  exit 0
fi
if [[ "$1" == package ]]; then
  make_archive() {
    package="$1"; target_dir="${CARGO_TARGET_DIR:-$FIXTURE_ROOT/target}"
    root="$target_dir/package/$package-1.2.3"
    mkdir -p "$root" "$target_dir/package"
    printf '%s\n' '[package]' "name = \"$package\"" 'version = "1.2.3"' >"$root/Cargo.toml"
    jq -n --arg sha "${ARCHIVE_COMMIT:-1111111111111111111111111111111111111111}" '{git:{sha1:$sha}}' >"$root/.cargo_vcs_info.json"
    tar -czf "$target_dir/package/$package-1.2.3.crate" -C "$target_dir/package" "$package-1.2.3"
    rm -rf "$root"
  }
  if [[ "$*" == *--workspace* ]]; then
    make_archive taskfleet-core; make_archive taskfleet
  else
    make_archive "${*: -1}"
  fi
  exit 0
fi
if [[ "$1" == publish ]]; then
  [[ "${CARGO_REGISTRY_TOKEN:-}" == test-token ]] || { echo 'cargo did not receive its scoped test token' >&2; exit 91; }
  echo 'error: crate taskfleet@1.2.3 already exists on crates.io index' >&2
  exit "${PUBLISH_STATUS:-101}"
fi
exit 92
STUB
cat >"$tmp/bin/curl" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
[[ -z "${CARGO_REGISTRY_TOKEN:-}" ]] || { echo 'curl inherited registry token' >&2; exit 95; }
output=''; url=''
while [[ $# -gt 0 ]]; do
  case "$1" in
    -o) output="$2"; shift 2 ;;
    -A|-w|--connect-timeout|--max-time) shift 2 ;;
    -sS|-L) shift ;;
    *) url="$1"; shift ;;
  esac
done
[[ -n "$output" && -n "$url" ]] || exit 93
package=taskfleet
archive="$FIXTURE_ROOT/target/package/$package-1.2.3.crate"
case "$url" in
  */crates/taskfleet/1.2.3)
    [[ "$REGISTRY_MODE" != transport-failure ]] || exit 7
    if [[ "$REGISTRY_MODE" == http500 ]]; then : >"$output"; printf 500; exit 0; fi
    count=0; [[ -f "$CURL_COUNT" ]] && count="$(cat "$CURL_COUNT")"; count=$((count+1)); printf '%s' "$count" >"$CURL_COUNT"
    if [[ "$REGISTRY_MODE" == absent || (("$REGISTRY_MODE" == duplicate-match || "$REGISTRY_MODE" == secondary-after-publish) && "$count" -eq 1) ]]; then : >"$output"; printf 404; exit 0; fi
    checksum="$(sha256sum "$archive" | awk '{print $1}')"
    description=cli; [[ "$REGISTRY_MODE" != metadata-mismatch ]] || description=wrong
    yanked=false; [[ "$REGISTRY_MODE" != yanked ]] || yanked=true
    jq -n --arg checksum "$checksum" --arg description "$description" --argjson yanked "$yanked" '{version:{checksum:$checksum,yanked:$yanked,license:"MIT",rust_version:"1.85",repository:"https://github.com/jarimustonen/taskfleet",homepage:"https://github.com/jarimustonen/taskfleet",description:$description}}' >"$output"
    printf 200 ;;
  */crates/taskfleet/owners)
    if [[ "$REGISTRY_MODE" == secondary500 || "$REGISTRY_MODE" == secondary-after-publish ]]; then : >"$output"; printf 500; exit 0; fi
    owner=jarimustonen; [[ "$REGISTRY_MODE" != owner-mismatch ]] || owner=intruder
    jq -n --arg owner "$owner" '{users:[{login:$owner}]}' >"$output"; printf 200 ;;
  */crates/taskfleet/1.2.3/dependencies)
    req='=1.2.3'; [[ "$REGISTRY_MODE" != dependency-mismatch ]] || req='^1.2.3'
    jq -n --arg req "$req" '{dependencies:[{crate_id:"taskfleet-core",req:$req,kind:"normal",optional:false,target:null,default_features:true,features:[]}]}' >"$output"; printf 200 ;;
  */crates/taskfleet/1.2.3/download)
    cp "$archive" "$output"
    [[ "$REGISTRY_MODE" != checksum-mismatch ]] || printf 'corrupt' >>"$output"
    printf 200 ;;
  *) exit 94 ;;
esac
STUB
cat >"$tmp/bin/sleep" <<'STUB'
#!/usr/bin/env bash
exit 0
STUB
chmod +x "$tmp/bin/git" "$tmp/bin/cargo" "$tmp/bin/curl" "$tmp/bin/sleep"
symlink_target_mode_after="$(LC_ALL=C ls -ld "$tmp/symlink-mode-probe")"
[[ "$symlink_target_mode_after" == "$symlink_target_mode_before" ]] || {
  echo 'fixture setup changed an executable symlink target mode' >&2
  exit 1
}

# Exercise every fixture entry with only the fixture bin on PATH. The full
# protocol cases below then prove that the real command arguments still work.
for tool in bash jq awk grep mktemp rm mkdir mv tar gzip sha256sum cat cp dirname git cargo curl sleep symlink-mode-probe; do
  set +e
  env -i HOME="$tmp" TMPDIR="$tmp" PATH="$tmp/bin" FIXTURE_ROOT="$fixture_root" CARGO_LOG="$tmp/cargo.log" \
    "$tmp/bin/$tool" </dev/null >/dev/null 2>&1
  status=$?
  set -e
  [[ "$status" -ne 126 && "$status" -ne 127 ]] || {
    echo "fixture tool is not executable under stripped PATH: $tool" >&2
    exit 1
  }
done

run_case() {
  local mode="$1" expected="$2" diagnostic="${3:-}"
  rm -rf "$tmp/repo/target"; : >"$tmp/cargo.log"; rm -f "$tmp/curl-count"
  set +e
  env -i HOME="$tmp" PATH="$tmp/bin" FIXTURE_ROOT="$fixture_root" CARGO_LOG="$tmp/cargo.log" CURL_COUNT="$tmp/curl-count" \
    REGISTRY_MODE="$mode" RELEASE_RECEIPT_DIR="$tmp/receipts-$mode" SOURCE_COMMIT=1111111111111111111111111111111111111111 \
    GITHUB_ACTIONS=true GITHUB_EVENT_NAME=push GITHUB_REF_TYPE=tag GITHUB_REF_NAME=v1.2.3 \
    GITHUB_REPOSITORY=jarimustonen/taskfleet GITHUB_SHA=1111111111111111111111111111111111111111 \
    CARGO_REGISTRY_TOKEN=test-token "$tmp/repo/scripts/publish-crates.sh" publish taskfleet >"$tmp/$mode.out" 2>"$tmp/$mode.err"
  status=$?
  set -e
  [[ "$status" -eq "$expected" ]] || { echo "$mode expected $expected, got $status" >&2; cat "$tmp/$mode.err" >&2; exit 1; }
  if [[ -n "$diagnostic" ]]; then grep -F "$diagnostic" "$tmp/$mode.err" >/dev/null || { cat "$tmp/$mode.err" >&2; exit 1; }; fi
}

# The load-bearing plan/package path emits exactly both archives.
rm -rf "$tmp/repo/target"; : >"$tmp/cargo.log"
env -i HOME="$tmp" PATH="$tmp/bin" FIXTURE_ROOT="$fixture_root" CARGO_LOG="$tmp/cargo.log" \
  SOURCE_COMMIT=1111111111111111111111111111111111111111 "$tmp/repo/scripts/publish-crates.sh" package >/dev/null
[[ "$(find "$tmp/repo/target/package" -name '*.crate' | wc -l | tr -d ' ')" == 2 ]]

# A caller-selected build cache also owns package archives and default receipts.
custom_target="$tmp/custom-target"
env -i HOME="$tmp" PATH="$tmp/bin" FIXTURE_ROOT="$fixture_root" CARGO_LOG="$tmp/cargo.log" \
  CARGO_TARGET_DIR="$custom_target" SOURCE_COMMIT=1111111111111111111111111111111111111111 \
  "$tmp/repo/scripts/publish-crates.sh" package >/dev/null
[[ "$(find "$custom_target/package" -name '*.crate' | wc -l | tr -d ' ')" == 2 ]]

run_case match 0
! grep -q '^publish ' "$tmp/cargo.log" || { echo 'matching existing crate was republished' >&2; exit 1; }
[[ -s "$tmp/receipts-match/taskfleet-1.2.3.json" ]]
run_case duplicate-match 0
grep -q '^publish ' "$tmp/cargo.log" || { echo 'absent crate did not attempt publish' >&2; exit 1; }
run_case metadata-mismatch 4 'registry metadata mismatch'
run_case yanked 4 'registry metadata mismatch'
run_case owner-mismatch 4 'registry owner set mismatch'
run_case dependency-mismatch 4 'registry dependency requirements mismatch'
run_case checksum-mismatch 4 'registry archive differs from the sealed local archive'
run_case http500 5 'registry state remained unavailable; no publish attempted'
! grep -q '^publish ' "$tmp/cargo.log"
run_case transport-failure 5 'registry state remained unavailable; no publish attempted'
! grep -q '^publish ' "$tmp/cargo.log"
run_case secondary500 5 'registry state remained unavailable; no publish attempted'
! grep -q '^publish ' "$tmp/cargo.log"
run_case secondary-after-publish 5 'resume reconciliation without another publish attempt'
[[ "$(grep -c '^publish ' "$tmp/cargo.log")" == 1 ]]
run_case absent 1 'remains absent after bounded publish retries'

# Activation and execution context are enforced at the mutating boundary.
jq '.activation = "blocked-r8-r9-r10"' "$tmp/repo/release/taskfleet-release.json" >"$tmp/topology.json"
mv "$tmp/topology.json" "$tmp/repo/release/taskfleet-release.json"
: >"$tmp/cargo.log"
set +e
env -i HOME="$tmp" PATH="$tmp/bin" FIXTURE_ROOT="$fixture_root" CARGO_LOG="$tmp/cargo.log" SOURCE_COMMIT=1111111111111111111111111111111111111111 \
  GITHUB_ACTIONS=true GITHUB_EVENT_NAME=push GITHUB_REF_TYPE=tag GITHUB_REF_NAME=v1.2.3 GITHUB_REPOSITORY=jarimustonen/taskfleet \
  GITHUB_SHA=1111111111111111111111111111111111111111 CARGO_REGISTRY_TOKEN=test-token \
  "$tmp/repo/scripts/publish-crates.sh" publish taskfleet >"$tmp/blocked.out" 2>"$tmp/blocked.err"
status=$?
set -e
[[ "$status" -eq 2 ]]; ! grep -q '^publish ' "$tmp/cargo.log"
jq '.activation = "ready"' "$tmp/repo/release/taskfleet-release.json" >"$tmp/topology.json"; mv "$tmp/topology.json" "$tmp/repo/release/taskfleet-release.json"

# A mismatched source receipt fails before any registry or publish side effect.
rm -rf "$tmp/repo/target"; : >"$tmp/cargo.log"
set +e
env -i HOME="$tmp" PATH="$tmp/bin" FIXTURE_ROOT="$fixture_root" CARGO_LOG="$tmp/cargo.log" CURL_COUNT="$tmp/curl-count" \
  REGISTRY_MODE=match ARCHIVE_COMMIT=2222222222222222222222222222222222222222 SOURCE_COMMIT=1111111111111111111111111111111111111111 \
  "$tmp/repo/scripts/publish-crates.sh" reconcile taskfleet >"$tmp/source.out" 2>"$tmp/source.err"
status=$?
set -e
[[ "$status" -eq 2 ]]
grep -F 'archive source commit' "$tmp/source.err" >/dev/null
! grep -q '^publish ' "$tmp/cargo.log"

echo 'crates.io reconciliation tests passed'
