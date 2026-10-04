//! Integration tests for the `doctor` subcommand (AGENTS-AI-FIRST-CLI
//! §18).
//!
//! Locks the contract an AI caller depends on: the §18 structured shape
//! under `--output json`, the streaming jsonl form, per-check `id`s and
//! statuses, exit-code semantics (any `fail` → 1; warnings never flip
//! it), the `skill.sync` drift WARN, the `schema.runs` corruption FAIL,
//! and the `--fix` / `--fix --dry-run` behaviour.
//!
//! Every test sandboxes `HOME`, `TASKFLEET_HOME`, and `PATH` into
//! tempdirs so it never touches the developer's real install and the
//! dependency checks are deterministic regardless of what is on the CI
//! machine's PATH.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Build a fake `PATH` dir containing stub executables for every binary
/// `doctor`'s `dep.*` checks look for, so those checks pass deterministically.
fn fake_path_dir() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for bin in ["tmux", "git", "workmux", "issuectl"] {
        let p = dir.path().join(bin);
        std::fs::write(&p, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    dir
}

struct Env {
    home: TempDir,
    orch: PathBuf,
    path: TempDir,
}

fn setup() -> Env {
    let home = tempfile::tempdir().expect("tempdir");
    let orch = home.path().join("orch");
    // Create the taskfleet home so `config.home` reports OK.
    std::fs::create_dir_all(&orch).unwrap();
    Env {
        home,
        orch,
        path: fake_path_dir(),
    }
}

fn bin(env: &Env) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    cmd.env("HOME", env.home.path());
    cmd.env("TASKFLEET_HOME", &env.orch);
    cmd.env("PATH", env.path.path());
    cmd.env_remove("GIT_BIN");
    cmd
}

/// Install a hand-written skill SKILL.md at the default claude path with
/// the given `cli_version`, so `skill.sync` sees it on disk.
fn install_skill(env: &Env, name: &str, cli_version: &str) {
    let dir = env.home.path().join(".claude/skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!(
            "---\nname: {name}\ndescription: test\ncli_version: \"{cli_version}\"\nschema_version: 1\n---\nbody\n"
        ),
    )
    .unwrap();
}

fn find_check<'a>(checks: &'a [Value], id: &str) -> &'a Value {
    checks
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("no check with id {id}; got {checks:?}"))
}

fn running_binary_commit(env: &Env) -> String {
    let out = bin(env)
        .args(["--output", "json", "version"])
        .output()
        .expect("spawn version");
    assert!(out.status.success(), "version failed: {out:?}");
    let value: Value = serde_json::from_slice(&out.stdout).expect("version json");
    value["data"]["commit"].as_str().unwrap().to_owned()
}

fn fake_taskfleet_checkout(env: &Env) -> PathBuf {
    let root = env.home.path().join("source");
    std::fs::create_dir_all(root.join("crates/taskfleet")).unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    std::fs::write(
        root.join("crates/taskfleet/Cargo.toml"),
        "[package]\nname = \"taskfleet\"\nversion = \"0.0.0\"\n",
    )
    .unwrap();
    root
}

