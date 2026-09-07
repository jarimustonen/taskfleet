//! Canonical Taskfleet public-input and state-home resolution.
//!
//! This is the only module that reads Taskfleet's branded environment variables
//! or selects the repository configuration file. Without an explicit home it
//! retains a meaningful adopted `.orchestratectl` root while canonical
//! `.taskfleet` contains no meaningful state; two meaningful roots fail closed.
//! Resolution is frozen for the process so a command cannot switch state roots
//! midway through execution.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::error::CliError;

pub const HOME_ENV: &str = "TASKFLEET_HOME";
pub const PROFILE_ENV: &str = "TASKFLEET_PROFILE";
pub const HARNESS_ENV: &str = "TASKFLEET_HARNESS";
pub const LOG_ENV: &str = "TASKFLEET_LOG";

/// Runtime-only paths whose presence does not establish meaningful run/config
/// ownership. The producers use these same constants so a rename cannot make
/// their own artifacts unexpectedly change default-root selection.
pub(crate) const LOG_FILE_NAME: &str = "taskfleet.log.jsonl";
pub(crate) const PI_PROVENANCE_FILE_NAME: &str = "pi-installed-skills.json";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HomeSource {
    CanonicalExplicit,
    CanonicalDefault,
    AdoptedLegacyDefault,
    InternalWorker,
}

pub enum RepositorySource<'a> {
    NotApplicable,
    RunCreate(Option<&'a str>),
}

