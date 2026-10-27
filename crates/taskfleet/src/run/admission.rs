//! Global worker-admission gate. Lock order: admission SH -> idempotency ->
//! per-run locks; upgrade EX -> per-run SH reads. Never acquire admission while
//! holding a per-run lock. The lock file is permanent (never unlink it), and
//! all versions participating in this protocol must use the same state root.
//! Before launching activation we persist an inhibit marker under EX. A dead
//! parent, timed-out child, or detached setsid descendant can never reopen
//! admissions by merely releasing the flock. Only a trusted synchronous
//! activation success, or explicit operator recovery, removes the marker.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use fs4::{FileExt, TryLockError};
use serde::Serialize;
use taskfleet_core::STATE_SCHEMA_VERSION;

use crate::error::CliError;
use crate::output::{self, OutputFormat, OutputSpec};
use crate::run::from_core;
use crate::run::upgrade_survey::{self, Abandoned, LiveRun, Survey, RECEIPT_DIR};

const FILE: &str = ".worker-admission.lock";
const DEFAULT_WAIT: u64 = 10;
const INHIBIT: &str = ".worker-admission-inhibited";

pub(super) struct Gate(File);

fn acquire(root: &Path, exclusive: bool, wait: u64) -> Result<Gate, CliError> {
    std::fs::create_dir_all(root).map_err(|e| io_error(root, e))?;
    let path = root.join(FILE);
    // Refuse a redirected lock path. O_NOFOLLOW backs up the lstat check.
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(CliError::system(
            "admission_lock_invalid",
            format!("symlink lock: {}", path.display()),
        ));
    }
    let mut opts = OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = opts.open(&path).map_err(|e| io_error(&path, e))?;
    let deadline = Instant::now() + Duration::from_secs(wait);
    loop {
        let result = if exclusive {
            FileExt::try_lock(&file)
        } else {
            FileExt::try_lock_shared(&file)
        };
        match result {
            Ok(()) => return Ok(Gate(file)),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(TryLockError::WouldBlock) => {
                return Err(CliError::system(
                    "admission_busy",
                    format!(
                        "worker admission lock {} busy after {wait}s; retry later",
                        path.display()
                    ),
                ))
            }
            Err(TryLockError::Error(e)) => return Err(io_error(&path, e)),
        }
    }
}

fn io_error(path: &Path, e: io::Error) -> CliError {
    CliError::system("admission_lock_io", format!("{}: {e}", path.display()))
}

/// Acquired before any create-side idempotency or per-run lock and held until
/// the create call returns, including child publication and replay repair.
pub(super) fn admit(root: &Path) -> Result<Gate, CliError> {
    let gate = acquire(root, false, DEFAULT_WAIT)?;
    ensure_not_inhibited(root)?;
    Ok(gate)
}

fn ensure_not_inhibited(root: &Path) -> Result<(), CliError> {
    let path = root.join(INHIBIT);
    match path.symlink_metadata() {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(CliError::system("upgrade_inhibited", format!(
            "worker admission inhibited at {}; inspect activation and descendants, then use run upgrade-lock --recover --confirm-quiescent with a trusted synchronous reconciliation command", path.display()
        ))),
        Err(e) => Err(io_error(&path, e)),
    }
}

fn sync_root(root: &Path) -> Result<(), CliError> {
    File::open(root)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| io_error(root, e))
}

fn inhibit(root: &Path) -> Result<(), CliError> {
    let path = root.join(INHIBIT);
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = opts.open(&path).map_err(|e| io_error(&path, e))?;
    file.sync_all().map_err(|e| io_error(&path, e))?;
    sync_root(root)
}

fn clear_inhibit(root: &Path) -> Result<(), CliError> {
    let path = root.join(INHIBIT);
    // Only remove our own regular file. A replaced marker is never authority
    // to admit workers, even after a successful activation.
    let meta = path.symlink_metadata().map_err(|e| io_error(&path, e))?;
    if !meta.file_type().is_file() || meta.len() != 0 {
        return Err(CliError::system(
            "upgrade_state_unverifiable",
            format!(
                "inhibit marker changed at {}; admission remains blocked",
                path.display()
            ),
        ));
    }
    std::fs::remove_file(&path).map_err(|e| io_error(&path, e))?;
    sync_root(root)
}

