# TODO

Session-level plan + handoff. Longer-running planning/design docs live under
`issues/<slug>/{design,plan,breakdown,…}.md`; this file points at issuectl issues
for the actual tracked work. Standing rules and canonical learnings live in the
root `AGENTS.md` (operating policy + state-integrity invariants) — this file
holds only the **active handoff** and a **compact stint archive**.

---

## 🔄 Continue here (ALOITA TÄSTÄ), 2026-09-27 (**round delivered; interactive portfolio run awaits Jari**)

This Taskfleet round delivered two approved issues and one direct follow-up. The bundled `/stint-start` guidance now chooses aggregate waiting only when every worker must finish before any result is useful, or independent harness-notified waits when early results matter (`intake-feature-taskfleet-6c36d6a57f68`, closed). The real Shipshape protocol fixture defaults its large scratch/build target to disk-backed `/var/tmp` while honoring caller `TMPDIR`, and tests cover both placements (`intake-bug-taskfleet-a7d294746acd`, fixed). The first attempt stopped on missing persistent Haapa prerequisites; a retry harvested its implementation, ran the full gate with disposable prerequisites, and landed. Jari explicitly authorized discarding the failed original's preserved branch/worktree; `taskfleet run discard 01m3hnqxcazsbm7xyg5tq06838` recorded that authorization and removed both. No session-owned preserved worker work remains.

Jari then directed that release operations require the **newest Shipshape available from Homebrew**, not a stale fixed 0.12.2 pin. The release wrapper and gate now verify the tap's stable version and admit the exact validated 0.12.3 build; if the tap cannot be checked or has advanced beyond the validated protocol, release fails closed until revalidation. The held-tag authorization and exact-commit local gate remain intact. The 0.12.3 protocol fixture performed cut/resume only against disposable remotes, and `scripts/validate-local-release.sh` passed on clean commit `7ada4886` (1,124 release nextest tests, package and distribution checks). No release tag was pushed, and repository work did not install Taskfleet or its skills. All changes are pushed to `origin/main` at `7ada4886`; the previous published release is not changed by these source commits.

**Next session:** re-read the current AGENTS.md release policy and live `issuectl dag --json --reservations …`; this narrative is not a schedule. The two completed issues have no remaining lane work, and the 2026-09-27 DAG check reported no scheduled or unscheduled issues. There is no deployment step or test-account reset for this repository. The portfolio run `01m3hnkt3j0ckaxb6b4qev1r47` was deliberately interactive: Jari retains the explicit `taskfleet run merge|cancel` decision after review; do not infer completion from this handoff.

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
