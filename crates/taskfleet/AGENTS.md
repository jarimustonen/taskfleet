# crates/taskfleet

This crate is the whole product surface: the `taskfleet` binary, its JSON
contracts, the supervisors, and the bundled skills that other agents run. Two
things are at stake in every change here. The CLI output is a contract that
skills and other agents parse, so a changed field or message is a public
change even when it looks cosmetic. And the code touches things on the
developer's machine that are precious and outside the repository: installed
skill directories, the tmux server the user works in, git worktrees, and the
run state under `~/.taskfleet`. Most of what follows exists to keep those two
facts in view.

The module headers (`//!`) are thorough and current. Read the header of any
module before changing it; this file only holds what the headers and the root
`AGENTS.md` do not say. The CLI conventions themselves are the
`/ai-first-cli-canon` skill (the `§` numbers in code comments refer to it).

## One binary, one home

Every hidden re-entry (detached supervision, reattach, doctor fixes, the
worker shim) goes through `src/self_exec.rs`, which starts `current_exe()`.
Looking the product name up on `PATH` could land on a different installed
version, and a run that mixes binaries cannot enforce newer invariants such as
evidence capture. `src/home.rs` is the only reader of the `TASKFLEET_*`
environment variables and the only chooser of the repository config file, and
it freezes its answers for the process so a command cannot switch state roots
midway. Help output does not touch the filesystem, so `--help` works with no
home and no config.

ADR 0002 made the identity single: no second home, config file, resolver,
state mover, or transition warning for any earlier name. This is enforced
mechanically. `scripts/check-canonical-identity.sh` fails the gate on any
tracked path or text that mentions the retired name, comments and test
fixtures included.

## Skills: catalog, installer, and the pi record

A bundled skill is `skills/<name>/SKILL.template.md`, rendered by `build.rs`
with the crate version and embedded with `include_str!`. Three places name the
catalog by hand and none of them notices the others: the `SKILLS` constant in
`src/skill.rs` (a template that is not listed there never ships), the `vec!`
literal in `tests/skill.rs::skill_list_json_pins_catalog_shape` (pinned on
purpose so a silent addition or removal fails a test), and the skill-list
snapshots. Adding or removing a skill means touching all three; the snapshot
loop below catches only the third.

`skill list --output json` describes the install contract (layouts, flags,
no-clobber default), and the same test pins it. The parts worth knowing before
editing the installer: a plain install targets Claude, pi, and Codex together;
Claude and pi get byte-identical skill trees while Codex gets one
self-contained prompt with any companion files inlined; nothing is overwritten
without `--force`; and `--target` is an isolated base that never touches
Taskfleet's own state. `--dest` survives only for one named skill on one
runtime.

The pi mirror is the delicate part. Its directory carries no ownership marker,
so the only proof that Taskfleet wrote a file there is the out-of-band record
at `<home>/state/pi-installed-skills.json`, a flat per-file map of hashes
(schema v3; older nested records are upgraded on read, and a record newer than
v3 or corrupt fails the install closed rather than being replaced by an empty
one, because that would erase tracking for every mirror at once). Everything
that deletes in the pi tree follows from that: a file is removed only when its
bytes still hash to the record, a user-edited copy is dropped from tracking
and left alone, and a hand-authored pi skill was never recorded so is never
touched. `doctor` reports pi drift but offers no automatic fix, because the
fix command reinstalls to all runtimes with `--force` and would overwrite a
deliberately newer or edited Claude copy. The record's read-modify-write is
unlocked, like the Claude and Codex markers, so two installs at once can lose
each other's additions. The history and the reasons for the flat model are in
the issues `pidev-pi-skill-lifecycle` and `pi-provenance-flat-file-model`; the
mechanics are in `src/skill.rs` around `cmd_install` and in
`src/doctor/checks/skill.rs`.

Repository work leaves the developer's installed skills alone (root
`AGENTS.md`), and the tests honour that by pointing `HOME` and
`TASKFLEET_HOME` at temp dirs. A test that installs to a real home would edit
the user's tools.