fn stub_git_head(env: &Env, root: &Path, head: &str) {
    let script = format!(
        "#!/bin/sh\nif [ \"$3\" = rev-parse ] && [ \"$4\" = --show-toplevel ]; then\n  printf '%s\\n' '{}'\nelif [ \"$3\" = rev-parse ] && [ \"$4\" = HEAD ]; then\n  printf '%s\\n' '{}'\nelse\n  exit 1\nfi\n",
        root.display(), head
    );
    let path = env.path.path().join("git");
    std::fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn binary_commit_matches_applicable_repository_head() {
    let env = setup();
    let root = fake_taskfleet_checkout(&env);
    let commit = running_binary_commit(&env);
    assert_eq!(
        commit.len(),
        40,
        "build commit must be a full SHA: {commit}"
    );
    assert!(
        commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "build commit must be hexadecimal: {commit}"
    );
    stub_git_head(&env, &root, &commit);

    let out = bin(&env)
        .current_dir(&root)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn doctor");
    assert!(out.status.success(), "matching commit should pass: {out:?}");
    let value: Value = serde_json::from_slice(&out.stdout).expect("doctor json");
    let check = find_check(value["data"]["checks"].as_array().unwrap(), "binary.commit");
    assert_eq!(check["status"], "ok");
    assert_eq!(check["details"]["binary_commit"], commit);
    assert_eq!(check["details"]["repository_head"], commit);
    assert_eq!(check["details"]["comparison"], "match");
}

#[test]
fn binary_commit_mismatch_warns_without_failing() {
    let env = setup();
    let root = fake_taskfleet_checkout(&env);
    let head = "0000000000000000000000000000000000000000";
    stub_git_head(&env, &root, head);

    let out = bin(&env)
        .current_dir(&root)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn doctor");
    assert!(
        out.status.success(),
        "mismatch warning must not fail: {out:?}"
    );
    let value: Value = serde_json::from_slice(&out.stdout).expect("doctor json");
    let check = find_check(value["data"]["checks"].as_array().unwrap(), "binary.commit");
    assert_eq!(check["status"], "warn");
    assert_eq!(check["details"]["repository_head"], head);
    assert_eq!(check["details"]["comparison"], "mismatch");
    assert!(check["message"].as_str().unwrap().contains(head));

    let text = bin(&env)
        .current_dir(&root)
        .args(["--output", "text", "doctor"])
        .output()
        .expect("spawn text doctor");
    assert!(text.status.success());
    let stdout = String::from_utf8(text.stdout).unwrap();
    assert!(stdout.contains("WARN binary.commit"));
    assert!(stdout.contains("differs from repository HEAD"));
}

#[test]
fn binary_commit_is_disclosed_when_repository_comparison_is_not_applicable() {
    let env = setup();
    let commit = running_binary_commit(&env);
    let out = bin(&env)
        .current_dir(env.home.path())
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn doctor");
    assert!(out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).expect("doctor json");
    let check = find_check(value["data"]["checks"].as_array().unwrap(), "binary.commit");
    assert_eq!(check["status"], "ok");
    assert_eq!(check["details"]["binary_commit"], commit);
    assert_eq!(check["details"]["repository_head"], Value::Null);
    assert_eq!(check["details"]["comparison"], "not_applicable");
}

#[test]
fn binary_commit_is_not_compared_in_a_foreign_git_repository() {
    let env = setup();
    let root = env.home.path().join("foreign-source");
    std::fs::create_dir_all(root.join("crates/taskfleet")).unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    std::fs::write(
        root.join("crates/taskfleet/Cargo.toml"),
        "[package]\nname = \"another-cli\"\nversion = \"0.0.0\"\n",
    )
    .unwrap();
    stub_git_head(&env, &root, "1111111111111111111111111111111111111111");

    let out = bin(&env)
        .current_dir(&root)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn doctor");
    assert!(out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).expect("doctor json");
    let check = find_check(value["data"]["checks"].as_array().unwrap(), "binary.commit");
    assert_eq!(check["status"], "ok");
    assert_eq!(check["details"]["comparison"], "not_applicable");
}

#[test]
fn binary_commit_reports_unavailable_when_applicable_head_cannot_be_read() {
    let env = setup();
    let root = fake_taskfleet_checkout(&env);
    let script = format!(
        "#!/bin/sh\nif [ \"$3\" = rev-parse ] && [ \"$4\" = --show-toplevel ]; then\n  printf '%s\\n' '{}'\nelse\n  exit 1\nfi\n",
        root.display()
    );
    let git = env.path.path().join("git");
    std::fs::write(&git, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let out = bin(&env)
        .current_dir(&root)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn doctor");
    assert!(out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).expect("doctor json");
    let check = find_check(value["data"]["checks"].as_array().unwrap(), "binary.commit");
    assert_eq!(check["status"], "ok");
    assert_eq!(check["details"]["repository_head"], Value::Null);
    assert_eq!(check["details"]["comparison"], "unavailable");
}

#[test]
fn healthy_install_passes_with_exit_zero() {
    let env = setup();
    let out = bin(&env)
        .args(["--output", "text", "doctor"])
        .output()
        .expect("spawn");
    // No FAILs (deps stubbed, home writable, no broken runs) → exit 0,
    // even though skill.sync warns about not-installed skills.
    assert!(out.status.success(), "expected exit 0; got {out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("config.home"));
    assert!(stdout.contains("dep.tmux"));
    assert!(stdout.contains("summary:"));
}

#[test]
fn json_emits_section18_bundled_shape() {
    let env = setup();
    let out = bin(&env)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["schema_version"], 1);
    let checks = v["data"]["checks"].as_array().expect("checks array");
    assert_ne!(checks.len(), 0);
    // Each check has the §18 fields.
    for c in checks {
        assert!(c["id"].is_string());
        assert!(c["status"].is_string());
        assert!(c["message"].is_string());
    }
    let summary = &v["data"]["summary"];
    assert!(summary["ok"].is_number());
    assert!(summary["warn"].is_number());
    assert!(summary["fail"].is_number());
    // config.home is OK in a freshly-created home.
    assert_eq!(find_check(checks, "config.home")["status"], "ok");
    assert_eq!(find_check(checks, "dep.git")["status"], "ok");
}

#[test]
fn jsonl_streams_one_check_per_line_then_summary_event() {
    let env = setup();
    let out = bin(&env).arg("doctor").output().expect("spawn");
    let stdout = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert!(lines.len() >= 2);
    // Every line is self-describing: it carries schema_version and an
    // event discriminator so a streaming consumer can version-check and
    // route each record independently.
    let last: Value = serde_json::from_str(lines.last().unwrap()).expect("summary json");
    assert_eq!(last["event"], "summary");
    assert_eq!(last["schema_version"], 1);
    assert!(last["ok"].is_number());
    let first: Value = serde_json::from_str(lines[0]).expect("check json");
    assert_eq!(first["event"], "check");
    assert_eq!(first["schema_version"], 1);
    assert!(first["id"].is_string());
}

#[test]
fn broken_manifest_fails_schema_check_and_exits_one() {
    let env = setup();
    let run_dir = env.orch.join("runs").join("01jxbad0000000000000000000");
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(run_dir.join("manifest.json"), "{ this is not json").unwrap();

    let out = bin(&env)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(1), "any fail → exit 1");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let checks = v["data"]["checks"].as_array().unwrap();
    let c = find_check(checks, "schema.runs.01jxbad0000000000000000000");
    assert_eq!(c["status"], "fail");
    assert!(c["fix_suggestion"]
        .as_str()
        .unwrap()
        .contains("run cancel 01jxbad0000000000000000000"));
    assert_eq!(v["data"]["summary"]["fail"], 1);
}

#[test]
fn invalid_run_directory_name_is_surfaced_as_warn() {
    let env = setup();
    // A directory under runs/ whose name isn't a valid run id must not be
    // silently ignored — the doctor surfaces it so a failed migration or
    // foreign dir is visible rather than quarantined.
    let run_dir = env.orch.join("runs").join("badname");
    std::fs::create_dir_all(&run_dir).unwrap();

    let out = bin(&env)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let checks = v["data"]["checks"].as_array().unwrap();
    let c = find_check(checks, "schema.runs.badname");
    assert_eq!(c["status"], "warn");
    assert!(c["message"]
        .as_str()
        .unwrap()
        .contains("not a valid run directory"));
}

#[test]
fn skill_drift_warns_with_install_suggestion() {
    let env = setup();
    install_skill(&env, "taskfleet-run-overview", "0.0.0");

    let out = bin(&env)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let checks = v["data"]["checks"].as_array().unwrap();
    let c = find_check(checks, "skill.sync.taskfleet-run-overview");
    assert_eq!(c["status"], "warn");
    assert!(c["message"].as_str().unwrap().contains("0.0.0"));
    assert!(c["fix_suggestion"]
        .as_str()
        .unwrap()
        .contains("skill install taskfleet-run-overview --force"));
    // A drift WARN never flips the exit code.
    assert!(out.status.success(), "warnings must not fail the run");
}

#[test]
fn skill_orphan_warns_only_for_managed_deregistered_dir() {
    let env = setup();
    // A managed-but-de-registered skill dir (carries the marker, not in
    // the catalog) → WARN. A user's own same-shaped dir WITHOUT the marker
    // must not be flagged.
    let managed = env.home.path().join(".claude/skills/gone-skill");
    std::fs::create_dir_all(&managed).unwrap();
    let managed_body = "---\nname: gone-skill\n---\n";
    std::fs::write(managed.join("SKILL.md"), managed_body).unwrap();
    std::fs::write(
        managed.join(".taskfleet-managed"),
        format!(
            "managed-by: taskfleet\ncli_version: 9.9.9\nskill_name: gone-skill\nsha256: {}\n",
            sha256_hex(managed_body.as_bytes())
        ),
    )
    .unwrap();

    let user = env.home.path().join(".claude/skills/my-own-skill");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("SKILL.md"), "---\nname: my-own-skill\n---\n").unwrap();

    let out = bin(&env)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let checks = v["data"]["checks"].as_array().unwrap();
    let c = find_check(checks, "skill.orphan.gone-skill");
    assert_eq!(c["status"], "warn");
    assert!(c["message"].as_str().unwrap().contains("de-registered"));
    assert!(
        !checks
            .iter()
            .any(|c| c["id"] == "skill.orphan.my-own-skill"),
        "unmanaged user skill must not be flagged as an orphan"
    );
    // An orphan WARN never flips the exit code.
    assert!(out.status.success(), "warnings must not fail the run");
}

