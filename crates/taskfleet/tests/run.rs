//! Integration tests for the `run` subcommand family.
//!
//! Every test points the binary at a fresh `TempDir` via
//! `TASKFLEET_HOME` so the user's real `~/.taskfleet/` is
//! never touched.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use serde_json::{json, Value};
use tempfile::TempDir;

mod common;
use common::{NativeSpawnTools, TestHome};

fn bin(home: &TempDir) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    c.env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path());
    c.env("TASKFLEET_TEST_SKIP_MATERIALIZE", "1");
    c
}

fn run_ok(cmd: &mut Command) -> Value {
    let out = cmd.output().expect("spawn");
    assert!(
        out.status.success(),
        "exit={:?} stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout is valid JSON")
}

fn run_fail(cmd: &mut Command) -> (i32, Value) {
    let out = cmd.output().expect("spawn");
    assert!(!out.status.success(), "expected failure");
    let code = out.status.code().expect("exit code");
    let stderr = String::from_utf8(out.stderr).expect("utf8");
    let last = stderr.lines().last().expect("stderr has at least one line");
    let v: Value = serde_json::from_str(last).expect("error envelope JSON");
    (code, v)
}

fn create(home: &TempDir, kind: &str, title: &str) -> String {
    let v = run_ok(bin(home).args([
        "--output", "json", "run", "create", "--kind", kind, "--title", title,
    ]));
    v["data"]["run_id"]
        .as_str()
        .expect("run_id is string")
        .to_string()
}

fn create_for_repo(home: &TempDir, title: &str, repo: &std::path::Path) -> String {
    let v = run_ok(bin(home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        title,
        "--source-repo",
        repo.to_str().unwrap(),
    ]));
    v["data"]["run_id"].as_str().unwrap().to_string()
}

fn git_init(path: &std::path::Path) {
    std::fs::create_dir_all(path).unwrap();
    let out = Command::new("git")
        .args(["init", "--quiet"])
        .arg(path)
        .output()
        .expect("git available for repository identity test");
    assert!(
        out.status.success(),
        "git init: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write_profiles(home: &TempDir) {
    std::fs::write(
        home.path().join("config.toml"),
        r#"
[profiles.capable]
description = "General fictional worker"
capability = "capable"
residency = "remote"
agents = [
  { harness = "claude", command = ["/bin/sh"] },
  { harness = "pi", command = ["/bin/sh", "--fictional"], telemetry = "worker-v1" },
]

[profile]
default = "capable"
"#,
    )
    .unwrap();
}

#[test]
fn profile_resolution_matches_dry_run_persisted_create_and_show() {
    let home = TestHome::new();
    write_profiles(&home);

    let dry = run_ok(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "preview",
        "--dry-run",
    ]));
    let selection = &dry["data"]["selection"];
    assert_eq!(selection["schema_version"], 1);
    assert_eq!(selection["profile"], "capable");
    assert_eq!(selection["selection_source"], "user-default");
    assert_eq!(selection["interaction"], "autonomous");
    assert_eq!(selection["selected"]["candidate_index"], 1);
    assert_eq!(selection["selected"]["harness"], "pi");
    assert_eq!(
        selection["selected"]["command"],
        json!(["/bin/sh", "--fictional"])
    );
    assert_eq!(
        selection["fallback"][0]["reason"],
        "autonomous_harness_unsupported"
    );
    assert!(
        !home.path().join("runs").exists(),
        "dry-run created run state"
    );

    let created = run_ok(bin(&home).args([
        "--output", "json", "run", "create", "--kind", "spinoff", "--title", "stored",
    ]));
    assert_eq!(created["data"]["selection"], *selection);
    let run_id = created["data"]["run_id"].as_str().unwrap();
    let shown = run_ok(bin(&home).args(["--output", "json", "run", "show", run_id]));
    assert_eq!(shown["data"]["manifest"]["selection"], *selection);
}

#[test]
fn telemetry_requirement_and_support_come_only_from_recorded_policy() {
    let home = TestHome::new();
    write_profiles(&home);

    // Autonomous resolution skips Claude and records adapted pi. No sample is
    // needed for support to be configured.
    let autonomous = run_ok(bin(&home).args([
        "--output", "json", "run", "create", "--kind", "spinoff", "--title", "auto",
    ]));
    let autonomous_id = autonomous["data"]["run_id"].as_str().unwrap();
    create_node(&home, autonomous_id, "n-0001");
    let shown = run_ok(bin(&home).args(["--output", "json", "run", "show", autonomous_id]));
    assert_eq!(shown["data"]["telemetry"][0]["requirement"], "required");
    assert_eq!(shown["data"]["telemetry"][0]["support"], "configured");
    assert_eq!(shown["data"]["telemetry"][0]["sample"], "absent");

    // Explicit interaction selects the first (Claude) candidate. Interaction,
    // not kind/profile/sample, makes telemetry optional; the recorded selected
    // candidate, not sample arrival, makes support unsupported.
    let interactive = run_ok(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "human",
        "--interactive",
    ]));
    let interactive_id = interactive["data"]["run_id"].as_str().unwrap();
    create_node(&home, interactive_id, "n-0001");
    let shown = run_ok(bin(&home).args(["--output", "json", "run", "show", interactive_id]));
    assert_eq!(
        shown["data"]["manifest"]["selection"]["selected"]["harness"],
        "claude"
    );
    assert_eq!(shown["data"]["telemetry"][0]["requirement"], "optional");
    assert_eq!(shown["data"]["telemetry"][0]["support"], "unsupported");
    assert_eq!(shown["data"]["telemetry"][0]["sample"], "absent");
}

