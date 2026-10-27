---
created: 2026-10-27
updated: 2026-10-27
type: bug
reporter: conductor
status: untriaged
priority: high
provenance: other
provenance_detail: 0.17.2 release gate on Trude
source_ref: release:0.17.2/tmux-tab
---

# tmux 3.4 (Ubuntu 24.04) prints tab separators in -F formats as '_': session and multiplexer parsing break on Linux

## Description

The 0.17.2 local release gate on Trude (Ubuntu 24.04 arm64, tmux 3.4) failed in session::tests (private_workmux_shell_closes_only_when_exact_and_idle, real_retention_archives_before_killing_only_owned_pane_and_keeps_active_worker, real_private_tmux_expiry_preserves_unrelated_split_and_is_idempotent): 'index out of bounds: the len is 1 but the index is 1' when splitting -P -F output on '\t'. Verified: `tmux -f /dev/null -S <sock> new-session -d -P -F $'#{window_id}\t#{pane_id}' 'sleep 5'` prints '@0_%0' on tmux 3.4 (tmux replaces non-printable characters in format output). Production code uses the same tab-separated formats (crates/taskfleet/src/session.rs ~402, 605, 759, 1034, 1089, 1202; run/spawn.rs; multiplexer/tmux.rs), so on Ubuntu hosts these parses silently fail or misparse — possibly behind the false 'supervisor died mid-run' stalls and the session-retention behaviour seen on Trude. Use a printable separator that cannot occur in the fields (or one format per field), parse robustly, and cover it with a test that runs against the installed tmux. Release 0.17.2 waits for this.
