#!/usr/bin/env bash
# Real-engine protocol gate for the exact Shipshape 0.12.2 build admitted by
# scripts/shipshape-release.sh. The engine itself performs plan/cut/resume against
# a disposable bare origin; only external build/publish/network tools are stubbed.
# No public ref or registry is reachable.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
readonly expected_commit="d1d48d692707fee0d98697721e763a59e7ee3fb7"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/shipshape-current-protocol.XXXXXX")"
cleanup() {
  status=$?
  if [[ "$status" -ne 0 && "${KEEP_FAILED_FIXTURE:-0}" == 1 ]]; then
    failed="${TMPDIR:-/tmp}/shipshape-current-protocol-failed"
    rm -rf "$failed"
    mv "$tmp" "$failed"
    echo "failed protocol fixture preserved at $failed" >&2
  else
    rm -rf "$tmp"
  fi
}
trap cleanup EXIT
mkdir -p "$tmp/bin" "$tmp/home" "$tmp/cargo"

real_git="$(command -v git)"
real_shipshape="$(command -v shipshape)"
version_json="$($real_shipshape version --json)"
jq -e --arg commit "$expected_commit" '
  .schema_version == 1 and .data.schema_version == 1 and
  .data.version == "0.12.2" and .data.commit == $commit
' <<<"$version_json" >/dev/null || {
  echo "test requires the validated shipshape 0.12.2 commit $expected_commit" >&2
  exit 1
}

test -z "$(git -C "$repo_root" status --porcelain)" || {
  echo "real shipshape protocol test requires a clean source tree" >&2
  exit 1
}

for tool in bash jq sed awk grep mktemp rm mkdir chmod mv dirname pwd sleep tar; do
  tool_path="$(command -v "$tool")" || { echo "test prerequisite missing: $tool" >&2; exit 1; }
  ln -s "$tool_path" "$tmp/bin/$tool"
done
# Never put rustup proxies in the isolated PATH: with an isolated HOME they can
# select/update a global toolchain. Pin this test to the already-installed active
# toolchain's real binaries instead.
toolchain_bin="$(dirname "$(rustup which cargo)")"
for tool in cargo rustc rustdoc; do
  test -x "$toolchain_bin/$tool" || { echo "active toolchain is missing $tool" >&2; exit 1; }
  ln -s "$toolchain_bin/$tool" "$tmp/bin/$tool"
done
cat >"$tmp/bin/shipshape" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$SHIPSHAPE_ARGV_LOG"
exec "$SHIPSHAPE_REAL_BIN" "$@"
STUB
chmod +x "$tmp/bin/shipshape"
cat >"$tmp/bin/dist" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$DIST_STUB_LOG"
case "$*" in
  --version) printf '%s\n' 'cargo-dist 0.28.2' ;;
  'plan --output-format=json') printf '%s\n' '{"releases":[]}' ;;
  build) mkdir -p target/distrib ;;
  *) echo "dist fixture: unexpected arguments: $*" >&2; exit 96 ;;
esac
STUB
chmod +x "$tmp/bin/dist"

