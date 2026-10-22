# Taskfleet architecture

Taskfleet is a local, file-backed orchestration CLI. The `taskfleet` binary is
the only command and hidden subprocesses re-execute that exact current binary.

## Code map

- `crates/taskfleet-core/` owns durable run schemas, append-only events, flock
  witnesses, reduction, path containment, and atomic projection writes.
- `crates/taskfleet/` owns the CLI, run/node/event commands, harness selection,
  worker prompt generation, supervisors, merge recovery, doctor, and bundled
  skills.
- `contracts/worker-telemetry-v1/` is the runtime-neutral adapter contract. Its
  `TASKFLEET_*` variables and `taskfleet.worker-telemetry-adapter` id form its
  protocol vocabulary.
- `docs/decisions/` records architectural decisions. ADR 0002 establishes
  Taskfleet as the repository's sole identity.

## State and process boundaries

Each run has an append-only `events.jsonl`, reduced `manifest.json` and node
projections, and a per-run supervisor. Writers append and apply under the
`LockedRun` witness; multi-file readers take a shared lock. `run merge` is an
OID-recorded recoverable transaction and is the only success truth. The
supervisor is the canonical worktree/tmux teardown actor and preserves all work
outside an explicit merge.

Worker creation is native Taskfleet code in `run/spawn.rs`: it validates git
inputs, invokes `workmux add` with typed argv, obtains the exact tmux
socket/session/window/pane identity, copies the prompt, and owns rollback of all
partial side effects. Every worker runs through generated private launchers. An
inner launcher durably publishes a nonce-bound run/node/attempt PID and process
start identity immediately before `exec` of the recorded candidate; creation
validates that handshake and liveness before `node.created` and atomic run
publication. No Homebase script or executable-name/process-tree inference is a
production dependency. `git`, `tmux`, and `workmux` remain explicit external CLI
dependencies.

On Linux the user manager owns supervisor services and a state-home-specific,
foreground tmux server service. Workers belong to that server's cgroup, not the
creating daemon's. Validated private sockets under the user's runtime directory
remain reachable across PrivateTmp namespaces; the manager boundary also works
from PATH/USER/HOME-only callers. `run/attach.rs` exposes recorded display
identity on show/create and validates it for read-only native viewing or a
machine-readable descriptor; it never recovers or creates a server. Non-Linux
launch remains native. See
[Linux lifetime](docs/LINUX-LIFETIME.md) for prerequisites, discovery and tests.

Claude notification/progress subprocess hooks in `claude_hooks.rs` feed the
existing awaiting-input event contract independently of worker telemetry. They
never answer prompts or grant teardown authority. `run wait --check-in` bounds
parent inspection without inventing worker state from activity proxies. See
[Claude prompt signals](docs/CLAUDE-PROMPT-SIGNALS.md) for supported paths and unknowns.

Creator run and optional native-session attribution are immutable display facts,
not ownership or fan-out edges. See [Creator attribution](docs/CREATOR-ATTRIBUTION.md)
for the environment input, replay and worker-environment contract.

Read-only per-node worker transcript discovery is shared by `run show` and
`run list` in `run/transcripts.rs`; it projects existing native evidence, never
resume/control authority. See [Run transcripts](docs/RUN-TRANSCRIPTS.md) for
path anchors, availability, identity and live-to-archive semantics.

Terminal records and reports outlive worker, display and Git-resource teardown;
see [Terminal run history](docs/TERMINAL-RUN-HISTORY.md) for the consumer read
contract, display-only expiration boundary and regression evidence.

State lives under `~/.taskfleet` by default. `TASKFLEET_HOME` can select a
different root and `TASKFLEET_PROFILE` can select a configured profile. The
resolver never probes or moves any other product's state.

## Skills and generated prompts

Bundled templates live at `crates/taskfleet/skills/<name>/SKILL.template.md` and
are rendered with the package version at build time. Generic workflow identities
(`/worktree-*`, `/fan-out`, `/stint-*`) remain stable; Taskfleet-owned overview
and low-level skills use `taskfleet-*` names. New prompts and copyable commands
always use the canonical command and exact full run id.

Default installation writes Claude and pi layouts; Codex uses a flat prompt
layout. Canonical ownership markers and pi schema-v3 provenance record hashes.
The installer writes only the current Taskfleet skill catalog and does not
search for or rename unrelated installed files.

## Release boundary

The release contract publishes `taskfleet-core` followed by `taskfleet`, then
builds Taskfleet archives and updates the canonical Homebrew tap. The pinned
Shipshape wrapper keeps the release tag held until exact-main CI succeeds.
Repository development never publishes, tags, installs Taskfleet, or mutates a
tap.
