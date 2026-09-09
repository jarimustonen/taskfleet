//! Integration tests for tolerant, layered `config` inspection (§8).

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn bin(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    command.env("TASKFLEET_HOME", home);
    command.env("HOME", home);
    command.env_remove("TASKFLEET_HARNESS");
    command.env_remove("TASKFLEET_LOG");
    command
}

fn envelope_json(command: &mut Command) -> Value {
    let output = command.output().expect("spawn");
    assert!(output.status.success(), "exit: {:?}", output.status);
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let value: Value = serde_json::from_str(stdout.trim()).expect("valid JSON envelope");
    assert_eq!(value["schema_version"], 1, "envelope schema: {value}");
    value
}

fn show_json(command: &mut Command) -> Value {
    let envelope = envelope_json(command);
    assert_eq!(envelope["warnings"], serde_json::json!([]));
    envelope["data"].clone()
}

fn key<'a>(data: &'a Value, name: &str) -> &'a Value {
    data["keys"]
        .as_array()
        .expect("keys array")
        .iter()
        .find(|row| row["key"] == name)
        .unwrap_or_else(|| panic!("key {name} present in {data}"))
}

#[test]
fn config_path_text_prints_bare_path() {
    let home = TempDir::new().unwrap();
    let output = bin(home.path())
        .args(["--output", "text", "config", "path"])
        .output()
        .expect("spawn");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        home.path().join("config.toml").display().to_string()
    );
}

#[test]
fn config_path_json_pins_payload_shape() {
    let home = TempDir::new().unwrap();
    let data = show_json(bin(home.path()).args(["config", "path", "--output", "json"]));
    let fields: BTreeSet<&str> = data
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        BTreeSet::from(["schema_version_config", "path", "exists"])
    );
    assert_eq!(data["schema_version_config"], 2);
    assert_eq!(data["exists"], false);
    assert_eq!(
        data["path"],
        home.path().join("config.toml").display().to_string()
    );
}

#[test]
fn config_path_reports_exists_true_when_present() {
    let home = TempDir::new().unwrap();
    std::fs::write(home.path().join("config.toml"), "[harness]\n").unwrap();
    let data = show_json(bin(home.path()).args(["config", "path", "--output", "json"]));
    assert_eq!(data["exists"], true);
}

#[test]
fn config_show_json_pins_payload_and_default_layers() {
    let home = TempDir::new().unwrap();
    let data = show_json(bin(home.path()).args(["config", "show", "--output", "json"]));

    let fields: BTreeSet<&str> = data
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        BTreeSet::from([
            "schema_version_config",
            "path",
            "exists",
            "valid",
            "invalid_layer_count",
            "keys",
            "unrecognized",
        ])
    );
    assert_eq!(data["schema_version_config"], 2);
    assert_eq!(data["exists"], false);
    assert_eq!(data["valid"], true);
    assert_eq!(data["invalid_layer_count"], 0);
    assert_eq!(data["unrecognized"], serde_json::json!([]));

    let names: Vec<&str> = data["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "harness.default",
            "harness.spinoff",
            "harness.research",
            "harness.technical-decision",
            "harness.fan-out",
            "tmux.default_session",
            "tmux.persistent",
            "tmux.completed_window_ttl",
            "tmux.completed_window_max",
        ]
    );

    for row in data["keys"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["key"].as_str().unwrap().starts_with("harness."))
    {
        let fields: BTreeSet<&str> = row
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            fields,
            BTreeSet::from([
                "key",
                "effective_value",
                "effective_source",
                "valid",
                "validation_error",
                "secret",
                "layers",
            ])
        );
        assert_eq!(row["effective_value"], "pi");
        assert_eq!(row["effective_source"], "default");
        assert_eq!(row["valid"], true);
        assert_eq!(row["validation_error"], Value::Null);
        assert_eq!(row["secret"], false);
        assert_eq!(row["layers"].as_array().unwrap().len(), 1);
    }
}

#[test]
fn config_show_file_layers_include_override_and_inherited_default() {
    let home = TempDir::new().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        "[harness]\ndefault = \"pi\"\n\n[harness.per_kind]\nresearch = \"claude\"\n",
    )
    .unwrap();
    let data = show_json(bin(home.path()).args(["config", "show", "--output", "json"]));

    assert_eq!(key(&data, "harness.default")["effective_source"], "file");
    let research = key(&data, "harness.research");
    assert_eq!(research["effective_value"], "claude");
    assert_eq!(
        research["layers"][0]["origin_key"],
        "harness.per_kind.research"
    );
    assert_eq!(research["layers"][1]["origin_key"], "harness.default");
    assert_eq!(research["layers"][2]["source"], "default");
    assert_eq!(key(&data, "harness.spinoff")["effective_value"], "pi");
}

#[test]
fn config_show_env_override_keeps_file_layers_visible() {
    let home = TempDir::new().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        "[harness]\ndefault = \"claude\"\n\n[harness.per_kind]\nresearch = \"claude\"\n",
    )
    .unwrap();
    let mut command = bin(home.path());
    command.env("TASKFLEET_HARNESS", "pi");
    let data = show_json(command.args(["config", "show", "--output", "json"]));

    for row in data["keys"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["key"].as_str().unwrap().starts_with("harness."))
    {
        assert_eq!(row["effective_value"], "pi", "row: {row}");
        assert_eq!(row["effective_source"], "env", "row: {row}");
    }
    let research = key(&data, "harness.research");
    assert_eq!(research["layers"][0]["source"], "env");
    assert_eq!(research["layers"][1]["value"], "claude");
    assert_eq!(
        research["layers"][1]["origin_key"],
        "harness.per_kind.research"
    );
    assert_eq!(research["layers"][1]["active"], false);
}

