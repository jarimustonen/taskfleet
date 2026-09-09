//! `config show` — tolerant, layered configuration inspection (§8).
//!
//! Unlike execution, inspection does not deserialize through [`crate::config::Config`]: it
//! parses the file as raw TOML and validates every harness layer independently.
//! This lets a caller see an invalid file value, including one shadowed by
//! `TASKFLEET_HARNESS`, without weakening the strict resolver used by
//! `run create`. Only unreadable or syntactically invalid TOML is fatal.
//!
//! Each key has an ordered `layers` stack (highest precedence first), an
//! `effective_value` / `effective_source`, and effective-layer validity. File
//! layers carry `origin_key`, which distinguishes a per-kind override from the
//! inherited `[harness] default`. Invalid layers carry `validation_error` and
//! also produce an envelope warning, even when a valid higher layer shadows
//! them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;
use taskfleet_core::Kind;

use crate::config::{config_path, CONFIG_SCHEMA_VERSION};
use crate::error::CliError;
use crate::harness::{
    select::{validate_harness_name, HarnessNameError, HARNESS_ENV},
    DEFAULT_HARNESS, KNOWN_HARNESSES,
};
use crate::output::{self, OutputFormat, OutputSpec};
use crate::run::kind_kebab;

const REDACTED: &str = "<redacted>";

const CREATABLE_KINDS: &[Kind] = &[
    Kind::Spinoff,
    Kind::Research,
    Kind::TechnicalDecision,
    Kind::FanOut,
];

#[derive(Debug, Serialize)]
struct ConfigShowPayload {
    schema_version_config: u32,
    path: String,
    exists: bool,
    /// True only when every physical layer is valid and no schema-invalid
    /// harness entry was found.
    valid: bool,
    invalid_layer_count: usize,
    /// Known precedence keys. `key` is unique within this array.
    keys: Vec<ConfigKey>,
    /// Parseable file entries that the strict harness schema would reject.
    unrecognized: Vec<UnrecognizedConfig>,
}

#[derive(Debug, Serialize)]
struct UnrecognizedConfig {
    origin_key: String,
    value: String,
    valid: bool,
    validation_error: String,
}

#[derive(Debug, Serialize)]
struct ConfigKey {
    key: String,
    effective_value: String,
    effective_source: &'static str,
    /// Validity of the effective layer. Shadowed layers retain independent
    /// validity in `layers`, so an env override cannot launder a bad file value.
    valid: bool,
    validation_error: Option<String>,
    secret: bool,
    /// Highest precedence first. Exactly one layer is `active`.
    layers: Vec<ConfigLayer>,
}

#[derive(Debug, Clone, Serialize)]
struct ConfigLayer {
    value: String,
    source: &'static str,
    /// Exact TOML key for file layers; absent for env and built-in defaults.
    origin_key: Option<String>,
    valid: bool,
    validation_error: Option<String>,
    active: bool,
}

#[derive(Default)]
struct RawHarness {
    default: Option<toml::Value>,
    per_kind: BTreeMap<String, toml::Value>,
    unknown: BTreeMap<String, toml::Value>,
    section_error: Option<(String, String)>,
    tmux: Option<toml::Value>,
}

impl ConfigLayer {
    fn harness(value: String, source: &'static str, origin_key: Option<String>) -> Self {
        let validation_error = harness_validation_error(&value);
        Self {
            value,
            source,
            origin_key,
            valid: validation_error.is_none(),
            validation_error,
            active: false,
        }
    }

    fn invalid_file(value: String, origin_key: String, error: String) -> Self {
        Self {
            value,
            source: "file",
            origin_key: Some(origin_key),
            valid: false,
            validation_error: Some(error),
            active: false,
        }
    }
}

impl ConfigKey {
    fn raw(
        key: impl Into<String>,
        value: String,
        source: &'static str,
        valid: bool,
        validation_error: Option<String>,
    ) -> Self {
        Self {
            key: key.into(),
            effective_value: value.clone(),
            effective_source: source,
            valid,
            validation_error: validation_error.clone(),
            secret: false,
            layers: vec![ConfigLayer {
                value,
                source,
                origin_key: (source == "file").then(|| "tmux".into()),
                valid,
                validation_error,
                active: true,
            }],
        }
    }