/// Seed a pi mirror `SKILL.md` at `~/.pi/agent/skills/<name>/` with the given
/// `cli_version`, plus a provenance record entry naming it managed. The recorded
/// hash is arbitrary — the drift/orphan checks exercised here never require a
/// hash match (only the Equal-version OK arm does, which is covered by the
/// install-then-doctor test).
fn seed_pi_mirror(env: &Env, name: &str, cli_version: &str) {
    let dir = env.home.path().join(".pi/agent/skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!(
            "---\nname: {name}\ndescription: test\ncli_version: \"{cli_version}\"\nschema_version: 1\n---\nbody\n"
        ),
    )
    .unwrap();

    let record = env.orch.join("state").join("pi-installed-skills.json");
    std::fs::create_dir_all(record.parent().unwrap()).unwrap();
    let mut prov: Value = std::fs::read_to_string(&record)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({ "schema_version": 1, "skills": {} }));
    prov["skills"][name] =
        serde_json::json!({ "sha256": "0".repeat(64), "cli_version": cli_version });
    std::fs::write(&record, serde_json::to_string_pretty(&prov).unwrap()).unwrap();
}

#[test]
fn pi_sync_drift_warns_with_install_suggestion() {
    // A recorded pi mirror older than the binary → WARN keyed `skill.sync.
    // <name>.pi`, with the same forced-reinstall suggestion as the claude check.
    let env = setup();
    seed_pi_mirror(&env, "taskfleet-run-overview", "0.0.0");

    let out = bin(&env)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let checks = v["data"]["checks"].as_array().unwrap();
    let c = find_check(checks, "skill.sync.taskfleet-run-overview.pi");
    assert_eq!(c["status"], "warn");
    assert!(c["message"].as_str().unwrap().contains("0.0.0"));
    assert!(c["fix_suggestion"]
        .as_str()
        .unwrap()
        .contains("skill install taskfleet-run-overview --force"));
    assert!(out.status.success(), "warnings must not fail the run");
}

