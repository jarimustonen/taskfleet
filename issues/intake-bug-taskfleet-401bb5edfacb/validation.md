# Live-worker message validation — 2026-10-24

## Change and self-review

Native `node.created` initializes Pending even though it already records the
worker's session. Message eligibility now accepts Pending and Running, not just
Running. Terminal run/node and recorded worker-exit refusals have explicit
reasons. Ownership, selected Pi harness, native session, attempt-specific socket,
matching acknowledgement identity, and metadata-only audit remain unchanged.
No PID, spinner, prompt, or tmux-input inference was added.

The launcher in `crates/taskfleet/src/run/spawn.rs` already writes the bundled
`message-extension.ts`, exports the attempt socket and session identity, and
passes `--extension "$TASKFLEET_MESSAGE_EXTENSION"` to Pi. No launcher or
extension repair was necessary. The extension acknowledges `sendUserMessage`
steering acceptance, never reading or compliance.

## Tests

`cargo test --release -p taskfleet --test run_message`: all four tests passed.
These exercise both Pending and Running through the CLI and a matching socket
acknowledgement; terminal run/node states and recorded exits; caller ownership
and Claude; missing and wrong-attempt adapters. Existing message unit tests
cover wrong acknowledgement identity, stale sockets, and input validation.

## Real Pi proof

Command: `python3 scripts/native-message-smoke.py` after `cargo build --release`.
The script invokes this checkout's `target/release/taskfleet`, never the installed
binary. It copies the current installed configuration into a private disposable
state home and uses its unchanged default profile: `capable`, user-default,
Pi command `["pi"]`, telemetry `worker-v1`. The real Pi installation and its
existing authentication remain in use; no skills/configuration were installed
or changed. Git source and worker worktrees are throwaway repositories. Linux
uses the production manager-backed tmux server keyed to this disposable home.

Successful run: `01m5qkxb2rcbsfhmcgbpr609ya`.

1. Native create returned a Pending node. The bundled extension's attempt-0
   socket appeared and the assistant produced `MESSAGE_READY` in its explicit
   native session, finishing its first turn.
2. `run message` delivered the separate instruction to write an action marker:

   ```json
   {"run_id":"01m5qkxb2rcbsfhmcgbpr609ya","node":"n-0001","message_id":"0ea69210-3b13-4baa-8d8b-0b8797c080bd","delivery":"accepted","read_or_acted":"unconfirmed","attempt_seq":4}
   ```

3. The worker used its tool to create `message-acted.txt` containing exactly
   `MESSAGE_ACTED`. This independently proves action, not merely acceptance.
4. Event 4 (`worker.message.attempted`, adapter `pi-extension-v1`, attempt 0)
   and event 5 (`worker.message.result`, delivery `accepted`, attempt_seq 4)
   recorded the matching message id without the instruction body.
5. `run cancel` completed supervisor teardown. `run discard --force --reason
   "dispose isolated live-message smoke files"` recorded authorization at seq 13
   and returned `discarded:true`, `removed:{worktree:true,branch:true}`. The
   worktree was verified absent. The exact private tmux server was stopped and
   the disposable state/source directories removed.

Local raw success output: `history/message-live-proof-3.log` (supplementary,
not required to understand this record).

Two initial fixture attempts failed before launching Pi because the host's
seat-routing `workmux` wrapper looked under the disposable HOME and could not
find its native executable (`workmux_add_failed`, exit 127). The script now
selects the existing native executable explicitly on Linux and respects the
production state-home-derived tmux socket. Both exact fixture-only servers
from those attempts were removed; no shared server was stopped. The third
attempt passed end to end. No delivery implementation failure was hidden.

## Release

This source fix does not change the installed fleet. The conductor must merge
it, plan/cut a patch release through `scripts/shipshape-release.sh`, validate the
exact release commit and verify the publication legs. The approved contract
publishes taskfleet-core then the exact-pinned CLI to crates.io, plus GitHub
binaries and the canonical Homebrew tap. Fleet units subsequently upgrade via
Homebase's declared distribution/convergence path. This worker does not cut a
release or install the resulting tool.

## Targeted review and gate

Focused self-review plus `/llm-review` completed: finder `claude-fable-5-1`,
then verifier `gpt-6-astra` for the surviving process-boundary finding. The
review identified one informative-error defect: a bundled adapter whose socket
has not bound yet was called unsupported/old. Missing extension now remains
`message_unsupported`; absent current-attempt socket is `message_undeliverable`,
explicitly startup/shutdown unknown and no message sent. Tests cover those
separately, including refusing a different attempt's socket. The suite now has
five integration tests.

Two smoke findings were checked and dropped against current code: Linux cleanup
already targeted the state-home-derived socket, and initial readiness already
parsed exact assistant `MESSAGE_READY` text. These repairs preceded the successful
proof and the completed review. No unresolved actionable findings or follow-up
issues remain. Local review artifacts are `history/review-live-message.md` and
`history/assessment-live-message.{json,md}`; this summary stands without them.

The first full-gate attempt stopped on Clippy's `assert_is_empty` lint in the new
test; the assertion now prints unexpected stdout via `assert_eq!`. A second
attempt was intentionally stopped during compilation to incorporate the reviewed
error-classification fix before validating a fresh exact commit. The final
`scripts/validate-local-release.sh` result is recorded in the issue's closing
comment; closing/merging is contingent on that complete gate passing.
