# Agents policy (`.issuectl/AGENTS.md`)

This is the repo-local policy for the issue tracker under `issues/`.
issuectl wrote the first version and still owns the block between the
`issuectl-managed` sentinels at the bottom: `issuectl doctor --fix`
regenerates that block from `issues/.schema.yaml` and leaves the prose
alone. `issuectl agents init --force` is not a refresh; it replaces the
prose with the stock template.

Three sources already cover what an agent needs most of the time. The
`/issue` skill (`.claude/skills/issue/SKILL.md`) carries the command
workflows and the JSON contract. `issuectl <command> --help` is current
for any flag and is where the lane-design and intake-flow guidance is
summarized. The top-level `AGENTS.md` holds the repository-wide rules:
issues are flat with status in frontmatter, every planning document
lives under its issue directory, an issue that only points at a
`history/` file is unreadable to anyone else, and automated review
residuals do not become issues without an observed occurrence. This
file adds what an agent touching the tracker here would otherwise learn
the hard way.

## Why every mutation goes through issuectl

Several sessions write to `issues/` at once in this repository: worktree
workers, the orchestrator, and intake filings from review skills. The
mutation commands (`update`, `set`, `apply`, `note`, `check`, `close`,
`label`, `depend`, `intake …`) take the repository write lock, validate
against the schema, and return a version token that a later write can
pass as `--expected-version` so it fails instead of overwriting when
another session got there first. A hand edit to frontmatter skips all of
that: it can land a value the schema rejects, silently discard another
session's change, and leave the `updated:` and `closed:` stamps wrong.
The body below the frontmatter is ordinary markdown; `note`, `check`,
and `body` cover most edits, and editing prose by hand is fine when they
do not.

An invalid status or field value is rejected with the allowed set
spelled out. That message is the schema talking; the fix is a different
value, not a retry.

## Scheduling lives in the frontmatter

`lane`, `lane_seq`, `collision`, and `blocked_by` are the execution plan.
`issuectl dag` derives each lane's head-of-line from them, `/stint-start`
spawns only what the DAG reports spawnable, and nothing else ever sweeps
the tracker. This repository has no backlog: an open non-epic issue is
either in a lane or newly arrived and awaiting a decision, and `/wrap-up`
re-presents anything else at the end of every session because an issue
outside the DAG would otherwise never be picked up. `lane: unlaned` is
not an absent lane; it means confirmed parallel-safe work. `issuectl dag
--help` explains how lanes are meant to be designed, as serial queues cut
at independently mergeable boundaries rather than theme labels, and why
priority outranks `lane_seq`. The hot-file clusters named in the
top-level `AGENTS.md` are what `collision:` tokens are for.

Labels describe content. Lifecycle is never encoded in a label: the
tracker migrated from label-encoded intake state to the `untriaged`,
`deferred`, and `needs-info` statuses, and a `deferred` or `blocked`
label would fall outside every query and every DAG computation that
reads status.

## Intake

Bundled skills file their findings with `issuectl intake file`, which
lands them as `untriaged`. Disposing of a report through `issuectl
intake` (`accept`, `defer`, `need-info`, `reject`, `duplicate`,
`cannot-reproduce`, `obsolete`) rather than a bare status change records
the disposition fields that later triage and metrics read, such as
`disposition_reason` and `duplicate_of`. Before an intake report costs a
worker, reproduce it against the installed taskfleet: the top-level
`AGENTS.md` records three stints in a row where the report described a
fixed defect or a misread JSON surface.

## Body, commits, and closing

No issue type here declares required sections, so the body exists for
the next reader, not the validator. Practice in this repository is a
`## Description` on every issue, a `## Resolution` when it closes, notes
through `issuectl note` so comments, decisions, and agent runs carry a
timestamp and author, and an `## Acceptance Criteria` checklist when the
work has a definition of done. That exact heading and the `- [ ]` syntax
are what `issuectl ready` and `issuectl check` read; the lower-case
variant in some older issues is invisible to them. A closing issue
should say what happened and where the change landed, because it is the
record the next investigation of the same area will find.

Commits reference issues with a `Refs-Issue: @<slug>` trailer, or
`Fixes-Issue:` when the commit resolves the issue. `issuectl
sync-commits` walks history and records trailered commits on the issue,
and `changelog` and `timeline` read the same trailers, so `--add-commit`
is only for a commit that has none and a missing trailer is worth fixing
before the commit is pushed.

`issuectl doctor` is a cheap read-only check for dangling epic and slug
references, closed issues without a `closed:` date, and drift in the
managed block below. The repository is clean today, so a finding after
your work is yours. It earns a run after a `rename`, after touching many
issues, and before a handoff.

## What is at stake

Everything in `issues/` is in git, so a wrong mutation is recoverable
and none of this needs permission. What deserves care is the signal the
tracker sends to other agents. A wrong disposition or a missing lane
misroutes future scheduling and triage. `rename` rewrites references in
every other issue. `bulk` touches everything a query matches and has a
`--dry-run` for that reason. When a choice would change how someone else
acts, record the reason with `issuectl note --decision` at the time; the
reason is the part the next reader cannot reconstruct.

<!-- issuectl-managed:start -->

<!-- issuectl-managed:format=1 -->

## Schema-derived rules (generated)

_Regenerated by `issuectl doctor --fix`. Do not hand-edit between the sentinels._

### Frontmatter fields

- `assessment_classification` (optional, scalar)
- `assessment_outcome` (optional, scalar)
- `assignee` (optional, scalar)
- `blocked_by` (optional, list)
- `closed` (optional, scalar)
- `closed_by` (optional, scalar)
- `collision` (optional, list)
- `created` (optional, scalar)
- `deferred_until` (optional, scalar)
- `disposition_note` (optional, scalar)
- `disposition_reason` (optional, scalar) — allowed: by-design, out-of-scope, wontfix, withdrawn, superseded
- `duplicate_of` (optional, scalar)
- `epic` (optional, scalar)
- `labels` (optional, list)
- `lane` (optional, scalar)
- `originating_run` (optional, scalar)
- `originating_run_kind` (optional, scalar)
- `owner` (optional, scalar)
- `priority` (required, scalar) — allowed: normal, high
- `provenance` (optional, scalar)
- `provenance_detail` (optional, scalar)
- `related` (optional, list)
- `reporter` (optional, scalar)
- `review_confidence` (optional, scalar)
- `review_severity` (optional, scalar)
- `review_source` (optional, scalar)
- `review_status` (optional, scalar) — allowed: requested, in-review, approved, changes-requested
- `review_target` (optional, scalar)
- `reviewer` (optional, scalar)
- `size` (optional, scalar) — allowed: S, M, L, XL
- `slug` (optional, scalar)
- `source_ref` (optional, scalar)
- `status` (required, scalar) — allowed: open, in-progress, testing, untriaged, deferred, needs-info, done, fixed, wontfix, duplicate, cannot-reproduce, obsolete
- `type` (required, scalar) — allowed: bug, task, feature, improvement, chore, epic
- `updated` (optional, scalar)

### Required body sections by issue type

_No per-type body-section requirements declared._

### Status-transition rules

_No transition rules declared (lenient default)._

<!-- issuectl-managed:end -->
