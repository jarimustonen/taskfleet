//! User-owned executable profile resolution.
//!
//! Executable argv is accepted only from the user config. Repository config is
//! selection-only. Resolution is deterministic and fallback is create-time
//! static eligibility only; launch/runtime failures never advance candidates.

use std::path::Path;

use taskfleet_core::{
    AgentSelection, Kind, Lifecycle, SelectedAgentCandidate, SkippedAgentCandidate,
};

use crate::config::{AgentCandidate, Config, RepoConfig};
use crate::error::CliError;
use crate::run::kind_kebab;

pub const PROFILE_ENV: &str = crate::home::PROFILE_ENV;

/// Result of resolving profile-aware or backward-compatible harness selection.
#[derive(Debug)]
pub struct Resolution {
    pub harness: crate::harness::select::HarnessChoice,
    /// `None` is the deliberate no-profile compatibility mode. It preserves the
    /// old launcher and is rendered as `legacy-unrecorded` on read surfaces.
    pub selection: Option<AgentSelection>,
}

#[derive(Clone, Copy)]
enum Source {
    Cli,
    Environment,
    RepositoryPerKind,
    UserPerKind,
    RepositoryDefault,
    UserDefault,
}

impl Source {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Environment => "environment",
            Self::RepositoryPerKind => "repository-per-kind",
            Self::UserPerKind => "user-per-kind",
            Self::RepositoryDefault => "repository-default",
            Self::UserDefault => "user-default",
        }
    }
}

struct Requested<'a> {
    name: &'a str,
    harness_alias: bool,
    source: Source,
}

pub fn resolve(
    kind: Kind,
    lifecycle: Lifecycle,
    profile_flag: Option<&str>,
    harness_flag: Option<&str>,
    source_repo: Option<&str>,
) -> Result<Resolution, CliError> {
    if let Some(raw) = source_repo {
        if !Path::new(raw).is_dir() {
            return Err(CliError::user(
                "invalid_source_repo",
                format!("--source-repo '{raw}' is not an existing directory"),
            )
            .with_invalid_value(raw));
        }
    }
    let profile_env = crate::home::profile()?;
    let harness_env = crate::home::harness()?;
    let profile_flag = nonempty(profile_flag);
    let harness_flag = nonempty(harness_flag);
    let profile_env_ref = nonempty(profile_env.as_deref());
    let harness_env_ref = nonempty(harness_env.as_deref());

    if profile_flag.is_some() && harness_flag.is_some() {
        return conflicting("CLI", "--profile", "--harness");
    }
    if profile_flag.is_none() {
        if let Some(harness) = harness_flag {
            return legacy_harness(kind, Some(harness), None);
        }
        if profile_env_ref.is_some() && harness_env_ref.is_some() {
            return conflicting(
                "environment",
                PROFILE_ENV,
                crate::harness::select::HARNESS_ENV,
            );
        }
        if profile_env_ref.is_none() {
            if let Some(harness) = harness_env_ref {
                return legacy_harness(kind, None, Some(harness));
            }
        }
    }

    let config = Config::load()?;
    let higher_level_selected = profile_flag.is_some() || profile_env_ref.is_some();
    let repo = if higher_level_selected {
        RepoConfig::default()
    } else {
        RepoConfig::load()?
    };

    resolve_with(
        kind,
        lifecycle,
        profile_flag,
        harness_flag,
        profile_env_ref,
        harness_env_ref,
        &config,
        &repo,
        executable_available,
    )
}

