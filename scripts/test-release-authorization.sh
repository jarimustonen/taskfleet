#!/usr/bin/env bash
# Deterministic security regression test for release-tag authorization.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
release="$repo_root/.github/workflows/release.yml"
publish="$repo_root/.github/workflows/publish-crates.yml"

# The production policy parser is a separate fixture so malformed/redacted API
# responses cannot be hidden by the tag-authorizer's stubbed policy boundary.
"$repo_root/scripts/test-release-github-policy.sh" >/dev/null

check_workflow() {
  local workflow="$1"
  # cargo-dist is tag-only: no PR can execute generated code and no reusable
  # call inherits repository release secrets.
  if grep -A12 '^on:' "$workflow" | grep -Eq 'pull_request:|workflow_dispatch:'; then return 1; fi
  if grep -F 'secrets: inherit' "$workflow" >/dev/null; then return 1; fi
  if grep -F 'custom-taskfleet-release-gate' "$workflow" >/dev/null; then return 1; fi
  [[ "$(grep -Fc 'name: "Require wrapper-authorized exact-main release tag"' "$workflow")" -ge 1 ]] || return 1
  grep -F 'run: "./scripts/verify-release-tag-authorization.sh"' "$workflow" >/dev/null || return 1
  [[ "$(grep -Fc '"GH_TOKEN": "${{ secrets.HOMEBREW_TAP_TOKEN }}"' "$workflow")" == 1 ]] || return 1
  if grep -F '"GH_TOKEN": "${{ github.token }}"' "$workflow" >/dev/null; then return 1; fi
  grep -A8 '^  build-local-artifacts:' "$workflow" | grep -F 'needs:' >/dev/null || return 1
  grep -A12 '^  build-local-artifacts:' "$workflow" | grep -F 'needs.plan.outputs.publishing == '\''true'\''' >/dev/null || return 1
  grep -A8 '^  build-global-artifacts:' "$workflow" | grep -F -- '- build-local-artifacts' >/dev/null || return 1
  grep -A12 '^  host:' "$workflow" | grep -F -- '- build-local-artifacts' >/dev/null || return 1
  # Exact 0.33.0 still accepts a skipped local matrix. The validated plan must
  # therefore retain non-null gated local jobs for every admitted release.
  grep -A12 '^  host:' "$workflow" | grep -F 'needs.build-local-artifacts.result == '\''skipped'\''' >/dev/null || return 1
}
check_workflow "$release"

# crates.io keeps its credential in a dedicated push-only job.
# workflow_dispatch remains credential-free and can only build package archives.
if grep -A12 '^on:' "$publish" | grep -Eq 'pull_request:'; then
  echo "PR-triggered crates workflow unexpectedly exposes a release gate" >&2; exit 1
fi
[[ "$(grep -Fc 'GH_TOKEN: ${{ secrets.HOMEBREW_TAP_TOKEN }}' "$publish")" == 1 ]] || {
  echo "crates tag gate does not receive the release credential exactly once" >&2; exit 1
}
grep -A4 '^  release-authorization:' "$publish" | grep -F 'if: ${{ github.event_name == '\''push'\'' }}' >/dev/null || {
  echo "crates authorization job is not restricted to tag push events" >&2; exit 1
}
grep -A4 '^  publish-core:' "$publish" | grep -F 'release-authorization' >/dev/null || {
  echo "crates publication does not depend on release authorization" >&2; exit 1
}
if grep -F 'GH_TOKEN: ${{ github.token }}' "$publish" >/dev/null; then
  echo "crates tag gate still uses the redacted workflow token" >&2; exit 1
fi

# The authorization script itself is exercised, not merely grepped. Every
# independently mutable coordinate must fail closed.
tmp="$(mktemp -d "${TMPDIR:-/tmp}/taskfleet-release-auth.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/repo/scripts" "$tmp/repo/release" "$tmp/bin"
for tool in bash jq git awk; do
  if [[ "$tool" == git && -n "${REAL_GIT:-}" ]]; then
    tool_path="$REAL_GIT"
  else
    tool_path="$(command -v "$tool")" || { echo "test prerequisite missing: $tool" >&2; exit 1; }
  fi
  ln -s "$tool_path" "$tmp/bin/$tool"
