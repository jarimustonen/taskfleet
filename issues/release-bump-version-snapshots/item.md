---
created: 2026-10-25
updated: 2026-10-25
type: bug
status: fixed
priority: normal
provenance: chat
source_ref: taskfleet:01m5rgq5gdrzaebwv70kmbr1nx/brief
originating_run: 01m5rgq5gdrzaebwv70kmbr1nx
originating_run_kind: spinoff
closed: 2026-10-25
closed_by: agent
---

# Release bump fails on version-bearing snapshots

## Description

## Description

The 0.17.0 → 0.17.1 release failed during the bump hook in Shipshape run `01M5RGN8KHHRC9XD216YP0Y77E`. Version and skill-list envelope snapshots still expected 0.17.0. Nothing was tagged or published.

The hook already sets `INSTA_UPDATE=always` and allowlists all five version-bearing snapshots, but invokes ambient `cargo`. On haapa that is a host offload wrapper. The archived test ran in `/j/src` and failed instead of updating, so the hook cannot rely on that invocation preserving either its environment or writes in the release checkout.

Evidence: `/home/jari/.local/state/hb-exec/results/755840617da4a9aaa689e290ccf6179491841575cf4199f9d35e8be8b34fd949-088fa4f4c2a9459580b2b6c0653a0e76/artifacts.tar` (stdout.log, stderr.log).

## Reproduction

Run the release bump hook after bumping the workspace version, lockfile and exact core pin to 0.17.1 on a machine whose ambient Cargo offloads tests without forwarding INSTA_UPDATE. The first version_text and skill_list_json mismatches abort the suite.

## Acceptance Criteria

- [x] The hook regenerates the five version-bearing snapshots in its own checkout using stable Cargo, independent of the host offload wrapper.
- [x] A disposable clone bumped to 0.17.1 passes the bump hook and release-mode workspace tests.
- [x] The real tree passes scripts/validate-local-release.sh; no installed taskfleet binary or skills are changed and no release is cut.

## Comments

### 2026-10-25T03:34:17Z · @agent

Verified: full scripts/validate-local-release.sh passed at 3e8714ae60e7858aa303afbde4722d03b2787ccd (gate-final.log); 1278 release tests passed, 1 skipped, doctests passed. Disposable 0.17.1 clone passed hook + release workspace suite; byte-checked all diffs as version-only. Focused self-review complete. Logs and recovery commands: validation.md and /var/tmp/agent-keep/release-bump-version-snapshots/. First gate exposed missing rustup in the stripped-PATH protocol fixture; strict lookup-only stub fixed it and the full rerun passed.
