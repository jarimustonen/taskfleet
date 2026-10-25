---
created: 2026-10-25
updated: 2026-10-25
type: bug
reporter: jari
status: untriaged
priority: normal
provenance: agent:homebase-wrapup
source_ref: agent:homebase-wrapup/reporter:jari/id:oh-conductor-2026-10-25-await-text
---

# run wait does not flag a Claude worker waiting on a plain-text question

## Description

run wait does not flag a Claude worker waiting on a plain-text question

Observed 2026-10-25 (open-habitat conductor, run 01m5rd6959p12f9eq8p8dankmp, profile prototype-opus, Claude harness, interactive headless).

The worker finished a step and ended its turn with a plain-text question to the caller ("Either add 216.128.7.99/32 to the pilot's firewall yourself, or tell me to add it ..."), then sat idle at the prompt for ~30 minutes.

`taskfleet run wait <id> --check-in 45m --output json` and `run show` kept reporting status `pending`, `awaiting_input: false`, `attention_required: false`. Only Claude permission prompts (Notification hook, matcher permission_prompt) set awaiting_input; a Stop hook with the worker idle at an empty prompt and no report/merge is not surfaced.

Expected: an idle Claude worker that ended its turn without `run merge`/`node report` (Stop hook fired, no further tool use) is surfaced as awaiting input / attention after a short grace period, so the conductor's wait settles instead of waiting for the next check-in.

Reported by jari via agent:homebase-wrapup.
