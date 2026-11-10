---
created: 2026-11-10
updated: 2026-11-10
type: feature
status: untriaged
priority: normal
provenance: other
provenance_detail: Open Habitat ADR 0036 change list (Taskfleet / conductor skills row)
source_ref: taskfleet:01m71zxvdgwg2vsmd0cdrffz5p/adr-0036:conductor-separate-product-root
originating_run: 01m71zxvdgwg2vsmd0cdrffz5p
originating_run_kind: spinoff
---

# Conductor skills: separate private narrative root from the product tracker and worker source

## Description

A project can split its maintainer workflow from its product: a public product
repository holds code, the issuectl tracker, DAG and prototypes, and a
private sidecar repository holds the conductor's `TODO.md` narrative,
trial notes and private per-run output. Open Habitat adopts this split
(its design record checked the existing interfaces at taskfleet 0.20.0,
commit `6ad8d7ca`): no binary feature is required, because
`issuectl --root <product>` and `taskfleet run create --source-repo
<product> --source-branch main` already work from another cwd. What is
missing is in the conductor skills, which today assume the narrative
checkout, the tracker and the worker source are the same repository.

Teach `/stint-start`, `/stint-handoff`, `/night-stint-prep` (and the
issue-resolution part of `/worktree-spinoff` where the conductor builds the
brief) to:

- take the narrative root (cwd, private) and an explicit product root
  (tracker and worker source) as separate inputs, read the DAG with
  `issuectl --root <product> dag --json`, resolve `issuectl:<slug>` against
  the product, and pass `--source-repo <absolute product root>`
  `--source-branch <branch>` instead of letting cwd choose the source;
- keep public worker filing (intake, unlaned, provenance), status updates,
  close/stamp, full run IDs and the single `run merge` unchanged; no
  generated intake-policy change;
- optionally permit a worker to write private output only under
  `<narrative root>/worker-results/<full-run-id>/`, never staging there; the
  conductor reconciles each such directory after verifying the landed product
  commit and commits explicit paths;
- recover after interruption: a run that landed in the product while its
  private notes are still uncommitted is reconciled from its recorded run ID,
  not re-run.

Note the run-create brief copy lands under `history/.worktree/` in the product
checkout; private briefs rely on that ignore rule, so the skill should not
weaken or bypass it.

Acceptance: with a single-repository project, behaviour is unchanged. With a
split project, a round launched from the narrative root creates a worker in
the product repository, the worker updates and closes the product issue and
merges there, children of a driver run follow the same source, and a restart
after product landing with uncommitted private notes reconciles correctly.
