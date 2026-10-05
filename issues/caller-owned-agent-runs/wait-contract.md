# Caller-owned Pi lifecycle (bounded wait slice)

`run session bind` verifies the native session header and checkout and records
an immutable UUID/path. Create the native header, bind it, then call `run session
update <full-run-id> --node n-0001 --generation 1 --pi-session-id <uuid>
--session-path <absolute-jsonl> --checkout <verified-checkout> --state started`
**before** launching Pi. The `started` fact is registration of an attempt, not
proof of launch or liveness. After reaping that exact child, attest `exited
--reason <reason>` with the same generation and identity; if no child existed,
attest `launch-failed`; if control or exit cannot be proved, attest
`control-uncertain`. All updates are same-UID local attestations, not verified
host ownership. The host must verify its own child identity, native history,
checkout and ownership lock, and reconcile a lost reply by `run show` before
retrying. Same current fact replays without another event. Conflicting or stale
facts fail. A next generation may register only after an exited or launch-failed
fact, and only against the same immutable bound session; uncertainty bars
restart. The host still needs a writer fence and no-duplicate launch protocol;
this CLI neither launches Pi nor authorizes merge, cancel, salvage or discard.

`run wait` reads manifest and node together under the shared run lock. For
caller-owned runs only, a told current-generation exit, launch failure or
uncertainty settles a nonterminal wait; started alone does not. Terminal status
wins. The response carries `caller_agent.state` (the **current** projection),
`current_generation`, `settled_generation`, `settled_state`, session identity,
and reason (latched settling reason when applicable). In a stop/restart race,
`settled_state` describes why this wait woke while `state` describes the new
writer attempt; do not treat the older exit as current. On timeout, an active
registration is still `started` with `control: unknown`; silence proves nothing.
`--any` grades only settled runs. `--fail-on-error` returns 3 for caller attention,
2 for timeout, without changing manifest status. Ordinary worker wait output
remains unchanged. `run show` and `node show` expose the durable current fact.