#[test]
fn materialized_create_routes_through_the_recorded_exact_argv() {
    let home = TestHome::new();
    let scratch = TempDir::new().unwrap();
    let observed = scratch.path().join("observed.txt");
    let worker = scratch.path().join("worker.sh");
    std::fs::write(
        &worker,
        "#!/bin/sh\nprintf '%s\\n' \"$TASKFLEET_RUN_ID\" \"$TASKFLEET_NODE_ID\" \"$TASKFLEET_ATTEMPT\" \"${TASKFLEET_INTERNAL_WORKER_AWAIT_PUBLICATION-unset}\" \"${TASKFLEET_INTERNAL_WORKER_STATE_ROOT-unset}\" \"$(($# + 1))\" \"$0\" \"$@\" > \"$CAPTURE_OUTPUT\"\nexec /bin/sleep 2\n",
    )
    .unwrap();
    std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            r#"[profiles.local]
description="Local fixture"
capability="fast"
residency="local"
agents=[{{harness="pi",command=["/bin/sh","{}","one two","quote'arg"],telemetry="worker-v1"}}]
[profile]
default="local"
"#,
            worker.display()
        ),
    )
    .unwrap();

    let native = NativeSpawnTools::new();
    let worktree = native.worktree("worktree");

    // A hostile product-name executable proves both the worker shim and detached
    // supervisor re-enter the exact current binary rather than consulting PATH.
    let hostile_marker = scratch.path().join("hostile-product-binary-ran");
    let hostile = scratch.path().join("taskfleet");
    std::fs::write(
        &hostile,
        format!("#!/bin/sh\n: > '{}'\nexit 99\n", hostile_marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&hostile, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    command
        // Simulate an equipped developer shell. NativeSpawnTools must remove
        // this ambient placement so the explicit fixture session below is the
        // only placement contract the create can use.
        .env("TMUX", "/hostile/ambient-tmux,1,2")
        .current_dir(home.path().parent().unwrap())
        // A relative selected root must be made absolute before it crosses the
        // worktree/self-exec boundary.
        .env("TASKFLEET_HOME", home.path().file_name().unwrap())
        .env("HOME", home.path())
        // Only the hostile product-name fixtures are searchable. Every tool
        // this scenario legitimately needs is addressed by absolute path.
        .env("PATH", scratch.path());
    native.configure(&mut command, &worktree, "fixture");
    assert!(command
        .get_envs()
        .any(|(key, value)| key == "TMUX" && value.is_none()));
    command.env("CAPTURE_OUTPUT", &observed).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--tmux-session",
        "fixture",
        "--title",
        "exact",
        "--task",
        "do it",
    ]);
    let created = run_ok(&mut command);
    let run_id = created["data"]["run_id"].as_str().unwrap();
    let node: Value = serde_json::from_slice(
        &std::fs::read(
            home.path()
                .join("runs")
                .join(run_id)
                .join("nodes/n-0001.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(node["tmux_identity"]["session"], "fixture");
    let pid = node["agent_pid"].as_i64().unwrap();
    for _ in 0..200 {
        if observed.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let observed_bytes = std::fs::read_to_string(&observed).unwrap_or_else(|error| {
        let run_dir = home.path().join("runs").join(run_id);
        let events =
            std::fs::read_to_string(run_dir.join("events.jsonl")).unwrap_or_default();
        let launcher = std::fs::read_to_string(
            run_dir.join("agent-launch-n-0001-attempt-0.sh"),
        )
        .unwrap_or_default();
        let stderr = std::fs::read_to_string(native.agent_stderr()).unwrap_or_default();
        let logs = std::fs::read_to_string(home.path().join("logs/taskfleet.log.jsonl"))
            .unwrap_or_default();
        panic!(
            "worker did not start: {error}; events={events}; launcher={launcher} stderr={stderr} logs={logs}"
        )
    });
    let expected_prefix = format!(
        "{run_id}\nn-0001\n0\nunset\nunset\n5\n{}\none two\nquote'arg\n--\n",
        worker.display()
    );
    assert!(observed_bytes.starts_with(&expected_prefix));
    assert!(observed_bytes.ends_with("\n\ndo it\n"));
    // The recorded PID is the run-worker shim. Let its short-lived child exit
    // naturally so the shim records the durable worker.exited fact.
    assert!(pid > 0);
    let events = home.path().join("runs").join(run_id).join("events.jsonl");
    for _ in 0..100 {
        if std::fs::read_to_string(&events)
            .unwrap_or_default()
            .contains("\"worker.exited\"")
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(
        std::fs::read_to_string(events)
            .unwrap()
            .contains("\"worker.exited\""),
        "run-worker did not record the candidate's exit"
    );
    assert!(
        !hostile_marker.exists(),
        "a hidden path resolved a product name through PATH"
    );
}

#[test]
fn profile_launch_failure_stays_a_launch_failure_without_publication_or_fallback() {
    let home = TestHome::new();
    std::fs::write(
        home.path().join("config.toml"),
        r#"[profiles.only]
description="Only candidate"
capability="fast"
residency="local"
agents=[{harness="pi",command=["/bin/sh"],telemetry="worker-v1"}]
[profile]
default="only"
"#,
    )
    .unwrap();
    let native = NativeSpawnTools::new();
    let mut command = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    command
        .env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin");
    let worktree = native.worktree("worktree");
    native.configure(&mut command, &worktree, "headless");
    command.env("WORKMUX_BIN", "/usr/bin/false").args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--headless",
        "--title",
        "fails",
        "--task",
        "do it",
    ]);
    let (exit, error) = run_fail(&mut command);
    assert_eq!(exit, 1);
    assert_eq!(error["error"]["code"], "workmux_add_failed");
    assert!(
        !home.path().join("runs").exists()
            || std::fs::read_dir(home.path().join("runs"))
                .unwrap()
                .next()
                .is_none(),
        "failed profile launch published a run"
    );
}

#[test]
fn idempotent_profile_replay_uses_recorded_selection_after_config_drift() {
    let home = TestHome::new();
    write_profiles(&home);
    let first = run_ok(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "replay",
        "--idempotency-key",
        "profile-replay",
    ]));
    let recorded = first["data"]["selection"].clone();
    std::fs::write(
        home.path().join("config.toml"),
        "this is no longer valid toml = [",
    )
    .unwrap();
    let replay = run_ok(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "replay",
        "--idempotency-key",
        "profile-replay",
    ]));
    assert_eq!(replay["data"]["idempotent_replay"], true);
    assert_eq!(replay["data"]["selection"], recorded);
}

#[test]
fn exhausted_profile_returns_full_compact_attempt_without_mutation() {
    let home = TestHome::new();
    std::fs::write(
        home.path().join("config.toml"),
        r#"[profiles.secure]
description="Local"
capability="fast"
residency="local"
agents=[{harness="pi",command=["definitely-missing-taskfleet-fixture"],telemetry="worker-v1"}]
[profile]
default="secure"
"#,
    )
    .unwrap();
    let (code, error) = run_fail(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "none",
        "--dry-run",
    ]));
    assert_eq!(code, 1);
    assert_eq!(error["error"]["code"], "profile_candidates_exhausted");
    let selection = &error["error"]["details"]["selection"];
    assert_eq!(selection["profile"], "secure");
    assert_eq!(selection["interaction"], "autonomous");
    assert!(selection["selected"].is_null());
    assert_eq!(selection["fallback"][0]["reason"], "executable_missing");
    assert!(!home.path().join("runs").exists());
}

#[test]
fn explicit_invalid_source_repo_fails_instead_of_reading_cwd_config() {
    let home = TestHome::new();
    write_profiles(&home);
    let missing = home.path().join("missing-repo");
    let (code, error) = run_fail(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "bad-root",
        "--profile",
        "capable",
        "--source-repo",
        missing.to_str().unwrap(),
        "--dry-run",
    ]));
    assert_eq!(code, 1);
    assert_eq!(error["error"]["code"], "invalid_source_repo");
}

#[test]
fn repository_config_is_selection_only_and_cannot_define_commands() {
    let home = TestHome::new();
    write_profiles(&home);
    let repo = TempDir::new().unwrap();
    std::fs::create_dir(repo.path().join(".git")).unwrap();
    std::fs::write(
        repo.path().join(".taskfleet.toml"),
        r#"[profiles.evil]
description = "checkout executable"
capability = "fast"
residency = "local"
agents = [{ harness = "pi", command = ["/bin/sh"], telemetry = "worker-v1" }]
"#,
    )
    .unwrap();
    let (code, error) = run_fail(bin(&home).current_dir(repo.path()).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "reject",
        "--dry-run",
    ]));
    assert_eq!(code, 1);
    assert_eq!(error["error"]["code"], "invalid_repository_config");
    assert!(!home.path().join("runs").exists());
}

fn create_node(home: &TempDir, run_id: &str, node_id: &str) {
    let node_file = home.path().join(format!("{node_id}-created.json"));
    std::fs::write(
        &node_file,
        serde_json::to_vec(&json!({"kind": "spinoff"})).unwrap(),
    )
    .unwrap();
    run_ok(bin(home).args([
        "--output",
        "json",
        "event",
        "create",
        run_id,
        "--kind",
        "node.created",
        "--node-id",
        node_id,
        "--from-file",
        node_file.to_str().unwrap(),
    ]));
}

/// `run create --interactive` persists explicit `lifecycle: interactive`
/// how-run state — on the create envelope, in the stored manifest (`run show`),
/// and on the `run list` row — while a plain create stays `autonomous`. This is
/// the told-not-guessed flag that replaced the removed kind-derived interactive
/// lifecycle (design.md §6).
#[test]
fn create_interactive_persists_lifecycle() {
    let home = TestHome::new();

    // Explicit --interactive on a spinoff topology: lifecycle is interactive
    // everywhere it surfaces.
    let v = run_ok(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "hands-on",
        "--interactive",
    ]));
    assert_eq!(v["data"]["lifecycle"], "interactive");
    let run_id = v["data"]["run_id"].as_str().unwrap().to_string();

    let v = run_ok(bin(&home).args(["--output", "json", "run", "show", &run_id]));
    assert_eq!(v["data"]["manifest"]["lifecycle"], "interactive");

    let v = run_ok(bin(&home).args(["--output", "json", "run", "list"]));
    let row = v["data"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["run_id"] == run_id.as_str())
        .expect("interactive run in list");
    assert_eq!(row["lifecycle"], "interactive");

    // The default (flag omitted) stays autonomous.
    let auto = create(&home, "spinoff", "hands-off");
    let v = run_ok(bin(&home).args(["--output", "json", "run", "show", &auto]));
    assert_eq!(v["data"]["manifest"]["lifecycle"], "autonomous");
}

#[test]
fn create_then_list_then_show_then_cancel_flow() {
    let home = TestHome::new();
    let run_id = create(&home, "spinoff", "integration");

    // list returns the just-created run with status pending and
    // node_count 0 (no node.created yet — that's the supervisor's job).
    let v = run_ok(bin(&home).args(["--output", "json", "run", "list"]));
    let runs = v["data"]["runs"].as_array().expect("runs array");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["run_id"], run_id);
    assert_eq!(runs[0]["status"], "pending");
    assert_eq!(runs[0]["kind"], "spinoff");

    // show returns the full manifest under `manifest` and counters.
    let v = run_ok(bin(&home).args(["--output", "json", "run", "show", &run_id]));
    assert_eq!(v["data"]["manifest"]["run_id"], run_id);
    assert_eq!(v["data"]["counts"]["nodes"], 0);

    // cancel emits run.status:cancelled. With 0 nodes, no synthesized
    // node.report events — `cancelled_nodes` must be empty.
    let v = run_ok(bin(&home).args(["--output", "json", "run", "cancel", &run_id]));
    assert_eq!(v["data"]["already_cancelled"], false);
    assert_eq!(v["data"]["cancelled_nodes"].as_array().unwrap().len(), 0);

    // Idempotent re-cancel.
    let v = run_ok(bin(&home).args(["--output", "json", "run", "cancel", &run_id]));
    assert_eq!(v["data"]["already_cancelled"], true);

    let v = run_ok(bin(&home).args(["--output", "json", "run", "show", &run_id]));
    assert_eq!(v["data"]["manifest"]["status"], "cancelled");
}

