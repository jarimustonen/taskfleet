---
created: 2026-09-08
updated: 2026-09-08
type: bug
reporter: jari
status: in-progress
priority: high
epic: rename-taskfleet
lane: state-home
collision: [crates/taskfleet/src/home.rs]
---

# Restore the single canonical default state root

# Restore the single canonical default state root

## Observed

During the 2026-09-08 user-requested regression and simplification round, `bash scripts/check-canonical-identity.sh` fails on main 02b8145. The resolver in `crates/taskfleet/src/home.rs` once again discovers a retired-name home and classifies two directories as meaningful/incidental; `config/mod.rs`, state-home tests, and the historical dual-root issue also contain retired identity references. Commit 934a296 reintroduced this after clean-break a357c0c.

## Decision and scope

The current user-supplied AGENTS.md and ADR 0002 require one default home, no alias/adopter, and no state mover. The current user explicitly requested fixing real regressions and removing unnecessary complexity. Restore the default to `~/.taskfleet`; nondefault locations remain accessible through explicit `TASKFLEET_HOME`. This corrects maintained source only: do not install a binary, move/delete user state, or edit another repository.

The earlier dual-root issue described a real visibility problem. Retain its truthful history in neutral terms and document that implicit discovery is superseded by the clean-break contract; do not claim user data was migrated or deleted. Preserve process-frozen resolution, explicit overrides, internal-worker binding, strict validation, and read-only command purity. Remove root-discovery/classification helpers and enum variants once unused. Do not solve this by exempting or disabling the identity gate.

## Acceptance criteria

- Default resolution depends only on HOME and the canonical suffix; alternate directories cannot change it.
- Explicit TASKFLEET_HOME and worker/supervisor root binding continue to work.
- No real user state or installed artifacts are changed.
- Obsolete classifier code/tests/prose are removed or rewritten around the actual one-root contract, using neutral nondefault fixture paths.
- The canonical identity inventory and full Rust/snapshot/doc gates pass.
- The canonical inventory is included in routine validation guidance so a green Rust gate cannot hide this regression again.
