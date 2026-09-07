# End-to-end stint: product and design option map

## Purpose and fixed boundaries

This is an exploration, not an architecture decision. It maps ways to turn the current sequence of `stint-start`, `worktree-status`, `stint-handoff`, and `wrap-up` into a bounded, resumable experience without reviving Taskfleet's former campaign orchestrator.

The following boundaries apply to every option:

- **issuectl owns execution scheduling.** Its reservation-aware DAG is the source for lane order, dependencies, collisions, and spawnability. A stint must not copy or reinterpret that graph.
- **`TODO.md` owns narrative handoff.** It describes intent and context, not live scheduling or machine state.
- **Repository `AGENTS.md` owns project policy.** Green gates, deploy/release mechanics, hot files, and repository-specific autonomy remain there.
- **Taskfleet owns worker/run truth.** `run merge` remains the only success truth; typed terminal outcomes and explicit cancellation remain authoritative. Advisory telemetry never completes, retries, merges, cancels, cleans up, or advances a stint.
- **A harness may wake a conversation, not become the durable truth.** The external `taskfleet-pi-telemetry` adapter now exists, but its samples remain observations. A pi adapter or neutral runner may wait on public contracts and inject a new turn.
- **The loop is bounded.** No option may continuously invent work from the backlog or turn user silence into approval.
- **Public artifacts stay neutral.** User names, private paths, personal release preferences, and private prompt text belong in user-owned configuration.

## Current journey

### Happy path

1. The user invokes `stint-start`.
2. The conductor pulls, reads repository policy and the `TODO.md` narrative, checks git/deployment ground truth, reconstructs live Taskfleet ownership, and reads `issuectl dag --json --reservations ...`.
3. It selects only accepted, scheduled work; launches disjoint work in parallel and collision-prone work sequentially; then uses `taskfleet run wait` and each result's `landed` evidence.
4. It reads worker reports, runs the repository green gate, optionally deploys under repository policy, and gathers the round's durable facts.
5. `worktree-status` turns those facts already present in the conversation into a product-owner summary, test points, decisions, discussion points, and follow-ups.
6. The user responds. Small feedback starts a fresh `stint-start` pass; larger feedback is recorded durably.
7. When the user asks to finish, `stint-handoff` verifies that no run still owns work, verifies the issue DAG, updates and commits the `TODO.md` narrative, invokes `wrap-up`, performs any declared reset, and checks that the repository is clean.

The individual stages are already composable. The gap is continuity: the user or conversational agent must remember which skill comes next, and a fresh session has no single durable answer to “this stint is awaiting Jari's response” or “those answers have been acknowledged and the next stint is eligible.”

### Important interruption paths

- **No prepared narrative:** `stint-start` pauses for one planning pass rather than treating the whole open backlog as the agenda.
- **Worker requests a decision:** `node.awaiting_input` is durable and fenced by `event_seq`. It becomes visible immediately and, after a fixed grace, settles `run wait` without terminalizing the run. The issue hold must remain until the request is resolved and ownership is released.
- **Clean exit without merge:** `run wait` reports `attention_required`; a human chooses a deliberate finish/salvage or cancellation path.
- **Failed, stalled, or recoverable worker:** preserved work remains owned. Recovery is explicit and reviewed; a conductor must not treat a returned wait as permission to schedule over it.
- **Green gate failure:** deployment stops and a fix round may be launched. Red work is never deployed.
- **Deployment needs consent:** silence or ambiguous policy pauses the flow; it does not imply go-ahead.
- **Live ownership during handoff:** the handoff records the run and preserved-work fact in `TODO.md`, commits that narrative, and stops before claiming a complete wrap-up.
- **Invalid issue DAG:** execution stops. During terminal handoff, the failure is recorded in the narrative and `/wrap-up` is not declared complete.
- **Conversation or pi session ends:** Taskfleet runs and issue/TODO state survive, but the conversational chain does not automatically resume unless a user or external runner starts another turn.

Telemetry can help the conductor decide whether to inspect or wait, but `current tool_running`, `settled`, stale, absent, and invalid are all non-authoritative observations. In particular, telemetry must not close any interruption path above.

## Shared vocabulary for comparing options

The options can use this small conceptual lifecycle without requiring all of it to become Taskfleet state:

- **ready** — coherent narrative and valid scheduling sources allow a new round;
- **executing** — the conductor has live run ownership or is processing the current round;
- **preparing-handoff** — workers have settled and durable facts are being reconciled;
- **awaiting-user** — a bounded set of test results or decisions has been presented and user input is required;
- **finalizing** — recorded answers are being applied to issues/narrative and terminal checks are running;
- **ready-next** — the acknowledgement/finalization succeeded and a later invocation may begin a new stint.

