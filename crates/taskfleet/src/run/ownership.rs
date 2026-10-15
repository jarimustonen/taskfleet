//! Exact owning-run discovery for `run show --current`.
//!
//! A worker branch contains only a compact ULID-derived display identifier (or
//! the legacy timestamp prefix). It is not identity. Ownership is instead resolved
//! from the durable node projection whose recorded worktree path is exactly the
//! current git worktree root. The checked-out branch is used as corroborating
//! evidence, not as a fuzzy selector.

use std::path::{Path, PathBuf};

use taskfleet_core::{read_node_opt, NodeId, RunId, RunLock, RunPaths};

use crate::error::CliError;
use crate::run::{from_core, runs_root};

#[derive(Debug)]
struct CurrentWorktree {
    root: PathBuf,
    branch: String,
}

/// Resolve the one run/node that owns the current git worktree.
///
/// Every valid run projection is inspected under that run's shared lock. Reads
/// fail closed: malformed node evidence cannot be skipped because doing so could
/// hide a second owner and turn ambiguity into a wrong unique result.
pub fn resolve_current(root: &Path, cwd: &Path) -> Result<RunId, CliError> {
    resolve_current_with(root, cwd, false)
}

fn resolve_current_with(root: &Path, cwd: &Path, allow_detached: bool) -> Result<RunId, CliError> {
    let current = current_worktree(cwd, allow_detached)?;
    let runs = runs_root(root);
    let entries = std::fs::read_dir(&runs).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            CliError::user(
                "run_owner_not_found",
                format!(
                    "no taskfleet run owns current worktree {} (runs directory is absent)",
                    current.root.display()
                ),
            )
        } else {
            CliError::system("io_error", format!("read_dir {}: {e}", runs.display()))
        }
    })?;

    let mut owners = Vec::<(RunId, NodeId)>::new();
    let mut stale = Vec::<(RunId, NodeId)>::new();

    for entry in entries {
        let entry = entry.map_err(|e| {
            CliError::system("io_error", format!("read_dir {}: {e}", runs.display()))
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(run_id) = RunId::parse_str(&name) else {
            continue;
        };
        let paths = RunPaths::from_validated(entry.path(), run_id.clone()).map_err(|e| {
            CliError::user(
                "run_owner_malformed",
                format!("cannot inspect ownership evidence for run {run_id}: {e}"),
            )
        })?;
        let _guard = RunLock::acquire_shared(&paths.lock()).map_err(from_core)?;
        let evidence = scan_nodes(&paths, &current)?;
        for (node_id, branch_match) in evidence {
            if branch_match {
                owners.push((run_id.clone(), node_id));
            } else {
                stale.push((run_id.clone(), node_id));
            }
        }
    }

    match owners.len() {
        1 if stale.is_empty() => Ok(owners.pop().expect("one owner").0),
        1 => {
            // A second projection claiming this exact path is duplicate evidence
            // even when its branch field is stale. Never prefer the plausible row.
            let mut claims = owner_labels(&owners);
            claims.extend(stale_labels(&stale));
            Err(ambiguous_owner(&current.root, claims))
        }
        n if n > 1 => Err(ambiguous_owner(&current.root, owner_labels(&owners))),
        _ if !stale.is_empty() => Err(CliError::user(
            "run_owner_stale",
            format!(
                "ownership evidence for current worktree {} is stale: checked-out branch {:?} does not match the recorded owner",
                current.root.display(), current.branch
            ),
        )
        .with_expected(serde_json::Value::Array(
            stale_labels(&stale)
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ))),
        _ => Err(CliError::user(
            "run_owner_not_found",
            format!(
                "no taskfleet node records current worktree {} and branch {:?}; use the exact run id from the generated worker context",
                current.root.display(), current.branch
            ),
        )),
    }
}

/// Optional creator attribution for `run create`. An ordinary checkout has no
/// owner; stale, duplicate or malformed evidence is not an ordinary checkout.
pub fn creator_in_cwd(root: &Path) -> Result<Option<RunId>, CliError> {
    let cwd = std::env::current_dir()
        .map_err(|e| CliError::system("io_error", format!("current directory: {e}")))?;
    // A detached release checkout can create ordinary runs. Only a recorded
    // claim for that exact path makes its missing branch stale evidence.
    match resolve_current_with(root, &cwd, true) {
        Ok(id) => Ok(Some(id)),
        Err(error) if error.code == "run_owner_not_found" => Ok(None),
        Err(error) => Err(error),
    }
}

fn scan_nodes(
    paths: &RunPaths,
    current: &CurrentWorktree,
) -> Result<Vec<(NodeId, bool)>, CliError> {
    let entries = match std::fs::read_dir(paths.nodes_dir()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(CliError::system(
                "run_owner_malformed",
                format!(
                    "cannot read ownership evidence for run {}: {e}",
                    paths.run_id
                ),
            ))
        }
    };
    let mut matches = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| {
            CliError::system(
                "run_owner_malformed",
                format!(
                    "cannot enumerate ownership evidence for run {}: {e}",
                    paths.run_id
                ),
            )
        })?;
        let entry_path = entry.path();
        if entry_path.extension().and_then(|s| s.to_str()) != Some("json") {
            return Err(malformed_node(
                paths,
                &entry_path,
                "unexpected non-JSON entry in the owned nodes directory",
            ));
        }
        let Some(stem) = entry_path.file_stem().and_then(|s| s.to_str()) else {
            return Err(malformed_node(
                paths,
                &entry_path,
                "non-UTF-8 node filename",
            ));
        };
        let node_id = NodeId::parse_str(stem)
            .map_err(|e| malformed_node(paths, &entry_path, &e.to_string()))?;
        let node = read_node_opt(paths, &node_id)
            .map_err(|e| malformed_node(paths, &entry_path, &e.to_string()))?
            .ok_or_else(|| {
                malformed_node(
                    paths,
                    &entry_path,
                    "node projection disappeared during the ownership scan",
                )
            })?;
        let Some(recorded_path) = node.worktree_path.as_deref() else {
            continue;
        };
        let recorded_path = Path::new(recorded_path);
        if !recorded_path.is_absolute() {
            return Err(malformed_node(
                paths,
                &entry_path,
                "recorded worktree_path must be absolute",
            ));
        }
        let recorded_root = match std::fs::canonicalize(recorded_path) {
            Ok(path) => path,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(malformed_node(
                    paths,
                    &entry_path,
                    &format!("cannot canonicalize recorded worktree_path: {e}"),
                ))
            }
        };
        if recorded_root != current.root {
            continue;
        }
        // Branch corroboration is mandatory. A legacy projection with no branch
        // cannot safely distinguish a reused worktree path, so it is stale
        // evidence rather than a match. Existing runs with a recorded branch keep
        // working unchanged.
        let branch_match =
            !current.branch.is_empty() && node.branch.as_deref() == Some(current.branch.as_str());
        matches.push((node_id, branch_match));
    }
    Ok(matches)
}