#[test]
fn config_show_invalid_file_value_is_visible_and_successful() {
    let home = TempDir::new().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        "[harness]\ndefault = \"gpt\"\n",
    )
    .unwrap();
    let envelope = envelope_json(bin(home.path()).args(["config", "show", "--output", "json"]));
    let row = key(&envelope["data"], "harness.default");
    assert_eq!(row["effective_value"], "gpt");
    assert_eq!(row["valid"], false);
    assert!(row["validation_error"]
        .as_str()
        .unwrap()
        .contains("unknown harness 'gpt'"));
    assert_eq!(row["layers"][0]["value"], "gpt");
    assert_eq!(row["layers"][0]["valid"], false);
    assert_eq!(envelope["data"]["valid"], false);
    assert_eq!(envelope["data"]["invalid_layer_count"], 1);
    assert_eq!(envelope["warnings"].as_array().unwrap().len(), 1);
}

#[test]
fn config_show_env_does_not_launder_invalid_file_value() {
    let home = TempDir::new().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        "[harness.per_kind]\nresearch = \"gpt\"\n",
    )
    .unwrap();
    let mut command = bin(home.path());
    command.env("TASKFLEET_HARNESS", "pi");
    let envelope = envelope_json(command.args(["config", "show", "--output", "json"]));
    let row = key(&envelope["data"], "harness.research");
    assert_eq!(row["effective_value"], "pi");
    assert_eq!(row["valid"], true);
    assert_eq!(row["layers"][1]["value"], "gpt");
    assert_eq!(row["layers"][1]["valid"], false);
    assert_eq!(envelope["data"]["valid"], false);
    assert_eq!(envelope["data"]["invalid_layer_count"], 1);
    assert_eq!(envelope["warnings"].as_array().unwrap().len(), 1);
}

#[test]
fn config_show_invalid_env_is_reported_once() {
    let home = TempDir::new().unwrap();
    let mut command = bin(home.path());
    command.env("TASKFLEET_HARNESS", "gpt");
    let envelope = envelope_json(command.args(["config", "show", "--output", "json"]));
    assert_eq!(envelope["data"]["invalid_layer_count"], 1);
    let warnings = envelope["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].as_str().unwrap().contains("TASKFLEET_HARNESS"));
}

#[test]
fn config_show_whitespace_value_matches_execution_normalization() {
    let home = TempDir::new().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        "[harness]\ndefault = \" pi \"\n",
    )
    .unwrap();
    let data = show_json(bin(home.path()).args(["config", "show", "--output", "json"]));
    let row = key(&data, "harness.default");
    assert_eq!(row["effective_value"], "pi");
    assert_eq!(row["layers"][0]["value"], " pi ");
    assert_eq!(row["valid"], true);
}

#[test]
fn config_show_unrecognized_entries_are_separate_and_keys_stay_unique() {
    let home = TempDir::new().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        "[harness]\nresearch = \"pi\"\n\n[harness.per_kind]\ndefault = \"pi\"\n",
    )
    .unwrap();
    let envelope = envelope_json(bin(home.path()).args(["config", "show", "--output", "json"]));
    let data = &envelope["data"];
    let names: Vec<&str> = data["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["key"].as_str().unwrap())
        .collect();
    let unique: BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(names.len(), unique.len());
    assert_eq!(data["unrecognized"].as_array().unwrap().len(), 2);
    assert_eq!(data["valid"], false);
    assert_eq!(data["invalid_layer_count"], 2);
}

#[test]
fn config_show_parseable_wrong_type_is_tolerated() {
    let home = TempDir::new().unwrap();
    std::fs::write(home.path().join("config.toml"), "[harness]\ndefault = 42\n").unwrap();
    let envelope = envelope_json(bin(home.path()).args(["config", "show", "--output", "json"]));
    let row = key(&envelope["data"], "harness.default");
    assert_eq!(row["effective_value"], "42");
    assert_eq!(row["valid"], false);
    assert!(row["validation_error"]
        .as_str()
        .unwrap()
        .contains("expected a string"));
}

#[test]
fn config_show_unparseable_toml_remains_a_hard_error() {
    let home = TempDir::new().unwrap();
    std::fs::write(home.path().join("config.toml"), "[harness\n").unwrap();
    let output = bin(home.path())
        .args(["config", "show", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let value: Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
    assert_eq!(value["error"]["code"], "invalid_config");
}

#[test]
fn config_show_text_is_human_readable_and_layered() {
    let home = TempDir::new().unwrap();
    let output = bin(home.path())
        .args(["--output", "text", "config", "show"])
        .output()
        .expect("spawn");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("path:"), "text output: {stdout:?}");
    assert!(stdout.contains("exists: false"), "text output: {stdout:?}");
    assert!(stdout.contains("valid:  true"), "text output: {stdout:?}");
    assert!(stdout.contains("harness.default = pi (default, valid)"));
    assert!(stdout.contains("* default pi"), "text layers: {stdout:?}");
}

#[test]
fn config_show_secrets_flag_is_accepted_noop() {
    let home = TempDir::new().unwrap();
    let envelope = envelope_json(bin(home.path()).args([
        "config",
        "show",
        "--show-secrets",
        "--output",
        "json",
    ]));
    assert_eq!(envelope["warnings"], serde_json::json!([]));
}