/// Operator inputs to [`upgrade`]; each override flag is a separate, explicit
/// acceptance (see `upgrade_survey` for what each one may accept). The bools
/// mirror independent CLI flags.
#[allow(clippy::struct_excessive_bools)]
pub(super) struct UpgradeArgs {
    pub wait_secs: u64,
    pub command_timeout_secs: u64,
    pub argv: Vec<String>,
    pub recover: bool,
    pub confirm_quiescent: bool,
    pub allow_live_runs: bool,
    pub target_state_schema: Option<u32>,
    pub abandon_stale_creations: bool,
}

#[derive(Serialize)]
struct UpgradeResult {
    /// True only when nothing was overridden: no live run accepted and no
    /// creation abandoned.
    quiescent: bool,
    checked_runs: usize,
    command_exit: i32,
    recovered: bool,
    state_schema_version: u32,
    accepted_live_runs: Vec<LiveRun>,
    abandoned_creations: Vec<Abandoned>,
    /// Override receipt under `<root>/upgrade-receipts/`; null when no
    /// override accepted anything.
    receipt: Option<String>,
    /// Executables the accepted runs' launchers call (worker handshake,
    /// `run-worker`, Claude hooks). The activation must leave each in place
    /// or replace it in place while those runs live.
    predecessor_executables: Vec<PredecessorExecutable>,
}

#[derive(Clone, Serialize)]
struct PredecessorExecutable {
    path: String,
    run_ids: Vec<String>,
    present_after_activation: bool,
}

#[derive(Serialize)]
struct Receipt<'a> {
    receipt_id: &'a str,
    created_at: chrono::DateTime<chrono::Utc>,
    taskfleet_version: &'static str,
    mode: &'static str,
    activation_program: &'a str,
    state_schema_version: u32,
    target_state_schema: Option<u32>,
    accepted_live_runs: &'a [LiveRun],
    abandoned_creations: &'a [Abandoned],
    outcome: &'a str,
}

fn write_receipt(root: &Path, receipt: &Receipt<'_>) -> Result<String, CliError> {
    let dir = root.join(RECEIPT_DIR);
    upgrade_survey::private_dir(&dir)?;
    let path = dir.join(format!("{}.json", receipt.receipt_id));
    taskfleet_core::atomic::write_json_atomic(&path, receipt).map_err(from_core)?;
    Ok(path.display().to_string())
}

/// Decide, before any mutation, what the survey allows. Every refusal leaves
/// admission and state exactly as they were.
fn accept(
    root: &Path,
    survey: &Survey,
    args: &UpgradeArgs,
) -> Result<Vec<(String, Vec<String>)>, CliError> {
    let creation_ids: Vec<&String> = survey.creations.keys().collect();
    if !creation_ids.is_empty() && !args.abandon_stale_creations {
        let (id, creation) = survey.creations.iter().next().expect("nonempty");
        let path = creation
            .staging_dir
            .clone()
            .or_else(|| creation.control_files.first().cloned())
            .unwrap_or_else(|| root.join(".creating"));
        return Err(CliError::system(
            "upgrade_state_unverifiable",
            format!(
                "unpublished creation state at {} (creation {id}); inspect it, or pass --abandon-stale-creations to move provably abandoned creations aside",
                path.display()
            ),
        )
        .with_details(serde_json::json!({"creation_ids": creation_ids})));
    }
    let blockers: Vec<&String> = survey.live.iter().map(|r| &r.run_id).collect();
    if !args.allow_live_runs && (!blockers.is_empty() || !survey.unverifiable_runs.is_empty()) {
        let blockers: Vec<&String> = blockers
            .into_iter()
            .chain(&survey.unverifiable_runs)
            .collect();
        return Err(CliError::system(
            "upgrade_not_quiescent",
            format!(
                "{} live, awaiting-input, or recoverable runs block activation: {}; --allow-live-runs --target-state-schema <N> accepts them when the target release keeps this state schema",
                blockers.len(),
                blockers.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            ),
        )
        .with_details(serde_json::json!({
            "run_ids": blockers,
            "checked_runs": survey.checked_runs,
            "runs": survey.live,
        })));
    }
    if !survey.unverifiable_runs.is_empty() {
        return Err(CliError::system(
            "upgrade_state_unverifiable",
            format!(
                "unreadable supervisor pid record in runs {}; no override accepts an unclassifiable run",
                survey.unverifiable_runs.join(", ")
            ),
        )
        .with_details(serde_json::json!({"run_ids": survey.unverifiable_runs})));
    }
    if !blockers.is_empty() {
        let Some(target) = args.target_state_schema else {
            return Err(CliError::user(
                "upgrade_target_schema_required",
                "--allow-live-runs requires --target-state-schema",
            ));
        };
        let mismatched: Vec<&String> = survey
            .live
            .iter()
            .filter(|r| r.state_schemas.iter().any(|v| *v != target))
            .map(|r| &r.run_id)
            .collect();
        if target != STATE_SCHEMA_VERSION || !mismatched.is_empty() {
            return Err(CliError::system(
                "upgrade_state_schema_incompatible",
                format!(
                    "live runs may only cross an upgrade within one state schema: target {target}, this binary {STATE_SCHEMA_VERSION}, mismatched runs [{}]; settle the live runs first",
                    mismatched.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                ),
            )
            .with_details(serde_json::json!({
                "target_state_schema": target,
                "state_schema_version": STATE_SCHEMA_VERSION,
                "run_ids": blockers,
                "mismatched_run_ids": mismatched,
            })));
        }
    }
    let mut proven = Vec::new();
    for (id, creation) in &survey.creations {
        let evidence = upgrade_survey::prove_abandoned(root, id, creation)?;
        upgrade_survey::abandon_destination(root, id)?;
        proven.push((id.clone(), evidence));
    }
    Ok(proven)
}

