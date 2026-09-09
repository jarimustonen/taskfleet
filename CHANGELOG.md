# Changelog

All notable changes to Taskfleet are documented here. Historical releases retain the `taskfleet` name. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!-- oss-changelog:unreleased-start -->
## [Unreleased]

### Added

### Changed

### Fixed
<!-- oss-changelog:unreleased-end -->

## [0.10.0] - 2026-09-09

### Fixed

- Linux native merge executes a write-open tempfile and fails ETXTBSY (`intake-bug-taskfleet-41424b3d959e`).
- Parse every shipped skill description as valid YAML (`skill-description-yaml`).
- Retention tmux fixture can continue before child reaches SIGSTOP (`intake-bug-taskfleet-47b9a99a2591`).
- Supervisor retry test reads partially written fixture output under load (`intake-bug-taskfleet-184a86a0335a`).

## [0.9.0] - 2026-09-08

### Added

- Archive exact Pi session and terminal worker evidence (`durable-worker-evidence`).

### Changed

- Validate explicit source binding for archived historical run retirement (`historical-source-binding`).

## [0.8.2] - 2026-09-08

### Added

- Add `run discard` as an audited, idempotent cleanup path for preserved work
  from terminal failed or cancelled runs, with explicit force required for a
  verified dirty worktree.
- Expose preserved work and recorded source-repository identity in run output;
  add exact repository filtering to `run list` and an explicit
  `condition-met`/`timed-out` outcome to `run wait`.

- Add configurable agent profiles (`add-configurable-agent`).
- Add teardown for terminal failed runs with preserved worktrees (`intake-feature-taskfleet-41343c4dd3e4`).
- Archive exact Pi session and terminal worker evidence (`durable-worker-evidence`).
- Bound completed windows in persistent agent sessions (`agent-session-retention`).
- Capture autonomous agent pane output to durable `<run-dir>/agent.log` (`capture-agent-output-to-run-dir`).
- Remove end-to-end stint friction without durable lifecycle state (`end-end-stint`).
- Spinoff blocked on user input at a genuine fork must propagate to the parent agent, not silently block (`uncommonly-fuzzy-swing`).
- Stamp issue trailers in issue-driven worktree skills (`run-merge-stamp`).
- run list has no repo filter, so sibling-repo runs are indistinguishable by title (`phenomenally-noisy-behavior`).
- stint-start should safely rebase a clean diverged main (`intake-feature-taskfleet-5565259bd11f`).

### Changed

- Carry a bounded stint from orientation through its product-owner report,
  isolate handoff from unrelated global runs, and let workers choose
  proportionate review depth.
- Use Shipshape 0.12.2's authenticated stored bump plan, canonical schema-v2
  distribution contract, resumable verification, and final default-branch
  phase while retaining Taskfleet's held-tag authorization gate.
- Replace GitHub test CI with one fail-closed local release-validation script on
  the exact clean bump commit; retain ordered crates.io publication and
  cargo-dist GitHub binary/Homebrew workflows.

- Adopt ossctl for cutting this project's releases (`adopt-ossctl-release-cut`).
- Align green gate with CI (`align-green-gate`).
- Audit: no user-specific facts in a public artifact (`audit-no-user-specifics`).
- Bundled skills still claim publishing channels are TBD (`skills-stale-tbd-channels`).
- Cargo scaffolding for taskfleet workspace (`cargo-scaffolding`).
- Conform skill installer to canon section 15 (`overly-knowing-family`).
- Converge canonical Taskfleet filesystem paths (`taskfleet-filesystem-path-convergence`).
- Cut the dead plan module from taskfleet-core's public API (`cut-plan-module`).
- DECISION (ADR): harden the current model vs re-architect the lifecycle core (`arch-decision-rearchitect-vs-harden`).
- Define external pi telemetry adapter contract (`worker-telemetry-pi-adapter`).
- Deterministic floor: baseline snapshot + supervisor-enforced gates (tests/clippy vs baseline, file-scope, test-gaming) as standalone module (`deterministic-floor`).
- Distinguish untriaged work from explicit deferral (`distinguish-untriaged-work`).
- Establish Taskfleet as the sole product identity (`rename-taskfleet`).
- Implement configurable agent profile resolver (`worker-profile-config-resolver`).
- Implement worker telemetry control and bounded sample (`worker-telemetry-core-control`).
- Integrate selected agent launch and telemetry visibility (`worker-telemetry-harness-enforcement`).
- Keep only the Taskfleet identity (`taskfleet-zero-legacy-identity`).
- Let workers choose proportionate review depth (`worker-review-scope-discretion`).
- MVP polish wave 2 (B4 + B5 + B6 + B7) (`mvp-polish-wave-2`).
- Make pi the built-in default harness per ADR 0001 D4 (`harness-pi-default`).
- Migrate stint skills from TODO markdown DAG to issuectl dag (`stint-skills-issuectl-dag`).
- Recover interrupted run-create reservations and child publication (`create-idempotency-lease-recovery`).
- Redesign spin-off issuectl materialization to avoid duplicate external issues (`spinoff-issuectl-materialization-arch`).
- Remove project-specific intake concepts leaked into stint-handoff + execution-DAG (keep these skills generic/open-source) (`stint-skills-drop-intake-specifics`).
- Remove stint local source installation (`remove-stint-local-install`).
- Replace unmaintained fs2 with fs4 (or rustix) (`replace-fs2-with-fs4`).
- Reshape worker control plane implementation DAG (`reshape-worker-control-plane-dag`).
- Review worker telemetry and agent profiles as one control plane (`worker-control-plane-review`).
- Structured --help --json output across all subcommands (`help-json-structured`).
- Taskfleet bug analysis should emit canonical triage heading (`canonical-triage-heading`).
- Use current Shipshape release protocol (`shipshape-current-protocol`).
- Use local validation instead of GitHub test CI (`local-release-validation`).
- Validate and roll out worker telemetry end to end (`worker-telemetry-e2e-rollout`).
- Worktree-filed issues must not be laned and must record AI-review provenance (`worktree-issue-provenance`).
- plan.json v2 schema + validator (checks/assertions, immutable plan_rev, intent_rev, DAG validity) (`plan-json-v2-schema`).
- supervise_gates: replace fixed sleeps with readiness polling (V8 flake under load) (`supervise-gate-test-flake`).
- supervisor: qualify tmux liveness by session:window_id + socket (`supervisor-tmux-window-identity`).
- taskfleet-core: verify projection id key matches file body (`projection-key-body-consistency`).
- tracing_appender::non_blocking for JSONL log throughput (`nonblocking-log-appender`).

### Fixed

- Resolve build-script input paths from each Cargo invocation so a shared target
  directory cannot retain a removed release-worktree path in the cached build
  script executable.
- Restore the single canonical default state root and enforce its identity in
  routine CI.
- Reconcile a confirmed late recovery merge from failed to done instead of
  leaving a landed run reported as failed.
- Preserve workmux-owned project prefixes on worker window names.
- Require bug-analysis workers to emit the canonical `## Triage analysis`
  heading.
- Seed the real-protocol release fixture with its own unpublished note so it
  remains valid when the production changelog has just been finalized.

-  (`agent-skips-run-merge-idle-pending`).
-  (`stint-handoff-nonexistent`).
- Autonomous-merge SKILLs do not tell agent to submit terminal node.report (`spinoff-must-submit-node-report`).
- Blocked terminal report (success:false) deletes the worktree branch instead of preserving it for the human (`blocked-report-deletes-branch`).
- Bundled SKILLs document flag forms, terminology, and envelope shapes that don't match the binary (`skill-binary-doc-sync`).
- Cancelled run hides preserved dirty worktree (`cancelled-run-hides-preserved-worktree`).
- Clippy format push warnings break CI (`clippy-format-push-ci`).
- Doctor skill test depends on host tools (`doctor-test-host-tools`).
- Interactive 'code' run self-merged to done without user's /worktree-merge (`interactive-code-run-self-merged`).
- Isolate protocol fixture from finalized release notes (`protocol-fixture-release-notes`).
- Read build paths from the current Cargo invocation (`build-script-worktree-path`).
- Recovery merge leaves landed run failed (`recovery-merge-status`).
- Release gate fails on CI jq and workflow token (`taskfleet-release-gate-ci-portability`).
- Release wrapper rejects ossctl 0.10.1 (`release-wrapper-rejects-3`).
- Release wrapper uses unsupported gh repo shorthand (`release-wrapper-uses`).
- Restore bare-CI portability before Taskfleet R8 (`pre-r8-ci-portability`).
- Restore the single canonical default state root (`canonical-root-regression`).
- Restore the workmux project prefix emoji on Taskfleet worktree windows (`workmux-project-prefix-emoji`).
- Spawning session gets no completion notification when an async run finishes/merges (`no-completion-notification-to-parent`).
- Taskfleet selected incidental default state over runs in a nondefault root (`taskfleet-dual-state-root`).
- Use ULID entropy in run-create branch names (`run-branch-name-ulid-entropy`).
- Worker Claude process can hang mid-run; run reports failed though work is committed-unmerged (`worker-process-hang`).
- contract under-declares the release surface: no homebrew target, and cargo-publish contradicts the documented CI publish (`contract-declare-ci-publish-surface`).
- notify test fires_hook_with_completion_env is order-dependent (TOCTOU on async hook file) (`notify-test-toctou-flake`).
- publish-crates fixture chmods symlinked system tools on Linux CI (`publish-crates-fixture-symlink-chmod`).
- run create with a long --title spawns stillborn (tmux window-name truncation mismatch) (`run-create-long-title-stillborn`).
- run show reports null worktree_path/source_branch for a live pending run (`run-show-null-worktree-path`).
- run wait JSON cannot distinguish a timeout from a settled result (`run-wait-json`).
- stint-handoff blocks on unrelated global runs (`intake-bug-taskfleet-6edf517c691a`).
- version_envelopes snapshot stale after bump to 0.1.5 (`version-envelopes-snapshot`).

