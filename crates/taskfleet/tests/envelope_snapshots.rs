//! Integration-level **envelope snapshot** suite.
//!
//! The per-subcommand suites (`run.rs`, `event.rs`, …) assert *semantics*
//! field-by-field. This suite locks the *shape* of the machine-readable
//! contract with `insta` snapshots, so envelope drift (a renamed field, a
//! changed nesting, a dropped `schema_version`) is caught the moment it
//! happens — regardless of which subcommand a contributor touched.
//!
//! What is locked, per `AGENTS-AI-FIRST-CLI.md`:
//!
//! - **Success envelope** (§10, stdout): `{schema_version, data, warnings?}`
//! - **Error envelope** (§10, stderr): `{schema_version, error: {code,
//!   message, invalid_value?, expected?}}`, with exit 1 (user/validation)
//!   or 2 (refused-but-actionable / system)
//! - **Dry-run / planning envelopes** (§11): the `dry_run`/`would_be`
//!   shapes each mutating verb emits
//! - **Format coverage** (§9): each noun has at least one success snapshot
//!   in `text`, `json` (pretty single document) and `jsonl` (compact
//!   one-line) so a change to *any* renderer is visible. (Not every
//!   subcommand × format combination is snapshotted — see
//!   `tests/snapshots/README.md` for the exact matrix.)
//!
//! As a backstop against a blanket `cargo insta accept` silently blessing
//! a `schema_version` bump or a dropped `data`/`error` key, [`ok_stdout`]
//! and [`err_stderr`] also assert the envelope invariants *structurally*,
//! independent of the snapshot.
//!
//! ## Determinism
//!
//! Non-deterministic values are redacted via `insta` filters (regex
//! substitution on the rendered output) before the snapshot is compared.
//! Per-test dynamic filters run first, global filters last — see
//! [`snapshot`]:
//!
//! - `run_id` (a ULID) — per-test dynamic filter, also rewrites the copy
//!   embedded in `dir` paths and error messages
//! - the temp `$TASKFLEET_HOME` path — per-test dynamic filter
//! - any other ULID-shaped token, `commit` (git HEAD hash), timestamps
//!   (`created_at`/`ts`/… in all three serialised forms) and live PIDs —
//!   global, boundary-/case-anchored filters
//!
//! ## Updating snapshots
//!
//! See `tests/snapshots/README.md`. In short: `cargo insta test
//! --review`, or `INSTA_UPDATE=always cargo test -p taskfleet --release
//! --test envelope_snapshots` then inspect the diff.

use std::time::Duration;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};
use tempfile::TempDir;

/// The envelope `schema_version` every payload must carry (§10). Asserted
/// structurally (below) so a blanket `cargo insta accept` can't silently
/// bless a bump. Mirrors `taskfleet_core::SCHEMA_VERSION`.
const ENVELOPE_SCHEMA: u64 = 1;

// ----------------------------------------------------------------------
// Harness
// ----------------------------------------------------------------------

/// A binary handle pointed at an isolated `$TASKFLEET_HOME`. The
/// `TASKFLEET_TEST_SKIP_MATERIALIZE` escape hatch keeps `run create` from
/// shelling out to `create.sh` / spawning a supervisor (same pattern the
/// sibling suites use), so output is deterministic skeleton-only state.
fn bin(home: &TempDir) -> Command {
    let mut c = Command::cargo_bin("taskfleet").expect("binary builds");
    c.env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path());
    c.env("TASKFLEET_TEST_SKIP_MATERIALIZE", "1");
    // Hermetic env: the CLI logs to a file and forces `color=never`, but
    // clear any inherited log/color knobs so a developer's shell can't
    // perturb the captured stdout/stderr.
    c.env_remove("TASKFLEET_LOG");
    c.env_remove("NO_COLOR");
    c.env_remove("CLICOLOR");
    // Creator attribution is ambient too: a run created from inside a Pi agent
    // records `created_by_session` from this variable, which would leak the
    // caller's session into `run show` / `event tail` snapshots.
    c.env_remove("TASKFLEET_CREATOR_SESSION");
    // A snapshot test must fail, never wedge CI — guard the streaming
    // verb (`event tail`) against a future default-follow regression.
    c.timeout(Duration::from_secs(30));
    c
}