/// `run show` exposes the complete terminal report from its default worker so
/// an orchestrator can read the worker's reasoning without reaching into the
/// node projection. `node show` retains both the projection-native
/// `last_report` and its consumer-facing `report` alias.
#[test]
fn show_surfaces_default_worker_report() {
    let home = TestHome::new();
    let run_id = create(&home, "spinoff", "report surface");

    // Create the default worker node through the public event path.
    create_node(&home, &run_id, "n-0001");
    let before_report = run_ok(bin(&home).args(["--output", "json", "run", "show", &run_id]));
    assert!(before_report["data"]["report"].is_null());

    let report_file = home.path().join("report.json");
    std::fs::write(
        &report_file,
        serde_json::to_vec(&json!({
            "success": true,
            "summary": "explained the result",
            "discussion_items": [],
            "spinoff_proposals": [],
            "wrap_up_recommendations": ["run the next lane"],
        }))
        .unwrap(),
    )
    .unwrap();
    run_ok(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        &run_id,
        "n-0001",
        "--from-file",
        report_file.to_str().unwrap(),
    ]));

    let shown = run_ok(bin(&home).args(["--output", "json", "run", "show", &run_id]));
    let report = &shown["data"]["report"];
    assert_eq!(report["summary"], "explained the result");
    assert_eq!(report["discussion_items"], json!([]));
    assert_eq!(
        report["wrap_up_recommendations"],
        json!(["run the next lane"])
    );
    let node = run_ok(bin(&home).args(["--output", "json", "node", "show", &run_id, "n-0001"]));
    assert_eq!(report, &node["data"]["report"]);
    assert_eq!(report, &node["data"]["last_report"]);
}

#[test]
fn show_hides_single_worker_report_on_multi_node_runs() {
    let home = TestHome::new();
    let run_id = create(&home, "fan-out", "many reports");
    create_node(&home, &run_id, "n-0001");
    create_node(&home, &run_id, "n-0002");
    let report_file = home.path().join("first-node-report.json");
    std::fs::write(
        &report_file,
        serde_json::to_vec(&json!({"success": true, "summary": "first only"})).unwrap(),
    )
    .unwrap();
    run_ok(bin(&home).args([
        "--output",
        "json",
        "node",
        "report",
        &run_id,
        "n-0001",
        "--from-file",
        report_file.to_str().unwrap(),
    ]));

    let shown = run_ok(bin(&home).args(["--output", "json", "run", "show", &run_id]));
    assert!(shown["data"]["report"].is_null());
}

/// Regression for `run-show-json-null-fields`: `run show`'s `data` must carry
/// the run's identity + liveness at the TOP level, in the SAME flat shape a
/// `run list` row uses — `data.run_id` / `data.kind` / `data.status` /
/// `data.title` / `data.supervisor` — not only nested under `data.manifest`.
/// A consumer that reused `run list`'s flat field layout on `run show` output
/// (the bundled `worktree-spinoff` skill reads `data.supervisor`) observed a
/// silent `null` for every field, for a live, resolvable run. This pins the
/// flat placement, asserts `supervisor` is no longer nested under `manifest`,
/// and checks the two verbs agree field-for-field — so a future re-nesting
/// fails loudly.
#[test]
fn show_surfaces_run_row_at_top_level_matching_list() {
    let home = TestHome::new();
    let run_id = create(&home, "spinoff", "sup-placement");

    let show = run_ok(bin(&home).args(["--output", "json", "run", "show", &run_id]));
    let data = show["data"].as_object().expect("data must be an object");

    // The run-list row fields are reachable FLAT on run show's data.
    assert_eq!(data["run_id"], run_id, "data.run_id must be flat");
    assert_eq!(data["kind"], "spinoff", "data.kind must be flat");
    assert_eq!(data["status"], "pending", "data.status must be flat");
    assert_eq!(data["title"], "sup-placement", "data.title must be flat");

    // Supervisor is a well-formed object at the top level with exactly the
    // documented keys (a stray extra key would be silent schema drift).
    let sup = data["supervisor"]
        .as_object()
        .expect("data.supervisor must be an object");
    let mut sup_keys: Vec<&str> = sup.keys().map(String::as_str).collect();
    sup_keys.sort_unstable();
    assert_eq!(
        sup_keys,
        ["alive", "pid", "state"],
        "data.supervisor must carry exactly pid + state + alive"
    );

    // The nested `manifest` still exists (back-compat) but must NOT re-introduce
    // the buried supervisor. `contains_key` distinguishes "absent" from an
    // explicit `null` — `Value::Index` would report both as `is_null()`.
    let manifest = data["manifest"]
        .as_object()
        .expect("data.manifest must remain for back-compat");
    assert!(
        !manifest.contains_key("supervisor"),
        "supervisor must be absent from data.manifest, not merely null"
    );

    // The two verbs agree field-for-field on the shared row shape, so a consumer
    // can switch between a list row and a show payload without re-pathing.
    let list = run_ok(bin(&home).args(["--output", "json", "run", "list"]));
    let row = list["data"]["runs"][0]
        .as_object()
        .expect("run list row must be an object");
    for key in [
        "run_id",
        "kind",
        "status",
        "title",
        "node_count",
        "supervisor",
    ] {
        assert_eq!(
            data.get(key),
            row.get(key),
            "run show and run list disagree on flat field `{key}`"
        );
    }
}

#[test]
fn list_repo_filter_uses_git_identity_and_excludes_nested_repository() {
    let home = TestHome::new();
    let repos = TempDir::new().unwrap();
    let outer = repos.path().join("outer");
    let nested = outer.join("vendor/independent");
    let subdir = outer.join("src/deep");
    let linked = repos.path().join("outer-linked");
    let other = repos.path().join("other");
    git_init(&outer);
    let commit = Command::new("git")
        .args(["-C"])
        .arg(&outer)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(["commit", "--quiet", "--allow-empty", "-m", "init"])
        .output()
        .unwrap();
    assert!(
        commit.status.success(),
        "git commit: {}",
        String::from_utf8_lossy(&commit.stderr)
    );
    let add = Command::new("git")
        .args(["-C"])
        .arg(&outer)
        .args(["worktree", "add", "--quiet", "-b", "linked"])
        .arg(&linked)
        .output()
        .unwrap();
    assert!(
        add.status.success(),
        "git worktree add: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    git_init(&nested);
    git_init(&other);
    std::fs::create_dir_all(&subdir).unwrap();

    let outer_id = create_for_repo(&home, "outer-run", &outer);
    let linked_id = create_for_repo(&home, "linked-run", &linked);
    let nested_id = create_for_repo(&home, "nested-run", &nested);
    let _other_id = create_for_repo(&home, "other-run", &other);
    let _unknown_id = create(&home, "spinoff", "unrecorded-run");

    // `.` from a subdirectory resolves the containing repository. A nested
    // independent repository must not match merely because its path has the
    // selected checkout as a prefix.
    let mut command = bin(&home);
    command
        .current_dir(&subdir)
        .args(["--output", "json", "run", "list", "--repo", "."]);
    let listed = run_ok(&mut command);
    let rows = listed["data"]["runs"].as_array().unwrap();
    assert_eq!(
        rows.len(),
        2,
        "main and linked worktrees share one exact git identity"
    );
    let listed_ids: Vec<&str> = rows
        .iter()
        .map(|row| row["run_id"].as_str().unwrap())
        .collect();
    assert!(listed_ids.contains(&outer_id.as_str()));
    assert!(listed_ids.contains(&linked_id.as_str()));
    assert!(!listed_ids.contains(&nested_id.as_str()));
    assert!(rows
        .iter()
        .any(|row| row["source_repo"] == outer.display().to_string()));

    // Without the filter, repository provenance stays explicit and unknown
    // provenance remains a truthful null rather than an inferred identity.
    let all = run_ok(bin(&home).args(["--output", "json", "run", "list"]));
    let rows = all["data"]["runs"].as_array().unwrap();
    assert_eq!(rows.len(), 5);
    let unrecorded = rows
        .iter()
        .find(|row| row["title"] == "unrecorded-run")
        .unwrap();
    assert!(unrecorded["source_repo"].is_null());
}

#[test]
fn create_dry_run_does_not_touch_filesystem() {
    let home = TestHome::new();
    let v = run_ok(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "x",
        "--dry-run",
    ]));
    assert_eq!(v["data"]["dry_run"], true);
    assert_eq!(v["data"]["supervisor"], "not-spawned-dry-run");
    // No runs dir should exist (root not initialized).
    assert!(!home.path().join("runs").exists());
}

