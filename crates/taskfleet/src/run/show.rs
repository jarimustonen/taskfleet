//! `run show` — full manifest + counters for one run.

use serde::Serialize;
use serde_json::Value;

use taskfleet_core::{read_manifest_opt, read_node_opt, NodeId, RunLock, Status};

use crate::error::CliError;
use crate::output::{self, OutputFormat, OutputSpec};
use crate::run::dto::{ManifestView, SupervisorState, SupervisorView};
use crate::run::stalled::StallKind;
use crate::run::{from_core, run_paths_from_cli_arg};

/// The single reporting node of a single-worker worktree run (`n-0001`);
/// mirrors `run merge` / `run wait`'s `DEFAULT_NODE_ID`. Its terminal report is
/// where a supervisor stamps the `recoverable_work` stranded-work signal.
const DEFAULT_NODE_ID: &str = "n-0001";

#[derive(Serialize)]
struct ShowPayload<'a> {
    /// The run-list-shaped summary row, flattened to the TOP level of `data`
    /// (`data.run_id`, `data.kind`, `data.status`, `data.title`,
    /// `data.created_at`, `data.node_count`, `data.supervisor`, `data.stalled`).
    /// This is the *same* shape a `run list` row carries, so a consumer can
    /// address a run's identity + liveness the same way across both verbs
    /// (issue `run-show-json-null-fields`: the reported all-null payload came
    /// from re-using `run list`'s flat field layout on `run show`, whose fields
    /// were reachable only under `data.manifest`). `manifest` below keeps the
    /// full nested detail for existing consumers that poll `data.manifest.*`.
    #[serde(flatten)]
    summary: crate::run::dto::RunSummary,
    manifest: ManifestView<'a>,
    counts: Counts,
    /// Per-node last-told activity and freshness. Observational only: every row
    /// explicitly leaves the run status unchanged.
    telemetry: Vec<crate::run::telemetry::NodeTelemetryView>,
    /// Rebase-robust landing signal for the reporting node: true when the
    /// worker's committed work has landed in the target, confirmed by patch-id
    /// equivalence against the *current* target tip (`git cherry`) — NOT by
    /// branch-ref ancestry, which a caller-side `git rebase` invalidates. Falls
    /// back to the durable merge marker when git verification is unavailable
    /// (issue `landing-signal-reliable-after-rebase`). Callers should trust this
    /// instead of running `git merge-base --is-ancestor` by hand.
    landed: bool,
    /// How `landed` was decided: `git-verified` | `report-marker` | `unverified`.
    landed_method: &'static str,
    /// Terminal report from the default worker node. This is a read-only
    /// convenience surface for callers that otherwise need `node show`; the
    /// projection-native `node show` field remains `last_report`.
    report: Option<Value>,
    /// Native Pi transcript/session and final terminal evidence for the worker.
    /// Archived artifact paths are run-relative; the pending live-session path
    /// is the exact run-bound private source. `status` is explicit
    /// (`pending|failed|complete`).
    #[serde(skip_serializing_if = "Option::is_none")]
    evidence: Option<taskfleet_core::WorkerEvidence>,
    /// The `recoverable_work` block from the default node's terminal report,
    /// present only when a dead agent left unmerged commits ahead of source
    /// (issue `agent-death-strands-recoverable-work`). Surfaced so a caller can
    /// spot salvageable work on a `failed` run without inspecting the node
    /// projection or running `git log <source>..<branch>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    recoverable_work: Option<Value>,
    /// Current retained worktree/branch inventory for failed or cancelled nodes.
    /// Always present and never inferred from historical cleanup events.
    preserved_work: Vec<crate::run::retained::PreservedWork>,
    /// Suspected *false-failed* run (issue `raw-git-selfmerge-false-failed`):
    /// present only when the run is `failed` yet git confirms the worker's
    /// content is already in source and no `run merge` recorded it — the raw-git
    /// self-merge-then-death tradeoff. A non-mutating hint pointing at
    /// `run salvage`; NEVER an auto-success. `None` on any other run. See
    /// [`crate::run::false_failed`].
    #[serde(skip_serializing_if = "Option::is_none")]
    false_failed: Option<crate::run::false_failed::FalseFailedView>,
    // `landed`/`landed_method`/`recoverable_work` are computed detail unique to
    // `run show`. The run's `stalled` hint (see [`crate::run::stalled`]) and the
    // `supervisor` liveness probe live on the flattened `summary` row above, so
    // they read the same way as a `run list` row (issue `run-show-json-null-fields`).
}

