---
created: 2026-09-09
updated: 2026-09-09
type: bug
reporter: jari
status: untriaged
priority: normal
provenance: agent:haapa-migration
source_ref: agent:haapa-migration/reporter:jari/id:haapa-taskfleet-supervisor-retry-partial-read-20260909
---

# Supervisor retry test reads partially written fixture output under load

## Description

Supervisor retry test reads partially written fixture output under load

Observed twice during the real full release nextest gate on Gertrud while an isolated Podman Linux regression VM was active: taskfleet supervise::tests::profile_retry_uses_recorded_candidate_and_absolute_attempt failed at crates/taskfleet/src/supervise/mod.rs:4767. Native worker run01m22cwzt2nkcevchwy5j5t33z (Linux ETXTBSY fix, sourcebasec6afc73) recorded full-gate exit100 twice. This report is separate from the ETXTBSY implementation issue.

First retained stderr proc_8214: FAIL13.533s, observed="01jxwd0000000000000000000w\nn-0001\n3\n"; summary666/1117 tests run,665passed1failed1skipped. Second proc_9169: FAIL11.768s, observed=""; summary647/1117 tests run,646passed1failed1skipped. Both panics assert the expected run-id/node/attempt/candidate/--session prefix, but see partial or empty data. Do not call either aborted run a successful full suite.

Source-grounded likely fixture race: the test waits only until the output path exists, then immediately reads it. The spawned fake harness creates/truncates the file and writes multiple lines; file existence is not writer completion. Under VM/concurrent release-test load, reader can see zero or partial bytes. Investigate full writer completion or explicit end-of-record/atomic-publication synchronization rather than arbitrary sleeps or weakening the expected content.

Worker stopped its disposable Podman VM, then reran the EXACT full command `cargo nextest run --locked --release --workspace`, without narrowing or ignoring tests. proc_dfcb passed1117tests,1skipped,102.870s. Full scripts/validate-local-release.sh is still running separately at reporting time. The VM-load correlation is observed, not a deterministic causal proof; the partial-read assertion evidence is concrete. Isolated focused pass alone is not the acceptance proof.

Exact stderr logs and hashes privately preserved on Gertrud /tmp/taskfleet-supervisor-retry-race-evidence/{proc_8214-stderr.log,proc_9169-stderr.log,proc_dfcb-stderr.log,manifest.json}; copies staged on Haapa at the same /tmp directory. No product source changes by reporter. User authorizes filing encountered substantial problems. Suggested owner Taskfleet, bounded test-reliability bug in supervisor test scope. A truly green final full release gate may proceed; retain both failures and add deterministic race regression/robust synchronization rather than silently relabeling this as external flakiness.
