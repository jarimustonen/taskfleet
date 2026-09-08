//! `run create` — top-level + child-spawn run initialization.
//!
//! Top-level: materializes the worktree + tmux window + agent in an invisible
//! staging run dir, emits `node.created` with the discovered agent PID, then
//! atomically publishes the run and spawns its supervisor. A caller interrupted
//! while native materialization is under load can therefore never leave a
//! visible zero-node run behind.
//!
//! Child-spawn (`--parent-run-id` + `--parent-node-id`) follows the same staging
//! protocol, then emits `child.spawned` only after the child is published. This
//! keeps the parent's DAG bookkeeping transactional: a failed spawn emits NO
//! `child.spawned`, so it cannot leave a phantom 0-node child in `pending` on
//! the parent (issue: failed-spawn-leaves-phantom-child). The CLI does NOT spawn
//! a supervisor for the child — the parent's supervisor sees `child.spawned` in
//! its tail-follow loop and is the sole spawner of child supervisors
//! (single-arbiter invariant, design.md §7.2).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use chrono::{Duration, Utc};
use serde::Serialize;
use serde_json::{json, Value};

use taskfleet_core::{ensure_root, new_run_id, Kind, Lifecycle, RunId};

use crate::error::CliError;
use crate::idempotency;
use crate::output::{self, OutputFormat, OutputSpec};
use crate::run::{
    from_core, kind_kebab, lifecycle_for, lifecycle_kebab, parse_node_id, parse_run_id,
    require_nonempty, run_paths_exact, spawn, supervisor_spawn,
};

/// Drop-releases an idempotency reservation unless disarmed.
///
/// Armed the moment `reserve` wins the key; disarmed only once the complete run
/// has been atomically published. Thus every ordinary error before publication
/// releases it on unwind instead of making a retry replay an invisible staging
/// directory. Release is ownership-checked, so this never clobbers another
/// run's key. NOTE: `Drop` does not run on a hard process kill — the staging
/// directory is deliberately invisible to normal run readers in that case.
struct ReservationGuard {
    repo: Option<String>,
    branch: Option<String>,
    key: String,
    run_id: String,
    armed: bool,
    preserve_cleanup_obligation: bool,
}

impl ReservationGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ReservationGuard {
    fn drop(&mut self) {
        if self.armed && !self.preserve_cleanup_obligation {
            let _ = idempotency::release(
                self.repo.as_deref(),
                self.branch.as_deref(),
                &self.key,
                &self.run_id,
            );
        }
    }
}

/// A flat 1:1 mirror of `run create`'s clap flags (skip-materialize,
/// no-hooks, headless, dry-run), so the pedantic `struct_excessive_bools`
/// lint is allowed here — same allowance as the help-module arg bags.
#[allow(clippy::struct_excessive_bools)]
pub struct Args<'a> {
    pub skip_materialize: bool,
    pub kind: Kind,
    pub title: String,
    pub source_repo: Option<String>,
    pub source_branch: Option<String>,
    pub task: Option<String>,
    pub prompt_file: Option<String>,
    pub layout: Option<String>,
    pub no_hooks: bool,
    /// Place the worker's tmux window in a detached "headless" session.
    pub headless: bool,
    /// Explicit tmux session name; implies headless and overrides the
    /// `--headless` default name.
    pub tmux_session: Option<String>,
    /// Seconds native materializer waits for the agent to become discoverable,
    /// forwarded as `--agent-startup-timeout`. Validated to [1, 600] by
    /// clap; defaults to 90 (see the flag docs in `run/mod.rs`).
    pub agent_startup_timeout: u32,
    pub parent_run_id: Option<String>,
    pub parent_node_id: Option<String>,
    /// Raw legacy `--harness <name>` selector. In profile mode it aliases a
    /// same-named user profile whose candidates all use that harness; with no
    /// configured profiles it preserves the pre-profile harness resolver.
    pub harness: Option<String>,
    /// User-owned executable profile requested by `--profile`.
    pub profile: Option<String>,
    /// Mark the run **interactive** (`--interactive`). Sets the run's how-run
    /// [`Lifecycle`] to [`Lifecycle::Interactive`], recorded on `run.created`, so
    /// the supervisor waits for an explicit `run merge` / `run cancel` and never
    /// auto-terminalizes from a dead pid (design.md §6). `false` (the default) is
    /// the autonomous fire-and-forget worker. Orthogonal to `kind` — NOT derived
    /// from it.
    pub interactive: bool,
    /// Completion-notification command (`--notify`). Persisted into the
    /// `run.created` event as `notify_cmd` so the supervisor can run it once
    /// on the terminal transition. `None` for a run created without `--notify`.
    pub notify: Option<String>,
    pub idempotency_key: Option<String>,
    pub dry_run: bool,
    pub spec: &'a OutputSpec,
    pub warnings: &'a [String],
}

