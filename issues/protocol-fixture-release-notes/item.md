---
created: 2026-09-08
updated: 2026-09-08
type: bug
status: open
priority: high
lane: release-toolchain
---

# Isolate protocol fixture from finalized release notes

## Description

The exact 0.8.1 gate at 144ebfd passed 1100 nextest tests, 2 doctests, rustdoc, MSRV and dependency checks, then test-shipshape-release-current-protocol.sh failed because its disposable checkout inherited the now-empty production Unreleased section. Shipshape correctly rejected the fixture bump with no authored, fragment, or trailer-derived notes. Seed fixture-owned release notes before sealing its fixture plan and surface the cut error instead of a silent grep failure. Verify against finalized HEAD and reuse the already-green unchanged Rust evidence; do not repeat Rust builds for this shell-only fix.
