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
        let report = if status == "done" {
            json!({"success":true,"summary":"external delivery complete"})
        } else if status == "cancelled" {
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
fn done_report_only_retained_work_is_visible_and_wait_grades_it() {
    let f = Fixture::terminal("done", "report-only");
    std::fs::write(f.worktree.join("delivery"), "delivered\n").unwrap();
    git(&f.worktree, &["add", "delivery"]);
    git(&f.worktree, &["commit", "-qm", "delivery"]);
    let shown = f.show();
    let retained = &shown["data"]["preserved_work"][0];
    assert_eq!(shown["data"]["status"], "done");
    assert_eq!(shown["data"]["landed"], false);
    assert_eq!(retained["unmerged_commits"], 1);
    assert_eq!(retained["reason"], "done-with-unmerged-work");
    let out = command(&f.home, &["run", "wait", &f.run_id, "--fail-on-error"]);
    assert_eq!(out.status.code(), Some(3));
    let wait: Value = serde_json::from_slice(&out.stdout).unwrap();
    let row = &wait["data"]["runs"][0];
    assert_eq!(row["report_only"], true);
    assert_eq!(row["merged"], false);
    assert_eq!(
        row["preserved_work"][0]["reason"],
        "done-with-unmerged-work"
    );
}

#[test]
fn done_report_only_without_remaining_resources_is_acknowledged_without_error() {
    // The origin-less success report is also a legacy replay fixture.
    let f = Fixture::terminal("done", "no-work");
    git(
        f.source.path(),
        &["worktree", "remove", f.worktree.to_str().unwrap()],
    );
    git(f.source.path(), &["branch", "-d", "--", "wt/retained"]);
    assert_eq!(f.show()["data"]["preserved_work"], json!([]));
    let out = command(&f.home, &["run", "wait", &f.run_id, "--fail-on-error"]);
    assert!(out.status.success());
    let wait: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(wait["data"]["runs"][0]["report_only"], true);
    assert_eq!(wait["data"]["runs"][0]["preserved_work"], json!([]));
}

#[test]
fn show_reports_clean_dirty_and_absent_current_resources() {
    let f = Fixture::failed();
    let clean = f.show();
    assert_eq!(clean["data"]["preserved_work"][0]["cleanliness"], "clean");
    assert_eq!(
        clean["data"]["preserved_work"][0]["reason"],
        "terminal-work-preserved"
    );
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
    let oid = git(f.source.path(), &["rev-parse", "refs/heads/wt/retained"]);
    let node = NodeId::parse_str("n-0001").unwrap();
    append_and_apply_event(
        &f.paths,
        "cleanup.discard_authorized",
        Some(&node),
        Some(&format!("discard-authorized:{}:n-0001", f.run_id)),
        json!({
            "actor":"uid:recorded", "reason":"superseded", "force":false,
            "worktree_path":f.worktree, "branch":"wt/retained",
            "cleanliness":"clean", "unmerged_commits":0, "branch_oid":oid
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
    let oid = git(f.source.path(), &["rev-parse", "refs/heads/wt/retained"]);
    let node = NodeId::parse_str("n-0001").unwrap();
    append_and_apply_event(
        &f.paths,
        "cleanup.discard_authorized",
        Some(&node),
        Some(&format!("discard-authorized:{}:n-0001", f.run_id)),
        json!({
            "actor":"uid:recorded", "reason":"superseded", "force":false,
            "worktree_path":f.worktree, "branch":"wt/retained",
            "cleanliness":"clean", "unmerged_commits":0, "branch_oid":oid
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

fn envelope(out: &Output, success: bool) -> Value {
    assert_eq!(
        out.status.success(),
        success,
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stream = if success { &out.stdout } else { &out.stderr };
    assert!(
        if success {
            out.stderr.is_empty()
        } else {
            out.stdout.is_empty()
        },
        "unexpected output: {out:?}"
    );
    let mut de = serde_json::Deserializer::from_slice(stream).into_iter::<Value>();
    let value = de.next().unwrap().unwrap();
    assert!(de.next().is_none(), "multiple envelopes: {out:?}");
    value
}

#[cfg(unix)]
#[test]
fn failed_removal_deregisters_then_same_reason_resumes_without_erasing_unrelated_work() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::failed();
    let readonly = f.worktree.join("readonly");
    std::fs::create_dir(&readonly).unwrap();
    std::fs::write(readonly.join("file"), "recover me").unwrap();
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o500)).unwrap();
    let args = [
        "run", "discard", &f.run_id, "--reason", "operator", "--force",
    ];
    let first = envelope(&command(&f.home, &args), false);
    assert_eq!(first["error"]["code"], "discard_partial_cleanup");
    assert!(readonly.join("file").exists());
    assert!(!git(f.source.path(), &["worktree", "list", "--porcelain"])
        .contains(&f.worktree.display().to_string()));
    let auth = std::fs::read_to_string(f.paths.events()).unwrap();
    assert_eq!(auth.matches("cleanup.discard_authorized").count(), 1);
    let changed = envelope(
        &command(
            &f.home,
            &["run", "discard", &f.run_id, "--reason", "other", "--force"],
        ),
        false,
    );
    assert_eq!(changed["error"]["code"], "idempotency_conflict");
    let no_force = envelope(
        &command(
            &f.home,
            &["run", "discard", &f.run_id, "--reason", "operator"],
        ),
        false,
    );
    assert_eq!(no_force["error"]["code"], "idempotency_conflict");
    let retry = envelope(&command(&f.home, &args), false);
    assert_eq!(retry["error"]["code"], "discard_partial_cleanup");
    assert!(readonly.join("file").exists());
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o700)).unwrap();
    let done = envelope(&command(&f.home, &args), true);
    assert_eq!(
        done["data"]["removed"],
        json!({"worktree":true,"branch":true})
    );
    assert!(!f.worktree.exists());
    assert_eq!(
        envelope(&command(&f.home, &args), true)["data"]["already_discarded"],
        true
    );
    assert_eq!(
        std::fs::read_to_string(f.paths.events())
            .unwrap()
            .matches("cleanup.discard_authorized")
            .count(),
        1
    );
}

#[cfg(unix)]
#[test]
fn orphan_replacement_and_moved_branch_are_not_discarded() {
    use std::os::unix::fs::PermissionsExt;
    for replace in [true, false] {
        let f = Fixture::failed();
        let readonly = f.worktree.join("readonly");
        std::fs::create_dir(&readonly).unwrap();
        std::fs::write(readonly.join("file"), "valuable").unwrap();
        std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o500)).unwrap();
        let args = [
            "run", "discard", &f.run_id, "--reason", "operator", "--force",
        ];
        assert_eq!(
            envelope(&command(&f.home, &args), false)["error"]["code"],
            "discard_partial_cleanup"
        );
        std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o700)).unwrap();
        if replace {
            std::fs::remove_dir_all(&f.worktree).unwrap();
            std::fs::create_dir(&f.worktree).unwrap();
            std::fs::write(f.worktree.join("new"), "not ours").unwrap();
        } else {
            let old = git(f.source.path(), &["rev-parse", "refs/heads/wt/retained"]);
            let new = git(
                f.source.path(),
                &[
                    "commit-tree",
                    &format!("{old}^{{tree}}"),
                    "-p",
                    &old,
                    "-m",
                    "new",
                ],
            );
            git(
                f.source.path(),
                &["update-ref", "refs/heads/wt/retained", &new, &old],
            );
        }
        assert_eq!(
            envelope(&command(&f.home, &args), false)["error"]["code"],
            "preserved_work_unverifiable"
        );
        assert!(if replace {
            f.worktree.join("new").exists()
        } else {
            readonly.join("file").exists()
        });
        assert_eq!(
            std::fs::read_to_string(f.paths.events())
                .unwrap()
                .matches("cleanup.discard_authorized")
                .count(),
            1
        );
    }
}

