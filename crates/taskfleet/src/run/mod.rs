//! `run` subcommand — top-level lifecycle for taskfleet runs.
//!
//! Sets the noun-module pattern that `node`, `event`, `discussion`,
//! `spinoff` will follow: one file per verb, shared types in `mod.rs`,
//! single `dispatch` entry point called from `cli.rs`.

pub mod attention;
pub mod awaiting_input;
pub mod cancel;
pub mod create;
pub mod discard;
pub mod dto;
pub mod false_failed;
pub mod landed;
pub mod list;
pub mod merge;
pub mod merge_recovery;
pub mod ownership;
pub mod reattach;
pub mod retained;
pub mod salvage;
pub mod show;
pub mod spawn;
pub mod stalled;
pub mod supervisor_readiness;
pub mod supervisor_spawn;
pub mod telemetry;
pub mod wait;
pub mod worker;

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Subcommand, ValueEnum};
use taskfleet_core::{
    is_run_id_prefix, IdValidationError, Kind, Lifecycle, NodeId, RunId, RunPaths,
};

use crate::error::CliError;
use crate::output::OutputSpec;

/// The creatable run kinds accepted by `run create --kind`. The 0.2 cut removed
/// `code` / `orchestrate` / `orchestrated` / `bugfix` / `make-skill`; the
/// read-only [`Kind::Unknown`] catch-all is deliberately not an input.
#[derive(Debug, Clone, Copy, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum KindArg {
    Spinoff,
    Research,
    TechnicalDecision,
    FanOut,
}

impl From<KindArg> for Kind {
    fn from(k: KindArg) -> Self {
        match k {
            KindArg::Spinoff => Kind::Spinoff,
            KindArg::Research => Kind::Research,
            KindArg::TechnicalDecision => Kind::TechnicalDecision,
            KindArg::FanOut => Kind::FanOut,
        }
    }
}

