# Creator attribution

For ordinary Taskfleet-owned runs, Taskfleet records two independent, immutable
display associations in `run.created`
and the manifest, exposed beside each other by `run show --json` (including its
`manifest`) and each `run list --json` row:

- `created_by_run_id`: the exact Taskfleet run owning the creating worktree,
  resolved from durable run/node records, not a branch-name fragment.
- `created_by_session`: an optional object such as
  `{"harness":"pi","session_id":"12345678-1234-4234-8234-123456789abc"}`.
  A host launching a browser conversation can export
  `TASKFLEET_CREATOR_SESSION=pi:12345678-1234-4234-8234-123456789abc`;
  the agent does not need to pass a flag.

The environment input is create-only, not a configuration default. The separate
Git-only `--agent-owner caller` foundation does not consume this input or launch
workers; this contract does not change that path. When absent,
`created_by_session` is `null`. Older manifests and events missing the field
also decode as `null`; there is no historical inference or backfill.

For a new create, only the exact harness prefix `pi:` and a 36-byte lowercase
hexadecimal UUID spelling (`8-4-4-4-12`, with hyphens) are accepted. UUID version
bits are not restricted. Empty, whitespace, uppercase, alternate UUID spellings,
unknown harnesses, extra suffixes and non-UTF-8 input fail with
`invalid_creator_session` before publishing a run. Values are never trimmed or
normalized. Unset the variable to create without session attribution. This also
applies to a new dry-run; unrelated read commands do not validate this input.

A successful keyed replay returns the original run without re-reading either
creator association: a changed, absent or malformed session variable cannot
replace the original value (including original absence). Recovery from the event
log uses the same reducer and existing exclusive run lock. Reapplying creation
against an existing manifest does not rewrite attribution.

Generated outer and inner worker launchers unconditionally unset the variable,
for both interactive and autonomous Pi/Claude workers and retries. The autonomous
worker shim also removes it from its child environment. This works even when an
existing tmux server supplies its own stale variable. On Linux, manager-owned
services exclude it from forwarded values and use systemd's `UnsetEnvironment`
property to remove a value retained by the user manager. No shared tmux or
systemd-manager environment is modified. A nested worker's new run is therefore
attributed to the precise `created_by_run_id`, not its ancestor's browser session.

Neither association grants authority, proves identity, transfers ownership, or
joins the fan-out DAG. `parent_run_id` / `parent_node_id` alone retain their
existing child-supervision meaning. A same-user process can explicitly supply a
misleading session claim; the only supported use is grouping display rows. There
is no host registry, browser control endpoint, or session-based access check.