#[test]
fn pi_orphan_warns_for_managed_deregistered_mirror() {
    // A pi mirror the provenance record names but the catalog no longer ships →
    // WARN keyed `skill.orphan.<name>.pi`. A pi mirror the record does NOT name
    // (a user's own pi skill) is never recorded, hence never flagged.
    let env = setup();
    seed_pi_mirror(&env, "gone-skill", "0.0.1");
    // A user's own pi skill: on disk but NOT in the provenance record.
    let user = env.home.path().join(".pi/agent/skills/my-pi-skill");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("SKILL.md"), "---\nname: my-pi-skill\n---\n").unwrap();

    let out = bin(&env)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let checks = v["data"]["checks"].as_array().unwrap();
    let c = find_check(checks, "skill.orphan.gone-skill.pi");
    assert_eq!(c["status"], "warn");
    assert!(c["message"].as_str().unwrap().contains("de-registered"));
    assert!(
        !checks
            .iter()
            .any(|c| c["id"] == "skill.orphan.my-pi-skill.pi"),
        "an unrecorded user pi skill must not be flagged as an orphan"
    );
    assert!(out.status.success(), "warnings must not fail the run");
}

#[test]
fn pi_checks_absent_without_provenance_record() {
    // No provenance record (a host that never dual-homed into pi) → no pi checks
    // at all, keeping a pi-less tree free of pi noise.
    let env = setup();
    let out = bin(&env)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let checks = v["data"]["checks"].as_array().unwrap();
    assert!(
        !checks
            .iter()
            .any(|c| c["id"].as_str().unwrap().split('.').next_back() == Some("pi")),
        "no pi checks should be emitted without a provenance record"
    );
}

