# Transcript discovery validation

## Contract and scope

The additive `node_transcripts` array is shared by `run show` and `run list`.
It covers every node, including terminal runs, without changing the existing
single-node raw evidence/caller fields. `docs/RUN-TRANSCRIPTS.md` defines all
fields, path anchors, availability values, read-only trust boundary and retry
limitations. No schema, event, archive, cleanup policy or control operation was
introduced. Only the existing evidence test was extended in supervisor code.

## Deterministic evidence

`tests/run_transcripts.rs` seeds cold runs using the production event/reducer
API, with no harness, tmux, supervisor or user manager:

- Zero-node and two-node runs; stable sorted IDs; no discovery from native Pi
  history. Legacy missing evidence and harness remain explicitly unknown or
  not-recorded. Claude is explicitly unsupported.
- The complete pending DTO shape is pinned, including null paths, exact
  session ID, attempt zero, cwd, and both path anchors.
- Missing live file, failed capture, retry to a new session/attempt, ignored
  stale prior-attempt archival, complete-but-missing archive and a non-file
  archive target.
- Both list and show execute concurrently with an exclusive archive/status
  transaction. Their resulting manifest status and per-node evidence come
  from the complete generation; another node keeps its own session identity.
- An unreadable sibling projection produces a list warning without removing
  the canonical history row or default-node worktree diagnostics. Show retains
  its existing strict read failure behavior.

`supervise::evidence::tests::archives_exact_transcript_and_header_only_resume_copy`
executes the real filesystem archive path with a declared tmux stub, then
removes the checkout directory. Discovery reads the live bytes before archive
and original bytes afterward; node/session/attempt/cwd remain identical, the
live source is missing, and the archive is byte-identical. The test separately
checks that the resume header is rewritten, never advertised as the original.

The existing envelope snapshots were regenerated and individually reviewed:
only the new additive discovery field is intended. The structural DTO wire
pin is also updated, with no schema version bump or dropped fields.

## Validation commands

All builds/tests inherit disk-backed
`TMPDIR=/var/tmp/taskfleet-transcript-build`.

Focused release validation:

```sh
cargo test --release -p taskfleet --test run
cargo test --release -p taskfleet --test envelope_snapshots --test run_transcripts --test linux_lifetime --lib
```

The transcript and Linux user-manager fixture executables are also run with a
stripped PATH containing only declared Git (fixture shell/Python dependencies
use their explicit absolute paths). No ambient tmux or harness is available.

Final required gate, run on a clean implementation commit:

```sh
scripts/validate-local-release.sh
```

The worker's terminal report records the gate commit/result and local log path.
The conductor owns the integrated-main gate and release; this worker neither
installs Taskfleet/skills nor tags/publishes.

## Source-grounded self-review

Checked the evidence schema and reducers: live paths are state-root-relative,
original/resume paths run-root-relative, and stale attempts cannot rewrite
current evidence. Checked the archive ordering: artifact writes precede the
locked archive event, live deletion follows it. The reader holds the existing
shared lock for manifest/node/evidence composition; file availability remains
an explicitly documented advisory observation, not a lifecycle or identity
heuristic. There is no new writer/security/lifecycle mechanism requiring an
independent reviewer.

Limitations are deliberate: current attempt only, Pi evidence only, no
caller-session substitution, no file-open lease, no consumer header validation
inside the CLI, and no transcript retention changes. The browser must validate
its own anchored opens and headers, and gets no continuation/control authority.
The separate terminal-run-history contract remains conductor-owned follow-up.
