use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};
use sysinfo::{Pid, System};
use taskfleet_core::{append_and_apply_event, ensure_root, new_run_id, NodeId, RunPaths};
use tempfile::TempDir;

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

fn command(home: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_taskfleet"))
        .env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path())
        .args(["--output", "json"])
        .args(args)
        .output()
        .unwrap()
}

struct Fixture {
    home: TempDir,
    source: TempDir,
    worktree: PathBuf,
    run_id: String,
    paths: RunPaths,
}

impl Fixture {
    fn failed() -> Self {
        Self::terminal("failed", "retained")
    }

    fn cancelled() -> Self {
        Self::terminal("cancelled", "retained-cancelled")
    }

    fn terminal(status: &str, worktree_name: &str) -> Self {
        let home = TempDir::new().unwrap();
        let source = TempDir::new().unwrap();
        git(source.path(), &["init", "-q", "-b", "main"]);
        git(source.path(), &["config", "user.email", "t@t"]);
        git(source.path(), &["config", "user.name", "t"]);
        std::fs::write(source.path().join("base"), "base\n").unwrap();
        git(source.path(), &["add", "base"]);
        git(source.path(), &["commit", "-qm", "base"]);
        let worktree = source.path().join(worktree_name);
        git(
            source.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "wt/retained",
                worktree.to_str().unwrap(),
            ],
        );

