# crates/taskfleet

The Taskfleet CLI library and sole executable. Verb-noun structure (`run create`,
`node list`, `event tail`, `skill install`, etc.) follows `ai-first-cli-canon`.
Bundled skills live under `skills/<name>/SKILL.template.md` and are embedded via
`build.rs` plus `include_str!`.

## Canonical CLI and identity

`src/lib.rs` owns the parser/execution engine and `src/main.rs` is its only
binary entry point. Hidden self-execution is centralized in `src/self_exec.rs`
and always starts `current_exe()`; never add a product-name PATH lookup or a
second dispatcher.

`src/home.rs` is the sole resolver for `TASKFLEET_HOME`, `TASKFLEET_PROFILE`,
`TASKFLEET_HARNESS`, `TASKFLEET_LOG`, `~/.taskfleet`, and `.taskfleet.toml`.
Dispatch parses first, freezes environment values and repository-config bytes,
and resolves them before logging or command writes. Structured and text help
remain filesystem-pure. Do not add a second home, config file, namespace,
resolver, state mover, receipt, or transition warning.

## `doctor` binary build provenance

`doctor` always emits the stable `binary.commit` check first. Its optional `details` object exposes `binary_commit`, `repository_head`, and `comparison` (`match`, `mismatch`, `unavailable`, or `not_applicable`) so machine callers never scrape hashes from prose. When cwd is inside a Taskfleet checkout, a recorded build commit that differs from `HEAD` is a WARN, never a FAIL; branch and released-binary mismatches are legitimate. Outside this project's checkout, or when either reference cannot be established, the check remains informational. It never offers an autonomous fix or manages the installed binary.

## `skill install` native Claude, pi, and Codex targets

A default `skill install` targets **all three** supported runtimes. Claude Code receives `.claude/skills/<name>/SKILL.md`, pi receives `.pi/agent/skills/<name>/SKILL.md`, and Codex receives the flat, self-contained `.codex/prompts/<name>.md`. `--agent claude|pi|codex|all` selects explicitly. `--target <dir>` replaces the install base while preserving those layouts; `--dry-run` validates and reports the complete plan without writes. Every target is no-clobber unless explicit `--force` authorizes replacement. The legacy `--dest` exact-file override remains only for a named skill and a single runtime.

Claude and pi use native Agent Skill trees. Their `SKILL.md` bytes and any bundled companion resources are identical siblings. Codex has no companion files: required resource content is appended into its one prompt and sibling links become in-document links. The current catalog bundles no companions, but the generic renderer and tests preserve this contract. Default-path pi lifecycle remains tracked in the out-of-band provenance record; pi carries no in-dir marker. See `src/skill.rs` `cmd_install`.

### pi mirror lifecycle — out-of-band provenance (`pidev-pi-skill-lifecycle`)

The pi dir may hold no `.taskfleet-managed` marker, so its lifecycle is keyed on a single **out-of-band** JSON record at `<resolved Taskfleet home>/state/pi-installed-skills.json`. As of **schema v3** the record is a **flat per-file model** — `{ schema_version, skills: { <name>: { cli_version, files: { <relpath>: { sha256, kind: "skill"|"companion" } } } } }` — where every mirrored file (the `SKILL.md` body AND each companion sibling) is one independent `files` entry keyed by relpath. The body is no longer an ownership root nesting companions under it; that pre-v3 nesting (`{ sha256, cli_version, companions: { <file>: sha } }`) forced several lifecycle point-fixes (a companion written while the body write was skipped had no record to attach to; prune coupled companion cleanup to body divergence), which the flat model removes (issue `pi-provenance-flat-file-model`). **Read/upgrade path:** loading a legacy v1 (bare `sha256`/`cli_version`) or v2 (`+ companions` map) record reconstructs the `files` map from the legacy fields in place (`RawPiSkillRecord` via `#[serde(from)]`), so old records keep working; the strict future-schema guard still fails an install closed on a record newer than v3. On every pi write, the record is union-merged: a SKILL.md write inserts/refreshes the `SKILL.md` file entry (`kind: skill`) and the skill's `cli_version`, a companion write files itself directly (`kind: companion`, creating the skill entry if the body write was skipped) — so a targeted single-skill install never forgets the rest of the managed set. It is the SOLE authority for two decisions, both of which **never touch a pi dir we did not write** (a user's hand-authored pi skill is never recorded):

