//! User-facing configuration file — the `file` layer of the
//! flag > env > file > default precedence (AGENTS-AI-FIRST-CLI §8) — plus the
//! read-only `config` noun that inspects it.
//!
//! Two responsibilities live here:
//!
//! - [`Config`] / [`config_path`] — the loader for the `file` layer, consumed by
//!   the harness resolver ([`crate::harness::select`]).
//! - The `config` subcommand ([`ConfigAction`], [`dispatch`]) — `config path`
//!   prints the file location and `config show` prints a tolerant, layered view
//!   of raw and effective values (`env > file > default`), including validity
//!   for every layer. This inspection path does not weaken strict execution
//!   validation. Read-only: it never mutates the file. Verbs live in [`show`]
//!   and [`path`].
//!
//! Location: `<resolved Taskfleet home>/config.toml`. `TASKFLEET_HOME` is
//! authoritative when set; otherwise `home.rs` selects `~/.taskfleet` or keeps
//! a meaningful adopted `~/.orchestratectl` root in place. The file
//! is entirely optional; a missing or empty file yields [`Config::default`], so
//! every setting falls through to its built-in default. This is the first
//! config-file layer in the tool — before it, configuration was purely
//! environment-variable driven (see `home.rs`).
//!
//! It carries legacy `[harness]` aliases plus user-owned `[profile]` selection
//! and `[profiles.<name>]` executable definitions, consumed by
//! `run create --profile` / `--harness` (see [`crate::harness::profile`]):
//!
//! ```toml
//! [harness]
//! # Default harness for every run kind unless overridden below.
//! default = "pi"
//!
//! [harness.per_kind]
//! # Per-kind overrides, keyed by the kebab-case run kind. Lets a repo default
//! # autonomous work to pi while interactive `code` stays on the global default.
//! research = "pi"
//! spinoff = "pi"
//! ```
//!
//! Parsing is strict where it catches user mistakes, lenient where strictness
//! would only hurt forward-compat:
//!
//! - A syntactically invalid `config.toml` is a hard [`CliError`] (a silently
//!   ignored config is worse than a loud one — the caller would reason about a
//!   value the file was supposed to set).
//! - An unknown **key** inside `[harness]` (`defualt = …`) is rejected
//!   (`deny_unknown_fields` on [`HarnessConfig`]); a typo'd `[harness.per_kind]`
//!   run-kind key is validated against [`taskfleet_core::Kind::WIRE_NAMES`] and
//!   likewise rejected. Both fail loudly rather than silently no-op'ing.
//! - An unknown top-level **section** is tolerated (no `deny_unknown_fields` on
//!   [`Config`]) so a newer build's future `[section]` never bricks an older
//!   build reading the same home.

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::Subcommand;
use serde::Deserialize;

use crate::error::CliError;
use crate::output::OutputSpec;

pub mod path;
pub mod show;

/// Schema version of the `config` subcommand's JSON payloads (the
/// `schema_version_config` field on `config path` / `config show`). Bumped
/// independently of the run-state schema so an agent can pin the shape of the
/// config surface (AGENTS-AI-FIRST-CLI §10).
///
/// Version history:
/// - v1: effective-only `value` / `source` rows.
/// - v2: layered rows with independent validity, a unique `keys` collection,
///   unrecognized file entries, and a top-level validity summary.
pub const CONFIG_SCHEMA_VERSION: u32 = 2;

/// The `config` noun — read-only layered inspection of the configuration
/// (AGENTS-AI-FIRST-CLI §8). Never mutates `config.toml`.
#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Print the config file location (whether or not the file exists).
    Path,
    /// Print raw layered and effective configuration with per-layer validity.
    /// Invalid values are reported as warnings rather than hiding the value.
    /// Secret-valued keys are redacted unless `--show-secrets` is given.
    Show {
        /// Reveal secret-valued keys instead of redacting them. Emits a
        /// warning on stderr in text mode or in the JSON warnings envelope.
        /// No config key is secret today, so this is a forward-compatible
        /// no-op on the current surface.
        #[arg(long)]
        show_secrets: bool,
    },
}

/// Dispatch a `config` subcommand to its verb module.
pub fn dispatch(
    action: ConfigAction,
    spec: &OutputSpec,
    warnings: &[String],
) -> Result<(), CliError> {
    match action {
        ConfigAction::Path => path::run(spec, warnings),
        ConfigAction::Show { show_secrets } => show::run(show_secrets, spec, warnings),
    }
}

