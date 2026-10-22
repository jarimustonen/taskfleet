# Handoff preservation validation

## Executor-style scenario review

Reviewed the complete stint-handoff template, including tracker-free handling,
runtime fallback, ownership inventory, scheduling, and final completion wording.
These are instruction-level walkthroughs, not claims of live cleanup execution.

| Input | Expected execution under the revised instructions | Resource/schedule outcome |
| --- | --- | --- |
| Clean harvested retained work; authorized native cleanup refuses | Inspect exact owned run, verify delivery in git, record retained inventory and native refusal in narrative, commit narrative, run permitted wrap-up lesson capture, tool-friction reporting and lightweight checks | Retained copy untouched; settlement incomplete; next authorized recovery action reported |
| Genuine unmerged retained work; no disposition | Inventory and record run id, slug, retained work and pending user choice; commit narrative; continue independent preservation | No salvage, harvest, discard or deletion merely for tidiness; ownership remains unresolved |
| No retained resources; valid schedule | Verify owned inventory is empty, check schedule with foreign holds, commit narrative then run wrap-up | Preservation and settlement may both be reported complete when independently evidenced |
| No external wrap-up skill; retained cleanup blocked | Save lessons in specific existing agent instructions or owning document, report tool friction through permitted path, run enabled lightweight tracker checks, commit exact paths separately | Fallback preservation proceeds; missing external skill is not itself a failure; retained resources untouched |
| Invalid DAG (including command failure or non-executable disposition in lane) | Record failure and affected slugs in narrative and final report, skip schedule-dependent actions, continue independent saving and permitted lightweight checks | No fabricated schedule or automatic issue disposition; verification remains failed |
| Foreign retained run cannot be mapped to a hold | Read only; disclose live reservation uncertainty in terminal report for next conductor; continue preservation | No control of foreign run; no claim scheduling is verified beyond available evidence |

The clean-index/file-conflict preflight still protects other sessions' edits.
A blocked or unsafe individual saving action is disclosed and skipped rather
than granting new permissions. Required failures remain failures; successful
preservation is not described as full handoff/resource/scheduling completion.
No concrete executor ambiguity remained requiring an external targeted review.

## Regression evidence

`tests/skill.rs::handoff_preservation_is_independent_of_settlement` invokes the
production embedded `skill print` path with isolated HOME/TASKFLEET_HOME and
empty PATH (no stubs needed: printing must invoke no external tools). It pins
the common preservation contract and final ordering as reviewed snapshots and
rejects the former wrap-up suppression wording. Existing skill tests cover
catalog and rendering; no catalog membership or description changed.

The required clean-commit release gate log is
`/var/tmp/taskfleet-jfdxgrhmmb/gate.log`; focused release test log is
`/var/tmp/taskfleet-jfdxgrhmmb/focused-local.log`.