#[derive(Serialize)]
struct CreatedPayload<'a> {
    run_id: &'a str,
    dir: String,
    supervisor: SupervisorField,
    kind: KindStr,
    lifecycle: LifecycleStr,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_run_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_node_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tmux_window: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree_path: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    idempotent_replay: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dry_run: Option<bool>,
    selection: SelectionView<'a>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum SelectionView<'a> {
    Recorded(&'a taskfleet_core::AgentSelection),
    Legacy(&'static str),
}

#[derive(Serialize)]
#[serde(untagged)]
enum SupervisorField {
    Pid(u32),
    Note(&'static str),
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum KindStr {
    Spinoff,
    Research,
    TechnicalDecision,
    FanOut,
    Unknown,
}
impl From<Kind> for KindStr {
    fn from(k: Kind) -> Self {
        match k {
            Kind::Spinoff => KindStr::Spinoff,
            Kind::Research => KindStr::Research,
            Kind::TechnicalDecision => KindStr::TechnicalDecision,
            Kind::FanOut => KindStr::FanOut,
            Kind::Unknown => KindStr::Unknown,
        }
    }
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum LifecycleStr {
    Autonomous,
    Interactive,
}
impl From<Lifecycle> for LifecycleStr {
    fn from(l: Lifecycle) -> Self {
        match l {
            Lifecycle::Autonomous => LifecycleStr::Autonomous,
            Lifecycle::Interactive => LifecycleStr::Interactive,
        }
    }
}

/// Successful spawn details captured for both the node.created event
/// and the emit payload. Kept Option-free so callers handle the
/// "no spawn happened" (dry-run, idempotent replay) case explicitly.
struct SpawnResult {
    branch: String,
    worktree_path: String,
    tmux_window: String,
    agent_pid: i32,
}

pub fn run(args: Args<'_>) -> Result<(), CliError> {
    let title = require_nonempty(&args.title, "title")?;

    // How-run state is a TOLD fact from the explicit `--interactive` flag, not
    // inferred from `kind` (design.md §2/§6). `--interactive` → interactive; the
    // default falls back to the kind's autonomous seed (`lifecycle_for`), so a
    // plain `run create` is byte-identical to before this flag existed.
    let lifecycle = if args.interactive {
        Lifecycle::Interactive
    } else {
        lifecycle_for(args.kind)
    };

    // Idempotent replay is a read of an already-published resource, not a new
    // resolution. Short-circuit before loading config or probing PATH so config
    // drift cannot change or prevent the recorded answer.
    if !args.dry_run {
        if let Some(key) = args.idempotency_key.as_deref() {
            if let Some(existing) = idempotency::lookup(
                args.source_repo.as_deref(),
                args.source_branch.as_deref(),
                key,
            )? {
                let root = crate::home::root_dir()?;
                if let ExistingReservation::Published(dir) =
                    classify_existing_reservation(&root, &existing, Utc::now())?
                {
                    repair_parent_child_publication(&root, &dir)?;
                    let existing_id = parse_run_id(&existing.run_id)?;
                    let paths = run_paths_exact(&root, &existing_id)?;
                    let manifest = taskfleet_core::RunLock::with_shared_lock(&paths.lock(), || {
                        taskfleet_core::read_manifest_opt(&paths)
                    })
                    .map_err(from_core)?
                    .ok_or_else(|| {
                        CliError::system(
                            "run_not_published",
                            format!("published run {} has no manifest", existing.run_id),
                        )
                    })?;
                    let parent_run_id = manifest.parent_run_id.as_ref().map(ToString::to_string);
                    let parent_node_id = manifest.parent_node_id.as_ref().map(ToString::to_string);
                    return emit(EmitInput {
                        run_id: &existing.run_id,
                        dir: dir.display().to_string(),
                        kind: manifest.kind,
                        lifecycle: manifest.lifecycle,
                        parent_run_id: parent_run_id.as_deref(),
                        parent_node_id: parent_node_id.as_deref(),
                        node_id: None,
                        spawn: None,
                        supervisor_pid: None,
                        idempotent_replay: Some(true),
                        dry_run: None,
                        selection: manifest.agent_selection.as_ref(),
                        spec: args.spec,
                        warnings: args.warnings,
                    });
                }
            }
        }
    }

    // Resolve user-owned profiles and deterministic static fallback before any
    // mutation. With no configured profiles this deliberately returns the old
    // harness-only launcher for backward compatibility.
    let explicit_interaction = if args.interactive {
        Lifecycle::Interactive
    } else {
        Lifecycle::Autonomous
    };
    let resolution = crate::harness::profile::resolve(
        args.kind,
        explicit_interaction,
        args.profile.as_deref(),
        args.harness.as_deref(),
        args.source_repo.as_deref(),
    )?;
    let harness = &resolution.harness;
    // Native launch always has an exact candidate, including the built-in
    // no-profile compatibility path. Record it so retries never re-resolve from
    // current configuration or infer an executable from process names.
    let recorded_selection = resolution.selection.clone().unwrap_or_else(|| {
        spawn::builtin_agent_selection(&harness.name, explicit_interaction == Lifecycle::Autonomous)
    });
    let agent_selection = Some(&recorded_selection);

    // Validate the optional completion hook up front (fail fast, before we
    // touch disk): an all-whitespace `--notify` is a caller mistake. `None`
    // (flag omitted) is the common case and leaves the run hook-less.
    let notify_cmd = match args.notify.as_deref() {
        Some(raw) => Some(require_nonempty(raw, "notify")?),
        None => None,
    };

    // Resolve the optional headless target session up-front so a malformed
    // `--tmux-session` fails before we touch disk or the parent log — same
    // fail-fast contract as the prompt-source resolution below. `None` keeps
    // the existing foreground-spawn behavior (opt-in only).
    let parent_session = resolve_parent_session(args.headless, args.tmux_session.as_deref())?;

    let is_child = args.parent_run_id.is_some();
    if args.parent_run_id.is_some() ^ args.parent_node_id.is_some() {
        return Err(CliError::user(
            "invalid_arguments",
            "--parent-run-id and --parent-node-id must be set together",
        ));
    }

    // Resolve the prompt source up-front so a missing `--task` /
    // `--prompt-file` fails before we touch disk or the parent log.
    // Dry-run still requires it: rejecting late would let CI scripts
    // pass invalid configs as long as they always run --dry-run.
    // `--skip-materialize` is the test-only escape hatch that produces
    // only the run skeleton; there's no agent to prompt so neither
    // flag is required.
    // Test-only env override: integration tests use a bare run dir
    // without a real native materializer available. The env var implies
    // `--skip-materialize` and is set by the `bin()` helper in
    // `tests/`. Production callers never set it.
    let skip_materialize =
        args.skip_materialize || std::env::var("TASKFLEET_TEST_SKIP_MATERIALIZE").is_ok();
    let prompt_source = if skip_materialize {
        None
    } else {
        Some(resolve_prompt_source(
            args.task.as_deref(),
            args.prompt_file.as_deref(),
        )?)
    };

    if is_child && args.dry_run {
        return Err(CliError::user(
            "dry_run_unsupported",
            "child-spawn create cannot be truthfully dry-run; use --idempotency-key for safe retry",
        ));
    }

    // Validate the parent pointers strictly (RunId / NodeId shapes) ONCE, keeping
    // the typed ids so the exact path helper (and the `child.spawned` append)
    // reuse them without a re-parse — a parent pointer is exact-only and must
    // never route through the fuzzy CLI resolver. The `String` forms are derived
    // from the typed ids purely for the event-data payload and downstream
    // `as_deref`.
    let parent_run_id_typed = match args.parent_run_id.as_deref() {
        Some(v) => Some(parse_run_id(v)?),
        None => None,
    };
    let parent_node_id_typed = match args.parent_node_id.as_deref() {
        Some(v) => Some(parse_node_id(v)?),
        None => None,
    };
    let parent_run_id = parent_run_id_typed.as_ref().map(|r| r.as_str().to_string());
    let parent_node_id = parent_node_id_typed
        .as_ref()
        .map(|n| n.as_str().to_string());

    let root = crate::home::root_dir()?;

    let run_id = new_run_id();
    // Validate the freshly generated id (infallible in practice) so run_dir
    // gets a typed RunId rather than a raw &str.
    let run_id_typed = parse_run_id(&run_id)?;
    let child_dir = taskfleet_core::run_dir(&root, &run_id_typed);

    if args.dry_run {
        return emit(EmitInput {
            run_id: &run_id,
            dir: child_dir.display().to_string(),
            kind: args.kind,
            lifecycle,
            parent_run_id: None,
            parent_node_id: None,
            // Dry-run writes nothing to disk, so there is no node to report.
            node_id: None,
            spawn: None,
            supervisor_pid: None,
            idempotent_replay: None,
            dry_run: Some(true),
            selection: agent_selection,
            spec: args.spec,
            warnings: args.warnings,
        });
    }

    // Atomically reserve the idempotency key BEFORE materializing the run.
    // The top-of-function `lookup` is only a fast path; THIS reservation is the
    // authoritative check-and-set that closes the duplicate-create race. The
    // key file becomes visible to other callers only here — the old code stored
    // it after full materialization (seconds later, once native materializer had spawned
    // the whole worktree), so two near-simultaneous same-key calls both missed
    // the lookup and both spawned. `reserve` is an atomic filesystem operation:
    // exactly one concurrent caller wins and materializes; the losers observe
    // the reservation and replay the winner's run instead of spawning a
    // duplicate. Skipped on dry-run (handled above — dry-run persists nothing).
    let mut reclaimed_staging_runs = Vec::new();
    let mut materializer_lease = None;
    let mut reservation: Option<ReservationGuard> = if let Some(key) =
        args.idempotency_key.as_deref()
    {
        let lease = idempotency::MaterializerLease::acquire(&root, &run_id)?;
        let creator = idempotency::CreatorLease {
            pid: std::process::id(),
            pid_start_secs: crate::supervise::watchdog::pid_start_time(std::process::id()),
            started_at: Utc::now(),
            materializer_lease_path: Some(lease.path().display().to_string()),
        };
        materializer_lease = Some(lease);
        let proposed = idempotency::ReservationRecord::new(&run_id, creator);
        let mut observed = match idempotency::reserve(
            args.source_repo.as_deref(),
            args.source_branch.as_deref(),
            key,
            &proposed,
        )? {
            idempotency::Reservation::Reserved => None,
            idempotency::Reservation::AlreadyReserved(existing) => Some(existing),
        };
        for _ in 0..8 {
            let Some(existing) = observed.take() else {
                break;
            };
            match classify_existing_reservation(&root, &existing, Utc::now())? {
                ExistingReservation::Published(dir) => {
                    repair_parent_child_publication(&root, &dir)?;
                    let existing_id = parse_run_id(&existing.run_id)?;
                    let existing_paths = run_paths_exact(&root, &existing_id)?;
                    let manifest =
                        taskfleet_core::RunLock::with_shared_lock(&existing_paths.lock(), || {
                            taskfleet_core::read_manifest_opt(&existing_paths)
                        })
                        .map_err(from_core)?
                        .ok_or_else(|| {
                            CliError::system(
                                "run_not_published",
                                format!("published run {} has no manifest", existing.run_id),
                            )
                        })?;
                    return emit(EmitInput {
                        run_id: &existing.run_id,
                        dir: dir.display().to_string(),
                        kind: manifest.kind,
                        lifecycle: manifest.lifecycle,
                        parent_run_id: parent_run_id.as_deref(),
                        parent_node_id: parent_node_id.as_deref(),
                        node_id: None,
                        spawn: None,
                        supervisor_pid: None,
                        idempotent_replay: Some(true),
                        dry_run: None,
                        selection: manifest.agent_selection.as_ref(),
                        spec: args.spec,
                        warnings: args.warnings,
                    });
                }
                ExistingReservation::CreatorLive => {
                    let wait_ms = std::env::var("TASKFLEET_IDEMPOTENCY_PUBLISH_WAIT_MS")
                        .ok()
                        .and_then(|v| v.trim().parse::<u64>().ok())
                        .unwrap_or(30_000);
                    let deadline =
                        std::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
                    loop {
                        if std::time::Instant::now() >= deadline {
                            return Err(CliError::system(
                                "idempotency_creator_live",
                                format!(
                                    "idempotency key is still being materialized by live creator pid {} for run {} after {wait_ms}ms; retry later",
                                    existing.creator.as_ref().map_or(0, |c| c.pid), existing.run_id
                                ),
                            )
                            .with_invalid_value(existing.run_id));
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        let Some(current) = idempotency::lookup(
                            args.source_repo.as_deref(),
                            args.source_branch.as_deref(),
                            key,
                        )?
                        else {
                            // The owner failed cleanly and released. Attempt our
                            // proposal again through the authoritative reserve.
                            observed = match idempotency::reserve(
                                args.source_repo.as_deref(),
                                args.source_branch.as_deref(),
                                key,
                                &proposed,
                            )? {
                                idempotency::Reservation::Reserved => None,
                                idempotency::Reservation::AlreadyReserved(value) => Some(value),
                            };
                            break;
                        };
                        if !matches!(
                            classify_existing_reservation(&root, &current, Utc::now())?,
                            ExistingReservation::CreatorLive
                        ) {
                            observed = Some(current);
                            break;
                        }
                    }
                }
                ExistingReservation::Unverifiable => {
                    return Err(CliError::system(
                            "idempotency_creator_unverifiable",
                            format!(
                                "idempotency key points to unpublished run {}, but its creator identity cannot be verified; inspect the reservation and staging run before retrying",
                                existing.run_id
                            ),
                        )
                        .with_invalid_value(existing.run_id));
                }
                ExistingReservation::CreatorDead => {
                    let mut replacement = proposed.clone();
                    replacement
                        .stale_run_ids
                        .clone_from(&existing.stale_run_ids);
                    replacement.stale_run_ids.push(existing.run_id.clone());
                    replacement.stale_run_ids.sort();
                    replacement.stale_run_ids.dedup();
                    let existing_id = parse_run_id(&existing.run_id)?;
                    let published_manifest =
                        taskfleet_core::run_dir(&root, &existing_id).join("manifest.json");
                    match idempotency::reclaim(
                        args.source_repo.as_deref(),
                        args.source_branch.as_deref(),
                        key,
                        &existing,
                        &replacement,
                        &published_manifest,
                    )? {
                        idempotency::Reclaim::Reclaimed => {
                            reclaimed_staging_runs = replacement.stale_run_ids;
                            break;
                        }
                        idempotency::Reclaim::Published => observed = Some(existing),
                        idempotency::Reclaim::Changed(current) => observed = Some(current),
                    }
                }
            }
        }
        if observed.is_some() {
            return Err(CliError::system(
                "idempotency_reservation_contended",
                "idempotency reservation changed repeatedly while reclaiming; retry",
            ));
        }
        Some(ReservationGuard {
            repo: args.source_repo.clone(),
            branch: args.source_branch.clone(),
            key: key.to_string(),
            run_id: run_id.clone(),
            armed: true,
            preserve_cleanup_obligation: !reclaimed_staging_runs.is_empty(),
        })
    } else {
        None
    };

    // Keep the inheritable materializer lease alive through native materializer and the
    // publication rename. The binding is intentionally read here so lints and
    // future refactors cannot shorten its lifetime before publication.
    let _materializer_lease = materializer_lease.as_ref();
    ensure_root(&root).map_err(from_core)?;
    // A run becomes externally visible only when its fully materialized state
    // is renamed from this sibling root into `<root>/runs`. In particular, a
    // A harness/client timeout while native materialization is waiting for a
    // loaded headless tmux session leaves no manifest that `run list`/`run wait` could mistake
    // for an accepted run.
    let staging_root = root.join(".creating");
    ensure_root(&staging_root).map_err(from_core)?;
    for stale_run_id in &reclaimed_staging_runs {
        let stale_id = parse_run_id(stale_run_id)?;
        let stale_dir = taskfleet_core::run_dir(&staging_root, &stale_id);
        rollback_reclaimed_materialization(&stale_dir, &stale_id);
        match std::fs::remove_dir_all(&stale_dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                // Keep the replacement reservation durable: its stale_run_ids
                // obligation lets the next retry resume cleanup. Releasing it
                // here would lose the only pointer to the inherited staging dir.
                if let Some(g) = reservation.as_mut() {
                    g.disarm();
                }
                return Err(CliError::system(
                    "stale_staging_cleanup_failed",
                    format!("remove reclaimed staging run {}: {e}", stale_dir.display()),
                ));
            }
        }
    }
    if !reclaimed_staging_runs.is_empty() {
        let key = args
            .idempotency_key
            .as_deref()
            .expect("reclaimed staging requires an idempotency key");
        idempotency::finish_stale_cleanup(
            args.source_repo.as_deref(),
            args.source_branch.as_deref(),
            key,
            &run_id,
        )?;
        if let Some(g) = reservation.as_mut() {
            g.preserve_cleanup_obligation = false;
        }
    }

    if is_child {
        // Validate the parent exists up front (fail fast before we create the
        // child run dir or shell out to native materializer). The `child.spawned` event
        // itself is emitted only AFTER native materializer succeeds — see below — so a
        // native materializer failure never pollutes the parent's DAG bookkeeping.
        let parent_run_id = parent_run_id.as_deref().unwrap();
        // `is_child` ⇒ the parent pointer was validated to a typed `RunId` above;
        // reuse it for the exact path — a parent pointer must never fuzzy-resolve.
        let parent_paths = run_paths_exact(&root, parent_run_id_typed.as_ref().expect("is_child"))?;
        if !parent_paths.manifest().exists() {
            return Err(CliError::user(
                "parent_not_found",
                format!("parent run {parent_run_id} does not exist"),
            )
            .with_invalid_value(parent_run_id));
        }
    }

    // Materialize in an invisible sibling root. Only after `node.created` is
    // durable do we rename this directory into the public `<root>/runs` tree.
    // `rename` is atomic because both roots live under the same state root.
    let staging_dir = taskfleet_core::run_dir(&staging_root, &run_id_typed);
    std::fs::create_dir_all(&staging_dir).map_err(|e| {
        CliError::system(
            "io_error",
            format!("mkdir {}: {}", staging_dir.display(), e),
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&staging_dir, std::fs::Permissions::from_mode(0o700)).map_err(
            |e| CliError::system("io_error", format!("chmod {}: {e}", staging_dir.display())),
        )?;
    }
    let paths = taskfleet_core::RunPaths::from_validated(&staging_dir, run_id_typed.clone())
        .map_err(from_core)?;

    // Materialize the prompt file under <run-dir>/prompt.md unless the
    // caller supplied one outside the run dir (in which case we use the
    // file as-is so they keep ownership over it). When
    // `--skip-materialize` is set there's no spawn, no prompt.
    // Add the exact run context and issue-filing boundary to every worker.
    // Harnesses that need translation (currently pi research) receive that note
    // in the same generated preamble.
    let prompt_path = match prompt_source {
        Some(src) => {
            let preamble =
                crate::harness::prompt::worker_prompt_preamble(&harness.name, args.kind, &run_id);
            Some(resolve_prompt_file(&staging_dir, src, &preamble)?)
        }
        None => None,
    };

    // Write run.created BEFORE shelling out so a native materializer crash leaves
    // a recoverable run on disk that `run show`/`run cancel` can see. For a
    // child spawn this run dir is instead removed wholesale on native materializer
    // failure (see the spawn-failure arms below) — nothing references it yet
    // because `child.spawned` is emitted only after success.
    let mut data = serde_json::Map::new();
    data.insert("kind".into(), Value::String(kind_kebab(args.kind).into()));
    data.insert(
        "lifecycle".into(),
        Value::String(lifecycle_kebab(lifecycle).into()),
    );
    data.insert("title".into(), Value::String(title.clone()));
    let materialization_repo = args.source_repo.clone().or_else(|| {
        (!skip_materialize)
            .then(std::env::current_dir)
            .transpose()
            .ok()
            .flatten()
            .and_then(|path| path.canonicalize().ok().or(Some(path)))
            .and_then(|path| path.to_str().map(str::to_owned))
    });
    if let Some(v) = materialization_repo.as_deref() {
        data.insert("source_repo".into(), Value::String(v.into()));
    }
    if let Some(v) = args.source_branch.as_deref() {
        data.insert("source_branch".into(), Value::String(v.into()));
    }
    // Record the headless session Taskfleet is about to create via
    // the native materializer's `--parent-session` so the supervisor can tear it down once
    // its last managed window is gone. `None` for a foreground spawn — that
    // window lives in the user's own session, which is never a teardown target
    // (issue `headless-tmux-session-not-torn-down`).
    if let Some(v) = parent_session.as_deref() {
        data.insert("managed_tmux_session".into(), Value::String(v.into()));
    }
    // Persist the terminal-completion hook so the supervisor can run it once
    // when this run settles (issue `no-completion-notification-to-parent`).
    // Trimmed and empty-rejected up front — an all-whitespace `--notify` is a
    // caller mistake, not a silent no-op hook.
    if let Some(cmd) = notify_cmd.as_deref() {
        data.insert("notify_cmd".into(), Value::String(cmd.into()));
    }
    // Record the resolved harness (folded into `manifest.harness` by the reducer)
    // and, for provenance, which precedence layer chose it. `harness_source` is
    // event-log-only — it explains *why* this run got this harness without
    // bloating the manifest projection.
    data.insert("harness".into(), Value::String(harness.name.clone()));
    data.insert(
        "harness_source".into(),
        Value::String(harness.source.as_str().into()),
    );
    if let Some(selection) = agent_selection {
        data.insert(
            "agent_selection".into(),
            serde_json::to_value(selection).expect("agent selection serializes"),
        );
    }
    if let Some(v) = args.task.as_deref() {
        data.insert("task".into(), Value::String(v.into()));
    }
    if is_child {
        data.insert(
            "parent_run_id".into(),
            Value::String(parent_run_id.clone().unwrap()),
        );
        data.insert(
            "parent_node_id".into(),
            Value::String(parent_node_id.clone().unwrap()),
        );
    }
    // As with `child.spawned` above, `run create` dedups via the CLI-level
    // idempotency reservation (`reserve`, above), not the folded event-log
    // scan — so the append carries no idempotency key.
    taskfleet_core::append_and_apply_event(&paths, "run.created", None, None, Value::Object(data))
        .map_err(from_core)?;

    // The 0.2 cut removed the `orchestrate` DAG-driver kind — the only kind that
    // synthesized its own `n-0001` driver node here. Every surviving kind's node
    // is materialized by native materialization (a `fan-out` driver's included), so the
    // envelope carries no synthesized node id (`None`) on this path.

    // Emit `child.spawned` on the parent's log. This is what makes the parent
    // supervisor discover and adopt the child (§7.2). It is emitted only once
    // the child is known-live: for a materialized child that means AFTER
    // native materializer returns success (see the call site below the spawn); for a
    // `--skip-materialize` skeleton child there is no native materializer that can fail,
    // so the child is live the moment its run dir exists. Either way a failed
    // spawn emits no `child.spawned` and leaves no phantom child on the parent.
    let emit_child_spawned = || -> Result<(), CliError> {
        if !is_child {
            return Ok(());
        }
        let parent_paths = run_paths_exact(&root, parent_run_id_typed.as_ref().expect("is_child"))?;
        let child_data = json!({
            "child_run_id": run_id,
            "child_node_id": "n-0001",
            "child_kind": kind_kebab(args.kind),
            "child_title": title,
        });
        // The key is child-identity-specific, not the caller's create key. A
        // retry after child publication can therefore repair this exact parent
        // edge without deduping a distinct child or appending it twice.
        let edge_key = format!("child-spawned:{run_id}");
        append_child_spawned_if_missing(
            &parent_paths,
            parent_node_id_typed.as_ref().expect("is_child"),
            &run_id,
            &edge_key,
            child_data,
        )?;
        Ok(())
    };

    // `--skip-materialize` short-circuit: skeleton-only run, used by
    // tests that need a run dir without booting a real worktree/agent.
    if skip_materialize {
        publish_staging_run(&staging_dir, &child_dir)?;
        // Publication is the idempotency commit point even for a skeleton; do
        // not release the key if the following parent append has an I/O error.
        if let Some(g) = reservation.as_mut() {
            g.disarm();
        }
        // Deterministic integration-test seam for the cross-log crash window.
        // It is reachable only through the test-only skeleton path, never for a
        // production materialization.
        if cfg!(debug_assertions)
            && std::env::var("TASKFLEET_TEST_SKIP_MATERIALIZE").is_ok_and(|v| v == "1")
            && std::env::var("TASKFLEET_TEST_FAIL_AFTER_PUBLISH").is_ok_and(|v| v == "1")
        {
            return Err(CliError::system(
                "test_fail_after_publish",
                "injected failure after child publication",
            ));
        }
        // The skeleton child is live as soon as its run dir is published — emit
        // `child.spawned` here. The idempotency key was already reserved before
        // materialization (see `reserve` above), same as the materialized path.
        emit_child_spawned()?;
        // A `--skip-materialize` / `TASKFLEET_TEST_SKIP_MATERIALIZE` run is a pure
        // skeleton with NO supervisor (the only kind that needed a supervisor on
        // this path was the removed `orchestrate` driver).
        return emit(EmitInput {
            run_id: &run_id,
            dir: child_dir.display().to_string(),
            kind: args.kind,
            lifecycle,
            parent_run_id: parent_run_id.as_deref(),
            parent_node_id: parent_node_id.as_deref(),
            node_id: None,
            spawn: None,
            supervisor_pid: None,
            idempotent_replay: None,
            dry_run: None,
            selection: agent_selection,
            spec: args.spec,
            warnings: args.warnings,
        });
    }

    let prompt_path = prompt_path.expect("non-skip path resolves prompt source");
    // Launch from the exact selection already recorded in `run.created`: no
    // config reload, candidate re-resolution, or fallback can occur between
    // recording and launch.
    let launcher_selection = &recorded_selection;
    let agent_launcher = spawn::write_agent_launcher(
        &staging_dir,
        &root,
        launcher_selection,
        &run_id,
        "n-0001",
        0,
    )?;
    let agent_override = agent_launcher.path().to_str().ok_or_else(|| {
        CliError::system(
            "agent_launcher_path_invalid",
            format!(
                "agent launcher path is not UTF-8: {}",
                agent_launcher.path().display()
            ),
        )
    })?;
    // Shell out to native materializer. A failure remains private and is removed: no
    // public manifest exists until a live worker node is durable. This holds for
    // top-level and child runs alike, so neither can strand a visible 0-node
    // `pending` run. The cleanup is helper-wrapped so both failure arms (the
    // native materializer non-zero exit and the PID-liveness re-check) share it.
    let branch_name = derive_branch_name(args.kind, &run_id_typed, &title);
    let spawn_req = spawn::SpawnRequest {
        kind: kind_kebab(args.kind),
        // Profile-backed launches pass only the private launcher path; that
        // launcher execs the selected candidate's exact recorded argv. Legacy
        // launches retain the historical harness → workmux-agent mapping.
        agent: Some(agent_override),
        branch: &branch_name,
        prompt_file: &prompt_path,
        layout: args.layout.as_deref(),
        no_hooks: args.no_hooks,
        keep_tmux_on_error: false,
        parent_session: parent_session.as_deref(),
        agent_startup_timeout: args.agent_startup_timeout,
        // Fork the worktree's branch from the named source branch (e.g. an
        // orchestrate integration branch) rather than workmux's default base.
        // `None` for runs without --source-branch keeps the prior behaviour.
        source_branch: args.source_branch.as_deref(),
        // Pin the exact repository recorded in run.created so hard-kill
        // reconciliation and retries act on the same checkout.
        cwd: materialization_repo.as_deref().map(Path::new),
        launcher: Some(&agent_launcher),
    };
    // On any spawn failure for a child, drop the orphan run dir before
    // returning the error. Best-effort: a leftover dir is far less harmful
    // than a panic mid-error-handling, so a remove failure is swallowed.
    let cleanup_orphan_child = || {
        agent_launcher.cleanup_private_session();
        let _ = std::fs::remove_dir_all(&staging_dir);
        // The reservation remains armed until publication, so an ordinary spawn
        // failure releases it on unwind and a keyed retry starts cleanly. A hard
        // client kill can leave staging state, but never a public run manifest.
    };
    let mut outcome = match spawn::materialize_native(&spawn_req) {
        Ok(o) => o,
        Err(e) => {
            cleanup_orphan_child();
            return Err(e);
        }
    };
    // Capture the branch's fork point — the tip the worktree was just created
    // from — as an immutable reference for the supervisor's later merge
    // reconciliation. Right after native materialization creates the worktree, the new
    // branch's HEAD *is* the fork base; recording it lets the supervisor
    // distinguish "did work that merged into source" from "never diverged"
    // (issues `false-failed-after-merge` /
    // `supervisor-stuck-pending-after-self-merge`). Best-effort: a git failure
    // leaves `base_sha` null and the reconcile fallback simply does not fire.
    let base_sha = capture_base_sha(&outcome.worktree_path);
    // An explicit source is authoritative. Otherwise read the branch-creation
    // provenance from the materialized worker branch itself. Unlike ambient
    // cwd, that reflog is anchored to the repo and base workmux actually used.
    let materialized_source_branch = args
        .source_branch
        .clone()
        .or_else(|| capture_materialized_source_branch(&outcome.worktree_path, &outcome.branch));

    // Re-verify the exact handshaken identity at the last boundary before the
    // durable node append. A replacement process must never become a new
    // baseline merely because the original PID was reused.
    if let Err(e) = spawn::verify_agent_pid(
        outcome.agent_pid_hint,
        outcome.agent_start_time,
        &outcome.agent_start_identity,
    ) {
        drop(outcome);
        cleanup_orphan_child();
        return Err(e);
    }

    // Emit node.created with the discovered metadata. The reducer creates
    // nodes/n-0001.json and fills a previously-unknown manifest source branch
    // in the same locked append/apply transaction.
    let node_data = json!({
        "kind": kind_kebab(args.kind),
        "branch": outcome.branch,
        "source_branch": materialized_source_branch,
        "base_sha": base_sha,
        "worktree_path": outcome.worktree_path,
        "tmux_window": outcome.tmux_window,
        // Qualified tmux identity (null on a native materializer that predates the
        // fields); the reducer folds these into Node.tmux_identity.
        "tmux_socket": outcome.tmux_socket,
        "tmux_session": outcome.tmux_session,
        "tmux_window_id": outcome.tmux_window_id,
        "tmux_pane_id": outcome.tmux_pane_id,
        // Exact native Pi session identity was assigned and its header durably
        // created by the pre-exec handshake before the candidate started.
        "pi_session_id": outcome.pi_session_id,
        "pi_session_path": outcome.pi_session_path,
        "pi_session_cwd": outcome.pi_session_cwd,
        "attempt": 0,
        "agent_pid": outcome.agent_pid_hint,
        "agent_pid_start_time": chrono::DateTime::<Utc>::from_timestamp(
            i64::try_from(outcome.agent_start_time).unwrap_or(i64::MAX),
            0,
        ),
        "task": args.task,
        "parent_node_id": parent_node_id,
    });
    if let Err(error) = taskfleet_core::append_and_apply_event(
        &paths,
        "node.created",
        Some(&parse_node_id("n-0001").expect("n-0001 is a valid node id")),
        None,
        node_data,
    ) {
        drop(outcome);
        cleanup_orphan_child();
        return Err(from_core(error));
    }

    // `node.created` is now durable in the staging directory. Publish it as a
    // single rename before telling a parent about it or launching a supervisor:
    // every successful `run create` therefore names an already-existing node.
    if let Err(error) = publish_staging_run(&staging_dir, &child_dir) {
        drop(outcome);
        cleanup_orphan_child();
        return Err(error);
    }
    outcome.commit();
    // Publication is the idempotency commit point. It precedes every fallible
    // operation: otherwise an error after the rename could release the key and
    // let a retry create a duplicate public child.
    if let Some(g) = reservation.as_mut() {
        g.disarm();
    }
    let paths = run_paths_exact(&root, &run_id_typed)?;

    // Emit-after-publication: only now that native materializer returned a live, verified
    // child does the parent learn about it. This preserves transactional parent
    // bookkeeping: a failed spawn emits no parent event and no public child.
    emit_child_spawned()?;

    // For top-level runs, spawn the supervisor and wait for its PID
    // file. Child-spawn delegates supervisor creation to the parent
    // supervisor (design.md §7.2 step 6).
    let supervisor_pid = if is_child {
        None
    } else {
        Some(spawn_supervisor_or_fail(&paths, &run_id)?)
    };

    let spawn_result = SpawnResult {
        branch: outcome.branch,
        worktree_path: outcome.worktree_path,
        tmux_window: outcome.tmux_window,
        agent_pid: outcome.agent_pid_hint as i32,
    };
    let _ = spawn_result.agent_pid; // recorded on the node via reducer; unused in payload

    emit(EmitInput {
        run_id: &run_id,
        dir: child_dir.display().to_string(),
        kind: args.kind,
        lifecycle,
        parent_run_id: parent_run_id.as_deref(),
        parent_node_id: parent_node_id.as_deref(),
        node_id: Some("n-0001"),
        spawn: Some(&spawn_result),
        supervisor_pid,
        idempotent_replay: None,
        dry_run: None,
        selection: agent_selection,
        spec: args.spec,
        warnings: args.warnings,
    })
}

const CREATOR_WITHOUT_IDENTITY_STALE_AFTER_MINS: i64 = 30;

#[derive(Debug, PartialEq, Eq)]
enum ExistingReservation {
    Published(PathBuf),
    CreatorLive,
    CreatorDead,
    Unverifiable,
}

/// Classify an existing reservation without guessing. Publication wins even if
/// the creator died immediately afterward. An unpublished creator with an
/// authoritative start-time identity is reclaimable only when that identity is
/// no longer live. A legacy/no-identity record fails closed after its bounded
/// trust window instead of treating a recycled PID as either live or dead.
fn classify_existing_reservation(
    root: &std::path::Path,
    record: &idempotency::ReservationRecord,
    now: chrono::DateTime<Utc>,
) -> Result<ExistingReservation, CliError> {
    classify_existing_reservation_with(root, record, now, |pid, start| {
        crate::supervise::pid_file::pid_live_with_identity(pid, start)
    })
}

fn classify_existing_reservation_with(
    root: &std::path::Path,
    record: &idempotency::ReservationRecord,
    now: chrono::DateTime<Utc>,
    owner_live: impl FnOnce(u32, Option<u64>) -> bool,
) -> Result<ExistingReservation, CliError> {
    let run_id = parse_run_id(&record.run_id)?;
    let dir = taskfleet_core::run_dir(root, &run_id);
    match dir.join("manifest.json").try_exists() {
        Ok(true) => return Ok(ExistingReservation::Published(dir)),
        Ok(false) => {}
        Err(e) => {
            return Err(CliError::system(
                "io_error",
                format!("check {}/manifest.json: {e}", dir.display()),
            ))
        }
    }
    let Some(creator) = record.creator.as_ref() else {
        return Ok(ExistingReservation::Unverifiable);
    };
    if let Some(path) = creator.materializer_lease_path.as_deref() {
        return Ok(match idempotency::materializer_liveness(path) {
            idempotency::LeaseLiveness::Live => ExistingReservation::CreatorLive,
            idempotency::LeaseLiveness::Dead => ExistingReservation::CreatorDead,
            idempotency::LeaseLiveness::Unverifiable => ExistingReservation::Unverifiable,
        });
    }
    let live = owner_live(creator.pid, creator.pid_start_secs);
    match (live, creator.pid_start_secs) {
        (false, _) => Ok(ExistingReservation::CreatorDead),
        (true, Some(_)) => Ok(ExistingReservation::CreatorLive),
        (true, None)
            if now.signed_duration_since(creator.started_at)
                < Duration::minutes(CREATOR_WITHOUT_IDENTITY_STALE_AFTER_MINS) =>
        {
            Ok(ExistingReservation::CreatorLive)
        }
        (true, None) => Ok(ExistingReservation::Unverifiable),
    }
}

/// Repair the child-publication/parent-edge crash window. The child's durable
/// manifest is the transaction record: once published, every keyed replay
/// appends the exact `child.spawned` edge idempotently to the recorded parent's
/// log before returning success.
fn repair_parent_child_publication(
    root: &std::path::Path,
    child_dir: &std::path::Path,
) -> Result<(), CliError> {
    let child_id = child_dir
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| CliError::system("invalid_run_id", "published child path has no run id"))?;
    let child_id_typed = parse_run_id(child_id)?;
    let child_paths = run_paths_exact(root, &child_id_typed)?;
    let manifest = taskfleet_core::RunLock::with_shared_lock(&child_paths.lock(), || {
        taskfleet_core::read_manifest_opt(&child_paths)
    })
    .map_err(from_core)?
    .ok_or_else(|| {
        CliError::system(
            "run_not_published",
            format!("published run {child_id} has no durable manifest"),
        )
    })?;
    let (Some(parent_run_id), Some(parent_node_id)) = (
        manifest.parent_run_id.as_ref(),
        manifest.parent_node_id.as_ref(),
    ) else {
        if manifest.parent_run_id.is_some() || manifest.parent_node_id.is_some() {
            return Err(CliError::system(
                "child_parent_link_invalid",
                format!("child run {child_id} has an incomplete parent identity"),
            ));
        }
        return Ok(());
    };
    let parent_paths = run_paths_exact(root, parent_run_id)?;
    let data = json!({
        "child_run_id": child_id,
        "child_node_id": "n-0001",
        "child_kind": kind_kebab(manifest.kind),
        "child_title": manifest.title,
    });
    let key = format!("child-spawned:{child_id}");
    append_child_spawned_if_missing(&parent_paths, parent_node_id, child_id, &key, data)
}

/// Append one parent edge while remaining compatible with pre-keyed logs. The
/// parent lock covers both the legacy child-id scan and the append, so a repair
/// cannot race another repair into writing a duplicate edge.
fn append_child_spawned_if_missing(
    parent_paths: &taskfleet_core::RunPaths,
    parent_node_id: &taskfleet_core::NodeId,
    child_run_id: &str,
    edge_key: &str,
    data: Value,
) -> Result<(), CliError> {
    taskfleet_core::RunLock::with_lock(parent_paths, |lock| {
        let events = taskfleet_core::read_all_events(&parent_paths.events())?;
        if events.iter().any(|event| {
            event.kind == "child.spawned"
                && event.data.get("child_run_id").and_then(Value::as_str) == Some(child_run_id)
        }) {
            return Ok(());
        }
        taskfleet_core::append_and_apply_unlocked(
            lock,
            parent_paths,
            "child.spawned",
            Some(parent_node_id),
            Some(edge_key),
            data,
        )?;
        Ok(())
    })
    .map_err(from_core)
}

/// Reconcile external resources left by a creator that was hard-killed after
/// `workmux add`. The inherited reservation lease prevents this from running
/// until the materializer subprocess has exited; the stale manifest supplies
/// the immutable repo/title/kind needed to reconstruct its unique branch.
fn rollback_reclaimed_materialization(stale_dir: &Path, stale_id: &RunId) {
    let Ok(paths) = taskfleet_core::RunPaths::from_validated(stale_dir, stale_id.clone()) else {
        return;
    };
    let Some(manifest) = taskfleet_core::read_manifest_opt(&paths).ok().flatten() else {
        return;
    };
    let Some(repo) = manifest.source_repo.as_deref() else {
        return;
    };
    let branch = derive_branch_name(manifest.kind, stale_id, &manifest.title);
    let workmux = std::env::var("WORKMUX_BIN").unwrap_or_else(|_| "workmux".into());
    let git = std::env::var("GIT_BIN").unwrap_or_else(|_| "git".into());
    let path = Command::new(&workmux)
        .args(["path", &branch])
        .current_dir(repo)
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let output = Command::new(&git)
                .args(["worktree", "list", "--porcelain"])
                .current_dir(repo)
                .stdin(Stdio::null())
                .output()
                .ok()?;
            let text = String::from_utf8(output.stdout).ok()?;
            let expected = format!("branch refs/heads/{branch}");
            text.split("\n\n").find_map(|record| {
                let mut worktree = None;
                let mut matches = false;
                for line in record.lines() {
                    if let Some(value) = line.strip_prefix("worktree ") {
                        worktree = Some(value.to_owned());
                    }
                    if line == expected {
                        matches = true;
                    }
                }
                matches.then_some(worktree).flatten()
            })
        });
    let _ = Command::new(&workmux)
        .args(["remove", "--force", &branch])
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if let Some(path) = path {
        let _ = Command::new(&git)
            .args(["worktree", "remove", "--force", &path])
            .current_dir(repo)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = Command::new(&git)
        .args(["branch", "-D", "--", &branch])
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Atomically publish a fully materialized staging run into the public run
/// tree. Both paths are siblings under the same filesystem in the normal state
/// root layout, so a successful rename cannot expose a partially-written
/// manifest or node projection.
fn publish_staging_run(
    staging_dir: &std::path::Path,
    child_dir: &std::path::Path,
) -> Result<(), CliError> {
    std::fs::rename(staging_dir, child_dir).map_err(|e| {
        CliError::system(
            "run_publish_failed",
            format!(
                "publish staged run {} to {}: {e}",
                staging_dir.display(),
                child_dir.display()
            ),
        )
    })
}

/// Spawn the run's supervisor and require it to confirm start, turning a
/// silent boot failure into a loud, actionable error.
///
/// [`supervisor_spawn::spawn_for_run`] returns
/// [`SupervisorSpawn::Unconfirmed`](crate::run::supervisor_spawn::SupervisorSpawn::Unconfirmed)
/// when the detached supervisor never confirms boot over its readiness pipe —
/// it died during init, reported a structured boot error, or the fork/exec
/// failed — the run would otherwise be left in `pending` with a dead/absent
/// supervisor and its envelope would misreport a bogus success (issue
/// `supervisor-spawn-fails-silently-at-run-create`, suggested-fix #1; the
/// timeout ambiguity itself is retired by `supervisor-confirm-readiness-pipe`).
/// We instead return `supervisor_spawn_failed` carrying the run id, so the
/// caller can inspect `supervisor.stderr.log`, `run reattach`, or `run cancel`
/// rather than hang until its own timeout. The run.created / node.created
/// events are already durable on disk (and the idempotency key, if any, was
/// reserved before this call), so the run is fully recoverable and a keyed
/// retry replays it rather than duplicating it.
fn spawn_supervisor_or_fail(
    paths: &taskfleet_core::RunPaths,
    run_id: &str,
) -> Result<u32, CliError> {
    match supervisor_spawn::spawn_for_run(paths, run_id)? {
        supervisor_spawn::SupervisorSpawn::Confirmed { pid } => Ok(pid),
        supervisor_spawn::SupervisorSpawn::Unconfirmed { reason } => Err(CliError::system(
            "supervisor_spawn_failed",
            format!(
                "supervisor for run {run_id} did not confirm boot ({reason}). The run is on \
                 disk in `pending`. Inspect '{}/supervisor.stderr.log', then \
                 `taskfleet run reattach {run_id}` to retry (a no-op if one is already \
                 live) or `taskfleet run cancel {run_id}` to tear it down",
                paths.root.display()
            ),
        )
        .with_invalid_value(run_id)),
    }
}

/// Default detached session name used when `--headless` is set without an
/// explicit `--tmux-session`. The user attaches with `tmux attach -t headless`.
const DEFAULT_HEADLESS_SESSION: &str = "headless";

/// Resolve the optional `--parent-session` value forwarded to native materializer from
/// the `--headless` / `--tmux-session` pair:
///
/// - `--tmux-session <name>` always wins (it also implies headless placement),
/// - `--headless` alone yields the [`DEFAULT_HEADLESS_SESSION`] name,
/// - neither yields `None` — the existing foreground spawn (opt-in only).
///
/// An explicit name is validated strictly: a tmux session name must be
/// non-empty and may not contain whitespace or tmux's `:`/`.` target
/// separators, which would otherwise be silently mis-parsed by tmux as a
/// `session:window.pane` target. The default `headless` name trivially passes.
/// Read the current `HEAD` commit SHA of the freshly-created worktree — the
/// branch's fork point — for [`Node::base_sha`](taskfleet_core::Node). Best-effort:
/// returns `None` if git is unavailable, the path is not a worktree, or the
/// output is not a clean SHA, in which case the supervisor's merge-reconcile
/// fallback simply does not fire for this node. Honors the `GIT_BIN` override so
/// tests can stub (or disable) git the same way the supervisor's teardown does.
pub(crate) fn capture_base_sha(worktree_path: &str) -> Option<String> {
    let git = std::env::var("GIT_BIN").unwrap_or_else(|_| "git".to_string());
    let out = std::process::Command::new(git)
        .arg("-C")
        .arg(worktree_path)
        .args(["rev-parse", "HEAD"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // A full `rev-parse HEAD` yields exactly a 40-char (SHA-1) or 64-char
    // (SHA-256) hex object id. Require that exact shape — an abbreviated or
    // malformed value would persist a base that silently disables or misfires the
    // reconcile check.
    let ok = matches!(sha.len(), 40 | 64) && sha.chars().all(|c| c.is_ascii_hexdigit());
    ok.then_some(sha)
}

/// Read the base named by git when the materializer created `branch`.
///
/// This is anchored to the new worktree rather than ambient cwd, so it cannot
/// accidentally persist a branch from another repository or race a later
/// checkout in the source worktree. Best-effort: custom reflog formats,
/// disabled reflogs, and unnamed sources such as `HEAD` remain unknown.
fn capture_materialized_source_branch(worktree_path: &str, branch: &str) -> Option<String> {
    let git = std::env::var("GIT_BIN").unwrap_or_else(|_| "git".to_string());
    let out = std::process::Command::new(git)
        .arg("-C")
        .arg(worktree_path)
        .args(["reflog", "show", "--format=%gs", "-1", branch])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .strip_prefix("branch: Created from ")
        .map(str::to_string)
        .filter(|base| !base.is_empty() && base != "HEAD")
}

fn resolve_parent_session(
    headless: bool,
    tmux_session: Option<&str>,
) -> Result<Option<String>, CliError> {
    match tmux_session {
        Some(raw) => {
            let name = raw.trim();
            if name.is_empty() {
                return Err(CliError::user(
                    "invalid_value",
                    "--tmux-session must not be empty or whitespace-only",
                )
                .with_invalid_value(raw));
            }
            if name
                .chars()
                .any(|c| c.is_whitespace() || c == ':' || c == '.')
            {
                return Err(CliError::user(
                    "invalid_value",
                    "--tmux-session must not contain whitespace or the ':'/'.' tmux target separators",
                )
                .with_invalid_value(raw));
            }
            Ok(Some(name.to_string()))
        }
        None if headless => Ok(Some(DEFAULT_HEADLESS_SESSION.to_string())),
        None => Ok(None),
    }
}

#[derive(Debug)]
enum PromptSource {
    Task(String),
    File(PathBuf),
}

fn resolve_prompt_source(
    task: Option<&str>,
    prompt_file: Option<&str>,
) -> Result<PromptSource, CliError> {
    match (task, prompt_file) {
        (Some(t), None) => {
            let t = t.trim();
            if t.is_empty() {
                return Err(CliError::user(
                    "invalid_value",
                    "--task must not be empty or whitespace-only",
                ));
            }
            Ok(PromptSource::Task(t.to_string()))
        }
        (None, Some(p)) => {
            let path = PathBuf::from(p);
            if !path.exists() {
                return Err(CliError::user(
                    "prompt_file_not_found",
                    format!("--prompt-file does not exist: {}", path.display()),
                )
                .with_invalid_value(p));
            }
            Ok(PromptSource::File(path))
        }
        (Some(_), Some(_)) => Err(CliError::user(
            "invalid_arguments",
            "--task and --prompt-file are mutually exclusive",
        )),
        (None, None) => Err(CliError::user(
            "missing-task-or-prompt-file",
            "either --task <text> or --prompt-file <path> is required",
        )),
    }
}

/// Materialize the worker's prompt with generated run context (and any harness
/// translation; see [`crate::harness::prompt`]).
///
/// The derived prompt (`preamble + "\n\n" + brief`) is always written into the
/// run dir. A caller-owned `--prompt-file` is read but never mutated.
fn resolve_prompt_file(
    run_dir: &std::path::Path,
    src: PromptSource,
    preamble: &str,
) -> Result<PathBuf, CliError> {
    let brief = match src {
        PromptSource::Task(t) => t,
        PromptSource::File(p) => std::fs::read_to_string(&p).map_err(|e| {
            CliError::user(
                "prompt_file_not_readable",
                format!("could not read --prompt-file {}: {e}", p.display()),
            )
            .with_invalid_value(p.display().to_string())
        })?,
    };
    spawn::write_prompt_file(run_dir, &format!("{preamble}\n\n{brief}"))
}

/// Longest ASCII workmux handle that survives its tmux-window naming path.
///
/// `workmux` flattens the slash in `wt/<id>-<slug>` before creating its window.
/// Above this bound its created name is truncated, while native materializer correctly
/// looks up the untruncated branch-derived name. Keep the *input* to both sides
/// within the bound instead of loosening the native materializer's authoritative lookup.
const MAX_WORKMUX_WINDOW_NAME_BYTES: usize = 50;
const BRANCH_DISPLAY_ID_CHARS: usize = 10;
const BRANCH_PREFIX_BYTES: usize = "wt/".len() + BRANCH_DISPLAY_ID_CHARS + "-".len();
const MIN_BRANCH_SLUG_BYTES: usize = 16;
const _: () = assert!(BRANCH_DISPLAY_ID_CHARS <= RunId::LEN);
const _: () = assert!(BRANCH_PREFIX_BYTES + MIN_BRANCH_SLUG_BYTES <= MAX_WORKMUX_WINDOW_NAME_BYTES);

/// Build the bounded branch name handed to native materializer.
///
/// The display identifier uses the last ten ULID characters: 50 bits from the
/// randomness field. Taking the old first ten characters encoded only the
/// millisecond timestamp and let same-millisecond runs with the same retained
/// slug collide. The identifier is display metadata, not run identity;
/// ownership discovery uses the recorded worktree path and branch.
fn derive_branch_name(kind: Kind, run_id: &RunId, title: &str) -> String {
    // RunId guarantees a canonical 26-byte lowercase Crockford ULID, so this
    // fixed byte slice cannot underflow or split a character boundary.
    let entropy_start = RunId::LEN - BRANCH_DISPLAY_ID_CHARS;
    let display_id = &run_id.as_str()[entropy_start..];
    let prefix = format!("wt/{display_id}-");
    let slug: String = title
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() {
        kind_kebab(kind).to_string()
    } else {
        slug
    };
    // `slug` is ASCII-only, so char and byte lengths agree. Do not grow this
    // independently of the workmux window-name bound above.
    let max_slug_len = MAX_WORKMUX_WINDOW_NAME_BYTES.saturating_sub(prefix.len());
    let slug: String = slug
        .chars()
        .take(max_slug_len)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string();
    format!("{prefix}{slug}")
}

struct EmitInput<'a> {
    run_id: &'a str,
    dir: String,
    kind: Kind,
    lifecycle: Lifecycle,
    parent_run_id: Option<&'a str>,
    parent_node_id: Option<&'a str>,
    /// The id of this run's own primary node, surfaced as the envelope's
    /// `node_id` so callers can discover it without guessing. `Some("n-0001")`
    /// once the node exists on disk (materialized worker, or the synthetic
    /// orchestrate driver node); `None` for dry-run / test-skip skeletons.
    node_id: Option<&'a str>,
    spawn: Option<&'a SpawnResult>,
    supervisor_pid: Option<u32>,
    idempotent_replay: Option<bool>,
    dry_run: Option<bool>,
    selection: Option<&'a taskfleet_core::AgentSelection>,
    spec: &'a OutputSpec,
    warnings: &'a [String],
}

fn emit(i: EmitInput<'_>) -> Result<(), CliError> {
    let supervisor = match (i.supervisor_pid, i.dry_run, i.idempotent_replay) {
        (Some(pid), _, _) => SupervisorField::Pid(pid),
        (None, Some(true), _) => SupervisorField::Note("not-spawned-dry-run"),
        (None, _, Some(true)) => SupervisorField::Note("recorded-on-prior-run"),
        // Child-spawn: parent supervisor handles supervisor creation.
        (None, _, _) => SupervisorField::Note("delegated-to-parent-supervisor"),
    };
    let payload = CreatedPayload {
        run_id: i.run_id,
        dir: i.dir,
        supervisor,
        kind: i.kind.into(),
        lifecycle: i.lifecycle.into(),
        parent_run_id: i.parent_run_id,
        parent_node_id: i.parent_node_id,
        node_id: i.node_id,
        tmux_window: i.spawn.map(|s| s.tmux_window.as_str()),
        worktree_path: i.spawn.map(|s| s.worktree_path.as_str()),
        branch: i.spawn.map(|s| s.branch.as_str()),
        idempotent_replay: i.idempotent_replay,
        dry_run: i.dry_run,
        selection: i.selection.map_or(
            SelectionView::Legacy("legacy-unrecorded"),
            SelectionView::Recorded,
        ),
    };
    match i.spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => {
            output::emit_envelope(&payload, i.spec, i.warnings)?;
        }
        OutputFormat::Text => {
            println!("run-id: {}", payload.run_id);
            println!("dir:    {}", payload.dir);
            println!("kind:   {}", kind_kebab(i.kind));
            match &payload.selection {
                SelectionView::Recorded(selection) => {
                    println!(
                        "profile: {} (source {}, {})",
                        selection.profile, selection.selection_source, selection.interaction
                    );
                    if let Some(requested) = selection.requested_harness.as_deref() {
                        println!("requested-harness: {requested}");
                    }
                    println!(
                        "agent:   candidate {} / {}",
                        selection.selected.candidate_index, selection.selected.harness
                    );
                    for skipped in &selection.fallback {
                        println!(
                            "skipped: candidate {} / {} — {}",
                            skipped.candidate_index, skipped.harness, skipped.reason
                        );
                    }
                }
                SelectionView::Legacy(value) => println!("profile: {value}"),
            }
            match &payload.supervisor {
                SupervisorField::Pid(p) => println!("status: running  (supervisor pid {p})"),
                SupervisorField::Note(n) => println!("status: pending  (supervisor: {n})"),
            }
            if let Some(b) = payload.branch {
                println!("branch: {b}");
            }
            if let Some(w) = payload.worktree_path {
                println!("path:   {w}");
            }
            if let Some(t) = payload.tmux_window {
                println!("tmux:   {t}");
            }
            if let (Some(p), Some(n)) = (payload.parent_run_id, payload.parent_node_id) {
                println!("parent: {p}/{n}");
            }
            if payload.idempotent_replay == Some(true) {
                println!("note:   returned from idempotency-key cache");
            }
            if payload.dry_run == Some(true) {
                println!("note:   --dry-run (no filesystem changes)");
            }
            output::emit_text_warnings(i.warnings);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch_name(kind: Kind, run_id: &str, title: &str) -> String {
        derive_branch_name(kind, &parse_run_id(run_id).unwrap(), title)
    }

    #[test]
    fn dead_creator_reservation_is_reclaimable_without_waiting() {
        let root = tempfile::TempDir::new().unwrap();
        let started = Utc::now();
        let record = idempotency::ReservationRecord::new(
            "01jxsnap000000000000000000",
            idempotency::CreatorLease {
                pid: 42,
                pid_start_secs: Some(100),
                started_at: started,
                materializer_lease_path: None,
            },
        );
        assert_eq!(
            classify_existing_reservation_with(root.path(), &record, started, |pid, start| {
                assert_eq!((pid, start), (42, Some(100)));
                false
            })
            .unwrap(),
            ExistingReservation::CreatorDead
        );
    }

    #[test]
    fn live_creator_reservation_is_never_reclaimed() {
        let root = tempfile::TempDir::new().unwrap();
        let started = Utc::now();
        let record = idempotency::ReservationRecord::new(
            "01jxsnap000000000000000000",
            idempotency::CreatorLease {
                pid: 42,
                pid_start_secs: Some(100),
                started_at: started,
                materializer_lease_path: None,
            },
        );
        assert_eq!(
            classify_existing_reservation_with(root.path(), &record, started, |_, _| true).unwrap(),
            ExistingReservation::CreatorLive
        );
    }

    #[test]
    fn stale_live_pid_without_start_identity_fails_closed() {
        let root = tempfile::TempDir::new().unwrap();
        let started = Utc::now() - Duration::minutes(31);
        let record = idempotency::ReservationRecord::new(
            "01jxsnap000000000000000000",
            idempotency::CreatorLease {
                pid: 42,
                pid_start_secs: None,
                started_at: started,
                materializer_lease_path: None,
            },
        );
        assert_eq!(
            classify_existing_reservation_with(root.path(), &record, Utc::now(), |_, _| true)
                .unwrap(),
            ExistingReservation::Unverifiable
        );
    }

    #[test]
    fn published_reservation_replays_even_when_creator_is_dead() {
        let root = tempfile::TempDir::new().unwrap();
        let started = Utc::now();
        let record = idempotency::ReservationRecord::new(
            "01jxsnap000000000000000000",
            idempotency::CreatorLease {
                pid: 42,
                pid_start_secs: Some(100),
                started_at: started,
                materializer_lease_path: None,
            },
        );
        let dir = root.path().join("runs").join(&record.run_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), "published").unwrap();
        assert_eq!(
            classify_existing_reservation_with(root.path(), &record, started, |_, _| {
                panic!("published state must win before liveness probe")
            })
            .unwrap(),
            ExistingReservation::Published(dir)
        );
    }

    #[test]
    fn child_edge_repair_recognizes_legacy_unkeyed_event() {
        let tmp = tempfile::TempDir::new().unwrap();
        let run_id = "01jxsnap000000000000000000";
        let dir = tmp.path().join(run_id);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = taskfleet_core::RunPaths::new(dir, run_id).unwrap();
        let node = parse_node_id("n-0001").unwrap();
        taskfleet_core::append_and_apply_event(
            &paths,
            "run.created",
            None,
            None,
            json!({"kind":"spinoff","lifecycle":"autonomous","title":"parent"}),
        )
        .unwrap();
        taskfleet_core::append_and_apply_event(
            &paths,
            "node.created",
            Some(&node),
            None,
            json!({"kind":"spinoff"}),
        )
        .unwrap();
        let child_id = "01jxsnap000000000000000001";
        let data = json!({
            "child_run_id": child_id,
            "child_node_id": "n-0001",
            "child_kind": "spinoff",
            "child_title": "child"
        });
        taskfleet_core::append_and_apply_event(
            &paths,
            "child.spawned",
            Some(&node),
            None,
            data.clone(),
        )
        .unwrap();

        append_child_spawned_if_missing(
            &paths,
            &node,
            child_id,
            &format!("child-spawned:{child_id}"),
            data,
        )
        .unwrap();
        let events = taskfleet_core::read_all_events(&paths.events()).unwrap();
        assert_eq!(
            events.iter().filter(|e| e.kind == "child.spawned").count(),
            1
        );
    }

    #[test]
    fn derive_branch_basic() {
        let b = branch_name(
            Kind::Spinoff,
            "01jx1ns7h8aaaaa9bbbbbbbbbb",
            "Login redirect bug",
        );
        assert_eq!(b, "wt/bbbbbbbbbb-login-redirect-bug");
    }

    #[test]
    fn derive_branch_uses_entropy_for_same_millisecond_and_retained_slug() {
        let timestamp = "01jx1ns7h8";
        let first = format!("{timestamp}0000000000000000");
        let second = format!("{timestamp}0000000000000001");
        let shared_prefix = "same normalized title ".repeat(4);
        let first_title = format!("{shared_prefix}first tail");
        let second_title = format!("{shared_prefix}second tail");

        let first_branch = branch_name(Kind::Spinoff, &first, &first_title);
        let second_branch = branch_name(Kind::Spinoff, &second, &second_title);

        assert_ne!(first_branch, second_branch);
        assert_eq!(first_branch.len(), MAX_WORKMUX_WINDOW_NAME_BYTES);
        assert_eq!(second_branch.len(), MAX_WORKMUX_WINDOW_NAME_BYTES);
        assert_eq!(
            first_branch,
            "wt/0000000000-same-normalized-title-same-normalize"
        );
        assert_eq!(
            second_branch,
            "wt/0000000001-same-normalized-title-same-normalize"
        );
    }

    #[test]
    fn derive_branch_uses_the_ulid_randomness_suffix_not_a_resolvable_prefix() {
        let run_id = "01jx1ns7h8aaaaa9bbbbbbbbbb";
        let branch = branch_name(Kind::Spinoff, run_id, "t");
        let display_id = branch
            .strip_prefix("wt/")
            .unwrap()
            .split('-')
            .next()
            .unwrap();

        assert_eq!(display_id, &run_id[RunId::LEN - BRANCH_DISPLAY_ID_CHARS..]);
        assert!(!run_id.starts_with(display_id));
    }

    #[test]
    fn derive_branch_empty_title_uses_kind() {
        let b = branch_name(Kind::Research, "01jx1234567890abcde1234567", "   !!!  ");
        assert!(b.ends_with("research"));
    }

    #[test]
    fn derive_branch_caps_the_flat_workmux_window_name_at_exact_boundary() {
        let run_id = "01jx1ns7h8aaaaa9bbbbbbbbbb";
        let at_limit = "x".repeat(36);
        let over_limit = format!("{at_limit}y");
        let expected = format!("wt/bbbbbbbbbb-{at_limit}");

        assert_eq!(branch_name(Kind::Spinoff, run_id, &at_limit), expected);
        assert_eq!(expected.len(), MAX_WORKMUX_WINDOW_NAME_BYTES);
        assert_eq!(
            branch_name(Kind::Spinoff, run_id, &over_limit),
            expected,
            "one byte over the workmux limit must derive the same bounded name"
        );
    }

    #[test]
    fn derive_branch_normalizes_unicode_whitespace_and_quotes_before_bounding() {
        let b = branch_name(
            Kind::Spinoff,
            "01jx1ns7h8aaaaa9bbbbbbbbbb",
            "  Café \"quoted\"  and ‘spaced’  ",
        );
        assert_eq!(b, "wt/bbbbbbbbbb-caf-quoted-and-spaced");
        assert!(b.is_ascii());
        assert!(!b.contains(char::is_whitespace));
        assert!(!b.contains(['\'', '\"']));
    }

    #[test]
    fn derive_branch_drops_a_separator_at_the_truncation_boundary() {
        let title = format!("{} y", "x".repeat(35));
        let branch = branch_name(Kind::Spinoff, "01jx1ns7h8aaaaa9bbbbbbbbbb", &title);

        assert_eq!(branch, format!("wt/bbbbbbbbbb-{}", "x".repeat(35)));
        assert_eq!(branch.len(), MAX_WORKMUX_WINDOW_NAME_BYTES - 1);
        assert!(!branch.ends_with('-'));
    }

    #[test]
    fn missing_task_and_prompt_file_errors() {
        let e = resolve_prompt_source(None, None).unwrap_err();
        assert_eq!(e.code, "missing-task-or-prompt-file");
    }

    #[test]
    fn task_and_prompt_file_conflict() {
        let e = resolve_prompt_source(Some("x"), Some("/tmp/p.md")).unwrap_err();
        assert_eq!(e.code, "invalid_arguments");
    }

    #[test]
    fn empty_task_rejected() {
        let e = resolve_prompt_source(Some("   "), None).unwrap_err();
        assert_eq!(e.code, "invalid_value");
    }

    #[test]
    fn resolve_prompt_file_task_with_preamble_prepends() {
        let dir = tempfile::TempDir::new().unwrap();
        let preamble =
            crate::harness::prompt::worker_prompt_preamble("pi", Kind::Research, "01JXRUNID000");
        let p = resolve_prompt_file(
            dir.path(),
            PromptSource::Task("research WAL implementations".into()),
            &preamble,
        )
        .unwrap();
        let body = std::fs::read_to_string(p).unwrap();
        // Common run context leads, then the harness shim, then the brief.
        assert!(body.starts_with("# Taskfleet run context"));
        assert!(body.contains("# Operating note — pi research worker"));
        assert!(body.contains("taskfleet run merge 01JXRUNID000"));
        assert!(body.trim_end().ends_with("research WAL implementations"));
    }

    #[test]
    fn resolve_prompt_file_with_preamble_never_mutates_caller_file() {
        // With a preamble, a --prompt-file is read and the derived prompt is
        // written into the run dir — the caller's file stays untouched.
        let dir = tempfile::TempDir::new().unwrap();
        let caller = dir.path().join("caller-prompt.md");
        std::fs::write(&caller, "caller-owned brief").unwrap();
        let preamble =
            crate::harness::prompt::worker_prompt_preamble("pi", Kind::Research, "01JXRUNID000");
        let p =
            resolve_prompt_file(dir.path(), PromptSource::File(caller.clone()), &preamble).unwrap();
        assert_ne!(
            p, caller,
            "derived prompt must be a new file in the run dir"
        );
        assert_eq!(
            std::fs::read_to_string(&caller).unwrap(),
            "caller-owned brief"
        );
        let body = std::fs::read_to_string(p).unwrap();
        assert!(body.starts_with("# Taskfleet run context"));
        assert!(body.contains("# Operating note — pi research worker"));
        assert!(body.trim_end().ends_with("caller-owned brief"));
    }

    #[test]
    fn parent_session_defaults_to_none() {
        assert_eq!(resolve_parent_session(false, None).unwrap(), None);
    }

    #[test]
    fn headless_yields_default_session() {
        assert_eq!(
            resolve_parent_session(true, None).unwrap().as_deref(),
            Some("headless")
        );
    }

    #[test]
    fn explicit_tmux_session_wins_and_implies_headless() {
        // Explicit name overrides the default even with --headless unset.
        assert_eq!(
            resolve_parent_session(false, Some("campaign"))
                .unwrap()
                .as_deref(),
            Some("campaign")
        );
        assert_eq!(
            resolve_parent_session(true, Some("campaign"))
                .unwrap()
                .as_deref(),
            Some("campaign")
        );
    }

    #[test]
    fn tmux_session_trimmed() {
        assert_eq!(
            resolve_parent_session(false, Some("  bg  "))
                .unwrap()
                .as_deref(),
            Some("bg")
        );
    }

    #[test]
    fn empty_tmux_session_rejected() {
        let e = resolve_parent_session(false, Some("   ")).unwrap_err();
        assert_eq!(e.code, "invalid_value");
    }

    #[test]
    fn tmux_session_with_separator_rejected() {
        for bad in ["a:b", "a.b", "a b"] {
            let e = resolve_parent_session(false, Some(bad)).unwrap_err();
            assert_eq!(e.code, "invalid_value", "expected reject for {bad:?}");
        }
    }
}
