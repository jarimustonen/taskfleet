#!/usr/bin/env bash
# Validate the Taskfleet-only cargo-dist topology without publishing.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$repo_root"

jq -e '
  .schema_version == 1 and .activation == "ready" and
  .cargo_dist.version == "0.28.2" and
  .cargo_dist.apps == ["taskfleet"] and
  .cargo_dist.tap == "jarimustonen/homebrew-taskfleet" and
  .cargo_dist.trigger == "tag-push" and
  .cargo_dist.pr_run_mode == "skip" and
  .source_repository.current == "jarimustonen/taskfleet"
' release/taskfleet-distribution.json >/dev/null

grep -F 'cargo-dist-version = "0.28.2"' dist-workspace.toml >/dev/null
grep -F 'pr-run-mode = "skip"' dist-workspace.toml >/dev/null
grep -F 'dispatch-releases = false' dist-workspace.toml >/dev/null
grep -F 'tap = "jarimustonen/homebrew-taskfleet"' dist-workspace.toml >/dev/null
if grep -Eq '^\[\[dist\.extra-artifacts\]\]' dist-workspace.toml; then
  echo "Taskfleet distribution must not carry transition artifacts" >&2
  exit 2
fi

cargo metadata --locked --no-deps --format-version 1 | jq -e '
  ([.packages[].name] | sort) == ["taskfleet", "taskfleet-core"] and
  ([.packages[] | .targets[] | select(.kind == ["bin"]) | .name]) == ["taskfleet"] and
  ([.packages[] | select(.name == "taskfleet") | .dependencies[] |
    select(.name == "taskfleet-core" and (.req | startswith("=")))] | length) == 1
' >/dev/null

grep -F 'repository: "jarimustonen/homebrew-taskfleet"' .github/workflows/release.yml >/dev/null

if [[ $# -gt 1 ]]; then
  echo "usage: scripts/validate-distribution-topology.sh [cargo-dist-plan.json]" >&2
  exit 2
fi
if [[ $# -eq 1 ]]; then
  plan="$1"
  [[ -f "$plan" ]] || { echo "cargo-dist plan not found: $plan" >&2; exit 2; }
  version="$(awk -F'"' '/^\[workspace\.package\]/{p=1;next} /^\[/{p=0} p&&/^version[[:space:]]*=/{print $2;exit}' Cargo.toml)"
  jq -e --arg version "$version" '
    .dist_version == "0.28.2" and
    .announcement_tag == ("v" + $version) and
    (.releases | length) == 1 and
    .releases[0].app_name == "taskfleet" and
    .releases[0].app_version == $version and
    .releases[0].hosting.github.owner == "jarimustonen" and
    .releases[0].hosting.github.repo == "taskfleet" and
    ([.artifacts[] | select(.kind == "executable-zip") | .target_triples[]] | sort) == [
      "aarch64-apple-darwin",
      "aarch64-unknown-linux-gnu",
      "x86_64-unknown-linux-gnu"
    ] and
    .artifacts["taskfleet-installer.sh"].kind == "installer" and
    .artifacts["taskfleet.rb"].kind == "installer" and
    (.artifacts["taskfleet.rb"].install_hint | contains("jarimustonen/taskfleet/taskfleet")) and
    ([.ci.github.artifacts_matrix.include[].targets[]] | sort) == [
      "aarch64-apple-darwin",
      "aarch64-unknown-linux-gnu",
      "x86_64-unknown-linux-gnu"
    ] and
    .ci.github.pr_run_mode == "skip"
  ' "$plan" >/dev/null || {
    echo "cargo-dist plan does not match the canonical Taskfleet release topology" >&2
    exit 2
  }
fi
printf 'Taskfleet distribution topology verified\n'
