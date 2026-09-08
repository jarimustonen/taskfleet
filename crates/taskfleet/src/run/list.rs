//! `run list` — walk `<root>/runs/` and emit a manifest summary per run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use taskfleet_core::{read_manifest_opt, read_node_opt, NodeId, RunLock, RunPaths};

use crate::error::CliError;
use crate::output::{self, OutputFormat, OutputSpec};
use crate::run::dto::{RunSummary, SupervisorState, SupervisorView};
use crate::run::{from_core, runs_root};

/// The single reporting node of a single-worker worktree run (`n-0001`); mirrors
/// `run show` / `run wait`. Its `worker_exit` fact drives the attention-required
/// verdict (design.md §2.5 / A5).
const DEFAULT_NODE_ID: &str = "n-0001";

pub struct Args<'a> {
    pub status: Option<String>,
    pub kind: Option<String>,
    pub repo: Option<PathBuf>,
    pub spec: &'a OutputSpec,
    pub warnings: &'a [String],
}

#[derive(Serialize)]
struct ListPayload {
    runs: Vec<RunSummary>,
}

/// Minimum age (from `manifest.created_at`) a zero-node, no-supervisor run must
/// reach before `run list` flags it `stillborn`.
///
/// `run list` sweeps EVERY run, including ones another process is mid-`run
/// create` on. For a top-level worker the supervisor is spawned only AFTER
/// `node.created`, so during the whole native materialization window (up to
/// `--agent-startup-timeout`, capped at 600s) a perfectly healthy in-flight run
/// transiently presents the exact stillborn shape — pending, 0 nodes, no
/// supervisor pid, `updated_at == created_at`. Gating the flag on an age that
/// comfortably exceeds the max create window keeps a bulk `run list` (e.g. a
/// monitor over `--json`, or the parallel-spawn wave in the incident that filed
/// this) from flagging — or a script from cancelling — a run that is simply
/// still being created.
///
/// `run show` / `run wait` deliberately need NO such gate: they are invoked on a
/// *specific* run whose `run create` already returned, where 0 nodes means
/// native materializer failed (definitively stillborn) — and `run wait` MUST settle
/// promptly, so [`is_stillborn`](crate::run::stalled::is_stillborn) itself stays
/// grace-free (issue `run-wait-stillborn-run-not-detected`, which a grace would
/// re-break). The gate lives here, at the one read surface that sweeps in-flight
/// creates. Matches the supervisor's own no-worker grace (900s). Overridable via
/// [`STILLBORN_LIST_GRACE_ENV`] (tests set `0` to flag immediately).
const STILLBORN_LIST_GRACE_SECS: i64 = 900;

/// Env override for [`STILLBORN_LIST_GRACE_SECS`] (whole seconds; unparseable →
/// default). Tests set `0` to flag a freshly-created stillborn run immediately.
const STILLBORN_LIST_GRACE_ENV: &str = "TASKFLEET_STILLBORN_LIST_GRACE_SECS";