## Tests: snapshots, spawn hygiene, end to end

Many integration tests lock the envelope shapes, help text, and skill catalog
with `insta`. Any change to the CLI surface, including an edited error message
or a workspace version bump, leaves `.snap.new` files after
`cargo test -p taskfleet`. Accept and re-run:

```bash
find crates/taskfleet/tests/snapshots -name "*.new" -exec sh -c 'mv "$1" "${1%.new}"' _ {} \;
cargo test -p taskfleet
```

Expect two or three rounds, because settling one snapshot exposes the next.
Read each accepted diff: the snapshot is the contract, and a blanket accept
can bless a dropped field or a schema-version bump that no test would then
catch (`tests/envelope_snapshots.rs` asserts a few invariants structurally as
a backstop, not a substitute). Five snapshots bake in the literal crate
version, so a version bump stales them like any other surface change;
`scripts/check-version-snapshots.sh` exists because that once surfaced only
on `main` after a tag. The release gate also refuses to run with any
`.snap.new` left in the tree.

Tests that exercise the real `run create` path spawn a real supervisor, which
double-forks and `setsid`s away from the test process, so neither `waitpid`
nor a group kill reaches it. The `TestHome` fixture in `tests/common/mod.rs`
reaps by the pid files under its own home on drop; a production-path test
that uses a bare `TempDir` instead leaks a supervisor. `NativeSpawnTools`
stubs only `git`, `tmux`, and `workmux`, runs each test from a disposable
repository, and strips `TMUX` so a test that forgets to declare `--headless`
or `--tmux-session` fails deterministically instead of landing in the
developer's real session. Everything else (materializer, generated launchers,
PID handshake, publication, supervisor) is production code. After a test run,
a surviving `taskfleet supervise` process from `target/debug` is a
missing-fixture bug; scope any `pgrep` to that path, since the same pattern
matches supervisors from every repository on the machine.

`tests/e2e_spinoff.rs` drives one complete headless spinoff (create, live
stub worker, `run merge`, roll-up, teardown) and asserts the canonical event
sequence, so a change to the lifecycle shows up there first. For a check
against a real pi, `scripts/native-spawn-smoke.sh` runs a bounded prompt from
a built `target/release/taskfleet` inside a disposable home, repository, and
private tmux server, and fails if anything outside that sandbox changed. An
improvised live smoke from an issue prompt has none of that containment and
runs a real agent against the real checkout and tmux server.

## Worker launch: profiles, launchers, evidence

Executable profiles live only in the user's `config.toml`; the repository's
`.taskfleet.toml` may choose profile names but is parsed through a
selection-only schema that rejects any argv, adapter path, or residency. The
reason is that a cloned repository must not be able to make `run create`
execute a command of its choosing on the user's machine. Selection precedence,
the `[profiles.<name>]` schema, and the legacy `--harness` alias are documented
in `src/config/mod.rs` and `src/harness/profile.rs`. Two properties are easy
to break from a distance: fallback between candidates is decided once at
create time from static eligibility (missing executable, autonomous harness
support, telemetry support), so a launch or runtime failure never advances to
the next candidate; and a retry regenerates its launcher from the recorded
`manifest.agent_selection`, never from current config, so a run reproduces
its own launch. Autonomous runs accept only pi with the `worker-v1` telemetry
adapter (`contracts/worker-telemetry-v1/`); explicit interactive runs accept
pi or Claude.

Launch is native code in `src/run/spawn.rs`. The generated launcher publishes
its own PID through the hidden `worker-handshake` command immediately before
it `exec`s the recorded candidate, so the recorded PID is the agent's without
any process-name or process-tree inference, and autonomous candidates are
wrapped in the hidden `run-worker` shim that records the true exit status as
a `worker.exited` fact. This is the "told, not guessed" model of ADR 0001; a
proposal to recover a worker's identity from names, trees, or timestamps is a
step back toward what that ADR removed.

