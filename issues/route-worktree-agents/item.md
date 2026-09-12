---
created: 2026-09-12
updated: 2026-09-12
type: improvement
status: open
priority: normal
---

# Route worktree agents by model capability

## Description

Define Taskfleet model-routing policy so autonomous work uses a model appropriate to task risk and boundedness instead of always inheriting ambient Pi defaults.

## Intent

Separate the policy into two layers:

1. Taskfleet configuration owns executable named profiles and their agent launch commands.
2. Bundled workflow skills explain the routing matrix and select a profile when creating a run; they must not hard-code model names in per-issue briefs.

Initial candidate routing:

| Work | Profile / model tier |
| --- | --- |
| Technical decision, ADR, broad or high-risk design | `capable` |
| Bounded implementation from an accepted design | `implementation` (for example Terra) |
| Mechanical, strongly tested refactor or documentation | `lightweight` (for example Luna) |

A caller may explicitly escalate a particular issue when its risk warrants it. Model choice is not a substitute for deterministic tests, scenario/user-environment validation, or the repository green gate.

## Ownership / likely surfaces

- profile schema, selection validation and recorded run selection in Taskfleet runtime/config
- `crates/taskfleet/skills/taskfleet-overview/SKILL.template.md`: routing matrix and configuration/source-of-truth explanation
- `crates/taskfleet/skills/worktree-spinoff/SKILL.template.md`: implementation default and escalation rule
- `crates/taskfleet/skills/worktree-technical-decision/SKILL.template.md`: decision default

Current `~/.taskfleet/config.toml` has only `capable`, launching bare `pi`; runs therefore inherit ambient Pi model selection. Do not change installed Taskfleet or user configuration as part of this issue. Design the configuration contract and backward-compatible behavior first.
