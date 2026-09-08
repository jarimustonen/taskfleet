#!/usr/bin/env bash
# Package, publish, and cryptographically reconcile Taskfleet's crates.io legs.
# Duplicate-version diagnostics are never interpreted: only registry receipts
# matching the local archive and source commit can complete a leg.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$repo_root"
readonly topology="$repo_root/release/taskfleet-release.json"
readonly cargo_bin="${CARGO_BIN:-cargo}"
readonly curl_bin="${CURL_BIN:-curl}"
readonly tar_bin="${TAR_BIN:-tar}"
readonly sha256_bin="${SHA256_BIN:-sha256sum}"
readonly sleep_bin="${SLEEP_BIN:-sleep}"
readonly registry="${CRATES_IO_API:-https://crates.io/api/v1}"
readonly user_agent="taskfleet-release-reconciler/1 (+https://github.com/jarimustonen/taskfleet)"
publish_token="${CARGO_REGISTRY_TOKEN:-}"
unset CARGO_REGISTRY_TOKEN

usage() {
  echo "usage: scripts/publish-crates.sh package | reconcile <package> | publish <package>" >&2
  exit 2
}

expected_repo="$(./scripts/validate-release-topology.sh)"

version="$(awk -F'"' '
  /^\[workspace\.package\]/ { in_package=1; next }
  /^\[/ { in_package=0 }
  in_package && /^version[[:space:]]*=/ { print $2; exit }
' Cargo.toml)"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$ ]] || {
  echo "invalid workspace version: ${version:-<missing>}" >&2
  exit 2
}
source_commit="${SOURCE_COMMIT:-$(git rev-parse HEAD)}"
[[ "$source_commit" =~ ^[0-9a-f]{40}$ ]] || { echo "invalid source commit: $source_commit" >&2; exit 2; }

assert_publish_context() {
  [[ "${GITHUB_ACTIONS:-}" == true && "${GITHUB_EVENT_NAME:-}" == push &&
     "${GITHUB_REF_TYPE:-}" == tag && "${GITHUB_REF_NAME:-}" == "v$version" &&
     "${GITHUB_REPOSITORY:-}" == "$expected_repo" && "${GITHUB_SHA:-}" == "$source_commit" ]] || {
    echo "cargo publication is restricted to the admitted GitHub version-tag workflow" >&2
    exit 2
  }
  jq -e '.activation == "ready"' "$topology" >/dev/null || {
    echo "release activation is blocked; refusing crates.io publication" >&2
    exit 2
  }
  [[ "$(git rev-parse HEAD)" == "$source_commit" && -z "$(git status --porcelain)" ]] || {
    echo "publish checkout must be clean and exactly match SOURCE_COMMIT" >&2
    exit 2
  }
  [[ -n "$publish_token" ]] || { echo "CARGO_REGISTRY_TOKEN is required in the tag workflow" >&2; exit 2; }
}

metadata_file="$(mktemp "${TMPDIR:-/tmp}/taskfleet-release-metadata.XXXXXX")"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/taskfleet-registry.XXXXXX")"
cleanup() { rm -f "$metadata_file"; rm -rf "$tmp_dir"; }
trap cleanup EXIT
"$cargo_bin" metadata --locked --no-deps --format-version 1 >"$metadata_file"
target_dir="$(jq -er '.target_directory | select(type == "string" and length > 0)' "$metadata_file")" || {
  echo "cargo metadata did not report a target directory" >&2
  exit 2
}
readonly target_dir
readonly receipt_dir="${RELEASE_RECEIPT_DIR:-$target_dir/release-receipts}"

package_manifest() {
  jq -er --arg package "$1" '.crates_io.legs[] | select(.package == $package) | .manifest' "$topology"
}

dependency_of() {
  jq -r --arg package "$1" '.crates_io.legs[] | select(.package == $package) | .depends_on // empty' "$topology"
}

assert_local_metadata() {
  local package="$1" manifest expected_repo dependency
  manifest="$(package_manifest "$package")"
  expected_repo="$(jq -r .repository "$topology")"
  jq -e --arg package "$package" --arg version "$version" --arg manifest "$repo_root/$manifest" --arg repo "https://github.com/$expected_repo" '
    [.packages[] | select(.name == $package and .version == $version and .manifest_path == $manifest and
      .repository == $repo and .homepage == $repo and .license == "MIT" and .rust_version == "1.85")] | length == 1
  ' "$metadata_file" >/dev/null || { echo "$package local package metadata does not match release topology" >&2; exit 2; }
  dependency="$(dependency_of "$package")"
  if [[ -n "$dependency" ]]; then
    jq -e --arg package "$package" --arg dependency "$dependency" --arg requirement "=$version" '
      [.packages[] | select(.name == $package) | .dependencies[] |
       select(.name == $dependency and .req == $requirement)] | length == 1
    ' "$metadata_file" >/dev/null || { echo "$package does not exactly pin $dependency to =$version" >&2; exit 2; }
  fi
}

