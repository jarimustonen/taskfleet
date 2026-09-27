---
name: worktree
description: Router for the `/worktree-*` family. Reads a free-form worktree request and hands it, arguments intact, to the sibling that owns that kind of work (`/worktree-spinoff`, `/worktree-research`, `/worktree-technical-decision`, `/worktree-bug-analysis`, `/fan-out`). Use when the user invokes `/worktree <task>`, `/worktree <issue-slug>`, or `/worktree --flag ... <task>`, or says "spawn a worktree for X" / "do this in a worktree" without naming a variant. Does not create worktrees itself, and does not route to `/worktree-merge`, `/worktree-status`, or `/complex-rebase`, which act on an existing worktree and are invoked directly.
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# worktree

Arguments: `$ARGUMENTS`

The user typed `/worktree` without saying which kind of worktree they want. Your
job is to work that out from their words, say in one line where you are sending
the request, and invoke that sibling skill with the arguments exactly as the user
wrote them. The sibling does everything else: it reads the issue, validates the
flags, writes the worker's brief, and creates the run. Nothing in this skill
creates a worktree, a run, a prompt file, or a branch.

## The family

Each sibling spawns one kind of autonomous run through `taskfleet run create`
and merges it back with `taskfleet run merge`. The request's shape tells you
which one it is.

- `/worktree-spinoff` — one focused piece of autonomous work: a coding task, a
  bug fix, a chore, or an issue slug on its own. This is the default when no
  other shape applies. A bug fix is a spinoff driven by the bug's issue slug;
  there is no separate bugfix variant.
- `/worktree-research` — an open inquiry that needs several sources read and
  synthesized into a written report: "research", "investigate", "survey",
  "compare the options for". Not a one-search factual lookup, not a summary of
  one document, not debugging, and not a request for a decision. The research
  skill refuses those itself, so sending them there wastes a round trip.
- `/worktree-technical-decision` — one architectural or technical call that
  ends in a recorded ADR: "decide whether X or Y", "settle the trade-off",
  "make the architectural call".
- `/worktree-bug-analysis` — understand an existing bug without fixing it, so
  the user can decide fix / defer / not-a-bug. It needs an already-filed bug
  slug and writes only to that issue. "Analyse", "understand", "figure out why"
  on a bug slug goes here; "fix" on the same slug is a spinoff.
- `/fan-out` — five or more identical, independent units over an enumerated
  set ("for every receipt in batch X", "apply this codemod to each package"),
  each writing a disjoint output. Fewer than five, or units that depend on each
  other or touch the same files, are spinoffs, not a fan-out.

Three more skills belong to the family but act on a worktree that already
exists, so there is nothing for you to route: `/worktree-merge` merges a
finished worktree back, `/worktree-status` summarizes one in plain language, and
`/complex-rebase` recovers a branch that has diverged too far for an ordinary
rebase. Each has preconditions you cannot check from here; if the request is one
of these, tell the user to invoke it directly and stop.

## Reading the request

Route on what the user is asking for, not on a word appearing in the text.
"Research whether the login bug is a race" is a bug to analyse or fix, not a
research report. A request that names two shapes, such as "compare the options
and decide", is genuinely two different runs producing two different artifacts,
and only the user knows which they want.

Decide from the user's words alone. Reading the issue or the repository would
not settle a routing question the words leave open, and the sibling is about to
read all of it anyway. If the words do not decide, ask one plain conversational
question and stop; the user answers in a second, whereas a run sent to the wrong
sibling costs a worktree, a worker's whole attempt, and a merge cycle before
anyone sees it was wrong. Ask in ordinary text, not a structured question
widget. When the answer comes, route the original request; the clarifying reply
is routing context, not the new task.

Do not quietly default to a spinoff because you are unsure. A spinoff will
write code and merge it into the user's source branch; if what they wanted was a
report or a decision, that is a wrong result, not a safe one.

## Handing over

Announce the choice in one line before invoking anything, for example
`Routing to /worktree-research — survey-of-options framing on an inquiry topic.`
That line is the user's chance to interrupt before a worktree exists.

Then invoke the sibling through the Skill tool with `$ARGUMENTS` unchanged:
the task text, any flags, in the order the user wrote them. Flags such as
`--headless`, `--tmux-session`, `--profile`, `--interactive`, or a leading
`--review` mean something specific to the sibling that receives them, and some
are not `taskfleet` flags at all but instructions the sibling folds into the
brief. Stripping, normalizing, or rephrasing here changes the brief the worker
will eventually receive, and rejecting an unknown flag is the sibling's job, with
the sibling's error message. Likewise do not expand or "helpfully" enrich the
task; the sibling owns the brief and knows what the worker needs.

Route to one sibling, once. If the Skill tool reports a failure, tell the user
what it said rather than trying another sibling: the failed attempt may already
have created a run or branch, and a second attempt for the same task can leave
two of them. Do not chain siblings ("research first, then fan out"); the user
invokes the second step when the first has landed.

## Requests that are not a new worktree

A question about worktrees, the family, or a sibling gets a direct answer, not
a run. Listing, pruning, removing, or hand-adding worktrees is `taskfleet run
list` or plain `git worktree`; point at the command. An empty `$ARGUMENTS` or
`--help` gets a short overview of the family and a request for a task.

## When nothing fits

If the request is real work that none of the siblings is shaped for, ask
whether it is a one-off or a workflow the user expects to repeat. A one-off is
a spinoff: the default worker handles arbitrary bounded tasks. A repeatable
workflow is a candidate for a new `/worktree-<x>` sibling; name the two closest
existing ones and why each falls short, and suggest authoring the new variant
as a separate step. Do not invent a sibling name that is not in the list above.

## Keeping this file current

This file is the family's registry. When a new `/worktree-<x>` skill is added,
give it one entry above describing the shape of request it owns, so that this
router can find it and the description of every neighbour still draws a clear
line against it.
