---
created: 2026-09-28
updated: 2026-09-28
type: task
reporter: agent
status: untriaged
priority: normal
provenance: other
provenance_detail: homebase rethink-all-instructions session
---

# Admit Shipshape 0.12.4 in the release wrapper

## Description

Shipshape 0.12.4 was released on 2026-09-28 and is now Homebrew stable and the homebase fleet pin. scripts/shipshape-release.sh admits only the exact tested Shipshape 0.12.3 build and refuses when a newer stable exists, so the next Taskfleet release is blocked until 0.12.4 is validated and allowlisted.

Evidence for a small validation: git diff v0.12.3 v0.12.4 in the shipshape repository (jarimustonen/ossctl) touches only the eleven bundled skill templates under crates/shipshape-cli/skills/ and the three version fields; the release engine source is unchanged. Release commit 8db733bd70f566ab39b4cd774901c6e2ca8fd2c3.

Taskfleet 0.11.4 was cut with the 0.12.3 wrapper minutes before 0.12.4 was published, so no release is currently stranded.
