#!/usr/bin/env bash
# Complete local release gate. Run only from the exact clean commit to release.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$repo_root"

require_command() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "local release validation prerequisite missing: $1" >&2
    exit 2
  }
}

for command in cargo cargo-nextest cargo-deny git jq rustup shipshape; do
  require_command "$command"
done

dist_bin="${DIST_BIN:-}"
if [[ -z "$dist_bin" ]]; then
  dist_bin="$(command -v dist || true)"
fi
[[ -n "$dist_bin" && -x "$dist_bin" ]] || {
  echo "local release validation prerequisite missing: cargo-dist 0.28.2 (set DIST_BIN to its dist executable)" >&2
  exit 2
}
[[ "$("$dist_bin" --version)" == "cargo-dist 0.28.2" ]] || {
  echo "local release validation requires cargo-dist 0.28.2: $dist_bin" >&2
  exit 2
}
rustup run 1.85 cargo --version >/dev/null 2>&1 || {
  echo "local release validation prerequisite missing: Rust toolchain 1.85" >&2
  exit 2
}

validated_head="$(git rev-parse --verify HEAD)"
[[ "$validated_head" =~ ^[0-9a-f]{40}$ ]] || {
  echo "cannot resolve the release validation commit" >&2
  exit 2
}
test -z "$(git status --porcelain --untracked-files=all)" || {
  echo "local release validation requires a clean working tree" >&2
  exit 2
}

run() {
  printf '\n==> %s\n' "$*"
  "$@"
}

run cargo fmt --all --check
run cargo clippy --locked --workspace --all-targets -- -D warnings
run cargo nextest run --locked --release --workspace
run cargo test --locked --release --workspace --doc
printf '\n==> cargo doc --locked --workspace --no-deps\n'
RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps
run rustup run 1.85 cargo check --locked --workspace --all-targets
run cargo deny --locked check

run ./scripts/check-version-snapshots.sh
run ./scripts/check-canonical-identity.sh
if find crates/taskfleet/tests/snapshots -name '*.snap.new' -print -quit | grep -q .; then
  echo "unreviewed insta snapshot changes remain (*.snap.new)" >&2
  exit 2
fi

run ./scripts/test-local-release-validation.sh
run ./scripts/test-shipshape-bump-hook.sh
run ./scripts/test-publish-crates.sh
run ./scripts/test-release-authorization.sh
run ./scripts/test-release-github-policy.sh
run ./scripts/test-distribution-topology.sh
run ./scripts/test-shipshape-release.sh
run ./scripts/test-shipshape-release-current-protocol.sh
run ./scripts/verify-release-activation.sh
run ./scripts/publish-crates.sh package

run "$dist_bin" generate --check
dist_plan="$(mktemp "${TMPDIR:-/tmp}/taskfleet-dist-plan.XXXXXX.json")"
cleanup() { rm -f "$dist_plan"; }
trap cleanup EXIT
printf '\n==> %s plan --output-format=json\n' "$dist_bin"
"$dist_bin" plan --output-format=json >"$dist_plan"
run ./scripts/validate-distribution-topology.sh "$dist_plan"
run shipshape contract validate --json
shipshape audit --json | jq -e '[.data.gaps[] | select(.severity == "blocking")] | length == 0' >/dev/null || {
  echo "Shipshape reports blocking OSS readiness gaps" >&2
  exit 1
}

[[ "$(git rev-parse --verify HEAD)" == "$validated_head" ]] || {
  echo "HEAD changed during local release validation (expected $validated_head)" >&2
  exit 2
}
test -z "$(git status --porcelain --untracked-files=all)" || {
  echo "working tree changed during local release validation of $validated_head" >&2
  exit 2
}
printf '\nLocal release validation passed for %s\n' "$validated_head"