This is not a second worker state machine. Worker states remain in Taskfleet, scheduling remains in issuectl, and narrative remains in `TODO.md`. Transitions into `awaiting-user` and `ready-next` must be explicit and fenced so an old response cannot acknowledge a newer checkpoint.

A useful universal loop bound would be:

- at most one prepared execution round per wake;
- at most one automatic fix round after a gate failure, if policy explicitly permits it;
- at most one handoff/finalization attempt per wake;
- always stop at unresolved user input, ambiguous ownership, invalid scheduling, failed required validation, or exhausted budget;
- never start the next substantive round merely because the previous checkpoint was acknowledged. A fresh invocation or explicit “continue” is still required.

## Option A — Skill-only chaining

### Shape

Add one neutral umbrella skill, for example `/end-end-stint`, which calls the existing skills in order and carries a small in-conversation phase marker. It would inspect durable sources on every invocation rather than persist a new stint resource.

Example journey:

> “Run the stint.” The umbrella invokes `stint-start`, waits for all owned runs, gathers facts, renders `worktree-status`, and stops at the first decision. Jari answers. The same conversation invokes the umbrella again; it records the answer, optionally performs one feedback round, then proposes or runs handoff according to policy. If pi disappears, Jari starts a new session with “continue the stint”; the skill reconstructs state from Taskfleet, issuectl, git, and `TODO.md`.

### Design properties

- **Ownership:** skills own sequencing; Taskfleet, issuectl, `TODO.md`, and `AGENTS.md` keep their existing domains.
- **Durable state:** no new state. Worker questions are durable through `node.awaiting_input`; session-level questions are durable only if written to an issue or `TODO.md` before pausing.
- **Wake/resume:** pi can run a background `taskfleet run wait` through a neutral process facility and inject a follow-up turn. Without that facility, the user invokes the skill again. The skill reconstructs rather than trusting conversation memory.
- **Policy precedence:** engine safety invariants first; explicit invocation constraints next; user-owned convenience defaults may restrict automation; repository `AGENTS.md` supplies project mechanics and may tighten them. Free-form prompt additions are context only. Any unresolved conflict stops.
- **No-pi operation:** fully usable in Claude, another harness, or a shell-driven agent because wake is optional and all reads are public CLI/file contracts.
- **Loop bounds:** encoded in prose and checked by the agent, ideally with a per-invocation round counter in conversation. This is the weakest enforcement of the four options.
- **Failure recovery:** rerun from the top and reconstruct. Existing Taskfleet recovery paths remain unchanged. A crash before session questions are written can lose the presentation/acknowledgement boundary.
- **Migration burden:** low. Existing skills remain directly usable; the umbrella mainly removes command recall.
- **Human-controlled:** semantic answers, lane-or-close disposition, ambiguous deploy/release authority, preserved-work disposition, and final acknowledgement.

### Trade-off

This is the least invasive design and best test of the intended user journey. Its weakness is exactly the requested “pause durably” behavior: persisting questions in `TODO.md` mixes temporary checkpoint material with long-lived narrative, while persisting them in issues can manufacture issue state. A skill-only phase marker also disappears with context.

## Option B — Thin durable checkpoint state in Taskfleet

### Shape

Add a narrowly scoped **stint checkpoint** resource, not a campaign scheduler. It records phase, generation, repository identity, references to owned run IDs, the presented questions/test points, answer references, and acknowledgement. It does not contain a work DAG, execute issue selection, own deployment, or infer worker completion.

Illustrative CLI only:

```bash
taskfleet stint create --repository "$PWD" --output json
taskfleet stint show 01... --output json
taskfleet stint update 01... --state awaiting-user \
  --checkpoint-file checkpoint.json --expected-generation 3 --output json
taskfleet stint update 01... --answers-file answers.json \
  --expected-generation 4 --output json
taskfleet stint update 01... --state ready-next \
  --acknowledge --expected-generation 5 --output json
```

Illustrative read shape:

```json
{
  "schema_version": 1,
  "data": {
    "stint_id": "01...",
    "state": "awaiting-user",
    "generation": 4,
    "repository": {"path": "/repo", "source": "create"},
    "run_refs": [{"run_id": "01...", "role": "round-worker"}],
    "checkpoint": {
      "presented_at": "2026-08-24T12:00:00Z",
      "items": [{"id": "q1", "kind": "decision", "prompt": "..."}]
    },
    "answers_recorded": false,
    "next_actions": ["record answers or leave checkpoint open"]
  },
  "warnings": []
}
```