// The `Create` variant is a wide clap arg-bag (~300 bytes of flags) next to
// small verb variants — the classic `large_enum_variant` shape. It is parsed
// exactly once per process and immediately destructured in `dispatch`; it is
// never stored in a collection or moved on a hot path, so the size gap costs
// nothing. Boxing a clap `#[derive(Subcommand)]` struct-variant is not
// expressible without splitting it into a separate `Args` struct, which buys no
// runtime benefit here — allow the lint deliberately.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand, Debug)]
pub enum RunAction {
    /// Create a new run. Top-level when `--parent-*` flags are absent,
    /// child-spawn when both are set (mutually required).
    Create {
        #[arg(long, value_enum)]
        kind: KindArg,
        #[arg(long)]
        title: String,
        #[arg(long)]
        source_repo: Option<String>,
        #[arg(long)]
        source_branch: Option<String>,
        #[arg(long, conflicts_with = "prompt_file")]
        task: Option<String>,
        /// Path to a prompt file (instead of inlining via --task). Used
        /// as-is and handed to native materializer.
        #[arg(long)]
        prompt_file: Option<String>,
        /// Workmux layout name; forwarded to native materializer as `-l <name>`.
        #[arg(long)]
        layout: Option<String>,
        /// Agent runtime to launch the worker under: `pi` (built-in default) |
        /// `claude` (non-default opt-in). Overrides `TASKFLEET_HARNESS`,
        /// the `config.toml` `[harness]` default, and the built-in default (in
        /// that precedence order). Without an executable profile, Taskfleet
        /// records and launches the corresponding `pi` or `claude` executable
        /// from PATH through its private generated workmux launcher. Recorded
        /// on the run and shown by `run show` / `run list --json`.
        #[arg(long)]
        harness: Option<String>,
        /// Select a user-owned executable profile from
        /// `$TASKFLEET_HOME/config.toml`.
        /// Overrides environment and
        /// repository/user defaults. Conflicts with the legacy `--harness`
        /// alias at the same precedence level.
        #[arg(long)]
        profile: Option<String>,
        /// Mark this run **interactive** (human-driven). The supervisor then
        /// never auto-terminalizes or auto-tears-down from a dead pid or a worker
        /// exit — it waits for an explicit `run merge` (→ teardown) or `run
        /// cancel`; the human owns the whole lifecycle (design.md §6). Opt-in;
        /// omit it (the default) for an autonomous fire-and-forget worker. This is
        /// the explicit how-run state that replaced the removed `code` kind —
        /// orthogonal to `--kind`, so any topology can be interactive.
        #[arg(long)]
        interactive: bool,
        /// Skip workmux post-create hooks; forwarded to native materializer.
        #[arg(long)]
        no_hooks: bool,
        /// Spawn the worker's tmux window in a detached "headless"
        /// session instead of the foreground one, so a campaign of many
        /// spawns does not clutter the user's window list. Attach later
        /// with `tmux attach -t headless`. Opt-in; default is foreground.
        #[arg(long)]
        headless: bool,
        /// Explicit tmux session name for the worker's window. Implies
        /// headless placement and overrides `--headless`'s default
        /// session name. Forwarded to native materializer / workmux as
        /// `--parent-session <name>`.
        #[arg(long)]
        tmux_session: Option<String>,
        /// Seconds Taskfleet waits for the freshly launched candidate's private
        /// PID handshake before rolling materialization back. Range 1–600.
        /// Defaults to 90 because Taskfleet spawns are frequently part of
        /// high-fan-out batches that
        /// self-load the host; bump it further on an already-loaded box.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=600), default_value_t = 90)]
        agent_startup_timeout: u32,
        #[arg(long, requires = "parent_node_id")]
        parent_run_id: Option<String>,
        #[arg(long, requires = "parent_run_id")]
        parent_node_id: Option<String>,
        /// Shell command the supervisor runs when this run reaches a terminal
        /// state (`done | failed | cancelled`), BEFORE teardown. Runs via
        /// `sh -c <cmd>` with `TASKFLEET_RUN_ID`, `TASKFLEET_STATUS`, `TASKFLEET_SUMMARY`,
        /// `TASKFLEET_RUN_KIND`, and `TASKFLEET_RUN_TITLE` in the environment — so a
        /// spawning session can learn of completion without polling (e.g.
        /// append a line to a file the harness watches, or post a desktop
        /// notification). At-least-once: deduped on a durable `run.notified`
        /// marker (so the healthy path fires once), but a supervisor crash in
        /// the window between firing and recording the marker re-fires on
        /// restart — a duplicate is preferred over a missed notification, so
        /// the command should tolerate running more than once.
        #[arg(long)]
        notify: Option<String>,
        #[arg(long)]
        idempotency_key: Option<String>,
        #[arg(long)]
        dry_run: bool,
        /// **Test-only.** Skip the native materializer shell-out and supervisor
        /// spawn; produce only the on-disk run skeleton (manifest +
        /// run.created event). Hidden from `--help`. Never set this in
        /// production — the run will be missing its worktree, tmux
        /// window, and supervisor.
        #[arg(long, hide = true)]
        skip_materialize: bool,
    },
    /// List runs on disk.
    List {
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        /// Include only runs whose recorded source repository belongs to the git
        /// repository containing this path. Accepts `.` and paths inside linked
        /// worktrees. Runs without recorded repository identity do not match.
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Show one run's manifest and counters. Pass an id, or use `--current`
    /// inside a worker worktree to resolve its exact owning run from durable
    /// node ownership metadata (never from the branch's short run-id prefix).
    Show {
        #[arg(required_unless_present = "current", conflicts_with = "current")]
        run_id: Option<String>,
        /// Resolve the exact run whose node records the current git worktree and
        /// checked-out branch. Missing, duplicate, stale, or malformed evidence
        /// is an error; this never falls back to a run-id prefix.
        #[arg(long)]
        current: bool,
    },
    /// Cancel a run (all live nodes → `run.status: cancelled`), or a single
    /// live node with `--node <id>` (branch-preserving; the run stays live
    /// while siblings run and the supervisor rolls it up once every node
    /// settles). Idempotent.
    Cancel {
        run_id: String,
        /// Cancel exactly ONE live node (e.g. `n-0002`) instead of the whole
        /// run — for unblocking a single stuck fan-out child without killing
        /// the batch. The node's branch + worktree are preserved
        /// (source-relative teardown); the run is NOT terminalized while other
        /// nodes remain live.
        #[arg(long)]
        node: Option<String>,
        #[arg(long)]
        note: Option<String>,
    },
    /// Merge a worktree run's branch back to its source, then submit the
    /// terminal `node.report` so the supervisor winds the run down and
    /// tears the worktree/window/branch down. Owns the full merge
    /// lifecycle: rebase + merge (via the bundled merge backend) AND the
    /// report, in one call.
    Merge {
        run_id: String,
        /// Merge target branch. Defaults to the run's recorded
        /// `source_branch`, then to main/master auto-detection.
        #[arg(long)]
        source: Option<String>,
        /// Reporting node id (defaults to `n-0001`).
        #[arg(long)]
        node_id: Option<String>,
        /// Optional §7.3 report payload (JSON file) to submit on a clean
        /// merge. Lets an autonomous kind carry its rich `discussion_items`
        /// / `spinoff_proposals` / `wrap_up_recommendations` in the same
        /// call. `run merge` stamps it `via: "explicit-merge"`. Omit it for
        /// a minimal `{success, summary}` report.
        #[arg(long)]
        report_file: Option<std::path::PathBuf>,
        /// Resolve inputs and report the planned merge without running it
        /// or appending any event.
        #[arg(long)]
        dry_run: bool,
    },
    /// Salvage a stuck single-worker run: fence its prior worker (verifying
    /// identity first) and drive `run merge` from the preserved worktree's
    /// current git state — the fenced manual finish for an
    /// `attention-required` run (a worker that exited cleanly but skipped
    /// `run merge`) or a `failed` run whose branch was preserved. Refuses an
    /// already-`done`/`cancelled` run, a multi-node run, a run with no
    /// preserved worktree/branch, and a live worker it cannot safely fence.
    Salvage {
        run_id: String,
        /// Merge target branch. Defaults to the run's recorded `source_branch`.
        #[arg(long)]
        source: Option<String>,
        /// Optional §7.3 report payload (JSON file) to submit on the salvage
        /// merge, forwarded to `run merge` (stamped `via: "explicit-merge"`).
        #[arg(long)]
        report_file: Option<std::path::PathBuf>,
        /// Permit fencing a *live* worker: SIGTERM the recorded agent pid
        /// (only ever when its start-time identity matches). Without it, a
        /// still-alive original worker is a refusal — salvage never kills a
        /// running worker implicitly.
        #[arg(long)]
        fence: bool,
        /// Resolve inputs and report the planned salvage (worker state,
        /// whether a fence would fire, the planned merge) without fencing or
        /// merging anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove one failed/cancelled node's retained worktree and branch after
    /// recording explicit durable authorization. This never changes run status
    /// or marks work landed.
    Discard {
        run_id: String,
        /// Select one retained node. Required when more than one is present.
        #[arg(long)]
        node: Option<String>,
        /// Non-empty audit reason for permanently deleting the retained work.
        #[arg(long)]
        reason: String,
        /// Acknowledge deletion of verified staged, modified, and untracked
        /// work. Never bypasses identity or Git-verification failures.
        #[arg(long)]
        force: bool,
        /// Perform all read-only eligibility checks and report the plan without
        /// recording authorization or deleting resources.
        #[arg(long)]
        dry_run: bool,
    },
    /// Block until one or more runs reach a terminal state
    /// (`done | failed | cancelled`) and emit a structured summary, so
    /// callers stop hand-rolling `run show` poll loops. Read-only: never
    /// mutates run state. Exit codes: `0` condition met, `1` usage/unknown
    /// run, `2` timeout, `3` (`--fail-on-error`) a settled run failed.
    Wait {
        /// One or more run ids to wait on.
        #[arg(required = true, num_args = 1..)]
        run_id: Vec<String>,
        /// Return once *every* listed run is terminal (default).
        #[arg(long, conflicts_with = "any")]
        all: bool,
        /// Return as soon as *one* listed run is terminal.
        #[arg(long)]
        any: bool,
        /// Give up after this duration (e.g. `30s`, `5m`, `1h`; a bare integer
        /// is seconds, so `2400` == `2400sec`); exit code `2` distinguishes
        /// timeout from a met condition. Defaults to `6h` — a sane ceiling so a
        /// wait on a stuck run can never block an orchestrator forever; pass a
        /// larger value for a long campaign.
        #[arg(long, value_parser = wait::parse_duration, default_value = "6h")]
        timeout: Option<Duration>,
        /// Exit `3` if the condition is met but a settled run was
        /// `failed`/`cancelled` (default: exit `0` for any terminal state).
        #[arg(long)]
        fail_on_error: bool,
        /// Emit one JSONL line per run state-transition to stderr for live UIs.
        #[arg(long)]
        progress: bool,
        /// Override the internal poll cadence (default: bounded backoff,
        /// 100ms→2s). Callers shouldn't normally need this.
        #[arg(long, value_parser = wait::parse_duration)]
        poll_interval: Option<Duration>,
    },
    /// Restart the run's supervisor process. Refuses if the recorded
    /// supervisor PID is still alive. Spawns `taskfleet supervise
    /// <run-id>` detached with stdout/stderr → `supervisor.stderr.log`.
    Reattach {
        run_id: String,
        /// Pass `--once` to the spawned supervisor (test-only).
        #[arg(long, hide = true)]
        once: bool,
        /// Pass `--max-iter <n>` to the spawned supervisor (test-only).
        #[arg(long, hide = true)]
        max_iter: Option<u32>,
    },
}

pub fn dispatch(action: RunAction, spec: &OutputSpec, warnings: &[String]) -> Result<(), CliError> {
    match action {
        RunAction::Create {
            kind,
            title,
            source_repo,
            source_branch,
            task,
            prompt_file,
            layout,
            harness,
            profile,
            interactive,
            no_hooks,
            headless,
            tmux_session,
            agent_startup_timeout,
            parent_run_id,
            parent_node_id,
            notify,
            idempotency_key,
            dry_run,
            skip_materialize,
        } => create::run(create::Args {
            skip_materialize,
            kind: kind.into(),
            title,
            source_repo,
            source_branch,
            task,
            prompt_file,
            layout,
            harness,
            profile,
            interactive,
            no_hooks,
            headless,
            tmux_session,
            agent_startup_timeout,
            parent_run_id,
            parent_node_id,
            notify,
            idempotency_key,
            dry_run,
            spec,
            warnings,
        }),
        RunAction::List { status, kind, repo } => list::run(list::Args {
            status,
            kind,
            repo,
            spec,
            warnings,
        }),
        RunAction::Show { run_id, current } => {
            let run_id = if current {
                let root = crate::home::root_dir()?;
                let cwd = std::env::current_dir().map_err(|e| {
                    CliError::system("io_error", format!("read current directory: {e}"))
                })?;
                ownership::resolve_current(&root, &cwd)?.to_string()
            } else {
                run_id.expect("clap requires run_id unless --current")
            };
            show::run(&run_id, spec, warnings)
        }
        RunAction::Cancel { run_id, node, note } => {
            cancel::run(&run_id, node.as_deref(), note.as_deref(), spec, warnings)
        }
        RunAction::Merge {
            run_id,
            source,
            node_id,
            report_file,
            dry_run,
        } => merge::run(merge::Args {
            run_id,
            source,
            node_id,
            report_file,
            dry_run,
            spec,
            warnings,
        }),
        RunAction::Salvage {
            run_id,
            source,
            report_file,
            fence,
            dry_run,
        } => salvage::run(salvage::Args {
            run_id,
            source,
            report_file,
            fence,
            dry_run,
            spec,
            warnings,
        }),
        RunAction::Discard {
            run_id,
            node,
            reason,
            force,
            dry_run,
        } => discard::run(discard::Args {
            run_id,
            node,
            reason,
            force,
            dry_run,
            spec,
            warnings,
        }),
        RunAction::Wait {
            run_id,
            all: _,
            any,
            timeout,
            fail_on_error,
            progress,
            poll_interval,
        } => wait::run(wait::Args {
            run_ids: run_id,
            any,
            timeout,
            fail_on_error,
            progress,
            poll_interval,
            spec,
            warnings,
        }),
        RunAction::Reattach {
            run_id,
            once,
            max_iter,
        } => reattach::run(&run_id, once, max_iter, spec, warnings),
    }
}

/// Map a `Kind` to its default `Lifecycle`. Thin alias over
/// [`Kind::lifecycle`] so CLI call sites keep their existing
/// free-function spelling while the source of truth lives in core.
pub fn lifecycle_for(k: Kind) -> Lifecycle {
    k.lifecycle()
}

/// A parsed-but-unresolved CLI run selector: either a fully-validated exact
/// [`RunId`] or a well-formed shorter *prefix* (like `git` short SHAs) awaiting a
/// directory scan.
///
/// Prefix (fuzzy) resolution is a **CLI-only** concern — it exists so a human or
/// AI caller can address a run by an unambiguous short id. Internal, supervisor,
/// reducer, and event-data paths never construct a [`RunSelector::Prefix`]: they
/// already hold a typed [`RunId`] and call [`run_paths_exact`] directly, so a
/// truncated id can never silently fuzzy-resolve to the wrong run (a
/// confused-deputy risk). A `Prefix` is only ever produced by
/// [`RunSelector::parse`] on a raw CLI argument and only ever resolved at verb
/// entry via [`RunSelector::resolve`] (or the [`run_paths_from_cli_arg`]
/// convenience).
#[derive(Debug)]
pub enum RunSelector {
    /// A fully-validated 26-char ULID — resolves with no directory scan.
    Exact(RunId),
    /// A well-formed shorter run-id prefix awaiting resolution against
    /// `<root>/runs/`. The payload is [`RunIdPrefix`], whose private field can
    /// only be built by [`RunSelector::parse`] — so no code outside this module
    /// can hand-construct a prefix and feed it to [`RunSelector::resolve`], which
    /// is what makes fuzzy resolution a *type-level* CLI-only concern rather than
    /// a convention.
    Prefix(RunIdPrefix),
}

/// A well-formed run-id prefix, constructible **only** via
/// [`RunSelector::parse`] (the private field seals it). This is the type-level
/// half of the confused-deputy guarantee: an internal caller cannot fabricate a
/// `RunSelector::Prefix` to route a truncated id through fuzzy resolution.
#[derive(Debug)]
pub struct RunIdPrefix(String);

impl RunIdPrefix {
    fn as_str(&self) -> &str {
        &self.0
    }
}

impl RunSelector {
    /// Classify a raw CLI run-id argument as an exact ULID or a well-formed
    /// prefix, rejecting malformed input up front.
    ///
    /// - A full-length value must be an exact valid ULID → [`RunSelector::Exact`]
    ///   (no scan needed to resolve later), preserving the existing exact-id
    ///   behaviour and each caller's own valid-but-missing `run_not_found`.
    /// - A shorter, well-formed value becomes a [`RunSelector::Prefix`] to be
    ///   matched against the run directories under `<root>/runs/` at resolve time.
    /// - A malformed value (empty, non-Crockford char, impossible leading digit,
    ///   over-length non-ULID) surfaces `invalid_run_id`, keeping a typo distinct
    ///   from a well-formed-but-unknown prefix.
    pub fn parse(arg: &str) -> Result<Self, CliError> {
        // Full-length (or longer): must be an exact ULID. A 26-char string that is
        // not a valid ULID (wrong charset, timestamp overflow) stays
        // `invalid_run_id` rather than being reinterpreted as a length-26 prefix
        // that matches nothing.
        if arg.len() >= RunId::LEN {
            return RunId::parse_str(arg).map(RunSelector::Exact).map_err(|e| {
                CliError::user(
                    "invalid_run_id",
                    format!("run id {arg:?} is not a valid ULID: {e}"),
                )
                .with_invalid_value(arg)
            });
        }
        // Shorter than a ULID: a prefix. Reject a malformed prefix up front so a
        // typo is `invalid_run_id`, not a silent no-match.
        if !is_run_id_prefix(arg) {
            return Err(CliError::user(
                "invalid_run_id",
                format!(
                    "run id {arg:?} is not a valid ULID or run-id prefix: \
                     expected up to {} lowercase Crockford base32 characters (leading 0-7)",
                    RunId::LEN
                ),
            )
            .with_invalid_value(arg));
        }
        Ok(RunSelector::Prefix(RunIdPrefix(arg.to_string())))
    }

    /// Resolve to a typed [`RunId`], scanning `<root>/runs/` **only** for a
    /// [`RunSelector::Prefix`]. An [`RunSelector::Exact`] returns verbatim with no
    /// scan, so the supervisor's hot lookups (which always pass full child ids)
    /// pay nothing — but they get there through [`run_paths_exact`], never here.
    pub fn resolve(self, root: &Path) -> Result<RunId, CliError> {
        match self {
            RunSelector::Exact(rid) => Ok(rid),
            RunSelector::Prefix(prefix) => resolve_prefix(root, prefix.as_str()),
        }
    }
}

/// Scan `<root>/runs/` for the single run whose id starts with `arg` (assumed a
/// well-formed prefix — [`RunSelector::parse`] has already rejected malformed
/// input): exactly one match resolves; several match surfaces `ambiguous_run_id`
/// (listing the candidates in `expected`); none match surfaces `run_not_found`.
///
/// The prefix scan is a best-effort read of the runs directory, deliberately NOT
/// under a namespace lock (no such lock exists — a run is not known until after
/// the scan). It fails *closed*: a `read_dir` iteration error propagates as
/// `io_error` rather than dropping a candidate, so an ambiguous prefix can never
/// be silently narrowed to a single (wrong) match. A run created or torn down
/// concurrently with the scan is an accepted race — a resolved id that is then
/// deleted before the caller locks it surfaces as the caller's own
/// `run_not_found` (see e.g. `cancel`'s `NotFound` handling).
fn resolve_prefix(root: &Path, arg: &str) -> Result<RunId, CliError> {
    let runs_dir = runs_root(root);
    let entries = match std::fs::read_dir(&runs_dir) {
        Ok(entries) => entries,
        // No runs dir yet ⇒ no run can match ⇒ not-found (not a system error).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(prefix_not_found(arg)),
        Err(e) => {
            return Err(CliError::system(
                "io_error",
                format!("read_dir {}: {}", runs_dir.display(), e),
            ))
        }
    };
    // Fail closed: propagate a per-entry iteration error instead of dropping the
    // entry (a dropped candidate could turn an ambiguous prefix into a falsely
    // unique one and then act on the wrong run).
    let mut matches: Vec<RunId> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| {
            CliError::system(
                "io_error",
                format!("read_dir {}: {}", runs_dir.display(), e),
            )
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        // Match on the entry NAME being a valid run id sharing the prefix; a
        // foreign dir (non-ULID name) can never be resolved to. Entry type is not
        // filtered — an exact-id lookup lets `from_validated` surface a
        // `corrupt_run` for a symlinked run dir, so counting it as a candidate
        // keeps prefix and exact addressing consistent for that corruption case.
        if name.starts_with(arg) {
            if let Ok(rid) = RunId::parse_str(&name) {
                matches.push(rid);
            }
        }
    }
    match matches.len() {
        0 => Err(prefix_not_found(arg)),
        1 => Ok(matches.pop().expect("len checked == 1")),
        n => {
            // Sort only here, where the candidate list is actually emitted, so a
            // deterministic error is presented without paying for a sort on the
            // common unique / not-found paths.
            matches.sort();
            Err(CliError::user(
                "ambiguous_run_id",
                format!(
                    "run id prefix {arg:?} matches {n} runs; use more characters to disambiguate"
                ),
            )
            .with_invalid_value(arg)
            .with_expected(serde_json::Value::Array(
                matches
                    .into_iter()
                    .map(|r| serde_json::Value::String(r.as_str().to_string()))
                    .collect(),
            )))
        }
    }
}

/// `run_not_found` for a well-formed prefix that matched no run.
fn prefix_not_found(arg: &str) -> CliError {
    CliError::user(
        "run_not_found",
        format!("no run matching id prefix {arg:?}"),
    )
    .with_invalid_value(arg)
}

/// Resolve `<root>/runs/<run-id>` from a **typed** [`RunId`] and return validated
/// `RunPaths` — **exact-only, no directory scan**.
///
/// This is the path helper every internal, supervisor, reducer, and event-data
/// call site uses: they already hold a validated [`RunId`], so there is no
/// truncated string that could fuzzy-resolve to the wrong run. Prefix
/// resolution is deliberately *not* reachable from here — it lives only in
/// [`RunSelector`] / [`run_paths_from_cli_arg`], gated to CLI verb entry.
///
/// `run_dir` only accepts a `RunId`, so a `..`/absolute component can never
/// reach the filesystem, and `from_validated` runs the symlink-root guard (a
/// symlinked run dir maps to the `corrupt_run` envelope rather than being
/// silently followed).
///
/// Scope of the guarantee: this closes the *truncation → prefix* confused-deputy
/// route only. It does NOT prove the named run is actually a child of / related
/// to the caller — a corrupt or forged event carrying a *valid but unrelated*
/// full ULID still resolves to that other run. Reciprocal parent/child
/// validation is separate hardening (see the `run-paths-typed-selector-split`
/// spinoffs), not something the id type can enforce.
pub fn run_paths_exact(root: &Path, run_id: &RunId) -> Result<RunPaths, CliError> {
    let dir = taskfleet_core::run_dir(root, run_id);
    RunPaths::from_validated(dir, run_id.clone()).map_err(from_core)
}

/// Resolve `<root>/runs/<run-id>` from a raw **CLI** run-id argument and return
/// validated `RunPaths`.
///
/// Accepts an unambiguous run-id prefix as well as a full ULID (see
/// [`RunSelector`]) — this is the CLI verb-entry chokepoint for verbs that
/// intentionally accept a human/AI-entered run *selector*, so prefix acceptance
/// is uniform across them. It is `pub(crate)` and named for its role so an
/// internal caller does not reach for it by muscle memory: internal, supervisor,
/// reducer, and event-data paths hold a typed [`RunId`] and call
/// [`run_paths_exact`], which cannot fuzzy-resolve. (Relationship pointers like
/// `run create --parent-run-id` are exact-only — they validate with
/// [`parse_run_id`] and never route through here.)
///
/// A malformed run-id is a distinct, machine-actionable error class from a
/// well-formed id that simply names no run, so it surfaces as `invalid_run_id`
/// carrying the core validator's reason (length, charset, ULID range) rather
/// than being collapsed into `run_not_found`. Callers that look a run up by id
/// still emit their own `run_not_found` for the valid-but-missing case.
pub(crate) fn run_paths_from_cli_arg(root: &Path, run_id: &str) -> Result<RunPaths, CliError> {
    // Resolve a prefix (if any) to a typed, already-validated run id, then hand
    // off to the exact path helper. Prefix resolution happens here and nowhere
    // downstream.
    let rid = RunSelector::parse(run_id)?.resolve(root)?;
    run_paths_exact(root, &rid)
}

/// Trim a CLI string argument and reject empty/whitespace-only values.
pub fn require_nonempty(value: &str, field: &str) -> Result<String, CliError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CliError::user(
            "invalid_value",
            format!("--{field} must not be empty or whitespace-only"),
        )
        .with_invalid_value(value));
    }
    Ok(trimmed.to_string())
}