#[cfg(unix)]
#[test]
fn retry_accepts_only_stale_marker_pointing_into_original_git_common_dir() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::failed();
    let marker = std::fs::read_to_string(f.worktree.join(".git")).unwrap();
    let readonly = f.worktree.join("readonly");
    std::fs::create_dir(&readonly).unwrap();
    std::fs::write(readonly.join("file"), "x").unwrap();
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o500)).unwrap();
    let args = [
        "run", "discard", &f.run_id, "--reason", "operator", "--force",
    ];
    assert_eq!(
        envelope(&command(&f.home, &args), false)["error"]["code"],
        "discard_partial_cleanup"
    );
    std::fs::write(
        f.worktree.join(".git"),
        "gitdir: /var/tmp/other/.git/worktrees/not-ours\n",
    )
    .unwrap();
    assert_eq!(
        envelope(&command(&f.home, &args), false)["error"]["code"],
        "preserved_work_unverifiable"
    );
    std::fs::write(f.worktree.join(".git"), marker).unwrap();
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        envelope(&command(&f.home, &args), true)["data"]["discarded"],
        true
    );
}

#[test]
fn branch_only_discard_pins_ref_even_when_checkout_is_already_absent() {
    let f = Fixture::failed();
    git(
        f.source.path(),
        &["worktree", "remove", f.worktree.to_str().unwrap()],
    );
    // Register the branch elsewhere to make the first deletion refuse, after
    // recording a branch-only receipt. Then move its tip before retrying.
    let other = f.source.path().join("other");
    git(
        f.source.path(),
        &[
            "worktree",
            "add",
            "-q",
            other.to_str().unwrap(),
            "wt/retained",
        ],
    );
    let args = ["run", "discard", &f.run_id, "--reason", "operator"];
    // Initial observation has no worktree and sees a branch checked out elsewhere;
    // the guard refuses branch removal after persisting the authorization.
    assert_eq!(
        envelope(&command(&f.home, &args), false)["error"]["code"],
        "discard_partial_cleanup"
    );
    git(
        f.source.path(),
        &["worktree", "remove", other.to_str().unwrap()],
    );
    let old = git(f.source.path(), &["rev-parse", "refs/heads/wt/retained"]);
    let moved = git(
        f.source.path(),
        &[
            "commit-tree",
            &format!("{old}^{{tree}}"),
            "-p",
            &old,
            "-m",
            "moved",
        ],
    );
    git(
        f.source.path(),
        &["update-ref", "refs/heads/wt/retained", &moved, &old],
    );
    assert_eq!(
        envelope(&command(&f.home, &args), false)["error"]["code"],
        "discard_partial_cleanup"
    );
    assert_eq!(
        git(f.source.path(), &["rev-parse", "refs/heads/wt/retained"]),
        moved
    );
}

