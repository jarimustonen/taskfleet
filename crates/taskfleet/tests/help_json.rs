//! Structured-help (`--help --output json|jsonl`) contract suite
//! (AGENTS-AI-FIRST-CLI §14).
//!
//! `--help --output json` projects the clap command surface onto a
//! schema-versioned payload that trained agents read. A renamed/removed
//! field, a re-sorted list, or a dropped `schema_version_help` is a
//! breaking change to that surface — these snapshots catch it the moment
//! it happens.
//!
//! Three drill-down levels are locked, per the issue:
//!   1. top-level `--help` (the whole tree)
//!   2. a leaf verb (`run create`)
//!   3. a noun-only node (`run`, listing its verbs)
//!
//! ## Determinism
//!
//! The only non-deterministic field is the root `version` (the crate
//! version, which moves every release); it is redacted to `[VERSION]`.
//! The help surface carries no ids, timestamps, or commit hashes.
//!
//! ## Updating snapshots
//!
//! `cargo insta test --review`, or `INSTA_UPDATE=always cargo test
//! -p taskfleet --test help_json` then inspect the diff.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;

fn bin() -> Command {
    let mut c = Command::cargo_bin("taskfleet").expect("binary builds");
    // Hermetic: help rendering never touches state, but clear inherited
    // color/log knobs so a developer's shell can't perturb stdout.
    c.env_remove("TASKFLEET_LOG");
    c.env_remove("NO_COLOR");
    c.env_remove("CLICOLOR");
    c
}

/// Run a structured-help command expected to succeed (exit 0, clean
/// stderr) and return its stdout. Also asserts the standard success
/// envelope structurally, so a blanket `cargo insta accept` can't bless a
/// dropped `data`/`schema_version`.
#[cfg(unix)]
#[test]
fn structured_help_rejects_non_utf8_output_path_without_lossy_routing() {
    use std::os::unix::ffi::OsStringExt as _;
    let mut path = b"/tmp/taskfleet-help-".to_vec();
    path.push(0xff);
    path.extend_from_slice(b".json");
    bin()
        .arg("--help")
        .arg("--output")
        .arg(std::ffi::OsString::from_vec(path))
        .assert()
        .code(1)
        .stderr(predicate::str::contains("invalid_arguments"));
}

fn help_stdout(args: &[&str]) -> String {
    let out = bin()
        .args(args)
        .assert()
        .success()
        .stderr(predicate::str::is_empty())
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).expect("stdout is utf8");
    let v: Value = serde_json::from_str(s.trim()).expect("help stdout is a JSON envelope");
    assert_eq!(v["schema_version"], 1, "envelope schema: {v}");
    assert_eq!(v["data"]["schema_version_help"], 3, "help schema: {v}");
    assert!(v.get("error").is_none(), "help envelope carries error: {v}");
    s
}