/// The parsed `config.toml`. All sections optional; an absent section leaves the
/// corresponding settings at their built-in defaults.
///
/// Deliberately NOT `deny_unknown_fields` at the top level: an unknown *section*
/// (e.g. a future `[ui]` written by a newer build sharing the same home) must not
/// brick an older build's `run create`. Strictness lives inside every known
/// harness/profile section and candidate.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct Config {
    /// `[harness]` — legacy harness-selection aliases for `run create`.
    #[serde(default)]
    pub harness: HarnessConfig,
    /// `[profile]` — user-level profile selection defaults.
    #[serde(default)]
    pub profile: ProfileSelectionConfig,
    /// `[profiles.<name>]` — user-owned executable definitions. Repository
    /// configuration is parsed through [`RepoConfig`] and cannot define these.
    #[serde(default)]
    pub profiles: BTreeMap<String, AgentProfile>,
}

/// User/repository profile-name defaults. This section contains names only;
/// executable content belongs exclusively to user [`Config::profiles`].
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProfileSelectionConfig {
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub per_kind: BTreeMap<String, String>,
}

/// A user-owned executable profile.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentProfile {
    pub description: String,
    pub capability: ProfileCapability,
    pub residency: ProfileResidency,
    pub agents: Vec<AgentCandidate>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileCapability {
    Fast,
    Capable,
    UltraCapable,
}

impl ProfileCapability {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Capable => "capable",
            Self::UltraCapable => "ultra-capable",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileResidency {
    Local,
    Remote,
}

impl ProfileResidency {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
        }
    }
}

/// One ordered argv candidate. `command` is argv, never a shell string.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentCandidate {
    pub harness: String,
    pub command: Vec<String>,
    #[serde(default)]
    pub telemetry: Option<String>,
}

/// Selection-only repository configuration from canonical
/// `<repo>/.taskfleet.toml` or its bounded legacy fallback.
/// `deny_unknown_fields` is load-bearing: `[profiles]`, commands, argv, adapter
/// paths, and residency reclassification all fail rather than becoming trusted
/// executable input from a checkout.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepoConfig {
    #[serde(default)]
    pub profile: ProfileSelectionConfig,
}

/// The `[harness]` section: a global default plus per-kind overrides.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HarnessConfig {
    /// Default harness for all run kinds unless a `per_kind` entry overrides it.
    /// `None` (key absent) falls through to the built-in default (`pi`).
    #[serde(default)]
    pub default: Option<String>,
    /// Per-kind overrides, keyed by kebab-case run kind (`research`, `spinoff`,
    /// …). A `BTreeMap` keeps the parse deterministic. Because `deny_unknown_fields`
    /// cannot police map keys, [`Config::load_from`] validates every key against
    /// [`taskfleet_core::Kind::WIRE_NAMES`] at load time — a typo'd kind (`reserach`)
    /// fails loudly instead of silently no-op'ing (which would defeat the whole
    /// point of the override).
    #[serde(default)]
    pub per_kind: BTreeMap<String, String>,
}

/// The config file path under the resolved Taskfleet home. Inspectable so a
/// caller never has to guess where settings come from (AGENTS-AI-FIRST-CLI §8).
pub fn config_path() -> Result<PathBuf, CliError> {
    Ok(crate::home::root_dir()?.join("config.toml"))
}

impl Config {
    /// Load the config from the default [`config_path`]. A missing file is not an
    /// error — it yields [`Config::default`]. A present-but-malformed file (bad
    /// TOML, unknown key, wrong value type) is a hard [`CliError`] naming the
    /// path, so a typo can never silently drop a setting.
    pub fn load() -> Result<Self, CliError> {
        Self::load_from(&config_path()?)
    }

