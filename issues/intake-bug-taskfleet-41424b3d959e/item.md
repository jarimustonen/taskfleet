---
created: 2026-09-09
updated: 2026-09-09
type: bug
reporter: jari
status: fixed
priority: high
provenance: agent:homebase-haapa-migration
source_ref: agent:homebase-haapa-migration/reporter:jari/id:homebase-haapa-taskfleet-etxtbsy-20260909
lane: merge-runtime
collision: [crates/taskfleet/src/run/merge.rs]
closed: 2026-09-09
closed_by: pi
---

# Linux native merge executes a write-open tempfile and fails ETXTBSY

## Description

Linux native merge executes a write-open tempfile and fails ETXTBSY

Observed production Linux failure in Taskfleet 0.8.2 commit 0e03bac26eaa7033433b7f0b5523ef61adcca2c9: native run merge failed twice with merge_spawn_failed / ETXTBSY on generated /tmp/taskfleet-merge-*.sh. Homebase runtime run 01m22a1j2q0tcmk6ypnq565bkk preserves clean tested commits 6a28f7d5 and 6407bf62; a second continuation worker also hit the same failure. This blocks native completion despite successful required tests.

Concrete source cause: crates/taskfleet/src/run/merge.rs materialize_merge_sh writes and flushes a writable tempfile::NamedTempFile, chmods it0700, and returns MergeScript::Temp(tmp). run_merge_sh retains that object while Command::new(script.path()).output() calls exec. Linux execve rejects a script whose inode remains open for writing (ETXTBSY). flush does not close the descriptor. Retrying unchanged native salvage uses merge::execute and the same spawning code, so it cannot fix the defect. Dry-run never executes the backend. Local v0.9 implementation still has this lifetime pattern (independent conductor verification).

Run events show merge.started then merge.aborted twice at 2026-09-09T05:39:00Z and05:39:04Z; source expected b9d541939b3e2e94e43ca393849fee1df17dbcff and worker6407bf620783e686b8b539a9e3be21d9dfa57bd0, no success report. Full source evidence is available to Homebase migration conductor in /tmp/haapa-development-runtime-terminal.json.

Minimal fix: close the writable file before exec while preserving automatic pathname cleanup and lifetime through the child, e.g. convert NamedTempFile into TempPath and retain that guard. Preserve native CAS/merge transaction/source checks/reporting/cleanup. Add a Linux regression invoking the REAL default embedded backend without TASKFLEET_MERGE_SH; current real-script integration tests override the backend and therefore miss this lifetime bug. Test successful native run merge and salvage, script path lifetime/cleanup on failure, and unchanged source/ref/report behavior on spawn failure.

The existing TASKFLEET_MERGE_SH external backend path can temporarily execute an exact byte-verified published bundled script after its write descriptor closes, retaining the native driver guards; this is an internal test override, not a product fix or authorization for ad-hoc Git merging. No binary patch/install/release was performed by the reporting agent.

Owner: Taskfleet. Suggested straightforward disposition: high-priority bug in a dedicated merge-runtime lane, collision token for crates/taskfleet/src/run/merge.rs; coordinate any release with active retention owner. Do not silently classify it as transient external locking.
