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

<!-- intakectl:analysis:start job:b76c57b1-c1e8-48b7-9690-27d2bd499623 generation:0 -->
## Triage analysis

### Confirmed: deterministic race condition in retention test fixture

The bug is a **classic SIGSTOP/SIGCONT ordering race**, confirmed by source inspection at `crates/taskfleet/src/session.rs:1128-1193`.

### Race mechanics

The fixture creates a shell script:

```sh
#!/bin/sh
kill -STOP $$
echo archived
```

`tmux new-window` spawns this script and returns the pane PID. The test then:

1. Sets `remain-on-exit on` and OWNER_OPTION on the window
2. Immediately sends **SIGCONT** to the pane PID (`libc::kill(pane_pid, SIGCONT)`)
3. Polls `#{pane_dead}` for up to 2 seconds

The script's own `kill -STOP $$` sends SIGSTOP to itself — the process stops at that `kill` call. The test's SIGCONT is meant to wake it so it finishes with `echo archived` and exits.

**The race**: there is NO handshake proving the script reached `kill -STOP $$` before SIGCONT is sent. Under concurrent load (native full release gate + other tests), SIGCONT can arrive at the process BEFORE the script executes `kill -STOP $$`. In that scenario:

- SIGCONT on a running process is a no-op (no-op, not queued)
- The script then hits `kill -STOP $$` and **stops permanently** — no further SIGCONT is coming because the test already sent it
- `echo archived` never runs, the pane never exits, `#{pane_dead}` stays 0
- The test hits the 2-second timeout and panics with "retained test pane did not exit"

### Observed evidence matches the race

The failure at `session.rs:1191` `"retained test pane did not exit"` is the exact symptom of the pane being stopped forever. The prior full-gate pass and later full-gate failure are consistent with a load-dependent race window: under less contention SIGCONT arrives after STOP; under more contention it arrives before STOP.

### Recommended fix: stopped-state readiness handshake

Replace the shell script with a two-phase pattern that signals readiness before stopping:

```diff
- #!/bin/sh\nkill -STOP $$\necho archived\n
+ #!/bin/sh\necho ready >&2\nkill -STOP $$\necho archived\n
```

And have the test poll stderr for the `ready` line before sending SIGCONT:

```rust
// Wait for child to signal it reached the STOP point
let ready_deadline = Instant::now() + Duration::from_secs(5);
loop {
    if read_child_stderr(pane_pid).contains("ready") { break; }
    assert!(Instant::now() < ready_deadline, "child never reached STOP point");
    sleep(10ms);
}
unsafe { libc::kill(pane_pid, SIGCONT); }
```

Alternatively, use a file-based handshake:

```diff
- #!/bin/sh\nkill -STOP $$\necho archived\n
+ #!/bin/sh\ntouch ready.marker\nkill -STOP $$\necho archived\n
```

And poll `ready.marker.exists()` before SIGCONT. Both approaches guarantee the child is in the STOP state before CONT is delivered.

### What does NOT fix this

- **Arbitrary longer sleeps** before SIGCONT — the race window is unbounded under extreme load; a sleep just shifts the probability.
- **Longer pane_dead timeout** — the pane never exits, so no timeout length helps.
- **Weakening the assertion** — the test must verify the pane actually exits for the retention cleanup path to be tested.
- **Removing the STOP/CONT dance entirely** — the test needs a pane that dies on demand to exercise the retention expiry path; the STOP/CONT pattern is the correct mechanism, just missing the handshake.

### Scope and risk

- Only `real_private_tmux_expiry_preserves_unrelated_split_and_is_idempotent` at session.rs:1087 uses this exact fixture pattern.
- The sibling test `real_retention_archives_before_killing_only_owned_pane_and_keeps_active_worker` at session.rs:1258 may have similar fixture logic (check before assuming no race).
- No production code is affected — this is a test-only fixture race.
- The fix is a small script change plus a polling loop before SIGCONT, no structural changes to the retention logic itself.
<!-- intakectl:analysis:end job:b76c57b1-c1e8-48b7-9690-27d2bd499623 generation:0 -->