/// Map an id-validation failure to the CLI's `invalid_id` error envelope,
/// carrying the offending value (`invalid_value`) and the accepted-shape hint
/// (`expected`, e.g. `n-NNNN`). This is the single boundary where a malformed
/// id surfaces to an AI caller; the typed newtype is the only thing a path
/// helper will accept downstream.
pub fn invalid_id(value: &str, err: &IdValidationError) -> CliError {
    CliError::user("invalid_id", err.to_string())
        .with_invalid_value(value)
        .with_expected(serde_json::Value::String(err.expected().to_string()))
}

/// Validate a `run_id` clap or event-data argument into a typed [`RunId`].
/// CLI verb-entry call sites instead go through [`run_paths_from_cli_arg`], which
/// both validates and builds the [`RunPaths`]; use this when only validation of a
/// bare run-id string is needed (e.g. an event-data `child_run_id`).
pub fn parse_run_id(value: &str) -> Result<RunId, CliError> {
    RunId::parse_str(value).map_err(|e| invalid_id(value, &e))
}

/// Validate a `node_id` clap argument into a typed [`NodeId`] before it can
/// reach any path helper.
pub fn parse_node_id(value: &str) -> Result<NodeId, CliError> {
    NodeId::parse_str(value).map_err(|e| invalid_id(value, &e))
}