A mutation would support dry-run and optimistic generation checks. `stint show` would report references and provenance, not recompute issuectl scheduling or claim referenced workers are complete.

### Design properties

- **Ownership:** Taskfleet owns only checkpoint durability and fencing. Skills remain policy/execution clients. issuectl still decides what may run; Taskfleet runs still decide worker outcomes.
- **Durable state:** append-only or crash-safe checkpoint transitions under `$TASKFLEET_HOME`, with a generation token preventing stale answers. The record stores references and snapshots of what was presented, not a copied schedule.
- **Wake/resume:** a neutral `stint wait <id>` or event tail could settle on `awaiting-user`, `ready-next`, or an explicit blocked state. A pi adapter, generic runner, cron job, or human can consume the same JSON. Wake delivery remains outside the state record.
- **Policy precedence:** the checkpoint may record the effective decision and its sources, but it should not become a policy engine in the first slice. Hard engine invariants cannot be relaxed. User config can cap automation and provide neutral defaults; repository policy can further restrict and supply mechanics but cannot exceed the user cap. Explicit command flags can restrict a run further, not grant forbidden authority. Prompt text never overrides structured gates.
- **No-pi operation:** first-class. The user or any agent can `show`, record answers, acknowledge, and rerun the skill manually.
- **Loop bounds:** machine-enforceable generation transitions and per-wake budgets. `awaiting-user` cannot transition to `ready-next` without answers plus explicit acknowledgement; `ready-next` cannot silently create work.
- **Failure recovery:** resume from the durable generation. A lost wake is harmless. Worker recovery still uses existing commands. Reconciliation detects referenced runs that remain live, awaiting input, stalled, or preserved and refuses finalization.
- **Migration burden:** medium to high: a new schema, CLI surface, event/storage tests, config inspection, skill updates, and migration/doctor behavior. Care is needed not to turn checkpoint references into a new parent/child orchestration graph.
- **Human-controlled:** checkpoint answers and final acknowledgement; lane-or-close disposition; any action prohibited or not positively authorized by effective policy; preserved-work choices.

### Trade-off

This most directly satisfies durable pause and exact acknowledgement. It also creates the greatest architectural temptation: once Taskfleet has a `stint` noun, adding scheduling, retries, deployment, and policy evaluation there could recreate the broad orchestration surface rejected by the thin-supervisor ADR. A viable design needs a strict negative capability list and tests proving the checkpoint cannot mutate run or issue truth.

## Option C — External harness-neutral lifecycle runner

### Shape

Build a small separate runner around Taskfleet and issuectl's public contracts. It owns durable **job/checkpoint metadata** and bounded wake delivery. The pi integration is one adapter for injecting a turn; a headless CLI driver is another. The runner invokes skills or agent turns, but it never imports Taskfleet internals or the telemetry adapter's manager/events.

Neutral runner contract:

```text
start / status / bounded logs / stop / bounded wait
```

Possible lifecycle record:

```json
{
  "schema_version": 1,
  "job_id": "stint-01...",
  "phase": "awaiting-user",
  "wake_budget": {"used": 2, "max": 3},
  "taskfleet_runs": ["01..."],
  "checkpoint_ref": "checkpoints/4.json",
  "last_result": {"kind": "turn-settled", "authoritative": false}
}
```

Example journey:

> A shell user starts a lifecycle job. The runner invokes an agent turn, then waits on `taskfleet run wait` and issue/TODO checks. When the condition settles, it invokes one more turn with a bounded context envelope. That turn prepares the product-owner checkpoint. The runner stops in `awaiting-user`. Jari answers through pi or a CLI file; an explicit resume starts finalization.

### Design properties

