//! Wire DTOs for the `node` noun.
//!
//! These decouple the CLI `--json` contract from the on-disk projection
//! (`taskfleet_core::Node`). Handlers serialize a `*View` — never the
//! projection struct — so the disk schema can evolve without leaking into
//! the public envelope.
//!
//! Deliberately dropped from the wire contract:
//!
//! - `last_processed_report_seq_by_child` — an internal idempotency
//!   cursor (per-child highest-report watermark) used by the reducer to
//!   make report processing replay-safe. It is projection bookkeeping
//!   with no meaning to a wire consumer, the same category as
//!   `Manifest::applied_seq`.
//!
//! `kind` / `status` render through the kebab helpers in [`crate::run`]
//! so a new enum variant is a compile error here, not a silent wire
//! change.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use taskfleet_core::schema::TmuxIdentity;
use taskfleet_core::{ChildRef, Node, NodeId, RunId};

use crate::run::{kind_kebab, status_kebab};

/// Full single-node wire view (`node show --json`).
///
/// Borrows from the projection: the `show` handler holds the `Node` for
/// the lifetime of the emit. Field order and names mirror the established
/// wire contract; the internal `last_processed_report_seq_by_child`
/// cursor is intentionally absent (see module docs).
#[derive(Serialize)]
pub struct NodeView<'a> {
    pub schema_version: u32,
    pub node_id: &'a NodeId,
    pub run_id: &'a RunId,
    pub parent_node_id: Option<&'a NodeId>,
    pub kind: &'static str,
    pub status: &'static str,
    pub task: Option<&'a str>,
    pub worktree_path: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub tmux_window: Option<&'a str>,
    pub tmux_identity: Option<&'a TmuxIdentity>,
    pub agent_pid: Option<i32>,
    pub agent_pid_start_time: Option<DateTime<Utc>>,
    pub supervisor_pid: Option<i32>,
    pub children: &'a [ChildRef],
    pub started_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    /// Projection-native report field. Kept for backwards compatibility with
    /// callers that already read the on-disk-shaped wire view.
    pub last_report: &'a Option<Value>,
    /// Stable, consumer-facing alias for the worker's terminal report.
    ///
    /// Unlike `last_report`, this does not expose that the projection stores
    /// only the latest report. Both fields intentionally carry identical data.
    pub report: &'a Option<Value>,
}

impl<'a> From<&'a Node> for NodeView<'a> {
    fn from(n: &'a Node) -> Self {
        Self {
            schema_version: n.schema_version,
            node_id: &n.node_id,
            run_id: &n.run_id,
            parent_node_id: n.parent_node_id.as_ref(),
            kind: kind_kebab(n.kind),
            status: status_kebab(n.status),
            task: n.task.as_deref(),
            worktree_path: n.worktree_path.as_deref(),
            branch: n.branch.as_deref(),
            tmux_window: n.tmux_window.as_deref(),
            tmux_identity: n.tmux_identity.as_deref(),
            agent_pid: n.agent_pid,
            agent_pid_start_time: n.agent_pid_start_time,
            supervisor_pid: n.supervisor_pid,
            children: &n.children,
            started_at: n.started_at,
            updated_at: n.updated_at,
            last_report: &n.last_report,
            report: &n.last_report,
        }
    }
}

/// One row of `node list --json`.
///
/// Owned: built inside the run lock from short-lived projections.
#[derive(Serialize)]
pub struct NodeSummary {
    pub node_id: String,
    pub kind: String,
    pub status: String,
    pub parent_node_id: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub children: u32,
}

impl From<&Node> for NodeSummary {
    fn from(n: &Node) -> Self {
        Self {
            node_id: n.node_id.to_string(),
            kind: kind_kebab(n.kind).to_string(),
            status: status_kebab(n.status).to_string(),
            parent_node_id: n.parent_node_id.as_ref().map(ToString::to_string),
            updated_at: n.updated_at,
            children: n.children.len() as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map};
    use taskfleet_core::{Kind, Status};

    fn ts() -> DateTime<Utc> {
        "2024-01-01T00:00:00Z".parse().unwrap()
    }

    fn sample() -> Node {
        Node {
            schema_version: 1,
            node_id: NodeId::parse_str("n-0001").unwrap(),
            run_id: RunId::parse_str("01arz3ndektsv4rrffq69g5fav").unwrap(),
            parent_node_id: None,
            kind: Kind::Spinoff,
            status: Status::Pending,
            task: None,
            worktree_path: Some("/tmp/seed-wt".to_string()),
            branch: Some("wt/seed".to_string()),
            base_sha: None,
            tmux_window: Some("seed-win".to_string()),
            tmux_identity: None,
            evidence: None,
            retained_display: None,
            retention_unavailable: None,
            agent_pid: Some(4242),
            agent_pid_start_time: None,
            supervisor_pid: None,
            children: Vec::new(),
            started_at: Some(ts()),
            updated_at: ts(),
            last_report: None,
            last_processed_report_seq_by_child: Map::new(),
            retry_attempts: 0,
            worker_exit: None,
            pending_merge: None,
            first_death_at: None,
            awaiting_input: None,
        }
    }

    #[test]
    fn view_pins_wire_shape() {
        let n = sample();
        let got = serde_json::to_value(NodeView::from(&n)).unwrap();
        assert_eq!(
            got,
            json!({
                "schema_version": 1,
                "node_id": "n-0001",
                "run_id": "01arz3ndektsv4rrffq69g5fav",
                "parent_node_id": null,
                "kind": "spinoff",
                "status": "pending",
                "task": null,
                "worktree_path": "/tmp/seed-wt",
                "branch": "wt/seed",
                "tmux_window": "seed-win",
                "tmux_identity": null,
                "agent_pid": 4242,
                "agent_pid_start_time": null,
                "supervisor_pid": null,
                "children": [],
                "started_at": "2024-01-01T00:00:00Z",
                "updated_at": "2024-01-01T00:00:00Z",
                "last_report": null,
                "report": null,
            })
        );
    }

    /// The dropped internal cursor must never reach the wire: bumping it on
    /// the projection leaves the DTO output byte-identical.
    #[test]
    fn internal_cursor_does_not_leak() {
        let base = serde_json::to_value(NodeView::from(&sample())).unwrap();
        let mut bumped = sample();
        bumped
            .last_processed_report_seq_by_child
            .insert("c-01arz3ndektsv4rrffq69g5fav".to_string(), json!(7));
        let after = serde_json::to_value(NodeView::from(&bumped)).unwrap();
        assert_eq!(base, after, "internal cursor leaked into node DTO");
        assert!(
            after.get("last_processed_report_seq_by_child").is_none(),
            "internal cursor must be absent from the wire contract"
        );
    }

    #[test]
    fn report_alias_matches_last_report() {
        let mut n = sample();
        n.last_report = Some(json!({"summary": "worker finished"}));
        let got = serde_json::to_value(NodeView::from(&n)).unwrap();
        assert_eq!(got["report"], got["last_report"]);
    }

    #[test]
    fn summary_pins_wire_shape() {
        let n = sample();
        let got = serde_json::to_value(NodeSummary::from(&n)).unwrap();
        assert_eq!(
            got,
            json!({
                "node_id": "n-0001",
                "kind": "spinoff",
                "status": "pending",
                "parent_node_id": null,
                "updated_at": "2024-01-01T00:00:00Z",
                "children": 0,
            })
        );
    }
}