fn legacy_harness(
    kind: Kind,
    flag: Option<&str>,
    env: Option<&str>,
) -> Result<Resolution, CliError> {
    Ok(Resolution {
        harness: crate::harness::select::resolve_with(kind, flag, env, &Config::default())?,
        selection: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn resolve_with(
    kind: Kind,
    lifecycle: Lifecycle,
    profile_flag: Option<&str>,
    harness_flag: Option<&str>,
    profile_env: Option<&str>,
    harness_env: Option<&str>,
    config: &Config,
    repo: &RepoConfig,
    available: impl Fn(&str) -> bool,
) -> Result<Resolution, CliError> {
    let profile_flag = nonempty(profile_flag);
    let harness_flag = nonempty(harness_flag);
    let profile_env = nonempty(profile_env);
    let harness_env = nonempty(harness_env);

    let kind_name = kind_kebab(kind);
    let user_profile_kind = config.profile.per_kind.get(kind_name).map(String::as_str);
    let user_harness_kind = config.harness.per_kind.get(kind_name).map(String::as_str);

    let requested = if profile_flag.is_some() || harness_flag.is_some() {
        if profile_flag.is_some() && harness_flag.is_some() {
            return conflicting("CLI", "--profile", "--harness");
        }
        profile_flag.map_or_else(
            || Requested {
                name: harness_flag.expect("one CLI selector is present"),
                harness_alias: true,
                source: Source::Cli,
            },
            |name| Requested {
                name,
                harness_alias: false,
                source: Source::Cli,
            },
        )
    } else if profile_env.is_some() || harness_env.is_some() {
        if profile_env.is_some() && harness_env.is_some() {
            return conflicting(
                "environment",
                PROFILE_ENV,
                crate::harness::select::HARNESS_ENV,
            );
        }
        profile_env.map_or_else(
            || Requested {
                name: harness_env.expect("one environment selector is present"),
                harness_alias: true,
                source: Source::Environment,
            },
            |name| Requested {
                name,
                harness_alias: false,
                source: Source::Environment,
            },
        )
    } else if let Some(name) = repo.profile.per_kind.get(kind_name) {
        Requested {
            name,
            harness_alias: false,
            source: Source::RepositoryPerKind,
        }
    } else if user_profile_kind.is_some() || user_harness_kind.is_some() {
        if user_profile_kind.is_some() && user_harness_kind.is_some() {
            return conflicting(
                "user per-kind config",
                &format!("profile.per_kind.{kind_name}"),
                &format!("harness.per_kind.{kind_name}"),
            );
        }
        user_profile_kind.map_or_else(
            || Requested {
                name: user_harness_kind.expect("one user per-kind selector is present"),
                harness_alias: true,
                source: Source::UserPerKind,
            },
            |name| Requested {
                name,
                harness_alias: false,
                source: Source::UserPerKind,
            },
        )
    } else if let Some(name) = repo.profile.default.as_deref() {
        Requested {
            name,
            harness_alias: false,
            source: Source::RepositoryDefault,
        }
    } else if config.profile.default.is_some() || config.harness.default.is_some() {
        if config.profile.default.is_some() && config.harness.default.is_some() {
            return conflicting("user default config", "profile.default", "harness.default");
        }
        config.profile.default.as_deref().map_or_else(
            || Requested {
                name: config
                    .harness
                    .default
                    .as_deref()
                    .expect("one user default selector is present"),
                harness_alias: true,
                source: Source::UserDefault,
            },
            |name| Requested {
                name,
                harness_alias: false,
                source: Source::UserDefault,
            },
        )
    } else if config.profiles.is_empty() {
        let harness = crate::harness::select::resolve_with(kind, None, None, config)?;
        return Ok(Resolution {
            harness,
            selection: None,
        });
    } else {
        return Err(CliError::user(
            "profile_required",
            "profiles are configured but no profile was selected; pass --profile, set TASKFLEET_PROFILE, or configure profile.default",
        ));
    };

    // With no profile definitions at all, preserve the pre-profile launcher.
    // An explicit profile request cannot be honored and still fails loudly.
    if config.profiles.is_empty() && requested.harness_alias {
        let harness =
            crate::harness::select::resolve_with(kind, harness_flag, harness_env, config)?;
        return Ok(Resolution {
            harness,
            selection: None,
        });
    }

    if requested.harness_alias {
        crate::harness::select::validate_harness_name(requested.name).map_err(|_| {
            CliError::user(
                "invalid_harness",
                format!(
                    "unknown legacy harness alias '{}'; known harnesses: {}",
                    requested.name,
                    crate::harness::KNOWN_HARNESSES.join(", ")
                ),
            )
            .with_invalid_value(requested.name)
            .with_expected(serde_json::json!(crate::harness::KNOWN_HARNESSES))
        })?;
    }
    let profile = config.profiles.get(requested.name).ok_or_else(|| {
        CliError::user(
            "unknown_profile",
            format!(
                "profile '{}' selected from {} is not defined in the user config; repository config and legacy harness aliases may only name user-owned profiles",
                requested.name,
                requested.source.as_str()
            ),
        )
        .with_invalid_value(requested.name)
        .with_expected(serde_json::json!(config.profiles.keys().collect::<Vec<_>>()))
    })?;

    if requested.harness_alias
        && profile
            .agents
            .iter()
            .any(|candidate| candidate.harness != requested.name)
    {
        return Err(CliError::user(
            "harness_alias_mismatch",
            format!(
                "legacy harness alias '{}' requires every candidate in profile '{}' to use harness '{}'",
                requested.name, requested.name, requested.name
            ),
        )
        .with_invalid_value(requested.name));
    }

    let mut fallback = Vec::new();
    let mut selected = None;
    for (index, candidate) in profile.agents.iter().enumerate() {
        let reason = skip_reason(candidate, lifecycle, &available);
        if let Some(reason) = reason {
            fallback.push(SkippedAgentCandidate {
                candidate_index: u8::try_from(index)
                    .expect("profiles contain at most 8 candidates"),
                harness: candidate.harness.clone(),
                reason: reason.to_string(),
            });
        } else {
            selected = Some(SelectedAgentCandidate {
                candidate_index: u8::try_from(index)
                    .expect("profiles contain at most 8 candidates"),
                harness: candidate.harness.clone(),
                command: candidate.command.clone(),
                telemetry: candidate.telemetry.clone(),
            });
            break;
        }
    }

    let interaction = match lifecycle {
        Lifecycle::Autonomous => "autonomous",
        Lifecycle::Interactive => "explicit-interactive",
    };
    let selected = selected.ok_or_else(|| {
        CliError::user(
            "profile_candidates_exhausted",
            format!(
                "profile '{}' has no eligible {interaction} candidate; skipped: {}",
                requested.name,
                fallback
                    .iter()
                    .map(|row| format!("{}:{}:{}", row.candidate_index, row.harness, row.reason))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
        .with_invalid_value(requested.name)
        .with_details(serde_json::json!({
            "selection": {
                "schema_version": 1,
                "profile": requested.name,
                "selection_source": requested.source.as_str(),
                "interaction": interaction,
                "capability": profile.capability.as_str(),
                "residency": profile.residency.as_str(),
                "requested_harness": requested.harness_alias.then_some(requested.name),
                "selected": null,
                "fallback": fallback,
            }
        }))
    })?;
    let harness = crate::harness::select::HarnessChoice {
        name: selected.harness.clone(),
        source: match requested.source {
            Source::Cli => crate::harness::select::HarnessSource::Flag,
            Source::Environment => crate::harness::select::HarnessSource::Env,
            Source::RepositoryPerKind
            | Source::UserPerKind
            | Source::RepositoryDefault
            | Source::UserDefault => crate::harness::select::HarnessSource::File,
        },
    };
    Ok(Resolution {
        harness,
        selection: Some(AgentSelection {
            schema_version: 1,
            profile: requested.name.to_string(),
            selection_source: requested.source.as_str().to_string(),
            interaction: interaction.to_string(),
            capability: profile.capability.as_str().to_string(),
            residency: profile.residency.as_str().to_string(),
            requested_harness: requested.harness_alias.then(|| requested.name.to_string()),
            selected,
            fallback,
        }),
    })
}

fn skip_reason(
    candidate: &AgentCandidate,
    lifecycle: Lifecycle,
    available: &impl Fn(&str) -> bool,
) -> Option<&'static str> {
    let Some(executable) = candidate.command.first() else {
        return Some("executable_missing");
    };
    if !available(executable) {
        Some("executable_missing")
    } else if lifecycle == Lifecycle::Autonomous
        && candidate.harness != "pi"
        && candidate.harness != "claude"
    {
        Some("autonomous_harness_unsupported")
    } else if lifecycle == Lifecycle::Autonomous
        && candidate.harness == "pi"
        && candidate.telemetry.as_deref() != Some("worker-v1")
    {
        Some("telemetry_unsupported")
    } else {
        None
    }
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn conflicting<T>(level: &str, profile: &str, harness: &str) -> Result<T, CliError> {
    Err(CliError::user(
        "conflicting_profile_selection",
        format!("{level} sets both '{profile}' and legacy '{harness}'; set exactly one at this precedence level"),
    ))
}

fn executable_available(executable: &str) -> bool {
    let path = Path::new(executable);
    if path.components().count() > 1 {
        return is_executable(path);
    }
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|directory| is_executable(&directory.join(executable)))
    })
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::config::{
        AgentProfile, ProfileCapability, ProfileResidency, ProfileSelectionConfig,
    };

    fn candidate(harness: &str, executable: &str, telemetry: Option<&str>) -> AgentCandidate {
        AgentCandidate {
            harness: harness.into(),
            command: vec![executable.into(), "--model".into(), "fictional".into()],
            telemetry: telemetry.map(str::to_string),
        }
    }

    fn config(agents: Vec<AgentCandidate>) -> Config {
        Config {
            profiles: BTreeMap::from([(
                "capable".into(),
                AgentProfile {
                    description: "General work".into(),
                    capability: ProfileCapability::Capable,
                    residency: ProfileResidency::Remote,
                    agents,
                },
            )]),
            profile: ProfileSelectionConfig {
                default: Some("capable".into()),
                per_kind: BTreeMap::new(),
            },
            ..Config::default()
        }
    }

    fn resolve_test(
        lifecycle: Lifecycle,
        config: &Config,
        repo: &RepoConfig,
        profile: Option<&str>,
        harness: Option<&str>,
    ) -> Result<Resolution, CliError> {
        resolve_with(
            Kind::Spinoff,
            lifecycle,
            profile,
            harness,
            None,
            None,
            config,
            repo,
            |exe| exe != "missing",
        )
    }

    #[test]
    fn autonomous_skips_in_exact_reason_order() {
        let cfg = config(vec![
            candidate("claude", "missing", None),
            candidate("pi", "pi", None),
            candidate("pi", "pi", None),
            candidate("pi", "pi-adapted", Some("worker-v1")),
        ]);
        let got = resolve_test(
            Lifecycle::Autonomous,
            &cfg,
            &RepoConfig::default(),
            None,
            None,
        )
        .unwrap()
        .selection
        .unwrap();
        assert_eq!(got.selected.candidate_index, 3);
        assert_eq!(got.selected.command[0], "pi-adapted");
        assert_eq!(
            got.fallback
                .iter()
                .map(|row| row.reason.as_str())
                .collect::<Vec<_>>(),
            [
                "executable_missing",
                "telemetry_unsupported",
                "telemetry_unsupported"
            ]
        );
    }

    #[test]
    fn autonomous_accepts_claude_without_pi_telemetry() {
        let cfg = config(vec![candidate("claude", "claude", None)]);
        let got = resolve_test(
            Lifecycle::Autonomous,
            &cfg,
            &RepoConfig::default(),
            None,
            None,
        )
        .unwrap()
        .selection
        .unwrap();
        assert_eq!(got.selected.harness, "claude");
        assert_eq!(got.selected.telemetry, None);
    }

    #[test]
    fn explicit_interactive_accepts_claude_without_telemetry() {
        let cfg = config(vec![candidate("claude", "claude", None)]);
        let got = resolve_test(
            Lifecycle::Interactive,
            &cfg,
            &RepoConfig::default(),
            None,
            None,
        )
        .unwrap()
        .selection
        .unwrap();
        assert_eq!(got.interaction, "explicit-interactive");
        assert_eq!(got.selected.harness, "claude");
    }

    #[test]
    fn repository_per_kind_beats_user_defaults_but_only_names_definition() {
        let mut cfg = config(vec![candidate("pi", "pi", Some("worker-v1"))]);
        cfg.profiles.insert(
            "secure".into(),
            AgentProfile {
                description: "Local".into(),
                capability: ProfileCapability::Fast,
                residency: ProfileResidency::Local,
                agents: vec![candidate("pi", "local-pi", Some("worker-v1"))],
            },
        );
        let repo = RepoConfig {
            profile: ProfileSelectionConfig {
                default: None,
                per_kind: BTreeMap::from([("spinoff".into(), "secure".into())]),
            },
        };
        let got = resolve_test(Lifecycle::Autonomous, &cfg, &repo, None, None)
            .unwrap()
            .selection
            .unwrap();
        assert_eq!(got.profile, "secure");
        assert_eq!(got.residency, "local");
        assert_eq!(got.selected.command[0], "local-pi");
        assert_eq!(got.selection_source, "repository-per-kind");
    }

    #[test]
    fn precedence_is_cli_env_repo_kind_user_kind_repo_default_user_default() {
        let mut cfg = config(vec![candidate("pi", "pi", Some("worker-v1"))]);
        for name in ["cli", "env", "repo-kind", "user-kind", "repo-default"] {
            cfg.profiles
                .insert(name.into(), cfg.profiles["capable"].clone());
        }
        cfg.profile
            .per_kind
            .insert("spinoff".into(), "user-kind".into());
        let repo = RepoConfig {
            profile: ProfileSelectionConfig {
                default: Some("repo-default".into()),
                per_kind: BTreeMap::from([("spinoff".into(), "repo-kind".into())]),
            },
        };
        let select = |profile_flag, profile_env, config: &Config, repo: &RepoConfig| {
            resolve_with(
                Kind::Spinoff,
                Lifecycle::Autonomous,
                profile_flag,
                None,
                profile_env,
                None,
                config,
                repo,
                |_| true,
            )
            .unwrap()
            .selection
            .unwrap()
        };
        assert_eq!(select(Some("cli"), Some("env"), &cfg, &repo).profile, "cli");
        assert_eq!(select(None, Some("env"), &cfg, &repo).profile, "env");
        assert_eq!(select(None, None, &cfg, &repo).profile, "repo-kind");
        let mut no_repo_kind = repo.clone();
        no_repo_kind.profile.per_kind.clear();
        assert_eq!(select(None, None, &cfg, &no_repo_kind).profile, "user-kind");
        let mut no_user_kind = cfg.clone();
        no_user_kind.profile.per_kind.clear();
        assert_eq!(
            select(None, None, &no_user_kind, &no_repo_kind).profile,
            "repo-default"
        );
        no_repo_kind.profile.default = None;
        assert_eq!(
            select(None, None, &no_user_kind, &no_repo_kind).profile,
            "capable"
        );
    }

    #[test]
    fn legacy_alias_requires_matching_candidate_harness() {
        let mut cfg = config(vec![candidate("pi", "pi", Some("worker-v1"))]);
        let profile = cfg.profiles.remove("capable").unwrap();
        cfg.profiles.insert("claude".into(), profile);
        cfg.profile.default = None;
        let err = resolve_test(
            Lifecycle::Interactive,
            &cfg,
            &RepoConfig::default(),
            None,
            Some("claude"),
        )
        .unwrap_err();
        assert_eq!(err.code, "harness_alias_mismatch");
    }

    #[test]
    fn no_profiles_preserves_legacy_default() {
        let got = resolve_test(
            Lifecycle::Autonomous,
            &Config::default(),
            &RepoConfig::default(),
            None,
            None,
        )
        .unwrap();
        assert_eq!(got.harness.name, "pi");
        assert!(got.selection.is_none());
    }

    #[test]
    fn higher_cli_layer_shadows_lower_config_conflicts() {
        let mut cfg = config(vec![candidate("pi", "pi", Some("worker-v1"))]);
        cfg.harness.default = Some("pi".into());
        cfg.profile
            .per_kind
            .insert("spinoff".into(), "capable".into());
        cfg.harness.per_kind.insert("spinoff".into(), "pi".into());
        let got = resolve_test(
            Lifecycle::Autonomous,
            &cfg,
            &RepoConfig::default(),
            Some("capable"),
            None,
        )
        .unwrap()
        .selection
        .unwrap();
        assert_eq!(got.selection_source, "cli");
    }

    #[test]
    fn same_level_inputs_conflict() {
        let err = resolve_test(
            Lifecycle::Autonomous,
            &config(vec![candidate("pi", "pi", Some("worker-v1"))]),
            &RepoConfig::default(),
            Some("capable"),
            Some("pi"),
        )
        .unwrap_err();
        assert_eq!(err.code, "conflicting_profile_selection");
    }
}