/// The run/node fields read under the shared lock to compute `landed` after the
/// lock is released (the git shell-out must not run while the flock is held).
struct LandingFields {
    source_repo: Option<String>,
    source_branch: Option<String>,
    worktree_path: Option<String>,
    branch: Option<String>,
    base_sha: Option<String>,
    report: Option<Value>,
    evidence: Option<taskfleet_core::WorkerEvidence>,
}

#[derive(Serialize)]
struct Counts {
    nodes: u64,
}

pub fn run(run_id: &str, spec: &OutputSpec, warnings: &[String]) -> Result<(), CliError> {
    let root = crate::home::root_dir()?;
    let paths = run_paths_from_cli_arg(&root, run_id)?;
    // Hold the run's shared lock for the whole manifest + projection-dir scan so
    // a concurrent reducer cannot leave us with a manifest counter that
    // disagrees with the projection files we count (design.md §4). The lock is
    // released before any output is formatted.
    let scanned = RunLock::with_shared_lock(&paths.lock(), || {
        let Some(manifest) = read_manifest_opt(&paths)? else {
            return Ok(None);
        };
        let nodes =
            crate::run::retained::read_nodes(&paths, crate::run::retained::MissingNodesDir::Empty)?;
        let counts = Counts {
            nodes: nodes.len() as u64,
        };
        // Probe supervisor liveness INSIDE the shared-lock window so it is read
        // in the same critical section as `manifest.status`, letting a caller
        // reason "status pending + supervisor dead => orphaned" off one scan.
        // Caveat: this is NOT a transactionally consistent pairing. The shared
        // lock only serializes against cooperating flock holders — the reducer's
        // manifest/counter/projection writes (invariant 3). The supervisor's pid
        // file is written under the exclusive lock (`claim_pid_atomic`) but
        // *removed* without it (`pid_file::remove_if_owner`), and involuntary
        // process death is unsynchronized entirely, so the liveness bit is a
        // best-effort point-in-time hint, not a value welded to `status`. Read it
        // here anyway so the hint is as fresh as the manifest it sits beside.
        let supervisor = SupervisorView::probe(&paths);
        // Read the reporting node ONCE inside the shared-lock window, alongside
        // the manifest/counters, so the `landed` git-verification inputs and the
        // `recoverable_work` signal are a single consistent snapshot with
        // `manifest.status` (state-integrity invariant 3). The git shell-out that
        // turns these inputs into `landed` runs AFTER the lock is released — never
        // hold the flock across a subprocess.
        let node_id =
            NodeId::parse_str(DEFAULT_NODE_ID).expect("DEFAULT_NODE_ID is a valid node id");
        let node = read_node_opt(&paths, &node_id)?;
        // `recoverable_work` is gated on a `failed` status: the supervisor only
        // stamps the block on the failed-synthesis path, so a block on a
        // non-failed report is stale/spoofed (unknown report fields are permitted
        // by the validator) and is not surfaced. A run with no `n-0001` node (or
        // no such block) simply yields `None`.
        let recoverable_work = if matches!(manifest.status, Status::Failed) {
            node.as_ref()
                .and_then(|n| n.last_report.as_ref())
                .and_then(|r| r.get("recoverable_work"))
                .filter(|v| v.is_object())
                .cloned()
        } else {
            None
        };
        // Supervisor-death stall verdict over the same shared-lock snapshot —
        // `manifest.status` + the supervisor liveness probe above + the manifest
        // counters/timestamps. `Stillborn` = died before creating `n-0001`
        // (issue `run-wait-stillborn-run-not-detected`); `Orphaned` = died
        // mid-run with ≥1 node, idle past the grace (issue `run-wait-still`).
        // (The 0.2 cut removed the orchestrate-driver "never driven" stall shape
        // along with the `orchestrate` kind.)
        let now = chrono::Utc::now();
        let stall = crate::run::stalled::stall_kind(
            manifest.status,
            // Indeterminate (`Unreadable`/`Unknown`) supervisor states must not
            // drive a stillborn/orphaned verdict — see `presumed_working`.
            supervisor.presumed_working(),
            manifest.node_count,
            manifest.created_at,
            manifest.updated_at,
            now,
        );
        let stalled = stall.is_some();
        // Idle minutes for the human message, only meaningful when stalled. Both
        // supervisor-death shapes clock idle from `manifest.updated_at` (they
        // have no live driver node to read).
        // Clamp to 0: clock skew or a future timestamp must never print a
        // negative "idle -3 min" in the human hint.
        let stalled_idle_min = if stall.is_some() {
            Some(
                now.signed_duration_since(manifest.updated_at)
                    .num_minutes()
                    .max(0),
            )
        } else {
            None
        };
        // Attention-required (design.md §2.5 / A5): the reporting node's worker
        // exited cleanly but the node is still non-terminal — it skipped
        // `run merge`. A durable told fact (`node.worker_exit`), read here in the
        // same shared-lock snapshot as `manifest.status`, so the verdict and the
        // status it explains cannot straddle a reducer write. Checked
        // independently of the stall shapes: a clean-exited worker is
        // attention-required even if its supervisor later died (the manual finish,
        // not `run reattach`, is the fix — see `crate::run::attention`).
        // Attention only applies to a SINGLE-worker run: a fan-out (multi-node)
        // run's per-node attention is the delegated `per-node-run` follow-up, so
        // don't false-flag the whole run off `n-0001` alone (design.md §2.5).
        let attention = if manifest.node_count == 1 {
            node.as_ref().and_then(|n| {
                crate::run::attention::is_attention_required(n.status, n.worker_exit.as_ref())
                    // `is_attention_required` guarantees `worker_exit` is Some+clean.
                    .then_some(n.worker_exit.as_ref())
                    .flatten()
                    .map(|exit| {
                        crate::run::attention::AttentionView::build(
                            manifest.run_id.as_str(),
                            now,
                            exit,
                            n.agent_pid,
                            n.worktree_path.clone(),
                            manifest.source_branch.clone(),
                        )
                    })
            })
        } else {
            None
        };
        let awaiting_input = node
            .as_ref()
            .and_then(|n| n.awaiting_input.as_ref())
            .map(|open| crate::run::awaiting_input::AwaitingInputView::build(open, now));
        let landing = LandingFields {
            source_repo: manifest.source_repo.clone(),
            source_branch: manifest.source_branch.clone(),
            worktree_path: node.as_ref().and_then(|n| n.worktree_path.clone()),
            branch: node.as_ref().and_then(|n| n.branch.clone()),
            base_sha: node.as_ref().and_then(|n| n.base_sha.clone()),
            report: node.as_ref().and_then(|n| n.last_report.clone()),
            evidence: node.and_then(|n| n.evidence),
        };
        Ok(Some((
            manifest,
            counts,
            supervisor,
            recoverable_work,
            stalled,
            stall,
            stalled_idle_min,
            attention,
            awaiting_input,
            landing,
            nodes,
        )))
    })
    .map_err(from_core)?;
    let (
        manifest,
        counts,
        supervisor,
        recoverable_work,
        stalled,
        stall,
        stalled_idle_min,
        attention,
        awaiting_input,
        landing,
        nodes,
    ) = match scanned {
        Some(v) => v,
        None => {
            return Err(
                CliError::user("run_not_found", format!("no run with id {run_id}"))
                    .with_invalid_value(run_id),
            );
        }
    };
    let git = crate::git::repo::Git::with_bin(crate::supervise::cleanup::git_bin());
    let mut preserved_work: Vec<_> = nodes
        .iter()
        .filter_map(|node| crate::run::retained::observe(&manifest, node, &git))
        .map(|observation| observation.view)
        .collect();
    preserved_work.sort_by(|a, b| a.node_id.cmp(&b.node_id));

    // Telemetry gets its own complete shared-lock scan. It is deliberately
    // outside the canonical status/attention/outcome tuple above: this module
    // can enrich output but cannot become an input to those decisions.
    let (telemetry, telemetry_warning) = crate::run::telemetry::read_views(&paths, &manifest);
    let mut output_warnings = warnings.to_vec();
    if let Some(error) = telemetry_warning.as_deref() {
        output_warnings.push(format!(
            "telemetry unavailable for run {}: {error}; run status unchanged",
            manifest.run_id
        ));
    }
    // Git-verified `landed` (issue `landing-signal-reliable-after-rebase`),
    // computed outside the shared lock: the rebase-robust signal a caller should
    // trust instead of hand-rolling `git merge-base --is-ancestor`.
    let signal = crate::run::landed::landing_signal(
        &crate::run::landed::LandingInputs {
            source_repo: landing.source_repo.as_deref(),
            source_branch: landing.source_branch.as_deref(),
            worktree_path: landing.worktree_path.as_deref(),
            branch: landing.branch.as_deref(),
            base_sha: landing.base_sha.as_deref(),
            report: landing.report.as_ref(),
        },
        &crate::supervise::cleanup::git_bin(),
    );
    // Suspected false-failed (issue `raw-git-selfmerge-false-failed`): a `failed`
    // run whose worker content git CONFIRMS is in source, with no `run merge` on
    // record — the raw-git self-merge-then-death tradeoff. Computed off the same
    // git-verified `landed` signal + the terminal report; a non-mutating hint,
    // never an auto-success (the removed git-reconcile-implies-done probe,
    // invariant 7). Gated on a single-worker run so a fan-out driver node's
    // `n-0001` landing never false-flags the whole run (mirrors `attention`).
    let false_failed = (manifest.node_count == 1
        && crate::run::false_failed::is_false_failed_suspected(
            matches!(manifest.status, Status::Failed),
            signal.landed,
            signal.method,
            landing.base_sha.is_some(),
            landing.report.as_ref(),
        ))
    .then(|| crate::run::false_failed::FalseFailedView::build(manifest.run_id.as_str()));
    // The flattened top-level row: same shape as a `run list` row, so
    // `data.supervisor` / `data.status` / `data.kind` read identically across
    // both verbs (issue `run-show-json-null-fields`). `manifest` keeps the full
    // nested detail alongside it.
    let summary = crate::run::dto::RunSummary::from(&manifest)
        .with_worktree_path(landing.worktree_path.clone())
        .with_supervisor(supervisor)
        .with_stalled(stalled)
        .with_stillborn(matches!(
            &stall,
            Some(crate::run::stalled::StallKind::Stillborn)
        ))
        .with_attention(attention)
        .with_awaiting_input(awaiting_input)
        .with_telemetry_counts(
            crate::run::telemetry::counts(&telemetry),
            telemetry_warning.is_none(),
        );
    let payload = ShowPayload {
        summary,
        manifest: ManifestView::from(&manifest),
        counts,
        telemetry,
        landed: signal.landed,
        landed_method: signal.method.wire(),
        // A top-level run report has one unambiguous meaning only for a
        // single-worker run. Multi-node runs expose each worker through
        // `node show`; never present n-0001 as the whole run's report.
        report: (manifest.node_count == 1)
            .then(|| landing.report.clone())
            .flatten(),
        evidence: (manifest.node_count == 1)
            .then(|| landing.evidence.clone())
            .flatten(),
        recoverable_work,
        preserved_work,
        false_failed,
    };
    match spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => {
            output::emit_envelope(&payload, spec, &output_warnings)?;
        }
        OutputFormat::Text => {
            println!("run-id:        {}", payload.manifest.run_id);
            println!(
                "title:         {}",
                output::escape_one_line(payload.manifest.title)
            );
            println!("status:        {}", payload.manifest.status);
            if payload.summary.stalled {
                let idle =
                    stalled_idle_min.map_or_else(String::new, |m| format!(" (idle {m} min)"));
                match stall {
                    Some(StallKind::Stillborn) => println!(
                        "stalled:       true — stillborn run: supervisor died before creating any \
                         worker node (pending, 0 nodes, no progress since creation{idle}). \
                         The run cannot progress on its own; `run reattach {run_id}` to recover it, \
                         or `run cancel {run_id}` to lay it to rest.",
                        run_id = payload.manifest.run_id
                    ),
                    Some(StallKind::Orphaned) => println!(
                        "stalled:       true — orphaned run: supervisor died mid-run, leaving work \
                         stranded ({status}, {nodes} node(s), idle past the grace window{idle}). \
                         No actor can roll it up; `run reattach {run_id}` to revive the supervisor \
                         (it then rolls the run up or fails it), or `run cancel {run_id}`.",
                        status = payload.manifest.status,
                        nodes = payload.manifest.node_count,
                        run_id = payload.manifest.run_id
                    ),
                    None => println!(
                        "stalled:       true — orchestrate driver looks undriven: pending, no children{idle}. \
                         Verify no orchestrator agent is still driving this run before acting; \
                         if none is, `run cancel {}` and relaunch with an active orchestrator.",
                        payload.manifest.run_id
                    ),
                }
            }
            if let Some(awaiting) = &payload.summary.awaiting_input_detail {
                println!(
                    "awaiting-input: true — {} open decision(s), idle {} min, escalated={}",
                    awaiting.open_discussion_count,
                    awaiting.pending_age_secs / 60,
                    awaiting.escalated
                );
                for item in &awaiting.discussion_items {
                    let topic = item.get("topic").and_then(Value::as_str).unwrap_or("?");
                    let default = item
                        .get("recommended_default")
                        .and_then(Value::as_str)
                        .unwrap_or("?");
                    println!("  decision: {topic} (recommended default: {default})");
                }
            }
            if let Some(att) = &payload.summary.attention {
                // Non-terminal, deliberate: the worker finished but skipped
                // `run merge`. Print the resume context a PO needs to find and
                // finish the worktree (design.md §2.5). NOT a stall — no
                // `run reattach`.
                let idle = att.pending_age_secs / 60;
                let pid = att
                    .worker_pid
                    .map_or_else(|| "?".to_string(), |p| p.to_string());
                let wt = att.worktree_path.as_deref().unwrap_or("?");
                println!(
                    "attention:     true — {reason} (worker pid {pid} exited, node non-terminal, \
                     idle {idle} min). worktree {wt}. {hint}",
                    reason = att.reason,
                    hint = att.resume_hint,
                );
            }
            println!(
                "landed:        {} ({})",
                payload.landed, payload.landed_method
            );
            if let Some(evidence) = &payload.evidence {
                println!(
                    "evidence:      {} session={} transcript={}",
                    evidence.status,
                    evidence.session_id,
                    evidence
                        .transcript_path
                        .as_deref()
                        .unwrap_or(match evidence.status {
                            taskfleet_core::EvidenceStatus::Failed => "(failed)",
                            _ => "(pending)",
                        })
                );
                if let Some(error) = evidence.error.as_deref() {
                    println!("evidence-error: {}", output::escape_one_line(error));
                }
            }
            if let Some(ff) = &payload.false_failed {
                // Non-terminal-changing hint: the run is `failed` but its content
                // is git-verified in source with no `run merge` recorded (raw-git
                // self-merge then death). Never auto-success — steer to salvage.
                println!("false-failed:  true — {}. {}", ff.reason, ff.resume_hint);
            }
            println!("kind:          {}", payload.manifest.kind);
            println!("lifecycle:     {}", payload.manifest.lifecycle);
            match &payload.manifest.selection {
                crate::run::dto::AgentSelectionView::Recorded(selection) => {
                    println!(
                        "profile:       {} (source {}, {})",
                        selection.profile, selection.selection_source, selection.interaction
                    );
                    if let Some(requested) = selection.requested_harness.as_deref() {
                        println!("requested-harness: {requested}");
                    }
                    println!(
                        "agent:         candidate {} / {}",
                        selection.selected.candidate_index, selection.selected.harness
                    );
                    for skipped in &selection.fallback {
                        println!(
                            "skipped:       candidate {} / {} — {}",
                            skipped.candidate_index, skipped.harness, skipped.reason
                        );
                    }
                }
                crate::run::dto::AgentSelectionView::Legacy(value) => {
                    println!("profile:       {value}");
                }
            }
            println!("created_at:    {}", payload.manifest.created_at);
            println!("updated_at:    {}", payload.manifest.updated_at);
            println!("nodes:         {}", payload.counts.nodes);
            for telemetry in &payload.telemetry {
                println!(
                    "telemetry:     {}",
                    crate::run::telemetry::text_line(telemetry)
                );
            }
            match payload.summary.supervisor.state {
                SupervisorState::Alive => match payload.summary.supervisor.pid {
                    Some(pid) => println!("supervisor:    pid {pid} (alive)"),
                    None => println!("supervisor:    (alive)"),
                },
                SupervisorState::Dead => match payload.summary.supervisor.pid {
                    Some(pid) => println!(
                        "supervisor:    pid {pid} (dead — run `taskfleet run reattach {}` to recover)",
                        payload.manifest.run_id
                    ),
                    None => println!("supervisor:    (dead)"),
                },
                SupervisorState::NotRecorded => println!("supervisor:    (none recorded)"),
                SupervisorState::Unreadable => {
                    println!("supervisor:    (pid file present but unreadable — inspect supervisor.pid under the run directory)");
                }
                SupervisorState::Unknown => println!("supervisor:    (not probed)"),
            }
            if let Some(line) =
                crate::run::wait::recoverable_summary(payload.recoverable_work.as_ref())
            {
                println!("recoverable:   {line}");
            }
            for preserved in &payload.preserved_work {
                println!(
                    "preserved:     {} worktree={} ({}) branch={} ({}) cleanliness={} unmerged={} verification={}",
                    preserved.node_id,
                    preserved.worktree_path.as_deref().unwrap_or("(none)"),
                    if preserved.worktree_present { "present" } else { "absent" },
                    preserved.branch.as_deref().unwrap_or("(none)"),
                    if preserved.branch_present { "present" } else { "absent" },
                    preserved.cleanliness,
                    preserved.unmerged_commits.map_or_else(|| "?".to_string(), |n| n.to_string()),
                    preserved.verification,
                );
            }
            output::emit_text_warnings(&output_warnings);
        }
    }
    Ok(())
}