## [0.7.1] - 2026-09-07

### Changed

- Converge canonical Taskfleet filesystem paths (`taskfleet-filesystem-path-convergence`).

### Fixed

- Taskfleet selects incidental canonical state over adopted legacy runs (`taskfleet-dual-state-root`).

## [0.7.0] - 2026-09-06

### Changed

- Make Taskfleet the sole maintained product identity across packages, commands,
  state/config paths, environment and protocol fields, skills, documentation,
  repository metadata, and release/distribution contracts.

- Keep only the Taskfleet identity (`taskfleet-zero-legacy-identity`).

### Removed

- Remove the transitional package, executable, dispatch, resolver, state-adoption,
  warning, installer, and skill-rename machinery. Existing installations and state
  are intentionally not discovered or changed.

## [0.6.1] - 2026-09-06

### Fixed

- Make release-policy verification compatible with hosted jq 1.6 and supply its administration-readable credential only to tag-push authorization gates.

- Release gate fails on CI jq and workflow token (`taskfleet-release-gate-ci-portability`).

## [0.6.0] - 2026-09-06

### Added

- **Bundled skill installation now conforms to project-canon §15 (`overly-knowing-family`).** Claude, pi, and Codex are explicit first-class targets; the default and `--agent all` install all three; `skill list --json` declares complete capability/layout metadata; `--target` and `--dry-run` provide isolated, inspectable installs; and Codex prompts embed any required resource content while Claude/pi retain native Agent Skill trees.
- **Worker selection can resolve user-owned executable profiles (`worker-profile-config-resolver`).** `run create --profile` now selects bounded capability/residency profiles from user config, enforces autonomous pi+`worker-v1` eligibility with deterministic non-weakening fallback, and records the requested and selected choice across dry-run, create, and `run show`; repository config remains selection-only and legacy no-profile runs stay readable.
- **Recorded worker profiles now drive launch and telemetry policy (`worker-telemetry-harness-enforcement`).** Create and retry execute the selected candidate's exact recorded argv, export exact adapter identity only for configured pi telemetry, and expose recorded `requirement`/`support` in `run show` without inferring either from samples.

- Establish Taskfleet as the canonical product identity (`rename-taskfleet`).

### Changed

- Adopt the Taskfleet package and command surface.
- Conform the skill installer to canon section 15 (`overly-knowing-family`).
- Use Shipshape for release automation.
- Define and validate the worker telemetry control plane and configurable agent
  profiles (`worker-telemetry-pi-adapter`, `worker-profile-config-resolver`,
  `worker-telemetry-core-control`, `worker-telemetry-harness-enforcement`).
- Remove stint-time local source installation (`remove-stint-local-install`).

### Fixed

- Restore bare-CI portability before Taskfleet R8 (`pre-r8-ci-portability`).
- publish-crates fixture chmods symlinked system tools on Linux CI (`publish-crates-fixture-symlink-chmod`).

## [0.5.1] - 2026-08-23

## [0.5.0] - 2026-08-21

### Changed

- **Release cuts now run through ossctl's resumable engine (`adopt-ossctl-release-cut`).** The engine owns the two-crate version bump, exact internal pin, lockfile, changelog, and version snapshots; a held tag-push checkpoint preserves the exact-SHA main-CI gate before CI publishes crates, binaries, and Homebrew artifacts.

## [0.4.1] - 2026-08-18

### Changed

- **Bundled install guidance now reflects the live release channels (`skills-stale-tbd-channels`).** Removed the obsolete warning that the working Homebrew and cargo-dist shell installer commands were placeholders.
- **Bundled worker guidance now verifies the artifact CI ships (`align-green-gate`).** Worktree briefs use the locked release-mode nextest, doctest, clippy, and rustdoc gate, and workers build and invoke their worktree-local binary instead of mutating the user's global taskfleet installation.

## [0.4.0] - 2026-08-17

### Fixed

- **Pending materialized runs expose their repository coordinates (`run-show-null-worktree-path`).** `run show` and `run list` now surface the default worker's `worktree_path` and the run's effective `source_branch` as soon as worktree creation succeeds, rather than leaving both fields null until callers inspect git worktrees or tmux panes.

### Added

- **Autonomous workers can now propagate genuine decision forks (`uncommonly-fuzzy-swing`).** A worker records a durable `node.awaiting_input` event carrying report-shaped discussion items with the question, options, and recommended default instead of blocking indefinitely on stdin. `run show` and `run list` expose the open request immediately; after a restart-safe three-minute grace, `run wait` settles and the existing `--notify` hook fires with awaiting-input context. `node.input_resolved` clears the generation safely, terminal reports and retries clear stale requests, and bundled spinoff guidance requires a bounded default-or-blocked-report path that preserves blocked work.

### Changed

- **Bundled stint scheduling now uses issuectl directly (`stint-skills-issuectl-dag`).** `/stint-start` and `/stint-handoff` read lane order, dependencies, collision tokens, computed heads, and reservation-aware spawnability from `issuectl dag --json`; `TODO.md` is now handoff narrative only. The retired `AGENTS-EXECUTION-DAG.md` companion is no longer bundled or installed. Until reconciled, `doctor` reports the managed copies as orphan companions. Remove them by running `taskfleet skill install --force` and then `taskfleet skill install --agent codex --force`; the second command reconciles the Codex mirror.

## [0.3.0] - 2026-08-17

### Removed

- **Dead `taskfleet-core` plan API (`cut-plan-module`).** **Breaking:** removed the unused Plan v3 schema module and its public re-exports from `taskfleet-core`.

### Added

- **`config show` is now a layered, tolerant inspection surface (config schema v2) (`config-show-layered-view`).** Each key exposes the raw configured layers (file — including `[harness.per_kind]` — env, default) alongside the effective winner, with per-row validity and a `validation_error` instead of a hard exit on the invalid value the user is trying to debug; only unparseable TOML remains a hard error. File-layer validation no longer depends on whether `TASKFLEET_HARNESS` shadows it, so an invalid file value can neither kill the inspection nor hide behind env. The execution path keeps strict validation, and the `--show-secrets` warning now rides the JSON `warnings` envelope.

### Changed

- **Fresh installations now launch workers with pi.dev by default (`harness-pi-default`).** The built-in harness fallback is `pi` rather than `claude`, per ADR 0001 D4; the existing flag, environment, and config precedence remains unchanged.
- **Crates.io publishing is gated on the full CI test suite (`release-gate-on-ci`).** `publish-crates.yml` now runs the same gate CI applies to `main` (fmt, clippy, locked workspace tests, docs) plus a tag/manifest version-match check before any publish step, so a `vX.Y.Z` tag pushed onto a red or mismatched commit can no longer publish. `OSS-RELEASE.md` and `AGENTS.md` document the tag-triggered flow and the CI-green-gated tag push; the retired local two-crate `cargo publish` sequence is gone from the release docs.

### Fixed

- **A hard-killed `run create` no longer leaves permanently unreclaimable staging state (`create-idempotency-lease-recovery`).** Pre-publication idempotency reservations now carry a durable creator lease (pid + start-time identity with a staleness bound), so a keyed retry can distinguish a live materializer from a dead one: it returns the original run when it published, atomically reclaims stale staging when the creator is provably dead, and fails closed with an actionable error when liveness is unverifiable. Parent `child.spawned` read repair makes keyed child publication idempotent across the two event logs; the unkeyed case is tracked as `recover-unkeyed-child-publication`.

- **Tmux stub tests serialize writing and execution on Linux (`tmux-stub-etxtbsy-flake`).** A parallel test process can fork while another test still has a stub script open for writing; the forked child transiently inherits that descriptor before `exec`, so Linux rejects execution of the script with `ETXTBSY`. The fake-tmux fixture now holds a shared test-local mutex from stub creation through the test's tmux commands, eliminating that write-fd inheritance window for the whole fake-stub family.