#[test]
fn pi_in_sync_after_install_is_ok() {
    // End-to-end: a real `skill install` writes the pi mirror AND its provenance
    // (correct content hash), so `doctor` reports the pi mirror in sync — proving
    // the OK arm's hash-match path against real bytes.
    let env = setup();
    assert!(bin(&env)
        .args(["skill", "install", "taskfleet-run-overview"])
        .output()
        .expect("spawn")
        .status
        .success());

    let out = bin(&env)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let checks = v["data"]["checks"].as_array().unwrap();
    let c = find_check(checks, "skill.sync.taskfleet-run-overview.pi");
    assert_eq!(c["status"], "ok", "pi mirror should be in sync: {c}");
}

#[test]
fn pi_orphan_companion_flagged_then_cleared_by_force() {
    // A pi companion the record tracks that the binary no longer bundles is
    // flagged as skill.orphan.<name>.pi.<file>; a --force reinstall reconciles it
    // (removes file + record entry) so the warning clears — the loop is fixable
    // (review finding F1).
    let env = setup();
    assert!(bin(&env)
        .args(["skill", "install", "stint-start"])
        .output()
        .expect("spawn")
        .status
        .success());
    let pi_dir = env.home.path().join(".pi/agent/skills/stint-start");
    let record_path = env.orch.join("state").join("pi-installed-skills.json");

    // Give the orphan matching bytes/hash so --force recognises it as our copy.
    let old_bytes = b"stale\n";
    std::fs::write(pi_dir.join("OLD-COMPANION.md"), old_bytes).unwrap();
    let mut prov: Value = serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    prov["skills"]["stint-start"]["files"]["OLD-COMPANION.md"] = serde_json::json!({
        "sha256": sha256_hex(old_bytes),
        "kind": "companion"
    });
    std::fs::write(&record_path, serde_json::to_string_pretty(&prov).unwrap()).unwrap();

    // doctor flags the orphan companion.
    let out = bin(&env)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let checks = v["data"]["checks"].as_array().unwrap();
    let c = find_check(checks, "skill.orphan.stint-start.pi.OLD-COMPANION.md");
    assert_eq!(
        c["status"], "warn",
        "orphan pi companion must be flagged: {c}"
    );

    // A --force reinstall clears it.
    assert!(bin(&env)
        .args(["skill", "install", "stint-start", "--force"])
        .output()
        .expect("spawn")
        .status
        .success());
    let out2 = bin(&env)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn");
    let v2: Value = serde_json::from_slice(&out2.stdout).expect("json");
    let checks2 = v2["data"]["checks"].as_array().unwrap();
    assert!(
        !checks2
            .iter()
            .any(|c| c["id"] == "skill.orphan.stint-start.pi.OLD-COMPANION.md"),
        "the orphan warning must be gone after a --force reinstall"
    );
}