    /// Load from an explicit path (the seam the tests drive).
    pub fn load_from(path: &std::path::Path) -> Result<Self, CliError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            // Absent config → all defaults. This is the overwhelmingly common
            // case (most users never write a config.toml).
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(CliError::system(
                    "config_unreadable",
                    format!("could not read config file {}: {e}", path.display()),
                ));
            }
        };
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        let config: Self = toml::from_str(&text).map_err(|e| {
            CliError::user(
                "invalid_config",
                format!("config file {} is not valid: {e}", path.display()),
            )
        })?;
        // `deny_unknown_fields` guards the `[harness]` struct fields but not the
        // dynamic `per_kind` map keys, so validate them here — a typo'd kind
        // (`reserach = "pi"`) is a mistake the user wants surfaced, not silently
        // ignored at lookup time.
        validate_kind_keys(path, "harness.per_kind", &config.harness.per_kind)?;
        validate_kind_keys(path, "profile.per_kind", &config.profile.per_kind)?;
        validate_profile_reference(path, config.profile.default.as_deref())?;
        for value in config.profile.per_kind.values() {
            validate_profile_reference(path, Some(value))?;
        }
        validate_profiles(path, &config.profiles)?;
        Ok(config)
    }
}

impl RepoConfig {
    /// Load the repository selection frozen by dispatcher preflight. The bytes
    /// are never reopened after logging starts.
    pub fn load() -> Result<Self, CliError> {
        let (path, bytes) = crate::home::repository_config()?;
        match bytes {
            Some(bytes) => Self::load_bytes(&path, &bytes),
            None => Ok(Self::default()),
        }
    }

    /// Load selection-only repository configuration. A missing file is empty;
    /// malformed or executable-bearing repository configuration fails closed.
    #[cfg(test)]
    pub fn load_from(path: &std::path::Path) -> Result<Self, CliError> {
        const MAX_REPOSITORY_CONFIG_BYTES: u64 = 64 * 1024;
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(CliError::system(
                    "repository_config_unreadable",
                    format!(
                        "could not inspect repository config {}: {e}",
                        path.display()
                    ),
                ));
            }
        };
        if !metadata.file_type().is_file() || metadata.len() > MAX_REPOSITORY_CONFIG_BYTES {
            return Err(CliError::user(
                "invalid_repository_config",
                format!(
                    "repository config {} must be a regular file no larger than {MAX_REPOSITORY_CONFIG_BYTES} bytes",
                    path.display()
                ),
            ));
        }
        let bytes = std::fs::read(path).map_err(|e| {
            CliError::system(
                "repository_config_unreadable",
                format!("could not read repository config {}: {e}", path.display()),
            )
        })?;
        Self::load_bytes(path, &bytes)
    }

    fn load_bytes(path: &std::path::Path, bytes: &[u8]) -> Result<Self, CliError> {
        const MAX_REPOSITORY_CONFIG_BYTES: usize = 64 * 1024;
        if bytes.len() > MAX_REPOSITORY_CONFIG_BYTES {
            return Err(CliError::user(
                "invalid_repository_config",
                format!("repository config {} must be no larger than {MAX_REPOSITORY_CONFIG_BYTES} bytes", path.display()),
            ));
        }
        let text = std::str::from_utf8(bytes).map_err(|e| {
            CliError::user(
                "invalid_repository_config",
                format!("repository config {} is not UTF-8: {e}", path.display()),
            )
        })?;
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        let config: Self = toml::from_str(text).map_err(|e| {
            CliError::user(
                "invalid_repository_config",
                format!(
                    "repository config {} is selection-only and is not valid: {}; executable profiles, commands, argv, adapter paths, and residency belong only in the user config",
                    path.display(),
                    e.message()
                ),
            )
        })?;
        validate_kind_keys(path, "profile.per_kind", &config.profile.per_kind)?;
        validate_profile_reference(path, config.profile.default.as_deref())?;
        for value in config.profile.per_kind.values() {
            validate_profile_reference(path, Some(value))?;
        }
        Ok(config)
    }
}

fn validate_kind_keys(
    path: &std::path::Path,
    section: &str,
    values: &BTreeMap<String, String>,
) -> Result<(), CliError> {
    for key in values.keys() {
        if !taskfleet_core::Kind::WIRE_NAMES.contains(&key.as_str()) {
            return Err(CliError::user(
                "invalid_config",
                format!(
                    "config file {} has an unknown run kind '{key}' in [{section}]; valid kinds: {}",
                    path.display(),
                    taskfleet_core::Kind::WIRE_NAMES.join(", ")
                ),
            )
            .with_invalid_value(key.clone()));
        }
    }
    Ok(())
}

fn valid_profile_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
        && !name.ends_with('-')
        && !name.contains("--")
}