/// Structurally assert the success-envelope invariants (§10) on any
/// single-document JSON/JSONL stdout — independent of the snapshot, so a
/// dropped `schema_version`/`data` or a bumped schema can't be rubber-
/// stamped through `cargo insta accept`. Text and multi-line streams
/// (e.g. `event tail`) don't parse as one document and are skipped.
fn assert_success_envelope(stdout: &str) {
    if let Ok(v) = serde_json::from_str::<Value>(stdout) {
        if v.is_object() {
            assert_eq!(v["schema_version"], json!(ENVELOPE_SCHEMA), "envelope: {v}");
            assert!(
                v.get("data").is_some(),
                "success envelope missing data: {v}"
            );
            assert!(
                v.get("error").is_none(),
                "success envelope carries error: {v}"
            );
        }
    }
}

/// Error-envelope twin of [`assert_success_envelope`].
fn assert_error_envelope(stderr: &str) {
    if let Ok(v) = serde_json::from_str::<Value>(stderr) {
        if v.is_object() {
            assert_eq!(v["schema_version"], json!(ENVELOPE_SCHEMA), "envelope: {v}");
            let err = v.get("error").expect("error envelope missing error");
            assert!(
                err.get("code").and_then(Value::as_str).is_some(),
                "err.code"
            );
            assert!(
                err.get("message").and_then(Value::as_str).is_some(),
                "err.message"
            );
            assert!(v.get("data").is_none(), "error envelope carries data: {v}");
        }
    }
}

/// Run a command expected to **succeed** (exit 0) and return its stdout.
/// stderr is asserted empty: §2 keeps the data channel and the
/// diagnostic channel separate.
fn ok_stdout(cmd: &mut Command) -> String {
    let out = cmd
        .assert()
        .success()
        .stderr(predicate::str::is_empty())
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).expect("stdout is utf8");
    assert_success_envelope(&s);
    s
}

/// Run a command expected to **fail** with `code`, returning the trimmed
/// stderr error envelope. stdout is asserted empty (§2): a failure must
/// not also dribble a partial payload onto the data channel.
fn err_stderr(cmd: &mut Command, code: i32) -> String {
    let out = cmd
        .assert()
        .failure()
        .code(code)
        .stdout(predicate::str::is_empty())
        .get_output()
        .stderr
        .clone();
    let s = String::from_utf8(out)
        .expect("stderr is utf8")
        .trim_end()
        .to_string();
    assert_error_envelope(&s);
    s
}

/// Parse a success envelope's stdout into JSON (for extracting ids the
/// snapshot can't, since they're redacted).
fn data_value(stdout: &str) -> Value {
    serde_json::from_str(stdout).expect("stdout is a JSON envelope")
}

/// Per-test dynamic redactions: the temp home path, and (optionally) the
/// run-id ULID. Both are regex-escaped — the home path contains `.` and
/// (on some platforms) other metacharacters.
fn redactions(home: &TempDir, run_id: Option<&str>) -> Vec<(String, &'static str)> {
    let mut v = vec![(regex::escape(&home.path().to_string_lossy()), "[HOME]")];
    if let Some(r) = run_id {
        v.push((regex::escape(r), "[RUN_ID]"));
    }
    v
}