Every pi launcher also owns its session: it assigns the UUID, writes the
native session header into the run's private staging area, and starts pi with
`--session <path>`. That is why a profile whose argv contains session
selection flags is rejected rather than overridden, and why evidence can be
exact instead of "the newest session file". On the terminal transition
`supervise::evidence` archives the transcript, a resume copy, the final pane
history, and the terminal report before any cleanup. A failed capture is a
durable `worker.evidence.failed` fact that vetoes cleanup and retries with
backoff, so a run that seems stuck at teardown is usually waiting on
evidence, and the fix is to look at that event rather than to force cleanup.
The original transcript is never rewritten. Reaching into pi's session store
by mtime or newest file, or into a harness process manager, was considered and
rejected (root `AGENTS.md`, homebase ADR 0011).

## Sessions and retention

`[tmux]` in the user's `config.toml` opts autonomous workers into a named
persistent session with bounded inert displays for completed runs; the fields
and their validation are in `src/config/mod.rs`, and an explicit
`--tmux-session` or `--headless` on `run create` always wins. Without the
section, workers take the historical foreground placement and immediate
cleanup.

The retention code in `src/session.rs` is deliberately paranoid about
identity: it records the exact socket, server PID, window, and pane at create
time and rechecks all of them, plus its own option markers, before it removes
anything. Names carry no authority. A window-name prefix match once found the
user's own pane, and the same session hosts the user's real shells. So the
rules that look excessive (remove only the owned pane, close the workmux
companion shell only when it is provably untouched, leave a modified or shared
window alone, keep the last pane as an inert anchor rather than killing the
session) each protect something the user was using. `session maintain`
applies the TTL and count bounds from a timer; it scans state without `$TMUX`
or a repository cwd, rechecks every identity under the run's shared lock, and
never starts a tmux server or removes archives or git-preserved work. Homebase
owns the timer unit. A unit that binds to the shared tmux server with
`Requires=` or `PartOf=` would let maintenance start or restart the user's
server, which is the one thing the command itself refuses to do.

## Worker prompt context and ownership discovery

Every materialized prompt gets a preamble from
`harness::prompt::worker_prompt_preamble`. It is the right place for worker
policy because it is the only text that knows the exact run id and reaches
workers launched from a custom `--task` or `--prompt-file`, which no bundled
skill does. Today it carries the closing identity and the issue-filing
boundary (worker-filed issues go through `issuectl intake file`, are born
unlaned, and carry machine-visible `ai-review` provenance); the full text is
in `src/harness/prompt.rs`. issuectl remains the only writer of issue storage.
A caller's `--prompt-file` is copied to `<run-dir>/prompt.md` so the preamble
can be prepended without mutating the caller's file.

A worker branch name contains a short display fragment of the run id, not the
id. A legacy fragment can be an ambiguous prefix, a new one may not resolve at
all, and neither sorts chronologically, so nothing should pass a fragment as a
run-id argument. Generated prompts carry the full id, and `run show --current`
(`src/run/ownership.rs`) resolves the owning run from the durable node
projection whose recorded worktree path is the current worktree root, with
the branch as corroboration; it fails closed on anything ambiguous because a
guessed owner would merge or discard the wrong work.

## Inspection surfaces

`config path` and `config show` are read-only and documented in
`src/config/mod.rs` and `src/config/show.rs`. The one design point to
preserve: `config show` parses the file tolerantly and separately from the
strict execution loader, so an invalid value stays visible and individually
validated even when an environment variable shadows it, while `run create`
still refuses it. Secret redaction is wired but inert until a key is marked
secret.

`doctor` runs its checks in a fixed order with `binary.commit` first. A build
commit that differs from the checkout's `HEAD` is a warning, since released
binaries and branch work differ legitimately, and `doctor` never manages the
installed binary. The `--fix` subset is drift-only and, for the reasons above,
excludes pi.