#[test]
fn fix_reinstalls_drifted_skill() {
    let env = setup();
    install_skill(&env, "taskfleet-run-overview", "0.0.0");
    let on_disk = env
        .home
        .path()
        .join(".claude/skills/taskfleet-run-overview/SKILL.md");

    let out = bin(&env)
        .args(["--output", "json", "doctor", "--fix"])
        .output()
        .expect("spawn");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let fixes = v["data"]["fixes_applied"]
        .as_array()
        .expect("fixes_applied");
    let f = fixes
        .iter()
        .find(|f| f["check_id"] == "skill.sync.taskfleet-run-overview")
        .expect("install fix applied");
    assert_eq!(f["applied"], true);

    // The on-disk skill now matches the running binary's version.
    let after = std::fs::read_to_string(&on_disk).unwrap();
    assert!(
        after.contains(&format!("cli_version: \"{}\"", env!("CARGO_PKG_VERSION"))),
        "skill was not re-installed: {after}"
    );
}

#[test]
fn fix_dry_run_emits_plan_and_changes_nothing() {
    let env = setup();
    install_skill(&env, "taskfleet-run-overview", "0.0.0");
    let on_disk = env
        .home
        .path()
        .join(".claude/skills/taskfleet-run-overview/SKILL.md");

    let out = bin(&env)
        .args(["--output", "json", "doctor", "--fix", "--dry-run"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "dry-run exits 0");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["dry_run"], true);
    let would = v["would"].as_array().expect("would array");
    assert!(would
        .iter()
        .any(|w| w["target"] == "taskfleet-run-overview" && w["action"] == "install"));

    // Nothing was applied — the drifted file is untouched.
    let after = std::fs::read_to_string(&on_disk).unwrap();
    assert!(after.contains("0.0.0"), "dry-run must not mutate: {after}");
}

#[test]
fn dry_run_without_fix_is_rejected() {
    let env = setup();
    let out = bin(&env)
        .args(["doctor", "--dry-run"])
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(1));
    let err: Value = serde_json::from_slice(&out.stderr).expect("err envelope");
    assert_eq!(err["error"]["code"], "invalid_arguments");
}

#[test]
fn dead_supervisor_pid_warns() {
    let env = setup();
    let run_dir = env.orch.join("runs").join("01jxdead000000000000000000");
    std::fs::create_dir_all(&run_dir).unwrap();
    // PID 2^31-1 is effectively never live.
    write_minimal_manifest(&run_dir, "01jxdead000000000000000000");
    std::fs::write(run_dir.join("supervisor.pid"), "2147483647").unwrap();

    let out = bin(&env)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let checks = v["data"]["checks"].as_array().unwrap();
    let c = find_check(checks, "data.orphan-supervisor.01jxdead000000000000000000");
    assert_eq!(c["status"], "warn");
    assert!(c["fix_suggestion"]
        .as_str()
        .unwrap()
        .contains("run reattach 01jxdead000000000000000000"));
}