#[cfg(unix)]
#[test]
fn clean_nonforce_partial_retry_preserves_new_files_until_force_upgrade() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::failed();
    let readonly = f.worktree.join("readonly");
    std::fs::create_dir(&readonly).unwrap();
    std::fs::write(readonly.join("tracked"), "initial").unwrap();
    git(&f.worktree, &["add", "readonly/tracked"]);
    git(&f.worktree, &["commit", "-qm", "tracked work"]);
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o500)).unwrap();
    let plain = ["run", "discard", &f.run_id, "--reason", "operator"];
    assert_eq!(
        envelope(&command(&f.home, &plain), false)["error"]["code"],
        "discard_partial_cleanup"
    );
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(readonly.join("new-after-discard"), "new work").unwrap();
    assert_eq!(
        envelope(&command(&f.home, &plain), false)["error"]["code"],
        "force_required"
    );
    assert!(readonly.join("new-after-discard").exists());
    let forced = [
        "run", "discard", &f.run_id, "--reason", "operator", "--force",
    ];
    assert_eq!(
        envelope(&command(&f.home, &forced), true)["data"]["discarded"],
        true
    );
    assert!(!f.worktree.exists());
    assert_eq!(
        envelope(&command(&f.home, &forced), true)["data"]["already_discarded"],
        true
    );
    let events = std::fs::read_to_string(f.paths.events()).unwrap();
    assert_eq!(events.matches("cleanup.discard_authorized").count(), 1);
    assert_eq!(
        events.matches("cleanup.discard_force_authorized").count(),
        1
    );
}

