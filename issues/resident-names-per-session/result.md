# Result: resident names scoped by creator session

Resident names are now scoped by (repository, creator session). The creator
session is the `created_by_session` attribution `run create` already records
from `TASKFLEET_CREATOR_SESSION`; nothing new is trusted.

- `run create --resident --name <n>` refuses `resident_name_in_use` only while
  a live run of the same repository **and** the same creator session has the
  name. Unattributed creators share one scope, as before among themselves.
- Name resolution (`run show`, `wait`, `message`, `land`, `cancel`, `merge`)
  searches the caller's own session first. One match resolves; several are
  `ambiguous_run_name`.
- An attributed caller with no run of that name in its session gets
  `resident_name_other_session`, listing the other sessions' full run ids in
  the message and in `expected`. It never reaches another conversation's
  worker by name.
- An unattributed caller (a terminal user) with no unattributed match keeps
  the repository-wide lookup: a unique match resolves, several are
  `ambiguous_run_name`, whose message lists every full run id with its
  creator session.
- `run list --creator-session <pi:uuid|current>` lists one conversation's runs.
- ADR 0003 amended; `--name` help, the core schema comment, CHANGELOG and the
  `worktree-spinoff` skill's "Named support workers" text updated.

Evidence: `tests/resident.rs::resident_names_are_scoped_by_creator_session`
(two attributed sessions each create and resolve `reviewer`; same-session
duplicate refused; a third session refused with both ids; unattributed caller
gets the ambiguity with both ids; `run message` shares the resolver; cancel by
name from one session leaves the other's run alone; an unattributed creator
forms its own scope; `run list --creator-session`).

## Decisions for later review

1. **An unattributed caller falls back to all sessions.** The issue asked for
   attributed callers to stop at their own session; for unattributed callers
   it kept today's repository-wide lookup. I kept that fallback, but searched
   the unattributed scope first, so a terminal user who created their own
   `reviewer` gets theirs rather than an ambiguity with a Pi conversation's.
2. **A distinct error code for "only other sessions have it".**
   `resident_name_other_session` instead of `run_not_found`, because the name
   resolver otherwise turns a not-found into the original `invalid_run_id`
   error and the caller would lose the run ids it needs.
3. **Invalid `TASKFLEET_CREATOR_SESSION` fails name resolution** with
   `invalid_creator_session`, like `run create` does, rather than silently
   resolving as unattributed. Id and prefix resolution never read it.
4. **`--creator-session current` without the variable** is
   `creator_session_unset`, not an empty list, so a misconfigured agent
   notices.