/// Render a `Kind` as its canonical kebab-case wire string. Delegates to
/// [`Kind::wire_name`] — the single source of truth — so create/list/show/json/
/// text stay aligned with the enum (including the read-only `unknown` catch-all
/// a legacy on-disk run decodes to).
pub fn kind_kebab(k: Kind) -> &'static str {
    k.wire_name()
}

/// Refuse a MUTATING operation on a run recorded under a kind removed in the 0.2
/// cut ([`Kind::Unknown`] — `code`, `orchestrate`, `orchestrated`, `bugfix`,
/// `make-skill`, or any future/unknown wire value).
///
/// Such a run stays decodable so `run list` / `run show` / `doctor` can REPORT
/// it (ADR §D7 — the on-disk evidence corpus is never deleted), but it is
/// **read-only**: mutating it would append to its event log and rewrite
/// `manifest.json`, and because `Kind::Unknown` re-serializes to `"unknown"`
/// that write would destroy the original kind provenance the corpus preserves.
/// Refusing here makes "read-only" an enforced invariant rather than a
/// convention. Callers guard before their first append (merge / cancel /
/// supervise).
pub fn reject_legacy_kind(kind: Kind, run_id: &str) -> Result<(), CliError> {
    if kind == Kind::Unknown {
        return Err(CliError::user(
            "legacy_run_read_only",
            format!(
                "run {run_id} was recorded under a run kind removed in 0.2 and is read-only — \
                 inspect it with `run show` / `run list` / `doctor`, but it cannot be mutated \
                 (merged, cancelled, or supervised)"
            ),
        )
        .with_invalid_value(run_id));
    }
    Ok(())
}

