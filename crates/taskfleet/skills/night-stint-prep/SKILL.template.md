---
name: night-stint-prep
description: "Prepare an unattended overnight work session (yörupeama) from the live issuectl DAG and the issue texts: choose a bounded, useful wave, separate autonomous work from what waits for a human, and write and commit the TODO.md handoff the next `/stint-start` conductor will read. Use for 'prepare an overnight stint', 'suunnittele yörupeama', or bare `/night-stint-prep`. Not for running the round (`/stint-start`) or finishing a session (`/stint-handoff`)."
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# Prepare an overnight stint

The next conductor will work while the user is absent. Give it an agenda
grounded in the current repository, not a promise that every mechanically
spawnable DAG head will be done by morning. Preparation ends with a committed
handoff and a short briefing to the user; it does not launch workers, deliver
a design package, or deploy.

Read the repository's `AGENTS.md`, its session workflow, `TODO.md`, its issue
policy, and the current `issuectl dag --json` before planning. Read the actual
issue text of every plausible night unit, including `testing` heads: some
have code left to do, others wait only for a human verdict. Check
dependencies, collisions, and in-flight runs through the `taskfleet-overview`
skill before describing concurrency. A snapshot taken during preparation is
not a reservation for tonight; the next conductor rebuilds reservations and
the DAG at launch. Do not infer a worker's ownership from a global run list
or touch another session's uncommitted files. If the checkout cannot be
synced cleanly, state the limitation rather than inventing a current
frontier.

Choose a bounded, useful wave from the DAG: substantive implementation that
can be tested without the user, independent fixes or evidence preparation,
and a next slice only after its predecessor has landed. Distinguish a design
or architecture issue whose investigation can proceed from the authority
decision that cannot. Treat `untriaged` and unscheduled items as candidates,
not runnable work; if the user has explicitly accepted particular candidates
for scheduling, record that decision and either apply it through `issuectl`
or give the next conductor an unambiguous issuectl handoff. Do not silently
schedule the rest. Leave the exact lane order, blocker edges, collision
tokens, computed heads, and live holds in issuectl, not in TODO prose: they
change as workers land, and a prose copy is stale when written.

The user's attention is unavailable overnight. A worker can prepare a
recommendation, test with disposable synthetic identities, assemble a
redacted package for an outside party, document what to ask them, run the
repository's gate, and deploy to a disposable test server when the
repository grants that autonomy. Sending material to an outside party,
asking the user to perform a real browser or provider test, enrolling
another person, and choosing a major authority or security boundary are
morning activities. A worker should make progress around those boundaries
rather than claim a missing human check passed. The repository's deployment
policy, not the word "night", determines whether deployment is permitted; a
red or partially integrated pile does not become a test release. State which
evidence a morning test would add beyond synthetic results.

Default to one conductor with parallel worktree workers, even if the work
spans server and UI: there is one current DAG, one set of resource
reservations, and one integration and deployment decision. If two conductors
really have separable deliverables, define their issue ownership,
integration point, and communication channel before leaving. A jointly
edited `TODO.md` is a poor live mailbox; it is a handoff narrative, while
issue results and Taskfleet reports hold the work facts. Do not create a
second task tracker just for the night.

Write the repository's `TODO.md` handoff in its existing style. Name the
chosen issue slugs and their user-visible outcomes, distinguish
implementation from preparation and human-gated work, and say what the
morning report should contain: landed work, validation and installed
revision, packages ready but unsent, real-user checks not run, decisions
with a recommendation, and unresolved worker work. Preserve useful
historical context without claiming an old live version is still installed.
The new conductor reads the live DAG and adjusts if concurrent work changed
it; TODO is intent, not scheduling authority. Commit only your own handoff
and issue metadata, push under repository policy, and leave pre-existing
edits alone.

Before calling the plan ready, ask whether a fresh agent could tell what to
do first, what can follow if it lands, what waits for the user, and what
should not be started just because it appears at the DAG head. If not, the
handoff is still too generic. Brief the user on those choices and any
genuinely unresolved product decision; do not start `/stint-start` unless
asked.
