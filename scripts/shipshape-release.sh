#!/usr/bin/env bash
# Safe wrapper around the validated Shipshape 0.12.2 resumable protocol.
# It pauses a bump cut at tag push, advances main to the bump commit, waits for
# CI on that exact SHA, then resumes the journalled cut.
set -euo pipefail

readonly shipshape_0_12_2_commit="d1d48d692707fee0d98697721e763a59e7ee3fb7"
readonly topology_rel="release/taskfleet-release.json"
expected_repo=""
readonly -a never_resume_runs=(
  "01M0FD8FSTMGYG8YTV92WMWC87"
  "01M0FG88NAKBJ7Y3QNFZEHRM4K"
)
shipshape_version=""
run_id=""
tag=""
bump_commit=""

usage() {
  cat >&2 <<'EOF'
usage:
  scripts/shipshape-release.sh plan <major|minor|patch>
  scripts/shipshape-release.sh cut <plan-id>
  scripts/shipshape-release.sh resume <run-id>
  scripts/shipshape-release.sh verify <run-id>
EOF
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || { echo "required command not found: $1" >&2; exit 1; }
}

load_release_topology() {
  expected_repo="$(./scripts/validate-release-topology.sh)"
}

assert_cut_activated() {
  if ! "$repo_root/scripts/verify-release-activation.sh" >/dev/null; then
    local activation
    activation="$(jq -er .activation "$repo_root/$topology_rel")"
    echo "release cut activation is $activation; ADR 0002 R8-R10 gates must complete before a tag can be cut" >&2
    exit 2
  fi
}

validate_contract_targets() {
  local contract_json="$1"
  jq -e '
    [.data.targets[] | (.package + ":" + .registry + ":" + .adapter)] == [
      "taskfleet-core:crates.io:cargo-publish-ci",
      "taskfleet:crates.io:cargo-publish-ci",
      "taskfleet:gh-releases:cargo-dist",
      "taskfleet:homebrew:cargo-dist"
    ] and
    .data.schema_version == 2 and
    .data.release.bump_hook == "./scripts/shipshape-bump-hook.sh" and
    [.data.distributions[] | {
      package, adapter, gh_releases, installers, homebrew_tap, platforms
    }] == [{
      package: "taskfleet",
      adapter: "cargo-dist",
      gh_releases: true,
      installers: ["shell", "homebrew"],
      homebrew_tap: "jarimustonen/homebrew-taskfleet",
      platforms: [
        "aarch64-apple-darwin",
        "aarch64-unknown-linux-gnu",
        "x86_64-unknown-linux-gnu"
      ]
    }]
  ' <<<"$contract_json" >/dev/null || {
    echo "approved Shipshape contract does not match the admitted Taskfleet topology" >&2
    exit 2
  }
}

require_supported_shipshape() {
  local version_json commit
  version_json="$(shipshape version --json)"
  shipshape_version="$(jq -er '
    if .schema_version == 1 and .data.schema_version == 1
    then .data.version
    else error("unsupported version envelope")
    end
  ' <<<"$version_json")"
  commit="$(jq -er '.data.commit // ""' <<<"$version_json")"
  case "$shipshape_version:$commit" in
    "0.12.2:$shipshape_0_12_2_commit") ;;
    *)
      if [[ "$shipshape_version" == 0.12.2 ]]; then
        echo "shipshape $shipshape_version is not the exact build validated for the held-tag protocol; found commit ${commit:-<missing>}" >&2
      else
        echo "validated Shipshape 0.12.2 required; found $shipshape_version (revalidate the pre-tag protocol before accepting another version)" >&2
      fi
      exit 1
      ;;
  esac
}

assert_run_may_resume() {
  local candidate="$1" blocked
  [[ "$candidate" =~ ^[0-9A-HJKMNP-TV-Z]{26}$ ]] || {
    echo "invalid release run id: $candidate" >&2
    exit 2
  }
  for blocked in "${never_resume_runs[@]}"; do
    [[ "$candidate" != "$blocked" ]] || {
      echo "release run $candidate is permanently abandoned and must never be resumed" >&2
      exit 2
    }
  done
}