pub fn lifecycle_kebab(l: Lifecycle) -> &'static str {
    match l {
        Lifecycle::Autonomous => "autonomous",
        Lifecycle::Interactive => "interactive",
    }
}

pub fn status_kebab(s: taskfleet_core::Status) -> &'static str {
    use taskfleet_core::Status::{Blocked, Cancelled, Done, Failed, Pending, Running};
    match s {
        Pending => "pending",
        Running => "running",
        Blocked => "blocked",
        Done => "done",
        Failed => "failed",
        Cancelled => "cancelled",
    }
}

/// `<root>/runs/`.
pub fn runs_root(root: &Path) -> PathBuf {
    root.join("runs")
}

/// Map a `core::Error` into a `CliError`.
///
/// Every flavor of *corrupt persisted state* collapses into one non-retryable
/// `corrupt_state` user error (exit 1): a malformed `events.jsonl` line
/// ([`CorruptEventLog`]), a projection whose embedded id contradicts its path
/// ([`CorruptProjection`]), malformed state-file JSON ([`Json`]/[`JsonBare`]),
/// and a state file written by an unsupported build
/// ([`UnsupportedSchemaVersion`]). These are all data-integrity faults the
/// caller must investigate, not transient I/O to retry — surfacing them under
/// one user code (exit 1) keeps an AI caller's retry loop from hammering a file
/// that will never parse. Where the variant carries them, the two mismatched
/// ids / the bad-vs-supported schema versions ride along in `invalid_value` /
/// `expected` for the operator to diff.
///
/// A symlinked run dir, subdir, or state file is a separate tampered-run fault
/// (`corrupt_run`, exit 1). Everything else — genuine transient I/O — collapses
/// into the generic `io_error` system class (exit 2).
///
/// [`CorruptEventLog`]: taskfleet_core::Error::CorruptEventLog
/// [`CorruptProjection`]: taskfleet_core::Error::CorruptProjection
/// [`Json`]: taskfleet_core::Error::Json
/// [`JsonBare`]: taskfleet_core::Error::JsonBare
/// [`UnsupportedSchemaVersion`]: taskfleet_core::Error::UnsupportedSchemaVersion
pub fn from_core(err: taskfleet_core::Error) -> CliError {
    match err {
        taskfleet_core::Error::CorruptEventLog { .. }
        | taskfleet_core::Error::Json { .. }
        | taskfleet_core::Error::JsonBare(_) => CliError::user("corrupt_state", err.to_string()),
        taskfleet_core::Error::CorruptProjection {
            ref expected_id,
            ref body_id,
            ..
        } => {
            let (expected_id, body_id) = (expected_id.clone(), body_id.clone());
            CliError::user("corrupt_state", err.to_string())
                .with_invalid_value(body_id)
                .with_expected(serde_json::Value::String(expected_id))
        }
        taskfleet_core::Error::UnsupportedSchemaVersion {
            found,
            ref supported,
            ..
        } => {
            let supported = supported.clone();
            CliError::user("corrupt_state", err.to_string())
                .with_invalid_value(found.to_string())
                .with_expected(serde_json::json!({ "supported_schema_versions": supported }))
        }
        // A symlinked run dir, subdir, or state file is a tampered or corrupted
        // run, not a transient I/O fault to retry — it surfaces as a distinct
        // `corrupt_run` user error (exit 1) so a retry loop doesn't chase a path
        // that will never be a regular file. The offending path rides along in
        // `invalid_value` so an operator can go straight to it.
        taskfleet_core::Error::SymlinkRunDir { ref path }
        | taskfleet_core::Error::SymlinkSubdir { ref path, .. }
        | taskfleet_core::Error::SymlinkStateFile { ref path, .. } => {
            let path = path.display().to_string();
            CliError::user("corrupt_run", err.to_string()).with_invalid_value(path)
        }
        // An empty idempotency key is a caller/client error, not a system fault —
        // the CLI boundary normally rejects it first, so this is the core backstop.
        taskfleet_core::Error::EmptyIdempotencyKey => {
            CliError::user("invalid_value", err.to_string()).with_invalid_value("")
        }
        other => CliError::system("io_error", other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_run_dir(root: &Path, id: &RunId) {
        std::fs::create_dir_all(taskfleet_core::run_dir(root, id)).unwrap();
    }

    /// The exact path is closed to a truncated id at the **type level**:
    /// `run_paths_exact` accepts only a `&RunId`, and a `<26`-char string can
    /// never become one — `parse_run_id` rejects it loudly as `invalid_id`
    /// rather than letting it fuzzy-resolve to some other run (the confused-deputy
    /// risk this split exists to remove).
    #[test]
    fn run_paths_exact_only_accepts_a_full_typed_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let full = taskfleet_core::new_run_id();
        let rid = parse_run_id(&full).unwrap();
        make_run_dir(root, &rid);

        // A full typed id resolves exactly, no scan.
        let paths = run_paths_exact(root, &rid).unwrap();
        assert_eq!(paths.run_id.as_str(), full);

        // A truncated id cannot even be constructed into the `RunId` that
        // `run_paths_exact` demands — the fuzzy path is unreachable internally.
        let truncated = &full[..10];
        let err = parse_run_id(truncated).unwrap_err();
        assert_eq!(err.code, "invalid_id");
    }

    /// `RunSelector::parse` classifies a full ULID as `Exact` (no scan) and a
    /// well-formed shorter fragment as `Prefix` — the only way a `Prefix` is ever
    /// produced, and only from a raw CLI argument.
    #[test]
    fn run_selector_classifies_exact_vs_prefix() {
        let full = taskfleet_core::new_run_id();
        assert!(matches!(
            RunSelector::parse(&full).unwrap(),
            RunSelector::Exact(_)
        ));
        assert!(matches!(
            RunSelector::parse(&full[..10]).unwrap(),
            RunSelector::Prefix(_)
        ));
        // A malformed fragment is a loud typo, not a silent no-match.
        assert_eq!(
            RunSelector::parse("not-a-ulid!").unwrap_err().code,
            "invalid_run_id"
        );
    }

    /// The CLI verb-entry chokepoint still resolves an unambiguous prefix to the
    /// full run (behaviour preserved for `run-cancel-accept-unambiguous-prefix`).
    #[test]
    fn cli_verb_entry_resolves_unambiguous_prefix() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let full = taskfleet_core::new_run_id();
        make_run_dir(root, &parse_run_id(&full).unwrap());

        let paths = run_paths_from_cli_arg(root, &full[..10]).unwrap();
        assert_eq!(paths.run_id.as_str(), full);
    }

    /// The confused-deputy contrast, on the SAME truncated fragment and the SAME
    /// on-disk run: the CLI verb entry fuzzy-resolves it to the full run, but the
    /// internal typed boundary (`parse_run_id`) rejects it loudly — so a truncated
    /// `child_run_id` from event data can never reach `run_paths_exact` and
    /// silently resolve to that run.
    #[test]
    fn truncated_id_resolves_on_cli_path_but_is_rejected_on_internal_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let full = taskfleet_core::new_run_id();
        make_run_dir(root, &parse_run_id(&full).unwrap());
        let truncated = &full[..10];

        // CLI verb entry: truncated fragment fuzzy-resolves to the full run.
        assert_eq!(
            run_paths_from_cli_arg(root, truncated)
                .unwrap()
                .run_id
                .as_str(),
            full
        );

        // Internal path: the same fragment cannot be typed as a `RunId`, so it
        // never reaches `run_paths_exact` — the fuzzy match is unreachable. This
        // is the whole point of the split.
        assert_eq!(parse_run_id(truncated).unwrap_err().code, "invalid_id");
    }
}
