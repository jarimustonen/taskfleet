---
created: 2026-11-08
updated: 2026-11-08
type: improvement
reporter: jari
status: untriaged
priority: normal
provenance: other
provenance_detail: open-habitat helpers-across-conversations
source_ref: taskfleet:01m6y4rex13ekh2vp7pyr61g0x/open-habitat:helpers-across-conversations
originating_run: 01m6y4rex13ekh2vp7pyr61g0x
originating_run_kind: spinoff
---

# Scope resident names by creator session, not only by repository

## Description

## Problem

Open Habitat shows each conversation's named resident workers ("helpers") by
the creator session Taskfleet records (`created_by_session`, set from
`TASKFLEET_CREATOR_SESSION=pi:<uuid>`). Resident names, however, are
repository-scoped: `ensure_name_free` (`crates/taskfleet/src/run/resident.rs`)
refuses a name while any live run in the repository has it, and
`resolve_name` resolves among all of the repository's live runs.

Observed 2026-11-08 on Open Habitat's test server: a conversation asked for a
helper named `testiavustaja`; `run create --resident --name testiavustaja` was
refused with `resident_name_in_use` because an earlier, since archived
conversation in the same repository still had a waiting helper of that name.
The agent then inspected the other conversation's run and renamed its own.
Two conversations working in one repository naturally pick the same job
names (`reviewer`, `proto-login`), and a waiting helper legitimately lives up
to 24 h (idle expiry) or longer when dormant, so the collision is common.

A second effect: `run message <name>` / `run land <name>` from conversation B
resolves to conversation A's helper when B has none of that name, so a name
can reach another session's worker by accident.

## Proposal

Scope resident names by (repository, creator session):

1. `ensure_name_free(root, name, repo, creator)`: refuse only when a live run
   in the repository has the name **and** the same `created_by_session`
   (both `None` counts as the same scope, so unattributed callers keep
   today's behaviour among themselves).
2. `resolve_name(root, name, cwd)`: read the caller's
   `TASKFLEET_CREATOR_SESSION` (same parser as `home::creator_session`).
   - Attributed caller: candidates are the repository's live runs with that
     name and the same creator session. Zero is `run_not_found` (message:
     "no live resident run named … created by this session in this
     repository; use the run id for another session's run").
   - Unattributed caller (a terminal user): candidates are all the
     repository's live runs with the name; several stays
     `ambiguous_run_name` with the ids in `expected`, as now.
3. Optional: `run list --creator-session <pi:uuid|current>` filter, so an
   agent lists its own residents without jq.
4. Update the `--name` help, ADR 0003, and the worktree-spinoff skill's
   "Names resolve only among this repository's live runs" sentence.

`created_by_session` stays display attribution, never authorization: this is
a namespace, not access control. Any same-user caller can still address any
run by id; only name lookup and the uniqueness check change.

## Acceptance

- Two attributed sessions in one repository each create a resident named
  `reviewer`; each session's `run message reviewer` / `run land reviewer` /
  `run wait reviewer` reaches its own run.
- An unattributed caller resolving `reviewer` gets `ambiguous_run_name`
  listing both ids.
- The same session creating a second live `reviewer` is still refused.

## Meanwhile

Open Habitat's shipped helper recipe tells the agent to take `<name>-2` on
`resident_name_in_use`, leave the other run alone, and address its helper by
run id.