        ensure_root(home.path()).unwrap();
        let run_id = new_run_id();
        let paths = RunPaths::new(home.path().join("runs").join(&run_id), &run_id).unwrap();
        std::fs::create_dir_all(&paths.root).unwrap();
        append_and_apply_event(
            &paths,
            "run.created",
            None,
            None,
            json!({
                "kind": "spinoff", "lifecycle": "autonomous", "title": "retained",
                "source_repo": source.path(), "source_branch": "main"
            }),
        )
        .unwrap();
        let node = NodeId::parse_str("n-0001").unwrap();
        append_and_apply_event(
            &paths,
            "node.created",
            Some(&node),
            None,
            json!({"kind":"spinoff", "worktree_path":worktree, "branch":"wt/retained"}),
        )
        .unwrap();
        let report = if status == "cancelled" {
            json!({"success":false,"cancelled":true,"reason":"cancelled","summary":"cancelled"})
        } else {
            let mut report = json!({"success":false,"summary":"failed"});
            taskfleet_core::ReportOrigin::Supervisor.stamp(&mut report);
            report
        };
        append_and_apply_event(&paths, "node.report", Some(&node), None, report).unwrap();
        append_and_apply_event(&paths, "run.status", None, None, json!({"status":status})).unwrap();
        Self {
            home,
            source,
            worktree,
            run_id,
            paths,
        }
    }

    fn show(&self) -> Value {
        let out = command(&self.home, &["run", "show", &self.run_id]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

#[test]
fn show_reports_clean_dirty_and_absent_current_resources() {
    let f = Fixture::failed();
    let clean = f.show();
    assert_eq!(clean["data"]["preserved_work"][0]["cleanliness"], "clean");
    assert_eq!(
        clean["data"]["preserved_work"][0]["verification"],
        "verified"
    );

    std::fs::write(f.worktree.join("untracked"), "do not lose\n").unwrap();
    git(&f.worktree, &["config", "status.showUntrackedFiles", "no"]);
    let dirty = f.show();
    assert_eq!(dirty["data"]["preserved_work"][0]["cleanliness"], "dirty");

    git(
        f.source.path(),
        &[
            "worktree",
            "remove",
            "--force",
            f.worktree.to_str().unwrap(),
        ],
    );
    git(f.source.path(), &["branch", "-D", "--", "wt/retained"]);
    assert_eq!(f.show()["data"]["preserved_work"], json!([]));
}

#[test]
fn dry_run_previews_dirty_resources_without_requiring_force_or_mutating() {
    let f = Fixture::failed();
    std::fs::write(f.worktree.join("untracked"), "keep\n").unwrap();
    let out = command(
        &f.home,
        &[
            "run",
            "discard",
            &f.run_id,
            "--reason",
            "superseded",
            "--dry-run",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["data"]["force_required"], true);
    assert_eq!(value["data"]["cleanliness"], "dirty");
    assert_eq!(
        value["data"]["worktree_path"],
        f.worktree.display().to_string()
    );
    assert!(f.worktree.exists());
    let events = std::fs::read_to_string(f.paths.events()).unwrap();
    assert!(!events.contains("cleanup.discard_authorized"));
}

#[test]
fn clean_discard_is_audited_idempotent_and_does_not_change_terminal_truth() {
    let f = Fixture::failed();
    let args = ["run", "discard", &f.run_id, "--reason", "superseded"];
    let out = command(&f.home, &args);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["data"]["discarded"], true);
    assert!(!f.worktree.exists());
    assert!(!Command::new("git")
        .arg("-C")
        .arg(f.source.path())
        .args(["show-ref", "--verify", "--quiet", "refs/heads/wt/retained"])
        .status()
        .unwrap()
        .success());
    let events = std::fs::read_to_string(f.paths.events()).unwrap();
    assert_eq!(events.matches("cleanup.discard_authorized").count(), 1);
    assert_eq!(f.show()["data"]["status"], "failed");
    assert_eq!(f.show()["data"]["landed"], false);

    let retry = command(&f.home, &args);
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    let retry: Value = serde_json::from_slice(&retry.stdout).unwrap();
    assert_eq!(retry["data"]["already_discarded"], true);
    assert_eq!(
        std::fs::read_to_string(f.paths.events())
            .unwrap()
            .matches("cleanup.discard_authorized")
            .count(),
        1
    );
}

#[test]
fn dirty_discard_requires_force_then_removes_untracked_content() {
    let f = Fixture::failed();
    std::fs::write(f.worktree.join("untracked"), "keep\n").unwrap();
    let refused = command(
        &f.home,
        &["run", "discard", &f.run_id, "--reason", "superseded"],
    );
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("force_required"));
    assert!(!std::fs::read_to_string(f.paths.events())
        .unwrap()
        .contains("cleanup.discard_authorized"));

    let forced = command(
        &f.home,
        &[
            "run",
            "discard",
            &f.run_id,
            "--reason",
            "superseded",
            "--force",
        ],
    );
    assert!(
        forced.status.success(),
        "{}",
        String::from_utf8_lossy(&forced.stderr)
    );
    assert!(!f.worktree.exists());
}

#[test]
fn cancelled_dirty_work_is_visible_and_force_discard_keeps_cancelled_truth() {
    let f = Fixture::cancelled();
    std::fs::write(f.worktree.join("untracked"), "keep\n").unwrap();
    let shown = f.show();
    assert_eq!(shown["data"]["status"], "cancelled");
    assert_eq!(shown["data"]["preserved_work"][0]["cleanliness"], "dirty");

    let out = command(
        &f.home,
        &[
            "run",
            "discard",
            &f.run_id,
            "--reason",
            "superseded",
            "--force",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let shown = f.show();
    assert_eq!(shown["data"]["status"], "cancelled");
    assert_eq!(shown["data"]["landed"], false);
    assert_eq!(shown["data"]["preserved_work"], json!([]));
}

#[test]
fn detached_and_whitespace_named_registered_worktrees_are_exactly_discardable() {
    for name in ["detached", "trailing "] {
        let f = Fixture::terminal("failed", name);
        if name == "detached" {
            git(&f.worktree, &["checkout", "--detach", "-q"]);
            std::fs::write(f.worktree.join("detached-change"), "work\n").unwrap();
            git(&f.worktree, &["add", "detached-change"]);
            git(&f.worktree, &["commit", "-qm", "detached work"]);
        }
        let shown = f.show();
        assert_eq!(
            shown["data"]["preserved_work"][0]["verification"],
            "verified"
        );
        if name == "detached" {
            assert_eq!(shown["data"]["preserved_work"][0]["unmerged_commits"], 1);
        }
        let out = command(
            &f.home,
            &["run", "discard", &f.run_id, "--reason", "superseded"],
        );
        assert!(
            out.status.success(),
            "{name:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!f.worktree.exists());
    }
}

#[test]
fn stale_registration_replaced_by_another_repo_refuses_before_authorization() {
    let f = Fixture::failed();
    std::fs::remove_dir_all(&f.worktree).unwrap();
    std::fs::create_dir(&f.worktree).unwrap();
    git(&f.worktree, &["init", "-q"]);

    let out = command(
        &f.home,
        &[
            "run",
            "discard",
            &f.run_id,
            "--reason",
            "superseded",
            "--force",
        ],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("preserved_work_unverifiable"));
    assert!(f.worktree.exists());
    assert!(!std::fs::read_to_string(f.paths.events())
        .unwrap()
        .contains("cleanup.discard_authorized"));
}

fn rewind_terminal_projections_to_seq_two(f: &Fixture) {
    let manifest_path = f.paths.manifest();
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["status"] = json!("running");
    manifest["applied_seq"] = json!(2);
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let node_path = f.paths.node(&NodeId::parse_str("n-0001").unwrap());
    let mut node: Value = serde_json::from_slice(&std::fs::read(&node_path).unwrap()).unwrap();
    node["status"] = json!("pending");
    node["last_report"] = Value::Null;
    std::fs::write(node_path, serde_json::to_vec_pretty(&node).unwrap()).unwrap();
}

#[test]
fn legacy_and_invalid_node_refusals_precede_any_replay_or_torn_tail_mutation() {
    let f = Fixture::failed();
    let manifest_path = f.paths.manifest();
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["kind"] = json!("code");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .append(true)
        .open(f.paths.events())
        .unwrap()
        .write_all(br#"{"seq":999,"kind":"run.status""#)
        .unwrap();
    let node_path = f.paths.node(&NodeId::parse_str("n-0001").unwrap());
    let before = (
        std::fs::read(f.paths.events()).unwrap(),
        std::fs::read(&manifest_path).unwrap(),
        std::fs::read(&node_path).unwrap(),
    );

    let legacy = command(
        &f.home,
        &["run", "discard", &f.run_id, "--reason", "superseded"],
    );
    assert!(!legacy.status.success());
    assert!(String::from_utf8_lossy(&legacy.stderr).contains("legacy_run_read_only"));
    assert_eq!(std::fs::read(f.paths.events()).unwrap(), before.0);
    assert_eq!(std::fs::read(&manifest_path).unwrap(), before.1);
    assert_eq!(std::fs::read(&node_path).unwrap(), before.2);

    let invalid = command(
        &f.home,
        &[
            "run",
            "discard",
            &f.run_id,
            "--node",
            "not/a/node",
            "--reason",
            "superseded",
        ],
    );
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("invalid_node_id"));
    assert_eq!(std::fs::read(f.paths.events()).unwrap(), before.0);
    assert_eq!(std::fs::read(&manifest_path).unwrap(), before.1);
    assert_eq!(std::fs::read(&node_path).unwrap(), before.2);
}

#[test]
fn normal_discard_replays_unapplied_terminal_eligibility_but_dry_run_is_byte_read_only() {
    let dry = Fixture::failed();
    rewind_terminal_projections_to_seq_two(&dry);
    let event_before = std::fs::read(dry.paths.events()).unwrap();
    let manifest_before = std::fs::read(dry.paths.manifest()).unwrap();
    let node_before = std::fs::read(dry.paths.node(&NodeId::parse_str("n-0001").unwrap())).unwrap();
    let out = command(
        &dry.home,
        &[
            "run",
            "discard",
            &dry.run_id,
            "--reason",
            "superseded",
            "--dry-run",
        ],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("projection_stale"));
    assert_eq!(std::fs::read(dry.paths.events()).unwrap(), event_before);
    assert_eq!(
        std::fs::read(dry.paths.manifest()).unwrap(),
        manifest_before
    );
    assert_eq!(
        std::fs::read(dry.paths.node(&NodeId::parse_str("n-0001").unwrap())).unwrap(),
        node_before
    );

    let apply = Fixture::failed();
    rewind_terminal_projections_to_seq_two(&apply);
    let out = command(
        &apply.home,
        &["run", "discard", &apply.run_id, "--reason", "superseded"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!apply.worktree.exists());
}

#[test]
fn positively_live_worker_overrides_stale_told_exit_before_authorization() {
    let f = Fixture::failed();
    let pid = std::process::id();
    let system = System::new_all();
    let start = chrono::DateTime::from_timestamp(
        system.process(Pid::from_u32(pid)).unwrap().start_time() as i64,
        0,
    )
    .unwrap();
    let node_path = f.paths.node(&NodeId::parse_str("n-0001").unwrap());
    let mut node: Value = serde_json::from_slice(&std::fs::read(&node_path).unwrap()).unwrap();
    node["agent_pid"] = json!(pid);
    node["agent_pid_start_time"] = json!(start);
    node["worker_exit"] = json!({"code":0,"signal":null,"at":chrono::Utc::now()});
    std::fs::write(node_path, serde_json::to_vec_pretty(&node).unwrap()).unwrap();

    let out = command(
        &f.home,
        &["run", "discard", &f.run_id, "--reason", "superseded"],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("worker_live"));
    assert!(f.worktree.exists());
    assert!(!std::fs::read_to_string(f.paths.events())
        .unwrap()
        .contains("cleanup.discard_authorized"));
}

#[test]
fn interrupted_branch_only_retry_preserves_original_authorization_contract() {
    let f = Fixture::failed();
    let node = NodeId::parse_str("n-0001").unwrap();
    append_and_apply_event(
        &f.paths,
        "cleanup.discard_authorized",
        Some(&node),
        Some(&format!("discard-authorized:{}:n-0001", f.run_id)),
        json!({
            "actor":"uid:recorded", "reason":"superseded", "force":false,
            "worktree_path":f.worktree, "branch":"wt/retained",
            "cleanliness":"clean", "unmerged_commits":0
        }),
    )
    .unwrap();
    git(
        f.source.path(),
        &["worktree", "remove", f.worktree.to_str().unwrap()],
    );

    let conflict = command(
        &f.home,
        &["run", "discard", &f.run_id, "--reason", "changed"],
    );
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("idempotency_conflict"));
    let retry = command(
        &f.home,
        &["run", "discard", &f.run_id, "--reason", "superseded"],
    );
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    let retry: Value = serde_json::from_slice(&retry.stdout).unwrap();
    assert_eq!(retry["data"]["audit_event"]["actor"], "uid:recorded");
    assert_eq!(retry["data"]["removed"]["worktree"], false);
    assert_eq!(retry["data"]["removed"]["branch"], true);
}

fn add_failed_node(f: &Fixture, id: &str, branch: &str, dirname: &str) -> PathBuf {
    let path = f.source.path().join(dirname);
    git(
        f.source.path(),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            branch,
            path.to_str().unwrap(),
        ],
    );
    let node = NodeId::parse_str(id).unwrap();
    append_and_apply_event(
        &f.paths,
        "node.created",
        Some(&node),
        None,
        json!({"kind":"spinoff", "worktree_path":path, "branch":branch}),
    )
    .unwrap();
    let mut report = json!({"success":false,"summary":"failed"});
    taskfleet_core::ReportOrigin::Supervisor.stamp(&mut report);
    append_and_apply_event(&f.paths, "node.report", Some(&node), None, report).unwrap();
    path
}

#[test]
fn current_candidates_beat_history_and_two_current_nodes_require_selection() {
    let historical = Fixture::failed();
    let n1 = NodeId::parse_str("n-0001").unwrap();
    append_and_apply_event(
        &historical.paths,
        "cleanup.discard_authorized",
        Some(&n1),
        Some(&format!("discard-authorized:{}:n-0001", historical.run_id)),
        json!({
            "actor":"uid:recorded", "reason":"old", "force":false,
            "worktree_path":historical.worktree, "branch":"wt/retained",
            "cleanliness":"clean", "unmerged_commits":0
        }),
    )
    .unwrap();
    git(
        historical.source.path(),
        &["worktree", "remove", historical.worktree.to_str().unwrap()],
    );
    git(
        historical.source.path(),
        &["branch", "-D", "--", "wt/retained"],
    );
    let second = add_failed_node(&historical, "n-0002", "wt/second", "second");
    let out = command(
        &historical.home,
        &["run", "discard", &historical.run_id, "--reason", "new"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["data"]["node_id"], "n-0002");
    assert!(!second.exists());
    let auth_count = std::fs::read_to_string(historical.paths.events())
        .unwrap()
        .matches("cleanup.discard_authorized")
        .count();
    let retry = command(
        &historical.home,
        &["run", "discard", &historical.run_id, "--reason", "new"],
    );
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    let retry: Value = serde_json::from_slice(&retry.stdout).unwrap();
    assert_eq!(retry["data"]["node_id"], "n-0002");
    assert_eq!(retry["data"]["already_discarded"], true);
    assert_eq!(
        retry["data"]["removed"],
        json!({"worktree":false,"branch":false})
    );
    assert_eq!(
        std::fs::read_to_string(historical.paths.events())
            .unwrap()
            .matches("cleanup.discard_authorized")
            .count(),
        auth_count
    );

    let ambiguous = Fixture::failed();
    let second = add_failed_node(&ambiguous, "n-0002", "wt/second", "second");
    let out = command(
        &ambiguous.home,
        &[
            "run",
            "discard",
            &ambiguous.run_id,
            "--reason",
            "superseded",
        ],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("ambiguous_preserved_work"));
    assert!(ambiguous.worktree.exists());
    assert!(second.exists());
}

#[test]
fn incomplete_authorization_rejects_changed_inputs_and_reuses_recorded_actor() {
    let f = Fixture::failed();
    let node = NodeId::parse_str("n-0001").unwrap();
    append_and_apply_event(
        &f.paths,
        "cleanup.discard_authorized",
        Some(&node),
        Some(&format!("discard-authorized:{}:n-0001", f.run_id)),
        json!({
            "actor":"uid:recorded", "reason":"superseded", "force":false,
            "worktree_path":f.worktree, "branch":"wt/retained",
            "cleanliness":"clean", "unmerged_commits":0
        }),
    )
    .unwrap();
    let conflict = command(
        &f.home,
        &["run", "discard", &f.run_id, "--reason", "different"],
    );
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("idempotency_conflict"));

    let retry = command(
        &f.home,
        &["run", "discard", &f.run_id, "--reason", "superseded"],
    );
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    let retry: Value = serde_json::from_slice(&retry.stdout).unwrap();
    assert_eq!(retry["data"]["audit_event"]["actor"], "uid:recorded");
}
