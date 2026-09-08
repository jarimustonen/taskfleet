//! Final durable evidence capture for native Pi workers.
//!
//! Capture runs before terminal pane/worktree cleanup. A failed capture is a
//! durable `worker.evidence.failed` fact and vetoes cleanup, preserving the live
//! sources for a supervisor retry. Success is a single locked
//! `worker.evidence.archived` transition on the node projection.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use taskfleet_core::{append_and_apply_event, read_manifest_opt, Node, RunPaths};

const MAX_TRANSCRIPT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PANE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy)]
struct RetryState {
    attempts: u32,
    next: Instant,
}

fn retries() -> &'static Mutex<HashMap<String, RetryState>> {
    static RETRIES: OnceLock<Mutex<HashMap<String, RetryState>>> = OnceLock::new();
    RETRIES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn retry_key(paths: &RunPaths, node: &Node) -> String {
    format!("{}:{}", paths.root.display(), node.node_id)
}
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

/// Capture terminal evidence, returning false only when cleanup must be deferred.
pub(crate) fn archive_before_cleanup(paths: &RunPaths, node: &Node, tmux: &str) -> bool {
    let Some(recorded) = node.evidence.as_ref() else {
        return true;
    };
    let retry_key = retry_key(paths, node);
    if recorded.status == taskfleet_core::EvidenceStatus::Complete {
        retries().lock().unwrap().remove(&retry_key);
        return match remove_live_session(paths, &recorded.live_session_path) {
            Ok(()) => true,
            Err(error) => {
                eprintln!(
                    "supervisor evidence: archived source cleanup failed for {}/{}: {error}",
                    paths.run_id, node.node_id
                );
                false
            }
        };
    }
    if retries()
        .lock()
        .unwrap()
        .get(&retry_key)
        .is_some_and(|state| Instant::now() < state.next)
    {
        return false;
    }
    if let Err(error) = quiesce_worker(node) {
        return record_failure(paths, node, recorded, error);
    }
    match capture(paths, node, tmux) {
        Ok(mut data) => {
            let Some(fields) = data.as_object_mut() else {
                return false;
            };
            fields.insert("attempt".into(), json!(recorded.attempt));
            fields.insert("session_id".into(), json!(recorded.session_id));
            let key = format!(
                "worker-evidence:{}:{}:{}",
                node.node_id, recorded.attempt, recorded.session_id
            );
            if append_and_apply_event(
                paths,
                "worker.evidence.archived",
                Some(&node.node_id),
                Some(&key),
                data,
            )
            .is_ok()
            {
                retries().lock().unwrap().remove(&retry_key);
                match remove_live_session(paths, &recorded.live_session_path) {
                    Ok(()) => true,
                    Err(error) => {
                        eprintln!(
                            "supervisor evidence: archived source cleanup failed for {}/{}: {error}",
                            paths.run_id, node.node_id
                        );
                        false
                    }
                }
            } else {
                false
            }
        }
        Err(error) => record_failure(paths, node, recorded, error),
    }
}

fn record_failure(
    paths: &RunPaths,
    node: &Node,
    recorded: &taskfleet_core::WorkerEvidence,
    detail: String,
) -> bool {
    eprintln!(
        "supervisor evidence: capture failed for {}/{}: {detail}; preserving worker resources",
        paths.run_id, node.node_id
    );
    let retry_key = retry_key(paths, node);
    let mut states = retries().lock().unwrap();
    let attempts = states
        .get(&retry_key)
        .map_or(1, |state| state.attempts.saturating_add(1));
    #[cfg(not(test))]
    let delay = Duration::from_secs(1_u64 << attempts.min(6));
    #[cfg(test)]
    let delay = Duration::ZERO;
    states.insert(
        retry_key,
        RetryState {
            attempts,
            next: Instant::now() + delay,
        },
    );
    drop(states);
    let key = format!(
        "worker-evidence-failed:{}:{}:{}",
        node.node_id, recorded.attempt, recorded.session_id
    );
    let _ = append_and_apply_event(
        paths,
        "worker.evidence.failed",
        Some(&node.node_id),
        Some(&key),
        json!({
            "attempt": recorded.attempt,
            "session_id": recorded.session_id,
            "error": detail
        }),
    );
    false
}