## [0.2.2] - 2026-08-17

### Added

- **`--help --json` now uses the global JSON shorthand (`cli-canon-help-json`).** `taskfleet --help --json` and drill-down forms such as `taskfleet run create --json --help` emit the schema-versioned, clap-derived help envelope rather than text help, matching `--help --output json`. Supplying both output selectors remains a structured caller error.

### Fixed

- **Bundled `taskfleet-spawn-spinoff` guidance no longer describes the shipped spinoff surface as a preview (`spinoff-skill-stale-preview-banner`).** Removed the stale stop-gate and obsolete `not_implemented` fallback so agents invoke `run create --kind spinoff` directly.
- **`skill install --force` now replaces dangling symlink destinations (`skill-install-force-symlink`).** Install preflight uses non-following metadata, so a broken link is treated as an existing destination and atomically replaced instead of failing during creation.
- **Interrupted headless spinoff creation no longer publishes a stillborn run (`pi-spinoff-batch`).** `run create` now stages the prompt and durable projections outside the public run tree while `create.sh` blocks on workmux, tmux, and harness startup. It atomically publishes only after a live worker PID and `node.created` are durable, so a client timeout under a concurrent Pi batch cannot leave a successful-looking `pending` manifest with zero nodes. The parent-child event and idempotency commit point follow publication; private interrupted staging state remains available for diagnosis.
- **Tmux stub tests no longer intermittently fail with `ETXTBSY` on Linux (`tmux-stub-etxtbsy-flake`).** The fake executable is now synced and closed before it is made executable and spawned.

## [0.2.1] - 2026-08-16

### Added

- **A worker's terminal report is now readable without knowing the projection's field names (`spinoff-report-fields-null`).** Four separate bug reports claimed spinoff reports persisted as `null`; every one of them was a read-surface error, not data loss — the node projection's field is `last_report` (not `report`), and `run wait` emits `data.runs[]` (it can wait on several runs) while `run show` emits `data.<field>`, so `.data.status` and `.report.summary` correctly returned `null` on payloads that never carried them. The reports were intact in all four verified runs. `node show` now also exposes the terminal report as `data.report` alongside the unchanged `last_report`, and `run show` exposes it for single-worker runs (intentionally `null` for fan-out and other multi-node runs, where each worker is read with `node show`). The load-bearing half of the fix is documentation: the bundled skills taught agents how to *write* a report and never how to *read one back*, so `taskfleet-run-overview`, `worktree-spinoff`, `stint-start`, `stint-handoff`, and `fan-out` now carry the read-back guidance, the `run wait` / `run show` envelope difference, and a working `jq` probe. Additive throughout — no field renamed, no envelope reshaped.
- **`version` advertises the schema versions it supports, and `--json` is a global shorthand (`cli-canon-version-schemas`).** Closes AGENTS-AI-FIRST-CLI §10: the `version` payload now carries named envelope, state, config, help, and skill schema support derived from the real schema constants rather than a hardcoded literal that can rot, so an agent can detect drift instead of guessing. `--json` is accepted globally (previously `version` took only `--output json`), with `--output` resolved after parsing so a conflict between the two selectors is detected at any argument position.

### Fixed

- **`run create` with a long `--title` no longer spawns a stillborn run (`run-create-long-title-stillborn`).** The derived branch name could exceed workmux's 50-byte window-name input, so the window was created under a truncated name while `create.sh` looked it up by the untruncated one — `tmux-window-not-found`, leaving a `pending` run with no live worker and no useful commits. This was the only one of five run-create-stillbirth reports with a deterministic repro. Branch names are now bounded to that reproduced 50-byte boundary, keeping the created name and the lookup name the same derived value; the externally-owned `create.sh` lookup stays exact by design rather than being widened to a prefix match, which could bind teardown to an unrelated window.

## [0.2.0] - 2026-08-16

### Removed

- **0.2 simplification — second subtractive cut: the dead run kinds and the mid-run discussion/spinoff-proposal machinery are gone (`cut-run-kinds-discussion-machinery`).** **Breaking.** The `code`, `orchestrate`, `orchestrated`, `bugfix`, and `make-skill` run kinds are removed from `run create --kind` (the surviving kinds are `spinoff`, `research`, `technical-decision`, `fan-out`); with the interactive kinds gone, `Lifecycle::Interactive` empties and the kind-derived lifecycle inference in the supervisor collapses. The mid-run `discussion` / `spinoff` CLI subcommands, their `discussion.*` / `spinoff.*` event kinds + reducer projections, the `discussions/` / `spinoffs/` projection dirs, and the `open_discussions` / `pending_spinoffs` manifest counters are removed; the supervisor no longer derives per-run discussion/spinoff events from a child's report. The terminal-report `discussion_items[]` / `spinoff_proposals[]` fields are **kept** — decisions and follow-ups still ride the terminal `node.report`. The `run merge --confirm-interactive` flag and the interactive `code`-run merge gate are removed. Bundled skills `/worktree-code`, `/orchestrate`, `/worktree-orchestrated`, `/worktree-bugfix`, `/worktree-make-skill` are deleted and the `/worktree` router now routes only to surviving variants (default `/worktree-spinoff`). On-disk runs recorded under a removed kind still decode (read-only `Kind::Unknown`) so `doctor` / `run list` report — never delete — the evidence corpus (ADR §D7). Obsoletes `bundled-orchestrate-skill`.
- **0.2 simplification — first subtractive cut: the code-pipeline subsystem and the harness heavy layer are gone (`cut-pipeline-floor-harness-heavy`).** ~26.5k LOC removed: `pipeline`/`floor` and the experimental harness layer (`harness bakeoff`, `harness conformance`, the `CodeHarness` trait, and the `aider` + `claude-deepseek` adapters). The light `--harness claude|pi` launcher is unchanged and still selects the worker harness. **Breaking:** the `harness bakeoff` / `harness conformance` and pipeline subcommands no longer exist. Supersedes 7 now-obsolete pipeline/harness fix issues.

### Changed

- **Bundled stint/orchestrate skills: an `in-progress` issue is now a resumable head-of-line candidate, not excluded (`stint-head-of-line-in-progress-eligible`).** The execution-DAG eligibility rule no longer drops `in-progress` issues (`in-progress` means *started*, not *being worked right now*); the DAG is consulted only when nothing is actively running, so such an issue is surfaced for resumption. Double-work prevention moves entirely to the caller's reserve-at-launch guard. Mirrors issuectl's `dag-inprogress-is-spawnable`.
- **pi.dev skill-mirror provenance moves to a flat per-file model (schema v3) (`pi-provenance-flat-file-model`).** Each mirrored file (`SKILL.md` + every companion) is tracked individually rather than in a bundled per-skill record, with a read/upgrade path from v2. `--force` reconciliation, companions-first prune, and `doctor` coverage are preserved.

### Added

