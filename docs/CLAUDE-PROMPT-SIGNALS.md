# Claude prompt visibility

New Taskfleet Claude workers receive invocation-local `--settings` command hooks
and a fresh `--session-id`. Taskfleet writes no Claude user/project configuration.
Those settings/session flags (including resume/continue) are reserved in candidate
argv: conflicts fail explicitly instead of overriding a user's policy. Other
profile argv, including permission policy, remain user-owned.

## Signals and recovery

Claude `Notification` hooks with `notification_type: permission_prompt` or
`idle_prompt` open the existing durable `node.awaiting_input` request. The hook
is a subprocess of the harness: the agent need not be able to issue a tool call
while its modal is open. It emits no permission decision, never answers the
prompt and never changes status or authorizes teardown. No CPU, worktree mtime,
pane text, or git-idleness inference is performed.

`awaiting_input_detail.discussion_items[]` identifies the source as
`claude-notification-v1`, the notification type, session UUID and attempt. Raw
shell commands, messages and pane text are not copied into notification reports.
Duplicate notifications do not reset the grace clock or multiply questions.
The existing 180-second propagation grace applies from receipt: after it,
ordinary `run wait` settles and an enabled supervisor `--notify` hook pages once
per generation. `--terminal` still waits strictly for terminal status.

`PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `UserPromptSubmit` and `Stop`
clear a solely hook-owned idle request, using its exact event sequence under the
run lock. Permission requests clear only on `UserPromptSubmit` or `Stop`:
a different parallel tool finishing cannot prove its sibling's modal closed.
After approval, attention may remain until that turn ends. A manual or mixed request is never automatically cleared. Stale attempts,
foreign sessions, subagent hooks, terminal runs and exited workers are ignored.
Progress does not itself prove completion. If resolution evidence is missed,
attention can remain stale; inspect the worker and explicitly resolve the recorded
generation rather than answering a dangerous prompt blindly. To keep observing a
known open generation without repeatedly returning attention, use
`run wait --terminal --check-in 30m`; this does not answer or resolve it.

The bounded hook reader accepts up to 16 MiB (progress events contain tool
inputs/results); oversize input is ignored without a blocking decision. Input
contents are never used as executable commands or copied into attention details.

## Coverage and unknowns

The supported route is Claude's command-hook protocol, not rendered modal text.
Permission notification may be delayed by Claude before Taskfleet's own grace.
An interactive safety check routed through the ordinary permission ask UI uses
that notification path even when ordinary worker telemetry is absent. Idle input
after denial uses `idle_prompt`. In print mode Claude may instead deny/exit;
the existing told worker-exit rules still apply.

Existing running launchers are not changed. Wrappers/managed settings that disable
hooks, authentication or trust prompts before session initialization, unsupported
harnesses and new modal classes without these native notifications may supply no
signal. Existing `elicitation_dialog`, `elicitation_url_dialog` and remote-worker
notification classes are outside this permission/idle matcher. In particular,
`CLAUDE_CODE_DISABLE_PERMISSION_PROMPT_NOTIFY_HOOKS` can suppress native permission
notification delivery; Taskfleet does not override that user-owned switch.
No notification means **unknown**, not “healthy” or “not blocked”. Hooks
cannot fix those cases retroactively. Check-in is the bounded inspection path;
it is not a substitute claim that every possible modal is detected.

## Parent check-ins

```sh
taskfleet run wait "$run_id" --check-in 30m --output json
```

This is a one-shot interval return while the requested condition remains unmet:
`data.outcome: check-in`, exit **4**, never completion success or timeout. Inspect
and reinvoke to continue waiting. `--any`/`--all`, progress, terminal-only and
fail-on-error retain their meanings. A met condition wins over deadlines; timeout
wins an equal/earlier deadline. Existing exits are 0 condition met, 1 usage/unknown
run, 2 timeout, 3 settled failure under `--fail-on-error`.

With `--check-in`, each final `data.runs[]` includes `progress_evidence`: run age,
node projection timestamp/attempt, advisory telemetry availability and reported
input/unknown signal status. These are observations at summary time, not an atomic
snapshot with earlier settlement. Projection updates are not worktree changes.
File change, last commit and pane observations explicitly say unavailable rather
than traversing the worktree on every poll. This works in browser/headless parents
without tmux or a particular host background-process extension.

## Validation boundary

The disposable Claude-compatible harness fixture captures the real generated
settings, invokes the hook subprocess with native JSON and tests detection with
absent telemetry, wrong sessions, stale attempts, duplicate notifications, recovery,
manual coexistence and terminal fencing with an empty PATH. Parser and wait tests
cover malformed/bounded input, interval timing and precedence. Existing awaiting-input
tests cover durable grace/notify deduplication.

Read-only source inspection of installed Claude **2.1.280** found its
`permission_prompt` timer in the permission ask path, `idle_prompt` emission from
the interactive input controller, and hook dispatch separate from OS notification
delivery. This is source evidence plus fixture proof, **not a live authenticated
Claude safety-modal test**. Neither the original incident run nor household
sessions were used. Protocol reference: <https://code.claude.com/docs/en/hooks>.
