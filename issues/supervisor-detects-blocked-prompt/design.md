# Bounded prompt visibility

## Choice

Extend explicit signal ingestion, not supervisor inference (ADR 0001 D1/A5).
New Claude launchers install invocation-local command hooks through `--settings`
and own a fresh `--session-id`. No global/user/project settings are written.
Claude `Notification` with `permission_prompt` or `idle_prompt` opens the existing
`node.awaiting_input` contract, even with no worker telemetry. The helper runs as
a harness subprocess, not an agent tool call. It returns no hook decision and
never approves, denies, interrupts, cancels, or authorizes teardown.

Native signals are facts about the harness notification, not guesses from CPU,
mtime, pane text, or git activity. Current installed Claude 2.1.280 source has a
delayed permission notification timer in its ask path, and idle notification
emission from the interactive input controller. OS notification delivery is
separate from Notification hook delivery. `PermissionRequest` alone is not used:
other hooks may resolve a request before a human modal exists. Claude print mode
may deny instead of prompting; a true worker exit continues to use existing rules.

## Identity and recovery

The generated hook command pins the executable, absolute state root, full run id,
node, absolute attempt and session UUID. Incoming session id must match; subagent
hooks are ignored. Reads and appends share the run's exclusive lock. Stale
attempts, terminal runs/nodes and exited workers cannot reopen input. Duplicate
notifications do not extend the grace clock or accumulate discussion items.
Progress hooks (`PreToolUse`, `PostToolUse`, `PostToolUseFailure`,
`UserPromptSubmit`, `Stop`) resolve only a solely hook-owned request, fenced to
its event sequence. Tool hooks clear idle requests only; permission requires a
turn boundary (`UserPromptSubmit`/`Stop`), since other tools may run concurrently
with a pending modal. Mixed/manual requests are preserved for explicit resolution.
This trades occasional stale attention for never silently clearing a human fork.
All projection writes remain in the existing event/reducer APIs.

Session/resume/settings selection in Claude argv conflicts with launcher ownership
and is rejected explicitly. Existing runs are not retrofitted. Managed policies
or wrappers that disable hooks, pre-session trust/auth modals, and unsupported
harnesses may provide no signal. Absence is unknown, never health. No observational
fallback is added. Check-ins provide the bounded parent inspection in those cases.

## Check-in

`run wait --check-in <positive-duration>` is a one-shot interval return. It emits
`outcome: check-in`, exit 4, while the requested wait condition remains unmet.
A met condition wins over deadlines; timeout wins an equal/earlier interval.
The existing any/all, terminal-only, fail-on-error and progress semantics remain.
Check-in adds cheap projection evidence (run age, node update timestamp, attempt,
telemetry availability, prompt signal presence/unknown). It explicitly reports
file-change/commit/pane evidence unavailable: no recursive filesystem traversal or
pane subprocess on a poll. Ordinary wait remains host-runtime neutral.

## Evidence boundary

Deterministic disposable fixtures exercise the actual generated hook settings and
helper, absent telemetry, duplicate/recovery/stale-attempt/session/manual coexistence
and bounded waits with a stripped PATH. Read-only inspection of the installed
Claude establishes source wiring, not a live authenticated Claude/modal test.
No original incident run or household tmux is touched. Live external-harness proof
is separate and must not be claimed from fixture tests.
