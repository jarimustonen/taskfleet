//! Read-only native worker transcript discovery. Build under the run's shared
//! lock alongside manifest/nodes. File availability is only a point-in-time
//! observation: live-source removal follows the durable archive event and is
//! not locked. Neither a path nor availability grants writer/resume authority.

use std::path::{Component, Path};

use serde::Serialize;
use taskfleet_core::{EvidenceStatus, Manifest, Node, RunPaths};

#[derive(Debug, Serialize)]
pub struct TranscriptPath {
    pub anchor: &'static str,
    pub path: Option<String>,
    pub availability: &'static str,
}

#[derive(Debug, Serialize)]
pub struct NodeTranscript {
    pub node_id: String,
    pub harness: Option<String>,
    pub attempt: Option<u32>,
    pub session_id: Option<String>,
    pub original_cwd: Option<String>,
    pub worktree_path: Option<String>,
    pub support: &'static str,
    pub capture_status: Option<EvidenceStatus>,
    pub live: TranscriptPath,
    pub original: TranscriptPath,
}

fn observe(
    base: &Path,
    path: Option<&str>,
    anchor: &'static str,
    absent: &'static str,
) -> TranscriptPath {
    let mut view = TranscriptPath {
        anchor,
        path: None,
        availability: absent,
    };
    let Some(path) = path else { return view };
    // Never expose a lexically escaping or absolute path as an anchored read hint.
    if path.is_empty()
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        view.availability = "unavailable";
        return view;
    }
    match std::fs::metadata(base.join(path)) {
        Ok(metadata) if metadata.is_file() => {
            view.path = Some(path.to_owned());
            view.availability = "available";
        }
        Ok(_) => view.availability = "unavailable",
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => view.availability = "missing",
        Err(_) => view.availability = "unavailable",
    }
    view
}

pub(crate) fn views(
    root: &Path,
    paths: &RunPaths,
    manifest: &Manifest,
    nodes: &[Node],
) -> Vec<NodeTranscript> {
    let mut views: Vec<_> = nodes
        .iter()
        .map(|node| {
            let evidence = node.evidence.as_ref();
            // Evidence is an exact per-node Pi binding; never infer a session from
            // filenames, timestamps, the current profile, or caller-owned Pi state.
            let harness = if evidence.is_some() {
                Some("pi".to_owned())
            } else {
                manifest
                    .agent_selection
                    .as_ref()
                    .map(|s| s.selected.harness.clone())
                    .or_else(|| manifest.harness.clone())
            };
            let support = match harness.as_deref() {
                Some("pi") => "supported",
                Some(_) => "unsupported",
                None => "unknown",
            };
            let absent = if support == "unsupported" {
                "unsupported"
            } else {
                "not-recorded"
            };
            NodeTranscript {
                node_id: node.node_id.to_string(),
                harness,
                attempt: evidence.map(|e| e.attempt),
                session_id: evidence.map(|e| e.session_id.clone()),
                original_cwd: evidence.map(|e| e.original_cwd.clone()),
                worktree_path: node.worktree_path.clone(),
                support,
                capture_status: evidence.map(|e| e.status),
                live: observe(
                    root,
                    evidence.map(|e| e.live_session_path.as_str()),
                    "state-root",
                    absent,
                ),
                original: observe(
                    &paths.root,
                    evidence.and_then(|e| e.transcript_path.as_deref()),
                    "run-root",
                    absent,
                ),
            }
        })
        .collect();
    views.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    views
}
