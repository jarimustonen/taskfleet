# TODO

Session-level plan + handoff. Longer-running planning/design docs live under
`issues/<slug>/{design,plan,breakdown,…}.md`; this file points at issuectl issues
for the actual tracked work. Standing rules and canonical learnings live in the
root `AGENTS.md` (operating policy + state-integrity invariants) — this file
holds only the **active handoff** and a **compact stint archive**.

---

## 🔄 Continue here (ALOITA TÄSTÄ), 2026-09-08 (**follow-up agenda prepared; terminal wrap paused on foreign runs**)

**Taskfleet itself is clean and synchronized.** This session completed the post-rename issue sweep, recorded lightweight designs for explicit preserved-work disposition and a lower-friction stint lifecycle, confirmed the recovery-merge status contradiction from production events and source, and scheduled the accepted follow-ups. The workmux project-prefix regression is now the clearly named `@workmux-project-prefix-emoji` issue and is scheduled. Derive all execution order and readiness from the live `issuectl dag`; do not reconstruct it from this narrative.

**The clean-break rename and external adapter remain complete.** Taskfleet, its maintained ecosystem surfaces, active paths, state/config namespace, release topology, and private Pi telemetry adapter are converged under the canonical identity. Homebase also replaced the stale legacy `default = "pi"` harness alias with a user-owned `capable` profile backed by Pi. The profile fix passed focused tests and a side-effect-free Taskfleet create proof, was pushed, and converged on Gertrud, Hauis, and Haapa. Brunhild was still unreachable and remains unverified.

**No Taskfleet-repository run owns work, but the global ownership preflight is not terminal.** Two unrelated workers were still active with live supervisors and no reported recoverable-work marker when checked: Project Canon run `01m202sps8ctthzq6ma5pqp3sj` (`Taskfleet references`) and Reitti CLI run `01m2015e5pq98215wdvw8j0k5e` (`scaffold-rust-workspace`). They belong to concurrent sessions; do not cancel, merge, or otherwise take ownership of them. Under the currently installed `/stint-handoff` contract, their presence prevents accepting schedule verification and proceeding to `/wrap-up`, even though they are foreign to this repository.

**Issue hygiene is otherwise settled.** A fresh deep unlaned sweep found no open non-epic issue outside the execution DAG. The only unscheduled DAG row is the completed `@rename-taskfleet` epic, which still needs an explicit product closure decision; it is context only and not executable work.

**Exact next action.** Re-check the two foreign run IDs. If both have terminalized or otherwise relinquished ownership, rerun `/skill:stint-handoff` to complete schedule verification and `/wrap-up`; then a fresh Taskfleet session can invoke `/skill:stint-start` against the prepared live DAG. If either still owns work, leave it untouched and keep this handoff explicitly incomplete.

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
