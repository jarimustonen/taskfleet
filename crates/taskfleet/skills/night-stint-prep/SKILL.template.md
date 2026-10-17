---
name: night-stint-prep
description: "Prepare an unattended overnight work session (yörupeama) from accepted repository work (the live issuectl DAG when enabled): choose a bounded, useful wave, separate autonomous work from what waits for a human, and write and commit the TODO.md handoff the next `/stint-start` conductor will read. Use for 'prepare an overnight stint', 'suunnittele yörupeama', or bare `/night-stint-prep`. Not for running the round (`/stint-start`) or finishing a session (`/stint-handoff`)."
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# Prepare an overnight stint

The user is about to leave, and a `/stint-start` conductor will work through
the night from what you leave behind. Your product is a committed `TODO.md`
handoff and a short briefing. You do not launch workers, write a design
package, or deploy; the conductor does that, from the live repository, in the
morning's absence.

## Repository tracker (optional)

Use issuectl only when **both** repository instructions or an issuectl schema
establish that this repository uses it **and** its CLI is available. A directory named `issues/` alone may belong to another tracker;
check the repository policy before treating it as issuectl storage. This probe
checks the conventional schema-backed case from the repository root:

```sh
# issuectl-capability-probe
if [ ! -f issues/.schema.yaml ]; then
  printf 'tracker-not-used\n'
elif command -v issuectl >/dev/null 2>&1; then
  printf 'issuectl-ready\n'
else
  printf 'issuectl-cli-missing\n'
fi
```

For a documented issuectl tree without that schema path, verify configuration
by reading repository instructions or tracker files, not by running issuectl
against an unidentified `issues/` tree. Check CLI availability separately.
Do not install or initialize a tracker. Without both capabilities, use the free-text task and committed
repository artifacts; skip issue lookup, mutation, closure, trailers, and DAG
commands. If an explicit issue-driven request requires unavailable tracker data
or a required mutation, report that prerequisite and stop before spawning;
do not silently reinterpret a slug or hand-edit tracker metadata. A CLI command
failure in an enabled tracker is an error, not permission to switch modes.
`issuectl-cli-missing` is a disclosed missing capability, not proof that the
repository is tracker-free. If repository policy requires tracker scheduling or
writes for this task/stint, stop; only policy-permitted free-text work may use
the no-issuectl path. Record follow-up findings in the native report's
`spinoff_proposals` or `discussion_items` instead of creating tracker files.
State this capability result as a fact in every worker brief; it does not relax
the generated policy on how an issue, if filed, must be created.

## Runtime capabilities

Read sibling instructions through this harness's skill loader or file-reading
facility; no particular invocation tool is required. Bundled text is also
available with `taskfleet skill print <name>`. External review, Git-helper,
and session-saving skills mentioned here are optional unless the user or
repository mandates them. If absent, use ordinary Git, evidence-based self-review,
and repository-local handoff notes instead. A missing **required** review or
capability blocks completion and is disclosed, never claimed as run. Do not
install tools, change another user's configuration, or publish artifacts merely
to satisfy a skill reference.

## Without issuectl: prepare accepted tasks, not a private schedule

If the optional repository tracker is not enabled, use this path instead of
the tracker-backed preparation below. Read repository instructions, the current
handoff and the user's accepted scope. Choose a bounded next task that can be
completed without human decisions; name its goal, done criteria, exact gate,
and what must wait for the user. Describe follow-on intent only as conditional
on the first task landing, not as a new graph or tracker. The next conductor
works serially from this accepted scope and Taskfleet's live reports, checking
for overlapping runs before spawning. No lane, issue slug, reservation file,
or private DAG is required or created.

Write and commit the repository's handoff file (default `TODO.md`), preserving
useful history and other sessions' edits. Distinguish implementation, evidence
preparation, and human-gated decisions. Include validation/deployment limits,
unverified claims, known unresolved run ids, and the morning report expected.
No workers are launched here. Push only under repository policy. Night work
grants no extra decision, publishing, credential, or deployment authority.
Brief the user on the chosen scope and remaining human decisions.

## With issuectl: tracker-backed preparation

The remainder applies only with the optional repository tracker enabled. Read
its actual issue texts and live DAG; do not replace a failed tracker with prose.

## What the conductor can and cannot read

The conductor has the repository, the issues, and the live `issuectl dag
--json --reservations …` output. It does not have this conversation. So the
handoff carries intent and judgment, and the things that change by themselves
stay where they are computed: lane order, blocker edges, collision tokens,
computed heads, and which runs currently hold what. A prose copy of those is
stale the moment it is written, and the `stint-start` skill tells the
conductor to trust the DAG over the narrative. The same holds for your own
reading of in-flight runs: a snapshot taken now is not a reservation for
tonight, and the conductor rebuilds reservations at launch.

Read the repository's `AGENTS.md`, its session workflow, the current
`TODO.md`, and the issue policy before planning, since each repository decides
for itself what a worker may deploy and what the handoff block looks like.
Then read the actual text of every issue that might go into the wave,
`testing` heads included. The frontmatter says an issue is at the head of a
lane; only the body says whether it still has code to write or waits on a
human verdict. If the checkout cannot be synced cleanly, say so in the
handoff instead of describing a frontier you could not verify, and leave
another session's uncommitted files alone.

## Choosing the wave

A good night wave is bounded and useful: implementation that can be tested
without the user, independent fixes, evidence preparation, and a next slice
only after its predecessor has landed. The question for every candidate is
what a worker can finish alone. A design or architecture issue often splits:
the investigation can proceed, the authority decision cannot, and the handoff
should say which half tonight gets.

`untriaged` and unscheduled issues are candidates, not runnable work. If the
user has accepted particular ones for scheduling, apply that through
`issuectl` or hand the conductor an unambiguous `issuectl` instruction, and
record that it was the user's decision. The rest stay unscheduled; the
conductor must not learn from your handoff that something is approved when
nobody approved it.

What waits for morning is anything that needs the user's presence or
authority: sending material to an outside party, a real browser or provider
test, enrolling another person, a major authority or security boundary. A
worker can get right up to those edges by preparing the recommendation,
testing with disposable synthetic identities, assembling a redacted package,
writing down what to ask, running the gate, and deploying to a disposable
test server where the repository grants that. It should not claim a missing
human check passed, and the word "night" does not relax the repository's
deployment policy: a red or half-integrated pile is not a test release. Say
what evidence a morning test would add beyond the synthetic results.

Plan for one conductor with parallel worktree workers even when the work
spans server and UI. There is one DAG, one set of reservations, and one
integration and deployment decision, and two conductors sharing them need
their issue ownership, integration point, and channel defined before anyone
leaves. `TODO.md` is a poor live mailbox between them; it is a handoff
narrative, while issue results and Taskfleet reports hold the facts. Do not
invent a second tracker for the night.

## Writing the handoff

Write in the repository's existing `TODO.md` style. Name the chosen slugs and
what the user will be able to see when each lands, distinguish implementation
from preparation and from human-gated work, and say what the morning report
should contain: landed work, validation and the installed revision, packages
ready but unsent, real-user checks not run, decisions with a recommendation,
and unresolved worker work. Keep the useful history, but do not let an old
narrative claim a version is still installed when it is not. Commit only your
own handoff and issue metadata, push under the repository's policy, and leave
pre-existing edits alone.

Before calling it ready, read it as the fresh conductor would: is it clear
what to do first, what can follow if that lands, what waits for the user, and
what should not be started merely because it sits at a DAG head? If not, the
handoff is still too generic. Then brief the user on the choices and any
genuinely unresolved product decision. Starting `/stint-start` is their call.
