#!/usr/bin/env bash
#
# check-version-snapshots.sh — fail loudly when the committed `version_*` insta
# snapshots do not match the workspace crate version.
#
# Why this exists: the `version` command's output is snapshotted by
# `crates/taskfleet/tests/envelope_snapshots.rs`, so the literal crate version is
# baked into `envelope_snapshots__version_{text,json,jsonl}.snap`. Bumping
# `[workspace.package] version` in `Cargo.toml` without re-accepting those
# snapshots leaves them stale — `cargo test` then fails. During the v0.1.8
# release that stale-snapshot failure only surfaced on `main` CI *after* the tag
# was cut (the local integrated gate ran before the bump). This guard makes the
# mismatch fail fast in the local release gate instead of silently riding a release.
#
# Fix when it fails: refresh the snapshots and re-run the suite —
#   cargo insta test --accept -p taskfleet   # (or the sed/find accept loop)
#   cargo test --workspace
#
# Portable POSIX-ish bash: no PCRE lookahead, works on macOS BSD grep and GNU grep.
# Exit status: 0 = all version snapshots match; 1 = mismatch or missing input.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cargo_toml="$repo_root/Cargo.toml"
snap_dir="$repo_root/crates/taskfleet/tests/snapshots"

if [ ! -f "$cargo_toml" ]; then
  echo "check-version-snapshots: cannot find $cargo_toml" >&2
  exit 1
fi

# Workspace version from [workspace.package] — the first `version = "x.y.z"`
# under that table. Matches the crate version cargo compiles into the binary.
ws_version="$(
  awk '
    /^\[workspace\.package\]/ { in_wp = 1; next }
    /^\[/                     { in_wp = 0 }
    in_wp && /^version[[:space:]]*=/ {
      gsub(/^version[[:space:]]*=[[:space:]]*"/, "")
      gsub(/".*$/, "")
      print
      exit
    }
  ' "$cargo_toml"
)"

if [ -z "$ws_version" ]; then
  echo "check-version-snapshots: could not read [workspace.package] version from $cargo_toml" >&2
  exit 1
fi

status=0

# Report every distinct version token found in a snapshot that differs from
# ws_version. $1 = snapshot filename; the remaining args are grep -oE patterns
# whose matches each contain a semver token we compare against ws_version.
check_snapshot() {
  name="$1"; snap="$snap_dir/$name"; shift
  if [ ! -f "$snap" ]; then
    echo "check-version-snapshots: missing snapshot $snap" >&2
    status=1
    return
  fi

  found_any=0
  missing_required=0
  bad_versions=""
  for pattern in "$@"; do
    pattern_found=0
    while IFS= read -r match; do
      [ -n "$match" ] || continue
      found_any=1
      pattern_found=1
      ver="$(printf '%s' "$match" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?')"
      if [ "$ver" != "$ws_version" ]; then
        case " $bad_versions " in
          *" $ver "*) ;;                     # already reported for this file
          *) bad_versions="$bad_versions $ver" ;;
        esac
      fi
    done <<EOF
$(grep -oE "$pattern" "$snap" || true)
EOF
    if [ "$pattern_found" -eq 0 ]; then
      missing_required=1
    fi
  done

  if [ "$found_any" -eq 0 ]; then
    echo "check-version-snapshots: $name contains no recognizable version field" >&2
    status=1
  elif [ "$missing_required" -ne 0 ]; then
    echo "check-version-snapshots: $name is missing an expected version field" >&2
    status=1
  fi
  if [ -n "$bad_versions" ]; then
    echo "check-version-snapshots: $name encodes version(s)$bad_versions, expected $ws_version" >&2
    status=1
  fi
}

semver='[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?'

# text: header line `taskfleet <version>`.
check_snapshot "envelope_snapshots__version_text.snap" \
  "taskfleet ${semver}"

# version json / jsonl: every `"version"` and `"cli_version"` field.
for f in envelope_snapshots__version_json.snap envelope_snapshots__version_jsonl.snap; do
  check_snapshot "$f" \
    "\"version\": ?\"${semver}\"" \
    "\"cli_version\": ?\"${semver}\""
done

# Skill-list envelopes also expose the release-coupled `cli_version` field.
for f in envelope_snapshots__skill_list_json.snap envelope_snapshots__skill_list_jsonl.snap; do
  check_snapshot "$f" "\"cli_version\": ?\"${semver}\""
done

if [ "$status" -ne 0 ]; then
  echo >&2
  echo "check-version-snapshots: version_* snapshots are out of sync with Cargo.toml ($ws_version)." >&2
  echo "  Refresh them:  cargo insta test --accept -p taskfleet && cargo test --workspace" >&2
  exit 1
fi

echo "check-version-snapshots: version-bearing snapshots match workspace version $ws_version"