#[test]
fn create_child_dry_run_is_unsupported() {
    let home = TestHome::new();
    let parent = create(&home, "fan-out", "parent");
    let (code, v) = run_fail(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "x",
        "--parent-run-id",
        &parent,
        "--parent-node-id",
        "n-0001",
        "--dry-run",
    ]));
    assert_eq!(code, 1);
    assert_eq!(v["error"]["code"], "dry_run_unsupported");
}

#[test]
fn create_child_writes_child_spawned_to_parent_events() {
    let home = TestHome::new();
    let parent = create(&home, "fan-out", "parent");
    let v = run_ok(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "child",
        "--parent-run-id",
        &parent,
        "--parent-node-id",
        "n-0001",
    ]));
    let child = v["data"]["run_id"].as_str().unwrap().to_string();
    assert_eq!(v["data"]["parent_run_id"], parent);

    // Parent run's events.jsonl must contain a child.spawned record
    // naming the child's run_id. This is the §7.2 protocol — child.spawned
    // belongs on the *parent's* log, not the child's.
    let parent_events =
        std::fs::read_to_string(home.path().join("runs").join(&parent).join("events.jsonl"))
            .expect("parent events readable");
    let mut saw_child_spawned = false;
    for line in parent_events.lines() {
        let v: Value = serde_json::from_str(line).unwrap();
        if v["kind"] == "child.spawned" && v["data"]["child_run_id"] == child {
            saw_child_spawned = true;
        }
    }
    assert!(
        saw_child_spawned,
        "parent events missing child.spawned for {child}: {parent_events}"
    );

    // Child run's events must contain run.created (NOT child.spawned).
    let child_events =
        std::fs::read_to_string(home.path().join("runs").join(&child).join("events.jsonl"))
            .expect("child events readable");
    assert!(
        child_events.lines().any(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            v["kind"] == "run.created"
        }),
        "child missing run.created: {child_events}"
    );
}

#[test]
fn create_with_idempotency_key_returns_same_run_id() {
    let home = TestHome::new();
    let v1 = run_ok(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "x",
        "--idempotency-key",
        "abc",
    ]));
    let r1 = v1["data"]["run_id"].as_str().unwrap().to_string();

    let v2 = run_ok(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "x",
        "--idempotency-key",
        "abc",
    ]));
    assert_eq!(v2["data"]["run_id"], r1);
    assert_eq!(v2["data"]["idempotent_replay"], true);

    // Only one run materialized on disk.
    let count = std::fs::read_dir(home.path().join("runs")).unwrap().count();
    assert_eq!(count, 1);
}

/// Regression for `idempotency-key-allowed-duplicate-run`: firing N `run
/// create` calls with the SAME `--idempotency-key` concurrently must yield
/// exactly ONE run, not one per call. Before the fix the key was persisted only
/// after a run fully materialized, so near-simultaneous calls all missed the
/// pre-create lookup and each spawned a distinct run. The atomic reservation
/// closes that window: exactly one caller materializes, the rest replay.
#[test]
fn concurrent_same_idempotency_key_creates_one_run() {
    let home = TestHome::new();
    const N: usize = 8;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(N));

    let handles: Vec<_> = (0..N)
        .map(|_| {
            let path = home.path().to_path_buf();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut cmd = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
                cmd.env("TASKFLEET_HOME", &path)
                    .env("HOME", &path)
                    .env("TASKFLEET_TEST_SKIP_MATERIALIZE", "1")
                    .args([
                        "--output",
                        "json",
                        "run",
                        "create",
                        "--kind",
                        "spinoff",
                        "--title",
                        "race",
                        "--idempotency-key",
                        "same-key",
                    ]);
                // Release all processes as close together as possible.
                barrier.wait();
                let out = cmd.output().expect("spawn");
                assert!(
                    out.status.success(),
                    "exit={:?} stderr={}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr)
                );
                let v: Value = serde_json::from_slice(&out.stdout).expect("json");
                (
                    v["data"]["run_id"].as_str().unwrap().to_string(),
                    v["data"]["idempotent_replay"] == Value::Bool(true),
                )
            })
        })
        .collect();

    let results: Vec<(String, bool)> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    // Exactly one run materialized on disk.
    let count = std::fs::read_dir(home.path().join("runs")).unwrap().count();
    assert_eq!(count, 1, "expected exactly one run dir, got {count}");

    // Every call resolved to the same run-id (the single materialized run).
    let materialized = std::fs::read_dir(home.path().join("runs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name()
        .into_string()
        .unwrap();
    assert!(
        results.iter().all(|(id, _)| *id == materialized),
        "all callers must return the one materialized run-id {materialized}; got {results:?}"
    );

    // Exactly one caller was the creator (no replay); the other N-1 replayed.
    let replays = results.iter().filter(|(_, r)| *r).count();
    assert_eq!(
        replays,
        N - 1,
        "exactly one creator + {} replays expected; got {replays} replays",
        N - 1
    );
}

/// Regression for the reservation-leak the review caught: an error AFTER the
/// idempotency key is reserved but BEFORE the run is durable must free the key
/// (via the drop-guard), so a later retry with the same key re-spawns cleanly
/// instead of replaying a phantom run that was never materialized. Here the
/// early error is a child spawn against a non-existent parent (`parent_not_found`
/// fires after `reserve`); the retry is a valid top-level create with the same
/// key, which must mint a NEW run, not `idempotent_replay` the leaked one.
#[test]
fn early_failure_after_reserve_frees_the_key() {
    let home = TestHome::new();
    let key = "leak-key";

    // A child create against a parent that does not exist: reserve succeeds,
    // then the parent-existence check fails. The guard must release the key.
    let (code, v) = run_fail(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "orphan",
        "--parent-run-id",
        "01000000000000000000000000",
        "--parent-node-id",
        "n-0001",
        "--idempotency-key",
        key,
    ]));
    assert_eq!(code, 1);
    assert_eq!(v["error"]["code"], "parent_not_found");
    // No run materialized, and the key file was released (not left behind).
    let runs = home.path().join("runs");
    let count = std::fs::read_dir(&runs).map_or(0, Iterator::count);
    assert_eq!(count, 0, "no run should have materialized");

    // Retry with the SAME key as a normal top-level create: because the key was
    // freed, this is a fresh creation — not an idempotent replay of a phantom.
    let v2 = run_ok(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "retry",
        "--idempotency-key",
        key,
    ]));
    assert_ne!(
        v2["data"]["idempotent_replay"],
        Value::Bool(true),
        "a freed key must not replay: {v2}"
    );
    assert!(v2["data"]["run_id"].is_string());
    let count = std::fs::read_dir(&runs).unwrap().count();
    assert_eq!(count, 1, "exactly the retry's run exists");
}

#[test]
fn create_rejects_empty_title() {
    let home = TestHome::new();
    let (code, v) = run_fail(bin(&home).args([
        "--output", "json", "run", "create", "--kind", "spinoff", "--title", "   ",
    ]));
    assert_eq!(code, 1);
    assert_eq!(v["error"]["code"], "invalid_value");
}

#[test]
fn create_rejects_unbalanced_parent_flags() {
    let home = TestHome::new();
    // Only --parent-run-id without --parent-node-id is rejected by clap
    // (requires=...) with the structured envelope.
    let (code, v) = run_fail(bin(&home).args([
        "--output",
        "json",
        "run",
        "create",
        "--kind",
        "spinoff",
        "--title",
        "x",
        "--parent-run-id",
        "some-id",
    ]));
    assert_eq!(code, 1);
    assert!(
        v["error"]["code"].is_string(),
        "error.code must be present: {v}"
    );
}

#[test]
fn show_missing_run_returns_run_not_found() {
    let home = TestHome::new();
    // A well-formed ULID that simply names no run → run_not_found.
    let (code, v) = run_fail(bin(&home).args([
        "--output",
        "json",
        "run",
        "show",
        "01jzabsent0000000000000000",
    ]));
    assert_eq!(code, 1);
    assert_eq!(v["error"]["code"], "run_not_found");
    assert_eq!(v["error"]["invalid_value"], "01jzabsent0000000000000000");
}

