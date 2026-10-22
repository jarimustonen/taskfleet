---
created: 2026-10-17
updated: 2026-10-22
type: feature
reporter: jari
status: done
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:oh-conductor-2026-10-17-worker-visibility
lane: resources
lane_seq: 30
closed: 2026-10-22
closed_by: pi-worker
---

# Worker windows are invisible in the user's tmux: they live on Taskfleet…

## Description

Worker windows are invisible in the user's tmux: they live on Taskfleet's private tmux server

Observed 2026-10-17 on haapa with taskfleet 0.15.0 (Open Habitat conductor session). Jari could not find a running worker (run 01m54ze9q6kvqxaktkpzx08jj7, Homebase spinoff, `--interactive`) in his tmux: `tmux list-windows -a` on his normal server shows nothing. The window lives on Taskfleet's private server, `/run/user/1000/taskfleet/tmux-7c94f1c123bb2d377ca6.sock`, session `default`, window 2. Seeing it needs `tmux -S /run/user/1000/taskfleet/tmux-<id>.sock attach -t default:2`, which no one knows without reading the code. The same hid all overnight workers, including one stuck seven hours on a safety prompt (see supervisor-detects-blocked-prompt).

Expected: a human can find and watch any run's agent window with one obvious command, and interactive runs (which by definition a human may want to watch or merge) appear where the human's tmux/tw lists windows, or `taskfleet run show` prints the exact attach command (`data.attach` with socket, session and window) and `taskfleet run attach <run-id>` exists.

Suggestion: (1) `run show`/`run create` report the attach command; (2) `taskfleet run attach <id>`; (3) an option or config to place `--interactive` runs on the user's default tmux server (as before 0.15) while headless autonomous runs stay private.

<!-- intakectl:analysis:start job:8ae7ee88-4684-41e4-8c67-38e10c441207 generation:0 -->
## Triage analysis

**Finding:** This is a worker-discovery feature gap, not evidence that the worker failed to launch. The reported private-server placement matches the current Linux implementation. The installed binary reports version 0.15.0, and its `run --help` has no `attach` command. `reattach` restarts a supervisor; it does not open the worker display.

**Repository evidence:** `docs/LINUX-LIFETIME.md` explains that Linux workers live under a dedicated, state-home-specific tmux user service. `crates/taskfleet/src/run/spawn.rs::ensure_session` selects that manager-owned server, including foreground placement. Thus copying the caller's session name does not put the window on the caller's server, and ordinary `tmux list-windows -a` cannot discover it. The spawn path already captures socket, session, window and pane identity, and creation records them into `Node.tmux_identity`. `run/show.rs::ShowPayload` has no dedicated attach surface. The lifetime documentation gives a manual socket-qualified attachment recipe, but users must first locate the recorded identity.

**Impact and scope:** A user cannot readily observe or steer an interactive worker from their normal tmux window list. Autonomous workers are also harder to inspect when attention is needed. This analysis confirms the placement and command-surface gap from source and installed help; it did not attach to the cited historical worker or reproduce its seven-hour prompt wait. Prompt detection is a separate concern, not something an attach command alone resolves.

**Recommended implementation direction:** Expose a structured attachment descriptor in `run show` and `run create`, with a shell-quoted copyable command, then add `run attach <full-run-id>` using recorded identity rather than reconstructing socket names or matching window titles. Resolve node metadata under the shared run lock, verify the display still exists, and distinguish unavailable/cleaned displays from supervisor recovery. Specify behavior for invocation inside tmux and for runs without a unique worker display. Keep the dedicated Linux server as the default: moving interactive workers back onto the caller's server would undo the cgroup lifetime guarantee that lets them survive creator-service restarts. Any alternative placement option needs a separate lifetime design, not merely a visibility toggle.

**Verification targets:** Private-server and non-Linux placement; shell-sensitive socket/session values; invocation inside and outside tmux; missing or stale display identities; terminal runs with retained versus removed windows; and unchanged supervisor/cleanup ownership. Use isolated tmux stubs for regular tests and retain the real Linux lifetime smoke as evidence that discovery changes do not regress worker survival.
<!-- intakectl:analysis:end job:8ae7ee88-4684-41e4-8c67-38e10c441207 generation:0 -->

## Acceptance Criteria

- [x] `run show` and `run create` expose recorded exact display identity and a shell-quoted command; `run attach <full-run-id>` validates it before viewing.
- [x] JSON/JSONL attachment validates and returns a descriptor without terminal bytes; native attachment is read-only and refuses invocation inside tmux.
- [x] Missing, stale, incomplete, cleaned and non-unique displays fail explicitly without server creation, input injection or worker cleanup.
- [x] Independent viewer selection preserves other clients' current window, active pane and zoom; Linux placement/lifetime are unchanged.
- [x] Deterministic CLI regression tests, reviewed snapshots, focused independent review/assessment and opt-in private-socket native smoke cover the change; see `validation.md` and the final worker report for gate evidence.
