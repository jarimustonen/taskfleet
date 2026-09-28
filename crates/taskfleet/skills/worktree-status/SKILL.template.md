---
name: worktree-status
description: "Brief a non-technical decision-maker (product owner, manager, stakeholder) on where the current worktree session stands, in plain product language and from the conversation alone: what it set out to change, what is ready to try, what needs their call, what was deferred. Use for 'give me a status update on this worktree', 'where does this stand', 'brief the PO', or bare `/worktree-status`. Reads only the conversation, runs no git, touches no files. Not for PR descriptions, commit summaries, activity logs, or the developer-facing session save (`/wrap-up`)."
version: 1
cli_version: "{{CLI_VERSION}}"
schema_version: 1
---

# worktree-status

## Who this is for

The reader is a decision-maker who was not in the room: a product owner, a
manager, a stakeholder. They understand the product, its users, and the team.
They do not know git, branches, worktrees, skills, code review, or this
codebase, and they did not see this conversation. What they need is one chat
message that lets them do three things without a follow-up question:
understand what this session changed for the product and why, try the result
themselves, and give the answers only they can give. A snapshot that leaves
them asking "what does that mean?" has failed even if every fact in it is
right.

Reply inline in the chat and stop. The skill changes nothing: it writes no
file, opens no issue, and does not decide whether the work lands (that is
`/worktree-merge`). The developer-facing record of a session for future agents
is `/wrap-up`, not this.

## Where the facts come from

The conversation you are in is the whole source: the operator's requests, the
work done, the decisions surfaced, the questions left open. When a conductor
such as `/stint-start` has surfaced round results into the conversation (which
units landed, which issues closed, what worker worktrees reported back:
bug-analysis findings, ready-to-test facts, follow-up recommendations), use
them. For a delegated round they are usually where "Ready to test" and
"Spin-offs" come from, because the work happened out of sight in other
worktrees.

Do not run git, read repository files, or gather new sources. The snapshot
reports what this session knows and has checked, and the reader relies on it
being exactly that. A fresh look at the repository would mix in things nobody
in the session examined, presented with the same confidence as the session's
own findings. Where the conversation does not settle something, say it is
unknown rather than finding out.

Arguments: `$ARGUMENTS`, a focus hint if the caller gave one.

## Shape of the message

The snapshot has a fixed shape, and other skills produce the same one:
`/stint-start`'s report phase mirrors these sections, so readers learn to scan
them. Use these headings, in this order, and omit any section with nothing in
it; a heading over "(none)" wastes the reader's glance. A session that shipped
something usually has a Summary and Ready to test; a research or planning
session may be the Summary alone, and that is a complete snapshot.

```markdown
# <product-language title: what is being built or changed, never a branch name or slug>

## Summary
## Ready to test
### <one changed thing, product-language title>
## Decisions needed
### <product-language title>
## Discussion points
### <product-language title>
## Spin-offs
### <product-language title>
```

Each item is a paragraph of plain prose, roughly 100–200 words: long enough to
carry the why, short enough that a busy reader finishes it. Testing steps may
be a short numbered list instead.

**Summary.** What the session set out to do in product terms and where it
stands now. Lead with the why, framed as what changes for the user or the
product when this lands.

**Ready to test.** This is the section that makes the snapshot something the
reader can act on, so include it whenever concrete, user-observable behaviour
changed, with one subsection per changed thing. Write for a colleague who
already knows the product: what now works that did not before, where to look,
and what they should see if it is right. Skip navigation they could do in
their sleep; walk through a step only when it is genuinely new. Say what only
they can judge: data only they have, a device or account only they can reach,
a "does this feel right?" call. If something was built but deliberately left
unverified, say so and why, so they do not take it as tested. Omit the section
when nothing testable landed.

**Decisions needed.** Questions the work cannot proceed past without a call
from this reader: what is open, why a non-technical reader has to weigh in,
what the realistic options are. Leave out questions the team can answer itself
or that are purely mechanical. Each item here spends the reader's attention,
and a list padded with non-decisions teaches them to skim it.

**Discussion points.** Things that surfaced but were not resolved and need no
call now: open tensions, observations, things to revisit. Say what came up and
why it matters. The line between this and Decisions needed is whether the
reader has to act.

**Spin-offs.** Work deliberately split out: follow-up tracks, "we should also
do X" items, deferred pieces. For each, what was deferred, why not now, and
what the next step is.

## Language

Everything is in the reader's vocabulary. Words that mean nothing to them or
send them to a developer: worktree, branch, commit, merge, PR, rebase, skill,
slash-commands, file paths, commit hashes, branch names. Each has a plain
translation: "we did the work in parallel", not "we spun up a worktree"; "the
change is in", not "merged"; "follow-up work", not a run or a worktree. Before
sending, read the message as the reader would: would a non-technical person
know what is happening and why it matters? If a section fails that test,
rewrite it rather than adding a gloss.