- **Thin supervisor A6/A1/A5 — explicit worker-exit facts, typed outcomes, and attention-required visibility.** Workers now launch through a thin `run-worker` shim that records the true child exit status/signal as a durable `worker.exited` fact. The supervisor consumes a typed outcome table instead of the old pid×pane×branch×activity-clock inference: `run merge` is the only success truth; non-zero/signal exits fail with work preserved; exit-0-without-merge becomes visible `attention_required`, not done or failed. `run wait` now settles on that non-terminal attention state, and `run show` / `run list` surface resume hints without mutating the run terminal. The idle-unmerged / git-reconcile auto-success heuristics are gone.
- **Teardown safety hardening for non-merge outcomes.** Non-explicit-merge teardown now fails closed when it cannot prove cleanup is safe: dirty worktrees are preserved, git errors preserve instead of remove, detached-HEAD / stale-branch / no-branch worktrees with unique committed work are preserved, and source-relative branch checks are no longer the only guard. This closes the review-found data-loss edges introduced by relying on committed-branch reachability alone.
- **Run-report provenance and rollup correctness.** New `ReportOrigin` provenance distinguishes Agent, Supervisor, and RunMerge-authored reports. Outcome classification, reducer merge adoption, and the landed/report-marker fallback now trust typed `RunMerge` origin and only honor legacy `via` markers when the origin field is genuinely absent, keeping old runs readable while preventing forged merge markers. Supervisor run rollup is now log-authoritative for leaf nodes rather than projection-scan based, so a crash-interrupted node projection cannot make the run terminalize while a log-visible node is still live.
- **Bundled skill install guard for pi.dev.** The `stint-start` and `stint-handoff` bundled skill descriptions are trimmed below pi.dev's 1024-character description limit while keeping their Finnish/English trigger phrases and disambiguators, and a unit test now rejects any bundled skill description over that limit.
- **`config` noun — inspect the config file path and effective resolved config with per-key source (`config-subcommand`).** Follow-up to `run-create-harness-flag` (which introduced `~/.taskfleet/config.toml` `[harness]` but no inspection surface). Two read-only verbs per AGENTS-AI-FIRST-CLI §8: `taskfleet config path` prints the config file location (with `exists`, whether or not the file is present — so a caller never has to guess where to write settings), and `taskfleet config show` prints the effective resolved configuration as per-key rows — `harness.default` plus one `harness.<kind>` per creatable run kind — each carrying its `source` (`env | file | default`) so an agent can reason about **why** a value is what it is. The harness rows reuse the existing `harness::select` precedence resolver verbatim (per-kind override → section default → built-in), so `config show` never re-implements resolution; an `TASKFLEET_HARNESS` override honestly shadows every row to `source: "env"`. Each row carries a `secret` flag and the §8 redaction contract (`--show-secrets` to reveal, warning on stderr) is wired for future secret-valued keys, though none exist today. Strict validation: a bad harness value in the file fails the command loudly (`invalid_harness`) rather than being silently laundered. Both verbs support `--output json|jsonl|text`; the payloads carry `schema_version_config`. Never mutates the config file.
- **`run merge --report-file` no longer blocks a clean merge on a malformed *advisory* report field (`merge-report-schema-lenience`).** The terminal §7.3 report was validated strictly *before* the git merge, so an advisory-field typo (the recurring `title`/`detail` instead of `spinoff_proposals[].proposed_title`/`proposed_kind`/`rationale`) rejected the whole report and blocked the merge of already-committed, reviewed code. `run merge` now validates leniently: the required, correctness-bearing fields (`success`, the `cancelled`/`reason` §7.7 cross-constraints, root-object shape) stay **strict** — a violation there still returns `schema_violation` (now carrying its structured `expected` hint) and performs no merge — but the advisory sections (`summary`, `discussion_items`, `spinoff_proposals`, `wrap_up_recommendations`) **degrade gracefully**: a malformed element is dropped from the persisted report and surfaced as a machine-readable `report_advisory_warnings[]` entry (plus a human-readable `warnings` line), never a merge blocker. Also visible under `--dry-run`, which doubles as a report-file preflight. Merge authorization/provenance is unchanged — `run merge` stamps the authoritative `ReportOrigin::RunMerge` after validation; `node report` and merge-recovery keep strict validation. (New `taskfleet_core::sanitize_report_advisory`; 3-model `/llm-review` + `/assess-findings` applied.)
- **Thin supervisor A5 follow-up — per-node branch-preserving `run cancel --node` for fan-out (`per-node-run`).** `taskfleet run cancel <run-id> --node <node-id>` cancels exactly ONE live node of a multi-node fan-out — for unblocking a single stuck child without killing the batch (design §2.5). It appends only that node's terminal cancel `node.report` and **no** `run.status`: the run stays live while its siblings run, and the supervisor's rollup terminalizes the batch (`done | failed | cancelled`) only once every node has settled. The cancelled node classifies as `TerminalOutcome::Cancelled` → `Teardown::SourceRelative`, so its branch + worktree are preserved (invariant 5) — never force-deleted. The node set is resolved log-authoritatively (a node whose projection write was crash-interrupted is still cancellable; an absent id is a `node_not_found` user error), and the operation is idempotent: a duplicate per-node cancel converges/no-ops without a second report. The supervisor rollup now returns `cancelled` (not `failed`) when every terminal node was cancelled and none failed. The whole-run `run cancel <run-id>` form is unchanged. (design.md §2.5/§2.6.)
- **Thin supervisor — explicit `--interactive` how-run flag (`interactive-flag`).** `taskfleet run create --interactive` marks a run **human-driven** and persists it as explicit `lifecycle: interactive` how-run state (on `run.created` → the manifest, surfaced on `run show` and every `run list` row). Interactivity is now orthogonal to `--kind` — any topology can be interactive — replacing the kind-derived `Lifecycle::Interactive` inference the removed `code` kind used to carry (`Kind::lifecycle` now only *seeds the default*, never infers interactivity). Supervisor semantics match design §6: for an interactive run the watchdog is hands-off — it **never** auto-terminalizes or auto-tears-down from a dead pid, a told `worker.exited` failure, or the crash backstop; it waits for an explicit `run merge` (→ teardown) or `run cancel`, so the human owns the whole lifecycle. Autonomous fire-and-forget spinoffs are unchanged (the flag is opt-in; a default `run create` is byte-identical to before) and bundled spawn skills stay headless + autonomous unless `--interactive` is explicitly requested. (design.md §2/§6.)
- **Thin supervisor A3 — `run salvage`, the fenced manual resume/finish (`run-salvage-command`).** A new `taskfleet run salvage <run-id>` finishes a stuck single-worker run: an *attention-required* run (worker exited cleanly but skipped `run merge`) or a `failed`/blocked run whose branch the teardown gate preserved. It snapshots manifest + `n-0001` under the run lock, then refuses the cases it must not touch (already-`done`/`cancelled`, multi-node, no preserved worktree/branch, a never-started `Pending` run, and a live worker it cannot verify). It classifies the prior worker from durable facts — a live pid whose start-time identity positively matches **overrides** a stale `worker.exited` told fact — and fences a verified-live worker with `SIGTERM` only behind the explicit `--fence` flag (identity is re-verified immediately before the signal, so a recycled pid is never hit). It then drives the **exact `run merge` machinery** (crash-recovery, CAS-guarded source fast-forward, `via: "explicit-merge"` terminal report, supervisor teardown) from the preserved worktree's current git state — never a raw git self-merge — so terminal report/provenance and every state-integrity invariant hold. A `--dry-run` merge preflight validates `--source`/`--report-file` before any fence. `--dry-run` previews the plan without mutating anything. The attention-required resume hint (`run list`/`run show`/`run wait`) now points at `run salvage`. Bounded 0.2 residuals (parent-only process fence, non-atomic fence→merge, concurrent salvage) are the design's deferred 0.2.1 writer-lease work (§2.7); the fresh-agent continuation variant and per-node fan-out salvage are tracked in `run-salvage-fresh`. (design.md §2.2 / A3; 4-model `/llm-review` + `/assess-findings`.)
- **Thin supervisor A2 — deterministic, OID-based recovery for crashed `run merge` transactions (`merge-transaction-recovery`).** `run merge` spans two durability domains (git refs and the event log) and is not atomic across them; a crash after the git merge but before the terminal `explicit-merge` `node.report` used to leave the work *merged in source* with *no merge event* → a false `failed`. `run merge` now records a `merge.started` transaction (`op_id`, `expected_source_oid`, `worker_oid`, source/worker branch, driver pid) on `Node.pending_merge` **before** mutating git, and merge.sh guards the source-ref fast-forward with a compare-and-swap against `expected_source_oid` (refuses with a distinct `merge_source_moved` code if the target moved). On the next `run merge` (self-healing retry) or supervisor tick, recovery resolves that **one** recorded transaction by exact OID — completing it (appending the `explicit-merge` report the crash prevented) when the source ref moved and the worker's content is git-verified integrated, or rejecting it (`merge.aborted`, work preserved) when the source ref never moved or moved without the worker's content. No general branch-content heuristic; gated on the driver being dead so a live merge is never raced. (design.md §2.1b.)
- **CI guard: version snapshots must match the crate version (`release-version-snapshot-refresh`).** `scripts/check-version-snapshots.sh` fails loudly when the `version_*` insta snapshots drift from `[workspace.package] version`, wired as a fast dependency-free `version-snapshots` CI job — so a version bump can no longer silently leave stale snapshots and turn `main` red after the release tag is cut (as happened for v0.1.8). The release mechanics doc now names the snapshot-refresh step.

## [0.1.8] - 2026-08-13

### Fixed

- **A SIGINT/SIGTERM during supervisor boot now exits with the correct signal code (`signal-exit-143-regression`).** A signal arriving after the supervisor claimed its pid but before readiness took a `terminated_during_boot` path and exited 2 instead of the §7.8-mandated 143 (SIGTERM) / 130 (SIGINT) — an intermittent CI-red that surfaced only under `--release` load. A dedicated boot-signal short-circuit now emits `supervisor.exited`, removes the pid, records the readiness error after durable cleanup, and exits via the shared signal-exit path; a deterministic slow-boot barrier test pins SIGTERM→143 / SIGINT→130 and fails against the pre-fix code. (4-model `/llm-review` + `/assess-findings`.)
- **pi.dev skill installs now include each skill's companion files (`support-pi-dev`).** The pi.dev skill mirror wrote `SKILL.md` but dropped its sibling companion resources (e.g. `AGENTS-EXECUTION-DAG.md`), so `/stint-start` aborted with ENOENT under the pi harness. Companions are now mirrored byte-identically into the per-skill dir alongside `SKILL.md` (matching the claude layout, no link rewrite), with companion provenance (schema v2), `--force` reconciliation of dropped companions, companions-first prune, and `doctor` coverage. (4-model `/llm-review` + `/assess-findings`.)

### Changed

