# TODO

Session-level plan + handoff. Longer-running planning/design docs live under
`issues/<slug>/{design,plan,breakdown,…}.md`; this file points at issuectl issues
for the actual tracked work. Standing rules and canonical learnings live in the
root `AGENTS.md` (operating policy + state-integrity invariants) — this file
holds only the **active handoff** and a **compact stint archive**.

---

## 🔄 Continue here (ALOITA TÄSTÄ), 2026-09-08 (**stint complete; v0.8.2 published**)

**The regression and simplification round is delivered.** The original sweep resolved 13 issues through seven merged worktree units. Workmux now owns window display names; Taskfleet no longer overwrites project emoji prefixes. The default state root is explicit and canonical, run reads expose recorded repository identity and the actual wait outcome, preserved work has one shared observer and an explicit audited discard path, and confirmed recovery merges reconcile through the existing authoritative status fold. Bundled stint/spinoff guidance uses session-owned work and risk-proportionate review rather than requiring multi-model review for every production edit. The direction remains fewer duplicated decisions and inferred states, with stronger review where data loss, concurrency, or authority boundaries justify it.

**Publication is complete.** [Taskfleet v0.8.2](https://github.com/jarimustonen/taskfleet/releases/tag/v0.8.2) shipped from `0e03bac26eaa7033433b7f0b5523ef61adcca2c9`. Shipshape run `01M20YEEW9BQBRGTJAKYFZX2HH` is completed; both crates.io packages, all three binary targets, GitHub Release, and the canonical Homebrew formula were verified. [Distribution workflow](https://github.com/jarimustonen/taskfleet/actions/runs/34254929712) and [crates.io workflow](https://github.com/jarimustonen/taskfleet/actions/runs/34254929592) succeeded. The macOS runner completed its build in 81 seconds; most elapsed release time was local validation and corrected release-test assumptions. Earlier 0.8.0/0.8.1 attempts were abandoned before remote tag publication; never resume them.

**Release policy is already recorded in AGENTS.md and OSS-RELEASE.md.** The repository uses the validated Shipshape 0.12.2 protocol. GitHub test CI was removed on the user's instruction; the exact clean release commit passes `scripts/validate-local-release.sh` before the protected tag push. GitHub still owns publication. The final gate passed 1,100 release tests, two doctests, fmt/clippy/rustdoc, Rust 1.85, dependency checks, snapshots, identity, release fixtures, archives, and distribution/contract checks. Shared-target build-script relocation and fixture-owned changelog inputs were corrected and proven; evidence belongs in `@build-script-worktree-path` and `@protocol-fixture-release-notes`, not another troubleshooting checklist.

**Cross-repository identity verification is complete within its documented scope.** The 38-repository HEAD audit and four merged/pushed corrections are recorded in [local-head-verification.md](issues/rename-taskfleet/local-head-verification.md); the `@rename-taskfleet` epic is closed. Historical provenance and the retired tap's intentional formula migration mapping remain. Repository-local work did not install Taskfleet or its skills. At this handoff, a fresh installed `taskfleet version --json` and the installed handoff skill both report 0.8.2, superseding the earlier 0.7.1 observation.

**This session has relinquished its work.** All 15 session-owned Taskfleet spinoffs (seven regression units, four downstream wording corrections, Shipshape integration, local validation, and two release-fix units) report `landed: true`, with no preserved work. The live issuectl DAG was checked with applicable foreign-run reservations and has no malformed dependency graph or unscheduled open issue. Foreign sessions are not adopted or drained by this handoff.

**Continue from the live facts.** Start a fresh session with `jatketaan @TODO.md` and `/stint-start` when further work is wanted. Re-read the current repository policy and `issuectl dag`, reconstruct current run reservations, and select work there; this narrative contains no second schedule. No release, installation, or test-account reset remains for this stint. Avoid repeating unchanged-source validation merely as a review ritual; any change to the existing release gate itself is a separate deliberate implementation.

---

## Scheduling

Canonical scheduling lives in `issuectl` frontmatter (`lane:`, `lane_seq:`, `blocked_by:`, `collision:`). Do not maintain a markdown DAG or adjacent backlog in this file.

Use these views instead:

```bash
issuectl dag
issuectl dag --json
issuectl ls --status open
issuectl ls --status in-progress
```

`TODO.md` is only the session handoff and project notes; issue bodies and `issuectl dag` are the source of truth.

---

## Invariants + operating policy

The **7 state-integrity invariants** and the stint operating policy (release
mechanics, repository-local validation, green gate + integrated gate, hot files,
standing learnings) live in the root `AGENTS.md`. Read them before touching the reducer, the lock
layer, or `supervise/`, and before any release action.

---

## Stint archive (compact — durable facts only)

Full narratives live in git history of this file; canonical rules extracted from
these stints are in `AGENTS.md`.

- **Stint 5 (2026-08-17, v0.3.0).** Full round + release, 5 headless spinoffs (3 planned + 2 CI-red fixes), all
  first-spawn. Landed: `release-gate-on-ci` (publish-crates.yml now repeats the full main-CI gate + a tag/manifest
  version-match check before any publish step — proven live by that release), `create-idempotency-lease-recovery`
  (durable creator lease on pre-publication reservations; follow-up `recover-unkeyed-child-publication` closed
  `wontfix` on Jari's call), `config-show-layered-view` (config schema v2, layered tolerant inspection). Two CI-reds:
  `ci-red-release-mode-injection` (debug-only `TASKFLEET_TEST_*` hooks vs CI's `--release`) and
  `etxtbsy-cross-module-stub-race` (the third ETXTBSY; killed structurally by moving CI to cargo-nextest
  process-per-test rather than re-mutexing). Both are inputs to the stint-6 green-gate fix.
- **Review session (2026-08-17, after stint 4).** Fable-driven repo review + doc cleanup, parallel to the
  `add-configurable-agent` design session (design.md v2). `AGENTS.md` rewritten for consistency; `README.md` rewritten
  against 0.2.2 reality; stints 1–3 compressed into this archive. Code: `cut-plan-module` (dead 2013-line
  `taskfleet-core/src/plan.rs` removed — the breaking entry that made the next release 0.3.0) + `harness-pi-default`
  (built-in default flipped to `pi` per ADR 0001 D4). Epics `code-pipeline` + `lifecycle-architecture-review` closed.
- **Stint 4 (2026-08-17, v0.2.2).** Full round + release + a caught mistake: 4 parallel spinoffs (all first-spawn),
  v0.2.2 cut, then CI caught one fix incomplete and a 5th spinoff finished it. Landed: `pi-spinoff-batch` (staged
  atomic run-create publication — the load-bearing fix), `cli-canon-help-json` (§14, clap-derived help envelope),
  `tmux-stub-etxtbsy-flake` (took two spinoffs; its tmux-family mutex later proved too narrow — the class was killed
  structurally in stint 5 by nextest process-per-test in CI), `spinoff-skill-stale-preview-banner` +
  `skill-install-force-symlink`. Lane fix: phantom `supervise` lane merged into `lifecycle` (verified from git;
  `lifecycle` deliberately NOT split despite depth). Origin of the "DO NOT `cargo publish` locally — the tag push IS
  the publish" rule (promoted to `AGENTS.md` after v0.2.2 was luckily-unharmed published before CI reported). Triage
  sweep: 2 closed cannot-reproduce (incl. `run-wait-false-stillborn-slow-start` — did occur when filed, did not recur
  after staged-create; re-file readily), stale-pendings intake laned.
- **Stint 3 (2026-08-17, v0.2.1).** Full round + release, 3 parallel spinoffs, all landed first-spawn.
  Landed: `spinoff-report-fields-null` (report read-back surface + docs in five skills — the four "null report" bugs
  were all read-surface errors), `run-create-long-title-stillborn` (branch names bounded to workmux's 50-byte
  window-name input), `cli-canon-version-schemas` (§10: `supported_schemas` in `version`). Closed without code:
  `cli-canon-config` (already shipped in 0.2.0). This stint also exposed now-retired source-tree local-deploy rules;
  normal repository work no longer installs the tool. ADR 0011 (homebase) boundary recorded
  in `AGENTS.md`: no pi-processes dependency.
- **Stint 2 (2026-08-16, triage-only).** 39 unscheduled issues → 24 closed, 15 laned; whole queue verified against
  current code. Two-thirds of the queue was not real work: 4 "bugs" were the same report-read-surface mistake, 11 were
  LLM review-residue with template bodies pointing at gitignored `history/` files, 5 duplicates. Origin of the
  filing-bar and verify-against-running-binary rules (now in `AGENTS.md`). Queue hygiene: `issuectl doctor --fix`,
  recovered real close-dates from git. `audit-no-user-specifics` arrived and was laned (skills lane): grep of the
  shipped artifact hits 19 files, five of them bundled SKILL templates; zero hits under `crates/*/src/`.
  *Left for Jari, outside this repo:* `~/.claude/skills/triage-bugs/` had dangling symlinks after the homebase rename
  to `triage-unlaned-issues`; fix on the homebase side if not already done.
- **Stint 1 (2026-08-16, v0.2.0).** The 0.2 simplification shipped end-to-end: thin supervisor (A1 exit-status shim,
  A2 OID-based merge-transaction recovery, A6 typed outcome table, A5 `attention_required`, A3 fenced `run salvage`,
  explicit `--interactive`), the teardown work-preservation guards, `config path`/`config show`, and the kind/heuristic
  cuts. Everything is documented as invariants 5–7 + the ADR (`docs/decisions/0001-thin-supervisor-vs-harden.md`).
- **Pre-0.2 (2026-07 → 2026-08-15).** The pivot: bug-cluster analysis showed ~57% of open issues concentrated in the
  supervisor/lifecycle subsystem with one root cause (state INFERRED from `pid × pane × branch × report`), so patching
  was stopped and the `lifecycle-architecture-review` epic ran instead — three research worktrees
  (`analysis.md`/`feature-audit.md` with 717-run usage evidence/`alternatives.md`), DECISION-1 (cut/keep/reframe, with
  Jari), a facilitated design session → `design.md`, DECISION-2 (thin model) → the ADR. v0.1.5–v0.1.8 shipped along the
  way. Durable residue: (a) "a subsystem whose bugs are combinatorial needs an architecture review, not more patches";
  (b) supervisor deaths under spawn saturation — the *surfacing* half shipped in 0.1.5 and the staged-create fix (0.2.2)
  removed the main trigger; the remaining resilience work is tracked as `create-idempotency-lease-recovery` + the
  stale-pendings issue; (c) the disjoint-lanes/integrated-gate and worker-deaths-transient learnings, both promoted to
  `AGENTS.md`.

---
