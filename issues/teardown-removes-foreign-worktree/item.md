---
created: 2026-11-13
updated: 2026-11-13
type: bug
status: untriaged
priority: normal
provenance: chat
source_ref: conductor:2026-11-13/open-habitat-gate-worktree
---

# Spinoff teardown removed a foreign worktree's registration (.git/worktrees deleted)

## Description

## Observed (13.11.2026, Trude, taskfleet 0.20.0)

In `~/src/open-habitat`, a conductor ran the product's clean gate in a
manually added, detached worktree (`git worktree add --detach
/var/tmp/agent-scratch/oh-gate-604b03d1 <sha>`). While it ran, a headless
spinoff in the same repository (run `01m7akhreacxskhfg58sfby70d`) landed and
was torn down at about 16:18 local time. At that moment `.git/config` was
rewritten (its workmux sections changed) and the whole `.git/worktrees/`
directory disappeared, including the foreign gate worktree's registration;
its checkout stayed on disk with a `.git` file pointing to the deleted
`.git/worktrees/oh-gate-604b03d1`. The gate then failed with "fatal: not a
git repository" and "the gate changed the shared repository config".

`git worktree prune` alone would not remove a registration whose checkout
exists, so something in teardown (Taskfleet or workmux) removed more than
its own worktree.

## Expected

Teardown removes only its own worktree registration and its own config
section; foreign worktrees and unrelated config stay untouched.