- **`/stint-start` now resumes autonomously from the handoff-prepared agenda (`stint-start-autonomous`).** It trusts the `## 🔄 Continue here` block and execution DAG as its plan instead of re-confirming prepared work, while an explicit cold-start branch and hard stops ensure that "just go" never overrides deploy, green-gate, collision, or landing safety.

## [0.1.7] - 2026-08-12

### Fixed

- **An agent that ends its session without calling `run merge` no longer strands the run `pending` with committed-but-unmerged work (`agent-skips-run-merge-idle-pending`).** Root cause: the idle-TUI's CPU render-loop trickle perpetually re-stamped the supervisor's "activity" clock, so the idle-unmerged safety net could never fire. The CPU-activity clock is now rate-gated, so a genuinely idle agent that committed work but skipped the merge is detected and its branch + worktree preserved for recovery instead of looking indefinitely busy. (4-model `/llm-review` applied.)
- **`doctor` / `prune` now cover codex skills and their `_shared` companion files (`doctor-codex-companion-coverage`).** The sync / orphan / prune diagnostics extend to the codex flat layout and the shared companion subdir, so a drifted or orphaned codex companion is surfaced and prunable on par with the claude layout.
- **Cleared a main-wide CI-red rustdoc break in the docs job (`ci-docs-bakeoff-registry-link`).** A broken intra-doc link `[bakeoff::registry]` is demoted to a code span, so the docs job (and the users who build docs) is green again.

## [0.1.6] - 2026-08-11

### Added

- **`run create --harness <name>` selects the worker's code harness (`run-create-harness-flag`).** The existing `CodeHarness` seam (adapters `aider`, `claude`, `claude-deepseek`, `pi`) — previously reachable only via `harness bakeoff` — is now wired into the real worker-launch path for every run kind. Resolution follows flag > env (`TASKFLEET_HARNESS`) > config file > built-in default (`claude`), with a per-kind default so autonomous kinds (`spinoff`/`research`) can default to **pi.dev** while interactive `code` stays on Claude. The chosen harness is surfaced in `run show` / `run list --json` and the event log, and the supervisor preserves it across a retry. Claude remains the default.
- **`skill install` dual-homes skills into pi.dev's skill dir (`pidev-dual-home-skills`).** Each skill's `SKILL.md` is now installed into `~/.pi/agent/skills/<name>/` in addition to `~/.claude/skills/<name>/`, so the CLI's bundled skills are discoverable under the pi.dev harness (`/skill:name`). Vendored-filtering-aware (only `SKILL.md` is mirrored into the pi target); the Claude Code install path is byte-for-byte unchanged, and the pi mirror is decoupled from the all-or-nothing preflight so it can never block a Claude install.
- **Companion resources install for the codex flat layout (`skill-companion-codex-layout`).** Companion files install into the shared `~/.codex/prompts/_shared/` subdir with per-skill link rewrites; the claude layout is provably byte-for-byte unchanged.
- **`doctor` detects and `prune` removes orphan companion files (`doctor-orphan-companion-files`).** A companion resource a prior binary installed but the current binary no longer bundles is now surfaced as a distinct `skill.orphan.*` diagnostic (not conflated with missing/out-of-sync) and can be pruned.

### Fixed

- **`run show` / `run list` distinguish supervisor states instead of one boolean (`supervisorview-conflates-states`).** A wire-level `SupervisorState` enum (`alive` | `dead` | `not-recorded` | `unreadable` | `unknown`, `alive` kept as a back-compat boolean) replaces the collapsed condition. Closed a real probe read-then-stat TOCTOU (flagged unanimously by the review panel), and indeterminate states (`unreadable`/`unknown`) no longer drive stillborn/orphaned verdicts, so an unreadable pid file can't mislead a reattach/cancel decision.
- **The pipeline audit path records the effective tier and commit OID of committed-but-blocked work (`push-blocked-chunk-tier-and-commit-audit`).** `push_blocked_chunk` and the crash/panic audit path now record the promoted/effective tier (not the plan-declared one) plus the commit OID, threaded through `BuildAttempt`/`ChunkAttempt`/`WaveBuildOutcome::Blocked`.
- **Refreshed the stale `version_text` insta snapshot, clearing a main-wide CI red (`version-envelopes-snapshot`).**

## [0.1.5] - 2026-08-10

### Fixed

- **A stillborn run is now visibly flagged in `run list`, not shown as an ordinary `pending` row (`supervisor-dies-before-worker-node`).** A run whose supervisor died before creating the first worker node (dead/absent supervisor, 0 nodes, no progress) previously appeared in `run list` identical to a healthy just-created run — the "looks stuck until someone notices" failure that reproduced 3× under saturation. `run list` now wires in the same `is_stillborn` verdict `run wait` / `run show` already use (no extra I/O, under the existing shared lock) and renders a distinct `pending (stillborn)` marker. Pure read path — no event/reducer/schema/lock write; all five state-integrity invariants untouched.
- **`run wait` / `run show` now detect an *orphaned mid-run* run and settle promptly instead of blocking the full timeout (`run-wait-still`).** The stillborn fix only covered a supervisor that died before any node (`node_count == 0`). This handles the sibling case: a supervisor that died *after* creating `n-0001` but before rolling the run up — `node_count > 0`, dead supervisor, `pending`/`running`, idle past a 15-minute grace window (to avoid misreading a briefly-unschedulable but live supervisor). Such a run is now reported `stalled` with a per-kind reason and pointed at `run reattach` for recovery. Read-time only; shared lock preserved.
- **A wave-build worker that commits then panics/errors now has its own branch audited, not orphaned (`wave-terminal-worker-own-artifact-unaudited`).** Previously `WaveJob::Error`/`Panicked` discarded the terminal worker's artifact identity, leaving its committed `<slug>/chunk-<id>` branch on disk that no report named — an invariant-5 audit gap. The worker now carries its artifact identity (deterministic worktree/branch + observed head) across the `catch_unwind` boundary and records an audit-only `branch_preserved` `ChunkReport` (contents not vouched for, since the crash means it was never reviewed), so nothing committed silently vanishes from the ledger.

## [0.1.4] - 2026-08-10

### Fixed

- **`run show --output json` no longer returns an all-null payload for a resolvable live run (`run-show-json-null-fields`).** `run show` now surfaces the same populated data that `run list` / `event tail` resolve — the supervisor block is lifted to the top level of `.data` and the run-list row is flattened in — instead of the silent all-null object seen intermittently against a live run.
- **`run wait` / `run show` detect a stillborn run and return promptly (`run-wait-stillborn-run-not-detected`).** A run whose supervisor died before creating any node (dead supervisor, 0 nodes, no forward progress) is now reported as `stalled` and `run wait` returns immediately (non-zero under `--fail-on-error`) instead of blocking the full timeout.
- **The worktree-merge lock now works on stock macOS (`merge-lock-flock-not-portable-macos`).** `merge.sh` previously serialized concurrent merges with `flock`, which ships with util-linux and is absent on a stock Mac — so on macOS (the primary platform) the merge lock silently failed and `run merge` could misreport a lock error as `merge_in_progress`. Replaced it with a portable atomic `mkdir` mutex (same 600s timeout and serialization semantics, no external binary), so merges serialize correctly with no `flock` dependency.

### Changed

- **`run wait --timeout` accepts a bare integer as seconds (`run-wait-timeout-unit-required`).** `--timeout 2400` now means 2400 seconds; previously a unit was required (`2400sec`) and a bare integer was rejected instantly — which, for a backgrounded `run wait`, looked like the run had settled when it had not (silent-instant-exit). Unit-suffixed values (`2400sec`, `40min`, `500ms`) parse as before; the bare-integer path is gated on all-digits + overflow.
- **A hard pipeline failure now exits non-zero *and* surfaces the preserved-branch audit (`pipeline-hard-failure-carries-report`).** `pipeline run` carries a report on the hard-failure error path, so `cmd_run` both fails loudly and renders the `branch_preserved` siblings (the invariant-5 preservation is now auditable on the failure path, not just the success path).

### Security

- **Addressed RUSTSEC-2026-0009 without dropping the 1.85 MSRV (`ci-red-main-deny-docs`, `dry-run-projection-parity-flake`).** The `time` crate's stack-exhaustion DoS advisory (fixed only in `time ≥0.3.47`, which requires rustc 1.88 — above our 1.85 floor) is a transitive dependency via `tracing-appender`, used solely for log-file-rotation timestamps; we never parse untrusted time input, so the advisory is not exploitable here. Resolved by pinning `time` to `0.3.41` (keeping MSRV 1.85) plus a scoped, time-boxed `deny.toml` ignore documenting the rationale, and repaired the `taskfleet-core`/`taskfleet-cli` intra-doc links that had left CI red.

## [0.1.3] - 2026-08-06

Supersedes the 0.1.2 tag, whose prebuilt-binary + Homebrew release failed to
publish (a transient self-hosted-runner checkout error, compounded by two CI
breakages now fixed here); 0.1.2 remains on crates.io. 0.1.3 is the first
coherent cut across all three channels since 0.1.1.