- **Prune** (gated on the same full-catalog `--force` as the Claude dir prune): each tracked file of a de-registered skill is handled **independently** (flat per-file — no privileged body). A file is deleted only if its on-disk bytes still hash to the recorded value (proof it is our unmodified copy); a **diverged** copy (user-edited since we wrote it), a symlink, or a squatting dir is left in place and dropped from tracking (`pi_mirror_diverged` / `pi_companion_diverged` / relinquished), and a failed delete keeps the file tracked for a retry. **Companions are pruned BEFORE the `SKILL.md`** so the per-skill dir can empty out; unlike the pre-v3 model, a diverged body no longer shields the companions — an unmodified companion is still pruned even when the body diverged, and the body is deleted even when a companion delete failed (the Kept companion simply stays tracked, never stranded). Each file that is deleted/relinquished/absent is removed from the record's `files` map; the skill entry is dropped once `files` is empty. Finally the now-possibly-empty per-skill dir is best-effort removed (`remove_dir`, non-recursive — a user sibling, or a surviving diverged/Kept file, is preserved).
- **`doctor` drift** — `skill.sync.<name>.pi` (older/newer/unparseable/edited via the recorded hash) + `skill.orphan.<name>.pi` (de-registered but still on disk), plus the **companion** arms `skill.sync.<name>.pi.<file>` (forward drift of each bundled companion vs the embedded body — content-identity in-sync signal, same as the codex companion check) and `skill.orphan.<name>.pi.<file>` (a companion the record tracks that the binary no longer bundles). All gated on the record being non-empty so a host that never dual-homed into pi stays 0-warn. ALL pi arms are **advisory — no autonomous `FixAction`** (unlike the Claude older-drift arm): the fix applier runs `skill install <name> --force`, which dual-homes and would force-overwrite the Claude copy too, so autofixing pi drift could silently downgrade a deliberately newer/edited Claude copy. Symmetric with the codex checks. Mirrors the Claude/codex checks in `src/doctor/checks/skill.rs`.

Integrity: the record is **loaded and validated before any file is written** (`load_pi_provenance_for_write`) — a corrupt or future-schema record **fails the install closed** (never silently laundered to empty and overwritten, which would erase tracking for every mirror). Record-sourced skill names are validated as single path components before any `join`→`remove_file` (`is_simple_skill_name`), and the empty-dir cleanup is bound to `<pi-root>/<name>/`. The record read-modify-write is **unlocked** (parity with the Claude/codex markers): concurrent `skill install` runs can lose one another's additions, so mutation commands are not meant to run concurrently.

## Insta snapshot test loop

Many integration tests in `tests/` use `insta` for envelope / help / skill-catalog snapshots. After any CLI surface change (added flag, renamed verb, new bundled skill, edited error message), running `cargo test -p taskfleet` produces `.snap.new` files for every diff. Accept them with:

```bash
find crates/taskfleet/tests/snapshots -name "*.new" -exec sh -c 'mv "$1" "${1%.new}"' _ {} \;
cargo test -p taskfleet
```

Often takes **2–3 rounds** because the first accept-pass reveals further drifts only visible once earlier snapshots settle. Re-run the loop until `cargo test -p taskfleet` is green.

**A workspace version bump is a snapshot change too.** Five snapshots bake in the literal crate version: `envelope_snapshots__version_{text,json,jsonl}.snap` and `envelope_snapshots__skill_list_{json,jsonl}.snap`. Bumping `[workspace.package] version` in `Cargo.toml` stales them exactly like a CLI-surface edit — run the accept loop above (or `cargo insta test --accept -p taskfleet`) after the bump, or `cargo test` goes red. `scripts/check-version-snapshots.sh` (also a CI job) fails loudly on a version/snapshot mismatch as a fast pre-publish guard; the release-mechanics obligation is in `OSS-RELEASE.md` alongside the CHANGELOG-finalize step.

## Skill catalog: edit pin-test explicitly

The bundled-skill list is hardcoded in `tests/skill.rs::skill_list_json_pins_catalog_shape`. When adding or removing a bundled skill, **also edit that test's `vec![...]` literal** — the snapshot loop above will NOT catch it (different test). Forgetting this gives a single failing test on the next run.

## Test-spawn hygiene

