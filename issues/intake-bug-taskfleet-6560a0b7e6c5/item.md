---
created: 2026-09-10
updated: 2026-09-10
type: bug
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-taskfleet-cutover
source_ref: agent:homebase-taskfleet-cutover/reporter:jari/id:taskfleet:01m25aak6vsxwcfjppj44ht618/native-intake-lifecycle-gap
---

# Add a native bounded one-shot worker contract

## Description

Native one-shot profile can race publication and remain non-terminal after clean exit

Observed on Haapa with the released `taskfleet 0.10.0` binary at commit `76afe2a64e464e05c901b78bfc8c1013fdcd27fe` while evaluating removal of Homebase's temporary intake launcher.

Two native lifecycle properties still do not cover the intake one-shot contract:

1. `run create` starts the selected candidate while the run remains under `$TASKFLEET_HOME/.creating`. `crates/taskfleet/src/run/spawn.rs::write_agent_launcher` explicitly says Pi may start before publication, and `crates/taskfleet/src/run/create.rs` appends `node.created` and atomically renames the staging run into `runs/` only after native materialization returns. A candidate that immediately needs public `run show` / `node show` identity can therefore race publication.
2. The native `run-worker` records a clean `worker.exited`, but by design a clean exit without a terminal report remains non-terminal and `attention_required`; it does not translate a successfully completed one-shot Pi process into verified terminal success. Homebase's intake launcher currently performs that terminal-report bridge after `pi --print` exits.

Real-world impact: Intakectl's bounded, fixed-model Haapa analysis profile cannot safely replace `/home/jari/bin/haapa-intake-worker` with a direct `pi --print --model homebase-deepseek/deepseek-v4-flash` candidate without either an early identity race or a clean-exit/no-report hang. The bridge must remain until a released native contract supplies an attempt-bound publication barrier visible to the candidate and a fail-closed way to terminalize the successful one-shot contract (without treating arbitrary clean exits as merged work).

Expected: expose a native primitive/profile contract that preserves Taskfleet's no-false-success rule while allowing this trusted one-shot intake use case to (a) wait for public run+node identity before candidate work and (b) reach a verified terminal success after bounded clean completion. Failure, timeout, retry, duplicate-report, and stale-attempt paths must remain fail-closed.

This filing is also the bounded real intake canary for Homebase issue `taskfleet-launcher-cutover-order`: it must be deterministically filed and landed first, acknowledged with the real slug, then analyzed through the existing quarantine/pinned-base gate. It contains no secrets. Do not implement or disposition it as part of the canary analysis.

Originating run: `01m25aak6vsxwcfjppj44ht618` (kind `spinoff`).