/// The effective stillborn grace, honoring [`STILLBORN_LIST_GRACE_ENV`].
fn stillborn_list_grace() -> chrono::Duration {
    let secs = std::env::var(STILLBORN_LIST_GRACE_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(STILLBORN_LIST_GRACE_SECS);
    chrono::Duration::seconds(secs)
}

pub fn run(args: Args<'_>) -> Result<(), CliError> {
    let root = crate::home::root_dir()?;
    let runs_dir = runs_root(&root);
    // Resolve the selected repository once. Recorded source paths are resolved
    // lazily below and cached by distinct value, so linked worktrees compare by
    // their shared git common-dir without repeating probes for every run.
    let repo_identity = args
        .repo
        .as_deref()
        .map(|repo| repository_identity(repo, true))
        .transpose()?
        .flatten();
    let mut source_identities: HashMap<String, Option<PathBuf>> = HashMap::new();

    // Strict-input rule from AGENTS-AI-FIRST-CLI §1: only validate that
    // the filter values are well-formed strings. We don't reject unknown
    // kinds/statuses up front because a filter that matches nothing is a
    // legitimate empty result — different from a malformed value.
    if let Some(s) = &args.status {
        if s.trim().is_empty() {
            return Err(CliError::user(
                "invalid_value",
                "--status must not be empty",
            ));
        }
    }
    if let Some(k) = &args.kind {
        if k.trim().is_empty() {
            return Err(CliError::user("invalid_value", "--kind must not be empty"));
        }
    }

    // One stall deadline per invocation — every run in this listing is judged
    // against the same instant, so two runs with identical `updated_at` can't
    // disagree on `stalled` due to per-run clock sampling.
    let now = chrono::Utc::now();
    // Minimum age a run must reach before `run list` flags it `stillborn`.
    let stillborn_grace = stillborn_list_grace();

    let mut out: Vec<RunSummary> = Vec::new();
    let mut output_warnings = args.warnings.to_vec();
    let entries = match std::fs::read_dir(&runs_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return emit(out, args.spec, args.warnings);
        }
        Err(e) => {
            return Err(CliError::system(
                "io_error",
                format!("read_dir {}: {}", runs_dir.display(), e),
            ));
        }
    };

    for ent in entries {
        let ent = ent.map_err(|e| CliError::system("io_error", e.to_string()))?;
        if !ent.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        // The directory name must be a valid run id; foreign dirs are skipped.
        let Some(run_id) = ent.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Ok(paths) = RunPaths::new(ent.path(), run_id) else {
            continue;
        };
        // Each run carries its own `.lock`; take that run's shared lock for its
        // manifest read so the summary never reflects a manifest a reducer is
        // mid-rewrite on (design.md §4). A run with no `.lock` yet reads
        // lock-free (see `RunLock::acquire_shared`).
        // Read the manifest AND probe supervisor liveness under the SAME shared
        // lock so `status` and `supervisor` form one consistent snapshot — a
        // caller reasons "status pending + supervisor dead => orphaned", so the
        // pair must not straddle a reducer's status rollup + pid-file removal
        // (see show.rs). Costs one extra pid-file read per run; negligible for
        // realistic run counts.
        // Compute the `stillborn` hint under the SAME shared lock as the manifest
        // so the manifest and the pid probe form one consistent snapshot that
        // cannot straddle a reducer write. Read-only: touches no
        // event/reducer/schema path.
        let scanned = RunLock::with_shared_lock(&paths.lock(), || {
            let Some(m) = read_manifest_opt(&paths)? else {
                return Ok(None);
            };
            let supervisor = SupervisorView::probe(&paths);
            // Stillborn: created but never started — pending, a dead/absent
            // supervisor, zero nodes, and no forward progress since creation
            // (issue `supervisor-dies-before-worker-node`). Kind-agnostic and
            // derived purely from the manifest + the pid probe we already hold,
            // so it costs no extra I/O and is one consistent shared-lock
            // snapshot with `status`. Mirrors the same detector `run show` /
            // `run wait` already use, so a stillborn run is no longer a silent
            // `pending` row here that looks stuck until someone notices.
            //
            // Age-gated (unlike `show`/`wait`): a bulk `run list` sweeps runs
            // that another process is mid-`run create` on, which transiently
            // present the same shape — see [`STILLBORN_LIST_GRACE_SECS`]. Only a
            // run older than the max create window is flagged. The probe is a
            // racy liveness observation the shared lock cannot freeze, so a
            // recovery action must re-verify under an exclusive lock; this flag
            // is advisory.
            let stillborn = crate::run::stalled::is_stillborn(
                m.status,
                // Only a *confirmed* not-running supervisor flags a run stillborn;
                // an `Unreadable`/`Unknown` (indeterminate) state must not, or we
                // recreate the conflation this DTO change fixes.
                supervisor.presumed_working(),
                m.node_count,
                m.created_at,
                m.updated_at,
            ) && now.signed_duration_since(m.created_at) > stillborn_grace;
            // Attention-required (design.md §2.5 / A5): read the reporting node in
            // the SAME shared-lock snapshot as the manifest so the worker-exit fact
            // and `status` cannot straddle a reducer write. Gated on a SINGLE-worker
            // run (`node_count == 1`): a fan-out (multi-node) run's per-node
            // attention is the delegated `per-node-run` follow-up, so we never
            // false-flag the whole run off `n-0001`.
            // Read the default node unconditionally for materialization
            // coordinates. Its projection, not the derived manifest counter, is
            // the source of truth for whether the worktree exists. Best-effort:
            // one corrupt node degrades this diagnostic row rather than aborting
            // the entire listing.
            let node_id =
                NodeId::parse_str(DEFAULT_NODE_ID).expect("DEFAULT_NODE_ID is a valid node id");
            let default_node = read_node_opt(&paths, &node_id).ok().flatten();
            // Keep attention/awaiting-input single-worker scoped. n-0001 is not
            // representative of a multi-node run; widening this gate would be an
            // unrelated behavior change to those diagnostics.
            let diagnostic_node = (m.node_count == 1)
                .then_some(default_node.as_ref())
                .flatten();
            let attention = diagnostic_node.and_then(|n| {
                crate::run::attention::is_attention_required(n.status, n.worker_exit.as_ref())
                    // `is_attention_required` guarantees `worker_exit` is Some+clean.
                    .then_some(n.worker_exit.as_ref())
                    .flatten()
                    .map(|exit| {
                        crate::run::attention::AttentionView::build(
                            m.run_id.as_str(),
                            now,
                            exit,
                            n.agent_pid,
                            n.worktree_path.clone(),
                            m.source_branch.clone(),
                        )
                    })
            });
            let awaiting_input = diagnostic_node
                .and_then(|n| n.awaiting_input.as_ref())
                .map(|open| crate::run::awaiting_input::AwaitingInputView::build(open, now));
            let worktree_path = default_node.as_ref().and_then(|n| n.worktree_path.clone());
            Ok(Some((
                m,
                supervisor,
                stillborn,
                attention,
                awaiting_input,
                worktree_path,
            )))
        })
        .map_err(from_core)?;
        let (m, supervisor, stillborn, attention, awaiting_input, worktree_path) = match scanned {
            Some(v) => v,
            None => continue, // half-initialized run dir; skip silently
        };
        // `stalled` is the "pending but visibly not progressing" hint; since the
        // 0.2 cut removed the orchestrate driver (the only other stall shape),
        // it is now exactly the never-started `stillborn` variant.
        let stalled = stillborn;
        // Shape the manifest into its wire DTO once, then filter on the
        // canonical kebab strings it carries — the DTO's `From` renders
        // `kind` / `status` through the `run/mod.rs` helpers rather than
        // round-tripping the enums through `serde_json::to_value`.
        let summary = RunSummary::from(&m)
            .with_worktree_path(worktree_path)
            .with_supervisor(supervisor)
            .with_stalled(stalled)
            .with_stillborn(stillborn)
            .with_attention(attention)
            .with_awaiting_input(awaiting_input);
        // Filter the exact DTO values while still avoiding telemetry I/O for
        // excluded rows.
        if args
            .status
            .as_ref()
            .is_some_and(|filter| &summary.status != filter)
            || args
                .kind
                .as_ref()
                .is_some_and(|filter| &summary.kind != filter)
        {
            continue;
        }
        if let Some(selected) = repo_identity.as_ref() {
            let Some(source) = summary.source_repo.as_deref() else {
                // Legacy/unrecorded repository identity cannot truthfully match.
                continue;
            };
            let source_identity = if let Some(cached) = source_identities.get(source) {
                cached.clone()
            } else {
                let resolved = repository_identity(Path::new(source), false)?;
                source_identities.insert(source.to_string(), resolved.clone());
                resolved
            };
            if source_identity.as_ref() != Some(selected) {
                continue;
            }
        }
        // Advisory scan failures never suppress the canonical row and never
        // masquerade as invalid samples; availability + an envelope warning
        // preserve the distinction.
        let (telemetry_counts, telemetry_warning) = crate::run::telemetry::read_counts(&paths);
        if let Some(error) = telemetry_warning.as_deref() {
            output_warnings.push(format!(
                "telemetry unavailable for run {}: {error}; run status unchanged",
                m.run_id
            ));
        }
        out.push(summary.with_telemetry_counts(telemetry_counts, telemetry_warning.is_none()));
    }

    out.sort_by_key(|r| std::cmp::Reverse(r.created_at));
    emit(out, args.spec, &output_warnings)
}