validate_plan_id() {
  [[ "$1" =~ ^[0-9a-f]{64}$ ]] || {
    echo "invalid release plan id: $1" >&2
    exit 2
  }
}

validate_level() {
  case "$1" in major|minor|patch) ;; *) usage ;; esac
}

canonical_github_repo() {
  sed -E 's#^(git@github.com:|https://github.com/|ssh://git@github.com/)##; s#\.git$##' <<<"$1"
}

assert_repo_identity() {
  local origin_repo push_repo gh_repo
  origin_repo="$(canonical_github_repo "$(git remote get-url origin)")"
  push_repo="$(canonical_github_repo "$(git remote get-url --push --all origin)")"
  gh_repo="$(gh repo view "$expected_repo" --json nameWithOwner -q .nameWithOwner)"
  [[ "$origin_repo" == "$expected_repo" && "$push_repo" == "$expected_repo" && "$gh_repo" == "$expected_repo" ]] || {
    echo "release repository mismatch: origin=$origin_repo push=$push_repo gh=$gh_repo expected=$expected_repo" >&2
    exit 1
  }
}

show_run() {
  shipshape release show "$1" --json
}

read_run_coordinates() {
  local show_json="$1"
  bump_commit="$(jq -er '.data.state.bump.commit' <<<"$show_json")"
  tag="$(jq -er '
    .data.state.tags | keys |
    if length == 1 then .[0] else error("expected exactly one release tag") end
  ' <<<"$show_json")"
  [[ "$bump_commit" =~ ^[0-9a-f]{40}$ ]] || {
    echo "journal contains an invalid bump commit: $bump_commit" >&2
    exit 2
  }
  [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$ ]] || {
    echo "journal contains an invalid release tag: $tag" >&2
    exit 2
  }
  git cat-file -e "$bump_commit^{commit}" 2>/dev/null || {
    echo "journalled bump commit is unavailable locally: $bump_commit" >&2
    exit 2
  }
  [[ "$(git rev-parse --verify "refs/tags/$tag^{commit}")" == "$bump_commit" ]] || {
    echo "local release tag $tag does not point at journalled bump commit $bump_commit" >&2
    exit 2
  }
}