#[test]
fn show_malformed_run_id_returns_invalid_run_id() {
    let home = TestHome::new();
    // A malformed id is a distinct error class from a missing run.
    let (code, v) = run_fail(bin(&home).args(["--output", "json", "run", "show", "nope"]));
    assert_eq!(code, 1);
    assert_eq!(v["error"]["code"], "invalid_run_id");
    assert_eq!(v["error"]["invalid_value"], "nope");
}

#[test]
fn cancel_missing_run_returns_run_not_found() {
    let home = TestHome::new();
    let (code, v) = run_fail(bin(&home).args([
        "--output",
        "json",
        "run",
        "cancel",
        "01jzabsent0000000000000000",
    ]));
    assert_eq!(code, 1);
    assert_eq!(v["error"]["code"], "run_not_found");
}

#[test]
fn cancel_malformed_run_id_returns_invalid_run_id() {
    let home = TestHome::new();
    // Uppercase ULID-shaped input is non-canonical → invalid_run_id.
    let (code, v) = run_fail(bin(&home).args([
        "--output",
        "json",
        "run",
        "cancel",
        "01JZABSENT0000000000000000",
    ]));
    assert_eq!(code, 1);
    assert_eq!(v["error"]["code"], "invalid_run_id");
}

#[test]
fn cancel_resolves_unambiguous_run_id_prefix() {
    let home = TestHome::new();
    let run_id = create(&home, "spinoff", "prefix");
    // A prefix (first 12 chars) that names exactly one run resolves to the full
    // id and cancels it — the payload echoes the resolved full id, not the prefix.
    let prefix = &run_id[..12];
    let v = run_ok(bin(&home).args(["--output", "json", "run", "cancel", prefix]));
    assert_eq!(v["data"]["run_id"], run_id);
    assert_eq!(v["data"]["already_cancelled"], false);

    let v = run_ok(bin(&home).args(["--output", "json", "run", "show", prefix]));
    assert_eq!(v["data"]["manifest"]["run_id"], run_id);
    assert_eq!(v["data"]["manifest"]["status"], "cancelled");
}

#[test]
fn cancel_ambiguous_prefix_errors_with_candidates() {
    let home = TestHome::new();
    // Two runs created together share the ULID's leading timestamp chars; their
    // longest common prefix is guaranteed to match both (and only both).
    let a = create(&home, "spinoff", "one");
    let b = create(&home, "spinoff", "two");
    let lcp: String = a
        .chars()
        .zip(b.chars())
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x)
        .collect();
    assert!(
        !lcp.is_empty(),
        "ULIDs created together share the timestamp head"
    );

    let (code, v) = run_fail(bin(&home).args(["--output", "json", "run", "cancel", &lcp]));
    assert_eq!(code, 1);
    assert_eq!(v["error"]["code"], "ambiguous_run_id");
    let candidates = v["error"]["expected"]
        .as_array()
        .expect("expected is array");
    assert!(candidates.iter().any(|c| c == &json!(a)));
    assert!(candidates.iter().any(|c| c == &json!(b)));
}

#[test]
fn cancel_unknown_prefix_returns_run_not_found() {
    let home = TestHome::new();
    let _run_id = create(&home, "spinoff", "present");
    // A well-formed prefix that matches no run → run_not_found (not invalid).
    let (code, v) = run_fail(bin(&home).args(["--output", "json", "run", "cancel", "7zzzzzzzz"]));
    assert_eq!(code, 1);
    assert_eq!(v["error"]["code"], "run_not_found");
    assert_eq!(v["error"]["invalid_value"], "7zzzzzzzz");
}

#[test]
fn cancel_impossible_prefix_leading_digit_returns_invalid_run_id() {
    let home = TestHome::new();
    // A valid ULID's first char is bounded to 0..=7 (timestamp range), so an
    // 8-/9-leading prefix can never match any run — it is malformed, not merely
    // absent, and must classify as invalid_run_id (not run_not_found).
    let (code, v) = run_fail(bin(&home).args(["--output", "json", "run", "cancel", "8abc"]));
    assert_eq!(code, 1);
    assert_eq!(v["error"]["code"], "invalid_run_id");
    assert_eq!(v["error"]["invalid_value"], "8abc");
}

#[test]
fn reattach_spawns_supervisor_and_records_events() {
    let home = TestHome::new();
    let run_id = create(&home, "spinoff", "x");
    let v = run_ok(bin(&home).args(["--output", "json", "run", "reattach", &run_id, "--once"]));
    assert_eq!(v["data"]["action"], "reattached");
    assert!(v["data"]["supervisor_pid"].as_u64().is_some());

    // Give the spawned --once supervisor a beat to write its exit event.
    std::thread::sleep(std::time::Duration::from_millis(500));

    let events =
        std::fs::read_to_string(home.path().join("runs").join(&run_id).join("events.jsonl"))
            .unwrap();
    let kinds: Vec<String> = events
        .lines()
        .map(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            v["kind"].as_str().unwrap().to_string()
        })
        .collect();
    assert!(
        kinds.contains(&"supervisor.reattach-requested".to_string()),
        "kinds: {kinds:?}"
    );
    assert!(
        kinds.contains(&"supervisor.reattached".to_string()),
        "kinds: {kinds:?}"
    );
}

#[test]
fn reattach_missing_run_returns_run_not_found() {
    let home = TestHome::new();
    let (code, v) = run_fail(bin(&home).args([
        "--output",
        "json",
        "run",
        "reattach",
        "01jzabsent0000000000000000",
    ]));
    assert_eq!(code, 1);
    assert_eq!(v["error"]["code"], "run_not_found");
}