    fn from_layers(key: impl Into<String>, mut layers: Vec<ConfigLayer>) -> Self {
        let effective = layers
            .first_mut()
            .expect("every known config key has a built-in default layer");
        effective.active = true;
        let effective_value = validate_harness_name(&effective.value)
            .map_or_else(|_| effective.value.clone(), str::to_owned);
        Self {
            key: key.into(),
            effective_value,
            effective_source: effective.source,
            valid: effective.valid,
            validation_error: effective.validation_error.clone(),
            secret: false,
            layers,
        }
    }
}

pub fn run(show_secrets: bool, spec: &OutputSpec, warnings: &[String]) -> Result<(), CliError> {
    let path = config_path()?;
    let raw = load_raw_harness(&path)?;
    let env = crate::home::harness()?;
    // Match execution semantics: an empty/whitespace env value is unset.
    let env = env.as_deref().map(str::trim).filter(|v| !v.is_empty());

    let mut keys = Vec::with_capacity(1 + CREATABLE_KINDS.len() + raw.unknown.len());
    keys.push(ConfigKey::from_layers(
        "harness.default",
        harness_layers(
            env,
            raw.default
                .as_ref()
                .map(|value| (value, "harness.default".to_string())),
            None,
        ),
    ));
    for &kind in CREATABLE_KINDS {
        let name = kind_kebab(kind);
        keys.push(ConfigKey::from_layers(
            format!("harness.{name}"),
            harness_layers(
                env,
                raw.per_kind
                    .get(name)
                    .map(|value| (value, format!("harness.per_kind.{name}"))),
                raw.default
                    .as_ref()
                    .map(|value| (value, "harness.default".to_string())),
            ),
        ));
    }

    append_tmux_keys(raw.tmux.as_ref(), &mut keys);

    // Parseable but schema-invalid entries are inspection data, not fatal
    // errors. Keep them outside `keys`, whose logical key identifiers stay
    // unique and always describe a real precedence stack.
    let mut unrecognized = Vec::new();
    if let Some((value, error)) = raw.section_error {
        unrecognized.push(UnrecognizedConfig {
            origin_key: "harness".into(),
            value,
            valid: false,
            validation_error: error,
        });
    }
    for (name, value) in raw.unknown {
        let origin_key = format!("harness.{name}");
        let validation_error = if name == "per_kind" {
            format!(
                "expected [harness.per_kind] table, found {}",
                value.type_str()
            )
        } else {
            "unknown key in [harness]; expected default or per_kind".into()
        };
        unrecognized.push(UnrecognizedConfig {
            origin_key,
            value: raw_value(&value),
            valid: false,
            validation_error,
        });
    }
    for (name, value) in raw
        .per_kind
        .iter()
        .filter(|(name, _)| !Kind::WIRE_NAMES.contains(&name.as_str()))
    {
        unrecognized.push(UnrecognizedConfig {
            origin_key: format!("harness.per_kind.{name}"),
            value: raw_value(value),
            valid: false,
            validation_error: format!(
                "unknown run kind '{name}'; valid kinds: {}",
                Kind::WIRE_NAMES.join(", ")
            ),
        });
    }

    let mut command_warnings = warnings.to_vec();
    let any_secret = keys.iter().any(|key| key.secret);
    if show_secrets {
        add_show_secrets_warning(any_secret, &mut command_warnings);
    } else {
        for key in keys.iter_mut().filter(|key| key.secret) {
            redact_secret_key(key);
        }
    }
    let invalid_layer_count =
        append_validation_warnings(&keys, &unrecognized, &mut command_warnings);

    let payload = ConfigShowPayload {
        schema_version_config: CONFIG_SCHEMA_VERSION,
        path: path.display().to_string(),
        exists: path.exists(),
        valid: invalid_layer_count == 0,
        invalid_layer_count,
        keys,
        unrecognized,
    };
    match spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => {
            output::emit_envelope(&payload, spec, &command_warnings)?;
        }
        OutputFormat::Text => {
            println!("path:   {}", payload.path);
            println!("exists: {}", payload.exists);
            println!("valid:  {}", payload.valid);
            for key in &payload.keys {
                let validity = if key.valid { "valid" } else { "INVALID" };
                println!(
                    "{} = {} ({}, {validity})",
                    key.key, key.effective_value, key.effective_source
                );
                for layer in &key.layers {
                    let marker = if layer.active { "*" } else { " " };
                    let origin = layer
                        .origin_key
                        .as_deref()
                        .map(|origin| format!(" {origin}"))
                        .unwrap_or_default();
                    let validity = if layer.valid { "valid" } else { "INVALID" };
                    println!(
                        "  {marker} {:<7} {:<10} ({validity}){}",
                        layer.source, layer.value, origin
                    );
                    if let Some(error) = &layer.validation_error {
                        println!("      validation_error: {error}");
                    }
                }
            }
            for entry in &payload.unrecognized {
                println!("{} = {} (file, INVALID)", entry.origin_key, entry.value);
                println!("      validation_error: {}", entry.validation_error);
            }
            output::emit_text_warnings(&command_warnings);
        }
    }
    Ok(())
}