- **Ownership:** the external runner owns job liveness, logs, wake budgets, and checkpoint delivery. Taskfleet owns runs; issuectl owns scheduling; skills own workflow semantics.
- **Durable state:** runner-owned job and checkpoint files, with Taskfleet run IDs and repository identity as references. It must label telemetry and turn-settled signals as advisory.
- **Wake/resume:** strongest wake story. The runner waits on public `run wait`, event tail, or its own child process completion and injects a bounded next turn. pi is an adapter, not a prerequisite.
- **Policy precedence:** the runner passes policy context but cannot grant authority. User runner config sets ceilings and prompt paths; repository `AGENTS.md` supplies project policy; the invoked skill computes the intersection and reports provenance. Conflicts stop before side effects.
- **No-pi operation:** supported through the runner's CLI and any compatible harness. A plain manual mode can inspect status and resume with an answers file.
- **Loop bounds:** enforced by the runner: maximum wakes, wall-clock budget, one active turn, one outstanding checkpoint, and terminal stop reasons. No recursive agent self-spawn.
- **Failure recovery:** restart the runner and resume from job metadata. A killed adapter does not affect Taskfleet runs. Lost notifications are recovered by `status`/`wait`; duplicate wakes are fenced by checkpoint generation and single-flight leases.
- **Migration burden:** medium, but in another owning repository/package. It requires a stable neutral contract, packaging, isolated tests, and clear compatibility checks. It avoids a Taskfleet schema migration.
- **Human-controlled:** resume after a checkpoint, semantic decisions, final acknowledgement, and policy exceptions. `stop` must never be translated into Taskfleet cancellation unless the human separately requests that explicit action.

### Trade-off

This cleanly separates durable running from pi and keeps Taskfleet thin. It introduces another product and state root, however. Users must understand whether they are looking at runner-job state, Taskfleet run state, issue scheduling, or narrative handoff. The adapter therefore needs excellent provenance and “next action” output, and its repository ownership must be explicit before implementation.

## Option D — Minimal friction removal

### Shape

Do not add durable stint state or an external runner. Make two small workflow changes:

1. a combined entry skill runs `stint-start` through the product-owner report and emits one precise next command based on the observed stop reason; and
2. a combined finish skill records answers supplied in the current turn, runs `stint-handoff` and `wrap-up`, and verifies cleanliness.

Optionally, the start skill launches a bounded background `run wait` when the current harness supports wake-on-exit. If it does not, it prints a copyable resume invocation and the relevant run IDs. There is no promise of automatic wake.

Example:

> “Run a stint.” Work completes and the response ends with either “Reply to these two decisions, then run `/stint-finish`” or “No decisions remain; run `/stint-finish --acknowledge`.” If a worker remains live, it says exactly which public `run wait` command will settle. A later session can rerun the entry skill, but there is no durable session checkpoint.

### Design properties

- **Ownership:** unchanged; only skill ergonomics improve.
- **Durable state:** existing Taskfleet, issue, git, and `TODO.md` state only.
- **Wake/resume:** opportunistic background wait where supported; otherwise explicit user invocation.
- **Policy precedence:** unchanged from current skills. A tiny user config could enable automatic handoff, but repository policy and engine invariants still win; absence defaults to off.
- **No-pi operation:** complete but manual. Every proposed command works without pi-specific hooks.
- **Loop bounds:** one round and one finish attempt. No feedback mini-round is automatic.
- **Failure recovery:** rerun the relevant combined skill; it reconstructs from current sources and prints blockers.
- **Migration burden:** very low; mostly bundled-skill edits and snapshots.
- **Human-controlled:** the same controls as today, including the explicit transition into terminal handoff.

### Trade-off

This may remove most day-to-day annoyance for little risk and provides evidence about which missing automation actually matters. It does not meet the strongest reading of “durably awaiting user” or automatic feedback rounds. It is best treated either as an experiment or as a first slice that remains useful under another option.

## Policy layering common to all options

A safe effective-policy model should distinguish **mechanics**, **authority ceilings**, and **supplementary prose**:

1. **Taskfleet invariants and command preconditions** are non-overridable.
2. **Explicit invocation restrictions** can make this invocation safer or narrower.
3. **User-owned structured config** under `$TASKFLEET_HOME` sets automation ceilings and conveniences: auto-handoff, wake/round budgets, a private prompt-file path, and whether classes of actions are allowed, checkpointed, or forbidden.
4. **Repository `AGENTS.md`** supplies project-specific commands and policy. It may tighten the user ceiling. It cannot make an action autonomous when user config forbids it, and silence cannot grant authority.
5. **`TODO.md` and free-form prompt files** provide intent and context only. They cannot override structured prohibitions, invent a command, or waive a gate.

For an action such as deploy, an explainable result might be:

```json
{
  "action": "deploy",
  "effective": "checkpoint",
  "sources": [
    {"source": "user-config", "value": "allowed"},
    {"source": "repository-policy", "value": "checkpoint"}
  ],
  "reason": "most restrictive applicable policy wins"
}
```