/// Hold an exclusive gate over a trusted fleet activation argv. No shell is
/// involved. The lock fd is inherited by the immediate child, so killing the
/// CLI parent cannot silently unlock while its activation process still runs.
pub(super) fn upgrade(
    args: UpgradeArgs,
    spec: &OutputSpec,
    warnings: &[String],
) -> Result<(), CliError> {
    let root = crate::home::root_dir()?;
    let gate = acquire(&root, true, args.wait_secs)?;
    let recover = args.recover;
    if recover {
        if !args.confirm_quiescent {
            return Err(CliError::system("upgrade_recovery_unconfirmed", "recovery requires --confirm-quiescent after verifying all previous activation writers have stopped"));
        }
        if root
            .join(INHIBIT)
            .symlink_metadata()
            .is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
        {
            return Err(CliError::system(
                "upgrade_recovery_unneeded",
                "no inhibited upgrade to recover",
            ));
        }
        // A malformed or symlink marker still blocks admission; do not run
        // arbitrary code or clear it until its provenance is inspected.
        let marker = root.join(INHIBIT);
        let meta = marker
            .symlink_metadata()
            .map_err(|e| io_error(&marker, e))?;
        if !meta.file_type().is_file() || meta.len() != 0 {
            return Err(CliError::system(
                "upgrade_state_unverifiable",
                format!("invalid inhibit marker at {}", marker.display()),
            ));
        }
    } else {
        ensure_not_inhibited(&root)?;
    }
    let survey = upgrade_survey::survey(&root, args.wait_secs)?;
    let proven = accept(&root, &survey, &args)?;
    let checked = survey.checked_runs;
    let accepted_live_runs = survey.live.clone();
    if !recover {
        inhibit(&root)?;
    }
    // Admission is now excluded (marker or prior marker plus EX). Move the
    // proven-abandoned creations aside and record every override before the
    // activation can change the installation.
    let receipt_id = ulid::Ulid::new().to_string().to_lowercase();
    let mut abandoned = Vec::new();
    for (id, evidence) in proven {
        let creation = &survey.creations[&id];
        abandoned.push(upgrade_survey::move_aside(
            &root,
            &id,
            creation,
            evidence,
            &receipt_id,
        )?);
    }
    let overridden = !accepted_live_runs.is_empty() || !abandoned.is_empty();
    let receipt_for = |outcome: &str| -> Result<Option<String>, CliError> {
        if !overridden {
            return Ok(None);
        }
        write_receipt(
            &root,
            &Receipt {
                receipt_id: &receipt_id,
                created_at: chrono::Utc::now(),
                taskfleet_version: env!("CARGO_PKG_VERSION"),
                mode: if recover { "recover" } else { "activate" },
                activation_program: &args.argv[0],
                state_schema_version: STATE_SCHEMA_VERSION,
                target_state_schema: args.target_state_schema,
                accepted_live_runs: &accepted_live_runs,
                abandoned_creations: &abandoned,
                outcome,
            },
        )
        .map(Some)
    };
    let receipt = receipt_for("activating")?;
    let override_details = || {
        serde_json::json!({
            "receipt": receipt,
            "accepted_live_runs": accepted_live_runs.iter().map(|r| &r.run_id).collect::<Vec<_>>(),
            "abandoned_creations": abandoned.iter().map(|a| &a.creation_id).collect::<Vec<_>>(),
        })
    };
    let fail = |error: CliError, outcome: &str| -> CliError {
        if !overridden {
            return error;
        }
        // The pre-activation receipt already names every override; a failed
        // final rewrite must not mask the activation error.
        let _ = receipt_for(outcome);
        error.with_details(override_details())
    };
    let program = &args.argv;
    let mut cmd = Command::new(&program[0]);
    cmd.args(&program[1..]);
    // Activation output belongs on stderr; stdout is reserved for the JSON
    // result. A trusted program may emit diagnostics without corrupting it.
    cmd.stdout(std::process::Stdio::from(std::io::stderr()));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let fd = gate.0.as_raw_fd();
        // SAFETY: only async-signal-safe fcntl in the forked child, before exec.
        unsafe {
            cmd.pre_exec(move || {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = cmd.spawn().map_err(|e| {
        fail(
            CliError::system(
                "upgrade_command_failed",
                format!("start {:?}: {e}", program[0]),
            ),
            "spawn_failed",
        )
    })?;
    let deadline = Instant::now() + Duration::from_secs(args.command_timeout_secs);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| {
            fail(
                CliError::system(
                    "upgrade_command_failed",
                    format!("wait for activation: {e}"),
                ),
                "wait_failed",
            )
        })? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(fail(CliError::system("upgrade_command_timeout", format!(
                "activation exceeded {}s; admission remains inhibited; inspect detached descendants and reconcile before recovery", args.command_timeout_secs
            )), "timeout"));
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    if !status.success() {
        return Err(fail(
            CliError::system(
                "upgrade_command_failed",
                format!(
                    "activation exited with {status}; inspect and retry or roll back under the gate"
                ),
            ),
            "failed",
        ));
    }
    clear_inhibit(&root).map_err(|e| fail(e, "marker_unclearable"))?;
    let mut warnings = warnings.to_vec();
    if overridden && receipt_for("completed").is_err() {
        warnings.push(format!(
            "override receipt {} still reads outcome=activating",
            receipt.as_deref().unwrap_or_default()
        ));
    }
    let mut dependents = std::collections::BTreeMap::<&str, Vec<String>>::new();
    for run in &accepted_live_runs {
        for exe in &run.executables {
            dependents.entry(exe).or_default().push(run.run_id.clone());
        }
    }
    let predecessor_executables: Vec<PredecessorExecutable> = dependents
        .into_iter()
        .map(|(path, run_ids)| {
            let present = Path::new(path).is_file();
            if !present {
                warnings.push(format!(
                    "predecessor executable {path} is missing after activation; accepted live runs {} call it from their launchers and Claude hooks and will fail until it is restored",
                    run_ids.join(", ")
                ));
            }
            PredecessorExecutable {
                path: path.to_string(),
                run_ids,
                present_after_activation: present,
            }
        })
        .collect();
    let result = UpgradeResult {
        quiescent: !overridden,
        checked_runs: checked,
        command_exit: 0,
        recovered: recover,
        state_schema_version: STATE_SCHEMA_VERSION,
        accepted_live_runs,
        abandoned_creations: abandoned,
        receipt,
        predecessor_executables,
    };
    match spec.format {
        OutputFormat::Text => {
            println!("upgrade completed; {checked} runs checked");
            for run in &result.accepted_live_runs {
                println!(
                    "accepted live run {} ({})",
                    run.run_id,
                    run.reasons.join(", ")
                );
            }
            for creation in &result.abandoned_creations {
                println!(
                    "abandoned creation {} moved to {}",
                    creation.creation_id, creation.moved_to
                );
            }
            if let Some(receipt) = &result.receipt {
                println!("override receipt: {receipt}");
            }
            for warning in &warnings {
                eprintln!("warning: {warning}");
            }
        }
        _ => output::emit_envelope(&result, spec, &warnings)?,
    }
    drop(gate);
    Ok(())
}