fn append_tmux_keys(raw: Option<&toml::Value>, keys: &mut Vec<ConfigKey>) {
    let parsed = raw
        .cloned()
        .map(toml::Value::try_into::<crate::config::TmuxConfig>)
        .transpose();
    match parsed {
        Ok(config) => {
            let config = config.unwrap_or_default();
            let policy_error = config.retention_policy().err().map(|error| error.message);
            let session_error = config
                .default_session
                .as_deref()
                .and_then(|value| crate::config::validate_tmux_session_name(value).err())
                .map(|error| error.message)
                .or_else(|| policy_error.clone());
            let source = if raw.is_some() { "file" } else { "default" };
            keys.push(ConfigKey::raw(
                "tmux.default_session",
                config.default_session.unwrap_or_else(|| "<unset>".into()),
                source,
                session_error.is_none(),
                session_error,
            ));
            keys.push(ConfigKey::raw(
                "tmux.persistent",
                config.persistent.to_string(),
                source,
                policy_error.is_none(),
                policy_error.clone(),
            ));
            keys.push(ConfigKey::raw(
                "tmux.completed_window_ttl",
                config
                    .completed_window_ttl
                    .unwrap_or_else(|| "<unset>".into()),
                source,
                policy_error.is_none(),
                policy_error.clone(),
            ));
            keys.push(ConfigKey::raw(
                "tmux.completed_window_max",
                config
                    .completed_window_max
                    .map_or_else(|| "<unset>".into(), |value| value.to_string()),
                source,
                policy_error.is_none(),
                policy_error,
            ));
        }
        Err(error) => {
            let detail = format!("invalid [tmux] section: {error}");
            for key in [
                "default_session",
                "persistent",
                "completed_window_ttl",
                "completed_window_max",
            ] {
                keys.push(ConfigKey::raw(
                    format!("tmux.{key}"),
                    raw.map_or_else(|| "<unset>".into(), raw_value),
                    "file",
                    false,
                    Some(detail.clone()),
                ));
            }
        }
    }
}

fn harness_layers(
    env: Option<&str>,
    specific_file: Option<(&toml::Value, String)>,
    inherited_file: Option<(&toml::Value, String)>,
) -> Vec<ConfigLayer> {
    let mut layers = Vec::new();
    if let Some(value) = env {
        layers.push(ConfigLayer::harness(value.into(), "env", None));
    }
    if let Some((value, origin)) = specific_file {
        layers.push(file_harness_layer(value, &origin));
    }
    if let Some((value, origin)) = inherited_file {
        layers.push(file_harness_layer(value, &origin));
    }
    layers.push(ConfigLayer::harness(
        DEFAULT_HARNESS.into(),
        "default",
        None,
    ));
    layers
}

fn file_harness_layer(value: &toml::Value, origin: &str) -> ConfigLayer {
    match value.as_str() {
        Some(value) => ConfigLayer::harness(value.into(), "file", Some(origin.into())),
        None => ConfigLayer::invalid_file(
            raw_value(value),
            origin.into(),
            format!("expected a string harness name, found {}", value.type_str()),
        ),
    }
}

fn redact_secret_key(key: &mut ConfigKey) {
    key.effective_value = REDACTED.into();
    if key.validation_error.is_some() {
        key.validation_error = Some("invalid secret value (redacted)".into());
    }
    for layer in &mut key.layers {
        layer.value = REDACTED.into();
        if layer.validation_error.is_some() {
            layer.validation_error = Some("invalid secret value (redacted)".into());
        }
    }
}

