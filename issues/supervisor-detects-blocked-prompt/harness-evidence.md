# Harness investigation (read-only)

Installed on haapa: Homebase-managed Claude Code 2.1.280. Inspected the real
executable selected by `~/bin/claude`, without invoking a session or changing
configuration. Its embedded source exposes these paths:

- `vI` constructs native Notification hook JSON with `hook_event_name`,
  `message`, `title`, `notification_type` and common session/project fields;
  dispatch uses the notification type as matcher.
- SDK permission ask requests start a `permission_prompt` hook timer. The
  interactive dialog host separately defaults its cue type to `permission_prompt`
  (`U0` -> `Ome` -> `zP` -> `vI`). More specifically, dangerous-removal
  safety asks are marked bypass-immune before bypass-mode auto-allow, and
  interactive Bash selects `ZLe` (`permission_bash`). The dialog metadata table
  gives both generic `Rde` and Bash-specific `ZLe` `notification:Lh`; `Lh`
  supplies text without a type, so `U0` defaults it to `permission_prompt`.
  The dialog cue timer waits at least six seconds after mounting and for inactivity;
  `zP` dispatches the hook before the independent OS notification channel.
  Thus a terminal without OS notifications is not a reason to lose the hook.
- The idle-input controller emits `idle_prompt` with “Claude is waiting for
  your input”. It checks loading/dialog/delegated-work conditions itself;
  Taskfleet does not reconstruct them from pane content.
- Command hooks are subprocesses, with JSON stdin; the invocation can supply
  hooks via `--settings` and select its session via `--session-id`.
- PermissionRequest is earlier than an actual unanswered modal, so using it
  alone would incorrectly page when another hook auto-handles a request.
- Tool completion cannot clear permission visibility: Claude can execute tools
  concurrently. The implementation therefore clears permission only at a main
  turn boundary, and treats missing recovery evidence conservatively.

This supports the ordinary interactive permission/safety dialog and idle-input
route. It does not prove every possible startup/auth/trust or future modal emits
these events. Existing run launchers do not contain these hooks. Managed settings
or wrapper policy may disable them. Missing hooks/telemetry are unknown.

The automated fixture uses a disposable compatible worker, generated launcher
settings, real helper subprocess and real event reducer. No authenticated live
Claude run was performed; no API account or model was needed for the fixture.
The original reported run and household tmux were never accessed.