### Fixed

- **Concurrent wave builds now get adaptive tier promotion (`immoderately-dirty-cushion`).** Under `pipeline run --max-build-concurrency > 1`, a chunk that exhausted its floor re-code budget previously blocked terminally, even though the strictly-sequential path (`--max-build-concurrency 1`) would have promoted it to the next model tier and succeeded. On wave-build exhaustion the chunk is now re-queued into a sequential drain off the moved tip (the same pattern the merge phase uses for rebase-and-fix), so promotion runs and the outcome no longer depends on the concurrency setting; the preserved build-phase attempt is reconciled so no worktree is orphaned. A per-worker `catch_unwind` also turns a build-thread panic into a terminal stage-stop that still preserves sibling builds (state-integrity invariant 5).
- **Pin `time` to 0.3.41 to hold the 1.85 MSRV.** `time@0.3.51` / `time-core@0.1.9` (transitive via `tracing-appender`) raised their MSRV to rustc 1.88, above the project's declared 1.85, breaking the MSRV CI job and the release build. Pinned back to `time@0.3.41` (rust-version 1.67.1).

### Internal

- **`run cancel` prefix resolution is now type-safe (`run-paths-typed-selector-split`).** Prefix (fuzzy) run-id resolution is confined to CLI verb entry via a sealed `RunSelector`; internal / supervisor / reducer paths take an exact typed `RunId` through `run_paths_exact`, so a future caller passing a truncated id can no longer silently fuzzy-resolve to the wrong run (a confused-deputy risk). No user-visible behaviour change.
- De-flaked the `watchdog` snapshot invocation-count test (`immoderately-irate-north`) — isolation-safe counting so the integrated test suite is deterministic under parallelism.

## [0.1.2] - 2026-08-06

### Added

- **`run show` / `run list` flag an undriven `--kind orchestrate` run as `stalled` (`peculiarly-muddled-caption`).** A `--kind orchestrate` supervisor only *adopts* children — it does not itself drive the fan-out — so a driver run whose orchestrator agent never ran (or died immediately) could sit `pending` with zero children for hours, indistinguishable at a glance from a healthy long-running campaign (one real case ran 15h). Both commands now expose a read-time `stalled` boolean (and a `(stalled)` marker in the human status column) that is true when the driver node `n-0001` is still `pending` with **zero children** and no node-touching events past a 12-minute grace window. The signal is computed entirely at read time from existing timestamps + event sequence — no reducer, schema, or event-append path is touched, so the state-integrity invariants are unaffected.

### Changed

- **`run cancel` on an already-terminal run is now a user error, exit 1 (`cancel-run-already-terminal-error-class`).** Refusing to cancel a `Done`/`Failed` run is a deterministic domain refusal, not a system fault, so it now maps to `CliError::user` (exit 1) instead of `CliError::system` (exit 2) — exit-code class governs AI-caller retry behaviour, and exit 2 could trigger spurious retries of a permanently-refused operation. The error's `expected` hint also changes from the pipe-delimited string `"running|pending|blocked"` to a JSON array `["running","pending","blocked"]` (the non-terminal, cancellable states) for machine consumption, matching the array-valued `expected` convention used elsewhere.
- **Pipeline rollback pins durable per-chunk provenance refs (`pipeline-provenance-durable-refs`).** Before `rebuild_integration` resets `feat/<slug>` to the fork, the kept chunks' authored commit OIDs are now pinned under `refs/pipeline/prov/<run>/<chunk>` instead of relying on object-DB reachability, closing the (narrow) window where an external aggressive `git gc --prune=now` racing a rollback could sweep the orphaned authored commits. Teardown is gated on the merge outcome: a merged run's provenance refs are pruned, while a preserved/unmerged branch keeps its refs.

## [0.1.1] - 2026-08-05

### Added

- **`pipeline run` builds independent chunks concurrently (`pipeline-parallel-chunks`).** The T5 skeleton ran plan chunks strictly sequentially, each stacking on the moved `feat/<slug>` tip. Chunks with no dependency path between them can now build in parallel worktrees off a shared base and merge back in a deterministic order with the T3 floor re-checked at each merge; conflicts/regressions between concurrently-built chunks resolve via the deterministic rebase-and-fix protocol. Opt-in via a new `--max-build-concurrency` flag — the default of `1` leaves the proven sequential path completely untouched — and bounded by the existing §9 process budget. The floor gate is shared (`build_and_gate`) so it is byte-identical on both paths.
- **`run wait` / `run show` expose a rebase-robust `landed` signal (`landing-signal-reliable-after-rebase`).** The prior guidance to confirm a landing with `git merge-base --is-ancestor <branch> <target>` gave false negatives in exactly the high-parallel scenario the stint engine targets: a caller-side `git rebase` replays the worker's merge under a new hash while the branch ref stays at the pre-rebase hash, so the ancestry check reported already-merged work as "not landed" (risking a redundant re-spawn or a hand-salvage of merged work). The CLI now surfaces a first-class `landed` boolean computed git-authoritatively (cherry patch-id + ancestry net, with a report-marker fallback) against the current target tip, and the bundled `stint-start` / `worktree-spinoff` docs no longer rely on `merge-base --is-ancestor`.
- **`doctor` now audits bundled-skill companion resource files (`doctor-skill-companion-sync`).** The `skill.sync.<name>` check previously validated only each skill's `SKILL.md`, so a companion sibling (e.g. `stint-start/AGENTS-EXECUTION-DAG.md`) that was missing, stale, or user-edited left the skill's in-body link dangling while `doctor` still reported the skill in-sync. `doctor` now emits a per-companion `skill.sync.<name>.<file>` check that verifies the companion is present at its install path and byte-identical to the binary's bundled copy — classifying any drift (older `cli_version` → WARN + `--fix`, newer → upgrade-the-binary WARN, content edited at the same version → local-edits WARN) and naming the offending file in the message.

## [0.1.0] - 2026-08-04

First public release — published to crates.io (`taskfleet`, `taskfleet-core`) and
installable via Homebrew (`brew install jarimustonen/taskfleet/taskfleet`).
The first publishable cut: the CLI is real, the bundled skill family covers the full
agent loop, and run state survives crashes via an append-only event log + lock-gated
reducer. Groups the MVP foundation with the code-pipeline / harness work (mostly
behind-the-seam) that landed before the release.

### Fixed — code pipeline

- **`pipeline run` spec/verify: parse the model's `type:result` message, not the `type:system` init banner (`pipeline-claude-output-parse`).** `claude -p --output-format json` (Claude Code ≥ 2.1.211) emits a *sequence* of JSON messages — an init banner first, then the answer — so reading `.result` off the whole output failed and the raw-transcript fallback fed the init banner into the plan parser, making every live spec fail `missing field acceptance`. `run_claude` now parses the transcript with a streaming deserializer (tolerant of a top-level array, NDJSON, concatenated `{…}{…}`, and pretty-printed multi-line objects) and selects the last `type == "result"` message's `.result`. The raw-transcript fallback is now narrow — it fires only when NO Claude envelope was recognized; a recognized envelope with no usable result returns empty so the caller fails loudly rather than silently mis-parsing the banner. Fixes both spec and verify in the shared path.
- **`pipeline run` spec stage: schema-complete plan prompt + validation-error repair loop (`pipeline-spec-plan-conformance`).** The first live run failed at spec with `plan invalid: … missing field 'acceptance'` and the retry reproduced the same error because it re-prompted blind. The spec prompt now states which `plan.json` fields are REQUIRED (derived from the `taskfleet_core::plan` types so it can't drift) and that `acceptance` must carry ≥1 executable `{desc,run}` check; on a validation failure the driver now runs a bounded **repair loop** that feeds the exact validator error and the invalid JSON back to the model to correct precisely that error. The parse stays strict (no silent server-side patching); on exhaustion the last raw invalid plan is persisted to `<workdir>/plan.invalid.json` and the error surfaces the last validator message.

### Added — code pipeline, harness bake-off & completion hook

- **`pipeline run`: the first live end-to-end code pipeline (`pipeline-walking-skeleton`, T5).**
  A new ADDITIVE command `taskfleet pipeline run --intent <str|file>
  --source-branch <branch> [--files <f>…] [--slug …] [--repo …] [--workdir …]
  [--keep]` drives one feature through the whole loop (design §6):
  **spec[Opus] → code[claude-deepseek] → floor-gate → verify[Opus] → merge**. It
  forks a `feat/<slug>` integration branch, captures the T3 baseline at the fork,
  asks `claude` (Opus) for a validated `plan.json` v2, runs each chunk in an
  isolated worktree through the `claude-deepseek` `CodeHarness`, applies the
  deterministic T3 floor as the **hard merge gate** (a chunk or feature that fails
  the floor is preserved and never merged), has `claude` (Opus) run the plan's
  acceptance checks + judge product-vs-intent, and on green merges `feat/<slug>`
  into the source branch — emitting a structured report (plan chunk count,
  per-chunk floor verdicts, verify result, whether it merged, the final commit,
  and decision envelopes recording the deciding tier). Bold-to-live (design §14):
  it invokes the real agents and really merges, but it does NOT touch `run create`
  / the supervisor. The orchestration loop is unit-tested with a stub harness +
  scripted spec/verify against a real throwaway git repo (no network); the one
  live end-to-end test is gated behind `TASKFLEET_PIPELINE_LIVE=1`. The fix loop,
  re-spec, tier promotion, cost/circuit-breakers, and parallel chunks are deferred
  to filed follow-ups.

- **Harness bake-off: three new `CodeHarness` adapters + a `harness bakeoff`
  comparison runner (`harness-bakeoff`).** Behind the seam (not wired into
  `run create`/supervisor), the code-pipeline harness (design §10) now has four
  git-inspecting adapters — `aider`, `claude` (Claude Code `-p` headless),
  `claude-deepseek` (Claude Code via the deepseek wrapper), and `pi`
  (earendil-works/pi) — sharing one launch+git-outcome skeleton
  (`harness::support`) so they map an agent's *git* result (commit, changed files,
  self-checks) to a `ChunkResult`, never parsing tool prose. The new
  `taskfleet harness bakeoff --brief <file> [--files <f>…] [--only <names>]
  [--timeout <secs>]` command runs one brief through every *available* adapter in
  isolated throwaway git repos and prints a one-row-per-harness comparison
  (outcome / files-changed / +lines-/-lines / wall-time / cost / checks-pass) as a
  text table or, under `--output json`, a `{brief_file, selected, adapters[]}`
  envelope. It invokes the real agents (bold-to-live); adapters whose binary or
  credential is absent are reported as `unavailable`, not errored. Live agent
  tests are gated behind `TASKFLEET_HARNESS_LIVE=1`; the deterministic conformance
  suite drives each adapter through fixture scripts with no network.

