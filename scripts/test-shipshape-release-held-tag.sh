#!/usr/bin/env bash
# End-to-end regression for the validated shipshape 0.12.2 intentionally-held tag journal.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/home" "$tmp/work/release" "$tmp/work/scripts" \
  "$tmp/work/crates/taskfleet" "$tmp/common/shipshape-held-tags"
cp "$repo_root/release/taskfleet-release.json" "$tmp/work/release/"
cp "$repo_root/Cargo.toml" "$repo_root/Cargo.lock" "$repo_root/CHANGELOG.md" "$tmp/work/"
cp "$repo_root/crates/taskfleet/Cargo.toml" "$tmp/work/crates/taskfleet/"
cp "$repo_root/scripts/validate-release-topology.sh" "$tmp/work/scripts/"
cat >"$tmp/work/scripts/check-version-snapshots.sh" <<'STUB'
#!/usr/bin/env bash
exit 0
STUB
cat >"$tmp/work/scripts/validate-local-release.sh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$(git rev-parse HEAD)" >>"$LOCAL_VALIDATION_LOG"
case "${LOCAL_VALIDATION_MODE:-failure}" in
  failure) echo reached-local-release-validation >&2; exit 42 ;;
  success) exit 0 ;;
  dirty) printf 'changed\n' > local-validation-mutation; exit 0 ;;
  head-change) git fixture-move-head; exit 0 ;;
  *) exit 97 ;;
esac
STUB
cat >"$tmp/work/scripts/verify-release-activation.sh" <<'STUB'
#!/usr/bin/env bash
echo reached-post-validation-authorization-gates >&2
exit 43
STUB
chmod +x "$tmp/work/scripts/"*.sh

for tool in bash jq sed awk grep mktemp rm mkdir chmod mv dirname pwd; do
  tool_path="$(command -v "$tool")" || { echo "test prerequisite missing: $tool" >&2; exit 1; }
  ln -s "$tool_path" "$tmp/bin/$tool"
done

version="$(awk -F'"' '
  /^\[workspace\.package\]/ { in_package=1; next }
  /^\[/ { in_package=0 }
  in_package && /^version[[:space:]]*=/ { print $2; exit }
' "$repo_root/Cargo.toml")"
tag="v$version"
bump_commit=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
tag_oid=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
run_id=01M0JA657EJJJYC7J7230JF42N

cat >"$tmp/bin/git" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$GIT_STUB_LOG"
if [[ "$*" == "rev-parse $TAG^{commit}" || "$*" == "rev-parse --verify refs/tags/$TAG^{commit}" ]]; then
  [[ "${HELD_VARIANT:-valid}" != missing-local-tag ]] || exit 1
  if [[ "${HELD_VARIANT:-valid}" == wrong-tag-target ]]; then
    printf '%040d\n' 8
  else
    printf '%s\n' "$BUMP_COMMIT"
  fi
  exit 0
fi
if [[ "$*" == "rev-parse HEAD" ]]; then
  if [[ -f "$HEAD_MOVED_MARKER" ]]; then printf '%040d\n' 7; else printf '%s\n' "$BUMP_COMMIT"; fi
  exit 0
fi
if [[ "$*" == "rev-parse --verify refs/tags/$TAG" ]]; then
  if [[ "${HELD_VARIANT:-valid}" == moved-local-tag ]]; then
    printf '%040d\n' 9
  else
    printf '%s\n' "$TAG_OID"
  fi
  exit 0
fi
case "$*" in
  "rev-parse --show-toplevel") printf '%s\n' "$GIT_STUB_ROOT" ;;
  "remote get-url origin"|"remote get-url --push --all origin") printf '%s\n' 'git@github.com:jarimustonen/taskfleet.git' ;;
  "symbolic-ref --quiet --short HEAD") printf '%s\n' main ;;
  "cat-file -e "*) [[ "${HELD_VARIANT:-valid}" != missing-local-tag ]] ;;
  "rev-parse --git-common-dir") printf '%s\n' "$GIT_COMMON" ;;
  "ls-remote --tags origin "*)
    [[ "${HELD_VARIANT:-valid}" != remote-present ]] || printf '%s\trefs/tags/%s\n' "$BUMP_COMMIT" "$TAG"
    ;;
  "merge-base --is-ancestor "*) [[ "${HELD_VARIANT:-valid}" != bad-ancestry ]] ;;
  "fetch origin +refs/heads/main:refs/remotes/origin/main") ;;
  "rev-parse refs/remotes/origin/main") printf '%s\n' "$BUMP_COMMIT" ;;
  "status --porcelain"|"status --porcelain --untracked-files=all")
    [[ "${LOCAL_VALIDATION_MODE:-failure}" != dirty || ! -f local-validation-mutation ]] || printf '?? local-validation-mutation\n'
    ;;
  "fixture-move-head") printf moved >"$HEAD_MOVED_MARKER" ;;
  *) echo "stub git: unexpected arguments: $*" >&2; exit 96 ;;
