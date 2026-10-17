---
created: 2026-10-17
updated: 2026-10-17
type: feature
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:oh-conductor-2026-10-17-worker-visibility
---

# Worker windows are invisible in the user's tmux: they live on Taskfleet…

## Description

Worker windows are invisible in the user's tmux: they live on Taskfleet's private tmux server

Observed 2026-10-17 on haapa with taskfleet 0.15.0 (Open Habitat conductor session). Jari could not find a running worker (run 01m54ze9q6kvqxaktkpzx08jj7, Homebase spinoff, `--interactive`) in his tmux: `tmux list-windows -a` on his normal server shows nothing. The window lives on Taskfleet's private server, `/run/user/1000/taskfleet/tmux-7c94f1c123bb2d377ca6.sock`, session `default`, window 2. Seeing it needs `tmux -S /run/user/1000/taskfleet/tmux-<id>.sock attach -t default:2`, which no one knows without reading the code. The same hid all overnight workers, including one stuck seven hours on a safety prompt (see supervisor-detects-blocked-prompt).

Expected: a human can find and watch any run's agent window with one obvious command, and interactive runs (which by definition a human may want to watch or merge) appear where the human's tmux/tw lists windows, or `taskfleet run show` prints the exact attach command (`data.attach` with socket, session and window) and `taskfleet run attach <run-id>` exists.

Suggestion: (1) `run show`/`run create` report the attach command; (2) `taskfleet run attach <id>`; (3) an option or config to place `--interactive` runs on the user's default tmux server (as before 0.15) while headless autonomous runs stay private.