- **`run create --notify <cmd>` completion hook (`no-completion-notification-to-parent`).**
  A run created with `--notify <cmd>` now runs that command when the run reaches a
  terminal state (`done | failed | cancelled`), fired by the supervisor on the
  terminal transition **before** teardown removes the worktree/window. The command
  runs via `sh -c` with `TASKFLEET_RUN_ID`, `TASKFLEET_STATUS`, `TASKFLEET_SUMMARY`,
  `TASKFLEET_RUN_KIND`, and `TASKFLEET_RUN_TITLE` in its environment — the push signal a
  spawning session needs to learn of completion without polling (append a line to a
  file the harness watches, post a desktop toast, ping a FIFO). Delivery is
  **at-least-once**: firing is deduped on a durable `run.notified` marker (idempotency
  key `supervisor-notify:<run-id>`) recorded *after* the spawn under one exclusive
  lock, so the healthy path fires exactly once, but a supervisor crash in the window
  between firing and recording re-fires on restart — a duplicate is preferred over a
  missed completion signal, so hooks should tolerate running more than once. The
  command is spawned detached and reaped on a thread so a slow/hung hook cannot wedge
  the supervisor tick. New manifest field `notify_cmd` (`#[serde(default)]`, absent
  when the flag is omitted). The bundled `worktree-spinoff` and `worktree-code` skills
  document `--notify` and backgrounded `run wait` as the two ways completion reaches
  the spawning session, and no longer imply an undeliverable notification;
  `worktree-code`'s progress section is also corrected to branch on `manifest.status`
  (not `lifecycle`).

- **Flexible `plan.json` check contract (code-pipeline `plan-check-run-contract`,
  owner-locked 2026-07-23).** A check now carries the general goal (`desc`) plus a
  flexible shell command (`run`) with optional precision — `cwd` (repo-relative
  working directory) and `expect_exit` (expected exit code, default 0) — on both
  per-chunk `checks[]` and `acceptance[]` check items. Neither a rigid struct nor
  bare text: the goal is always communicated, the command stays expressive, and
  precision is available but not forced. `cwd` is held to the same repo-relative
  safety guard as `files_touched` (the floor gates possibly-adversarial code-node
  output, so an absolute / `..` / `~` cwd is rejected), and `expect_exit` is
  bounded to the shell exit range `0..=255`. The `plan::Check` type, structural
  validator, checked-in JSON Schema, and the deterministic-floor runner (which
  honours `cwd`/`expect_exit` and records `cwd` on the `CheckRun` audit result)
  are updated in lockstep; a check with only `desc`+`run` is unchanged
  (exit 0 = pass).

- **Inverted control loop scaffold (code-pipeline T4, behind the seam).** A new
  module (`crates/taskfleet-cli/src/pipeline/`) modelling the design §2 inversion: the
  supervisor owns the loop and the orchestrator is a stateless pure function
  returning discrete typed `Action` primitives (`ReCodeChunk`, `TriggerReSpec`,
  `AcceptChunk`, `PromoteTier`, `OpenDiscussion`, `ProposeSpinoff`,
  `DeclareConverged`, `Escalate`) — never prose. Each primitive is classified
  `Routine` vs `Consequential` (design §0.2; spin-off triviality carried by an
  explicit `SpinoffScope`, not overloaded onto finding `Severity`), and a
  `TieredOrchestrator<C, D>` routes consequential decisions from a fast
  coordinator to an expensive decider. Every decision is recorded atomically as a
  `DecisionRecord` (action + `DecisionEnvelope` with actor/inputs/reason/
  **`decision_tier`**/model/prompt-version + outcome), so the trail is causally
  replayable. The `drive` loop is fail-closed: a consequential action stamped
  `coordinator` is a caught `TierViolation` that escalates; a circuit-breaker trip
  escalates deterministically without consulting the orchestrator (design §9);
  and chunk preconditions are checked before any would-execute. Ships
  deterministic scripted coordinator/decider stubs and a stubbed `ActionExecutor`,
  fully unit-tested (routine FIX loop, decider-tier `DeclareConverged`/`Escalate`/
  `TriggerReSpec`, mis-tier rejection, superseded post-terminal actions, unknown-
  chunk rejection) with no LLM/network. Reviewed via `/llm-review` (3 models);
  real findings addressed. Unused by default — nothing constructs an
  `Orchestrator` or calls `drive` yet; T5 wires it into the real supervisor +
  event log (design §14).
- **`CodeHarness` execution control: timeout + cancellation (code-pipeline T0
  follow-up, still behind the seam).** `run_chunk` now takes a `CancelToken` and
  `ChunkRequest`/`Check` carry optional wall-clock `timeout`s, so a runaway or
  hung code-node can be bounded and aborted (design §9 circuit-breakers). The
  `AiderHarness` honours both — killing the agent's (and each check's) process
  group on expiry/cancel, draining the partial transcript, and returning
  `ChunkOutcome::Timeout`/`Cancelled` (never a hang); `StubHarness` gained a
  `SlowUntilCancel` behaviour so the conformance suite tests both deterministically
  with no network. Unblocks live wiring (T5).
- **Deterministic correctness floor (code-pipeline T3, behind the seam).** A
  standalone module (`crates/taskfleet-cli/src/floor/`) of pure gate functions plus a
  thin capture layer implementing the mechanical merge floor (design §4): a
  serde `BaselineSnapshot` captured at the `feat/<slug>` fork (test pass-list +
  clippy-warning-list hashes projecting down to `plan::Baseline`, optional
  coverage), a check runner over `taskfleet_core::plan::Check`, and five pure gates —
  checks-pass, no-regression (no baseline-passing test now fails), no-new-clippy,
  no-test-gaming (count/ignore/rename/assertion-density), and file-scope
  (`files_touched[]` + slack) — returning a structured `FloorVerdict` of
  `Violation`s. No LLM calls, no judgment, fully unit-tested from fixtures + temp
  git repos, no network. Unused by default — not wired into any live
  `run merge`/supervisor path; staged rollout (design §14) plugs it into the
  supervisor merge gate at T5.
- **`plan.json` v2 schema + validator (`taskfleet_core::plan`).** Serde types for the
  code-pipeline stage contract (`schema_version`, immutable `plan_rev`,
  `intent_rev`, `feature`, `baseline`, `acceptance[]` checks/assertions, and the
  `chunks[]` DAG) plus a structural validator that rejects unsupported schema
  majors and undeclared fields, enforces unique/acyclic chunk deps, ≥1 executable
  check per chunk and in `acceptance[]`, and safe repo-relative `files_touched`
  paths — with domain-typed `PlanValidationError`s the CLI can map to its
  `schema_violation` envelope. A checked-in Draft 2020-12 JSON Schema
  (`crates/taskfleet-core/schemas/plan.v2.schema.json`) is the machine-readable
  artifact, kept in sync with the Rust types by a drift-guard golden test. Read-
  only types + validation only — not yet wired into a live path (design.md §4/§7/
  §13, `issues/code-pipeline/plan-schema.md`).
