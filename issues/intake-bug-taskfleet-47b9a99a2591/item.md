---
created: 2026-09-09
updated: 2026-09-09
type: bug
reporter: jari
status: untriaged
priority: normal
provenance: agent:haapa-migration
source_ref: agent:haapa-migration/reporter:jari/id:haapa-taskfleet-retention-stop-race-20260909
---

# Retention tmux fixture can continue before child reaches SIGSTOP

## Description

Retention tmux fixture can continue before child reaches SIGSTOP

Observed full final-HEAD release gate failure in Taskfleet retention test on Gertrud, native worker01m22cwzt2nkcevchwy5j5t33z at finalHEADa20ce402e45b59fbef4d91914abae5f7ea0c03b6: session::tests::real_private_tmux_expiry_preserves_unrelated_split_and_is_idempotent failed3.302s, panic crates/taskfleet/src/session.rs:1191 "retained test pane did not exit". Prior explicit full nextest and earlier full local release gate passed; this later full-gate failure is preserved, not erased by focused retry.

Concrete fixture ordering: writes dead.sh with `kill -STOP $$` followed by echo archived; tmux new-window returns window/pane/PID; test sets remain-on-exit and owner options, immediately sends SIGCONT, then polls pane_dead for2s. There is NO handshake proving the child reached STOP before CONT. Under load CONT may arrive before the script reaches STOP, after which the script stops and never exits. This is a source-grounded hypothesis with exact observed failure, not yet a deterministic reproduced ordering. Failure is before the tested retention cleanup path. Investigate an explicit stopped-state/child readiness handshake with failure-safe owned fixture cleanup, not arbitrary longer sleeps or weakened lifecycle assertions.

Worker is isolating the existing test and intends rerun of complete required gate. No production process or code changes by reporter. Native stderr and SHA privately preserved on Gertrud and Haapa /tmp/taskfleet-retention-stop-race-evidence/{proc_4110-stderr.log,manifest.json}. This is distinct from @intake-bug-taskfleet-184a86a0335a (partial output read in supervisor retry test) and the Linux ETXTBSY native merge implementation bug. Taskfleet owner should disposition a bounded retention-test reliability fix; explicit source publication coordination is needed with current0.10 release owner.