/// Bind the version redaction and snapshot the raw rendered stdout.
fn snapshot(name: &str, value: &str) {
    let mut settings = insta::Settings::clone_current();
    // Root `version` field — the crate version moves every release.
    settings.add_filter(r#""version": "[^"]*""#, r#""version": "[VERSION]""#);
    settings.bind(|| insta::assert_snapshot!(name, value));
}

#[test]
fn top_level_help_json_locks_the_whole_surface() {
    // The entire command tree: every noun, verb, flag, and positional.
    // This is the §14 "schema as API surface" promise made concrete.
    // Pinned with `--depth tree` so the v3 default (Bounded(1), which
    // summarises immediate children) does not collapse the surface lock —
    // we still want a snapshot that fails on any flag/positional drift
    // anywhere in the tree (issue: help-json-depth-control).
    // `--json` is the global shorthand, so the root surface must not need
    // the longer `--output json` spelling.
    let stdout = help_stdout(&["--help", "--json", "--depth", "tree"]);
    snapshot("top_level_help_json", &stdout);
}

#[test]
fn top_level_help_json_default_depth_summarizes_subcommands() {
    // Default depth = 1: root is full, immediate children are summaries
    // (`has_subcommands`, no `flags`/`positionals`/`subcommands`). This is
    // the agent-friendly default — a 2100-line firehose was the bug the
    // depth control fixes.
    let stdout = help_stdout(&["--help", "--output", "json"]);
    let v: Value = serde_json::from_str(stdout.trim()).expect("json");
    let subs = v["data"]["subcommands"]
        .as_array()
        .expect("subcommands array");
    assert!(!subs.is_empty(), "root should list its subcommands");
    for s in subs {
        assert!(
            s.get("flags").is_none(),
            "depth=1 child must be a summary (no `flags`): {s}"
        );
        assert!(
            s.get("has_subcommands").is_some(),
            "summary carries `has_subcommands`: {s}"
        );
    }
}

#[test]
fn depth_two_expands_immediate_children_fully() {
    // `--depth 2`: immediate children are full CommandNodes (so they expose
    // `flags` etc.); grandchildren collapse to summaries.
    let stdout = help_stdout(&["--help", "--output", "json", "--depth", "2"]);
    let v: Value = serde_json::from_str(stdout.trim()).expect("json");
    let run = v["data"]["subcommands"]
        .as_array()
        .expect("subcommands array")
        .iter()
        .find(|s| s["command"] == "taskfleet run")
        .expect("run noun");
    assert!(run.get("flags").is_some(), "depth=2 child is Full: {run}");
    let verbs = run["subcommands"].as_array().expect("run has subcommands");
    for v in verbs {
        assert!(
            v.get("flags").is_none(),
            "depth=2 grandchild is Summary: {v}"
        );
    }
}

#[test]
fn invalid_depth_value_errors_with_invalid_arguments() {
    // §14: bad input is a structured error, not a silent default.
    let assert = bin()
        .args(["--help", "--output", "json", "--depth", "garbage"])
        .assert()
        .failure();
    let out = assert.get_output();
    assert!(out.stdout.is_empty(), "no help on stdout when input is bad");
    let stderr = String::from_utf8(out.stderr.clone()).expect("utf8");
    let v: Value = serde_json::from_str(stderr.trim()).expect("error envelope");
    assert_eq!(v["error"]["code"], "invalid_arguments", "envelope: {v}");
    assert_eq!(v["error"]["invalid_value"], "garbage", "envelope: {v}");
}

#[test]
fn leaf_verb_help_json() {
    // A leaf verb resolves to its own full flag/positional list,
    // independent of its parent noun (§14 drill-down).
    // Drill-down accepts the same global shorthand in either argument order.
    let stdout = help_stdout(&["run", "create", "--json", "--help"]);
    snapshot("run_create_help_json", &stdout);
}

#[test]
fn cancel_help_distinguishes_nodes_from_fan_out_child_runs() {
    let stdout = help_stdout(&["run", "cancel", "--help", "--json"]);
    let v: Value = serde_json::from_str(stdout.trim()).expect("json");
    let about = v["data"]["about"].as_str().expect("cancel description");
    let node = v["data"]["flags"]
        .as_array()
        .expect("flags")
        .iter()
        .find(|f| f["name"] == "node")
        .and_then(|f| f["help"].as_str())
        .expect("node help");
    for help in [about, node] {
        assert!(help.contains("separate runs"), "fan-out topology: {help}");
        assert!(
            help.contains("run id") || help.contains("<child-run-id>"),
            "cancel child by run id: {help}"
        );
        assert!(
            !help.contains("stuck fan-out child"),
            "retired model: {help}"
        );
    }
    assert!(node.contains("branch and worktree are preserved"));
    assert!(node.contains("run stays live while other nodes remain"));
    assert!(node.contains("run cancel <child-run-id>"));

    let text = bin()
        .args(["run", "cancel", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(text).expect("text help is utf8");
    assert!(
        text.contains("run cancel <child-run-id>"),
        "text help: {text}"
    );
    assert!(!text.contains("stuck fan-out child"), "text help: {text}");
}

#[test]
fn noun_only_help_json_lists_verbs() {
    // A noun-only node lists its verbs as nested subcommands.
    let stdout = help_stdout(&["run", "--help", "--output", "json"]);
    snapshot("run_help_json", &stdout);
}

#[test]
fn config_noun_help_json_lists_verbs() {
    // The `config` noun (config path / config show) — pins its verb surface
    // and per-verb flags (`--show-secrets`) as a §14 drill-down.
    let stdout = help_stdout(&["config", "--help", "--output", "json", "--depth", "tree"]);
    snapshot("config_help_json", &stdout);
}

// ----------------------------------------------------------------------
// Behavioural guards (not snapshots) for the success-criteria contract.
// ----------------------------------------------------------------------

#[test]
fn flags_are_sorted_by_name() {
    // Stability contract (v2): flags are sorted by `name` (the clap id),
    // which is always present even for short-only flags.
    let stdout = help_stdout(&["run", "create", "--help", "--output", "json"]);
    let v: Value = serde_json::from_str(stdout.trim()).expect("json");
    let names: Vec<String> = v["data"]["flags"]
        .as_array()
        .expect("flags array")
        .iter()
        .map(|f| f["name"].as_str().expect("name is string").to_string())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "flags must be sorted by name");
    assert!(!names.is_empty(), "leaf verb should expose flags");
}

#[test]
fn output_flag_reports_custom_accepted_values_and_arity() {
    // §14/§13 v2: the custom-parsed global `--output` enumerates its
    // accepted tokens (clap can't infer them) and is marked file-accepting.
    let stdout = help_stdout(&["run", "create", "--help", "--output", "json"]);
    let v: Value = serde_json::from_str(stdout.trim()).expect("json");
    let output = v["data"]["flags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "output")
        .expect("output flag present");
    assert_eq!(
        output["accepted_values"],
        serde_json::json!(["json", "jsonl", "text"]),
        "custom-parser accepted values: {output}"
    );
    assert_eq!(output["accepts_file_paths"], true);
    assert_eq!(output["is_global"], true);
    assert_eq!(output["defaults"], serde_json::json!(["jsonl"]));
    assert_eq!(output["arity"]["min"], 1);
    assert_eq!(output["arity"]["max"], 1);
}

#[test]
fn conflicting_flags_expose_the_edge() {
    // `--task` conflicts with `--prompt-file` (run create); v2 surfaces the
    // clap `conflicts_with` edge as a list of arg ids.
    let stdout = help_stdout(&["run", "create", "--help", "--output", "json"]);
    let v: Value = serde_json::from_str(stdout.trim()).expect("json");
    let task = v["data"]["flags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "task")
        .expect("task flag present");
    assert_eq!(
        task["conflicts_with"],
        serde_json::json!(["prompt_file"]),
        "conflicts_with edge: {task}"
    );
}

#[test]
fn requiring_flags_expose_the_edge() {
    // `--parent-run-id` and `--parent-node-id` are mutually required (run
    // create child-spawn); v2 surfaces the clap `requires` edge as a list of
    // arg ids. Recovered from the real tree, this also guards the
    // Debug-projection in `help::requires` against a clap format change.
    let stdout = help_stdout(&["run", "create", "--help", "--output", "json"]);
    let v: Value = serde_json::from_str(stdout.trim()).expect("json");
    let flag = |name: &str| {
        v["data"]["flags"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name)
            .unwrap_or_else(|| panic!("{name} flag present"))
            .clone()
    };
    assert_eq!(
        flag("parent_run_id")["requires"],
        serde_json::json!(["parent_node_id"]),
        "requires edge: {}",
        flag("parent_run_id")
    );
    assert_eq!(
        flag("parent_node_id")["requires"],
        serde_json::json!(["parent_run_id"]),
        "requires edge (reverse): {}",
        flag("parent_node_id")
    );
    // A flag with no requirement carries the additive empty default.
    assert_eq!(
        flag("title")["requires"],
        serde_json::json!([]),
        "non-requiring flag defaults to []: {}",
        flag("title")
    );
}

#[test]
fn jsonl_is_a_single_line() {
    // `--output jsonl` emits the same payload as one compact line.
    let stdout = help_stdout(&["--help", "--output", "jsonl"]);
    assert_eq!(
        stdout.lines().count(),
        1,
        "jsonl help must be a single line: {stdout:?}"
    );
}

#[test]
fn bare_help_is_unchanged_text() {
    // No explicit `--output`: clap's text help is preserved (§14
    // out-of-scope). Text help is not a JSON envelope.
    let out = bin()
        .args(["run", "create", "--help"])
        .output()
        .expect("spawn");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    assert!(
        stdout.starts_with("Create a new run"),
        "bare --help should render clap text: {stdout:?}"
    );
    assert!(
        serde_json::from_str::<Value>(stdout.trim()).is_err(),
        "bare --help must not be JSON"
    );
}

#[test]
fn output_text_with_help_stays_text() {
    // Explicit `--output text` + `--help` also keeps clap's text help.
    let out = bin()
        .args(["run", "create", "--help", "--output", "text"])
        .output()
        .expect("spawn");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    assert!(
        serde_json::from_str::<Value>(stdout.trim()).is_err(),
        "--output text --help must not be JSON: {stdout:?}"
    );
}

#[test]
fn output_equals_form_triggers_json_help() {
    // `--output=json` (equals form) must trigger structured help too.
    let stdout = help_stdout(&["run", "create", "--help", "--output=json"]);
    let v: Value = serde_json::from_str(stdout.trim()).expect("json");
    assert_eq!(v["data"]["command"], "taskfleet run create");
}

#[test]
fn json_and_output_are_conflicting_selectors_under_help_too() {
    for args in [
        ["--help", "--json", "--output", "json"],
        ["--output", "text", "--json", "--help"],
    ] {
        let assert = bin().args(args).assert().failure();
        let out = assert.get_output();
        assert!(out.stdout.is_empty(), "no help on stdout for a conflict");
        let stderr = String::from_utf8(out.stderr.clone()).expect("utf8");
        let v: Value = serde_json::from_str(stderr.trim()).expect("error envelope");
        assert_eq!(
            v["error"]["code"], "conflicting_output_flags",
            "envelope: {v}"
        );
    }
}

#[test]
fn invalid_output_is_not_hidden_by_json_help() {
    for args in [
        &["--help", "--json", "--output", "not-a-format"][..],
        &["--help", "--json", "--output"][..],
    ] {
        let assert = bin().args(args).assert().failure();
        let out = assert.get_output();
        assert!(out.stdout.is_empty(), "no help on stdout for bad output");
        let stderr = String::from_utf8(out.stderr.clone()).expect("utf8");
        let v: Value = serde_json::from_str(stderr.trim()).expect("error envelope");
        assert_eq!(v["error"]["code"], "invalid_arguments", "envelope: {v}");
    }
}

#[test]
fn double_dash_suppresses_json_help_detection() {
    // After `--`, a trailing `--help`/`--output` is positional data, not a
    // help request. Detection must not fire; clap handles the args (here
    // `run` requires a subcommand, so it errors — the point is that we did
    // NOT emit a JSON help envelope on stdout).
    let out = bin()
        .args(["run", "--", "--help", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(
        out.stdout.is_empty(),
        "`--` must suppress JSON help on stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn subcommands_are_sorted_by_name() {
    // Stability contract: nested subcommands sorted (by command path).
    let stdout = help_stdout(&["run", "--help", "--output", "json"]);
    let v: Value = serde_json::from_str(stdout.trim()).expect("json");
    let cmds: Vec<String> = v["data"]["subcommands"]
        .as_array()
        .expect("subcommands array")
        .iter()
        .map(|s| s["command"].as_str().expect("command string").to_string())
        .collect();
    let mut sorted = cmds.clone();
    sorted.sort();
    assert_eq!(cmds, sorted, "subcommands must be sorted");
    assert!(cmds.len() >= 4, "run should list its verbs: {cmds:?}");
}

#[test]
fn unknown_subcommand_with_json_help_errors() {
    // §14 tightening (help-json-clap-native-resolution): an unknown
    // subcommand under structured help is an error envelope (exit 1), not a
    // silent fall-back to root help. The clap lenient parse drops the
    // trailing `--help`/`--output` after the bad leading token, so this
    // falls through to clap's normal dispatch, which rejects the unknown
    // subcommand — either way the caller sees a structured error, no JSON
    // help on stdout.
    let assert = bin()
        .args(["bogus-subcommand", "--help", "--output", "json"])
        .assert()
        .failure();
    let out = assert.get_output();
    assert!(
        out.stdout.is_empty(),
        "no JSON help on stdout for an unknown subcommand: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8(out.stderr.clone()).expect("utf8");
    let v: Value = serde_json::from_str(stderr.trim()).expect("stderr is an error envelope");
    assert!(v.get("error").is_some(), "error envelope on stderr: {v}");
}

#[test]
fn unknown_subcommand_after_flags_errors() {
    // Flag-first ordering: `--help --output json bogus`. Here the clap
    // lenient parse *does* recover help + output and surfaces `bogus` as an
    // external subcommand the real tree rejects — exercising the resolver's
    // own UnknownSubcommand path (code `unknown_subcommand`), distinct from
    // clap's normal dispatch above.
    let assert = bin()
        .args(["--help", "--output", "json", "bogus-subcommand"])
        .assert()
        .failure();
    let out = assert.get_output();
    assert!(
        out.stdout.is_empty(),
        "no JSON help on stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8(out.stderr.clone()).expect("utf8");
    let v: Value = serde_json::from_str(stderr.trim()).expect("stderr is an error envelope");
    assert_eq!(
        v["error"]["code"], "unknown_subcommand",
        "resolver-level unknown-subcommand error: {v}"
    );
}

#[test]
fn nested_unknown_subcommand_after_a_valid_noun_errors() {
    // A stray token under a *valid* noun must not resolve to that noun's help
    // (`--output json --help run bogus` once rendered `run`). Recursive
    // external-subcommand handling makes it a structured error instead.
    let assert = bin()
        .args(["--output", "json", "--help", "run", "bogus"])
        .assert()
        .failure();
    let out = assert.get_output();
    assert!(
        out.stdout.is_empty(),
        "no JSON help on stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8(out.stderr.clone()).expect("utf8");
    let v: Value = serde_json::from_str(stderr.trim()).expect("stderr is an error envelope");
    assert_eq!(v["error"]["code"], "unknown_subcommand", "envelope: {v}");
}

#[test]
fn output_file_help_writes_json_file() {
    // A `.json` file destination routes the help envelope to that file.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("help.json");
    bin()
        .args(["--help", "--output", path.to_str().unwrap()])
        .assert()
        .success();
    let body = std::fs::read_to_string(&path).expect("help file written");
    let v: Value = serde_json::from_str(&body).expect("file is valid JSON");
    assert_eq!(v["data"]["schema_version_help"], 3);
}