cat >"$tmp/bin/git" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$GIT_STUB_LOG"
case "$*" in
  "remote get-url origin"|"remote get-url --push --all origin")
    printf '%s\n' 'git@github.com:jarimustonen/taskfleet.git'
    ;;
  "push origin refs/tags/v"*)
    printf '%s\n' "$*" >>"$TAG_PUSH_LOG"
    if [[ "${ALLOW_TAG_PUSH:-0}" == 1 ]]; then
      [[ "$*" == "push origin refs/tags/$EXPECTED_TAG:refs/tags/$EXPECTED_TAG" ]] || {
        echo "refusing unexpected fixture tag push: $*" >&2
        exit 97
      }
      actual_origin="$($REAL_GIT remote get-url origin)"
      [[ "$actual_origin" == "$FIXTURE_ORIGIN" ]] || {
        echo "refusing tag push to non-fixture origin: $actual_origin" >&2
        exit 97
      }
      [[ "$($REAL_GIT rev-parse --verify "refs/tags/$EXPECTED_TAG^{commit}")" == "$EXPECTED_BUMP_COMMIT" ]] || {
        echo "refusing fixture tag push whose target is not the bump commit" >&2
        exit 97
      }
      exec "$REAL_GIT" "$@"
    fi
    # Keep the cut transport local while supplying the production remote
    # coordinate to the wrapper-created hook. The wrapper already probes this
    # hook through a real local Git push before shipshape reaches this path.
    ref="${3:-}"
    local_ref="${ref%%:*}"
    remote_ref="${ref#*:}"
    [[ "$remote_ref" != "$ref" ]] || remote_ref="$local_ref"
    oid="$($REAL_GIT rev-parse --verify "$local_ref")"
    zeros="$(printf '%040d' 0)"
    printf '%s %s %s %s\n' "$local_ref" "$oid" "$remote_ref" "$zeros" |
      "$GIT_CONFIG_VALUE_0/pre-push" origin git@github.com:jarimustonen/taskfleet.git
    ;;
  "push origin HEAD:refs/heads/main"|push\ */probe.git\ refs/tags/v0.0.0-shipshape-pretag-probe)
    exec "$REAL_GIT" "$@"
    ;;
  push\ *)
    echo "git protocol stub: unexpected push form: $*" >&2
    exit 97
    ;;
  *) exec "$REAL_GIT" "$@" ;;
esac
STUB
chmod +x "$tmp/bin/git"

cat >"$tmp/bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$GH_STUB_LOG"
if [[ "$*" == '--version' ]]; then
  printf '%s\n' 'gh version 2.79.0 (fixture)'
  exit 0
fi
if [[ "$*" == 'repo view jarimustonen/taskfleet --json nameWithOwner -q .nameWithOwner' ]]; then
  printf '%s\n' jarimustonen/taskfleet
  exit 0
fi
if [[ "$*" == run\ list* ]]; then
  if [[ "${GH_MODE:-pretag}" == pretag ]]; then
    echo reached-exact-sha-ci-gate >&2
    exit 42
  fi
  case "$*" in
    "run list --workflow release.yml --branch $EXPECTED_TAG --event push --json databaseId,status,conclusion,headBranch,headSha,url --limit 20"|\
    "run list --workflow publish-crates.yml --branch $EXPECTED_TAG --event push --json databaseId,status,conclusion,headBranch,headSha,url --limit 20") ;;
    *) echo "gh protocol stub: unexpected delegated run list: $*" >&2; exit 98 ;;
  esac
  remote_sha="$($REAL_GIT --git-dir="$FIXTURE_ORIGIN" rev-parse "refs/tags/$EXPECTED_TAG^{commit}")"
  [[ "$remote_sha" == "$EXPECTED_BUMP_COMMIT" ]] || {
    echo "gh protocol stub: fixture remote tag does not resolve to expected bump commit" >&2
    exit 98
  }
  jq -n --arg branch "$EXPECTED_TAG" --arg sha "$EXPECTED_BUMP_COMMIT" \
    '[{databaseId:9001,status:"completed",conclusion:"failure",headBranch:$branch,headSha:$sha,url:"https://example.invalid/actions/9001"}]'
  exit 0
fi
if [[ "$*" == 'run view 9001 --json status,conclusion,url,jobs' && "${GH_MODE:-pretag}" != pretag ]]; then
  printf '%s\n' '{"status":"completed","conclusion":"failure","url":"https://example.invalid/actions/9001","jobs":[{"name":"fixture delegated publish","status":"completed","conclusion":"failure"}]}'
  exit 0
fi
echo "gh protocol stub: unexpected arguments: $*" >&2
exit 98
STUB
chmod +x "$tmp/bin/gh"