done
# The production verifier requires cargo to exist but deliberately does not
# execute it. A fixture stub avoids depending on a host rustup proxy after
# env -i removes HOME (as on GitHub-hosted runners).
cat >"$tmp/bin/cargo" <<'STUB'
#!/bin/sh
exit 99
STUB
chmod +x "$tmp/bin/cargo"
cp "$repo_root/scripts/verify-release-tag-authorization.sh" "$tmp/repo/scripts/"
cat >"$tmp/repo/scripts/verify-release-activation.sh" <<'STUB'
#!/bin/sh
[ "${ACTIVATION_OK:-0}" = 1 ]
STUB
cat >"$tmp/repo/scripts/verify-release-github-policy.sh" <<'STUB'
#!/bin/sh
[ "${POLICY_OK:-1}" = 1 ]
STUB
chmod +x "$tmp/repo/scripts/"*.sh
cat >"$tmp/repo/Cargo.toml" <<'TOML'
[workspace.package]
version = "0.6.0"
TOML
cat >"$tmp/bin/gh" <<'STUB'
#!/bin/sh
case "$*" in
  'api repos/jarimustonen/taskfleet --jq .node_id')
    printf '%s\n' "${REPO_NODE_ID:?}" ;;
  'api repos/jarimustonen/taskfleet/git/ref/heads/taskfleet-release-authorizations/v0.6.0')
    [ "${AUTH_REF_EXISTS:-1}" = 1 ] || exit 1
    jq -n --arg sha "${AUTH_SHA:?}" '{ref:"refs/heads/taskfleet-release-authorizations/v0.6.0",object:{type:"commit",sha:$sha}}' ;;
  *) echo "unexpected gh invocation: $*" >&2; exit 97 ;;
esac
STUB
chmod +x "$tmp/bin/gh"
git -C "$tmp/repo" init -q
git -C "$tmp/repo" add Cargo.toml scripts
git -C "$tmp/repo" -c user.name=fixture -c user.email=fixture@example.invalid commit -qm fixture
sha="$(git -C "$tmp/repo" rev-parse HEAD)"
run_auth() {
  (cd "$tmp/repo" && env -i PATH="$tmp/bin:/usr/bin:/bin" ACTIVATION_OK="${ACTIVATION_OK:-1}" POLICY_OK="${POLICY_OK:-1}" \
    REPO_NODE_ID="${REPO_NODE_ID:-R_kgDOS3Iezw}" AUTH_SHA="${AUTH_SHA:-$sha}" AUTH_REF_EXISTS="${AUTH_REF_EXISTS:-1}" \
    GITHUB_REPOSITORY="${FIXTURE_REPOSITORY:-jarimustonen/taskfleet}" \
    GITHUB_REF="${FIXTURE_REF:-refs/tags/v0.6.0}" GITHUB_REF_TYPE="${FIXTURE_REF_TYPE:-tag}" \
    GITHUB_REF_NAME="${FIXTURE_REF_NAME:-v0.6.0}" GITHUB_SHA="${FIXTURE_SHA:-$sha}" \
    scripts/verify-release-tag-authorization.sh)
}
run_auth >/dev/null
for case_name in repository repository_id event_ref ref_type tag version activation policy authorization_missing authorization_sha; do
  set +e
  case "$case_name" in
    repository) FIXTURE_REPOSITORY=x/y run_auth; status=$? ;;
    repository_id) REPO_NODE_ID=R_wrong run_auth; status=$? ;;
    event_ref) FIXTURE_REF=refs/tags/v0.6.1 run_auth; status=$? ;;
    ref_type) FIXTURE_REF_TYPE=branch run_auth; status=$? ;;
    tag) FIXTURE_REF_NAME=v0.6.1 FIXTURE_REF=refs/tags/v0.6.1 run_auth; status=$? ;;
    version) sed -i.bak 's/0.6.0/0.6.1/' "$tmp/repo/Cargo.toml"; run_auth; status=$?; mv "$tmp/repo/Cargo.toml.bak" "$tmp/repo/Cargo.toml" ;;
    activation) ACTIVATION_OK=0 run_auth; status=$? ;;
    policy) POLICY_OK=0 run_auth; status=$? ;;
    authorization_missing) AUTH_REF_EXISTS=0 run_auth; status=$? ;;
    authorization_sha) AUTH_SHA=ffffffffffffffffffffffffffffffffffffffff run_auth; status=$? ;;
  esac
  set -e
  [[ "$status" -ne 0 ]] || { echo "$case_name authorization mutation unexpectedly passed" >&2; exit 1; }
done

# The exact old unsafe properties must be rejected by the same checker.
unsafe="$tmp/unsafe.yml"
cp "$release" "$unsafe"
sed -i.bak '/^on:$/a\
  pull_request:' "$unsafe"
if check_workflow "$unsafe" 2>/dev/null; then
  echo "PR-triggered release workflow unexpectedly passed" >&2; exit 1
fi
cp "$release" "$unsafe"
sed -i.bak '/^jobs:$/i\
# secrets: inherit' "$unsafe"
if check_workflow "$unsafe" 2>/dev/null; then
  echo "secret-inheriting release workflow unexpectedly passed" >&2; exit 1
fi

printf 'Taskfleet structural release authorization fixtures passed\n'