#[test]
fn corrupt_supervisor_pid_warns() {
    let env = setup();
    let run_dir = env.orch.join("runs").join("01jxcrpt000000000000000000");
    std::fs::create_dir_all(&run_dir).unwrap();
    write_minimal_manifest(&run_dir, "01jxcrpt000000000000000000");
    std::fs::write(run_dir.join("supervisor.pid"), "not-a-pid").unwrap();

    let out = bin(&env)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let checks = v["data"]["checks"].as_array().unwrap();
    let c = find_check(checks, "data.orphan-supervisor.01jxcrpt000000000000000000");
    assert_eq!(c["status"], "warn");
    assert!(c["message"]
        .as_str()
        .unwrap()
        .contains("unreadable/unparseable"));
}

#[test]
fn fix_removes_stale_dead_supervisor_pid() {
    let env = setup();
    let run_dir = env.orch.join("runs").join("01jxstp0000000000000000000");
    std::fs::create_dir_all(&run_dir).unwrap();
    write_minimal_manifest(&run_dir, "01jxstp0000000000000000000");
    let pid_path = run_dir.join("supervisor.pid");
    std::fs::write(&pid_path, "2147483647").unwrap();
    // Backdate the mtime well past the 24h staleness threshold so the
    // safe fix is offered. `touch -t` is portable across macOS/Linux.
    let ok = Command::new("touch")
        .args(["-t", "202001010000"])
        .arg(&pid_path)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "touch -t failed; cannot backdate pid file");

    let out = bin(&env)
        .args(["--output", "json", "doctor", "--fix"])
        .output()
        .expect("spawn");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let fixes = v["data"]["fixes_applied"]
        .as_array()
        .expect("fixes_applied");
    let f = fixes
        .iter()
        .find(|f| f["check_id"] == "data.orphan-supervisor.01jxstp0000000000000000000")
        .expect("stale pid fix applied");
    assert_eq!(f["applied"], true, "fix outcome: {f:?}");
    assert!(!pid_path.exists(), "stale pid file should be removed");
}

/// Install a bundled skill (and its companion resources) through the real
/// `skill install` path, so companion checks see byte-identical on-disk
/// copies of what the binary ships.
fn install_bundled(env: &Env, name: &str) {
    let out = bin(env)
        .args([
            "--output", "json", "skill", "install", name, "--agent", "claude",
        ])
        .output()
        .expect("spawn install");
    assert!(out.status.success(), "install {name} failed: {out:?}");
}

// ---- codex flat-layout coverage ----

const CODEX_PROMPT_REL: &str = ".codex/prompts/stint-start.md";
const CODEX_MARKER_REL: &str = ".codex/prompts/_shared/.taskfleet-managed";
const CODEX_SKILL_ID: &str = "skill.sync.codex.stint-start";

/// Install a bundled skill to the codex flat layout through the real
/// `skill install --agent codex` path, so codex checks see byte-identical
/// on-disk copies and the shared provenance marker exists.
fn install_bundled_codex(env: &Env, name: &str) {
    let out = bin(env)
        .args([
            "--output", "json", "skill", "install", name, "--agent", "codex",
        ])
        .output()
        .expect("spawn codex install");
    assert!(out.status.success(), "codex install {name} failed: {out:?}");
}

#[test]
fn codex_checks_absent_without_marker() {
    // A claude-only install (no codex marker) must emit NO codex checks, so a
    // claude-primary tree stays free of spurious codex warnings.
    let env = setup();
    install_bundled(&env, "stint-start");

    let out = bin(&env)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let checks = v["data"]["checks"].as_array().unwrap();
    assert!(
        !checks.iter().any(
            |c| c["id"].as_str().unwrap().starts_with("skill.sync.codex")
                || c["id"].as_str().unwrap().starts_with("skill.orphan.codex")
        ),
        "codex checks must be gated on the codex provenance marker"
    );
}