fn quiesce_worker(node: &Node) -> Result<(), String> {
    if node.worker_exit.is_some() {
        return Ok(());
    }
    let Some(pid) = node.agent_pid.filter(|pid| *pid > 0).map(|pid| pid as u32) else {
        return Ok(());
    };
    if !crate::supervise::pid_file::pid_alive(pid) {
        return Ok(());
    }
    if let Some(expected) = node.agent_pid_start_time {
        if let Some(actual) = crate::supervise::watchdog::pid_start_time(pid) {
            let expected = expected.timestamp().max(0) as u64;
            if expected.abs_diff(actual) > 1 {
                return Ok(());
            }
        }
    }
    // Stop the exact writer without destroying its tmux pane. This breaks the
    // cancellation cycle: final pane history remains capturable, while no Pi
    // process can append after the transcript is stamped complete.
    let result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(format!("signal native Pi writer {pid}: {error}"));
        }
        return Ok(());
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while crate::supervise::pid_file::pid_alive(pid) {
        if std::time::Instant::now() >= deadline {
            return Err(format!("native Pi writer {pid} did not stop within 5s"));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Ok(())
}

fn capture(paths: &RunPaths, node: &Node, tmux: &str) -> Result<Value, String> {
    let evidence = node
        .evidence
        .as_ref()
        .ok_or("Pi evidence identity absent")?;
    let live = checked_live_path(paths, &evidence.live_session_path)?;
    let path_before = regular_metadata(&live)?;
    let (original, opened_before, opened_after) = read_nofollow(&live)?;
    let path_after_read = regular_metadata(&live)?;
    if !same_file_state(&path_before, &opened_before)
        || !same_file_state(&opened_before, &opened_after)
        || !same_file_state(&opened_after, &path_after_read)
        || original.len() as u64 != opened_after.len()
    {
        return Err("native transcript changed while it was read".into());
    }
    let (header, tail) = parse_header(&original)?;
    if header.get("type").and_then(Value::as_str) != Some("session")
        || header.get("version").and_then(Value::as_u64) != Some(3)
        || header.get("id").and_then(Value::as_str) != Some(&evidence.session_id)
        || header.get("cwd").and_then(Value::as_str) != Some(&evidence.original_cwd)
    {
        return Err("native transcript header does not match recorded session identity/cwd".into());
    }

    let report = node
        .last_report
        .as_ref()
        .ok_or("terminal node has no terminal report")?;
    let identity = node
        .tmux_identity
        .as_ref()
        .ok_or("terminal worker has no qualified tmux identity")?;
    let mut command = Command::new(tmux);
    if let Some(socket) = identity.socket.as_deref() {
        command.args(["-S", socket]);
    }
    command.args([
        "capture-pane",
        "-p",
        "-S",
        "-",
        "-E",
        "-",
        "-t",
        identity.capture_target(),
    ]);
    let pane = match crate::proc::run_with_timeout(command, CAPTURE_TIMEOUT, MAX_PANE_BYTES) {
        crate::proc::TimedOutcome::Exited {
            status,
            stdout,
            stderr,
        } if status.success() => {
            let _ = stderr;
            stdout.bytes
        }
        crate::proc::TimedOutcome::Exited { status, stderr, .. } => {
            return Err(format!(
                "tmux capture-pane exited {:?}: {}",
                status.code(),
                String::from_utf8_lossy(&stderr.bytes).trim()
            ));
        }
        crate::proc::TimedOutcome::TimedOut => return Err("tmux capture-pane timed out".into()),
        crate::proc::TimedOutcome::SpawnErr(error) => {
            return Err(format!("tmux capture-pane spawn failed: {error}"));
        }
    };

    // Prove the native writer did not move while pane/report capture ran.
    let after_capture = regular_metadata(&live)?;
    if !same_file_state(&path_after_read, &after_capture) {
        return Err("native transcript changed during final pane capture".into());
    }

    let manifest = read_manifest_opt(paths)
        .map_err(|e| e.to_string())?
        .ok_or("run manifest disappeared during evidence capture")?;
    let resume_cwd = manifest
        .source_repo
        .as_deref()
        .filter(|cwd| Path::new(cwd).is_dir())
        .ok_or("recorded source repository is unavailable for resumable copy")?;
    let mut resume_header = header;
    resume_header["cwd"] = Value::String(resume_cwd.to_string());
    let mut resume = serde_json::to_vec(&resume_header).map_err(|e| e.to_string())?;
    resume.push(b'\n');
    resume.extend_from_slice(tail);

    let evidence_root = paths.root.join("evidence");
    ensure_private_directory(&evidence_root)?;
    let dir = evidence_root.join(node.node_id.as_str());
    ensure_private_directory(&dir)?;
    let transcript = dir.join("pi-session.original.jsonl");
    let resume_path = dir.join("pi-session.resume.jsonl");
    let pane_path = dir.join("final-pane.log");
    let report_path = dir.join("terminal-report.json");
    write_atomic(&transcript, &original)?;
    write_atomic(&resume_path, &resume)?;
    write_atomic(&pane_path, &pane)?;
    let mut report_bytes = serde_json::to_vec_pretty(report).map_err(|e| e.to_string())?;
    report_bytes.push(b'\n');
    write_atomic(&report_path, &report_bytes)?;

    let digest = format!("{:x}", Sha256::digest(&original));
    Ok(json!({
        "transcript_path": relative(paths, &transcript)?,
        "resume_path": relative(paths, &resume_path)?,
        "pane_path": relative(paths, &pane_path)?,
        "report_path": relative(paths, &report_path)?,
        "transcript_sha256": digest,
    }))
}

fn checked_live_path(paths: &RunPaths, recorded: &str) -> Result<PathBuf, String> {
    let relative = PathBuf::from(recorded);
    let state_root = paths
        .root
        .parent()
        .and_then(Path::parent)
        .ok_or("run path has no state root")?;
    let expected_parent = PathBuf::from(".creating")
        .join("pi-sessions")
        .join(paths.run_id.as_str());
    if relative.is_absolute()
        || relative.parent() != Some(expected_parent.as_path())
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("recorded native transcript path is outside its private run source".into());
    }
    Ok(state_root.join(relative))
}