- **`CodeHarness` adapter contract (code-pipeline T0, behind the seam).** A
  versioned, harness-neutral trait (`crates/taskfleet-cli/src/harness/`) that lets the
  supervisor drive a code-writing agent over one chunk and consume a structured
  `ChunkResult` — never tool prose or exit-status guessing (design §10). Ships the
  request/result protocol (`ChunkRequest`, `ChunkResult`, `ChunkOutcome`, `Check`,
  `CheckResult`, `Usage`, `HarnessCapabilities`, structured `HarnessError`), an
  `AiderHarness` first adapter (shells out non-interactively, reads the outcome
  from git not stdout, `DEEPSEEK_API_KEY` from the env), a deterministic
  `StubHarness`, and a reusable conformance suite. Unused by default — not wired
  into any live `run create`/supervisor path; staged rollout (design §14) plugs it
  in later.

### Fixed — supervisor self-merge reconciliation

- **Supervisor reconciles run status with git after a self-merge.** A spinoff
  whose branch already merged into its `source_branch` is no longer reported
  `failed` (issue `false-failed-after-merge`) or left stuck at `pending`
  forever (issue `supervisor-stuck-pending-after-self-merge`) when its terminal
  `node.report` is lost or never flushed under high fan-out. The watchdog now
  checks `git merge-base --is-ancestor <branch> <source>` (plus an
  advanced-past-fork-point guard using the new `Node.base_sha`) before any
  terminal classification: a merged branch synthesizes a terminal SUCCESS
  (`via: "merge-reconciled"`) and tears down cleanly — even while the agent is
  still alive — instead of a false `agent-died`/`cleanup.branch_preserved`.
  Reconciliation is deliberately conservative so it can never destroy live work:
  it requires the branch to have advanced *forward* past its fork point
  (`base..branch > 0`, rejecting a fresh or `reset`-rewound branch) AND the
  worktree to be clean (an agent that merged then kept editing keeps its
  uncommitted work), re-verifies the merge under the run lock before recording
  it, and re-checks the source-relative unmerged gate at teardown so a branch
  that diverged after the report is preserved, not force-deleted.

### Added — MVP foundation

- **Run model.** Every spawn is a `run` (`~/.taskfleet/runs/<ulid>/`)
  with `events.jsonl` as the canonical source of truth and
  `manifest.json` / `nodes/` / `discussions/` / `spinoffs/` as
  projections reduced under a single per-run flock.
- **Run create kinds.** `code`, `spinoff`, `orchestrated`, `research`,
  `bugfix`, `technical-decision`, `make-skill`, `fan-out`, `orchestrate`.
- **Run merge.** `taskfleet run merge <run-id> [--report-file]`
  rebases + merges the worktree branch and submits the terminal node
  report in one call; supervisor tears down worktree + tmux window +
  branch automatically.
- **Supervisor.** Per-run watcher with a fresh-spawn grace window
  (no false watchdog misfires), terminal cleanup on `node.report`,
  detached-PTY support via `--headless` / `--tmux-session`.
- **Skill bundling.** 13 Claude Code skills bundled in the binary and
  deployed via `taskfleet skill install --force`:
  `taskfleet-overview`, `taskfleet-run-overview`, `taskfleet-spawn-spinoff`,
  `worktree-code`, `worktree-spinoff`, `worktree-merge`,
  `worktree-research`, `worktree-bugfix`, `worktree-technical-decision`,
  `worktree-make-skill`, `worktree-orchestrated`, `fan-out`,
  `orchestrate`. SKILL examples are CI-gated against the actual binary
  CLI surface.
- **Doctor.** `taskfleet doctor` reports schema, install, and
  skill-sync health (current: 63 ok / 0 fail).
- **AI-first CLI.** Every command follows the conventions in
  `AGENTS-AI-FIRST-CLI.md` (`--json` everywhere, JSONL logs, strict
  input validation, informative error envelopes, no interactive prompts).
- **`run create --agent-startup-timeout <seconds>`** (1–600, default 90).
  Forwarded to `create.sh`; higher than create.sh's own 30s default
  because taskfleet batch-spawns self-load the host and a fresh agent can miss
  a 30s window under load. Closes `run-create-agent-startup-timeout`.
- **Supervisor liveness surface.** `run show` / `run list` report
  `supervisor: {pid, alive}` (orphan detection), and `run merge` returns a
  machine-readable `supervisor: {state}` outcome
  (`alive | terminal | not-supervised | reattached | deferred`).

### Fixed (highlights from the MVP + follow-up campaigns)

- Append + projection are persisted under one flock; lock is held until
  every projection file is fsynced.
- **append + apply atomicity via `applied_seq` watermark** (`361839f`):
  the writer advances a per-manifest `applied_seq` only after every
  projection an event touches is fsynced; on next lock acquisition
  unapplied tail events (`seq > applied_seq`) are replayed before any
  new append, and an idempotency-key replay catches the projection up
  before returning. Legacy manifests self-migrate.
- **`events.jsonl` torn-write recovery** (`395ba03`): the recovery
  path truncates a partial trailing line at the byte boundary after
  the last fully-parseable event and warns about the discarded bytes.
- **`recover_last_seq` whitespace-tolerant** (`cc4ff46`): skips
  trailing whitespace-only lines instead of choking.
- **`run create --headless` no longer crashes create.sh** (`5ce764d`):
  Rust-side regression tests pin the `--parent-session` forwarding
  contract; the corresponding external integration fix landed alongside.
- **Orchestrated child worktrees fork from `--source-branch`**
  (`145905f`): not from `main`, so `/orchestrate` DAG dependencies hold.
- **Failed `create.sh` no longer leaves a phantom child** (`438aa29`):
  `child.spawned` is emitted only after create.sh returns success.
- **Supervisor cleanup `git worktree remove --force`** (`11a5850`):
  disposable untracked scratch in a worktree no longer orphans the
  worktree+branch.
- **`worktree-merge` recovers a renamed window** (`bfd7bfb`): the
  supervisor's cleanup falls back to a worktree-path lookup when the
  tmux window has been renamed or detached.
- Supervisor watchdog no longer false-fires during fresh agent spawns.
- Terminal cleanup completes the run AND removes the worktree, tmux
  window, and branch in one supervisor pass on `node.report`.
- `orchestrator.decision` and `discuss.critical` event kinds are accepted
  by the validator.
- **`run merge` no longer reports silent success when the supervisor is
  dead** (`supervisor-dead-merge-no-teardown`): it reads status +
  supervisor-pid liveness + the event log as one shared-locked decision
  and auto-reattaches a fresh supervisor when the recorded one is dead and
  the run is non-terminal, so the terminal report is consumed and teardown
  completes. Never silent — emits a warning plus the `supervisor: {state}`
  outcome above.
- **Blocked terminal report preserves the branch + worktree**
  (`blocked-report-deletes-branch`, high-severity silent data loss): a
  `node report` with `success: false` (the needs-a-human path) no longer
  force-deletes the worktree branch. Teardown is gated on the terminal
  outcome (`node_report_is_blocked`) plus a source-relative
  `git rev-list --count <source>..<branch>` safety net; `git branch -D`
  force-delete is reserved for a confirmed `run merge`
  (`via: "explicit-merge"`).
- `supervise_gates` + `e2e_spinoff` test binaries serialize on a process-
  wide file lock (`/tmp/taskfleet-test-supervise.lock` via `serial_test`'s
  `#[file_serial]`), removing a `cargo test --workspace` self-terminate
  flake. Closes `flaky-self-terminate-test`.

### Known gaps (carried to v0.2)

- `runwriter-batched-append-api` — a long-lived `RunWriter` guard with
  cached `next_seq` + batched fsync to cut the V4 append p99 (639ms) to
  the 10ms budget. Overlaps the just-landed `applied_seq` watermark /
  `LockedRun` witness / `AppendOutcome` primitives, so it lands cleaner
  once those have shaken out. Append p99 is well within budget for the
  one-per-action write cadence today; tight back-pressure loops (a
  supervisor batching many events) will surface it.
- `cancel-dead-supervisor-recovery`, `legacy-pid-identity-check`,
  `teardown-gate-trust-and-lifecycle` — supervisor-lifecycle follow-ups
  filed during the pre-1.0 hardening pass; none is a data-loss or
  correctness blocker (the blocked-report source-relative net already
  covers committed-work preservation on the cancel path).

[Unreleased]: https://github.com/jarimustonen/taskfleet/compare/v0.4.1...HEAD
[0.4.1]: https://github.com/jarimustonen/taskfleet/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/jarimustonen/taskfleet/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/jarimustonen/taskfleet/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/jarimustonen/taskfleet/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/jarimustonen/taskfleet/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/jarimustonen/taskfleet/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jarimustonen/taskfleet/releases/tag/v0.1.0
