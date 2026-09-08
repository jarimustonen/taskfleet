//! Integration tests for the `skill` subcommand.
//!
//! Locks the AGENTS-AI-FIRST-CLI §15 contract: list shape, `refused_overwrite`
//! error envelope on exit 2, `--force` recovery, the install-all (no-name)
//! form, and `--agent`/`--dest` mutual-exclusion rules. The shipped skill
//! catalog (names + non-empty descriptions) is pinned here so accidental
//! frontmatter breakage in `crates/taskfleet/skills/` is caught at CI time.
//!
//! Every test sets `TASKFLEET_HOME` (and `HOME` where the install
//! path resolves it) to a tempdir so we never mutate the developer's real
//! `~/.taskfleet/` or `~/.claude/` directories under CI or local
//! `cargo test`.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn bin(home: &TempDir) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_taskfleet"));
    // Sandbox log writes and any HOME-derived install paths into the
    // tempdir so test runs never touch the real `~/.taskfleet/` or
    // `~/.claude/`.
    cmd.env("TASKFLEET_HOME", home.path());
    cmd.env("HOME", home.path());
    cmd
}

fn mk_home() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

#[test]
fn skill_list_json_pins_catalog_shape() {
    let home = mk_home();
    let out = bin(&home)
        .args(["skill", "list", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "exit: {:?}", out.status);
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["schema_version"], 1);
    assert_eq!(
        v["supported_agents"],
        serde_json::json!(["claude", "pi", "codex"])
    );
    assert_eq!(v["skills"], v["data"]["skills"]);
    assert_eq!(v["install"], v["data"]["install"]);
    assert_eq!(
        v["data"]["supported_agents"],
        serde_json::json!(["claude", "pi", "codex"])
    );
    let install = &v["data"]["install"];
    assert_eq!(install["selection_flag"], "--agent");
    assert_eq!(install["default"], "all");
    assert_eq!(
        install["accepted_values"],
        serde_json::json!(["claude", "pi", "codex", "all"])
    );
    assert_eq!(install["target_flag"], "--target");
    assert_eq!(install["dry_run_flag"], "--dry-run");
    assert_eq!(install["force_flag"], "--force");
    assert_eq!(install["interactive"], false);
    assert_eq!(install["no_clobber_default"], true);
    assert_eq!(install["overwrite_requires_force"], true);
    assert_eq!(
        install["layouts"],
        serde_json::json!([
            {"agent":"claude","path":".claude/skills/<name>/...","form":"agent-skill-tree"},
            {"agent":"pi","path":".pi/agent/skills/<name>/...","form":"agent-skill-tree"},
            {"agent":"codex","path":".codex/prompts/<name>.md","form":"self-contained-prompt"}
        ])
    );
    let skills = v["data"]["skills"].as_array().expect("skills array");
    let mut names: Vec<&str> = skills.iter().map(|s| s["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    // Pin the exact catalog: a silent addition or removal must show up
    // as a test failure so the consumer-facing surface stays explicit.
    assert_eq!(
        names,
        vec![
            "fan-out",
            "stint-handoff",
            "stint-start",
            "taskfleet-overview",
            "taskfleet-run-overview",
            "taskfleet-spawn-spinoff",
            "worktree",
            "worktree-bug-analysis",
            "worktree-merge",
            "worktree-research",
            "worktree-spinoff",
            "worktree-status",
            "worktree-technical-decision",
        ]
    );
    for s in skills {
        assert!(
            !s["description"].as_str().unwrap_or("").is_empty(),
            "empty description for {}",
            s["name"]
        );
        assert_eq!(s["cli_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(s["skill_schema_version"], 1);
    }
}

#[test]
fn skill_show_text_prints_skill_md_contents() {
    let home = mk_home();
    let out = bin(&home)
        .args([
            "--output",
            "text",
            "skill",
            "show",
            "taskfleet-run-overview",
        ])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "exit: {:?}", out.status);
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    assert!(
        stdout.starts_with("---\nname: taskfleet-run-overview"),
        "show did not emit frontmatter: {stdout:?}"
    );
}

#[test]
fn skill_show_json_wraps_content_under_data() {
    let home = mk_home();
    let out = bin(&home)
        .args([
            "skill",
            "show",
            "taskfleet-run-overview",
            "--output",
            "json",
        ])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "exit: {:?}", out.status);
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["data"]["name"], "taskfleet-run-overview");
    let content = v["data"]["content"].as_str().expect("content str");
    assert!(content.starts_with("---\nname: taskfleet-run-overview"));
}

#[test]
fn skill_install_refuses_overwrite_then_force_succeeds() {
    let home = mk_home();
    let dest = home.path().join("SKILL.md");

    let out = bin(&home)
        .args(["skill", "install", "taskfleet-run-overview", "--dest"])
        .arg(&dest)
        .output()
        .expect("spawn");
    assert!(out.status.success(), "first install failed: {out:?}");
    assert!(dest.exists(), "destination not created");

    let out = bin(&home)
        .args(["skill", "install", "taskfleet-run-overview", "--dest"])
        .arg(&dest)
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(2), "expected exit 2");
    let err: Value = serde_json::from_slice(&out.stderr).expect("json err envelope");
    assert_eq!(err["schema_version"], 1);
    // The error code is snake_case per the project-wide convention; the
    // contract is shared with every other subcommand and AI callers
    // branch on the exact string.
    assert_eq!(err["error"]["code"], "refused_overwrite");
    assert_eq!(err["error"]["invalid_value"], dest.display().to_string());

    let out = bin(&home)
        .args(["skill", "install", "taskfleet-run-overview", "--dest"])
        .arg(&dest)
        .arg("--force")
        .output()
        .expect("spawn");
    assert!(out.status.success(), "force install failed: {out:?}");
}

#[cfg(unix)]
#[test]
fn skill_install_force_replaces_dangling_symlink() {
    // `Path::exists()` follows a symlink and returns false for this target.
    // The install preflight must instead see the link itself and authorize
    // `--force` to atomically replace it.
    let home = mk_home();
    let dest = home.path().join("SKILL.md");
    std::os::unix::fs::symlink(home.path().join("missing-SKILL.md"), &dest)
        .expect("create dangling symlink");
    assert!(
        std::fs::symlink_metadata(&dest)
            .expect("link metadata")
            .file_type()
            .is_symlink(),
        "fixture must be a symlink"
    );
    assert!(!dest.exists(), "fixture must be dangling");

    let out = bin(&home)
        .args(["skill", "install", "taskfleet-run-overview", "--dest"])
        .arg(&dest)
        .arg("--force")
        .output()
        .expect("spawn");
    assert!(out.status.success(), "force install failed: {out:?}");
    assert!(
        std::fs::symlink_metadata(&dest)
            .expect("installed file metadata")
            .file_type()
            .is_file(),
        "dangling symlink was not replaced"
    );
    assert!(
        std::fs::read_to_string(&dest)
            .expect("installed body")
            .contains("name: taskfleet-run-overview"),
        "installed body missing"
    );
}

#[test]
fn skill_install_with_default_paths_writes_under_home() {
    let home = mk_home();
    let out = bin(&home)
        .args([
            "skill",
            "install",
            "taskfleet-run-overview",
            "--output",
            "json",
        ])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "default install failed: {out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let installed = v["data"]["installed"].as_array().expect("installed array");
    // A default install dual-homes the skill: the claude copy plus the pi
    // mirror (see `skill_install_default_dual_homes_into_pi`).
    let claude = installed
        .iter()
        .find(|f| f["agent"] == "claude")
        .expect("claude entry");
    let expected: PathBuf = home
        .path()
        .join(".claude/skills/taskfleet-run-overview/SKILL.md");
    assert_eq!(claude["path"], expected.display().to_string());
    assert!(expected.exists(), "claude install not on disk");
}