git init --bare -q "$tmp/origin.git"
git -C "$repo_root" push -q "$tmp/origin.git" HEAD:refs/heads/main
git clone -q --branch main "$tmp/origin.git" "$tmp/repo"
git -C "$tmp/repo" config user.name protocol-test
git -C "$tmp/repo" config user.email protocol-test@example.invalid
# Canonical activation is simulated only inside the disposable fixture; no
# public tag or registry is reachable.
jq '.activation = "ready" | .repository = "jarimustonen/taskfleet"' \
  "$tmp/repo/release/taskfleet-release.json" >"$tmp/topology.json"
mv "$tmp/topology.json" "$tmp/repo/release/taskfleet-release.json"
jq '.activation = "ready" | .source_repository.current = "jarimustonen/taskfleet" |
  .cargo_dist.trigger = "tag-push" | .cargo_dist.pr_run_mode = "skip" |
  .cargo_dist.tap_secret_state = "active-proven-r10" |
  .cargo_dist.activation_gate = "scripts/verify-release-tag-authorization.sh" |
  .cargo_dist.authorization = "wrapper-ref-exact-tag-main-green-ci"' \
  "$tmp/repo/release/taskfleet-distribution.json" >"$tmp/distribution.json"
mv "$tmp/distribution.json" "$tmp/repo/release/taskfleet-distribution.json"
sed -i.bak 's/^dispatch-releases = true$/dispatch-releases = false/' "$tmp/repo/dist-workspace.toml"
rm "$tmp/repo/dist-workspace.toml.bak"
grep -F 'https://github.com/jarimustonen/taskfleet' "$tmp/repo/Cargo.toml" >/dev/null
git -C "$tmp/repo" add release/taskfleet-release.json release/taskfleet-distribution.json \
  dist-workspace.toml Cargo.toml .github/workflows/release.yml
git -C "$tmp/repo" commit -qm 'fixture: activate isolated release topology'
git -C "$tmp/repo" push -q origin HEAD:refs/heads/main

run_env=(
  env -i
  HOME="$tmp/home"
  CARGO_HOME="$tmp/cargo"
  TMPDIR="$tmp"
  PATH="$tmp/bin:/usr/bin:/bin"
  REAL_GIT="$real_git"
  SHIPSHAPE_REAL_BIN="$real_shipshape"
  SHIPSHAPE_ARGV_LOG="$tmp/shipshape.log"
  DIST_STUB_LOG="$tmp/dist.log"
  GIT_STUB_LOG="$tmp/git.log"
  TAG_PUSH_LOG="$tmp/tag-push.log"
  GH_STUB_LOG="$tmp/gh.log"
  GIT_CONFIG_GLOBAL=/dev/null
  GIT_CONFIG_NOSYSTEM=1
  GIT_TERMINAL_PROMPT=0
)

(
  cd "$tmp/repo"
  "${run_env[@]}" shipshape contract show --json --require-approved |
    jq -e '(.data.targets | length > 0) and
      (.data.targets | all(.adapter == "cargo-publish-ci" or .adapter == "cargo-dist"))' >/dev/null || {
      echo "refusing real protocol cut: every publish target must be delegated to CI" >&2
      exit 1
    }
  "${run_env[@]}" ./scripts/shipshape-release.sh plan minor >"$tmp/plan.json"
)
plan_id="$(jq -er '.data.plan_id' "$tmp/plan.json")"
jq -e '
  .schema_version == 1 and .data.bump.level == "minor" and
  (.data.bump.from_version | type == "string") and
  (.data.bump.to_version | type == "string") and
  .data.bump.from_version != .data.bump.to_version
' "$tmp/plan.json" >/dev/null

