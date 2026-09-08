//! One conservative observer for a run node's retained worker process.
//!
//! Destructive run operations use this instead of independently interpreting
//! PID liveness, recorded start identity, and told exit state.

use taskfleet_core::Node;

use crate::supervise::{pid_file, watchdog};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkerState {
    Exited,
    NoPid,
    Gone,
    Live { pid: u32, start_time: u64 },
    Unverifiable { pid: u32 },
}

impl WorkerState {
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::Exited => "exited",
            Self::NoPid => "no-pid",
            Self::Gone => "gone",
            Self::Live { .. } => "live",
            Self::Unverifiable { .. } => "unverifiable",
        }
    }
}

/// Positive proof that the original process is live takes precedence over a
/// told exit. Otherwise a told exit is authoritative. Without one, an alive
/// PID whose actual start identity cannot be read is unverifiable, while an
/// actual identity mismatch proves that the recorded worker is gone.
pub(crate) fn classify_worker(node: &Node) -> WorkerState {
    classify_worker_with(node, pid_file::pid_alive, watchdog::pid_start_time)
}

fn classify_worker_with(
    node: &Node,
    alive: impl Fn(u32) -> bool,
    actual_start: impl Fn(u32) -> Option<u64>,
) -> WorkerState {
    let Some(pid_i) = node.agent_pid else {
        return if node.worker_exit.is_some() {
            WorkerState::Exited
        } else {
            WorkerState::NoPid
        };
    };
    if pid_i <= 0 {
        return if node.worker_exit.is_some() {
            WorkerState::Exited
        } else {
            WorkerState::NoPid
        };
    }
    let pid = pid_i as u32;
    if !alive(pid) {
        return if node.worker_exit.is_some() {
            WorkerState::Exited
        } else {
            WorkerState::Gone
        };
    }

    let expected = node
        .agent_pid_start_time
        .map(|t| t.timestamp().max(0) as u64);
    let actual = actual_start(pid);
    if let (Some(expected), Some(actual)) = (expected, actual) {
        if expected.abs_diff(actual) <= 1 {
            return WorkerState::Live {
                pid,
                start_time: expected,
            };
        }
        return if node.worker_exit.is_some() {
            WorkerState::Exited
        } else {
            WorkerState::Gone
        };
    }

    if node.worker_exit.is_some() {
        WorkerState::Exited
    } else {
        WorkerState::Unverifiable { pid }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use taskfleet_core::{Kind, NodeId, RunId, Status};

    fn node(exit: bool) -> Node {
        Node {
            schema_version: 1,
            node_id: NodeId::parse_str("n-0001").unwrap(),
            run_id: RunId::parse_str("01arz3ndektsv4rrffq69g5fav").unwrap(),
            parent_node_id: None,
            kind: Kind::Spinoff,
            status: Status::Running,
            task: None,
            worktree_path: Some("/tmp/wt".into()),
            branch: Some("wt/x".into()),
            base_sha: None,
            tmux_window: None,
            tmux_identity: None,
            agent_pid: Some(42),
            agent_pid_start_time: Some(Utc.timestamp_opt(100, 0).unwrap()),
            supervisor_pid: None,
            children: vec![],
            started_at: None,
            updated_at: Utc::now(),
            last_report: None,
            last_processed_report_seq_by_child: serde_json::Map::new(),
            retry_attempts: 0,
            worker_exit: exit.then(|| taskfleet_core::WorkerExit {
                code: Some(0),
                signal: None,
                at: Utc::now(),
            }),
            pending_merge: None,
            first_death_at: None,
            awaiting_input: None,
        }
    }

    #[test]
    fn unavailable_actual_identity_is_not_a_verified_mismatch() {
        assert_eq!(
            classify_worker_with(&node(false), |_| true, |_| None),
            WorkerState::Unverifiable { pid: 42 }
        );
    }

    #[test]
    fn verified_mismatch_is_gone_without_told_exit() {
        assert_eq!(
            classify_worker_with(&node(false), |_| true, |_| Some(200)),
            WorkerState::Gone
        );
    }

    #[test]
    fn positive_live_proof_overrides_told_exit() {
        assert!(matches!(
            classify_worker_with(&node(true), |_| true, |_| Some(100)),
            WorkerState::Live { pid: 42, .. }
        ));
    }
}