#[test]
fn legacy_branch_only_receipt_without_oid_preserves_branch() {
    let f = Fixture::failed();
    let node = NodeId::parse_str("n-0001").unwrap();
    append_and_apply_event(&f.paths, "cleanup.discard_authorized", Some(&node),
        Some(&format!("discard-authorized:{}:n-0001", f.run_id)),
        json!({"actor":"uid:old", "reason":"operator", "force":false,
            "worktree_path":f.worktree,"branch":"wt/retained", "cleanliness":"clean","unmerged_commits":0})).unwrap();
    git(
        f.source.path(),
        &["worktree", "remove", f.worktree.to_str().unwrap()],
    );
    assert_eq!(
        envelope(
            &command(
                &f.home,
                &["run", "discard", &f.run_id, "--reason", "operator"]
            ),
            false
        )["error"]["code"],
        "preserved_work_unverifiable"
    );
    assert_ne!(
        git(f.source.path(), &["branch", "--list", "wt/retained"]),
        ""
    );
}

/// Real initialized submodule, with a space in the gitlink path. No network or
/// ambient file-protocol configuration is needed.
fn populate_submodule(f: &Fixture) {
    let module = f.home.path().join("module-source");
    std::fs::create_dir(&module).unwrap();
    git(&module, &["init", "-q", "-b", "main"]);
    git(&module, &["config", "user.email", "t@t"]);
    git(&module, &["config", "user.name", "t"]);
    std::fs::write(module.join("tracked"), "module\n").unwrap();
    git(&module, &["add", "."]);
    git(&module, &["commit", "-qm", "module"]);
    git(
        &f.worktree,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "--",
            module.to_str().unwrap(),
            "module space",
        ],
    );
    git(&f.worktree, &["commit", "-qam", "add module"]);
    assert_eq!(git(&f.worktree, &["status", "--porcelain"]), "");
}

/// Emulate an older release's missing modules-storage preflight, then fail
/// removal to retain its production-generated inode/repository/OID receipt.
/// All retained-resource and identity checks still run real Git.
fn record_nonforce_receipt(f: &Fixture) {
    use std::os::unix::fs::PermissionsExt;
    let real_git = git_binary();
    let stub = f.home.path().join("git-refuse-remove");
    std::fs::write(&stub, format!("#!/bin/sh\nif [ \"$3\" = rev-parse ] && [ \"$5\" = --git-path ]; then printf '%s\\n' '{}/absent-modules'; exit 0; fi\nif [ \"$3\" = worktree ] && [ \"$4\" = remove ]; then echo 'fixture removal refusal' >&2; exit 1; fi\nexec '{}' \"$@\"\n", f.home.path().display(), real_git.display())).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_taskfleet"))
        .env("TASKFLEET_HOME", f.home.path())
        .env("HOME", f.home.path())
        .env("GIT_BIN", &stub)
        .args([
            "--json", "run", "discard", &f.run_id, "--reason", "operator",
        ])
        .output()
        .unwrap();
    let error = envelope(&out, false);
    assert_eq!(error["error"]["code"], "discard_partial_cleanup");
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("fixture removal refusal"));
}

fn git_binary() -> PathBuf {
    // Resolve before PATH is stripped; only Git (and /bin/sh for the declared
    // refusal stub) is needed by these production CLI invocations.
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|p| p.join("git"))
        .find(|p| p.is_file())
        .unwrap()
}

fn isolated_discard(f: &Fixture, extra: &[&str]) -> Output {
    let bin = f.home.path().join("only-git");
    std::fs::create_dir_all(&bin).unwrap();
    let git = bin.join("git");
    if !git.exists() {
        std::os::unix::fs::symlink(git_binary(), &git).unwrap();
    }
    Command::new(env!("CARGO_BIN_EXE_taskfleet"))
        .env("TASKFLEET_HOME", f.home.path())
        .env("HOME", f.home.path())
        .env("PATH", bin)
        .env("GIT_BIN", &git)
        .args([
            "--json", "run", "discard", &f.run_id, "--reason", "operator",
        ])
        .args(extra)
        .output()
        .unwrap()
}