#[test]
fn list_filters_by_kind_and_status() {
    let home = TestHome::new();
    let a = create(&home, "spinoff", "a");
    let _b = create(&home, "research", "b");

    let v = run_ok(bin(&home).args(["--output", "json", "run", "list", "--kind", "spinoff"]));
    let runs = v["data"]["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["run_id"], a);

    // Filter that matches nothing returns an empty list (not an error).
    let v = run_ok(bin(&home).args(["--output", "json", "run", "list", "--status", "done"]));
    assert!(v["data"]["runs"].as_array().unwrap().is_empty());
}

#[test]
fn list_when_root_missing_returns_empty() {
    let home = TestHome::new();
    // No runs created — runs/ dir does not exist yet.
    let v = run_ok(bin(&home).args(["--output", "json", "run", "list"]));
    assert!(v["data"]["runs"].as_array().unwrap().is_empty());
}

/// `run list` flags a *stillborn* run — created, but its supervisor died before
/// creating any worker node (pending, no supervisor, 0 nodes, no progress since
/// creation) — with `stillborn: true`, so it is no longer a silent `pending`
/// row that looks stuck until someone notices (issue
/// `supervisor-dies-before-worker-node`). Under `TASKFLEET_TEST_SKIP_MATERIALIZE` a
/// fresh `run create` spawns no supervisor, giving exactly that shape. A run
/// that reached its first node is NOT stillborn — the two flags never coincide,
/// since stillborn requires `node_count == 0`.
///
/// `TASKFLEET_STILLBORN_LIST_GRACE_SECS=0` disables the age gate so the just-created
/// run flags immediately; the companion `list_within_grace_does_not_flag_
/// stillborn` test covers the default (grace-protected) create window.
#[test]
fn list_flags_stillborn_run() {
    let home = TestHome::new();
    let born = create(&home, "spinoff", "stillborn");
    let started = create(&home, "spinoff", "started");
    // Give `started` its first worker node → node_count 1, so it is not
    // stillborn even though its supervisor is likewise absent in this fixture.
    add_node(&home, &started, "n-0001");

    let v = run_ok(
        bin(&home)
            .env("TASKFLEET_STILLBORN_LIST_GRACE_SECS", "0")
            .args(["--output", "json", "run", "list"]),
    );
    let runs = v["data"]["runs"].as_array().expect("runs array");
    let row = |id: &str| {
        runs.iter()
            .find(|r| r["run_id"] == id)
            .unwrap_or_else(|| panic!("run {id} missing from list"))
    };

    let b = row(&born);
    assert_eq!(b["status"], "pending");
    assert_eq!(b["node_count"], 0);
    assert_eq!(b["supervisor"]["alive"], false);
    // No supervisor was spawned (SKIP_MATERIALIZE), so no pid file exists → the
    // distinct `not-recorded` state, not the conflated old `alive:false`.
    assert_eq!(b["supervisor"]["state"], "not-recorded");
    assert_eq!(
        b["stillborn"], true,
        "0-node dead-supervisor run is stillborn: {b}"
    );
    // `stalled` is the umbrella flag; a stillborn run trips it too so a caller
    // keying on the generic "not progressing" hint still catches it.
    assert_eq!(
        b["stalled"], true,
        "stillborn implies the umbrella stalled hint: {b}"
    );

    let s = row(&started);
    assert_eq!(s["node_count"], 1);
    assert_eq!(
        s["stillborn"], false,
        "a run that reached n-0001 is not stillborn: {s}"
    );
    assert_eq!(
        s["stalled"], false,
        "a non-orchestrate run with a node is not stalled: {s}"
    );

    // The plain-text row carries the `(stillborn)` marker, distinct from
    // `(stalled)`, so a human scanning `run list` sees the dead run.
    let out = bin(&home)
        .env("TASKFLEET_STILLBORN_LIST_GRACE_SECS", "0")
        .args(["--output", "text", "run", "list"])
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "run list (text) failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).expect("utf8");
    let born_line = text
        .lines()
        .find(|l| l.contains(&born))
        .expect("stillborn run present in text output");
    assert!(
        born_line.contains("pending (stillborn)"),
        "text row must mark the run stillborn: {born_line}"
    );
}

/// A run younger than the stillborn grace is NOT flagged, even though it has the
/// stillborn shape (pending, 0 nodes, no supervisor). This is the false-positive
/// guard: `run list` sweeps runs another process is mid-`run create` on, which
/// transiently present that exact shape during the create.sh window — a bulk
/// list (or a monitor over `--json`) must not flag/cancel a healthy in-flight
/// run. With the default grace (900s) a just-created run is well within the
/// window, so `stillborn` reads `false` (issue `supervisor-dies-before-worker-
/// node`, review finding: transient create-window false positive).
#[test]
fn list_within_grace_does_not_flag_stillborn() {
    let home = TestHome::new();
    let born = create(&home, "spinoff", "fresh");

    // No env override → the default 900s grace applies. The run was created
    // milliseconds ago, so it is inside the create window and must not flag.
    let v = run_ok(bin(&home).args(["--output", "json", "run", "list"]));
    let r = v["data"]["runs"]
        .as_array()
        .expect("runs array")
        .iter()
        .find(|r| r["run_id"] == born)
        .expect("run present");
    assert_eq!(r["status"], "pending");
    assert_eq!(r["node_count"], 0);
    assert_eq!(
        r["stillborn"], false,
        "a run inside the create-window grace must NOT be flagged stillborn: {r}"
    );
    assert_eq!(
        r["stalled"], false,
        "nor tripped as stalled while still within grace: {r}"
    );
}

// --- `run cancel` terminal-run + convergence semantics ---------------------
//
// These drive a run's state through the sanctioned `event create` write path
// (a temp JSON `--from-file` payload), then exercise `run cancel`.

/// Append one event via `event create`, writing `data` to a temp file the
/// command reads with `--from-file`. The `keep` `TempDir` must outlive the call.
fn event_create(home: &TempDir, run_id: &str, kind: &str, node_id: Option<&str>, data: Value) {
    let keep = TempDir::new().unwrap();
    let f = keep.path().join("data.json");
    std::fs::write(&f, serde_json::to_vec(&data).unwrap()).unwrap();
    let mut cmd = bin(home);
    cmd.args([
        "--output", "json", "event", "create", run_id, "--kind", kind,
    ]);
    if let Some(n) = node_id {
        cmd.args(["--node-id", n]);
    }
    cmd.args(["--from-file", f.to_str().unwrap()]);
    run_ok(&mut cmd);
}

fn add_node(home: &TempDir, run_id: &str, node_id: &str) {
    event_create(
        home,
        run_id,
        "node.created",
        Some(node_id),
        json!({ "kind": "spinoff" }),
    );
}

/// Settle a node via the §7.3-owned `node report` path (`node.report` is not
/// routable through `event create`).
fn node_report(home: &TempDir, run_id: &str, node_id: &str, data: Value) {
    let keep = TempDir::new().unwrap();
    let f = keep.path().join("report.json");
    std::fs::write(&f, serde_json::to_vec(&data).unwrap()).unwrap();
    run_ok(bin(home).args([
        "--output",
        "json",
        "node",
        "report",
        run_id,
        node_id,
        "--from-file",
        f.to_str().unwrap(),
    ]));
}

#[test]
fn cancel_done_run_is_refused_run_already_terminal() {
    let home = TestHome::new();
    let run_id = create(&home, "spinoff", "done-run");
    add_node(&home, &run_id, "n-0001");
    // Settle the node, then the run, to Done.
    node_report(&home, &run_id, "n-0001", json!({ "success": true }));
    event_create(
        &home,
        &run_id,
        "run.status",
        None,
        json!({ "status": "done" }),
    );

    let (code, v) = run_fail(bin(&home).args(["--output", "json", "run", "cancel", &run_id]));
    assert_eq!(code, 1);
    assert_eq!(v["error"]["code"], "run_already_terminal");
    assert_eq!(v["error"]["invalid_value"], "done");
    assert_eq!(
        v["error"]["expected"],
        json!(["running", "pending", "blocked"])
    );

    // The refusal mutated nothing: the run is still Done.
    let v = run_ok(bin(&home).args(["--output", "json", "run", "show", &run_id]));
    assert_eq!(v["data"]["manifest"]["status"], "done");
}

#[test]
fn cancel_already_cancelled_run_converges_live_node() {
    let home = TestHome::new();
    let run_id = create(&home, "spinoff", "interrupted-cancel");
    add_node(&home, &run_id, "n-0001");
    add_node(&home, &run_id, "n-0002");
    // Simulate an interrupted cancel: run marked cancelled, but n-0001 settled
    // while n-0002 is still live (its node.report never landed).
    node_report(
        &home,
        &run_id,
        "n-0001",
        json!({ "success": false, "cancelled": true, "reason": "stop" }),
    );
    event_create(
        &home,
        &run_id,
        "run.status",
        None,
        json!({ "status": "cancelled" }),
    );

    // Re-cancel converges the straggler and reports it.
    let v = run_ok(bin(&home).args(["--output", "json", "run", "cancel", &run_id]));
    assert_eq!(v["data"]["already_cancelled"], true);
    assert_eq!(
        v["data"]["cancelled_nodes"].as_array().unwrap(),
        &vec![Value::from("n-0002")]
    );
    assert_eq!(
        v["data"]["nodes_already_terminal"].as_array().unwrap(),
        &vec![Value::from("n-0001")]
    );

    // n-0002 is now cancelled on disk.
    let v = run_ok(bin(&home).args(["--output", "json", "run", "show", &run_id]));
    assert_eq!(v["data"]["counts"]["nodes"], 2);
}

#[test]
fn recancel_fully_converged_run_reports_no_new_changes() {
    let home = TestHome::new();
    let run_id = create(&home, "spinoff", "converged");
    add_node(&home, &run_id, "n-0001");

    // First cancel converges everything.
    let v = run_ok(bin(&home).args(["--output", "json", "run", "cancel", &run_id]));
    assert_eq!(v["data"]["already_cancelled"], false);
    assert_eq!(
        v["data"]["cancelled_nodes"].as_array().unwrap(),
        &vec![Value::from("n-0001")]
    );

    // Second cancel: already cancelled, nothing left to converge.
    let v = run_ok(bin(&home).args(["--output", "json", "run", "cancel", &run_id]));
    assert_eq!(v["data"]["already_cancelled"], true);
    assert!(v["data"]["cancelled_nodes"].as_array().unwrap().is_empty());
    assert_eq!(
        v["data"]["nodes_already_terminal"].as_array().unwrap(),
        &vec![Value::from("n-0001")]
    );
}

#[test]
fn cancel_does_not_over_report_already_terminal_node() {
    let home = TestHome::new();
    let run_id = create(&home, "spinoff", "mixed");
    add_node(&home, &run_id, "n-0001");
    add_node(&home, &run_id, "n-0002");
    // n-0001 finishes on its own (Done) before the cancel; a single lock makes
    // the cancel read it as terminal and skip it instead of over-reporting.
    node_report(&home, &run_id, "n-0001", json!({ "success": true }));

    let v = run_ok(bin(&home).args(["--output", "json", "run", "cancel", &run_id]));
    assert_eq!(v["data"]["already_cancelled"], false);
    assert_eq!(
        v["data"]["cancelled_nodes"].as_array().unwrap(),
        &vec![Value::from("n-0002")],
        "the already-Done node must not appear in cancelled_nodes"
    );
    assert_eq!(
        v["data"]["nodes_already_terminal"].as_array().unwrap(),
        &vec![Value::from("n-0001")]
    );
}

// --- per-node `run cancel --node <id>` (fan-out selectivity) ---------------

/// Read one node's status from disk via `node show`.
fn node_status_str(home: &TempDir, run_id: &str, node_id: &str) -> String {
    let v = run_ok(bin(home).args(["--output", "json", "node", "show", run_id, node_id]));
    v["data"]["status"].as_str().expect("status string").into()
}

#[test]
fn cancel_node_settles_one_child_and_leaves_run_and_siblings_live() {
    // The fan-out headline: `run cancel --node n-0002` settles ONLY that child,
    // preserves it Cancelled, and leaves the run + siblings untouched and
    // non-terminal — no `run.status` is appended while a sibling is still live.
    let home = TestHome::new();
    let run_id = create(&home, "fan-out", "batch");
    add_node(&home, &run_id, "n-0001");
    add_node(&home, &run_id, "n-0002");
    add_node(&home, &run_id, "n-0003");

    let v = run_ok(bin(&home).args([
        "--output", "json", "run", "cancel", &run_id, "--node", "n-0002", "--note", "stuck",
    ]));
    assert_eq!(v["data"]["node"], "n-0002");
    assert_eq!(v["data"]["cancelled"], true);
    assert_eq!(v["data"]["already_terminal"], false);
    assert!(
        v["data"]["run_rolled_up_to"].is_null(),
        "siblings still live → run not rolled up"
    );
    // The payload echoes the resolved full run id, not a prefix.
    assert_eq!(v["data"]["run_id"], run_id.as_str());

    // Only n-0002 settled; siblings still live; the run is NOT terminalized.
    assert_eq!(node_status_str(&home, &run_id, "n-0002"), "cancelled");
    assert_eq!(node_status_str(&home, &run_id, "n-0001"), "pending");
    assert_eq!(node_status_str(&home, &run_id, "n-0003"), "pending");
    let show = run_ok(bin(&home).args(["--output", "json", "run", "show", &run_id]));
    assert_eq!(
        show["data"]["manifest"]["status"], "pending",
        "per-node cancel must not terminalize the run while siblings are live"
    );

    // The synthesized report is the branch-preserving cancel shape.
    let node = run_ok(bin(&home).args(["--output", "json", "node", "show", &run_id, "n-0002"]));
    assert_eq!(node["data"]["last_report"]["cancelled"], true);
    assert_eq!(node["data"]["last_report"]["success"], false);
    assert_eq!(node["data"]["last_report"]["reason"], "stuck");
}

#[test]
fn cancel_node_duplicate_is_idempotent() {
    // A duplicate per-node cancel is a clean no-op: the second call reports the
    // node already-terminal and appends no second report.
    let home = TestHome::new();
    let run_id = create(&home, "fan-out", "dup");
    add_node(&home, &run_id, "n-0001");
    add_node(&home, &run_id, "n-0002");

    let v = run_ok(bin(&home).args([
        "--output", "json", "run", "cancel", &run_id, "--node", "n-0001",
    ]));
    assert_eq!(v["data"]["cancelled"], true);
    assert_eq!(v["data"]["already_terminal"], false);

    let v = run_ok(bin(&home).args([
        "--output", "json", "run", "cancel", &run_id, "--node", "n-0001",
    ]));
    assert_eq!(v["data"]["cancelled"], false);
    assert_eq!(v["data"]["already_terminal"], true);
    assert_eq!(node_status_str(&home, &run_id, "n-0001"), "cancelled");
}

#[test]
fn cancel_node_on_already_done_node_is_noop() {
    // Cancelling a node that finished on its own is an already-terminal no-op,
    // never an over-write of its success.
    let home = TestHome::new();
    let run_id = create(&home, "fan-out", "settled");
    add_node(&home, &run_id, "n-0001");
    node_report(&home, &run_id, "n-0001", json!({ "success": true }));

    let v = run_ok(bin(&home).args([
        "--output", "json", "run", "cancel", &run_id, "--node", "n-0001",
    ]));
    assert_eq!(v["data"]["cancelled"], false);
    assert_eq!(v["data"]["already_terminal"], true);
    assert_eq!(node_status_str(&home, &run_id, "n-0001"), "done");
}

#[test]
fn cancel_node_unknown_id_is_node_not_found() {
    let home = TestHome::new();
    let run_id = create(&home, "fan-out", "no-such-node");
    add_node(&home, &run_id, "n-0001");
    let (code, v) = run_fail(bin(&home).args([
        "--output", "json", "run", "cancel", &run_id, "--node", "n-0009",
    ]));
    assert_eq!(code, 1);
    assert_eq!(v["error"]["code"], "node_not_found");
    assert_eq!(v["error"]["invalid_value"], "n-0009");
}

#[test]
fn cancel_node_malformed_id_is_rejected_before_side_effects() {
    let home = TestHome::new();
    let run_id = create(&home, "fan-out", "bad-node-id");
    add_node(&home, &run_id, "n-0001");
    let (code, _v) = run_fail(bin(&home).args([
        "--output",
        "json",
        "run",
        "cancel",
        &run_id,
        "--node",
        "not-a-node",
    ]));
    assert_eq!(code, 1);
    // The live node is untouched — validation rejected the id before any lock.
    assert_eq!(node_status_str(&home, &run_id, "n-0001"), "pending");
}

#[test]
fn cancel_node_missing_run_is_run_not_found() {
    let home = TestHome::new();
    let (code, v) = run_fail(bin(&home).args([
        "--output",
        "json",
        "run",
        "cancel",
        "01000000000000000000000000",
        "--node",
        "n-0001",
    ]));
    assert_eq!(code, 1);
    assert_eq!(v["error"]["code"], "run_not_found");
}

#[test]
fn cancel_each_node_keeps_run_live_until_last_then_rolls_up() {
    // Cancelling every child one at a time keeps the run non-terminal until the
    // LAST child settles, then that cancel rolls the run up to `cancelled` in the
    // same transaction (llm-review C1 — no dependence on a live supervisor).
    let home = TestHome::new();
    let run_id = create(&home, "fan-out", "cancel-all");
    add_node(&home, &run_id, "n-0001");
    add_node(&home, &run_id, "n-0002");

    let v = run_ok(bin(&home).args([
        "--output", "json", "run", "cancel", &run_id, "--node", "n-0001",
    ]));
    assert_eq!(v["data"]["cancelled"], true);
    assert!(
        v["data"]["run_rolled_up_to"].is_null(),
        "run stays live while n-0002 is live"
    );
    let show = run_ok(bin(&home).args(["--output", "json", "run", "show", &run_id]));
    assert_eq!(show["data"]["manifest"]["status"], "pending");

    let v = run_ok(bin(&home).args([
        "--output", "json", "run", "cancel", &run_id, "--node", "n-0002",
    ]));
    assert_eq!(v["data"]["cancelled"], true);
    // The last per-node cancel terminalizes the run itself.
    assert_eq!(v["data"]["run_rolled_up_to"], "cancelled");
    let show = run_ok(bin(&home).args(["--output", "json", "run", "show", &run_id]));
    assert_eq!(show["data"]["manifest"]["status"], "cancelled");
    assert_eq!(node_status_str(&home, &run_id, "n-0001"), "cancelled");
    assert_eq!(node_status_str(&home, &run_id, "n-0002"), "cancelled");
}

/// A run recorded under a kind removed in the 0.2 cut (e.g. `code`) still
/// decodes read-only (`Kind::Unknown`) so `run list` / `run show` REPORT it
/// (ADR §D7 — the on-disk evidence corpus is never faulted or deleted), but
/// every MUTATING verb refuses it with `legacy_run_read_only`, so its manifest
/// is never rewritten (which would overwrite the legacy kind with `"unknown"`
/// and destroy provenance).
#[test]
fn legacy_removed_kind_run_is_read_only() {
    let home = TestHome::new();
    let run_id = create(&home, "spinoff", "will-be-legacied");

    // Forge a legacy on-disk run: rewrite the manifest's kind to a removed one.
    let manifest_path = home.path().join("runs").join(&run_id).join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    m["kind"] = json!("code");
    std::fs::write(&manifest_path, serde_json::to_vec(&m).unwrap()).unwrap();

    // READ paths still work — the run is reported, decoded as the read-only
    // `unknown` kind, never faulted.
    let v = run_ok(bin(&home).args(["--output", "json", "run", "list"]));
    let row = v["data"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["run_id"] == run_id)
        .expect("legacy run still listed");
    assert_eq!(row["kind"], "unknown");
    let v = run_ok(bin(&home).args(["--output", "json", "run", "show", &run_id]));
    assert_eq!(v["data"]["manifest"]["kind"], "unknown");

    // MUTATING verbs refuse it — no manifest rewrite, no corruption.
    let (code, err) = run_fail(bin(&home).args(["--output", "json", "run", "cancel", &run_id]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "legacy_run_read_only");

    let (code, err) = run_fail(bin(&home).args(["--output", "json", "run", "merge", &run_id]));
    assert_eq!(code, 1);
    assert_eq!(err["error"]["code"], "legacy_run_read_only");

    // The forged kind survived on disk — the refused mutations never rewrote it.
    let after: Value = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(
        after["kind"], "code",
        "legacy kind provenance must be preserved"
    );
}

/// Stamp a clean (`code: 0`) `worker_exit` fact plus a `worktree_path` / pid onto
/// `n-0001`'s projection — the durable shape the launcher shim's `worker.exited`
/// fold produces for a worker that finished normally. The node + run stay
/// non-terminal (no `node.report`, no terminal `run.status`). Patched directly on
/// the projection file (the `worker.exited` event kind is shim-only and not
/// routable through `event create`), mirroring `run_wait`'s
/// `backdate_manifest_updated_at`; the read paths under test only *read* the node
/// under a shared lock, so no reducer replay clobbers the patch.
fn stamp_clean_worker_exit(home: &TempDir, run_id: &str) {
    let path = home
        .path()
        .join("runs")
        .join(run_id)
        .join("nodes")
        .join("n-0001.json");
    let mut n: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read node")).expect("parse node");
    let obj = n.as_object_mut().expect("node is a JSON object");
    obj.insert(
        "worker_exit".into(),
        json!({ "code": 0, "signal": null, "at": "2026-08-15T10:00:00Z" }),
    );
    obj.insert("worktree_path".into(), json!("/tmp/wt/attention-seed"));
    obj.insert("agent_pid".into(), json!(4242));
    std::fs::write(&path, serde_json::to_vec(&n).expect("serialize node")).expect("write node");
}

/// `run show` surfaces the attention-required resume context for a run whose
/// worker exited cleanly but skipped `run merge` (design.md §2.5 / A5): the
/// `attention_required` flag plus the nested `attention` block (pending age,
/// worker pid, worktree, source branch, resume hint). NEVER terminal.
#[test]
fn show_surfaces_attention_required_run() {
    let home = TestHome::new();
    let run = create(&home, "spinoff", "attention-show");
    add_node(&home, &run, "n-0001");
    stamp_clean_worker_exit(&home, &run);

    let v = run_ok(bin(&home).args(["--output", "json", "run", "show", &run]));
    let d = &v["data"];
    assert_eq!(
        d["status"], "pending",
        "attention run must NOT be terminal: {d}"
    );
    assert_eq!(d["attention_required"], true, "must flag attention: {d}");
    let att = &d["attention"];
    assert_eq!(
        att["reason"], "worker exited cleanly without running `run merge`",
        "attention reason: {att}"
    );
    assert!(
        att["pending_age_secs"]
            .as_i64()
            .expect("pending_age_secs i64")
            >= 0,
        "pending age is non-negative: {att}"
    );
    assert!(
        att["resume_hint"]
            .as_str()
            .expect("resume_hint str")
            .contains("run salvage"),
        "resume hint names the manual finish: {att}"
    );
    // A supervisor-death stall this is NOT — no reattach hint.
    assert_eq!(
        d["stalled"], false,
        "attention is distinct from a stall: {d}"
    );

    // Text output carries an `attention:` line.
    let out = bin(&home)
        .args(["--output", "text", "run", "show", &run])
        .output()
        .expect("spawn");
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).expect("utf8");
    assert!(
        text.lines()
            .any(|l| l.starts_with("attention:") && l.contains("run salvage")),
        "text show must carry an attention line: {text}"
    );
}

/// `run list` flags an attention-required run with `attention_required: true` and
/// marks the plain-text row `(attention)`, distinct from `(stillborn)` /
/// `(stalled)`. A run still running (no worker exit) is not flagged.
#[test]
fn list_flags_attention_required_run() {
    let home = TestHome::new();
    let att = create(&home, "spinoff", "attention");
    add_node(&home, &att, "n-0001");
    stamp_clean_worker_exit(&home, &att);
    // A control run with a node but no worker exit — still working, not attention.
    let working = create(&home, "spinoff", "working");
    add_node(&home, &working, "n-0001");

    let v = run_ok(bin(&home).args(["--output", "json", "run", "list"]));
    let runs = v["data"]["runs"].as_array().expect("runs array");
    let row = |id: &str| {
        runs.iter()
            .find(|r| r["run_id"] == id)
            .unwrap_or_else(|| panic!("run {id} missing from list"))
    };
    let a = row(&att);
    assert_eq!(
        a["attention_required"], true,
        "clean-exit run is attention: {a}"
    );
    assert_eq!(
        a["status"], "pending",
        "attention run stays non-terminal: {a}"
    );
    assert!(a["attention"].is_object(), "attention block present: {a}");
    let w = row(&working);
    assert_eq!(
        w["attention_required"], false,
        "a still-working run (no worker exit) is not attention: {w}"
    );
    assert!(w.get("attention").is_none(), "no attention block: {w}");

    // Plain-text marker.
    let out = bin(&home)
        .args(["--output", "text", "run", "list"])
        .output()
        .expect("spawn");
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).expect("utf8");
    let att_line = text
        .lines()
        .find(|l| l.contains(&att))
        .expect("attention run present in text output");
    assert!(
        att_line.contains("pending (attention)"),
        "text row must mark the run attention: {att_line}"
    );
}