fn current_worktree(cwd: &Path, allow_detached: bool) -> Result<CurrentWorktree, CliError> {
    let cwd = std::fs::canonicalize(cwd).map_err(|e| {
        CliError::system("io_error", format!("canonicalize current directory: {e}"))
    })?;
    let mut cursor = Some(cwd.as_path());
    while let Some(dir) = cursor {
        let dot_git = dir.join(".git");
        if dot_git.exists() {
            let git_dir = resolve_git_dir(dir, &dot_git)?;
            let head = std::fs::read_to_string(git_dir.join("HEAD")).map_err(|e| {
                CliError::system(
                    "run_owner_malformed",
                    format!("cannot read {}: {e}", git_dir.join("HEAD").display()),
                )
            })?;
            let branch = match head.strip_prefix("ref: refs/heads/").map(str::trim) {
                Some(branch) => branch,
                None if allow_detached => "", // cannot corroborate a recorded owner
                None => {
                    return Err(CliError::user(
                        "run_owner_stale",
                        format!(
                            "current worktree {} has a detached or malformed HEAD; exact owning-run discovery requires its recorded branch",
                            dir.display()
                        ),
                    ));
                }
            };
            if branch.is_empty() && head.starts_with("ref: refs/heads/") {
                return Err(CliError::system(
                    "run_owner_malformed",
                    format!(
                        "{} contains an empty branch ref",
                        git_dir.join("HEAD").display()
                    ),
                ));
            }
            return Ok(CurrentWorktree {
                root: dir.to_path_buf(),
                branch: branch.to_string(),
            });
        }
        cursor = dir.parent();
    }
    Err(CliError::user(
        "run_owner_not_found",
        format!(
            "current directory {} is not inside a git worktree",
            cwd.display()
        ),
    ))
}

fn resolve_git_dir(worktree: &Path, dot_git: &Path) -> Result<PathBuf, CliError> {
    if dot_git.is_dir() {
        return std::fs::canonicalize(dot_git).map_err(|e| {
            CliError::system(
                "io_error",
                format!("canonicalize {}: {e}", dot_git.display()),
            )
        });
    }
    let marker = std::fs::read_to_string(dot_git).map_err(|e| {
        CliError::system(
            "run_owner_malformed",
            format!("cannot read git worktree marker {}: {e}", dot_git.display()),
        )
    })?;
    let Some(raw) = marker.strip_prefix("gitdir:").map(str::trim) else {
        return Err(CliError::system(
            "run_owner_malformed",
            format!(
                "git worktree marker {} does not contain a gitdir",
                dot_git.display()
            ),
        ));
    };
    if raw.is_empty() {
        return Err(CliError::system(
            "run_owner_malformed",
            format!(
                "git worktree marker {} contains an empty gitdir",
                dot_git.display()
            ),
        ));
    }
    let path = Path::new(raw);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        worktree.join(path)
    };
    std::fs::canonicalize(&path).map_err(|e| {
        CliError::system(
            "run_owner_malformed",
            format!("canonicalize gitdir {}: {e}", path.display()),
        )
    })
}

fn malformed_node(paths: &RunPaths, path: &Path, reason: &str) -> CliError {
    CliError::user(
        "run_owner_malformed",
        format!(
            "malformed ownership evidence for run {} at {}: {reason}",
            paths.run_id,
            path.display()
        ),
    )
}

fn owner_labels(rows: &[(RunId, NodeId)]) -> Vec<String> {
    rows.iter().map(|(r, n)| format!("{r}/{n}")).collect()
}

fn stale_labels(rows: &[(RunId, NodeId)]) -> Vec<String> {
    owner_labels(rows)
}

fn ambiguous_owner(worktree: &Path, mut claims: Vec<String>) -> CliError {
    claims.sort();
    CliError::user(
        "run_owner_ambiguous",
        format!(
            "multiple taskfleet nodes claim current worktree {}; refusing to choose an owning run",
            worktree.display()
        ),
    )
    .with_expected(serde_json::Value::Array(
        claims.into_iter().map(serde_json::Value::String).collect(),
    ))
}
