---
created: 2026-09-25
updated: 2026-09-25
type: bug
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-ops
source_ref: agent:homebase-ops/reporter:jari/id:taskfleet-macos-dist-runner-36021035698
---

# Self-hosted macOS release installs cargo-dist in persistent Cargo bin

## Description

Self-hosted macOS release installs cargo-dist in persistent Cargo bin

Taskfleet release run 36021035698 (2026-09-24) wrote cargo-dist 0.33.0 into the persistent shared Cargo bin on self-hosted runner hauis. The build-local-artifacts (aarch64-apple-darwin) job 107705797379 log proves the writer:
2026-09-24T15:33:52Z Run curl .../cargo-dist-installer.sh | sh
2026-09-24T15:33:54.8679610Z installing to /Users/jari/.cargo/bin
2026-09-24T15:33:54.8844040Z   dist
2026-09-24T15:33:54.8938540Z everything's installed!
At 15:33:54Z the runner created ~/.cargo/bin/dist inode 314103856 and cargo-dist alias inode 314103864. The binary SHA-256 ea012ef017eb2fd11570fdb4fa9cee893a2644c4c53144b1ef3663e77b3f2ba2 matches earlier residue. Cargo trackers did not register it. Homebase fleet doctor package/software-clis turned pending. This occurred ~40 min after the prior residue had been quarantined and doctor had zero pending, ruling out the old occurrence as its cause.
Source: .github/workflows/release.yml build-local-artifacts uses matrix.install_dist.run unmodified on the self-hosted macOS runner. Expected: macOS job installs dist in a unique RUNNER_TEMP directory and invokes precisely that copy, never touching ~/.cargo/bin; Linux paths/cache retain their current behavior. Mirror the proven isolation + real-release before/after inventory from Glasspad v0.18.4; add a macOS job check of command -v dist and persistent bin baseline/diff, and test the generated workflow so regeneration preserves the override. Do not delete the user's current binaries from the workflow. New occurrence was operator-quarantined after byte-level evidence capture at /Volumes/JARI-EXT/backup/cargo-dist-taskfleet-release-2026-09-24-run36021035698/; this issue remains valid until a real Taskfleet release proves the fix.