fn validate_profile_reference(path: &std::path::Path, value: Option<&str>) -> Result<(), CliError> {
    if let Some(name) = value {
        if !valid_profile_name(name) {
            return Err(CliError::user(
                "invalid_profile_name",
                format!(
                    "config file {} has invalid profile name '{name}'; expected lowercase letters/digits with single hyphens, starting with a letter (max 63 characters)",
                    path.display()
                ),
            )
            .with_invalid_value(name));
        }
    }
    Ok(())
}

fn validate_profiles(
    path: &std::path::Path,
    profiles: &BTreeMap<String, AgentProfile>,
) -> Result<(), CliError> {
    for (name, profile) in profiles {
        validate_profile_reference(path, Some(name))?;
        if profile.description.trim().is_empty() || profile.description.len() > 512 {
            return Err(CliError::user(
                "invalid_profile",
                format!("profile '{name}' description must be 1..=512 characters"),
            ));
        }
        if profile.agents.is_empty() || profile.agents.len() > 8 {
            return Err(CliError::user(
                "invalid_profile",
                format!("profile '{name}' agents must contain 1..=8 candidates"),
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for (index, candidate) in profile.agents.iter().enumerate() {
            if !crate::harness::KNOWN_HARNESSES.contains(&candidate.harness.as_str()) {
                return Err(CliError::user(
                    "invalid_profile",
                    format!(
                        "profile '{name}' candidate {index} has unknown harness '{}'; expected pi or claude",
                        candidate.harness
                    ),
                )
                .with_invalid_value(&candidate.harness));
            }
            if candidate.command.is_empty() || candidate.command.len() > 32 {
                return Err(CliError::user(
                    "invalid_profile",
                    format!(
                        "profile '{name}' candidate {index} command must contain 1..=32 argv items"
                    ),
                ));
            }
            let mut total = 0usize;
            for arg in &candidate.command {
                if arg.is_empty() || arg.len() > 4096 || arg.contains('\0') {
                    return Err(CliError::user(
                        "invalid_profile",
                        format!("profile '{name}' candidate {index} has an empty, oversized, or NUL-containing argv item"),
                    ));
                }
                total = total.saturating_add(arg.len());
            }
            if total > 16_384 {
                return Err(CliError::user(
                    "invalid_profile",
                    format!("profile '{name}' candidate {index} command exceeds 16384 bytes"),
                ));
            }
            let executable = std::path::Path::new(&candidate.command[0]);
            if executable.components().count() > 1 && !executable.is_absolute() {
                return Err(CliError::user(
                    "invalid_profile",
                    format!("profile '{name}' candidate {index} executable must be absolute or a bare PATH name; relative paths with separators are ambiguous"),
                )
                .with_invalid_value(&candidate.command[0]));
            }
            match (candidate.harness.as_str(), candidate.telemetry.as_deref()) {
                (_, None) | ("pi", Some("worker-v1")) => {}
                ("pi", Some(value)) => {
                    return Err(CliError::user(
                        "invalid_profile",
                        format!("profile '{name}' candidate {index} has unknown telemetry '{value}'; expected worker-v1"),
                    )
                    .with_invalid_value(value));
                }
                ("claude", Some(_)) => {
                    return Err(CliError::user(
                        "invalid_profile",
                        format!("profile '{name}' candidate {index}: telemetry is supported only for pi"),
                    ));
                }
                _ => {
                    return Err(CliError::user(
                        "invalid_profile",
                        format!("profile '{name}' candidate {index} has an unsupported harness/telemetry combination"),
                    ));
                }
            }
            if !seen.insert((
                candidate.harness.clone(),
                candidate.command.clone(),
                candidate.telemetry.clone(),
            )) {
                return Err(CliError::user(
                    "invalid_profile",
                    format!("profile '{name}' contains duplicate candidate {index}"),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &TempDir, body: &str) -> PathBuf {
        let p = dir.path().join("config.toml");
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn missing_file_is_default() {
        let dir = TempDir::new().unwrap();
        let cfg = Config::load_from(&dir.path().join("nope.toml")).unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn empty_file_is_default() {
        let dir = TempDir::new().unwrap();
        let p = write(&dir, "   \n\n");
        assert_eq!(Config::load_from(&p).unwrap(), Config::default());
    }

    #[test]
    fn parses_default_and_per_kind() {
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            r#"
[harness]
default = "pi"

[harness.per_kind]
research = "pi"
spinoff = "claude"
"#,
        );
        let cfg = Config::load_from(&p).unwrap();
        assert_eq!(cfg.harness.default.as_deref(), Some("pi"));
        assert_eq!(
            cfg.harness.per_kind.get("research").map(String::as_str),
            Some("pi")
        );
        assert_eq!(
            cfg.harness.per_kind.get("spinoff").map(String::as_str),
            Some("claude")
        );
    }

    #[test]
    fn unknown_key_is_rejected() {
        let dir = TempDir::new().unwrap();
        let p = write(&dir, "[harness]\ndefualt = \"pi\"\n");
        let err = Config::load_from(&p).unwrap_err();
        assert_eq!(err.code, "invalid_config");
    }

    #[test]
    fn malformed_toml_is_hard_error() {
        let dir = TempDir::new().unwrap();
        let p = write(&dir, "[harness\n");
        let err = Config::load_from(&p).unwrap_err();
        assert_eq!(err.code, "invalid_config");
    }

    #[test]
    fn unknown_per_kind_run_kind_is_rejected() {
        // deny_unknown_fields cannot police map keys, so load-time validation
        // must: a typo'd run kind fails loudly instead of silently no-op'ing.
        let dir = TempDir::new().unwrap();
        let p = write(&dir, "[harness.per_kind]\nreserach = \"pi\"\n");
        let err = Config::load_from(&p).unwrap_err();
        assert_eq!(err.code, "invalid_config");
        assert_eq!(err.invalid_value.as_deref(), Some("reserach"));
    }

    #[test]
    fn valid_per_kind_run_kinds_pass() {
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "[harness.per_kind]\nresearch = \"pi\"\ntechnical-decision = \"claude\"\n",
        );
        let cfg = Config::load_from(&p).unwrap();
        assert_eq!(
            cfg.harness.per_kind.get("research").map(String::as_str),
            Some("pi")
        );
    }

    #[test]
    fn parses_and_round_trips_strict_profile_argv() {
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            r#"
[profiles.secure]
description = "Local fictional worker"
capability = "fast"
residency = "local"
agents = [{ harness = "pi", command = ["/opt/fictional pi", "--model", "tiny"], telemetry = "worker-v1" }]
[profile]
default = "secure"
"#,
        );
        let cfg = Config::load_from(&p).unwrap();
        assert_eq!(
            cfg.profiles["secure"].agents[0].command,
            ["/opt/fictional pi", "--model", "tiny"]
        );
        assert_eq!(cfg.profiles["secure"].residency, ProfileResidency::Local);
    }

    #[test]
    fn rejects_unbounded_unknown_and_contradictory_profile_content() {
        let dir = TempDir::new().unwrap();
        for body in [
            r#"[profiles.bad]
description="x"
capability="capable"
residency="remote"
agents=[]
"#,
            r#"[profiles.bad]
description="x"
capability="capable"
residency="remote"
agents=[{harness="claude",command=["claude"],telemetry="worker-v1"}]
"#,
            r#"[profiles.bad]
description="x"
capability="capable"
residency="remote"
agents=[{harness="pi",command=["pi"],surprise=true}]
"#,
        ] {
            let p = write(&dir, body);
            assert!(Config::load_from(&p).is_err(), "accepted {body}");
        }
    }

    #[test]
    fn repository_config_accepts_selection_and_rejects_executable_definitions() {
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "[profile]\ndefault=\"secure\"\n[profile.per_kind]\nspinoff=\"capable\"\n",
        );
        assert_eq!(
            RepoConfig::load_from(&p)
                .unwrap()
                .profile
                .default
                .as_deref(),
            Some("secure")
        );
        let p = write(&dir, "[profiles.secure]\nresidency=\"local\"\n");
        assert_eq!(
            RepoConfig::load_from(&p).unwrap_err().code,
            "invalid_repository_config"
        );
    }

    #[test]
    fn unknown_top_level_section_is_tolerated() {
        // Forward-compat: a future section from a newer build must not brick an
        // older build reading the same home.
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "[ui]\ntheme = \"dark\"\n\n[harness]\ndefault = \"pi\"\n",
        );
        let cfg = Config::load_from(&p).unwrap();
        assert_eq!(cfg.harness.default.as_deref(), Some("pi"));
    }
}
