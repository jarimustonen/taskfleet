---
created: 2026-08-20
updated: 2026-09-07
type: feature
status: open
priority: normal
provenance: agent:issuectl-stint-wrapup
lane: run-read-surfaces
lane_seq: 20
---

# run list has no repo filter, so sibling-repo runs are indistinguishable by title

## Description

## Problem

`taskfleet run list` has no way to filter by repository. Runs from every repo on the
machine share one flat list (1010 runs on this machine at time of filing), so an
orchestrator opening a session in repo X must inspect runs one by one to find out which
ones actually belong to X.

## Why it matters

Cross-repo campaigns spawn **similarly-titled runs in several repos**, so titles are
actively misleading rather than merely unhelpful. This session (working in `issuectl`)
found two pending runs whose titles read as issuectl work:

```
{"run_id":"01m0evreacbddfg5z84w4pmw69","status":"pending","title":"doctor-report-binary-commit"}
{"run_id":"01m0evr1sfj655g3f6kzjzty92","status":"pending","title":"audit-no-user-specifics"}
```

Both are plausible issuectl work (issuectl has a `doctor` subcommand, and
`audit-no-user-specifics` matches an issuectl-repo issue slug). Only a per-run `run show`
disambiguated them:

```
$ taskfleet run show 01m0evreacbddfg5z84w4pmw69 --output json | jq -r '.data.worktree_path'
/Users/jari/Sources/taskfleet__worktrees/wt-01m0evreac-doctor-report-binary-commit
```

They are **taskfleet** runs, not issuectl runs.

The risk is concrete: an orchestrator that mistakes a sibling repo's run for its own can
conclude it has live workers holding resources it does not, or worse, treat a foreign run
as abandoned work needing salvage. The issuectl repo's own AGENTS.md carries a standing
warning about exactly this ("A same-titled orchestrator run in a sibling repo is NOT this
repo's issue... never infer from the run title"), which is evidence the trap bites in
practice — but a written warning is a weaker fix than a filter.

## Requested

A repo/worktree filter on `run list`, for example:

```
taskfleet run list --repo /Users/jari/Sources/issuectl
taskfleet run list --repo .        # the cwd's repo
```

Or, at minimum, include the source repo in each `run list --output json` row so a caller
can filter client-side in one pass instead of N `run show` calls.

## Severity

Convenience gap rather than a defect: the information is available, just not without a
per-run lookup. Filed at the maintainer's discretion after weighing it as lower-value than
the session's other findings.
