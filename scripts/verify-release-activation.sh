#!/usr/bin/env bash
# Mutating distribution boundary: all later gates and canonical identity required.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$repo_root"

./scripts/validate-release-topology.sh >/dev/null
jq -e '
  .activation == "ready" and
  .repository == "jarimustonen/taskfleet"
' release/taskfleet-release.json >/dev/null || {
  echo "release activation requires ready canonical Taskfleet repository topology" >&2
  exit 2
}
jq -e '
  .activation == "ready" and
  .source_repository.current == "jarimustonen/taskfleet" and
  .cargo_dist.trigger == "tag-push" and
  .cargo_dist.pr_run_mode == "skip" and
  .cargo_dist.authorization == "wrapper-ref-exact-tag-main-local-validation" and
  .cargo_dist.release_tag_ruleset == 22234415 and
  .cargo_dist.authorization_ref_ruleset == 22234417 and
  .cargo_dist.tap == "jarimustonen/homebrew-taskfleet" and
  .cargo_dist.tap_secret_state == "active-proven-r10"
' release/taskfleet-distribution.json >/dev/null || {
  echo "release activation requires completed R8/R9 distribution topology" >&2
  exit 2
}
if grep -Eq '^dispatch-releases[[:space:]]*=[[:space:]]*true' dist-workspace.toml; then
  echo "release activation requires cargo-dist tag dispatch" >&2
  exit 2
fi
grep -A12 '^on:' .github/workflows/release.yml | grep -Eq '^[[:space:]]+push:' || {
  echo "release activation requires a generated cargo-dist tag trigger" >&2
  exit 2
}
if grep -A12 '^on:' .github/workflows/release.yml | grep -Eq 'pull_request:|workflow_dispatch:'; then
  echo "release activation requires a tag-only cargo-dist workflow" >&2
  exit 2
fi
grep -F 'pr-run-mode = "skip"' dist-workspace.toml >/dev/null || {
  echo "release activation requires cargo-dist PR execution to be disabled" >&2
  exit 2
}
grep -A12 '^on:' .github/workflows/release.yml |
  grep -F -- "- '**[0-9]+.[0-9]+.[0-9]+*'" >/dev/null || {
  echo "release activation requires the exact cargo-dist version-tag pattern" >&2
  exit 2
}
./scripts/test-release-authorization.sh >/dev/null || {
  echo "release activation requires structural exact-SHA authorization" >&2
  exit 2
}
cargo metadata --locked --no-deps --format-version=1 | jq -e '
  [.packages[] | select(.name == "taskfleet-core" or .name == "taskfleet") |
    (.repository == "https://github.com/jarimustonen/taskfleet" and
     .homepage == "https://github.com/jarimustonen/taskfleet")] | length == 2 and all
' >/dev/null || {
  echo "release activation requires canonical Cargo repository/homepage metadata" >&2
  exit 2
}

printf 'Taskfleet release activation verified\n'