# Prove the engine, not the wrapper's JSON extraction, is the seal authority.
plan_file="$tmp/repo/.git/ossctl/plans/$plan_id.json"
cp "$plan_file" "$tmp/plan.original.json"
jq '.plan.bump.level = "major"' "$plan_file" >"$tmp/plan.tampered.json"
mv "$tmp/plan.tampered.json" "$plan_file"
set +e
(
  cd "$tmp/repo"
  "${run_env[@]}" shipshape release cut --plan "$plan_id" --bump major --json
) >"$tmp/tamper.stdout" 2>"$tmp/tamper.stderr"
tamper_status=$?
set -e
[[ "$tamper_status" -ne 0 ]] || { echo "shipshape accepted a plan with an invalid seal" >&2; exit 1; }
mv "$tmp/plan.original.json" "$plan_file"
(
  cd "$tmp/repo"
  "${run_env[@]}" shipshape release list --json |
    jq -e '.data.in_flight_count == 0 and (.data.unreadable | length) == 0' >/dev/null
)

set +e
(
  cd "$tmp/repo"
  "${run_env[@]}" ./scripts/shipshape-release.sh cut "$plan_id"
) >"$tmp/cut.stdout" 2>"$tmp/cut.stderr"
status=$?
set -e
[[ "$status" -eq 42 ]] || {
  echo "real shipshape cut did not stop at the isolated exact-SHA CI gate (status=$status)" >&2
  cat "$tmp/cut.stderr" >&2
  exit 1
}
grep -F reached-exact-sha-ci-gate "$tmp/cut.stderr" >/dev/null

(
  cd "$tmp/repo"
  "${run_env[@]}" shipshape release list --json >"$tmp/list.json"
)
run_id="$(jq -er --arg plan "$plan_id" '
  [.data.runs[] | select(.plan_id == $plan and .in_flight)] |
  if length == 1 then .[0].run_id else error("expected one held run") end
' "$tmp/list.json")"
(
  cd "$tmp/repo"
  "${run_env[@]}" shipshape release show "$run_id" --json >"$tmp/show.json"
)
tag="$(jq -er '.data.state.tags | keys | if length == 1 then .[0] else error("one tag required") end' "$tmp/show.json")"
bump_commit="$(jq -er '.data.state.bump.commit' "$tmp/show.json")"
expected_gh="run list -R jarimustonen/taskfleet --workflow ci.yml --branch main --commit $bump_commit --event push --limit 1 --json databaseId -q .[0].databaseId"
grep -Fx "$expected_gh" "$tmp/gh.log" >/dev/null || {
  echo "real protocol test did not query push CI for the exact bump SHA" >&2
  cat "$tmp/gh.log" >&2
  exit 1
}
grep -Fx "release cut --plan $plan_id --json" "$tmp/shipshape.log" >/dev/null || {
  echo "wrapper duplicated Shipshape's stored bump input instead of delegating it" >&2
  cat "$tmp/shipshape.log" >&2
  exit 1
}
[[ "$(wc -l <"$tmp/tag-push.log" | tr -d ' ')" == 1 ]] || {
  echo "expected exactly one held release-tag push attempt" >&2
  cat "$tmp/tag-push.log" >&2
  exit 1
}
jq -e --arg tag "$tag" '
  .data.state.status == "in_progress" and .data.state.current_phase == null and
  [.data.state.phases[] | {phase,outcome}] == [
    {phase:"bump",outcome:"ok"},{phase:"dry_run",outcome:"ok"},
    {phase:"build",outcome:"ok"},{phase:"publish",outcome:"ok"},
    {phase:"tag",outcome:"failed"}
  ] and
  .data.state.tags[$tag].created_local == true and
  .data.state.tags[$tag].pushed_remote == false and
  .data.state.tags[$tag].github_release == false and
  .data.state.tags[$tag].github_release_delegated == false and
  .data.last_seq == .data.state.applied_seq and
  [.data.recent_events[-3:][] | {kind,phase,tag,outcome}] == [
    {kind:"phase_entered",phase:"tag",tag:null,outcome:null},
    {kind:"tag_created_local",phase:null,tag:$tag,outcome:null},
    {kind:"phase_completed",phase:"tag",tag:null,outcome:"failed"}
  ]
' "$tmp/show.json" >/dev/null
[[ "$(git -C "$tmp/repo" rev-parse "$tag^{commit}")" == "$bump_commit" ]]
test -z "$(git -C "$tmp/repo" ls-remote --tags origin "refs/tags/$tag" "refs/tags/$tag^{}")"
test -s "$tmp/repo/.git/shipshape-held-tags/$run_id.json"