validate_bump_tree() {
  local version="${tag#v}" workspace_version core_pin
  workspace_version="$(awk -F'"' '
    /^\[workspace\.package\]/ { in_package=1; next }
    /^\[/ { in_package=0 }
    in_package && /^version[[:space:]]*=/ { print $2; exit }
  ' Cargo.toml)"
  core_pin="$(sed -nE 's/^taskfleet-core = \{[^}]*version = "=([^"]+)".*/\1/p' crates/taskfleet/Cargo.toml)"
  [[ "$workspace_version" == "$version" && "$core_pin" == "$version" ]] || {
    echo "bump tree mismatch: tag=$version workspace=$workspace_version taskfleet-core-pin=$core_pin" >&2
    exit 2
  }
  grep -F "## [$version] - " CHANGELOG.md | grep -Eq '^## \[[0-9]+\.[0-9]+\.[0-9]+\] - [0-9]{4}-[0-9]{2}-[0-9]{2}$' || {
    echo "CHANGELOG is not finalized with a dated section for $version" >&2
    exit 2
  }
  awk -v version="$version" '
    /^name = "taskfleet-core"$/ || /^name = "taskfleet"$/ { wanted=1; next }
    wanted && /^version = / {
      seen++
      if ($0 != "version = \"" version "\"") bad=1
      wanted=0
    }
    END { exit !(seen == 2 && !bad) }
  ' Cargo.lock || {
    echo "Cargo.lock does not carry both workspace packages at $version" >&2
    exit 2
  }
  ./scripts/check-version-snapshots.sh
  test -z "$(git status --porcelain)" || { echo "bump tree must be clean" >&2; exit 2; }
}

advance_main_to_bump() {
  local base origin_main
  base="$(git rev-parse HEAD)"
  git merge-base --is-ancestor "$base" "$bump_commit" || {
    echo "bump commit is not a descendant of local main" >&2
    exit 2
  }
  git fetch origin +refs/heads/main:refs/remotes/origin/main
  origin_main="$(git rev-parse refs/remotes/origin/main)"
  [[ "$origin_main" == "$base" || "$origin_main" == "$bump_commit" ]] || {
    echo "origin/main moved to $origin_main during the release; reconcile deliberately" >&2
    exit 1
  }
  if [[ "$base" != "$bump_commit" ]]; then
    git merge --ff-only "$bump_commit"
  fi
  validate_bump_tree
  if [[ "$origin_main" != "$bump_commit" ]]; then
    # Explicitly disable follow-tags: the held local release tag must not ride the
    # branch push before CI. A normal non-force push rejects any concurrent move.
    git -c push.followTags=false push origin HEAD:refs/heads/main
  fi
}

wait_for_exact_main_ci() {
  local sha="$1" id="" run_json
  for ((attempt = 0; attempt < 60; attempt++)); do
    id="$(gh run list -R "$expected_repo" --workflow ci.yml --branch main --commit "$sha" --event push --limit 1 --json databaseId -q '.[0].databaseId')"
    test -n "$id" && test "$id" != null && break
    sleep 5
  done
  test -n "$id" && test "$id" != null || { echo "no main CI run for $sha" >&2; exit 1; }
  run_json="$(gh run view -R "$expected_repo" "$id" --json headSha,headBranch,event)"
  jq -e --arg sha "$sha" '
    .headSha == $sha and .headBranch == "main" and .event == "push"
  ' <<<"$run_json" >/dev/null || {
    echo "GitHub run $id does not attest exact main SHA $sha" >&2
    exit 2
  }
  if ! gh run watch -R "$expected_repo" "$id" --exit-status; then
    echo "main CI failed for $sha; release $run_id remains untagged remotely" >&2
    exit 1
  fi
}

remote_tag_commit() {
  git ls-remote --tags origin "refs/tags/$tag" "refs/tags/$tag^{}" |
    awk '{ if (substr($2, length($2)-2) == "^{}") peeled=$1; else direct=$1 }
         END { print (peeled != "" ? peeled : direct) }'
}

assert_remote_tag_absent() {
  local remote_tag
  remote_tag="$(remote_tag_commit)"
  test -z "$remote_tag" || {
    echo "remote tag $tag already exists at $remote_tag; the pre-tag gate can no longer be established" >&2
    echo "publishing may be underway; inspect run $run_id and do not retag or publish manually" >&2
    exit 2
  }
}

release_authorization_ref() {
  printf 'refs/heads/taskfleet-release-authorizations/%s\n' "$tag"
}

record_release_authorization() {
  local ref ref_name response remote_ref remote_oid
  ref="$(release_authorization_ref)"
  git check-ref-format "$ref" >/dev/null || {
    echo "invalid release authorization ref: $ref" >&2
    exit 2
  }
  ref_name="${ref#refs/heads/}"
  if ! response="$(gh api "repos/$expected_repo/git/ref/heads/$ref_name" 2>/dev/null)"; then
    # GitHub's create-ref API is atomic and returns 422 if another writer wins;
    # unlike a normal branch push it can never fast-forward an existing receipt.
    response="$(gh api --method POST "repos/$expected_repo/git/refs" \
      -f ref="$ref" -f sha="$bump_commit")" || {
      echo "could not create release authorization $ref" >&2
      exit 2
    }
  fi
  remote_ref="$(jq -er .ref <<<"$response")"
  remote_oid="$(jq -er '.object.sha' <<<"$response")"
  [[ "$remote_ref" == "$ref" && "$remote_oid" == "$bump_commit" ]] || {
    echo "release authorization ${remote_ref:-<missing>} points at ${remote_oid:-<missing>}, expected $ref at $bump_commit" >&2
    exit 2
  }
}

held_checkpoint_path() {
  # The wrapper-owned namespace moved only after all legacy held cuts were
  # drained; engine plans and journals remain in the permanent ossctl namespace.
  local git_common_path git_common
  [[ "$run_id" =~ ^[0-9A-Za-z][0-9A-Za-z._-]*$ ]] || {
    echo "invalid release run id: $run_id" >&2
    exit 2
  }
  git_common_path="$(git rev-parse --git-common-dir)" || {
    echo "cannot locate the Git common directory" >&2
    exit 2
  }
  git_common="$(cd "$git_common_path" && pwd -P)" || {
    echo "cannot resolve the Git common directory: $git_common_path" >&2
    exit 2
  }
  printf '%s/shipshape-held-tags/%s.json\n' "$git_common" "$run_id"
}

assert_held_journal() {
  local show_json="$1"
  jq -e --arg tag "$tag" '
    .data.state.schema_version == 6 and
    .data.state.status == "in_progress" and
    (.data.state | has("current_phase")) and .data.state.current_phase == null and
    ([.data.state.phases[] | {phase, outcome}]) == [
      {"phase":"bump","outcome":"ok"},
      {"phase":"dry_run","outcome":"ok"},
      {"phase":"build","outcome":"ok"},
      {"phase":"publish","outcome":"ok"},
      {"phase":"tag","outcome":"failed"}
    ] and
    (.data.state.tags | keys) == [$tag] and
    .data.state.tags[$tag].created_local == true and
    .data.state.tags[$tag].pushed_remote == false and
    .data.state.tags[$tag].github_release == false and
    .data.state.tags[$tag].github_release_delegated == false and
    .data.last_seq == .data.state.applied_seq and
    ([.data.recent_events[] |
      if .kind == "phase_entered" and .phase == "tag" then "entered:tag"
      elif .kind == "tag_created_local" then "created:" + .tag
      elif .kind == "phase_completed" and .phase == "tag" then "completed:tag:" + .outcome
      elif (.phase? == "tag") or (.kind | startswith("tag_")) or (.kind | startswith("github_release_"))
        then "unexpected:" + .kind
      else empty end
    ]) == ["entered:tag", "created:" + $tag, "completed:tag:failed"] and
    ([.data.recent_events[-3:][] | {kind, phase, tag, outcome}]) == [
      {kind:"phase_entered", phase:"tag", tag:null, outcome:null},
      {kind:"tag_created_local", phase:null, tag:$tag, outcome:null},
      {kind:"phase_completed", phase:"tag", tag:null, outcome:"failed"}
    ] and
    .data.recent_events[-1].kind == "phase_completed" and
    .data.recent_events[-1].phase == "tag" and
    .data.recent_events[-1].outcome == "failed"
  ' <<<"$show_json" >/dev/null || {
    echo "run $run_id is not the exact validated shipshape held-tag journal" >&2
    jq -c '{status:.data.state.status,current_phase:.data.state.current_phase,
      phases:[.data.state.phases[]|{phase,outcome}],tags:.data.state.tags,
      tag_events:[.data.recent_events[]|select((.phase?=="tag") or (.kind|startswith("tag_")) or (.kind|startswith("github_release_")))]}' \
      <<<"$show_json" >&2 || true
    exit 2
  }
}