#[test]
fn skill_install_force_prunes_orphan_companion_file() {
    // A prior binary installed `stint-start` with an extra companion the
    // current binary no longer ships. It survives on disk and its
    // provenance marker still records it. A `--force` re-install must remove
    // the orphan file and report it under `pruned_companions`.
    let home = mk_home();
    // First install lays down the skill dir + marker.
    let first = bin(&home)
        .args(["skill", "install", "stint-start", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(first.status.success(), "first install failed: {first:?}");

    let skill_dir = home.path().join(".claude/skills/stint-start");
    let marker = skill_dir.join(".taskfleet-managed");
    let orphan = skill_dir.join("OLD-COMPANION.md");

    // Simulate the prior binary: drop the orphan file and record it in the
    // marker alongside whatever the marker already holds.
    std::fs::write(&orphan, "stale companion\n").expect("write orphan");
    let mut marker_body = std::fs::read_to_string(&marker).expect("read marker");
    marker_body.push_str("companion: OLD-COMPANION.md\n");
    std::fs::write(&marker, marker_body).expect("append marker");
    // Forced re-install prunes the orphan.
    let out = bin(&home)
        .args([
            "skill",
            "install",
            "stint-start",
            "--force",
            "--output",
            "json",
        ])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "force reinstall failed: {out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let pruned: Vec<&str> = v["data"]["pruned_companions"]
        .as_array()
        .expect("pruned_companions array")
        .iter()
        .map(|e| e.as_str().unwrap())
        .collect();
    assert_eq!(pruned, vec!["stint-start/OLD-COMPANION.md"]);
    assert!(!orphan.exists(), "orphan companion not removed by --force");
    // The rewritten marker no longer records the pruned orphan.
    let marker_after = std::fs::read_to_string(&marker).expect("read marker after");
    assert!(
        !marker_after.contains("OLD-COMPANION.md"),
        "marker still records the pruned orphan: {marker_after:?}"
    );
}

#[test]
fn forced_full_install_prunes_retired_dag_companion_from_all_mirrors() {
    let home = mk_home();
    assert!(bin(&home)
        .args(["skill", "install", "--agent", "all"])
        .output()
        .unwrap()
        .status
        .success());

    let retired_name = "AGENTS-EXECUTION-DAG.md";
    let retired_bytes = b"retired managed companion\n";

    let claude_dir = home.path().join(".claude/skills/stint-start");
    std::fs::write(claude_dir.join(retired_name), retired_bytes).unwrap();
    let claude_marker = claude_dir.join(".taskfleet-managed");
    let mut marker = std::fs::read_to_string(&claude_marker).unwrap();
    writeln!(marker, "companion: {retired_name}").expect("writing to a String cannot fail");
    std::fs::write(&claude_marker, marker).unwrap();

    let pi_dir = home.path().join(".pi/agent/skills/stint-start");
    std::fs::write(pi_dir.join(retired_name), retired_bytes).unwrap();
    let pi_record = env_orch_state_record(&home);
    let mut record: Value = serde_json::from_slice(&std::fs::read(&pi_record).unwrap()).unwrap();
    record["skills"]["stint-start"]["files"][retired_name] = serde_json::json!({
        "sha256": sha256_hex(retired_bytes),
        "kind": "companion"
    });
    std::fs::write(&pi_record, serde_json::to_vec_pretty(&record).unwrap()).unwrap();

    let codex_shared = home.path().join(".codex/prompts/_shared");
    std::fs::write(codex_shared.join(retired_name), retired_bytes).unwrap();
    let codex_marker = codex_shared.join(".taskfleet-managed");
    let mut marker = std::fs::read_to_string(&codex_marker).unwrap();
    writeln!(marker, "companion: {retired_name}").expect("writing to a String cannot fail");
    std::fs::write(&codex_marker, marker).unwrap();

    let doctor = bin(&home)
        .args(["doctor", "--output", "json"])
        .output()
        .unwrap();
    // Doctor may exit 1 when unrelated host dependencies are absent. Its JSON
    // report remains the source of truth for the orphan checks under test.
    let doctor: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let checks = doctor["data"]["checks"].as_array().unwrap();
    for id in [
        "skill.orphan.stint-start.AGENTS-EXECUTION-DAG.md",
        "skill.orphan.stint-start.pi.AGENTS-EXECUTION-DAG.md",
        "skill.orphan.codex._shared.AGENTS-EXECUTION-DAG.md",
    ] {
        let check = checks
            .iter()
            .find(|c| c["id"] == id)
            .unwrap_or_else(|| panic!("doctor did not report retired companion {id}: {checks:?}"));
        assert_eq!(check["status"], "warn", "{id}: {check:?}");
        assert!(
            check["fix_suggestion"]
                .as_str()
                .is_some_and(|s| s.contains("--force")),
            "{id}: {check:?}"
        );
    }

    assert!(bin(&home)
        .args(["skill", "install", "--force"])
        .output()
        .unwrap()
        .status
        .success());
    assert!(bin(&home)
        .args(["skill", "install", "--agent", "codex", "--force"])
        .output()
        .unwrap()
        .status
        .success());

    for retired in [
        claude_dir.join(retired_name),
        pi_dir.join(retired_name),
        codex_shared.join(retired_name),
    ] {
        assert!(
            !retired.exists(),
            "retired companion survived at {}",
            retired.display()
        );
    }
    assert!(!std::fs::read_to_string(claude_marker)
        .unwrap()
        .contains(retired_name));
    assert!(!std::fs::read_to_string(codex_marker)
        .unwrap()
        .contains(retired_name));
    let record: Value = serde_json::from_slice(&std::fs::read(pi_record).unwrap()).unwrap();
    assert!(record["skills"]["stint-start"]["files"]
        .get(retired_name)
        .is_none());

    let doctor = bin(&home)
        .args(["doctor", "--output", "json"])
        .output()
        .unwrap();
    let doctor: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    assert!(!doctor["data"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["id"].as_str().is_some_and(|id| id.contains(retired_name))));
}

#[test]
fn codex_install_writes_provenance_marker() {
    // A default `--agent codex` install records the installed prompt in the
    // shared provenance marker used by pruning and doctor.
    let home = mk_home();
    let out = bin(&home)
        .args([
            "skill",
            "install",
            "stint-start",
            "--agent",
            "codex",
            "--output",
            "json",
        ])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "codex install failed: {out:?}");

    assert!(home.path().join(".codex/prompts/stint-start.md").exists());
    assert!(
        !home
            .path()
            .join(".codex/prompts/_shared/AGENTS-EXECUTION-DAG.md")
            .exists(),
        "retired DAG companion must not be installed for codex"
    );
    // The marker records the prompt.
    let marker = home
        .path()
        .join(".codex/prompts/_shared/.taskfleet-managed");
    let body = std::fs::read_to_string(&marker).expect("codex marker not written");
    assert!(body.contains("managed-by: taskfleet"), "marker: {body}");
    assert!(body.contains("prompt: stint-start"), "marker: {body}");
    assert!(!body.contains("AGENTS-EXECUTION-DAG.md"), "marker: {body}");
}

#[test]
fn codex_force_prunes_orphan_prompt_and_companion() {
    // A prior binary installed a codex prompt + `_shared/` companion this
    // binary no longer ships; both linger on disk and the shared marker still
    // records them. A full-catalog `--force` install must remove BOTH and
    // report them (prompt under `pruned`, companion as `_shared/<file>` under
    // `pruned_companions`).
    let home = mk_home();
    // Full-catalog codex install lays down the flat prompts + shared marker.
    let first = bin(&home)
        .args(["skill", "install", "--agent", "codex", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(
        first.status.success(),
        "first codex install failed: {first:?}"
    );

    let prompts = home.path().join(".codex/prompts");
    let shared = prompts.join("_shared");
    let marker = shared.join(".taskfleet-managed");
    // Simulate the prior binary: a de-registered prompt + a de-registered
    // shared companion, both recorded in the marker.
    let orphan_prompt = prompts.join("gone-skill.md");
    let orphan_companion = shared.join("OLD-SHARED.md");
    std::fs::write(&orphan_prompt, "stale prompt\n").unwrap();
    std::fs::write(&orphan_companion, "stale shared\n").unwrap();
    let mut marker_body = std::fs::read_to_string(&marker).unwrap();
    marker_body.push_str("prompt: gone-skill\n");
    marker_body.push_str("companion: OLD-SHARED.md\n");
    std::fs::write(&marker, marker_body).unwrap();
    let out = bin(&home)
        .args([
            "skill", "install", "--agent", "codex", "--force", "--output", "json",
        ])
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "force codex reinstall failed: {out:?}"
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let pruned: Vec<&str> = v["data"]["pruned"]
        .as_array()
        .expect("pruned array")
        .iter()
        .map(|e| e.as_str().unwrap())
        .collect();
    assert!(
        pruned.contains(&"gone-skill"),
        "prompt not pruned: {pruned:?}"
    );
    let pruned_companions: Vec<&str> = v["data"]["pruned_companions"]
        .as_array()
        .expect("pruned_companions array")
        .iter()
        .map(|e| e.as_str().unwrap())
        .collect();
    assert!(
        pruned_companions.contains(&"_shared/OLD-SHARED.md"),
        "companion not pruned: {pruned_companions:?}"
    );

    assert!(!orphan_prompt.exists(), "orphan codex prompt not removed");
    assert!(
        !orphan_companion.exists(),
        "orphan codex companion not removed"
    );
    // The rewritten marker no longer records either pruned orphan.
    let marker_after = std::fs::read_to_string(&marker).unwrap();
    assert!(
        !marker_after.contains("gone-skill") && !marker_after.contains("OLD-SHARED.md"),
        "marker still records a pruned orphan: {marker_after:?}"
    );
}

#[test]
fn skill_install_agent_all_installs_to_both_default_paths() {
    let home = mk_home();
    let out = bin(&home)
        .args([
            "skill",
            "install",
            "taskfleet-spawn-spinoff",
            "--agent",
            "all",
            "--output",
            "json",
        ])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "agent=all install failed: {out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let installed = v["data"]["installed"].as_array().expect("installed");
    let agents: Vec<&str> = installed
        .iter()
        .map(|f| f["agent"].as_str().unwrap())
        .collect();
    assert!(agents.contains(&"claude"));
    assert!(agents.contains(&"codex"));
    assert!(home
        .path()
        .join(".claude/skills/taskfleet-spawn-spinoff/SKILL.md")
        .exists());
    assert!(home
        .path()
        .join(".codex/prompts/taskfleet-spawn-spinoff.md")
        .exists());
}

#[test]
fn skill_install_explicit_pi_only_uses_native_tree() {
    let home = mk_home();
    let out = bin(&home)
        .args([
            "skill",
            "install",
            "taskfleet-overview",
            "--agent",
            "pi",
            "--output",
            "json",
        ])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "pi install failed: {out:?}");
    assert!(home
        .path()
        .join(".pi/agent/skills/taskfleet-overview/SKILL.md")
        .is_file());
    assert!(!home
        .path()
        .join(".claude/skills/taskfleet-overview")
        .exists());
    assert!(!home
        .path()
        .join(".codex/prompts/taskfleet-overview.md")
        .exists());
}

#[test]
fn skill_install_target_preserves_all_native_layouts() {
    let home = mk_home();
    let target = home.path().join("isolated");
    let out = bin(&home)
        .args(["skill", "install", "taskfleet-overview", "--target"])
        .arg(&target)
        .args(["--output", "json"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "target install failed: {out:?}");
    for path in [
        ".claude/skills/taskfleet-overview/SKILL.md",
        ".pi/agent/skills/taskfleet-overview/SKILL.md",
        ".codex/prompts/taskfleet-overview.md",
    ] {
        assert!(target.join(path).is_file(), "missing {path}");
    }
    assert!(
        !home.path().join("state/pi-installed-skills.json").exists(),
        "isolated --target must not mutate normal provenance state"
    );
}

#[test]
fn skill_install_dry_run_reports_plan_and_writes_nothing() {
    let home = mk_home();
    let target = home.path().join("dry-target");
    let out = bin(&home)
        .args(["skill", "install", "taskfleet-overview", "--target"])
        .arg(&target)
        .args(["--dry-run", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "dry-run failed: {out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["data"]["dry_run"], true);
    let would = v["data"]["would"].as_array().expect("would array");
    assert_eq!(would.len(), 3, "one self-contained artifact per runtime");
    assert_eq!(
        would
            .iter()
            .map(|row| row["agent"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["claude", "pi", "codex"]
    );
    assert!(!target.exists(), "dry-run created the target tree");
}

#[test]
fn skill_install_target_collision_is_atomic_and_force_overwrites() {
    let home = mk_home();
    let target = home.path().join("isolated");
    let codex = target.join(".codex/prompts/taskfleet-overview.md");
    std::fs::create_dir_all(codex.parent().unwrap()).unwrap();
    std::fs::write(&codex, "mine\n").unwrap();

    let refused = bin(&home)
        .args(["skill", "install", "taskfleet-overview", "--target"])
        .arg(&target)
        .output()
        .expect("spawn");
    assert!(!refused.status.success());
    assert_eq!(std::fs::read_to_string(&codex).unwrap(), "mine\n");
    assert!(!target
        .join(".claude/skills/taskfleet-overview/SKILL.md")
        .exists());
    assert!(!target
        .join(".pi/agent/skills/taskfleet-overview/SKILL.md")
        .exists());

    let forced = bin(&home)
        .args(["skill", "install", "taskfleet-overview", "--target"])
        .arg(&target)
        .arg("--force")
        .output()
        .expect("spawn");
    assert!(forced.status.success(), "force failed: {forced:?}");
    assert!(std::fs::read_to_string(codex)
        .unwrap()
        .contains("name: taskfleet-overview"));
}

#[test]
fn skill_install_no_name_installs_every_skill() {
    let home = mk_home();
    let out = bin(&home)
        .args(["skill", "install", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "install-all failed: {out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let installed = v["data"]["installed"].as_array().expect("installed");
    let names: Vec<&str> = installed
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"taskfleet-run-overview"));
    assert!(names.contains(&"taskfleet-spawn-spinoff"));
}

#[test]
fn skill_install_agent_all_with_dest_is_rejected() {
    let home = mk_home();
    let dest = home.path().join("SKILL.md");
    let out = bin(&home)
        .args([
            "skill",
            "install",
            "taskfleet-run-overview",
            "--agent",
            "all",
            "--dest",
        ])
        .arg(&dest)
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(1), "expected user-error exit 1");
    let err: Value = serde_json::from_slice(&out.stderr).expect("err json");
    assert_eq!(err["error"]["code"], "invalid_arguments");
}

#[test]
fn skill_install_partial_failure_is_preflighted() {
    // Pre-create the codex destination so the second target in the plan
    // would fail. The preflight pass must refuse the whole install before
    // writing the claude target, so the user can retry once they decide
    // to pass --force, instead of being stuck in a half-installed state.
    let home = mk_home();
    let codex = home.path().join(".codex/prompts");
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::write(codex.join("taskfleet-run-overview.md"), "pre-existing").unwrap();

    let out = bin(&home)
        .args([
            "skill",
            "install",
            "taskfleet-run-overview",
            "--agent",
            "all",
        ])
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(2));
    let err: Value = serde_json::from_slice(&out.stderr).expect("err json");
    assert_eq!(err["error"]["code"], "refused_overwrite");
    // Crucially, the claude target was *not* touched — preflight saw the
    // codex collision and bailed before any write.
    assert!(!home
        .path()
        .join(".claude/skills/taskfleet-run-overview/SKILL.md")
        .exists());
}

#[test]
fn skill_install_accepts_bare_relative_dest() {
    let home = mk_home();
    // Bare relative path: parent is the empty string; normalized_parent
    // must coerce it to "." so create_dir_all does not blow up.
    let out = bin(&home)
        .current_dir(home.path())
        .args([
            "skill",
            "install",
            "taskfleet-run-overview",
            "--dest",
            "SKILL.md",
        ])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "bare-dest install failed: {out:?}");
    assert!(home.path().join("SKILL.md").exists());
}

#[test]
fn skill_show_unknown_emits_skill_not_found() {
    let home = mk_home();
    let out = bin(&home)
        .args(["skill", "show", "no-such-skill"])
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(1));
    let err: Value = serde_json::from_slice(&out.stderr).expect("json err envelope");
    assert_eq!(err["error"]["code"], "skill_not_found");
    assert_eq!(err["error"]["invalid_value"], "no-such-skill");
}

#[test]
fn skill_print_default_streams_skill_md_byte_identically() {
    // §16: `skill print` (default `--output jsonl`) writes the embedded
    // SKILL.md byte-identically — no envelope wrapping. The bytes must
    // equal what `skill install` would persist.
    let home = mk_home();
    let print_out = bin(&home)
        .args(["skill", "print", "taskfleet-overview"])
        .output()
        .expect("spawn");
    assert!(print_out.status.success(), "exit: {:?}", print_out.status);
    let dest = home.path().join("printed.md");
    let install_out = bin(&home)
        .args(["skill", "install", "taskfleet-overview", "--dest"])
        .arg(&dest)
        .output()
        .expect("spawn");
    assert!(install_out.status.success());
    let on_disk = std::fs::read(&dest).expect("read installed");
    assert_eq!(
        print_out.stdout, on_disk,
        "skill print stdout must equal skill install on-disk bytes"
    );
}

#[test]
fn skill_print_json_payload_pins_schema() {
    let home = mk_home();
    let out = bin(&home)
        .args(["skill", "print", "taskfleet-overview", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "exit: {:?}", out.status);
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["schema_version"], 1);
    let data = &v["data"];
    assert_eq!(data["name"], "taskfleet-overview");
    assert_eq!(data["schema_version"], 1);
    assert_eq!(data["schema_version_skill"], 1);
    assert_eq!(
        data["cli_version"].as_str().unwrap(),
        env!("CARGO_PKG_VERSION")
    );
    assert!(data["content"]
        .as_str()
        .unwrap()
        .starts_with("---\nname: taskfleet-overview"));
    assert!(data["path_in_repo"]
        .as_str()
        .unwrap()
        .contains("SKILL.template.md"));
}

#[test]
fn skill_print_unknown_emits_skill_not_found() {
    let home = mk_home();
    let out = bin(&home)
        .args(["skill", "print", "no-such-skill"])
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(1));
    let err: Value = serde_json::from_slice(&out.stderr).expect("err envelope");
    assert_eq!(err["error"]["code"], "skill_not_found");
}

#[test]
fn skill_install_over_older_version_requires_force_and_then_warns() {
    // §15 no-clobber applies uniformly even when version drift is known.
    // Once explicit --force authorizes replacement, preserve the §17 warning.
    let home = mk_home();
    let dest = home.path().join("SKILL.md");
    // Hand-write an "older" skill — cli_version 0.0.0 will always be
    // older than the current binary.
    std::fs::write(
        &dest,
        "---\nname: taskfleet-overview\ndescription: old\ncli_version: \"0.0.0\"\nschema_version: 1\n---\n",
    )
    .unwrap();

    let out = bin(&home)
        .args(["skill", "install", "taskfleet-overview", "--dest"])
        .arg(&dest)
        .args(["--output", "json"])
        .output()
        .expect("spawn");
    assert!(!out.status.success(), "older copy must not be clobbered");
    assert_eq!(
        std::fs::read_to_string(&dest).unwrap(),
        "---\nname: taskfleet-overview\ndescription: old\ncli_version: \"0.0.0\"\nschema_version: 1\n---\n"
    );

    let out = bin(&home)
        .args(["skill", "install", "taskfleet-overview", "--dest"])
        .arg(&dest)
        .args(["--force", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "forced upgrade failed: {out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let warnings = v["warnings"].as_array().expect("warnings array");
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("skill_version_drift")
                && w.as_str().unwrap().contains("0.0.0")),
        "expected skill_version_drift warning naming 0.0.0; got {warnings:?}"
    );
    // File was overwritten with the bundled body.
    let after = std::fs::read_to_string(&dest).unwrap();
    assert!(after.contains(&format!("cli_version: \"{}\"", env!("CARGO_PKG_VERSION"))));
}

#[test]
fn skill_install_over_newer_version_refuses_with_skill_version_too_new() {
    // §17 drift: install over a newer on-disk skill refuses with exit 2
    // and `skill_version_too_new` unless --force is passed.
    let home = mk_home();
    let dest = home.path().join("SKILL.md");
    std::fs::write(
        &dest,
        "---\nname: taskfleet-overview\ndescription: future\ncli_version: \"99.0.0\"\nschema_version: 1\n---\n",
    )
    .unwrap();

    let out = bin(&home)
        .args(["skill", "install", "taskfleet-overview", "--dest"])
        .arg(&dest)
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(2), "expected exit 2");
    let err: Value = serde_json::from_slice(&out.stderr).expect("err envelope");
    assert_eq!(err["error"]["code"], "skill_version_too_new");

    // --force overrides the refusal.
    let out = bin(&home)
        .args(["skill", "install", "taskfleet-overview", "--dest"])
        .arg(&dest)
        .arg("--force")
        .output()
        .expect("spawn");
    assert!(out.status.success(), "--force install must succeed");
}

const MARKER: &str = ".taskfleet-managed";

/// Write a valid provenance marker naming `skill_name` into `dir` — the
/// shape `is_managed_skill_dir` accepts.
fn write_marker(dir: &std::path::Path, skill_name: &str) {
    let skill_hash = sha256_hex(&std::fs::read(dir.join("SKILL.md")).unwrap());
    std::fs::write(
        dir.join(MARKER),
        format!(
            "managed-by: taskfleet\ncli_version: 9.9.9\nskill_name: {skill_name}\nsha256: {skill_hash}\n"
        ),
    )
    .unwrap();
}

#[test]
fn skill_install_default_stamps_provenance_marker() {
    // Every default claude install must drop the provenance marker beside
    // SKILL.md — it's what makes later pruning safe. The marker must name
    // the skill so a copied-and-renamed dir is never mistaken for it.
    let home = mk_home();
    assert!(bin(&home)
        .args(["skill", "install", "taskfleet-run-overview"])
        .output()
        .expect("spawn")
        .status
        .success());
    let marker = home
        .path()
        .join(".claude/skills/taskfleet-run-overview")
        .join(MARKER);
    assert!(
        marker.is_file(),
        "provenance marker not written next to SKILL.md"
    );
    let body = std::fs::read_to_string(&marker).unwrap();
    assert!(body.contains("managed-by: taskfleet"), "marker: {body}");
    assert!(
        body.contains("skill_name: taskfleet-run-overview"),
        "marker: {body}"
    );
}

#[test]
fn bundled_stint_guidance_distinguishes_untriaged_from_explicit_deferral() {
    fn normalized(body: &str) -> String {
        body.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    let home = mk_home();
    for skill in ["stint-start", "stint-handoff"] {
        let out = bin(&home)
            .args([
                "skill", "install", skill, "--agent", "all", "--output", "json",
            ])
            .output()
            .expect("spawn");
        assert!(out.status.success(), "install {skill} failed: {out:?}");
    }

    let start_path = home.path().join(".claude/skills/stint-start/SKILL.md");
    let start_raw = std::fs::read_to_string(&start_path).unwrap();
    let start = normalized(&start_raw);
    assert!(
        start.contains("candidate awaiting that gate stays `status: untriaged` with no `lane`, `lane_seq`, or `collision` assignment"),
        "stint-start must preserve unaccepted candidates as untriaged and unscheduled"
    );
    assert!(
        start.contains("`status: deferred` is reserved for an explicit human/product decision"),
        "stint-start must reserve deferred for an explicit human/product disposition"
    );
    assert!(
        start.contains("mechanically says `spawnable: true`; never launch it until it has passed the human lane-or-close gate"),
        "stint-start must not execute mechanically spawnable unscheduled rows"
    );
    assert!(
        start.contains("Omission from this round alone authorizes no lifecycle change"),
        "stint-start must not treat round omission as a deferral"
    );
    assert!(
        start.contains("on human acceptance for execution, move the issue from `untriaged` to the project's accepted active status and assign its lane metadata; on an explicit human/product “not now” decision, set `deferred` and remove `lane`, `lane_seq`, and every scheduling `collision` assignment"),
        "stint-start must move lifecycle and scheduling state together"
    );
    assert!(
        start.contains("Launch only a lane issue whose own `spawnable` field is true **and** whose status is an executable, triaged status"),
        "stint-start must status-gate the operative launch rule"
    );
    assert!(
        !start.contains("Leave deferred or out-of-plan entries unscheduled"),
        "stint-start must not restore the retired conflating guidance"
    );

    let handoff_path = home.path().join(".claude/skills/stint-handoff/SKILL.md");
    let handoff_raw = std::fs::read_to_string(&handoff_path).unwrap();
    let handoff = normalized(&handoff_raw);
    assert!(
        handoff.contains("unaccepted candidate stays `status: untriaged`, with no `lane`, `lane_seq`, or `collision` assignment"),
        "stint-handoff must preserve review candidates as untriaged and unscheduled"
    );
    assert!(
        handoff.contains("status is reserved for an explicit human/product “worthwhile, but not now” disposition"),
        "stint-handoff must reserve deferred for explicit product disposition"
    );
    assert!(
        handoff.contains("mechanical `spawnable: true` does not make it executable; it must pass human triage, move to an accepted active status, and gain a lane"),
        "stint-handoff must not present unscheduled rows as executable"
    );
    assert!(
        handoff.contains("is created with no lane assignment"),
        "stint-handoff must not conflate no lane with the literal unlaned lane"
    );
    assert!(
        !handoff.contains("Leave deferred or out-of-plan entries unscheduled"),
        "stint-handoff must not contain the retired conflating guidance"
    );

    for (skill, claude_raw) in [("stint-start", start_raw), ("stint-handoff", handoff_raw)] {
        let pi = std::fs::read_to_string(
            home.path()
                .join(format!(".pi/agent/skills/{skill}/SKILL.md")),
        )
        .unwrap();
        assert_eq!(pi, claude_raw, "{skill} pi mirror must match Claude");

        let codex = normalized(
            &std::fs::read_to_string(home.path().join(format!(".codex/prompts/{skill}.md")))
                .unwrap(),
        );
        assert!(
            codex.contains("status: untriaged") && codex.contains("status: deferred"),
            "{skill} Codex mirror must carry the lifecycle distinction"
        );
    }
}

#[test]
fn rendered_bug_analysis_skill_requires_the_canonical_triage_heading() {
    let home = mk_home();
    let out = bin(&home)
        .args(["skill", "print", "worktree-bug-analysis"])
        .output()
        .expect("print bundled bug-analysis skill");
    assert!(out.status.success(), "skill print failed: {out:?}");
    let rendered = String::from_utf8(out.stdout).expect("skill is utf-8");
    let normalized = rendered.split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(
        normalized.contains("under the exact heading `## Triage analysis`"),
        "rendered worker brief contract must require issuectl's canonical heading"
    );
    assert!(
        !rendered.contains("## Suspected Root Cause"),
        "rendered worker brief contract must not offer an alternative heading"
    );
}

#[test]
fn bundled_workflow_skills_render_the_bounded_stint_contract() {
    fn print_skill(home: &tempfile::TempDir, name: &str) -> String {
        let out = bin(home)
            .args(["skill", "print", name])
            .output()
            .expect("print bundled skill");
        assert!(out.status.success(), "print {name} failed: {out:?}");
        String::from_utf8(out.stdout)
            .expect("skill is utf-8")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    let home = mk_home();
    let start = print_skill(&home, "stint-start");
    for required in [
        "one exact next action",
        "at most one smaller follow-up",
        "normal source branch and both index and worktree are clean",
        "Resolve only clearly mechanical, narrow conflicts",
        "Never force-push",
        "this session launches or explicitly adopts",
        "Listing, showing, reserving, mentioning, or discovering a run never adopts it",
        "reservations, not owned work",
        "stop **spawning** as unverifiable",
        "The worker chooses proportionate review after seeing the final diff",
        "Reuse adequate existing evidence",
        "after the one feedback follow-up report: `/stint-handoff`",
        "applies to **every** return from Phases 0–3",
        "No durable stint/checkpoint state",
    ] {
        assert!(start.contains(required), "stint-start missing {required:?}");
    }
    for removed in [
        "`git pull --ff-only`",
        "/worktree-code",
        "/worktree-bugfix",
        "/orchestrate",
    ] {
        assert!(
            !start.contains(removed),
            "stint-start retains removed workflow {removed:?}"
        );
    }

    let handoff = print_skill(&home, "stint-handoff");
    for required in [
        "session launched or explicitly adopted",
        "treat session worker ownership as clear",
        "exact issuectl hold array shape",
        "An unmappable foreign run does not block terminal handoff",
        "continue through steps 2–3",
        "git diff --cached --quiet",
        "git diff --cached --name-only",
        "the user's explicit request to finish is sufficient",
        "direct standalone invocation",
        "No durable stint/checkpoint or global-run ownership",
    ] {
        assert!(
            handoff.contains(required),
            "stint-handoff missing {required:?}"
        );
    }
    assert!(
        !handoff.contains("Every live, awaiting-input, recoverable"),
        "handoff must not block on every global run"
    );

    let generic_spinoff = print_skill(&home, "taskfleet-spawn-spinoff");
    assert!(
        generic_spinoff.contains("worker chooses and explains review depth after the final diff")
    );

    let spinoff = print_skill(&home, "worktree-spinoff");
    for required in [
        "a bug closes as `fixed`; a feature/task/improvement/chore closes as `done`",
        "issuectl update <slug> --status in-progress --json",
        "issuectl close <slug> --status <fixed-or-done> --stamp --as <agent> --json",
        "`stamped` or `already_present`",
        "commit that metadata in a separate commit",
        "Do not add a `Fixes-Issue` trailer or close an issue for a freeform run",
    ] {
        assert!(
            spinoff.contains(required),
            "worktree-spinoff missing {required:?}"
        );
    }

    let analysis = print_skill(&home, "worktree-bug-analysis");
    assert!(analysis.contains("`Refs-Issue: @<slug>` trailer"));
    assert!(analysis.contains("never use `Fixes-Issue`, `issuectl close --stamp`"));

    let research = print_skill(&home, "worktree-research");
    assert!(research.contains("`Refs-Issue: @<slug>`"));
    assert!(research.contains("issuectl close <slug> --status done --stamp --as <agent> --json"));
    assert!(research.contains("Freeform research has no issue trailer"));

    let decision = print_skill(&home, "worktree-technical-decision");
    assert!(decision.contains("issuectl close <slug> --status done --stamp --as <agent> --json"));
    assert!(decision.contains("A freeform decision has no issue trailer"));

    let listed = bin(&home)
        .args(["skill", "list", "--output", "json"])
        .output()
        .expect("list bundled skills");
    assert!(listed.status.success(), "skill list failed: {listed:?}");
    let listed: serde_json::Value =
        serde_json::from_slice(&listed.stdout).expect("skill list json");
    let names = listed["data"]["skills"]
        .as_array()
        .expect("skills array")
        .iter()
        .map(|row| row["name"].as_str().expect("skill name"))
        .collect::<Vec<_>>();
    for retired in ["worktree-code", "worktree-bugfix", "orchestrate"] {
        assert!(
            !names.contains(&retired),
            "retired skill remains bundled: {retired}"
        );
    }
    for skill in names {
        let rendered = print_skill(&home, skill);
        if [
            "fan-out",
            "worktree-research",
            "worktree-spinoff",
            "worktree-technical-decision",
        ]
        .contains(&skill)
        {
            assert!(rendered.contains("<workmux-reported-window-name>"));
            for retired_emoji in ["🚀", "🔬", "⚖️", "🪭"] {
                assert!(
                    !rendered.contains(retired_emoji),
                    "{skill} invents retired kind emoji {retired_emoji}"
                );
            }
        }
        for retired in ["/worktree-code", "/worktree-bugfix", "/orchestrate"] {
            assert!(
                !rendered.contains(retired),
                "{skill} points to retired workflow {retired}"
            );
        }
    }
}

#[test]
fn skill_install_default_dual_homes_into_pi() {
    // A default `skill install` writes the claude copy AND mirrors the same
    // SKILL.md into pi.dev's per-skill dir (`~/.pi/agent/skills/<name>/`),
    // byte-for-byte identical. The claude path is proven unchanged: same
    // location, same bytes it has always written.
    let home = mk_home();
    let out = bin(&home)
        .args(["skill", "install", "stint-start", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "default install failed: {out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let installed = v["data"]["installed"].as_array().expect("installed array");
    let agents: Vec<&str> = installed
        .iter()
        .map(|f| f["agent"].as_str().unwrap())
        .collect();
    assert!(
        agents.contains(&"claude"),
        "claude entry missing: {agents:?}"
    );
    assert!(agents.contains(&"pi"), "pi entry missing: {agents:?}");

    let claude = home.path().join(".claude/skills/stint-start/SKILL.md");
    let pi = home.path().join(".pi/agent/skills/stint-start/SKILL.md");
    assert!(claude.exists(), "claude SKILL.md not on disk");
    assert!(pi.exists(), "pi SKILL.md not on disk");
    assert_eq!(
        std::fs::read(&claude).unwrap(),
        std::fs::read(&pi).unwrap(),
        "pi mirror must be byte-identical to the claude SKILL.md"
    );
    for retired in [
        home.path()
            .join(".claude/skills/stint-start/AGENTS-EXECUTION-DAG.md"),
        home.path()
            .join(".pi/agent/skills/stint-start/AGENTS-EXECUTION-DAG.md"),
    ] {
        assert!(
            !retired.exists(),
            "retired DAG companion must not be installed: {}",
            retired.display()
        );
    }

    // The pi mirror is still not a managed claude dir — no in-dir provenance
    // marker (lifecycle is keyed on the out-of-band record instead).
    assert!(
        !home
            .path()
            .join(".pi/agent/skills/stint-start")
            .join(MARKER)
            .is_file(),
        "pi mirror must not carry the claude provenance marker"
    );

    let record: Value = serde_json::from_slice(
        &std::fs::read(env_orch_state_record(&home)).expect("provenance record"),
    )
    .expect("record json");
    assert_eq!(
        record["skills"]["stint-start"]["files"]["SKILL.md"]["kind"], "skill",
        "body file recorded with kind=skill: {record}"
    );
}

#[test]
fn skill_install_force_reconciles_dropped_pi_companion() {
    // A pi companion a prior binary recorded that the current binary no longer
    // bundles must be removed on a --force install and dropped from the record —
    // otherwise the `skill.orphan.<name>.pi.<file>` doctor warning it raises would
    // be unfixable (issue support-pi-dev, review finding F1).
    let home = mk_home();
    assert!(bin(&home)
        .args(["skill", "install", "stint-start"])
        .output()
        .expect("spawn")
        .status
        .success());

    let pi_dir = home.path().join(".pi/agent/skills/stint-start");
    let record_path = env_orch_state_record(&home);

    // Simulate a prior binary having installed an extra companion the current
    // binary no longer ships. Its recorded hash matches the bytes on disk.
    let orphan = pi_dir.join("OLD-COMPANION.md");
    let orphan_bytes = b"former bundled companion\n";
    std::fs::write(&orphan, orphan_bytes).unwrap();

    let mut prov: Value = serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    prov["skills"]["stint-start"]["files"]["OLD-COMPANION.md"] = serde_json::json!({
        "sha256": sha256_hex(orphan_bytes),
        "kind": "companion"
    });
    std::fs::write(&record_path, serde_json::to_string_pretty(&prov).unwrap()).unwrap();

    // A --force redeploy reconciles the dropped companion.
    let out = bin(&home)
        .args([
            "skill",
            "install",
            "stint-start",
            "--force",
            "--output",
            "json",
        ])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "force redeploy failed: {out:?}");

    assert!(
        !orphan.exists(),
        "dropped pi companion must be removed on --force"
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let pruned_companions: Vec<&str> = v["data"]["pruned_companions"]
        .as_array()
        .expect("pruned_companions array")
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    assert!(
        pruned_companions.contains(&"stint-start/OLD-COMPANION.md"),
        "orphan pi companion must be reported: {pruned_companions:?}"
    );

    // The record no longer tracks it → the doctor loop is now cleared.
    let after: Value = serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    assert!(
        after["skills"]["stint-start"]["files"]
            .get("OLD-COMPANION.md")
            .is_none(),
        "reconciled companion must be dropped from the record: {after}"
    );
}

#[test]
fn skill_install_force_relinquishes_diverged_dropped_pi_companion() {
    // A dropped pi companion whose on-disk bytes DON'T match the recorded hash
    // (user-edited since we wrote it) must be LEFT on disk but dropped from
    // tracking on --force — we relinquish a copy we no longer recognise rather
    // than delete it (review finding F1 relinquish arm).
    let home = mk_home();
    assert!(bin(&home)
        .args(["skill", "install", "stint-start"])
        .output()
        .expect("spawn")
        .status
        .success());
    let pi_dir = home.path().join(".pi/agent/skills/stint-start");
    let record_path = env_orch_state_record(&home);
    let orphan = pi_dir.join("OLD-COMPANION.md");
    std::fs::write(&orphan, "user has since edited this\n").unwrap();
    let mut prov: Value = serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    // A recorded hash that does NOT match the on-disk bytes.
    prov["skills"]["stint-start"]["files"]["OLD-COMPANION.md"] = serde_json::json!({
        "sha256": "00000000000000000000000000000000000000000000000000000000deadbeef",
        "kind": "companion"
    });
    std::fs::write(&record_path, serde_json::to_string_pretty(&prov).unwrap()).unwrap();

    let out = bin(&home)
        .args([
            "skill",
            "install",
            "stint-start",
            "--force",
            "--output",
            "json",
        ])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "install failed: {out:?}");
    assert!(
        orphan.exists(),
        "a companion whose bytes don't match the record is left on disk"
    );
    let after: Value = serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    assert!(
        after["skills"]["stint-start"]["files"]
            .get("OLD-COMPANION.md")
            .is_none(),
        "a diverged orphan is relinquished (dropped from tracking): {after}"
    );
}

/// Path to the out-of-band pi provenance record under the test's taskfleet
/// state root. `bin` sets `TASKFLEET_HOME` to the tempdir root, so the
/// record resolves to `<home>/state/pi-installed-skills.json`.
fn env_orch_state_record(home: &tempfile::TempDir) -> std::path::PathBuf {
    home.path().join("state/pi-installed-skills.json")
}

#[test]
fn skill_install_force_repairs_missing_claude_when_pi_is_current() {
    // All runtimes are first-class no-clobber targets. A partial repair therefore
    // requires --force when another selected runtime already exists.
    let home = mk_home();
    // First install populates both homes.
    assert!(bin(&home)
        .args(["skill", "install", "stint-start"])
        .output()
        .expect("spawn")
        .status
        .success());
    let claude = home.path().join(".claude/skills/stint-start/SKILL.md");
    let pi = home.path().join(".pi/agent/skills/stint-start/SKILL.md");
    assert!(claude.exists() && pi.exists());

    // Delete only the claude copy, leaving the current pi target behind.
    std::fs::remove_dir_all(home.path().join(".claude/skills/stint-start")).unwrap();

    let refused = bin(&home)
        .args(["skill", "install", "stint-start"])
        .output()
        .expect("spawn");
    assert!(!refused.status.success());
    assert!(
        !claude.exists(),
        "failed preflight partially repaired Claude"
    );

    let forced = bin(&home)
        .args(["skill", "install", "stint-start", "--force"])
        .output()
        .expect("spawn");
    assert!(forced.status.success(), "forced repair failed: {forced:?}");
    assert!(claude.exists(), "claude skill was not repaired");
}

#[test]
fn skill_install_self_repairs_missing_pi_mirror_without_force() {
    // The inverse partial state: claude current, pi missing. A plain install
    // refuses the equal-version claude copy (existing contract), so use
    // --force — the refresh must (re)create the pi mirror.
    let home = mk_home();
    assert!(bin(&home)
        .args(["skill", "install", "stint-start"])
        .output()
        .expect("spawn")
        .status
        .success());
    let pi = home.path().join(".pi/agent/skills/stint-start/SKILL.md");
    std::fs::remove_dir_all(home.path().join(".pi/agent/skills/stint-start")).unwrap();
    assert!(!pi.exists());
    assert!(bin(&home)
        .args(["skill", "install", "stint-start", "--force"])
        .output()
        .expect("spawn")
        .status
        .success());
    assert!(pi.exists(), "pi mirror was not recreated by --force");
}

#[test]
fn skill_install_force_refreshes_stale_pi_mirror() {
    // A stale/divergent pi copy is refreshed only under --force.
    let home = mk_home();
    assert!(bin(&home)
        .args(["skill", "install", "stint-start"])
        .output()
        .expect("spawn")
        .status
        .success());
    let pi = home.path().join(".pi/agent/skills/stint-start/SKILL.md");
    std::fs::write(
        &pi,
        "---\nname: stint-start\ncli_version: \"0.0.0\"\n---\nstale\n",
    )
    .unwrap();

    // Non-force: pi is left alone (skipped) and a differ-warning is emitted;
    // but claude is equal-version so the plain plan refuses. Force the whole
    // redeploy and confirm pi is brought back in sync with the binary.
    let out = bin(&home)
        .args([
            "skill",
            "install",
            "stint-start",
            "--force",
            "--output",
            "json",
        ])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "force redeploy failed: {out:?}");
    let after = std::fs::read_to_string(&pi).unwrap();
    assert!(
        after.contains(&format!("cli_version: \"{}\"", env!("CARGO_PKG_VERSION"))),
        "pi mirror was not refreshed to the binary version under --force"
    );
}

#[test]
fn skill_install_divergent_pi_collision_fails_atomically_without_force() {
    // A pre-existing pi file must refuse the whole default-all plan without
    // force; no other selected runtime may be partially installed.
    let home = mk_home();
    let pi_dir = home.path().join(".pi/agent/skills/stint-start");
    std::fs::create_dir_all(&pi_dir).unwrap();
    let pi = pi_dir.join("SKILL.md");
    let user_content = "---\nname: stint-start\ncli_version: \"0.0.0\"\n---\nMINE — do not touch\n";
    std::fs::write(&pi, user_content).unwrap();

    // Fresh claude (no prior install) + differing pi present, no --force.
    let out = bin(&home)
        .args(["skill", "install", "stint-start", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(!out.status.success(), "collision must fail: {out:?}");
    assert_eq!(
        std::fs::read_to_string(&pi).unwrap(),
        user_content,
        "divergent pi file must not be clobbered without --force"
    );
    assert!(!home.path().join(".claude/skills/stint-start").exists());
    assert!(!home.path().join(".codex/prompts/stint-start.md").exists());
}

#[test]
fn skill_install_agent_all_also_dual_homes_into_pi() {
    // `--agent all` installs claude + codex, and still mirrors into pi.
    let home = mk_home();
    let out = bin(&home)
        .args([
            "skill",
            "install",
            "taskfleet-spawn-spinoff",
            "--agent",
            "all",
            "--output",
            "json",
        ])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "agent=all install failed: {out:?}");
    assert!(home
        .path()
        .join(".pi/agent/skills/taskfleet-spawn-spinoff/SKILL.md")
        .exists());
}

#[test]
fn skill_install_agent_codex_does_not_dual_home_into_pi() {
    // pi mirrors the claude corpus; a codex-only install must not touch it.
    let home = mk_home();
    let out = bin(&home)
        .args([
            "skill",
            "install",
            "taskfleet-run-overview",
            "--agent",
            "codex",
            "--output",
            "json",
        ])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "codex install failed: {out:?}");
    assert!(
        !home.path().join(".pi/agent/skills").exists(),
        "codex-only install must not create the pi skill dir"
    );
}

#[test]
fn skill_install_dest_does_not_dual_home_into_pi() {
    // A custom `--dest` is caller-managed; the pi mirror is skipped.
    let home = mk_home();
    let dest = home.path().join("custom/SKILL.md");
    let out = bin(&home)
        .args(["skill", "install", "taskfleet-run-overview", "--dest"])
        .arg(&dest)
        .args(["--output", "json"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "dest install failed: {out:?}");
    assert!(dest.exists(), "dest SKILL.md not on disk");
    assert!(
        !home.path().join(".pi/agent/skills").exists(),
        "--dest install must not create the pi skill dir"
    );
}

#[test]
fn skill_install_all_prunes_managed_orphan() {
    // A managed skill dir that is NOT in the catalog (valid marker naming
    // itself) must be pruned by the full-catalog --force install.
    let home = mk_home();
    let orphan = home.path().join(".claude/skills/gone-skill");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("SKILL.md"), "---\nname: gone-skill\n---\n").unwrap();
    write_marker(&orphan, "gone-skill");

    let out = bin(&home)
        .args(["skill", "install", "--force", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "install-all failed: {out:?}");
    assert!(!orphan.exists(), "managed orphan was not pruned");

    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let pruned: Vec<&str> = v["data"]["pruned"]
        .as_array()
        .expect("pruned array")
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    assert!(pruned.contains(&"gone-skill"), "pruned list: {pruned:?}");
    let warnings = v["warnings"].as_array().expect("warnings");
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("skill_pruned")
                && w.as_str().unwrap().contains("gone-skill")),
        "expected skill_pruned warning; got {warnings:?}"
    );
}

#[test]
fn skill_install_all_spares_unmanaged_same_name_dir() {
    // A user's hand-authored skill dir WITHOUT the marker must never be
    // touched, even though it is not in the catalog.
    let home = mk_home();
    let user_skill = home.path().join(".claude/skills/my-own-skill");
    std::fs::create_dir_all(&user_skill).unwrap();
    std::fs::write(
        user_skill.join("SKILL.md"),
        "---\nname: my-own-skill\n---\nmine\n",
    )
    .unwrap();

    let out = bin(&home)
        .args(["skill", "install", "--force", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "install-all failed: {out:?}");
    assert!(
        user_skill.join("SKILL.md").exists(),
        "unmanaged user skill was deleted — provenance guard failed"
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let pruned = v["data"]["pruned"].as_array().expect("pruned array");
    assert!(
        pruned.is_empty(),
        "unmanaged dir must not appear in pruned: {pruned:?}"
    );
}

#[test]
fn skill_install_all_keeps_registered_skills() {
    // A still-registered skill (installed with its marker) must survive a
    // subsequent full-catalog install — it is not an orphan.
    let home = mk_home();
    assert!(bin(&home)
        .args(["skill", "install", "--force"])
        .output()
        .expect("spawn")
        .status
        .success());
    let registered = home.path().join(".claude/skills/taskfleet-run-overview");
    assert!(registered.join("SKILL.md").exists());
    assert!(registered.join(MARKER).is_file());

    // Second full install must NOT prune the still-registered skill.
    let out = bin(&home)
        .args(["skill", "install", "--force", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "second install failed: {out:?}");
    assert!(
        registered.join("SKILL.md").exists(),
        "still-registered skill was pruned"
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert!(v["data"]["pruned"].as_array().expect("pruned").is_empty());
}

#[test]
fn skill_install_named_does_not_prune() {
    // A targeted `skill install <name>` must NEVER prune the rest of the
    // catalog — even a managed orphan is left alone.
    let home = mk_home();
    let orphan = home.path().join(".claude/skills/gone-skill");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("SKILL.md"), "---\nname: gone-skill\n---\n").unwrap();
    write_marker(&orphan, "gone-skill");

    let out = bin(&home)
        .args([
            "skill",
            "install",
            "taskfleet-run-overview",
            "--force",
            "--output",
            "json",
        ])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "named install failed: {out:?}");
    assert!(
        orphan.exists(),
        "targeted install pruned an orphan — must be scoped to install-all"
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert!(v["data"]["pruned"].as_array().expect("pruned").is_empty());
}

#[test]
fn skill_install_all_without_force_does_not_prune() {
    // Prune is gated on --force: a plain full-catalog install must never
    // delete a directory as a side effect, even a valid managed orphan.
    let home = mk_home();
    let orphan = home.path().join(".claude/skills/gone-skill");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("SKILL.md"), "---\nname: gone-skill\n---\n").unwrap();
    write_marker(&orphan, "gone-skill");

    let out = bin(&home)
        .args(["skill", "install", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "install-all failed: {out:?}");
    assert!(orphan.exists(), "prune ran without --force");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert!(v["data"]["pruned"].as_array().expect("pruned").is_empty());
}

#[test]
fn skill_install_all_spares_copied_and_renamed_managed_skill() {
    // The core data-loss guard: a user copies a managed skill (marker and
    // all) to a new name and edits it. The marker still names the ORIGINAL
    // skill, so it must not match the new dir name — the copy is spared.
    let home = mk_home();
    let copy = home.path().join(".claude/skills/my-worktree");
    std::fs::create_dir_all(&copy).unwrap();
    std::fs::write(copy.join("SKILL.md"), "---\nname: my-worktree\n---\nmine\n").unwrap();
    // Marker copied verbatim from `worktree` — names `worktree`, not the
    // new dir `my-worktree`.
    write_marker(&copy, "worktree");

    let out = bin(&home)
        .args(["skill", "install", "--force", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "install-all failed: {out:?}");
    assert!(
        copy.join("SKILL.md").exists(),
        "a copied-and-renamed managed skill was deleted — name-binding guard failed"
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert!(v["data"]["pruned"].as_array().expect("pruned").is_empty());
}

#[cfg(unix)]
#[test]
fn skill_install_all_does_not_follow_symlinked_orphan() {
    // A symlink under ~/.claude/skills pointing at an outside directory
    // must never be traversed by remove_dir_all, even if the target looks
    // managed. The symlink entry is skipped; the target is untouched.
    let home = mk_home();
    let outside = home.path().join("precious");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("SKILL.md"), "important user data\n").unwrap();
    write_marker(&outside, "evil");

    let skills = home.path().join(".claude/skills");
    std::fs::create_dir_all(&skills).unwrap();
    std::os::unix::fs::symlink(&outside, skills.join("evil")).unwrap();

    let out = bin(&home)
        .args(["skill", "install", "--force", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "install-all failed: {out:?}");
    assert!(
        outside.join("SKILL.md").exists(),
        "remove_dir_all followed a symlink and deleted an outside directory"
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert!(v["data"]["pruned"].as_array().expect("pruned").is_empty());
}

// --- pi.dev mirror lifecycle (out-of-band provenance) --------------------
//
// `TASKFLEET_HOME` and `HOME` both point at the tempdir root here (see
// `bin`), so the provenance record lands at `<home>/state/pi-installed-
// skills.json` and pi mirrors at `<home>/.pi/agent/skills/<name>/`.

/// Path to the out-of-band pi provenance record for this test home.
fn pi_provenance_path(home: &TempDir) -> PathBuf {
    home.path().join("state").join("pi-installed-skills.json")
}

fn read_provenance(home: &TempDir) -> Value {
    let body = std::fs::read_to_string(pi_provenance_path(home)).expect("provenance record");
    serde_json::from_str(&body).expect("provenance json")
}

#[test]
fn skill_install_writes_pi_provenance_record() {
    // Every pi mirror install records the skill's name + content hash in the
    // out-of-band provenance record — the sole signal later prune/doctor keys on.
    let home = mk_home();
    assert!(bin(&home)
        .args(["skill", "install", "stint-start"])
        .output()
        .expect("spawn")
        .status
        .success());

    let prov = read_provenance(&home);
    // v3: the record schema was bumped when it flattened to the per-file `files`
    // map, so an older binary refuses (rather than silently drops) the field.
    assert_eq!(prov["schema_version"], 3);
    let rec = &prov["skills"]["stint-start"];
    assert!(rec.is_object(), "stint-start not recorded: {prov}");
    let sha = rec["files"]["SKILL.md"]["sha256"].as_str().expect("sha256");
    assert_eq!(sha.len(), 64, "sha256 must be 32-byte hex");
    assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(rec["files"]["SKILL.md"]["kind"], "skill");
    assert_eq!(
        rec["cli_version"].as_str().unwrap(),
        env!("CARGO_PKG_VERSION")
    );
}

/// Seed a fake de-registered pi mirror by CLONING a real skill's just-written
/// pi mirror bytes + recorded hash under a name (`gone-skill`) not in the
/// catalog. Reusing a real skill's (bytes, hash) pair means the test needs no
/// sha256 implementation of its own. Returns the seeded mirror path.
fn seed_deregistered_pi_mirror(home: &TempDir, fake: &str, diverge: bool) -> PathBuf {
    // A real install first, so both the provenance record and a real pi mirror
    // (whose bytes hash to the recorded value) exist to clone from.
    assert!(bin(home)
        .args(["skill", "install", "stint-start"])
        .output()
        .expect("spawn")
        .status
        .success());

    let real_mirror = home.path().join(".pi/agent/skills/stint-start/SKILL.md");
    let real_bytes = std::fs::read(&real_mirror).unwrap();

    let mut prov = read_provenance(home);
    let recorded_hash = prov["skills"]["stint-start"]["files"]["SKILL.md"]["sha256"]
        .as_str()
        .unwrap()
        .to_string();

    // Write the fake mirror. If `diverge`, its bytes differ from the recorded
    // hash (simulating a user edit); otherwise they are our exact copy.
    let fake_dir = home.path().join(".pi/agent/skills").join(fake);
    std::fs::create_dir_all(&fake_dir).unwrap();
    let fake_mirror = fake_dir.join("SKILL.md");
    if diverge {
        std::fs::write(&fake_mirror, b"user has taken this over\n").unwrap();
    } else {
        std::fs::write(&fake_mirror, &real_bytes).unwrap();
    }

    // Record the fake skill with the recorded hash of the pristine copy (flat
    // per-file shape: the body is the `SKILL.md` file entry).
    prov["skills"][fake] = serde_json::json!({
        "cli_version": "0.0.1",
        "files": { "SKILL.md": { "sha256": recorded_hash, "kind": "skill" } },
    });
    std::fs::write(
        pi_provenance_path(home),
        serde_json::to_string_pretty(&prov).unwrap(),
    )
    .unwrap();

    fake_mirror
}

#[test]
fn skill_install_force_prunes_deregistered_pi_mirror() {
    // A pi mirror the record names but the catalog no longer ships is pruned by
    // the full-catalog --force install — keyed on the provenance record, and
    // only when the on-disk bytes still hash to the recorded value.
    let home = mk_home();
    let fake_mirror = seed_deregistered_pi_mirror(&home, "gone-skill", false);
    assert!(fake_mirror.exists());

    let out = bin(&home)
        .args(["skill", "install", "--force", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "install-all --force failed: {out:?}");

    assert!(!fake_mirror.exists(), "de-registered pi mirror not pruned");
    assert!(
        !fake_mirror.parent().unwrap().exists(),
        "emptied per-skill dir should be cleaned up"
    );
    // Dropped from the provenance record.
    let prov = read_provenance(&home);
    assert!(
        prov["skills"].get("gone-skill").is_none(),
        "gone-skill still tracked: {prov}"
    );
    // Reported + warned.
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let pruned: Vec<&str> = v["data"]["pruned"]
        .as_array()
        .expect("pruned")
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    assert!(pruned.contains(&"gone-skill"), "pruned: {pruned:?}");
    let warnings = v["warnings"].as_array().expect("warnings");
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("pi_mirror_pruned")
                && w.as_str().unwrap().contains("gone-skill")),
        "expected pi_mirror_pruned warning; got {warnings:?}"
    );
}

#[test]
fn skill_install_force_preserves_diverged_pi_mirror() {
    // A de-registered pi mirror the user has EDITED (bytes no longer hash to the
    // recorded value) is never deleted — taskfleet relinquishes management
    // instead, leaving the file and dropping it from the record.
    let home = mk_home();
    let fake_mirror = seed_deregistered_pi_mirror(&home, "gone-skill", true);

    let out = bin(&home)
        .args(["skill", "install", "--force", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "install-all --force failed: {out:?}");

    assert!(
        fake_mirror.exists(),
        "a user-edited (diverged) pi mirror must NOT be deleted"
    );
    let prov = read_provenance(&home);
    assert!(
        prov["skills"].get("gone-skill").is_none(),
        "diverged mirror should be dropped from tracking"
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let warnings = v["warnings"].as_array().expect("warnings");
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("pi_mirror_diverged")),
        "expected pi_mirror_diverged warning; got {warnings:?}"
    );
    // Never reported as pruned.
    let pruned = v["data"]["pruned"].as_array().expect("pruned");
    assert!(!pruned.iter().any(|p| p == "gone-skill"));
}

#[test]
fn skill_install_fails_closed_on_corrupt_pi_provenance() {
    // A corrupt provenance record must NOT be silently laundered to empty and
    // overwritten (which would erase tracking for every managed pi mirror). The
    // install fails closed, before any file is written, with an actionable code.
    let home = mk_home();
    std::fs::create_dir_all(home.path().join("state")).unwrap();
    std::fs::write(pi_provenance_path(&home), "{ this is not json").unwrap();
    // A skill that would otherwise be installed — prove nothing lands.
    let claude = home.path().join(".claude/skills/stint-start/SKILL.md");

    let out = bin(&home)
        .args(["skill", "install", "stint-start", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(
        !out.status.success(),
        "corrupt record must fail the install"
    );
    // Error envelope is emitted on stderr.
    let v: Value = serde_json::from_slice(&out.stderr).expect("json");
    assert_eq!(v["error"]["code"], "pi_provenance_corrupt");
    assert!(
        !claude.exists(),
        "no file should be written when the record is rejected pre-write"
    );
}

#[test]
fn skill_install_without_force_does_not_prune_pi_mirror() {
    // Prune is gated on --force, symmetric with the claude dir prune: a plain
    // full-catalog install never deletes a pi mirror as a side effect. Seeded
    // manually (no prior real install) so the plain full-catalog install below
    // is a clean first install rather than an overwrite. The seeded hash is
    // arbitrary — a non-force run never inspects it (prune is force-gated).
    let home = mk_home();
    let fake_dir = home.path().join(".pi/agent/skills/gone-skill");
    std::fs::create_dir_all(&fake_dir).unwrap();
    let fake_mirror = fake_dir.join("SKILL.md");
    std::fs::write(&fake_mirror, b"stale de-registered body\n").unwrap();
    std::fs::create_dir_all(home.path().join("state")).unwrap();
    std::fs::write(
        pi_provenance_path(&home),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "skills": { "gone-skill": { "sha256": "deadbeef", "cli_version": "0.0.1" } },
        }))
        .unwrap(),
    )
    .unwrap();

    let out = bin(&home)
        .args(["skill", "install", "--output", "json"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "install-all failed: {out:?}");
    assert!(fake_mirror.exists(), "pi prune ran without --force");
    // Still tracked (union-merge preserves the prior record entry).
    let prov = read_provenance(&home);
    assert!(prov["skills"].get("gone-skill").is_some());
}

#[test]
fn corrupt_marker_traversal_records_never_escape_agent_roots() {
    let home = mk_home();
    assert!(bin(&home)
        .args(["skill", "install", "--agent", "all", "--force"])
        .output()
        .unwrap()
        .status
        .success());

    let claude_victim = home.path().join("victim-claude.txt");
    let codex_victim = home.path().join(".codex/victim-codex.txt");
    std::fs::write(&claude_victim, b"claude user bytes\n").unwrap();
    std::fs::write(&codex_victim, b"codex user bytes\n").unwrap();

    let claude_marker = home
        .path()
        .join(".claude/skills/stint-start/.taskfleet-managed");
    let mut body = std::fs::read_to_string(&claude_marker).unwrap();
    body.push_str("companion: ../../../victim-claude.txt\n");
    std::fs::write(&claude_marker, body).unwrap();

    let codex_marker = home
        .path()
        .join(".codex/prompts/_shared/.taskfleet-managed");
    let mut body = std::fs::read_to_string(&codex_marker).unwrap();
    body.push_str("companion: ../../victim-codex.txt\n");
    std::fs::write(&codex_marker, body).unwrap();

    assert!(bin(&home)
        .args(["skill", "install", "--agent", "all", "--force"])
        .output()
        .unwrap()
        .status
        .success());
    assert_eq!(
        std::fs::read(&claude_victim).unwrap(),
        b"claude user bytes\n"
    );
    assert_eq!(std::fs::read(&codex_victim).unwrap(), b"codex user bytes\n");
}