archive_path() { printf '%s/package/%s-%s.crate\n' "$target_dir" "$1" "$version"; }

validate_archive() {
  local package="$1" archive root archive_commit dependency
  assert_local_metadata "$package"
  archive="$(archive_path "$package")"
  [[ -s "$archive" ]] || { echo "cargo did not create $archive" >&2; exit 2; }
  root="$package-$version"
  archive_commit="$("$tar_bin" -xOf "$archive" "$root/.cargo_vcs_info.json" | jq -er '
    select((.git.dirty // false) == false) | .git.sha1
  ')" || { echo "$package archive is dirty or has no readable source receipt" >&2; exit 2; }
  [[ "$archive_commit" == "$source_commit" ]] || {
    echo "$package archive source commit $archive_commit does not match $source_commit" >&2
    exit 2
  }
  printf 'packaged %s@%s from %s\n' "$package" "$version" "$source_commit"
}

prepare_archive() {
  local package="$1"
  "$cargo_bin" package --locked --no-verify --package "$package"
  validate_archive "$package"
}

prepare_workspace_archives() {
  "$cargo_bin" package --workspace --locked --no-verify
  while IFS= read -r package; do validate_archive "$package"; done < <(jq -r '.crates_io.legs[].package' "$topology")
}

http_get() {
  local url="$1" output="$2" status
  status="$("$curl_bin" -sS -L --connect-timeout 10 --max-time 60 -A "$user_agent" -o "$output" -w '%{http_code}' "$url")" || {
    echo "registry request failed: $url" >&2
    return 1
  }
  printf '%s\n' "$status"
}

sha256_file() { "$sha256_bin" "$1" | awk '{print $1}'; }

reconcile() {
  local package="$1" local_archive version_json owners_json deps_json remote_archive
  local status expected_checksum downloaded_checksum archive_commit dependency receipt expected_deps
  local_archive="$(archive_path "$package")"
  version_json="$tmp_dir/$package-version.json"
  owners_json="$tmp_dir/$package-owners.json"
  deps_json="$tmp_dir/$package-deps.json"
  remote_archive="$tmp_dir/$package-$version.crate"

  status="$(http_get "$registry/crates/$package/$version" "$version_json")" || return 5
  case "$status" in 200) ;; 404) return 3 ;; *) echo "$package@$version registry status $status is not authoritative absence" >&2; return 5 ;; esac
  status="$(http_get "$registry/crates/$package/owners" "$owners_json")" || return 5
  [[ "$status" == 200 ]] || return 5
  status="$(http_get "$registry/crates/$package/$version/dependencies" "$deps_json")" || return 5
  [[ "$status" == 200 ]] || return 5
  status="$(http_get "$registry/crates/$package/$version/download" "$remote_archive")" || return 5
  [[ "$status" == 200 ]] || return 5

  expected_checksum="$(sha256_file "$local_archive")"
  downloaded_checksum="$(sha256_file "$remote_archive")"
  [[ "$expected_checksum" == "$downloaded_checksum" ]] || {
    echo "$package@$version registry archive differs from the sealed local archive" >&2; return 4;
  }
  jq -e --arg checksum "$expected_checksum" --arg package "$package" \
    --arg repo "https://github.com/$expected_repo" \
    --slurpfile metadata "$metadata_file" '
    ($metadata[0].packages[] | select(.name == $package)) as $local |
    .version.checksum == $checksum and .version.yanked == false and
    .version.license == $local.license and .version.rust_version == $local.rust_version and
    .version.repository == $repo and .version.homepage == $repo and
    .version.description == $local.description
  ' "$version_json" >/dev/null || { echo "$package@$version registry metadata mismatch" >&2; return 4; }
  jq -e --slurpfile topology "$topology" '
    ([.users[].login] | unique | sort) == $topology[0].owners
  ' "$owners_json" >/dev/null || { echo "$package@$version registry owner set mismatch" >&2; return 4; }

  expected_deps="$(jq -c --arg package "$package" '
    [.packages[] | select(.name == $package) | .dependencies[] |
      {crate_id:.name,req,kind:(.kind // "normal"),optional,target,
       default_features:.uses_default_features,features:(.features | sort)}] |
    sort_by(.crate_id,.kind,.target,.req)
  ' "$metadata_file")"
  jq -e --argjson expected "$expected_deps" '
    ([.dependencies[] |
      {crate_id,req,kind:(.kind // "normal"),optional,target,
       default_features,features:(.features | sort)}] |
     sort_by(.crate_id,.kind,.target,.req)) == $expected
  ' "$deps_json" >/dev/null || { echo "$package@$version registry dependency requirements mismatch" >&2; return 4; }

  archive_commit="$("$tar_bin" -xOf "$remote_archive" "$package-$version/.cargo_vcs_info.json" | jq -er '.git.sha1')" || {
    echo "$package@$version registry archive has no readable source receipt" >&2; return 4;
  }
  [[ "$archive_commit" == "$source_commit" ]] || {
    echo "$package@$version registry source commit $archive_commit does not match $source_commit" >&2; return 4;
  }

  dependency="$(dependency_of "$package")"
  mkdir -p "$receipt_dir"
  receipt="$receipt_dir/$package-$version.json"
  jq -n --arg package "$package" --arg version "$version" --arg checksum "$expected_checksum" \
    --arg source_commit "$source_commit" --arg dependency "$dependency" --arg requirement "=$version" \
    --arg topology_sha256 "$(sha256_file "$topology")" --arg cargo_version "$("$cargo_bin" --version)" \
    --arg tag "${GITHUB_REF_NAME:-}" --arg workflow_run "${GITHUB_RUN_ID:-}" \
    --slurpfile owners "$owners_json" '
    {schema_version:1,registry:"crates.io",package:$package,version:$version,checksum:$checksum,
     owners:([$owners[0].users[].login] | unique | sort),source_commit:$source_commit,
     topology_sha256:$topology_sha256,cargo_version:$cargo_version,tag:$tag,workflow_run:$workflow_run,
     dependencies:(if $dependency == "" then [] else [{package:$dependency,requirement:$requirement}] end),
     metadata_verified:true,archive_verified:true}
  ' >"$receipt"
  printf 'reconciled %s@%s (%s)\n' "$package" "$version" "$expected_checksum"
}

publish_leg() {
  local package="$1" rc attempt reconcile_rc sealed_checksum cargo_log="$tmp_dir/$package-cargo-publish.log"
  prepare_archive "$package"

  # Never publish during an ambiguous registry state. Only an explicit 404 can
  # cross the mutation boundary; mismatches fail immediately and transients are
  # retried without invoking Cargo.
  for attempt in 1 2 3 4 5; do
    if reconcile "$package"; then
      echo "$package@$version already exists and its registry receipt matches exactly; publish skipped"
      return 0
    else
      rc=$?
    fi
    [[ "$rc" -eq 4 ]] && return 4
    [[ "$rc" -eq 3 ]] && break
    [[ "$attempt" -lt 5 ]] && "$sleep_bin" 15
  done
  [[ "$rc" -eq 3 ]] || { echo "$package@$version registry state remained unavailable; no publish attempted" >&2; return 5; }

  # Retry the transaction itself while the current package remains an
  # authoritative 404. This covers Cargo-index lag for exact prerequisites
  # without interpreting Cargo diagnostics as success.
  sealed_checksum="$(sha256_file "$(archive_path "$package")")"
  for attempt in 1 2 3 4 5 6; do
    if CARGO_REGISTRY_TOKEN="$publish_token" "$cargo_bin" publish --locked --no-verify --package "$package" >"$cargo_log" 2>&1; then
      rc=0
    else
      rc=$?
    fi
    cat "$cargo_log" >&2
    [[ "$(sha256_file "$(archive_path "$package")")" == "$sealed_checksum" ]] || {
      echo "$package@$version cargo publish changed the validated local archive" >&2
      return 4
    }
    for reconcile_attempt in 1 2 3 4 5 6; do
      if reconcile "$package"; then return 0; else reconcile_rc=$?; fi
      [[ "$reconcile_rc" -eq 4 ]] && return 4
      [[ "$reconcile_rc" -eq 3 ]] && break
      [[ "$reconcile_attempt" -lt 6 ]] && "$sleep_bin" 15
    done
    [[ "$reconcile_rc" -eq 3 ]] || {
      echo "$package@$version registry state became unavailable after publish; resume reconciliation without another publish attempt" >&2
      return 5
    }
    [[ "$attempt" -lt 6 ]] && "$sleep_bin" 30
  done
  echo "$package@$version remains absent after bounded publish retries (last cargo status $rc); resume this exact leg" >&2
  return 1
}

command="${1:-}"
package="${2:-}"
case "$command" in
  package)
    [[ -z "$package" ]] || usage
    prepare_workspace_archives
    ;;
  reconcile)
    [[ -n "$package" ]] || usage
    jq -e --arg package "$package" 'any(.crates_io.legs[]; .package == $package)' "$topology" >/dev/null || usage
    prepare_archive "$package"
    reconcile "$package"
    ;;
  publish)
    [[ -n "$package" ]] || usage
    jq -e --arg package "$package" 'any(.crates_io.legs[]; .package == $package)' "$topology" >/dev/null || usage
    assert_publish_context
    publish_leg "$package"
    ;;
  *) usage ;;
esac
