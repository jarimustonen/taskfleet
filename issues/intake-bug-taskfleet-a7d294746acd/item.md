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

<!-- intakectl:analysis:start job:9491b440-49b6-4ec1-91c1-e0436c65501e generation:0 -->
## Triage analysis

**Classification: confirmed environmental/resource-placement bug.** The release gate's Shipshape protocol fixture creates a large, isolated Cargo target beneath a temporary directory. `scripts/test-shipshape-release-current-protocol.sh` allocates its fixture with `mktemp -d "${TMPDIR:-/tmp}/shipshape-current-protocol.XXXXXX"` and sets `TMPDIR="$tmp"` for the fixture's Cargo commands. Consequently, unless the caller overrides `TMPDIR`, the nested Cargo build target resides on `/tmp`; on Haapa that filesystem is a 7.4 GiB tmpfs, which can exhaust despite ample root-disk space. The observed LLVM disk-quota error and successful full-gate rerun with disk-backed `TMPDIR` establish the cause and a workaround.

This is limited to the real-protocol fixture near the end of `scripts/validate-local-release.sh`; earlier Rust tests may pass before the gate reaches that fixture. The script's cleanup trap removes the temporary fixture on exit, but cleanup cannot prevent peak-space exhaustion during compilation. The reproducible remedy is to put this fixture's scratch directory on a disk-backed location by default (or choose a location based on available space), while retaining an explicit caller override and cleanup behavior. Any sizing/preflight should account for the nested Cargo target rather than only the fixture's initial directory. The separate dist-plan temporary file in `validate-local-release.sh` is not the reported multi-GiB allocation.

The issue report supplies a host reproduction, exact error, relevant allocation line, and a successful workaround; no additional bug-analysis work is needed to substantiate it.
<!-- intakectl:analysis:end job:9491b440-49b6-4ec1-91c1-e0436c65501e generation:0 -->
