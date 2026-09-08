//! Current retained-resource observation shared by `run show` and `run discard`.
//! This is deliberately an observation, not persisted run state.

use std::path::Path;

use serde::Serialize;
use taskfleet_core::{read_node_opt, Manifest, Node, NodeId, RunPaths, Status};

use crate::git::repo::Git;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PreservedWork {
    pub node_id: String,
    pub worktree_path: Option<String>,
    pub worktree_present: bool,
    pub branch: Option<String>,
    pub branch_present: bool,
    pub cleanliness: &'static str,
    pub unmerged_commits: Option<u64>,
    pub verification: &'static str,
}

#[derive(Debug, Clone)]
pub(crate) struct RetainedObservation {
    pub view: PreservedWork,
    pub source_repo: Option<String>,
    verified: bool,
}

impl RetainedObservation {
    pub(crate) fn verified_for_discard(&self) -> bool {
        self.verified
    }

    pub(crate) fn has_resources(&self) -> bool {
        self.view.worktree_present || self.view.branch_present
    }
}

/// Observe one terminal node. Returns `None` only when the node is outside the
/// failed/cancelled ownership scope or both recorded resources are verifiably
/// absent. Unknown Git state remains visible as an unverifiable row.
pub(crate) fn observe(manifest: &Manifest, node: &Node, git: &Git) -> Option<RetainedObservation> {
    if !matches!(manifest.status, Status::Failed | Status::Cancelled)
        || !matches!(node.status, Status::Failed | Status::Cancelled)
    {
        return None;
    }
    let worktree_path = node.worktree_path.clone();
    let branch = node.branch.clone().filter(|s| !s.is_empty());
    if worktree_path.is_none() && branch.is_none() {
        return None;
    }

    let (worktree_present, path_probe_verified) = match worktree_path.as_deref() {
        Some(path) => match Path::new(path).try_exists() {
            Ok(present) => (present, true),
            Err(_) => (true, false), // keep a possibly-present owned resource visible
        },
        None => (false, true),
    };

    let registrations = manifest
        .source_repo
        .as_deref()
        .and_then(|repo| git.worktree_registrations(repo));
    // A recorded source may itself be a linked worktree (or a canonicalizable
    // alias), so first-row path equality is not repository identity. The shared
    // common-dir resolver is the one owner of that fact; successful resolution
    // plus a registration list obtained through that source binds the repo.
    let source_identity = manifest.source_repo.as_deref().and_then(|source| {
        crate::run::list::repository_identity(Path::new(source), false)
            .ok()
            .flatten()
    });
    let source_verified = registrations.is_some() && source_identity.is_some();
    let registered = worktree_path.as_deref().and_then(|path| {
        registrations.as_ref().and_then(|rows| {
            rows.iter()
                .find(|row| paths_equivalent(Path::new(&row.path), Path::new(path)))
        })
    });
    // Registration alone is insufficient: a stale registered path can be
    // removed and replaced by an independent repository. Bind the directory
    // currently present at the path to the same git common-dir identity.
    let present_identity_matches = if worktree_present {
        worktree_path.as_deref().is_some_and(|path| {
            crate::run::list::repository_identity(Path::new(path), false)
                .ok()
                .flatten()
                .zip(source_identity.as_ref())
                .is_some_and(|(actual, expected)| actual == *expected)
        })
    } else {
        true
    };
    let registration_verified =
        !worktree_present || (registered.is_some() && present_identity_matches);
    let branch_binding_verified = if worktree_present {
        match (registered, branch.as_deref()) {
            (Some(row), Some(expected)) => row.branch.as_deref().is_none_or(|b| b == expected),
            (Some(_), None) | (None, _) => false,
        }
    } else {
        true
    };

    let branch_probe = match (manifest.source_repo.as_deref(), branch.as_deref()) {
        (Some(repo), Some(branch)) => git.branch_exists(repo, branch),
        (_, None) => Some(false),
        _ => None,
    };
    let branch_present = branch_probe == Some(true);
    let branch_probe_verified = branch_probe.is_some();

    if !worktree_present && branch_probe == Some(false) {
        return None;
    }

    let cleanliness_probe = if worktree_present && registered.is_some() {
        worktree_path
            .as_deref()
            .and_then(|path| git.worktree_status_clean(path))
    } else if worktree_present {
        None
    } else {
        Some(true)
    };
    let cleanliness = match cleanliness_probe {
        Some(true) if worktree_present => "clean",
        Some(false) => "dirty",
        Some(true) => "not-applicable",
        None => "unverifiable",
    };
    let commit_target = if worktree_present {
        worktree_path.as_deref().and_then(|path| git.head_oid(path))
    } else if branch_probe == Some(true) {
        branch.clone()
    } else {
        None
    };
    let unmerged_commits = match (
        manifest.source_repo.as_deref(),
        manifest.source_branch.as_deref(),
        commit_target.as_deref(),
        branch_probe,
    ) {
        (Some(repo), Some(source), Some(target), _) => git.rev_list_count(repo, source, target),
        (_, _, _, Some(false)) if !worktree_present => Some(0),
        _ => None,
    };
    let verified = source_verified
        && registration_verified
        && branch_binding_verified
        && branch_probe_verified
        && path_probe_verified
        && cleanliness_probe.is_some()
        && unmerged_commits.is_some();

    Some(RetainedObservation {
        view: PreservedWork {
            node_id: node.node_id.as_str().to_string(),
            worktree_path,
            worktree_present,
            branch,
            branch_present,
            cleanliness,
            unmerged_commits,
            verification: if verified { "verified" } else { "unverifiable" },
        },
        source_repo: manifest.source_repo.clone(),
        verified,
    })
}

pub(crate) enum MissingNodesDir {
    Empty,
    Error,
}

/// One run-local projection scanner shared by retained-work readers. Callers
/// explicitly choose whether a missing nodes directory means an empty read
/// (`run show`) or malformed mutable state (`run discard`).
pub(crate) fn read_nodes(
    paths: &RunPaths,
    missing: MissingNodesDir,
) -> taskfleet_core::Result<Vec<Node>> {
    let entries = match std::fs::read_dir(paths.nodes_dir()) {
        Ok(entries) => entries,
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                && matches!(missing, MissingNodesDir::Empty) =>
        {
            return Ok(Vec::new());
        }
        Err(e) => return Err(taskfleet_core::Error::io(paths.nodes_dir(), e)),
    };
    let mut nodes = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| taskfleet_core::Error::io(paths.nodes_dir(), e))?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(node_id) = NodeId::parse_str(stem) else {
            continue;
        };
        if let Some(node) = read_node_opt(paths, &node_id)? {
            nodes.push(node);
        }
    }
    nodes.sort_by(|a, b| a.node_id.as_str().cmp(b.node_id.as_str()));
    Ok(nodes)
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    left.canonicalize()
        .ok()
        .zip(right.canonicalize().ok())
        .is_some_and(|(left, right)| left == right)
}