/// Backdate a run manifest's `updated_at` to `minutes_ago` before now (mirrors
/// `run_wait`'s helper) so the orphaned-stall shape trips.
fn backdate_manifest_updated_at(home: &TempDir, run_id: &str, minutes_ago: i64) {
    let path = home.path().join("runs").join(run_id).join("manifest.json");
    let mut m: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read manifest")).expect("parse");
    let ts = (chrono::Utc::now() - chrono::Duration::minutes(minutes_ago)).to_rfc3339();
    m.as_object_mut()
        .unwrap()
        .insert("updated_at".into(), Value::String(ts));
    std::fs::write(&path, serde_json::to_vec(&m).expect("serialize")).expect("write manifest");
}

/// Precedence (design.md §2.5): a run whose worker exited cleanly AND whose
/// supervisor died mid-run (the orphaned shape) reports `attention_required`,
/// NOT `stalled`, on both `run show` and `run list`. The manual finish is the
/// fix, never `run reattach`.
#[test]
fn attention_beats_orphaned_stall_in_show_and_list() {
    let home = TestHome::new();
    let run = create(&home, "spinoff", "attention-vs-orphan");
    add_node(&home, &run, "n-0001");
    stamp_clean_worker_exit(&home, &run);
    // Age the manifest past the orphan grace so, absent the clean exit, this would
    // classify as an orphaned stall (no supervisor pid → dead).
    backdate_manifest_updated_at(&home, &run, 30);

    let show = run_ok(bin(&home).args(["--output", "json", "run", "show", &run]));
    assert_eq!(
        show["data"]["attention_required"], true,
        "clean exit wins over orphaned stall: {}",
        show["data"]
    );
    assert_eq!(
        show["data"]["stalled"], false,
        "attention must suppress the stall verdict in show: {}",
        show["data"]
    );

    let list = run_ok(
        bin(&home)
            .env("TASKFLEET_STILLBORN_LIST_GRACE_SECS", "0")
            .args(["--output", "json", "run", "list"]),
    );
    let row = list["data"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["run_id"] == run)
        .unwrap();
    assert_eq!(row["attention_required"], true);
    assert_eq!(
        row["stalled"], false,
        "attention must suppress the stall verdict in list: {row}"
    );
}

/// Fan-out gate (design.md §2.5): a multi-node run is NOT flagged
/// `attention_required` off `n-0001` alone — per-node attention is the delegated
/// `per-node-run` follow-up. Prevents false-flagging the whole run.
#[test]
fn multi_node_run_is_not_flagged_attention_off_n0001() {
    let home = TestHome::new();
    let run = create(&home, "fan-out", "fanout");
    add_node(&home, &run, "n-0001");
    add_node(&home, &run, "n-0002"); // node_count == 2
    stamp_clean_worker_exit(&home, &run); // n-0001 exits clean

    let show = run_ok(bin(&home).args(["--output", "json", "run", "show", &run]));
    assert_eq!(
        show["data"]["attention_required"], false,
        "a multi-node run must not be flagged attention off n-0001: {}",
        show["data"]
    );
    assert!(show["data"].get("attention").is_none());

    let list = run_ok(bin(&home).args(["--output", "json", "run", "list"]));
    let row = list["data"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["run_id"] == run)
        .unwrap();
    assert_eq!(row["attention_required"], false, "list: {row}");
}