#[derive(Clone, Debug)]
struct RepositoryConfigSelection {
    path: PathBuf,
    bytes: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
struct Inputs {
    root: PathBuf,
    home_source: HomeSource,
    profile: Option<String>,
    harness: Option<String>,
    log: Option<String>,
    repository_config: Option<RepositoryConfigSelection>,
}

static INPUTS: OnceLock<Inputs> = OnceLock::new();

pub fn initialize(
    internal_worker_root: Option<PathBuf>,
    repository_source: RepositorySource<'_>,
) -> Result<Vec<String>, CliError> {
    if INPUTS.get().is_some() {
        return Ok(Vec::new());
    }
    let (root, home_source) = if let Some(root) = internal_worker_root {
        if !root.is_absolute() {
            return Err(CliError::system(
                "invalid_internal_home",
                "TASKFLEET_INTERNAL_WORKER_STATE_ROOT must be absolute",
            ));
        }
        validate_selected_home(&root)?;
        (root, HomeSource::InternalWorker)
    } else if let Some(root) = env_path(HOME_ENV)? {
        let root = absolute_path(&root)?;
        validate_selected_home(&root)?;
        (root, HomeSource::CanonicalExplicit)
    } else {
        resolve_default_root()?
    };

    let repository_config = match repository_source {
        RepositorySource::NotApplicable => None,
        RepositorySource::RunCreate(source) => Some(resolve_repository_config(source)?),
    };
    let _ = INPUTS.set(Inputs {
        root,
        home_source,
        profile: env_utf8(PROFILE_ENV, true)?,
        harness: env_utf8(HARNESS_ENV, true)?,
        log: env_utf8(LOG_ENV, false)?,
        repository_config,
    });
    Ok(Vec::new())
}

pub fn root_dir() -> Result<PathBuf, CliError> {
    if let Some(inputs) = INPUTS.get() {
        return Ok(inputs.root.clone());
    }
    if let Some(root) = env_path(HOME_ENV)? {
        return absolute_path(&root);
    }
    resolve_default_root().map(|(root, _)| root)
}

pub fn profile() -> Result<Option<String>, CliError> {
    selected_text(PROFILE_ENV, true, |i| &i.profile)
}

pub fn harness() -> Result<Option<String>, CliError> {
    selected_text(HARNESS_ENV, true, |i| &i.harness)
}

pub fn log_filter() -> Result<Option<String>, CliError> {
    selected_text(LOG_ENV, false, |i| &i.log)
}

fn selected_text(
    name: &str,
    trim: bool,
    get: impl FnOnce(&Inputs) -> &Option<String>,
) -> Result<Option<String>, CliError> {
    INPUTS
        .get()
        .map_or_else(|| env_utf8(name, trim), |inputs| Ok(get(inputs).clone()))
}

pub fn home_source() -> Result<HomeSource, CliError> {
    INPUTS
        .get()
        .map(|inputs| inputs.home_source)
        .ok_or_else(|| {
            CliError::system(
                "resolver_not_initialized",
                "home resolver was not initialized",
            )
        })
}

fn resolve_default_root() -> Result<(PathBuf, HomeSource), CliError> {
    let home = std::env::var_os("HOME").ok_or_else(|| {
        CliError::system(
            "home_not_set",
            format!("neither {HOME_ENV} nor HOME is set"),
        )
    })?;
    if home.is_empty() {
        return Err(CliError::system(
            "home_not_set",
            "HOME is set to an empty string",
        ));
    }
    let account_home = absolute_path(Path::new(&home))?;
    let canonical = account_home.join(".taskfleet");
    let legacy = account_home.join(".orchestratectl");

    if roots_are_same(&canonical, &legacy) {
        return Ok((canonical, HomeSource::CanonicalDefault));
    }

    let canonical_kind = default_root_kind(&canonical)?;
    let legacy_kind = default_root_kind(&legacy)?;
    match (canonical_kind, legacy_kind) {
        (RootKind::Meaningful, RootKind::Meaningful) => Err(CliError::user(
            "conflicting_state_homes",
            format!(
                "both canonical home {} and adopted legacy home {} contain meaningful state; set {HOME_ENV} to the authoritative root",
                canonical.display(),
                legacy.display()
            ),
        )),
        (RootKind::Meaningful, _) | (_, RootKind::Empty | RootKind::Incidental) => {
            Ok((canonical, HomeSource::CanonicalDefault))
        }
        (RootKind::Empty | RootKind::Incidental, RootKind::Meaningful) => {
            Ok((legacy, HomeSource::AdoptedLegacyDefault))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RootKind {
    Empty,
    Incidental,
    Meaningful,
}

/// Classify a default root without changing it. Only the two runtime-only
/// trees Taskfleet itself creates are incidental. Unknown top-level entries,
/// symlinks, and unknown files inside those trees are meaningful so a newer
/// state format always fails closed instead of being guessed away.
fn default_root_kind(path: &Path) -> Result<RootKind, CliError> {
    validate_selected_home(path)?;
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(RootKind::Empty),
        Err(e) => {
            return Err(CliError::system(
                "home_unreadable",
                format!("could not inspect state home {}: {e}", path.display()),
            ))
        }
    };

    let mut saw_incidental = false;
    for entry in entries {
        let entry = entry.map_err(|e| {
            CliError::system(
                "home_unreadable",
                format!("could not inspect state home {}: {e}", path.display()),
            )
        })?;
        let name = entry.file_name();
        if name == OsStr::new("logs") {
            if !incidental_tree(&entry.path(), &[LOG_FILE_NAME])? {
                return Ok(RootKind::Meaningful);
            }
            saw_incidental = true;
        } else if name == OsStr::new("state") {
            if !incidental_tree(&entry.path(), &[PI_PROVENANCE_FILE_NAME])? {
                return Ok(RootKind::Meaningful);
            }
            saw_incidental = true;
        } else {
            return Ok(RootKind::Meaningful);
        }
    }
    Ok(if saw_incidental {
        RootKind::Incidental
    } else {
        RootKind::Empty
    })
}

fn incidental_tree(path: &Path, allowed_files: &[&str]) -> Result<bool, CliError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| {
        CliError::system(
            "home_unreadable",
            format!("could not inspect state path {}: {e}", path.display()),
        )
    })?;
    if !metadata.file_type().is_dir() {
        return Ok(false);
    }
    for entry in std::fs::read_dir(path).map_err(|e| {
        CliError::system(
            "home_unreadable",
            format!("could not inspect state path {}: {e}", path.display()),
        )
    })? {
        let entry = entry.map_err(|e| {
            CliError::system(
                "home_unreadable",
                format!("could not inspect state path {}: {e}", path.display()),
            )
        })?;
        let file_type = entry.file_type().map_err(|e| {
            CliError::system(
                "home_unreadable",
                format!(
                    "could not inspect state path {}: {e}",
                    entry.path().display()
                ),
            )
        })?;
        if !file_type.is_file()
            || !allowed_files
                .iter()
                .any(|allowed| entry.file_name() == OsStr::new(allowed))
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn roots_are_same(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn env_path(name: &str) -> Result<Option<PathBuf>, CliError> {
    match std::env::var_os(name) {
        None => Ok(None),
        Some(value) if value.is_empty() => Err(CliError::user(
            "home_not_set",
            format!("{name} is set to an empty string"),
        )),
        Some(value) => Ok(Some(PathBuf::from(value))),
    }
}

fn env_utf8(name: &str, trim: bool) -> Result<Option<String>, CliError> {
    match std::env::var(name) {
        Ok(value) => {
            let value = if trim {
                value.trim().to_string()
            } else {
                value
            };
            Ok((!value.is_empty()).then_some(value))
        }
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(CliError::user(
            "invalid_environment",
            format!("environment variable {name} is not valid UTF-8"),
        )),
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, CliError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|e| {
                CliError::system("home_unreadable", format!("read current directory: {e}"))
            })
    }
}

fn validate_selected_home(path: &Path) -> Result<(), CliError> {
    match std::fs::metadata(path) {
        Ok(metadata) if !metadata.is_dir() => Err(CliError::user(
            "invalid_home",
            format!(
                "state home {} exists but is not a directory",
                path.display()
            ),
        )),
        Ok(_) => std::fs::read_dir(path).map(|_| ()).map_err(|e| {
            CliError::system(
                "home_unreadable",
                format!("could not inspect state home {}: {e}", path.display()),
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CliError::system(
            "home_unreadable",
            format!("could not inspect state home {}: {e}", path.display()),
        )),
    }
}

pub fn repository_config() -> Result<(PathBuf, Option<Vec<u8>>), CliError> {
    INPUTS
        .get()
        .and_then(|inputs| inputs.repository_config.as_ref())
        .map(|selection| (selection.path.clone(), selection.bytes.clone()))
        .ok_or_else(|| {
            CliError::system(
                "resolver_not_initialized",
                "repository config was not resolved during dispatcher preflight",
            )
        })
}

fn resolve_repository_config(
    source_repo: Option<&str>,
) -> Result<RepositoryConfigSelection, CliError> {
    let path = repository_root(source_repo)?.join(".taskfleet.toml");
    let bytes = read_optional(&path)?;
    Ok(RepositoryConfigSelection { path, bytes })
}

fn repository_root(source_repo: Option<&str>) -> Result<PathBuf, CliError> {
    let start = source_repo
        .map_or_else(std::env::current_dir, |raw| Ok(PathBuf::from(raw)))
        .map_err(|e| CliError::system("io_error", format!("read current directory: {e}")))?;
    if !start.is_dir() {
        return Err(CliError::user(
            "invalid_source_repo",
            format!(
                "source repository {} is not an existing directory",
                start.display()
            ),
        )
        .with_invalid_value(start.display().to_string()));
    }
    let canonical = start.canonicalize().map_err(|e| {
        CliError::system(
            "source_repo_unreadable",
            format!("resolve source repository {}: {e}", start.display()),
        )
    })?;
    for ancestor in canonical.ancestors() {
        let marker = ancestor.join(".git");
        match marker.try_exists() {
            Ok(true) => return Ok(ancestor.to_path_buf()),
            Ok(false) => {}
            Err(e) => {
                return Err(CliError::system(
                    "source_repo_unreadable",
                    format!("inspect repository marker {}: {e}", marker.display()),
                ))
            }
        }
    }
    Ok(canonical)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, CliError> {
    const MAX_BYTES: u64 = 64 * 1024;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(CliError::system(
                "repository_config_unreadable",
                format!(
                    "could not inspect repository config {}: {e}",
                    path.display()
                ),
            ))
        }
    };
    if !metadata.file_type().is_file() || metadata.len() > MAX_BYTES {
        return Err(CliError::user(
            "invalid_repository_config",
            format!(
                "repository config {} must be a regular file no larger than {MAX_BYTES} bytes",
                path.display()
            ),
        ));
    }
    std::fs::read(path).map(Some).map_err(|e| {
        CliError::system(
            "repository_config_unreadable",
            format!("could not read repository config {}: {e}", path.display()),
        )
    })
}