#[test]
fn populated_submodule_discard_previews_and_requires_explicit_force() {
    let f = Fixture::failed();
    populate_submodule(&f);
    // Independently exercise the underlying refusal, not a mock.
    let refused = Command::new("git")
        .arg("-C")
        .arg(f.source.path())
        .args(["worktree", "remove"])
        .arg(&f.worktree)
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(f.worktree.join("module space/tracked").exists());
    let before = std::fs::read(f.paths.events()).unwrap();
    let preview = envelope(&isolated_discard(&f, &["--dry-run"]), true);
    assert_eq!(preview["data"]["cleanliness"], "clean");
    assert_eq!(preview["data"]["force_required"], true);
    assert_eq!(
        envelope(&isolated_discard(&f, &[]), false)["error"]["code"],
        "force_required"
    );
    assert_eq!(std::fs::read(f.paths.events()).unwrap(), before);
    assert_eq!(
        envelope(&isolated_discard(&f, &["--force"]), true)["data"]["discarded"],
        true
    );
    assert!(!f.worktree.exists());
    assert_eq!(
        envelope(&isolated_discard(&f, &["--force"]), true)["data"]["already_discarded"],
        true
    );
}

#[test]
fn existing_nonforce_submodule_receipt_supports_audited_force_upgrade() {
    let f = Fixture::failed();
    populate_submodule(&f);
    // Deinit leaves empty gitlink directories but retains module storage.
    // The stub emulates the old release's missing storage preflight.
    git(&f.worktree, &["submodule", "deinit", "-f", "--all"]);
    record_nonforce_receipt(&f);
    git(
        &f.worktree,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "update",
            "--init",
        ],
    );
    let before = std::fs::read(f.paths.events()).unwrap();
    for extra in [&["--dry-run"][..], &["--force", "--dry-run"][..]] {
        assert_eq!(
            envelope(&isolated_discard(&f, extra), true)["data"]["force_required"],
            true
        );
        assert_eq!(std::fs::read(f.paths.events()).unwrap(), before);
    }
    assert_eq!(
        envelope(&isolated_discard(&f, &[]), false)["error"]["code"],
        "force_required"
    );
    let done = envelope(&isolated_discard(&f, &["--force"]), true);
    assert_eq!(done["data"]["discarded"], true);
    let events = taskfleet_core::read_all_events(&f.paths.events()).unwrap();
    let original = events
        .iter()
        .find(|e| e.kind == "cleanup.discard_authorized")
        .unwrap();
    assert_eq!(original.data["force"], false);
    let upgrades: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "cleanup.discard_force_authorized")
        .collect();
    assert_eq!(upgrades.len(), 1);
    assert_eq!(upgrades[0].data["authorization_seq"], original.seq);
    assert_eq!(upgrades[0].data["reason"], "operator");
    assert_eq!(
        envelope(&isolated_discard(&f, &["--force"]), true)["data"]["already_discarded"],
        true
    );
}

#[test]
fn submodule_force_upgrade_preserves_changed_target_and_dirty_work() {
    for mutation in ["oid", "directory", "dirty", "reason"] {
        let f = Fixture::failed();
        populate_submodule(&f);
        git(&f.worktree, &["submodule", "deinit", "-f", "--all"]);
        record_nonforce_receipt(&f);
        git(
            &f.worktree,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--init",
            ],
        );
        match mutation {
            "oid" => {
                git(&f.worktree, &["commit", "--allow-empty", "-qm", "new tip"]);
            }
            "directory" => {
                std::fs::rename(&f.worktree, f.source.path().join("saved")).unwrap();
                std::fs::create_dir(&f.worktree).unwrap();
                std::fs::write(f.worktree.join("valuable"), "replacement").unwrap();
            }
            "dirty" => {
                std::fs::write(f.worktree.join("untracked"), "new work").unwrap();
            }
            "reason" => {}
            _ => unreachable!(),
        }
        let before = std::fs::read(f.paths.events()).unwrap();
        for dry in [true, false] {
            let mut extra = vec!["--force"];
            if dry {
                extra.push("--dry-run");
            }
            let out = if mutation == "reason" {
                command(
                    &f.home,
                    &[
                        "run",
                        "discard",
                        &f.run_id,
                        "--reason",
                        "different",
                        "--force",
                    ],
                )
            } else {
                isolated_discard(&f, &extra)
            };
            let error = envelope(&out, false);
            assert!(
                matches!(
                    error["error"]["code"].as_str(),
                    Some("idempotency_conflict" | "preserved_work_unverifiable")
                ),
                "{mutation}: {error}"
            );
            assert_eq!(std::fs::read(f.paths.events()).unwrap(), before);
            assert!(f.worktree.exists());
        }
    }
}