Integration tests that exercise `run create`'s production path spawn real `taskfleet supervise` subprocesses. The shared `tests/common/mod.rs` `TestHome` fixture reaps them on Drop — use it (`let home = TestHome::new()`) for any new test that goes through `bin(&home)`. `NativeSpawnTools` stubs only the explicit `git`/`tmux`/`workmux` CLI dependencies, so tests still execute the production materializer, generated launchers, PID handshake, publication transaction, and supervisor. There is no integration-test create-script backend.

After `cargo test -p taskfleet` finishes, `pgrep -lf "taskfleet.*supervise"` from the workspace `target/debug/` path should return nothing. Any survivor is a missing-fixture bug in the new test.

`NativeSpawnTools` runs each production-path test from a real disposable git repository and supplies a private fake tmux socket/server rooted in its own cryptographically unique `TempDir`. Its Drop guard identity-checks and stops the launched candidate, clears fake tmux state, and force-removes only fixture-owned disposable paths. Interruption tests additionally own every deliberately blocked child PID. Never point these tests at the source checkout or a shared tmux session.

For an explicit real-pi operator check, use `scripts/native-spawn-smoke.sh` after building `target/release/taskfleet`. It uses a bounded no-tools prompt, disposable HOME/repository/Taskfleet state, and a private `tmux -L` server. Its trap cancels the run, stops the private server/supervisor, removes the sandbox, and rejects any before/after change to source worktrees/`wt/*` refs, default-server tmux windows, or external run roots. Do not improvise a live smoke from an implementation issue prompt.

## End-to-end spinoff harness

`tests/e2e_spinoff.rs` drives one full autonomous-spinoff round-trip on every run — native `run create --kind spinoff --headless` (real generated launcher, durable PID handshake, detached supervisor) → live stub candidate → `run merge` → supervisor rolls the run up to `done`, tears down, and exits. It asserts the canonical event sequence (`run.created`, `node.created`, `supervisor.started`, `node.report`, `run.status`, `supervisor.exited`) and terminal manifest.

## Persistent worker-session retention

`[tmux]` in user `config.toml` may opt autonomous workers into one named
`default_session` plus `persistent = true`, `completed_window_ttl`, and
`completed_window_max`. No section means historical foreground placement and
immediate cleanup. Explicit `--tmux-session` and `--headless` placement win;
interactive runs remain foreground by default. The selected name is a tmux
SESSION on the invocation's actual socket, never a socket selector.

The create-time policy and exact socket/server-PID/start-marker/window/pane
identity are recorded. `supervise::evidence` completes first, then
`session::retain_completed_display` removes only the owned worker pane (leaving
unrelated split panes), creates one dead `remain-on-exit` display in the
surviving source repo, and records its option marker. Names show repo + short run
+ purpose but confer no authority. `taskfleet session maintain --output json`
scans canonical state without `$TMUX` or repo cwd, rechecks each run under its
shared lock and every exact tmux identity/marker, and applies TTL/count bounds.
It never starts a missing server and never removes archives or Git-preserved
work. Persistent sessions bypass `cleanup_managed_session`'s legacy synthetic-
shell heuristic. Across recorded policy generations in one exact session, the
strictest (lowest) completed-window maximum wins. Maintenance removes only the
exact owned pane; if it is the session's final pane, it remains as the inert
session anchor rather than destroying the persistent session.

A Homebase-managed systemd user timer should run every five minutes with an
explicit environment and a hard service timeout, for example:

```ini
[Service]
Type=oneshot
Environment=HOME=/Users/jari
Environment=TASKFLEET_HOME=/Users/jari/.taskfleet
Environment=PATH=/opt/homebrew/bin:/usr/bin:/bin
ExecStart=/opt/homebrew/bin/taskfleet session maintain --timeout-secs 30 --output jsonl
TimeoutStartSec=40
UMask=0077

[Timer]
OnBootSec=5min
OnUnitActiveSec=5min
Persistent=true
```

Homebase owns the actual unit installation and tmux-server ordering. The unit
must not use `Requires=`/`PartOf=` against the shared server or start a tmux
server when its recorded generation is absent.

## Durable native Pi worker evidence