assert_hook_marker() {
  local marker="$1" tag_oid tag_commit line
  local -a lines=()
  while IFS= read -r line || [[ -n "$line" ]]; do
    lines[${#lines[@]}]="$line"
  done <"$marker"
  [[ ${#lines[@]} -eq 6 ]] || {
    echo "pre-push marker must contain exactly six fields" >&2
    exit 2
  }
  [[ "${lines[0]}" == origin &&
     "$(canonical_github_repo "${lines[1]}")" == "$expected_repo" &&
     "${lines[2]}" == "refs/tags/$tag" &&
     "${lines[4]}" == "refs/tags/$tag" &&
     "${lines[5]}" =~ ^0+$ && ${#lines[5]} -eq ${#lines[3]} ]] || {
    echo "pre-push marker does not attest the expected new-tag push to origin" >&2
    exit 2
  }
  tag_oid="$(git rev-parse --verify "refs/tags/$tag")" || {
    echo "local release tag $tag is absent or invalid" >&2
    exit 2
  }
  tag_commit="$(git rev-parse --verify "refs/tags/$tag^{commit}")" || {
    echo "local release tag $tag does not resolve to a commit" >&2
    exit 2
  }
  [[ "${lines[3]}" == "$tag_oid" && "$tag_commit" == "$bump_commit" ]] || {
    echo "pre-push marker/local tag coordinates do not match journalled bump commit $bump_commit" >&2
    exit 2
  }
}

record_held_checkpoint() {
  local marker="$1" checkpoint_dir checkpoint_tmp held_checkpoint
  held_checkpoint="$(held_checkpoint_path)"
  checkpoint_dir="$(dirname "$held_checkpoint")"
  mkdir -p "$checkpoint_dir"
  chmod 700 "$checkpoint_dir"
  checkpoint_tmp="$(mktemp "$checkpoint_dir/.${run_id}.XXXXXX")"
  chmod 600 "$checkpoint_tmp"
  jq -n \
    --arg run_id "$run_id" --arg tag "$tag" --arg bump_commit "$bump_commit" \
    --arg marker_name "$(sed -n '1p' "$marker")" \
    --arg marker_remote "$(sed -n '2p' "$marker")" \
    --arg marker_local_ref "$(sed -n '3p' "$marker")" \
    --arg marker_local_oid "$(sed -n '4p' "$marker")" \
    --arg marker_remote_ref "$(sed -n '5p' "$marker")" \
    --arg marker_remote_oid "$(sed -n '6p' "$marker")" \
    '{schema_version:1, run_id:$run_id, tag:$tag, bump_commit:$bump_commit,
      marker:[$marker_name,$marker_remote,$marker_local_ref,$marker_local_oid,$marker_remote_ref,$marker_remote_oid]}' \
    >"$checkpoint_tmp"
  mv "$checkpoint_tmp" "$held_checkpoint"
}

assert_recorded_checkpoint() {
  local checkpoint
  checkpoint="$(held_checkpoint_path)"
  jq -e --arg run_id "$run_id" --arg tag "$tag" --arg bump_commit "$bump_commit" '
    .schema_version == 1 and .run_id == $run_id and .tag == $tag and .bump_commit == $bump_commit and
    (.marker | type == "array" and length == 6)
  ' "$checkpoint" >/dev/null 2>&1 || {
    echo "run $run_id has no valid wrapper-recorded pre-push hold evidence" >&2
    exit 2
  }
  assert_hook_marker <(jq -r '.marker[]' "$checkpoint")
}

resume_after_gate() {
  local show_json remote_tag pushed_remote
  assert_run_may_resume "$run_id"
  show_json="$(show_run "$run_id")"
  branch="$(git symbolic-ref --quiet --short HEAD)" || { echo "release resume must run on a branch" >&2; exit 1; }
  [[ "$branch" == main ]] || { echo "release resume must run on main (found $branch)" >&2; exit 1; }
  read_run_coordinates "$show_json"
  pushed_remote="$(jq -r --arg tag "$tag" '.data.state.tags[$tag].pushed_remote' <<<"$show_json")"
  [[ "$pushed_remote" == true || "$pushed_remote" == false ]] || {
    echo "run $run_id has an invalid pushed_remote tag state" >&2
    exit 2
  }

  if [[ "$pushed_remote" == false ]]; then
    assert_held_journal "$show_json"
    assert_recorded_checkpoint
    assert_remote_tag_absent
    advance_main_to_bump
    git fetch origin +refs/heads/main:refs/remotes/origin/main
    [[ "$(git rev-parse HEAD)" == "$bump_commit" && "$(git rev-parse refs/remotes/origin/main)" == "$bump_commit" ]] || {
      echo "local and remote main must both equal journalled bump commit $bump_commit" >&2
      exit 1
    }
    validate_bump_tree
    wait_for_exact_main_ci "$bump_commit"
    git fetch origin +refs/heads/main:refs/remotes/origin/main
    [[ "$(git rev-parse refs/remotes/origin/main)" == "$bump_commit" ]] || {
      echo "origin/main advanced after exact-SHA CI; release $run_id remains untagged" >&2
      exit 1
    }
    assert_recorded_checkpoint
    assert_repo_identity
    assert_run_may_resume "$run_id"
    assert_remote_tag_absent
    assert_cut_activated
    "$repo_root/scripts/verify-release-github-policy.sh" >/dev/null
    record_release_authorization
    assert_remote_tag_absent
    if ! shipshape release resume "$run_id" --json; then
      remote_tag="$(remote_tag_commit)"
      if [[ -z "$remote_tag" ]]; then
        echo "release authorization is recorded but $tag is still absent; only scripts/shipshape-release.sh resume $run_id may reconcile this coordinate" >&2
      else
        echo "release tag $tag may already be public; only run $run_id may be resumed or verified" >&2
      fi
      exit 1
    fi
  else
    remote_tag="$(remote_tag_commit)"
    [[ "$remote_tag" == "$bump_commit" ]] || {
      echo "remote tag $tag points at ${remote_tag:-<missing>}, expected $bump_commit" >&2
      exit 2
    }
    # The irreversible boundary is already crossed. Continue only this journal;
    # never create or push a replacement tag.
    assert_run_may_resume "$run_id"
    shipshape release resume "$run_id" --json
  fi

  remote_tag="$(remote_tag_commit)"
  [[ "$remote_tag" == "$bump_commit" ]] || {
    echo "remote tag $tag points at ${remote_tag:-<missing>}, expected CI-validated $bump_commit" >&2
    exit 2
  }
  shipshape release verify "$run_id" --json
  rm -f "$(held_checkpoint_path)"
}

require_command git
repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

require_command jq
load_release_topology

command="${1:-}"
case "$command" in
  plan|resume|verify) ;;
  cut) assert_cut_activated ;;
  *) usage ;;
esac

require_command shipshape
require_supported_shipshape

case "$command" in
  plan)
    [[ $# -eq 2 ]] || usage
    validate_level "$2"
    shipshape release list --json | jq -e '
      .data.in_flight_count == 0 and (.data.unreadable | length) == 0
    ' >/dev/null || { echo "an active or unreadable release run must be reconciled before planning" >&2; exit 1; }
    contract_json="$(shipshape contract show --json --require-approved)"
    validate_contract_targets "$contract_json"
    shipshape contract validate --json >/dev/null
    shipshape audit --json | jq -e '[.data.gaps[] | select(.severity == "blocking")] | length == 0' >/dev/null || {
      echo "blocking OSS readiness gaps prevent a release" >&2
      exit 1
    }
    test -z "$(git status --porcelain)" || { echo "working tree must be clean" >&2; exit 1; }
    # Prove the current exact graph is packageable before asking Shipshape to
    # seal the proposed bump. The engine owns the future-version bump plan; these
    # credential-free archives are a separate source-layout/pin proof.
    ./scripts/publish-crates.sh package >/dev/null
    exec shipshape release plan --bump "$2" --json
    ;;

  verify)
    [[ $# -eq 2 ]] || usage
    exec shipshape release verify "$2" --json
    ;;

  resume)
    [[ $# -eq 2 ]] || usage
    run_id="$2"
    assert_run_may_resume "$run_id"
    require_command gh
    assert_repo_identity
    resume_after_gate
    ;;

  cut)
    [[ $# -eq 2 ]] || usage
    plan_id="$2"
    validate_plan_id "$plan_id"
    require_command gh
    assert_repo_identity

    test -n "$plan_id" || usage
    test -z "$(git status --porcelain)" || { echo "working tree must be clean" >&2; exit 1; }
    branch="$(git symbolic-ref --quiet --short HEAD)" || { echo "release cut must run on a branch" >&2; exit 1; }
    [[ "$branch" == main ]] || { echo "release cut must run on main (found $branch)" >&2; exit 1; }
    [[ "$(git config --bool --get push.followTags || echo false)" != true ]] || {
      echo "push.followTags=true can leak the held release tag; unset it before cutting" >&2
      exit 1
    }

    git fetch origin +refs/heads/main:refs/remotes/origin/main
    [[ "$(git rev-parse HEAD)" == "$(git rev-parse refs/remotes/origin/main)" ]] || {
      echo "main must exactly match origin/main before the cut" >&2
      exit 1
    }
    base_commit="$(git rev-parse HEAD)"

    list_json="$(shipshape release list --json)"
    jq -e '.data.in_flight_count == 0 and (.data.unreadable | length) == 0' <<<"$list_json" >/dev/null || {
      echo "an active or unreadable release run must be reconciled before cutting" >&2
      exit 1
    }

    git_common="$(cd "$(git rev-parse --git-common-dir)" && pwd -P)"
    hooks="$(mktemp -d "$git_common/shipshape-pretag.XXXXXX")"
    marker="$hooks/tag-push-blocked"
    probe_tag="v0.0.0-shipshape-pretag-probe"
    cleanup() {
      git tag -d "$probe_tag" >/dev/null 2>&1 || true
      rm -rf "$hooks"
    }
    trap cleanup EXIT
    cat >"$hooks/pre-push" <<'HOOK'
#!/usr/bin/env bash
set -euo pipefail
while read -r local_ref local_oid remote_ref remote_oid; do
  if [[ "$local_ref" == refs/tags/v* || "$remote_ref" == refs/tags/v* ]]; then
    printf '%s\n' "$1" "$2" "$local_ref" "$local_oid" "$remote_ref" "$remote_oid" >"$SHIPSHAPE_PRETAG_MARKER"
    echo "release tag held locally until main CI is green on its exact commit" >&2
    exit 75
  fi
done
exit 0
HOOK
    chmod +x "$hooks/pre-push"

    # Prove Git resolves the absolute hooksPath on a real (local-only) tag push.
    git init --quiet --bare "$hooks/probe.git"
    git tag "$probe_tag" HEAD
    if SHIPSHAPE_PRETAG_MARKER="$marker" \
      GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.hooksPath GIT_CONFIG_VALUE_0="$hooks" \
      git push "$hooks/probe.git" "refs/tags/$probe_tag" >/dev/null 2>&1; then
      echo "pre-push safety hook did not reject a version tag" >&2
      exit 2
    fi
    test -s "$marker" || { echo "pre-push safety hook did not run on a real Git push" >&2; exit 2; }
    rm -f "$marker"
    git tag -d "$probe_tag" >/dev/null
    rm -rf "$hooks/probe.git"

    # Shipshape 0.12.2 reads the bump level from its authenticated stored plan.
    # Do not duplicate or reinterpret that plan in this adapter.
    cut_args=(release cut --plan "$plan_id" --json)

    if SHIPSHAPE_PRETAG_MARKER="$marker" \
      GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.hooksPath GIT_CONFIG_VALUE_0="$hooks" \
      shipshape "${cut_args[@]}"; then
      echo "safety stop failed: release cut passed the tag boundary before exact-SHA CI" >&2
      exit 2
    fi
    test -s "$marker" || {
      echo "cut failed before the pre-tag checkpoint; inspect shipshape output and release list" >&2
      exit 1
    }

    list_json="$(shipshape release list --json)"
    run_id="$(jq -er --arg plan "$plan_id" '
      [.data.runs[] | select(.plan_id == $plan and .in_flight)] |
      if length == 1 then .[0].run_id else error("expected one in-flight run for plan") end
    ' <<<"$list_json")"
    show_json="$(show_run "$run_id")"
    read_run_coordinates "$show_json"
    assert_hook_marker "$marker"
    assert_remote_tag_absent
    assert_held_journal "$show_json"
    git merge-base --is-ancestor "$base_commit" "$bump_commit" || {
      echo "journalled bump commit is not descended from sealed main" >&2
      exit 2
    }
    record_held_checkpoint "$marker"

    advance_main_to_bump
    resume_after_gate
    ;;

  *) usage ;;
esac
