---
schema_version: 2
status: approved
maturity: mvp
ecosystems: [rust]
targets:
  - {ecosystem: rust, package: taskfleet-core, registry: crates.io, adapter: cargo-publish-ci}
  - {ecosystem: rust, package: taskfleet, registry: crates.io, adapter: cargo-publish-ci}
  - {ecosystem: rust, package: taskfleet, registry: gh-releases, adapter: cargo-dist}
  - {ecosystem: rust, package: taskfleet, registry: homebrew, adapter: cargo-dist}
versioning: semver
changelog: {mode: curated, source: issuectl-trailers}
release: {model: gated, layout: single, bump_hook: "./scripts/shipshape-bump-hook.sh"}
distributions:
  - package: taskfleet
    adapter: cargo-dist
    gh_releases: true
    installers: [shell, homebrew]
    homebrew_tap: jarimustonen/homebrew-taskfleet
    platforms: [aarch64-apple-darwin, aarch64-unknown-linux-gnu, x86_64-unknown-linux-gnu]
provenance_level: keyless
dependency_bot: dependabot
health_badges: [ci, registry, license]
license: MIT
docs_site: none
---

# Taskfleet release contract

Taskfleet is one versioned Cargo workspace with two published packages:
`taskfleet-core`, then the exact-pinned `taskfleet` CLI. The same version tag
independently triggers cargo-dist's GitHub Release and canonical Homebrew legs.
The release publishes one executable, `taskfleet`.

## Release transaction

`scripts/shipshape-release.sh plan <major|minor|patch>` seals the non-mutating
plan. `scripts/shipshape-release.sh cut <plan-id>` owns version bumping, the
exact core pin, `Cargo.lock`, CHANGELOG finalization, version snapshots, bump
commit, exact-main CI gate, authorization ref, and tag push. Never invoke Cargo
publication locally, push a release tag manually, or use a bare Shipshape resume
while a tag is held locally.

The wrapper admits only Shipshape 0.12.2 build
`d1d48d692707fee0d98697721e763a59e7ee3fb7`. Shipshape owns the stored bump
plan and, after destination verification, idempotently records default-branch
advancement. The Taskfleet adapter must still hold the tag because Taskfleet's
tag starts both publishing workflows: it first advances `main` to the bump
commit, requires green `ci.yml` for that exact push SHA, and creates the
protected exact-commit authorization ref before resuming the immutable tag.
Both release workflows verify that ref. Registry versions are permanent and may
only be yanked, so a partial saga is resumed from the same immutable tag or fixed
forward with a new patch.

## Validation

Before a release:

- run the full repository green gate from `AGENTS.md`;
- run and review the complete insta snapshot loop;
- run release topology, publication, authorization, and distribution fixture
  tests;
- package the workspace with `cargo package --workspace --locked --no-verify`
  and inspect that archives contain exactly the two canonical packages;
- verify the generated cargo-dist plan contains only Taskfleet archives,
  checksums, installers, GitHub hosting, and the canonical Homebrew formula;
- verify the tree and remote `main` are clean, synchronized, and exact-SHA green.

`.github/workflows/publish-crates.yml` owns crates.io. cargo-dist owns the
generated `.github/workflows/release.yml`; regenerate it rather than editing it
by hand. `scripts/shipshape-release.sh verify <run-id>` is the read-only
cross-leg reconciliation surface.