#[test]
fn codex_skill_drift_warns() {
    let env = setup();
    install_bundled_codex(&env, "stint-start");
    // Roll the codex prompt back to an older cli_version (frontmatter only —
    // the version check reads it directly).
    std::fs::write(
        env.home.path().join(CODEX_PROMPT_REL),
        "---\nname: stint-start\ncli_version: \"0.0.0\"\nschema_version: 1\n---\nbody\n",
    )
    .unwrap();

    let out = bin(&env)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let checks = v["data"]["checks"].as_array().unwrap();
    let c = find_check(checks, CODEX_SKILL_ID);
    assert_eq!(c["status"], "warn");
    assert!(c["message"].as_str().unwrap().contains("0.0.0"));
    assert!(c["fix_suggestion"]
        .as_str()
        .unwrap()
        .contains("--agent codex --force"));
    // Codex drift carries no autonomous fix (InstallSkill is claude-scoped).
    let out2 = bin(&env)
        .args(["--output", "json", "doctor", "--fix", "--dry-run"])
        .output()
        .expect("spawn");
    let v2: Value = serde_json::from_slice(&out2.stdout).expect("json");
    let would = v2["would"].as_array().expect("would array");
    assert!(
        !would.iter().any(|w| w["check_id"] == CODEX_SKILL_ID),
        "codex drift must not be an autonomous fix: {would:?}"
    );
    assert!(out.status.success());
}

#[test]
fn codex_orphan_prompt_and_companion_warn() {
    let env = setup();
    install_bundled_codex(&env, "stint-start");
    let marker = env.home.path().join(CODEX_MARKER_REL);
    let prompts = env.home.path().join(".codex/prompts");
    let shared = prompts.join("_shared");

    // Simulate a prior binary: a de-registered prompt + shared companion,
    // both recorded in the marker and lingering on disk.
    std::fs::write(prompts.join("gone-skill.md"), "stale\n").unwrap();
    std::fs::write(shared.join("OLD-SHARED.md"), "stale\n").unwrap();
    let mut body = std::fs::read_to_string(&marker).unwrap();
    body.push_str("prompt: gone-skill\n");
    body.push_str("companion: OLD-SHARED.md\n");
    std::fs::write(&marker, body).unwrap();
    // A user's own prompt the marker never recorded must NOT be flagged.
    std::fs::write(prompts.join("my-own.md"), "mine\n").unwrap();

    let out = bin(&env)
        .args(["--output", "json", "doctor"])
        .output()
        .expect("spawn");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let checks = v["data"]["checks"].as_array().unwrap();

    let prompt_orphan = find_check(checks, "skill.orphan.codex.gone-skill");
    assert_eq!(prompt_orphan["status"], "warn");
    assert!(prompt_orphan["message"]
        .as_str()
        .unwrap()
        .contains("de-registered"));
    let companion_orphan = find_check(checks, "skill.orphan.codex._shared.OLD-SHARED.md");
    assert_eq!(companion_orphan["status"], "warn");

    // The still-bundled skill stays OK; the user's own prompt is never flagged.
    assert_eq!(find_check(checks, CODEX_SKILL_ID)["status"], "ok");
    assert!(
        !checks
            .iter()
            .any(|c| c["id"] == "skill.orphan.codex.my-own"),
        "an unrecorded user prompt must not be flagged as a codex orphan"
    );
    // Orphan WARNs never flip the exit code.
    assert!(out.status.success());
}

/// Write a schema-valid manifest so the orphan-supervisor test exercises
/// only the data-integrity path (and schema.runs stays OK for that run).
fn write_minimal_manifest(run_dir: &Path, run_id: &str) {
    let manifest = serde_json::json!({
        "schema_version": 1,
        "run_id": run_id,
        "kind": "spinoff",
        "lifecycle": "autonomous",
        "title": "t",
        "status": "running",
        "created_at": "2026-06-27T00:00:00Z",
        "updated_at": "2026-06-27T00:00:00Z",
        "source_repo": null,
        "source_branch": null,
        "worktree_root": null,
        "node_count": 0,
        "open_discussions": 0,
        "pending_spinoffs": 0,
        "parent_run_id": null,
        "parent_node_id": null
    });
    std::fs::write(
        run_dir.join("manifest.json"),
        serde_json::to_string(&manifest).unwrap(),
    )
    .unwrap();
}