/// Resolve a repository's identity as its absolute git common-dir.
///
/// Main and linked worktrees therefore compare equal, while an independent
/// repository nested under a checkout compares different. A caller-supplied
/// selector is an actionable error when invalid. A stale recorded manifest
/// source is instead unknown (`None`) and does not match the filter.
pub(crate) fn repository_identity(
    repo: &Path,
    selector: bool,
) -> Result<Option<PathBuf>, CliError> {
    if repo.as_os_str().is_empty() {
        return if selector {
            Err(CliError::user("invalid_value", "--repo must not be empty"))
        } else {
            Ok(None)
        };
    }
    let output = Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .map_err(|e| CliError::system("dependency_missing", format!("run git: {e}")))?;
    if !output.status.success() {
        if !selector {
            return Ok(None);
        }
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::user(
            "invalid_repository",
            format!(
                "--repo '{}' is not a readable git repository: {}",
                repo.display(),
                detail.trim()
            ),
        )
        .with_invalid_value(repo.display().to_string()));
    }

    let raw = String::from_utf8(output.stdout).map_err(|e| {
        CliError::system(
            "git_output_invalid",
            format!("git common-dir for '{}' was not UTF-8: {e}", repo.display()),
        )
    })?;
    // `rev-parse` terminates its one path with LF. Remove exactly that protocol
    // byte; `trim()` would corrupt a legitimate path with edge whitespace.
    let raw = raw.strip_suffix('\n').unwrap_or(&raw);
    let identity = PathBuf::from(raw);
    if !identity.is_absolute() || identity.as_os_str().is_empty() {
        return Err(CliError::system(
            "git_output_invalid",
            format!(
                "git returned invalid common-dir '{}' for '{}'",
                raw,
                repo.display()
            ),
        ));
    }
    Ok(Some(identity.canonicalize().unwrap_or(identity)))
}