/// Collect every distinct ULID-shaped token (Crockford base32, 26 chars,
/// case-insensitive, boundary-anchored) in `rendered`. Used by the `run
/// create` cases, where the id is minted by the call itself and so can't
/// be passed in advance. Collecting *all* of them — not just the first —
/// means a future second id in the same envelope can't leak.
fn find_ulids(rendered: &str) -> Vec<String> {
    let re = regex::Regex::new(r"(?i)\b[0-9a-hjkmnp-tv-z]{26}\b").expect("valid ulid regex");
    let mut out = Vec::new();
    for m in re.find_iter(rendered) {
        let s = m.as_str().to_string();
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// Like [`redactions`] but for `run create` output: pulls the minted
/// run-id(s) straight out of the rendered text so both the `run_id` field
/// and the copy embedded in `dir` collapse to `[RUN_ID]`.
fn minted_redactions(home: &TempDir, rendered: &str) -> Vec<(String, &'static str)> {
    let mut v = redactions(home, None);
    for u in find_ulids(rendered) {
        v.push((regex::escape(&u), "[RUN_ID]"));
    }
    v
}

/// Bind the per-test + global filters and assert the snapshot. `value` is
/// the raw rendered output (stdout for success, stderr for errors).
///
/// Order matters: the **dynamic** filters (home path, known run-id) run
/// FIRST so a temp path or id can't be partially clobbered by a global
/// filter; the **global** filters run last as a safety net for anything
/// non-deterministic still standing. All global patterns are boundary-
/// and case-anchored so they can't bite a deterministic mid-token match.
fn snapshot(name: &str, value: &str, dynamic: &[(String, &'static str)]) {
    let mut settings = insta::Settings::clone_current();
    for (pattern, replacement) in dynamic {
        settings.add_filter(pattern.as_str(), *replacement);
    }
    // Any ULID still standing (e.g. a future `event_id`) — collapse so it
    // can't leak and flake. Runs after the known run-id filters above.
    settings.add_filter(r"(?i)\b[0-9a-hjkmnp-tv-z]{26}\b", "[ULID]");
    // git HEAD hash (`version.commit`).
    settings.add_filter(r"\b[0-9a-f]{40}\b", "[COMMIT]");
    // Timestamps in every serialised form we emit:
    //   RFC3339 `…Z`              (manifest/json)
    //   RFC3339 `…+00:00`         (event tail jsonl/text)
    //   `YYYY-MM-DD HH:MM:SS… UTC` (chrono Display in text mode)
    settings.add_filter(
        r"\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2}| UTC)",
        "[TS]",
    );
    // Live PID inside the `supervisor_already_running` refusal message.
    settings.add_filter(r"(?i)pid \d+", "pid [PID]");
    settings.bind(|| insta::assert_snapshot!(name, value));
}

// ----------------------------------------------------------------------
// Seed helpers — build deterministic on-disk state via the sanctioned
// `run create` + `event create` write paths (no direct file pokes).
// ----------------------------------------------------------------------

fn create_run(home: &TempDir) -> String {
    let stdout = ok_stdout(bin(home).args([
        "--output", "json", "run", "create", "--kind", "spinoff", "--title", "seed-run",
    ]));
    data_value(&stdout)["data"]["run_id"]
        .as_str()
        .expect("run_id present")
        .to_string()
}

fn write_json(home: &TempDir, name: &str, v: Value) -> String {
    let p = home.path().join(name);
    std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    p.to_str().unwrap().to_string()
}

fn event_create(home: &TempDir, run_id: &str, kind: &str, node_id: Option<&str>, data: Value) {
    let file = write_json(home, &format!("ev-{kind}.json"), data);
    let mut args = vec![
        "--output".into(),
        "json".into(),
        "event".into(),
        "create".into(),
        run_id.into(),
        "--kind".into(),
        kind.into(),
        "--from-file".into(),
        file,
    ];
    if let Some(n) = node_id {
        args.push("--node-id".into());
        args.push(n.into());
    }
    ok_stdout(bin(home).args(&args));
}

/// Seed a single node (`n-0001`) with fixed, snapshot-stable fields.
fn seed_node(home: &TempDir, run_id: &str) {
    event_create(
        home,
        run_id,
        "node.created",
        Some("n-0001"),
        json!({
            "kind": "spinoff",
            "branch": "wt/seed",
            "worktree_path": "/tmp/seed-wt",
            "tmux_window": "seed-win",
            "agent_pid": 4242
        }),
    );
}

// ----------------------------------------------------------------------
// version
// ----------------------------------------------------------------------

#[test]
fn version_envelopes() {
    let home = TempDir::new().unwrap();
    let red = redactions(&home, None);
    for (fmt, name) in [
        ("text", "version_text"),
        ("json", "version_json"),
        ("jsonl", "version_jsonl"),
    ] {
        let out = ok_stdout(bin(&home).args(["--output", fmt, "version"]));
        snapshot(name, &out, &red);
    }
}

#[test]
fn global_arg_error_envelopes() {
    let home = TempDir::new().unwrap();
    // Unknown global flag → clap-mapped user error, exit 1.
    snapshot(
        "global_unknown_flag_error",
        &err_stderr(bin(&home).args(["--frobnicate", "version"]), 1),
        &[],
    );
    // Invalid `--output` value → invalid_arguments, exit 1.
    snapshot(
        "global_invalid_output_error",
        &err_stderr(bin(&home).args(["--output", "yaml", "version"]), 1),
        &[],
    );
}

// ----------------------------------------------------------------------
// skill
// ----------------------------------------------------------------------

#[test]
fn skill_envelopes() {
    let home = TempDir::new().unwrap();
    let red = redactions(&home, None);
    for (fmt, name) in [
        ("text", "skill_list_text"),
        ("json", "skill_list_json"),
        ("jsonl", "skill_list_jsonl"),
    ] {
        let out = ok_stdout(bin(&home).args(["--output", fmt, "skill", "list"]));
        snapshot(name, &out, &red);
    }
    // Unknown skill name → error envelope with an `expected.one_of` list.
    snapshot(
        "skill_show_unknown_error",
        &err_stderr(bin(&home).args(["skill", "show", "no-such-skill"]), 1),
        &[],
    );
}

// ----------------------------------------------------------------------
// run
// ----------------------------------------------------------------------

#[test]
fn run_create_envelopes() {
    let home = TempDir::new().unwrap();
    // Success in all three formats. Each `create` mints a fresh run-id,
    // all rewritten to [RUN_ID].
    for (fmt, name) in [
        ("text", "run_create_text"),
        ("json", "run_create_json"),
        ("jsonl", "run_create_jsonl"),
    ] {
        let out = ok_stdout(bin(&home).args([
            "--output",
            fmt,
            "run",
            "create",
            "--kind",
            "spinoff",
            "--title",
            "envelope demo",
        ]));
        let red = minted_redactions(&home, &out);
        snapshot(name, &out, &red);
    }

    // Dry-run planning envelope (§11): same payload shape, `dry_run:true`,
    // `supervisor:"not-spawned-dry-run"`.
    let out = ok_stdout(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "research",
        "--title",
        "dry",
        "--dry-run",
    ]));
    let red = minted_redactions(&home, &out);
    snapshot("run_create_dry_run_json", &out, &red);

    // Idempotent replay (§11): a repeat `--idempotency-key` returns the
    // original run (exit 0) with `idempotent_replay:true`.
    ok_stdout(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "idem",
        "--idempotency-key",
        "snap-key",
    ]));
    let out = ok_stdout(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "idem",
        "--idempotency-key",
        "snap-key",
    ]));
    let red = minted_redactions(&home, &out);
    snapshot("run_create_idempotent_replay_json", &out, &red);
}