# Exercise the actual 0.12.2 resume JSONL and verify envelope after the safe
# boundary. The release tag is pushed only to the local bare origin. Controlled
# failed workflow observations stop verify immediately, without registry/network
# observation, while proving resume's tag transition and verify's new delegated
# run fields.
set +e
(
  cd "$tmp/repo"
  "${run_env[@]}" ALLOW_TAG_PUSH=1 GH_MODE=delegated-failed \
    FIXTURE_ORIGIN="$tmp/origin.git" EXPECTED_TAG="$tag" EXPECTED_BUMP_COMMIT="$bump_commit" \
    shipshape release resume "$run_id" --json
) >"$tmp/resume.jsonl" 2>"$tmp/resume.stderr"
resume_status=$?
set -e
[[ "$resume_status" -eq 2 ]] || {
  echo "isolated resume should stop on the controlled delegated CI failure (status=$resume_status)" >&2
  cat "$tmp/resume.stderr" >&2
  exit 1
}
jq -Rse 'split("\n") | map(fromjson?) | any(.error.code == "delegated_run_failed")' \
  "$tmp/resume.stderr" >/dev/null
jq -s -e --arg tag "$tag" '
  any(.[]; .kind == "tag_pushed_remote" and .tag == $tag) and
  any(.[]; .kind == "phase_completed" and .phase == "tag" and .outcome == "ok") and
  any(.[]; .kind == "phase_entered" and .phase == "verify")
' "$tmp/resume.jsonl" >/dev/null
local_tag_object="$(git -C "$tmp/repo" rev-parse "refs/tags/$tag")"
remote_tag_object="$(git -C "$tmp/repo" ls-remote origin "refs/tags/$tag" | awk '{print $1}')"
remote_tag_commit="$(git -C "$tmp/repo" ls-remote origin "refs/tags/$tag^{}" | awk '{print $1}')"
[[ "$(git -C "$tmp/repo" cat-file -t "$local_tag_object")" == tag ]] || {
  echo "shipshape did not create an annotated release tag" >&2
  exit 1
}
[[ "$remote_tag_object" == "$local_tag_object" && "$remote_tag_commit" == "$bump_commit" ]] || {
  echo "fixture remote tag object/target does not match the local tag and bump commit" >&2
  exit 1
}
(
  cd "$tmp/repo"
  "${run_env[@]}" GH_MODE=delegated-failed FIXTURE_ORIGIN="$tmp/origin.git" \
    EXPECTED_TAG="$tag" EXPECTED_BUMP_COMMIT="$bump_commit" \
    shipshape release show "$run_id" --json >"$tmp/show-resumed.json"
  "${run_env[@]}" GH_MODE=delegated-failed FIXTURE_ORIGIN="$tmp/origin.git" \
    EXPECTED_TAG="$tag" EXPECTED_BUMP_COMMIT="$bump_commit" \
    shipshape release verify "$run_id" --json >"$tmp/verify.json"
)
jq -e --arg tag "$tag" '
  .schema_version == 1 and .data.state.schema_version == 6 and
  .data.state.tags[$tag].pushed_remote == true and
  ([.data.state.phases[] | select(.phase == "tag") | .outcome] | last) == "ok" and
  .data.state.current_phase == null
' "$tmp/show-resumed.json" >/dev/null
jq -e '
  .schema_version == 1 and .data.summary.delegated_failed == 4 and
  .data.summary.unknown == 4 and .data.summary.reconciled == 4 and
  ([.data.targets[].target] | sort) == [
    "rust:taskfleet-core:crates.io", "rust:taskfleet:crates.io",
    "rust:taskfleet:gh-releases",
    "rust:taskfleet:homebrew"
  ] and
  (.data.targets | all(
    .outcome == "unknown" and .delegated_run.status == "failed" and
    .delegated_run.run_id == 9001 and
    .delegated_run.url == "https://example.invalid/actions/9001"
  ))