pub(crate) fn discard_superseded_source(paths: &RunPaths, node: &Node) {
    if let Some(recorded) = node.evidence.as_ref() {
        let _ = remove_live_session(paths, &recorded.live_session_path);
    }
}

fn remove_live_session(paths: &RunPaths, recorded: &str) -> Result<(), String> {
    let path = checked_live_path(paths, recorded)?;
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "remove archived live transcript {}: {error}",
                path.display()
            ));
        }
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir(parent);
    }
    Ok(())
}

fn regular_metadata(path: &Path) -> Result<std::fs::Metadata, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("stat native transcript {}: {e}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "native transcript {} is not a regular file",
            path.display()
        ));
    }
    Ok(metadata)
}

#[cfg(unix)]
fn same_file_state(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
}

#[cfg(not(unix))]
fn same_file_state(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

fn read_nofollow(path: &Path) -> Result<(Vec<u8>, std::fs::Metadata, std::fs::Metadata), String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    taskfleet_core::nofollow(&mut options);
    let mut file = options
        .open(path)
        .map_err(|e| format!("open native transcript {}: {e}", path.display()))?;
    let before = file
        .metadata()
        .map_err(|e| format!("stat open native transcript {}: {e}", path.display()))?;
    if before.len() > MAX_TRANSCRIPT_BYTES {
        return Err(format!(
            "native transcript exceeds {MAX_TRANSCRIPT_BYTES} byte archive cap"
        ));
    }
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(MAX_TRANSCRIPT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read native transcript {}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_TRANSCRIPT_BYTES {
        return Err("native transcript grew beyond archive cap while reading".into());
    }
    let after = file
        .metadata()
        .map_err(|e| format!("restat open native transcript {}: {e}", path.display()))?;
    Ok((bytes, before, after))
}

fn parse_header(bytes: &[u8]) -> Result<(Value, &[u8]), String> {
    let newline = bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .ok_or("native transcript has no complete header line")?;
    let header = serde_json::from_slice(&bytes[..newline])
        .map_err(|e| format!("parse native transcript header: {e}"))?;
    Ok((header, &bytes[newline + 1..]))
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(format!("{} is not a real directory", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt as _;
                builder.mode(0o700);
            }
            builder
                .create(path)
                .map_err(|e| format!("create {}: {e}", path.display()))
        }
        Err(error) => Err(format!("stat {}: {error}", path.display())),
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("evidence path has no parent")?;
    let temp = parent.join(format!(".evidence-{}.tmp", ulid::Ulid::new()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
            taskfleet_core::nofollow(&mut options);
        }
        let mut file = options.open(&temp).map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        std::fs::rename(&temp, path).map_err(|e| e.to_string())?;
        std::fs::File::open(parent)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result.map_err(|e| format!("write evidence {}: {e}", path.display()))
}

fn relative(paths: &RunPaths, path: &Path) -> Result<String, String> {
    path.strip_prefix(&paths.root)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|_| format!("evidence path escaped run root: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;
    use taskfleet_core::{append_and_apply_event, read_node, NodeId};
    use tempfile::TempDir;

    fn setup(tmp: &TempDir) -> (RunPaths, NodeId, Vec<u8>) {
        let run_id = "01jxsnap000000000000000000";
        let root = tmp.path().join("runs").join(run_id);
        std::fs::create_dir_all(&root).unwrap();
        let paths = RunPaths::new(&root, run_id).unwrap();
        let node = NodeId::parse_str("n-0001").unwrap();
        let source = tmp.path().join("source");
        let worktree = tmp.path().join("worker");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        append_and_apply_event(
            &paths,
            "run.created",
            None,
            None,
            json!({
                "kind":"spinoff", "lifecycle":"autonomous", "title":"evidence",
                "source_repo": source, "source_branch":"main"
            }),
        )
        .unwrap();
        let session_id = "018f5f64-b137-7d44-b2b4-4f02c3f646e8";
        let live_name = format!("pi-session-{session_id}.jsonl");
        let live_relative = PathBuf::from(".creating/pi-sessions")
            .join(run_id)
            .join(&live_name);
        let live_path = tmp.path().join(&live_relative);
        std::fs::create_dir_all(live_path.parent().unwrap()).unwrap();
        let header = json!({
            "type":"session", "version":3, "id":session_id,
            "timestamp":"2026-09-08T00:00:00Z", "cwd":worktree
        });
        let mut original = serde_json::to_vec(&header).unwrap();
        original.extend_from_slice(b"\n{\"type\":\"message\",\"raw\":\"preserve ");
        original.extend_from_slice(&[0xf0, 0x9f, 0xa7, 0xaa]);
        original.extend_from_slice(b"\"}\n");
        std::fs::write(&live_path, &original).unwrap();
        append_and_apply_event(
            &paths,
            "node.created",
            Some(&node),
            None,
            json!({
                "kind":"spinoff", "worktree_path":worktree,
                "tmux_session":"private", "tmux_window_id":"@7", "tmux_pane_id":"%9",
                "pi_session_id":session_id, "pi_session_path":live_relative,
                "pi_session_cwd":worktree, "attempt":0
            }),
        )
        .unwrap();
        append_and_apply_event(
            &paths,
            "worker.exited",
            Some(&node),
            None,
            json!({"exit_code":0}),
        )
        .unwrap();
        append_and_apply_event(
            &paths,
            "node.report",
            Some(&node),
            None,
            json!({"success":true,"via":"explicit-merge","summary":"done"}),
        )
        .unwrap();
        (paths, node, original)
    }

    fn tmux_script(tmp: &TempDir, body: &str) -> String {
        let path = tmp.path().join(format!("tmux-{}.sh", ulid::Ulid::new()));
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn archives_exact_transcript_and_header_only_resume_copy() {
        let tmp = TempDir::new().unwrap();
        let (paths, node_id, original) = setup(&tmp);
        let tmux = tmux_script(&tmp, "printf 'FINAL PANE\\n'");
        let node = read_node(&paths, &node_id).unwrap();
        assert!(archive_before_cleanup(&paths, &node, &tmux));

        let projected = read_node(&paths, &node_id).unwrap();
        // A restarted supervisor trusts the durable complete projection and
        // neither needs the unlinked live source nor recaptures the pane.
        let broken_tmux = tmux_script(&tmp, "exit 71");
        assert!(archive_before_cleanup(&paths, &projected, &broken_tmux));
        let evidence = projected.evidence.unwrap();
        assert_eq!(evidence.status, taskfleet_core::EvidenceStatus::Complete);
        let archived = std::fs::read(paths.root.join(evidence.transcript_path.unwrap())).unwrap();
        assert_eq!(
            archived, original,
            "original transcript bytes are immutable"
        );
        let resume = std::fs::read(paths.root.join(evidence.resume_path.unwrap())).unwrap();
        let original_newline = original.iter().position(|byte| *byte == b'\n').unwrap();
        let resume_newline = resume.iter().position(|byte| *byte == b'\n').unwrap();
        let original_tail = &original[original_newline + 1..];
        let resume_tail = &resume[resume_newline + 1..];
        assert_eq!(resume_tail, original_tail, "only the header may change");
        let resume_header: Value = serde_json::from_slice(&resume[..resume_newline]).unwrap();
        assert_eq!(
            resume_header["cwd"],
            Value::String(tmp.path().join("source").to_string_lossy().into_owned())
        );
        assert_eq!(
            std::fs::read_to_string(paths.root.join(evidence.pane_path.unwrap())).unwrap(),
            "FINAL PANE\n"
        );
        assert!(std::fs::read_to_string(paths.events())
            .unwrap()
            .contains("worker.evidence.archived"));
    }

    #[test]
    fn failed_capture_is_durable_and_a_later_retry_can_complete() {
        let tmp = TempDir::new().unwrap();
        let (paths, node_id, _) = setup(&tmp);
        let failing = tmux_script(&tmp, "echo unavailable >&2; exit 1");
        let node = read_node(&paths, &node_id).unwrap();
        assert!(!archive_before_cleanup(&paths, &node, &failing));
        let failed = read_node(&paths, &node_id).unwrap();
        assert_eq!(
            failed.evidence.as_ref().unwrap().status,
            taskfleet_core::EvidenceStatus::Failed
        );
        assert!(failed.evidence.as_ref().unwrap().error.is_some());

        // A restarted supervisor reads the durable failed projection and retries;
        // success is allowed to advance failed -> complete exactly once.
        let succeeding = tmux_script(&tmp, "printf recovered");
        assert!(archive_before_cleanup(&paths, &failed, &succeeding));
        assert_eq!(
            read_node(&paths, &node_id)
                .unwrap()
                .evidence
                .unwrap()
                .status,
            taskfleet_core::EvidenceStatus::Complete
        );
    }

    #[test]
    fn terminal_cleanup_stops_live_writer_before_exact_capture() {
        let tmp = TempDir::new().unwrap();
        let (paths, node_id, _) = setup(&tmp);
        let live_relative = read_node(&paths, &node_id)
            .unwrap()
            .evidence
            .unwrap()
            .live_session_path;
        let live = tmp.path().join(live_relative);
        let mut child = std::process::Command::new("/bin/sh")
            .args([
                "-c",
                &format!(
                    "while :; do printf '{{\"type\":\"tick\"}}\\n' >> '{}'; sleep 0.01; done",
                    live.display()
                ),
            ])
            .spawn()
            .unwrap();
        let pid = child.id();
        let reaper = std::thread::spawn(move || child.wait().unwrap());
        let mut node = read_node(&paths, &node_id).unwrap();
        node.worker_exit = None;
        node.agent_pid = Some(pid as i32);
        node.agent_pid_start_time = crate::supervise::watchdog::pid_start_time(pid)
            .and_then(|seconds| chrono::DateTime::from_timestamp(seconds as i64, 0));
        std::thread::sleep(std::time::Duration::from_millis(30));
        let tmux = tmux_script(&tmp, "printf 'FINAL AFTER TERM\\n'");

        assert!(archive_before_cleanup(&paths, &node, &tmux));
        let _ = reaper.join().unwrap();
        let evidence = read_node(&paths, &node_id).unwrap().evidence.unwrap();
        assert_eq!(evidence.status, taskfleet_core::EvidenceStatus::Complete);
        assert!(
            !live.exists(),
            "live writer source is removed only after stop"
        );
    }

    #[test]
    fn transcript_writer_drift_fails_closed() {
        let tmp = TempDir::new().unwrap();
        let (paths, node_id, _) = setup(&tmp);
        let live = read_node(&paths, &node_id)
            .unwrap()
            .evidence
            .unwrap()
            .live_session_path;
        let live = tmp.path().join(live);
        let tmux = tmux_script(
            &tmp,
            &format!("printf drift >> '{}'; printf pane", live.display()),
        );
        let node = read_node(&paths, &node_id).unwrap();
        assert!(!archive_before_cleanup(&paths, &node, &tmux));
        let projected = read_node(&paths, &node_id).unwrap();
        assert_eq!(
            projected.evidence.unwrap().status,
            taskfleet_core::EvidenceStatus::Failed
        );
        assert!(!paths
            .root
            .join("evidence/n-0001/pi-session.original.jsonl")
            .exists());
    }
}