#[test]
fn run_show_envelopes() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    // Seed a worker node so the canonical `run show` example is a healthy,
    // started run — a 0-node run with no supervisor would (correctly) render
    // `stalled: true` (a stillborn run; issue
    // `run-wait-stillborn-run-not-detected`), which is not the representative
    // shape this snapshot pins.
    seed_node(&home, &run_id);
    let red = redactions(&home, Some(&run_id));
    for (fmt, name) in [
        ("text", "run_show_text"),
        ("json", "run_show_json"),
        ("jsonl", "run_show_jsonl"),
    ] {
        let out = ok_stdout(bin(&home).args(["--output", fmt, "run", "show", &run_id]));
        snapshot(name, &out, &red);
    }
}

#[test]
fn run_error_envelopes() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);

    // Validation error (exit 1): unknown `--kind` enum value (clap).
    snapshot(
        "run_create_unknown_kind_error",
        &err_stderr(
            bin(&home).args(["run", "create", "--kind", "bogus", "--title", "x"]),
            1,
        ),
        &[],
    );

    // Not-found error (exit 1): show a well-formed run id that names no run.
    snapshot(
        "run_show_not_found_error",
        &err_stderr(
            bin(&home).args(["run", "show", "01jzabsent0000000000000000"]),
            1,
        ),
        &[],
    );

    // Refused-but-actionable (exit 2): a live supervisor PID is recorded,
    // so `run reattach` refuses rather than double-spawning. We park the
    // test's *own* PID in the pid-file — guaranteed alive for the call.
    let pid_file = home
        .path()
        .join("runs")
        .join(&run_id)
        .join("supervisor.pid");
    std::fs::write(&pid_file, std::process::id().to_string()).unwrap();
    snapshot(
        "run_reattach_supervisor_alive_error",
        &err_stderr(bin(&home).args(["run", "reattach", &run_id]), 2),
        &redactions(&home, Some(&run_id)),
    );
}

// ----------------------------------------------------------------------
// event
// ----------------------------------------------------------------------