fn emit(runs: Vec<RunSummary>, spec: &OutputSpec, warnings: &[String]) -> Result<(), CliError> {
    match spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => {
            output::emit_envelope(&ListPayload { runs }, spec, warnings)?;
        }
        OutputFormat::Text => {
            if runs.is_empty() {
                println!("(no runs)");
            }
            for r in &runs {
                let sup = match r.supervisor.state {
                    SupervisorState::Alive => match r.supervisor.pid {
                        Some(pid) => format!("sup:alive({pid})"),
                        None => "sup:alive".to_string(),
                    },
                    SupervisorState::Dead => match r.supervisor.pid {
                        Some(pid) => format!("sup:dead({pid})"),
                        None => "sup:dead".to_string(),
                    },
                    SupervisorState::NotRecorded => "sup:none".to_string(),
                    SupervisorState::Unreadable => "sup:unreadable".to_string(),
                    SupervisorState::Unknown => "sup:unknown".to_string(),
                };
                // The status column carries a marker so a plain-text `run list`
                // no longer shows a zombie as an ordinary live `pending` run:
                // `(stillborn)` for a never-started run (supervisor died before
                // creating any worker node), `(stalled)` for an undriven
                // orchestrate driver. The two are mutually exclusive by
                // construction; stillborn is the more specific, so it wins.
                let status = if r.stillborn {
                    format!("{} (stillborn)", r.status)
                } else if r.stalled {
                    format!("{} (stalled)", r.status)
                } else if r.awaiting_input {
                    format!("{} (awaiting-input)", r.status)
                } else if r.attention_required {
                    // A worker that exited cleanly but skipped `run merge` — a
                    // non-terminal run awaiting a manual finish, distinct from a
                    // dead-supervisor stall (design.md §2.5).
                    format!("{} (attention)", r.status)
                } else {
                    r.status.clone()
                };
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\ttelemetry=absent:{},current:{},stale:{},clock_unreliable:{},invalid:{};run_status=unchanged",
                    r.run_id,
                    r.kind,
                    r.lifecycle,
                    status,
                    r.node_count,
                    sup,
                    output::escape_one_line(&r.title),
                    r.telemetry_counts.absent,
                    r.telemetry_counts.current,
                    r.telemetry_counts.stale,
                    r.telemetry_counts.clock_unreliable,
                    r.telemetry_counts.invalid,
                );
            }
            output::emit_text_warnings(warnings);
        }
    }
    Ok(())
}