Every Pi launcher owns session selection. It assigns a UUID, creates the exact
native v3 session header from the launcher's real cwd in the run-bound private
source at `.creating/pi-sessions/<run-id>/`, and invokes Pi through its supported
`--session <path>` surface. That source path is stable across atomic run
publication, so Pi can start immediately without replacing the assigned identity
with an implicit/newest session. Profile argv containing session-selection flags
is rejected rather than silently overridden.

On a terminal transition, `supervise::evidence` captures evidence **before**
window/worktree/session cleanup: a byte-identical original JSONL transcript, a
separate resume JSONL whose header alone changes `cwd` to the surviving source
repo, the final stable pane history, and the exact terminal report. The
`worker.evidence.archived` / `worker.evidence.failed` events fold under the run
lock into `Node.evidence`; `run show` exposes the explicit
`pending|failed|complete` status, a state-root-relative live source, and
run-relative artifact paths. Capture checks the native header against the
launch-time UUID/cwd, stops an identity-checked live writer without destroying
its pane, and fails closed if the file or writer state changes across transcript
and pane capture. A failure is durable, backs off up to 64 seconds, vetoes
cleanup, and retries (including after restart); partial artifacts are never
labeled complete. The original transcript is never rewritten. Runs self-exec
their creating binary; manually reattaching one with a pre-evidence binary is an
unsupported downgrade that cannot enforce this newer cleanup prerequisite. Do
not infer sessions from mtimes, scan for a newest file, or integrate a harness
process manager.

## `run create --profile` / legacy `--harness` (worker selection)

Executable profiles are defined only in the user-owned
the resolved Taskfleet home's `config.toml` as `[profiles.<name>]`. Each strict profile has
`description`, `capability = "fast" | "capable" | "ultra-capable"`,
`residency = "local" | "remote"`, and 1–8 ordered `agents` with bounded argv.
Candidates use `harness = "pi" | "claude"`; only pi may declare
`telemetry = "worker-v1"`. Canonical `<repo-root>/.taskfleet.toml` (or the bounded legacy fallback) is parsed through a
selection-only schema and may contain `[profile]` defaults/per-kind names, but
any executable definitions, argv, adapter paths, or residency fields fail.

Selection precedence is `--profile`/legacy `--harness` > mirrored environment >
repository per-kind > user per-kind > repository default > user default. Profile
and legacy harness selectors at one level conflict. A legacy harness selector is
only an alias for a same-named user profile whose candidates all use that
harness; it never synthesizes argv. Installations with no `[profiles]` retain the
pre-profile harness behavior.

Before mutation, candidates are checked in order for `executable_missing`, then
(for autonomous runs) `autonomous_harness_unsupported`, then
`telemetry_unsupported`. Autonomous accepts only pi+`worker-v1`; explicit
interactive accepts pi or Claude. Fallback cannot leave a profile (and therefore
cannot change residency), launch/runtime failures never advance it, and retry
reads the recorded candidate rather than current config. Dry-run/create/run show
surface the compact selection; old/no-profile manifests show
`legacy-unrecorded` without invented history.

Every create writes private outer and inner per-attempt launchers and passes only
the outer absolute path to `workmux add -a`. The outer launcher re-enters the
exact current executable through hidden `run-worker` for autonomous candidates.
The inner launcher invokes the hidden handshake helper with its own PID, then
immediately `exec`s the recorded candidate argv; argument boundaries and
workmux's existing `-- <prompt>` suffix remain unchanged. The helper atomically
and durably writes a nonce-bound run/node/attempt/PID/start-identity/pane record
under the mode-0700 staging run and blocks until run+node publication. Creation
validates binding, PID bounds, start identity, liveness, and exact qualified tmux
identity before appending `node.created` and publishing. Retry regenerates this
launcher solely from `manifest.agent_selection` and the new absolute attempt,
never current config. Legacy manifests without a selection retry through the
recorded built-in harness command rather than executable-name inference.

`run show` telemetry rows derive `requirement` solely from the manifest's explicit
lifecycle (`required` autonomous, `optional` explicit-interactive) and `support`
solely from the recorded candidate (`configured` only for pi+`worker-v1`, else
`unsupported`). Sample presence/freshness never changes either field.

## `config` noun (read-only config inspection)