' "$tmp/verify.json" >/dev/null
[[ "$(wc -l <"$tmp/tag-push.log" | tr -d ' ')" == 2 ]] || {
  echo "expected exactly one held and one fixture-local release-tag push attempt" >&2
  exit 1
}

# Exercise 0.12.2's final phase independently of external destinations. This
# tag-only fixture starts with origin/main already at the release commit, exactly
# the state Taskfleet's pre-tag adapter establishes. The engine must treat that
# ordinary fast-forward as idempotent and journal advance_branch before completion.
git init --bare -q "$tmp/advance-origin.git"
git --git-dir="$tmp/advance-origin.git" symbolic-ref HEAD refs/heads/main
git clone -q "$tmp/advance-origin.git" "$tmp/advance-repo"
git -C "$tmp/advance-repo" config user.name protocol-test
git -C "$tmp/advance-repo" config user.email protocol-test@example.invalid
mkdir -p "$tmp/advance-repo/src"
cat >"$tmp/advance-repo/Cargo.toml" <<'TOML'
[package]
name = "shipshape-advance-fixture"
version = "1.0.0"
edition = "2021"
publish = false
TOML
printf '%s\n' 'fn main() {}' >"$tmp/advance-repo/src/main.rs"
cat >"$tmp/advance-repo/OSS-RELEASE.md" <<'CONTRACT'
---
schema_version: 2
status: approved
maturity: mvp
ecosystems: [rust]
targets: []
versioning: semver
changelog: {mode: automated}
release: {model: gated, layout: single}
provenance_level: none
dependency_bot: dependabot
health_badges: []
license: MIT
docs_site: none
---
# Isolated advance-branch fixture
CONTRACT
git -C "$tmp/advance-repo" add Cargo.toml OSS-RELEASE.md src/main.rs
git -C "$tmp/advance-repo" commit -qm 'fixture: sealed release commit'
git -C "$tmp/advance-repo" push -qu origin HEAD:refs/heads/main
(
  cd "$tmp/advance-repo"
  "$real_shipshape" release plan --json >"$tmp/advance-plan.json"
  advance_plan="$(jq -er .data.plan_id "$tmp/advance-plan.json")"
  "$real_shipshape" release cut --plan "$advance_plan" --json >"$tmp/advance-cut.jsonl"
  "$real_shipshape" release list --json >"$tmp/advance-list.json"
)
advance_run="$(jq -er '.data.runs | if length == 1 then .[0].run_id else error("one run required") end' "$tmp/advance-list.json")"
(
  cd "$tmp/advance-repo"
  "$real_shipshape" release show "$advance_run" --json >"$tmp/advance-show.json"
)
jq -s -e '
  any(.[]; .kind == "default_branch_selected" and .branch == "main") and
  any(.[]; .kind == "default_branch_advanced" and .branch == "main") and
  any(.[]; .kind == "phase_completed" and .phase == "advance_branch" and .outcome == "ok")
' "$tmp/advance-cut.jsonl" >/dev/null
jq -e '
  .data.state.status == "completed" and
  .data.state.default_branch.branch == "main" and
  ([.data.state.phases[] | select(.phase == "advance_branch") | .outcome] | last) == "ok"
' "$tmp/advance-show.json" >/dev/null
[[ "$(git --git-dir="$tmp/advance-origin.git" rev-parse refs/heads/main)" == \
   "$(git -C "$tmp/advance-repo" rev-parse HEAD)" ]] || {
  echo "Shipshape advance_branch moved the isolated default branch unexpectedly" >&2
  exit 1
}

# Omission of --bump on cut is intentional and proven above: 0.12.2 authenticates
# and restores the bump from its stored plan. The complete fixture proves held-tag,
# resume, delegated verification, and default-branch behavior without a public
# remote release or registry publish.
echo "shipshape 0.12.2 real protocol test passed (held and locally resumed $run_id at $tag; public remotes untouched)"