fn append_validation_warnings(
    keys: &[ConfigKey],
    unrecognized: &[UnrecognizedConfig],
    warnings: &mut Vec<String>,
) -> usize {
    let mut physical_invalid = BTreeSet::new();
    for key in keys {
        for layer in key.layers.iter().filter(|layer| !layer.valid) {
            let origin = layer.origin_key.as_deref().unwrap_or_else(|| {
                if layer.source == "env" {
                    HARNESS_ENV
                } else {
                    &key.key
                }
            });
            physical_invalid.insert((
                layer.source.to_string(),
                origin.to_string(),
                layer.value.clone(),
                layer.validation_error.clone().unwrap_or_default(),
            ));
        }
    }
    for entry in unrecognized {
        physical_invalid.insert((
            "file".into(),
            entry.origin_key.clone(),
            entry.value.clone(),
            entry.validation_error.clone(),
        ));
    }
    for (source, origin, value, error) in &physical_invalid {
        warnings.push(format!(
            "{origin} ({source}) value '{value}' is invalid: {error}"
        ));
    }
    physical_invalid.len()
}

fn add_show_secrets_warning(any_secret: bool, warnings: &mut Vec<String>) {
    if any_secret {
        // JSON warnings belong in the stdout envelope (§10), never stderr.
        warnings.push("--show-secrets: secret-valued config keys are shown in plaintext".into());
    }
}

fn harness_validation_error(value: &str) -> Option<String> {
    match validate_harness_name(value) {
        Ok(_) => None,
        Err(HarnessNameError::Empty) => Some(format!(
            "empty harness name; known harnesses: {}",
            KNOWN_HARNESSES.join(", ")
        )),
        Err(HarnessNameError::Unknown) => Some(format!(
            "unknown harness '{}'; known harnesses: {}",
            value.trim(),
            KNOWN_HARNESSES.join(", ")
        )),
    }
}

fn raw_value(value: &toml::Value) -> String {
    value.as_str().map_or_else(
        || serde_json::to_string(value).unwrap_or_else(|_| format!("{value:?}")),
        str::to_owned,
    )
}

fn load_raw_harness(path: &Path) -> Result<RawHarness, CliError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RawHarness::default());
        }
        Err(error) => {
            return Err(CliError::system(
                "config_unreadable",
                format!("could not read config file {}: {error}", path.display()),
            ));
        }
    };
    if text.trim().is_empty() {
        return Ok(RawHarness::default());
    }
    let document: toml::Value = toml::from_str(&text).map_err(|error| {
        CliError::user(
            "invalid_config",
            format!("config file {} is not valid TOML: {error}", path.display()),
        )
    })?;
    let tmux = document.get("tmux").cloned();
    let Some(harness) = document.get("harness") else {
        return Ok(RawHarness {
            tmux,
            ..RawHarness::default()
        });
    };
    let Some(table) = harness.as_table() else {
        return Ok(RawHarness {
            section_error: Some((
                raw_value(harness),
                format!("expected [harness] table, found {}", harness.type_str()),
            )),
            tmux,
            ..RawHarness::default()
        });
    };

    let mut raw = RawHarness {
        tmux,
        ..RawHarness::default()
    };
    for (name, value) in table {
        match name.as_str() {
            "default" => raw.default = Some(value.clone()),
            "per_kind" => match value.as_table() {
                Some(per_kind) => raw.per_kind = per_kind.clone().into_iter().collect(),
                None => {
                    raw.unknown.insert(name.clone(), value.clone());
                }
            },
            _ => {
                raw.unknown.insert(name.clone(), value.clone());
            }
        }
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creatable_kinds_match_wire_names() {
        let names: Vec<&str> = CREATABLE_KINDS
            .iter()
            .map(|kind| kind.wire_name())
            .collect();
        assert_eq!(names, Kind::WIRE_NAMES);
    }

    #[test]
    fn show_secrets_warning_is_an_envelope_warning() {
        // Keep this policy pinned even while the current config has no secrets.
        let mut warnings = Vec::new();
        add_show_secrets_warning(true, &mut warnings);
        assert_eq!(
            warnings,
            ["--show-secrets: secret-valued config keys are shown in plaintext"]
        );
    }

    #[test]
    fn secret_redaction_scrubs_values_errors_and_warnings() {
        let mut key = ConfigKey::from_layers(
            "token",
            vec![ConfigLayer::invalid_file(
                "hunter2".into(),
                "token".into(),
                "invalid token 'hunter2'".into(),
            )],
        );
        key.secret = true;
        redact_secret_key(&mut key);
        let mut warnings = Vec::new();
        append_validation_warnings(&[key], &[], &mut warnings);
        let rendered = warnings.join(" ");
        assert!(!rendered.contains("hunter2"));
        assert!(rendered.contains(REDACTED));
    }

    #[test]
    fn harness_validation_normalizes_like_execution() {
        assert_eq!(validate_harness_name(" pi "), Ok("pi"));
        assert_eq!(harness_validation_error(" pi "), None);
    }
}