`taskfleet config path` / `config show` inspect the config surface (§8);
neither ever mutates `config.toml`. Lives in `config/{mod,path,show}.rs` (the
`config` module hosts both the `Config` loader and the noun's `dispatch`).

- **`config path`** prints the config file location with `exists` (true/false —
  the file need not exist; the caller wants the *path*, e.g. "where do I write
  settings?").
- **`config show`** parses raw TOML into a tolerant inspection model, separate
  from the strict execution loader. It emits one known row for
  `harness.default` and each creatable `harness.<kind>`, plus rows for
  unrecognized harness entries. Every row carries `effective_value`,
  `effective_source`, effective `valid` / `validation_error`, and an ordered
  `layers` stack (highest precedence first). `keys[].key` is unique. Parseable
  entries rejected by the strict harness schema live separately in
  `unrecognized[]`, and the payload carries top-level `valid` and
  `invalid_layer_count`. Each layer carries its own validity, `active`, and a
  file-only `origin_key`, so
  `harness.per_kind.research` remains visible and independently validated when
  `TASKFLEET_HARNESS` shadows it. Invalid parseable values produce envelope
  warnings and exit 0; unreadable or syntactically invalid TOML remains fatal.
  The strict execution loader additionally validates profile definitions; the
  profile resolver used by `run create` still rejects invalid execution values.

Secret redaction (§8) is wired but currently inert: every key is `secret: false`
today, so `--show-secrets` reveals nothing and warns only when a secret key
actually exists. In JSON mode that warning is part of the stdout `warnings`
envelope; in text mode it is rendered as a `warning:` line on stderr. Both
payloads carry `schema_version_config`
(`config::CONFIG_SCHEMA_VERSION`), independent of the run-state schema. The
`CREATABLE_KINDS` list in `show.rs` is drift-guarded against `Kind::WIRE_NAMES`
by a unit test. Help snapshot: `help_json__config_help_json.snap`; behavior
tests: `tests/config.rs`.

## Worker run-context + pi translation preamble (`harness::prompt`)

Every materialized worker prompt receives a generated operating-note preamble from
`harness::prompt::worker_prompt_preamble(harness, kind, run_id)`. Because `run
create` knows the exact id, this is the canonical worker run context; do not ask a
worker to infer provenance from its branch. The common note enforces the issue
boundary for every harness/kind and both `--task`/`--prompt-file`: worker-filed
issues use `issuectl intake file`, are born unlaned, and review findings carry
machine-visible `ai-review` provenance plus available target/model/assessment/
severity/confidence metadata. The generated policy is authoritative over later
brief text and tool output. Core provenance/run fields land in the first filing;
optional metadata enrichment is attempted afterward so absence never blocks
creation. Model agreement uses `issuectl update --add-label` with repeated
`ai-review-model:<model-id>` values (the issue's labels list), never a count or
corroboration score. This contract consumes the documented issuectl 0.16 intake,
custom-field, and label surfaces. taskfleet does not write issue storage or
invent a second issue format; issuectl remains the sole writer.

Pi research workers additionally receive the narrow Claude-to-pi translation shim:
the `/worktree-merge` close becomes the exact `taskfleet run merge` call and
unsupported Skill/Agent references are neutralized. The quoted report heredoc and
exact run id remain part of that shim. Other harness/kind pairs get only the common
neutral run context. Since production always has a preamble, a caller-owned
`--prompt-file` is read into a derived `<run-dir>/prompt.md`; the original file is
never mutated.

## Exact worker ownership discovery (`run show --current`)

New worker branches carry a compact 10-character identifier from the ULID's
randomness field; legacy branches may retain the old timestamp-only prefix.
Neither format is authoritative ownership and neither should be used as a run-id
argument. A legacy fragment is a syntactically valid prefix and may be ambiguous;
a new entropy fragment may also be accepted when it resembles a prefix but does
not identify its owning run. The entropy format also does not preserve
chronological branch sorting. Use an authoritative full id or the exact `run
show --current` ownership resolver. It finds the git worktree root without
shelling out, then scans durable node projections under each run's
shared lock for the exact canonical `worktree_path` and corroborating branch. It
returns the ordinary `run show` payload for exactly one owner. Missing,
duplicate, stale or absent branch, malformed node, detached HEAD, and unreadable
evidence have informative errors and all fail closed. Existing runs
remain compatible when they carry the normal recorded worktree path and branch;
a legacy branchless projection is refused because a reused path alone cannot
prove ownership. Every bundled worker closing recipe uses this surface; freshly
generated prompts also carry the already-known full run id.
