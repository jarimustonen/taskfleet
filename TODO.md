# TODO

Session-level plan + handoff. Longer-running planning/design docs live under
`issues/<slug>/{design,plan,breakdown,…}.md`; this file points at issuectl issues
for the actual tracked work. Standing rules and canonical learnings live in the
root `AGENTS.md` (operating policy + state-integrity invariants) — this file
holds only the **active handoff** and a **compact stint archive**.

---

## 🔄 Continue here (ALOITA TÄSTÄ), 2026-09-06 (**Taskfleet identity convergence complete; continue in the adapter project**)

**No run owns work.** `taskfleet run list --output json` returned zero runs after terminal migration histories were moved out of the active lookup root. Taskfleet `main` is clean and synchronized. The filesystem-convergence issue is closed, and exact-main CI `34050378721` passed for closure commit `47e96b6b`.

**Taskfleet migration is complete.** The product, command, crates, state/config, release topology, source repository, checkout/worktree roots, dependent repositories, fleet packages, and active runtime surfaces now use the canonical Taskfleet identity. The final maintained scan covered Taskfleet, the adapter, Homebase, issuectl, Shipshape, project-canon, intakectl, 3DBear, blog, and Deutschpad with zero retired tracked identity references. Gertrud, Hauis, and Haapa use canonical active paths; Brunhild was unreachable and remains explicitly unverified in Homebase.

**The external Pi adapter is delivered.** The separate private repository is `/Users/jari/Sources/taskfleet-pi-telemetry`, commit `65b032e5`, package version `0.1.0`. Its tests, all protocol fixtures, privacy review, disposable load, and real endpoint flow passed. Homebase commit `73b4a262` pins it immutably and deployed it on Gertrud, Hauis, and Haapa. A live Taskfleet run observed `current` advisory telemetry while normal `run merge` remained the only success truth.

**This Taskfleet repository has no prepared execution agenda.** The reservation-aware DAG read succeeded but currently has no lanes. It reports 21 unscheduled active items: most are untriaged intake awaiting a future human lane-or-close sweep, while `rename-taskfleet`, `taskfleet-dual-state-root`, `end-end-stint`, and `run-wait-json` are open and likewise require explicit lane-or-close disposition. These are context only, not executable work; do not infer acceptance from mechanical spawnability.

**Next session.** Switch to the `🛰️ taskfleet-pi-telemetry` tmux window at `/Users/jari/Sources/taskfleet-pi-telemetry` and invoke `/skill:stint-start`. That repository has its own current `TODO.md`, complete green gate, distribution boundary, hot-file guidance, and deliberately empty prepared agenda. This old Pi session remains bound to the removed pre-convergence cwd and should not be resumed.

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

## Piialiisan bugiraportit

- [ ] 🐛 Piialiisan bugiraportti: run create omits source_repo from fresh run manifest — jari via Telegram ([`intake-bug-taskfleet-19a653fff4c9`](issues/intake-bug-taskfleet-19a653fff4c9/item.md))
- [ ] 🐛 Piialiisan bugiraportti: run show cannot identify a run repository once its worktree is gone — jari via Telegram ([`intake-feature-taskfleet-f706c536df01`](issues/intake-feature-taskfleet-f706c536df01/item.md))
- [ ] 🐛 Piialiisan bugiraportti: Expose source_repo in run show JSON — jari via Telegram ([`intake-feature-taskfleet-635e9e31cdf2`](issues/intake-feature-taskfleet-635e9e31cdf2/item.md))
- [ ] 🐛 Piialiisan bugiraportti: stint-handoff blocks on unrelated concurrent agents — jari via Telegram ([`intake-bug-taskfleet-53fa835cfa74`](issues/intake-bug-taskfleet-53fa835cfa74/item.md))
- [ ] 🐛 Piialiisan bugiraportti: Add teardown for terminal failed runs with preserved worktrees — jari via Telegram ([`intake-feature-taskfleet-41343c4dd3e4`](issues/intake-feature-taskfleet-41343c4dd3e4/item.md))
- [ ] 🐛 Piialiisan bugiraportti: stint-handoff blocks on unrelated global runs — jari via Telegram ([`intake-bug-taskfleet-6edf517c691a`](issues/intake-bug-taskfleet-6edf517c691a/item.md))
- [ ] 🐛 Piialiisan bugiraportti: stint-start should safely rebase a clean diverged main — jari via Telegram ([`intake-feature-taskfleet-5565259bd11f`](issues/intake-feature-taskfleet-5565259bd11f/item.md))
