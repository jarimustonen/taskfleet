---
created: 2026-09-09
updated: 2026-09-09
type: bug
status: open
priority: high
lane: skills-format
collision: [taskfleet-skills]
---

# Parse every shipped skill description as valid YAML

## Problem

Actual Haapa Pi0.85.1 startup during the first real Shipshape session migration rejects the Taskfleet worktree-bug-analysis skill YAML. Source0.9.0 still has crates/taskfleet/skills/worktree-bug-analysis/SKILL.template.md line3 unquoted description containing `issue: reproduce/explain`; parsing the frontmatter with a real YAML parser fails ScannerError mapping values are not allowed here. Installed0.8.2 has the same problem. The runtime is otherwise usable but this shipped skill does not load.

## Acceptance

Quote/block-scalar the description without changing its meaning; ensure all shipped/generated skill frontmatter is parsed by a real YAML parser in an appropriate validation gate so this specific colon case cannot recur. Preserve template variable semantics and avoid broad editorial changes. Source-only, no globalTaskfleet/skill install, publish through normal heldtagrelease then Homebasefleet. Evidence from actual migration and sourceinspection, not hypothetical lint preference.
