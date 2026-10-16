# Read-only worker transcript discovery

`taskfleet run show <run-id> --json` exposes `data.node_transcripts`.
`taskfleet run list --json` exposes the identical view at
`data.runs[].node_transcripts`, including terminal/history runs. It contains
all recorded nodes, sorted by full `node_id`, not just `n-0001`. A run with
no nodes has `[]`. List retains a run's canonical row and emits an envelope
warning if its node projections cannot be read; its transcript array is then
empty, **not proof of no workers**. Show fails on unreadable node projections.

This is an additive read projection of existing node evidence. The older
single-worker `show` fields (`evidence`, `caller_pi_session`, etc.) are unchanged.
There is no new registry, archive, event, or retention policy. See
[Terminal run history](TERMINAL-RUN-HISTORY.md) for retained reports and the
boundary between display expiration and durable history.

## Row contract

Each row has:

- `node_id`: durable node identity within the enclosing full `run_id`.
- `harness`: `pi` for recorded Pi worker evidence; otherwise the recorded
  selected harness, falling back to the legacy manifest harness; `null` if
  unknown. No current config or executable discovery is consulted.
- `attempt`, `session_id`: exact evidence attempt and native session ID, or
  `null` when not recorded. An attempt can be zero. No ID is guessed from a
  filename, branch, timestamp, caller session, or another node.
- `original_cwd`: native original header cwd recorded in evidence, or `null`.
  It survives teardown. `worktree_path` is the node's recorded checkout path,
  not a promise that the directory still exists.
- `support`: `supported` for Pi transcript discovery, `unsupported` for other
  known harnesses (including Claude), `unknown` when the harness is unrecorded.
  Support does not imply that this particular node has a transcript.
- `capture_status`: existing evidence state `pending`, `failed`, or `complete`;
  `null` without worker evidence. A failed capture can still have a live file;
  a complete capture can have a missing archive (for example external deletion).
- `live`, `original`: objects with `anchor`, `path`, and `availability` below.

`live.anchor` is always `state-root`: resolve its relative path against the
selected Taskfleet home (`TASKFLEET_HOME`, normally `~/.taskfleet`).
`original.anchor` is always `run-root`: resolve against
`<state-root>/runs/<full-run-id>/`. These are the same conventions as the raw
`WorkerEvidence.live_session_path` and `WorkerEvidence.transcript_path`,
respectively. Never resolve either against cwd, the checkout, or Pi's ordinary
session store. Example paths:

```text
live:     .creating/pi-sessions/<run-id>/pi-session-<uuid>.jsonl
original: evidence/n-0001/pi-session.original.jsonl
```

`availability` is:

| Value | Meaning | `path` |
| --- | --- | --- |
| `available` | Recorded target currently observes as a regular file | relative string |
| `missing` | Recorded target does not exist | `null` |
| `unavailable` | Target cannot be inspected, is not a file, or recorded path is not relative/contained lexically | `null` |
| `not-recorded` | No path recorded (legacy node, no session yet, or archive not yet committed) | `null` |
| `unsupported` | No transcript discovery for this known non-Pi harness | `null` |

The original archive is byte-identical to the captured live transcript. This
surface **never substitutes the resume copy**, whose header cwd is rewritten.
The complete state records durable capture, not successful availability now.

## Identity, transitions, and trust

Use `(run_id, node_id, attempt, session_id)` to identify a worker transcript.
Node/session identity is stable from live capture to archive and after worktree
removal. Retries can replace the current node evidence with a new attempt and
session ID; this view is the current recorded attempt, not an attempt-history
index. It does not invent bindings for cold legacy runs without evidence.
Caller-owned Pi bindings remain on the separate caller surfaces, not here.

Manifest, nodes and evidence are read under the same per-run shared lock.
Archive capture writes files before committing its evidence event; the archive
path is not advertised until recorded. The live file is removed *after* that
commit, so both paths may briefly be available. File availability is a
point-in-time hint, not a lease: worker writes and live-source removal do not
hold the read lock, and either file can disappear before the consumer opens it.
Prefer an available original once recorded; on a vanished live read, refresh
the view and try the original. Never infer run completion from file availability.

These paths grant **no continuation, control, settlement, or writer authority**.
A read-only consumer must anchor and contain its own open (including symlink
handling), validate the native session header/type and exact session ID, and
check the live header cwd against the recorded checkout. For the archive,
validate against `original_cwd`, even if the checkout no longer exists. The
same-user state and filesystem can be modified externally; an advertised path
alone is not an authenticated session. No browser/controller operations are
added by this contract.