#[test]
fn event_envelopes() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    seed_node(&home, &run_id);
    let red = redactions(&home, Some(&run_id));

    // `event create` success in json/jsonl/text. Each iteration appends a
    // distinct node, so `seq` increments deterministically (3, 4, 5) and
    // node ids are fixed — fully reproducible run to run.
    let nc = write_json(&home, "ev-extra.json", json!({"kind": "spinoff"}));
    for (i, (fmt, name)) in [
        ("json", "event_create_json"),
        ("jsonl", "event_create_jsonl"),
        ("text", "event_create_text"),
    ]
    .into_iter()
    .enumerate()
    {
        let node = format!("n-100{i}");
        let out = ok_stdout(bin(&home).args([
            "--output",
            fmt,
            "event",
            "create",
            &run_id,
            "--kind",
            "node.created",
            "--node-id",
            &node,
            "--from-file",
            &nc,
        ]));
        snapshot(name, &out, &red);
    }

    // Dry-run envelope for a write event.
    let st = write_json(&home, "ev-status.json", json!({"status": "running"}));
    let out = ok_stdout(bin(&home).args([
        "--output",
        "json",
        "event",
        "create",
        &run_id,
        "--kind",
        "node.status",
        "--node-id",
        "n-0001",
        "--from-file",
        &st,
        "--dry-run",
    ]));
    snapshot("event_create_dry_run_json", &out, &red);

    // Validation error (exit 1): pretty json forbidden for a stream.
    snapshot(
        "event_tail_unsupported_format_error",
        &err_stderr(
            bin(&home).args(["--output", "json", "event", "tail", &run_id]),
            1,
        ),
        &red,
    );

    // Validation error (exit 1): unknown event kind, with the closed set
    // echoed in `expected`.
    let e = write_json(&home, "ev-bad.json", json!({}));
    snapshot(
        "event_create_unknown_kind_error",
        &err_stderr(
            bin(&home).args([
                "event",
                "create",
                &run_id,
                "--kind",
                "bogus.kind",
                "--from-file",
                &e,
            ]),
            1,
        ),
        &red,
    );
}

/// `event tail` is the streaming verb (§12): jsonl + text only (pretty
/// json is rejected). It runs on its own freshly-seeded run so the
/// snapshot captures exactly `run.created` + `node.created` and is
/// decoupled from the `event create` cases above. Terminates with the
/// `{"event":"result"}` envelope.
#[test]
fn event_tail_envelopes() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    seed_node(&home, &run_id);
    let red = redactions(&home, Some(&run_id));

    for (fmt, name) in [("jsonl", "event_tail_jsonl"), ("text", "event_tail_text")] {
        let out = ok_stdout(bin(&home).args(["--output", fmt, "event", "tail", &run_id]));
        snapshot(name, &out, &red);
    }
}

// ----------------------------------------------------------------------
// node
// ----------------------------------------------------------------------

#[test]
fn node_envelopes() {
    let home = TempDir::new().unwrap();
    let run_id = create_run(&home);
    seed_node(&home, &run_id);
    let red = redactions(&home, Some(&run_id));

    for (fmt, name) in [
        ("text", "node_list_text"),
        ("json", "node_list_json"),
        ("jsonl", "node_list_jsonl"),
    ] {
        let out = ok_stdout(bin(&home).args(["--output", fmt, "node", "list", &run_id]));
        snapshot(name, &out, &red);
    }

    let out = ok_stdout(bin(&home).args(["--output", "json", "node", "show", &run_id, "n-0001"]));
    snapshot("node_show_json", &out, &red);
    let out = ok_stdout(bin(&home).args(["--output", "text", "node", "show", &run_id, "n-0001"]));
    snapshot("node_show_text", &out, &red);

    // Dry-run envelope for `node report`.
    let rep = write_json(
        &home,
        "report.json",
        json!({"success": true, "summary": "ok"}),
    );
    let out = ok_stdout(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        &run_id,
        "n-0001",
        "--from-file",
        &rep,
        "--dry-run",
    ]));
    snapshot("node_report_dry_run_json", &out, &red);

    // Validation error (exit 1): unknown node id.
    snapshot(
        "node_show_not_found_error",
        &err_stderr(bin(&home).args(["node", "show", &run_id, "n-9999"]), 1),
        &red,
    );

    // Wet (non-dry) `node report` success envelope — locks the real write
    // shape, not just the dry-run plan. Last, since it mutates n-0001.
    let out = ok_stdout(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        &run_id,
        "n-0001",
        "--from-file",
        &rep,
    ]));
    snapshot("node_report_json", &out, &red);
}