esac
STUB
chmod +x "$tmp/bin/git"

cat >"$tmp/bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$GH_STUB_LOG"
if [[ "$*" == 'repo view jarimustonen/taskfleet --json nameWithOwner -q .nameWithOwner' ]]; then
  printf '%s\n' jarimustonen/taskfleet
  exit 0
fi
if [[ "$*" == run\ list* ]]; then
  echo reached-exact-sha-ci-gate >&2
  exit 42
fi
echo "stub gh: unexpected arguments: $*" >&2
exit 98
STUB
chmod +x "$tmp/bin/gh"

cat >"$tmp/bin/shipshape" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$SHIPSHAPE_STUB_LOG"
if [[ "$*" == 'version --json' ]]; then
  printf '%s\n' '{"schema_version":1,"data":{"version":"0.12.2","commit":"d1d48d692707fee0d98697721e763a59e7ee3fb7","schema_version":1}}'
  exit 0
fi
if [[ "$*" == "release show $RUN_ID --json" ]]; then
  pushed_event=''
  outcome=failed
  [[ "${HELD_VARIANT:-valid}" != unexpected-event ]] || pushed_event='{"seq":25,"kind":"tag_pushed_remote","tag":"'"$TAG"'"}'
  [[ "${HELD_VARIANT:-valid}" != wrong-phase ]] || outcome=ok
  jq -n --arg run "$RUN_ID" --arg tag "$TAG" --arg bump "$BUMP_COMMIT" --arg outcome "$outcome" --argjson pushed "${pushed_event:-null}" '
    {data:{last_seq:25,state:{schema_version:6,run_id:$run,status:"in_progress",current_phase:null,applied_seq:25,
      bump:{commit:$bump},phases:[
        {phase:"bump",outcome:"ok"},{phase:"dry_run",outcome:"ok"},{phase:"build",outcome:"ok"},
        {phase:"publish",outcome:"ok"},{phase:"tag",outcome:$outcome}],
      tags:{($tag):{created_local:true,pushed_remote:false,github_release:false,github_release_delegated:false}}},
      recent_events:([{"seq":23,"kind":"phase_entered","phase":"tag"},{"seq":24,"kind":"tag_created_local","tag":$tag}]
        + (if $pushed == null then [] else [$pushed] end)
        + [{"seq":25,"kind":"phase_completed","phase":"tag","outcome":$outcome}])}}'
  exit 0
fi
echo "stub shipshape: unexpected arguments: $*" >&2
exit 99
STUB
chmod +x "$tmp/bin/shipshape"

write_checkpoint() {
  local remote="${1:-git@github.com:jarimustonen/taskfleet.git}"
  jq -n --arg run "$run_id" --arg tag "$tag" --arg bump "$bump_commit" --arg oid "$tag_oid" --arg remote "$remote" '
    {schema_version:1,run_id:$run,tag:$tag,bump_commit:$bump,
     marker:["origin",$remote,"refs/tags/"+$tag,$oid,"refs/tags/"+$tag,"0000000000000000000000000000000000000000"]}' \
    >"$tmp/common/shipshape-held-tags/$run_id.json"
}

