---
created: 2026-10-27
updated: 2026-10-27
type: bug
reporter: conductor
status: fixed
priority: high
provenance: other
provenance_detail: 0.17.2 release gate on Trude
source_ref: release:0.17.2/tmux-tab
closed: 2026-10-27
closed_by: agent:taskfleet-run-01m5ybxbbwg76p93t39a3k8cb1
commits:
- hash: 235cc13b
  summary: 'fix: force tmux UTF-8 client mode so -F formats parse on Linux'
---

# tmux 3.4 (Ubuntu 24.04) prints tab separators in -F formats as '_': session and multiplexer parsing break on Linux

## Description

The 0.17.2 local release gate on Trude (Ubuntu 24.04 arm64, tmux 3.4) failed in session::tests (private_workmux_shell_closes_only_when_exact_and_idle, real_retention_archives_before_killing_only_owned_pane_and_keeps_active_worker, real_private_tmux_expiry_preserves_unrelated_split_and_is_idempotent): 'index out of bounds: the len is 1 but the index is 1' when splitting -P -F output on '\t'. Verified: `tmux -f /dev/null -S <sock> new-session -d -P -F $'#{window_id}\t#{pane_id}' 'sleep 5'` prints '@0_%0' on tmux 3.4 (tmux replaces non-printable characters in format output). Production code uses the same tab-separated formats (crates/taskfleet/src/session.rs ~402, 605, 759, 1034, 1089, 1202; run/spawn.rs; multiplexer/tmux.rs), so on Ubuntu hosts these parses silently fail or misparse — possibly behind the false 'supervisor died mid-run' stalls and the session-retention behaviour seen on Trude. Use a printable separator that cannot occur in the fields (or one format per field), parse robustly, and cover it with a test that runs against the installed tmux. Release 0.17.2 waits for this.

## Resolution

### 2026-10-27T10:15:33Z · @agent:taskfleet-run-01m5ybxbbwg76p93t39a3k8cb1

Root cause is the tmux client's locale, not tmux 3.4 itself. tmux treats a client as UTF-8 only when $TMUX is set or the first of LC_ALL/LC_CTYPE/LANG names a UTF-8 locale. Any other client gets sanitized output: control bytes including the -F tab become '_', and non-ASCII (the emoji window-name prefix) becomes underscores. Reproduced on Trude: `env -i tmux list-windows -F '#{window_id}\t#{pane_id}'` prints `@0_%0`; adding LANG=C.UTF-8, TMUX=..., or `-u` prints the tab. The gate failed because its environment had neither TMUX nor a UTF-8 locale.

Fix: every taskfleet tmux client now passes `-u` (multiplexer::tmux::command / UTF8_CLIENT_FLAG): the Tmux backend, session retention/maintenance (tmux_exec_for), evidence capture, pipe-pane capture, the watchdog probe, run attach, the spawn path, the retry kill-window, the private-server bootstrap, and the workmux PATH wrapper. Tab separators stay as they are: with UTF-8 mode they are printed raw, and `-u` is needed anyway because no separator choice can keep emoji window names intact under sanitization. New test multiplexer::tmux::tests::real_tmux_formats_parse_under_a_non_utf8_detached_client drives the production parsers through the installed tmux under LC_ALL=C without $TMUX; without the flag it fails (list_session_windows returns no windows). Full gate passed on fb48da8e on Trude with TMUX, LANG, LC_* and the creator-session variables unset.

Linux-only symptoms:
- Session retention: yes, explained. Under sanitized output list_session_windows sees no tab and returns an empty window list, which cleanup_managed_session treats as "retain", so empty managed headless sessions lingered. Retention scans (session.rs) could not parse list-panes/list-sessions rows either and preserved displays as unsafe.
- False 'supervisor died mid-run' stalls in run wait (profoundly-dangerous-father): not explained by this bug. run wait's supervisor liveness is a /proc pid + start-identity probe with no tmux involvement. The incident run 01m5y18wr663zmxacfkwedv8a3 shows `supervisor.exited reason=stale-on-reattach` for pid 711717 while that supervisor was alive (it later exited work-complete itself), i.e. the pid start-identity check misjudged a live supervisor — the same family as tmux-server-identity-clock-drift / supervisor-false-pid-recycled, and worth checking whether that run's pid record predates the 0.17.2 start-identity fix. Separately, the legacy (name-only) watchdog path could misread emoji window names as absent under sanitization; that is fixed here too, but it affects worker reaping, not the supervisor-died verdict.
