---
created: 2026-10-15
updated: 2026-10-15
type: feature
reporter: jari
status: in-progress
priority: normal
provenance: chat
source_ref: chat:2026-10-13/claude-headless-harness
lane: harness
lane_seq: 10
---

# Run Claude Code as an autonomous headless harness

## Description

# Run Claude Code as an autonomous headless worker

## Goal
Support Claude Code as a selectable autonomous Taskfleet harness, including headless spinoff runs that can commit, call `run merge`, report exit correctly, retain work and capture usable evidence. Pi must keep working unchanged.

## Current constraint
Today `harness::profile` rejects Claude for autonomous lifecycle; configuration rejects Claude `worker-v1` telemetry; launcher and evidence capture assume Pi native sessions. Interactive Claude support is not an autonomous run. Do not simulate Pi's native session format or infer done from Claude's exit. Use the existing told merge/worker-exit contract and a truthful Claude capability/telemetry/evidence policy, with explicit unsupported results where necessary. A profile must remain user-owned; repository config cannot define argv.

## Acceptance
A user can choose a Claude profile for a normal headless run, and a disposable-repository end-to-end test drives a real-shaped Claude stub from create through report/merge/teardown (and a failed exit preserving work). CLI validation, selection snapshots, docs and skills agree on the supported capabilities and output. No real installed Claude or production taskfleet state is modified. If Claude needs a separate adapter for telemetry, implement it or explicitly scope safe evidence and surface absent telemetry honestly, not by labelling it `worker-v1` without protocol support.