The hard question is that `AGENTS.md` is prose, not a typed config. An initial design should avoid pretending Taskfleet can mechanically parse all repository policy. The executing skill can report a sourced interpretation and fail closed on ambiguity. A future structured repository policy would need its own explicit decision rather than silently moving policy into `.taskfleet.toml`.

## Promising hybrids

- **D then B:** remove manual chaining first, observe actual friction, then add only the checkpoint fields proven necessary.
- **B plus C:** Taskfleet stores a tiny authoritative checkpoint generation; an external runner owns wake delivery and bounded turns. This gives durable acknowledgement without making Taskfleet a harness runner.
- **A plus C:** keep workflow semantics entirely in skills while an external runner contributes only durable start/status/logs/stop/wait and wake delivery.
- **D plus a checkpoint file:** a repository-neutral, user-home checkpoint written by the skill could test durability before committing to a public Taskfleet noun. The risk is creating an undocumented quasi-API with weaker locking and migration behavior.

No hybrid should use worker telemetry as a lifecycle transition, copy the issue DAG, or let “agent settled” mean “round complete.”

## Comparison

| Option | Durable session checkpoint | Automatic wake | Taskfleet change | No-pi parity | Enforced loop bound | Main risk |
|---|---|---|---|---|---|---|
| A. Skill-only chaining | Weak; only existing artifacts | Optional/session-dependent | None | Strong | Mostly prose | checkpoint lost with context |
| B. Thin Taskfleet checkpoint | Strong and fenced | Via neutral wait/event consumer | Medium/high | Strong | Strong | grows into forbidden orchestration |
| C. External lifecycle runner | Strong in runner state | Strong | None or tiny contract additions | Strong | Strong | another state root/product to operate |
| D. Minimal friction removal | None beyond current state | Opportunistic | None | Strong but manual | Strong and simple | does not satisfy durable pause |

| Dimension | A | B | C | D |
|---|---|---|---|---|
| Recovery after lost conversation | reconstruct | exact checkpoint resume | exact runner-job resume | reconstruct |
| Policy provenance potential | narrative | structured checkpoint view | structured runner view | narrative |
| Migration burden | low | highest | medium, external | lowest |
| Human acknowledgement | conversational | durable generation-fenced mutation | durable runner checkpoint | explicit finish invocation |
| Broad-orchestrator pressure | low | highest | medium, outside Taskfleet | very low |

## Open questions for Jari

1. Is the essential durable object the **whole stint**, or only the **user checkpoint and its acknowledgement**?
2. After a fully green round with no questions, should automatic handoff run immediately, or should there still be one explicit “finish this stint” acknowledgement?
3. Should “ready to test” items count as a checkpoint even when no design decision is needed?
4. May one user response trigger one automatic feedback round, or should every new implementation round require an explicit continue?
5. Is a separate neutral runner acceptable operationally, or is one Taskfleet state root materially more important than keeping Taskfleet's surface small?
6. Which actions should user config be allowed to cap: commit, pull/rebase, push, deploy, release, handoff, and issue mutation? Which should never be globally pre-authorized?
7. When repository prose and user config disagree, is “most restrictive wins” sufficient, or should conflict itself always require a checkpoint?
8. Where should checkpoint answers be retained long-term: in the checkpoint record, copied into the relevant issue/TODO narrative during finalization, or both with one canonical owner?
9. What should happen when handoff discovers a still-live or preserved worker: remain `preparing-handoff`, become a distinct blocked checkpoint, or finish the narrative but leave the stint unacknowledgeable?
10. Does Jari want pi kept alive between checkpoints, or is a durable notification plus a fresh session preferable once context is large?

## Suggested narrowing order

1. **Walk through three real examples first:** a green no-question round, a two-question test/decision round, and a failed worker with preserved work. Decide exactly where an automatic chain must stop in each.
2. **Choose the durable boundary:** no new checkpoint, checkpoint-only Taskfleet state, or external runner state. Defer detailed command naming until this ownership choice is made.
3. **Decide acknowledgement semantics and loop budgets:** what input is required, what a stale answer does, and whether one feedback round may auto-start.
4. **Set the policy intersection:** identify user ceilings, repository-owned mechanics, safe defaults, and the effective-policy explanation expected from `config show --json` or its equivalent.
5. **Prototype the smallest journey at skill level:** even if B or C is favored, use D/A-style chaining to validate the product flow without committing to storage architecture.
6. **Only then make an architectural decision:** compare the observed need for durable generation fencing and wake reliability against the cost of a Taskfleet noun or a separate runner. If the decision changes Taskfleet's architectural boundary, record it explicitly before implementation issues are scheduled.
