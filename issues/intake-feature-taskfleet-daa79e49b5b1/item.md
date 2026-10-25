---
created: 2026-10-25
updated: 2026-10-25
type: feature
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:wrapup-20261025-release-race
---

# release wrapper aborts cut when issue-only intake commits land on main

## Description

release wrapper aborts cut when issue-only intake commits land on main

During the 0.17.1 cut on 2026-10-25, `scripts/shipshape-release.sh cut <plan>` (shipshape run 01M5RMTFRRCG3HAA29FS1G1C1X) built, delegated and created the held local tag, then stopped with `origin/main moved to 167a90ec during the release; reconcile deliberately`. The two new commits on origin/main were intake filings (`bug intake: intake-bug-taskfleet-a94f2603219c`) touching only `issues/`. The documented recovery is abandon, delete the local tag, pull, re-plan and cut again (~10 min each); the third attempt succeeded.

The wrapper requires origin/main == base or == bump commit (scripts/shipshape-release.sh ~line 281). Intake filings arrive on taskfleet main routinely, so a cut can lose this race repeatedly. Expected: when every commit between base and origin/main touches only `issues/`, rebase the bump commit onto origin/main, re-run the exact-HEAD validation on the result, and continue; refuse as today for any other change.

Reported by jari via agent:homebase-wrapup.
