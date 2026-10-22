# Worker display attachment delivery

## Product behavior

`run show` and `run create` expose `data.attach` with an explicit discovery state,
recorded socket/session/window/pane/server-generation identity, and a shell-quoted
command pinned to this executable and state home. Discovery is not a liveness
claim. Full-ID `run attach` validates the exact recorded display; JSON/JSONL only
return the validated descriptor. Native viewing requires terminal stdin/stdout
outside tmux, and opens a read-only client. Inside tmux it refuses rather than
switch a potentially unrelated client. Missing, stale, incomplete, expired and
non-unique displays are explicit, with no default socket fallback or server start.

Linux worker placement and systemd lifetime remain unchanged. No alternative
placement flag, new skill, worker input injection, supervisor recovery or worker
teardown ownership was added. Caller-create changes are additive discovery only;
the predecessor preflight, preservation and discard paths are unchanged.

Native viewing uses an ephemeral independent session linking only the owned
window, not a grouped session. Client-local active-pane selection and `-Z` leave
the source current window, active pane and zoom unchanged. A hidden zoom target
is refused rather than changing another client's layout. The temporary viewer
expires after detach. A bounded explicit tmux setup acknowledgement prevents
post-attach setup errors from being reported as successful viewing.

## Review and assessment

The focused independent `/llm-review` used claude-fable-5-1 as finder and a narrow
gpt-6-astra verification at the shared-client selection boundary. `/assess-findings`
confirmed and fixed both candidates:

- Pane-targeted native attach changed the source session's selected window and
  shared active pane. This could redirect another client's next input. Private
  tmux probes reproduced it. The independent linked view above fixes it; a grouped
  draft was rejected because it could trigger source `destroy-unattached keep-last`.
- An intentionally unknown tmux command suppressed the intended stale-display
  message at parse time. A read-only runtime failure against an impossible numeric
  session ID now prints the useful diagnostic and fails nonzero.

No deferred findings or follow-up issues. Scratch reports, raw review outputs and
assessment JSON/Markdown are preserved under
`/var/tmp/taskfleet-attach-build/review/` before merge; they are supplementary to
the findings and fixes described here, not the issue's only evidence.

## Verification

Default deterministic tests in `crates/taskfleet/tests/run_attach.rs` invoke the
production CLI under stripped PATH with an explicit strict tmux stub. They cover
private/non-Linux-style sockets, quoted state homes and shell-sensitive session
names, copied commands, inside/outside tmux, missing/stale/generation-mismatched
and incomplete/malformed identities, no display/multiple nodes, terminal retained
and expired displays, native exact-pane read-only command construction, and setup
acknowledgement failure. `spawn_all_kinds.rs` also compares actual create/show
attachment DTOs after native materialization with declared dependency stubs.
Existing caller-create and session-maintenance regression tests remain enabled.

The opt-in `scripts/attach-view-smoke.py` takes explicit Taskfleet and tmux binary
paths. It seeds disposable state through the CLI event API, uses a private socket
and PTY, and starts no agent/supervisor. On tmux 3.6 it verified normal viewing,
hidden-target refusal and a visible zoomed target, unchanged source selections
and zoom, preservation of an unattached keep-last source, and viewer expiry.
This smoke is not an undeclared dependency of bare-CI tests.

Reviewed insta diffs add the attachment DTOs and attach help/default metadata;
existing fields, schema versions and catalog membership are preserved. No catalog
pin change is needed because there is no new skill.

Local evidence directory: `/var/tmp/taskfleet-attach-build/`.
The terminal worker report records the final command exit statuses and gate log.
Mandatory pre-merge gate, on the clean committed tree:

```sh
umask 077
export TMPDIR=/var/tmp/taskfleet-attach-build CARGO_BUILD_JOBS=2
export PATH="$(dirname "$(rustup which --toolchain stable cargo)"):$PATH"
export DIST_BIN=/absolute/path/to/dist  # cargo-dist 0.33.0
scripts/validate-local-release.sh
```

## Serial release notes

`CHANGELOG.md` Unreleased notes consolidate this attachment feature and the five
predecessors through `4854ebca`: submodule discard/explicit-force receipt upgrade
with the same reason and identity/OID pins, caller writer-fence preflight (older
ambiguous artifacts remain preserved/manual), bare-idle false-positive removal,
independent handoff preservation, and reply-capability-aware decision guidance.
No Claude reply transport is claimed. Existing version entries, versions and tags
are untouched. This worker does not cut, publish, push or install anything.