#[test]
fn submodule_force_upgrade_resumes_branch_only_after_ref_lock_refusal() {
    let f = Fixture::failed();
    populate_submodule(&f);
    git(&f.worktree, &["submodule", "deinit", "-f", "--all"]);
    record_nonforce_receipt(&f);
    git(
        &f.worktree,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "update",
            "--init",
        ],
    );
    let lock = f.source.path().join(".git/refs/heads/wt/retained.lock");
    std::fs::write(&lock, "fixture ref lock").unwrap();
    let error = envelope(&isolated_discard(&f, &["--force"]), false);
    assert_eq!(error["error"]["code"], "discard_partial_cleanup");
    assert!(!f.worktree.exists());
    std::fs::remove_file(lock).unwrap();
    let retry = envelope(&isolated_discard(&f, &["--force"]), true);
    assert_eq!(
        retry["data"]["removed"],
        json!({"worktree":false,"branch":true})
    );
    assert_eq!(
        std::fs::read_to_string(f.paths.events())
            .unwrap()
            .matches("cleanup.discard_force_authorized")
            .count(),
        1
    );
}

#[test]
fn deinitialized_submodule_storage_still_requires_force_before_receipt() {
    let f = Fixture::failed();
    populate_submodule(&f);
    git(&f.worktree, &["submodule", "deinit", "-f", "--all"]);
    assert_eq!(
        std::fs::read_dir(f.worktree.join("module space"))
            .unwrap()
            .count(),
        0
    );
    assert!(Path::new(&git(
        &f.worktree,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "modules"
        ]
    ))
    .is_dir());
    let raw = Command::new("git")
        .arg("-C")
        .arg(f.source.path())
        .args(["worktree", "remove"])
        .arg(&f.worktree)
        .output()
        .unwrap();
    assert!(!raw.status.success());
    let before = std::fs::read(f.paths.events()).unwrap();
    assert_eq!(
        envelope(&isolated_discard(&f, &["--dry-run"]), true)["data"]["force_required"],
        true
    );
    assert_eq!(
        envelope(&isolated_discard(&f, &[]), false)["error"]["code"],
        "force_required"
    );
    assert_eq!(std::fs::read(f.paths.events()).unwrap(), before);
    assert_eq!(
        envelope(&isolated_discard(&f, &["--force"]), true)["data"]["discarded"],
        true
    );
}

#[test]
fn detached_absent_branch_orphan_keeps_force_upgrade_recovery() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::failed();
    git(&f.worktree, &["checkout", "--detach", "-q"]);
    git(f.source.path(), &["branch", "-D", "wt/retained"]);
    let readonly = f.worktree.join("readonly");
    std::fs::create_dir(&readonly).unwrap();
    std::fs::write(readonly.join("tracked"), "initial").unwrap();
    git(&f.worktree, &["add", "readonly/tracked"]);
    git(&f.worktree, &["commit", "-qm", "detached work"]);
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o500)).unwrap();
    assert_eq!(
        envelope(&isolated_discard(&f, &[]), false)["error"]["code"],
        "discard_partial_cleanup"
    );
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        envelope(&isolated_discard(&f, &["--force"]), true)["data"]["discarded"],
        true
    );
    assert!(!f.worktree.exists());
}
