---
created: 2026-09-24
updated: 2026-09-24
type: bug
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:homebase-wrapup-taskfleet-gate-tmpfs-2026-09-23
---

# Release gate exhausts tmpfs on Haapa

## Description

Release gate exhausts tmpfs on Haapa

## Observed
On Haapa, from a clean Taskfleet checkout, `scripts/validate-local-release.sh` passed fmt, clippy, release nextest, doctests, rustdoc, cargo check, cargo deny and most release fixtures, then failed inside `scripts/test-shipshape-release-current-protocol.sh` while compiling the fixture's fresh Cargo target under `${TMPDIR:-/tmp}`. The failure was `rustc-LLVM ERROR: IO failure on output stream: Disk quota exceeded` (OS error 122); `/tmp` is a 7.4 GiB tmpfs, while the root filesystem had >260 GiB free. This prevented a required green release gate for an otherwise passing fix.

## Expected
A normal validation run on the supported Haapa development host should not fail merely because the fixture's isolated multi-GiB target is allocated on tmpfs. Keep the test isolated and clean up temporary state, but provide a safe disk-backed default or detect insufficient free space and give an actionable prerequisite before compiling.

## Reproduction / workaround
`scripts/validate-local-release.sh` with default `TMPDIR` failed at the protocol test. The same full gate passed with `TMPDIR=/home/jari/.cache/taskfleet-release-fixtures CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0` (a disposable disk-backed directory); no source changes were needed. Relevant script: `scripts/test-shipshape-release-current-protocol.sh` (`mktemp -d "${TMPDIR:-/tmp}/shipshape-current-protocol.XXXXXX"`).