run_case() {
  local variant="$1" validation_mode="${2:-failure}"
  rm -f "$tmp/work/local-validation-mutation" "$tmp/head-moved"
  : >"$tmp/local-validation.log"
  env -i HOME="$tmp/home" PATH="$tmp/bin" \
    HELD_VARIANT="$variant" LOCAL_VALIDATION_MODE="$validation_mode" \
    LOCAL_VALIDATION_LOG="$tmp/local-validation.log" HEAD_MOVED_MARKER="$tmp/head-moved" \
    GIT_STUB_LOG="$tmp/git.log" GH_STUB_LOG="$tmp/gh.log" \
    SHIPSHAPE_STUB_LOG="$tmp/shipshape.log" GIT_STUB_ROOT="$tmp/work" GIT_COMMON="$tmp/common" \
    BUMP_COMMIT="$bump_commit" TAG_OID="$tag_oid" TAG="$tag" RUN_ID="$run_id" \
    "$repo_root/scripts/shipshape-release.sh" resume "$run_id" >"$tmp/stdout" 2>"$tmp/stderr"
}

assert_rejected_before_validation() {
  local variant="$1"
  : >"$tmp/gh.log"
  set +e
  run_case "$variant"
  status=$?
  set -e
  [[ "$status" -ne 0 ]] || {
    echo "$variant was not rejected before local validation (status=$status)" >&2
    cat "$tmp/stderr" >&2
    exit 1
  }
  test ! -s "$tmp/local-validation.log" || {
    echo "$variant reached local validation" >&2
    exit 1
  }
  if [[ "$variant" == wrong-phase || "$variant" == unexpected-event ]]; then
    grep -F "is not the exact validated shipshape held-tag journal" "$tmp/stderr" >/dev/null || {
      echo "$variant failed for the wrong reason" >&2
      cat "$tmp/stderr" >&2
      exit 1
    }
  fi
}

write_checkpoint
set +e
run_case valid failure
status=$?
set -e
[[ "$status" -eq 1 ]] || {
  echo "local validation failure did not stop the valid held journal (status=$status)" >&2
  cat "$tmp/stderr" "$tmp/git.log" "$tmp/shipshape.log" "$tmp/gh.log" >&2
  exit 1
}
grep -F reached-local-release-validation "$tmp/stderr" >/dev/null
grep -F "local release validation failed for $bump_commit" "$tmp/stderr" >/dev/null
[[ "$(cat "$tmp/local-validation.log")" == "$bump_commit" ]]
! grep -F 'api ' "$tmp/gh.log" >/dev/null || { echo "failed validation reached authorization creation" >&2; exit 1; }

# Successful validation advances to the authorization gates. Stop there before
# the fixture can create a protected ref or publish its local-only tag.
set +e
run_case valid success
status=$?
set -e
[[ "$status" -eq 2 ]] || { echo "successful validation did not reach authorization gates" >&2; cat "$tmp/stderr" >&2; exit 1; }
grep -F reached-post-validation-authorization-gates "$tmp/stderr" >/dev/null

for mode in dirty head-change; do
  set +e
  run_case valid "$mode"
  status=$?
  set -e
  [[ "$status" -eq 2 ]] || { echo "$mode validation mutation was not rejected (status=$status)" >&2; cat "$tmp/stderr" >&2; exit 1; }
  ! grep -F reached-post-validation-authorization-gates "$tmp/stderr" >/dev/null || {
    echo "$mode validation mutation reached authorization gates" >&2
    exit 1
  }
done

rm -f "$tmp/common/shipshape-held-tags/$run_id.json"
assert_rejected_before_validation valid
write_checkpoint https://github.com/unrelated/repo.git
assert_rejected_before_validation valid
write_checkpoint
for variant in remote-present missing-local-tag moved-local-tag wrong-tag-target bad-ancestry wrong-phase unexpected-event; do
  assert_rejected_before_validation "$variant"
done

echo "release wrapper held-tag journal tests passed"
