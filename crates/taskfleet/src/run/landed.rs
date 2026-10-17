//! The rebase-robust `landed` signal surfaced by `run wait` / `run show`.
//!
//! ## Why this exists (issue `landing-signal-reliable-after-rebase`)
//!
//! The `worktree-spinoff` / `stint` skills used to tell the caller to confirm a
//! landing *from git* with
//!
//! ```text
//! git merge-base --is-ancestor <worker-branch> <target>
//! ```
//!
//! That check is a **false-negative trap in exactly the environment `/stint`
//! targets** — a heavy-parallel repo where other sessions push to `origin/main`
//! continuously, forcing the conductor to `git rebase origin/main` its local
//! `main` repeatedly during a round. After such a rebase:
//!
//! - the worker's merge commit is **replayed under a new hash** on the rebased
//!   local `main`;
//! - the worker **branch ref** still points at its **pre-rebase** hash;
//! - `git merge-base --is-ancestor <branch> main` then returns **false** even
//!   though the worker's *content* is fully merged.
//!
//! The conductor concludes "the worker died / didn't land" and nearly takes a
//! destructive recovery action. Observed firing twice in one real session.
//!
//! ## The fix: patch-id equivalence + ancestry, not branch-ref ancestry alone
//!
//! [`git_verify_landed`] confirms a landing by **content**, in two rungs so it is
//! sound for every way this codebase integrates a branch (a rebase-then-`--ff-only`
//! merge — see `run merge`'s `merge.sh`):
//!
//! 1. **Patch-id equivalence (`git cherry`).** `git -C <repo> cherry <source>
//!    <branch> <base>` reports each of the branch's own commits as `-` (a
//!    patch-equivalent commit exists in the target) or `+` (none does). Patch-id
//!    is stable across a rebase — a replayed commit keeps its patch-id under a new
//!    hash — so a landing stays confirmed after the caller rebases their local
//!    target. If every commit is `-`, landed. (Verified: even immediately after a
//!    rebase-then-ff merge, `--is-ancestor` reads false while `cherry` reads `-`.)
//! 2. **Ancestry safety net (`merge-base --is-ancestor`).** Only consulted when
//!    rung 1 did not already confirm (a `+` line, or an empty `base..branch` range
//!    — e.g. `base_sha` absent). If the branch tip is literally reachable from the
//!    target *and* the branch advanced past its fork point, it landed. This rung
//!    catches a fast-forward / plain-merge landing whose commits `cherry` could not
//!    range over, and it is only reached AFTER rung 1, so it can never reintroduce
//!    the rebase false negative (the rebase case is already `-` at rung 1). The
//!    advanced-past-`base` guard rejects a never-committed or rewound branch, which
//!    is trivially an ancestor yet merged nothing.
//!
//! A `+` line that ancestry also can't rescue is a **genuine** unlanded commit —
//! reported as `landed: false, git-verified` (authoritative: git's live view wins
//! over a stale marker, so a worker that committed more after its merge is not
//! silently counted as fully landed).
//!
//! When neither rung can run — the branch ref was already torn down by the
//! supervisor, no `source_repo`/`branch` was recorded, or git errors — the signal
//! falls back to the durable **report marker**: a confirmed `run merge` terminal
//! `node.report` (a typed `ReportOrigin::RunMerge` origin, or — for a legacy
//! report with no origin field — `via: "explicit-merge"`; issue
//! `retire-via-string`) — the only success-completion marker in the thin model.
//! That marker is the recorded
//! fact that the merge completed; it was correct in the session where the ancestry
//! check lied, and it is the real post-teardown case (the branch ref is force-
//! deleted after a confirmed merge, so only the marker remains).
//!
//! `landed` means **integrated into the target's history** (patch-present or
//! reachable), NOT "the expected tree is currently checked out": a later commit on
//! the target that reverts the work is a separate event this flag does not model.
//!
//! The caller reads one boolean (`landed`) plus a `landed_method`
//! (`git-verified` | `report-marker` | `unverified`) that says how the verdict
//! was reached, and never has to run `merge-base --is-ancestor` by hand.

use std::process::{Command, Stdio};

use serde_json::Value;

use taskfleet_core::ReportOrigin;

/// How a [`LandingSignal`] verdict was reached — surfaced as `landed_method`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LandedMethod {
    /// Git resolved the target and worker branch and decided the verdict
    /// authoritatively — patch-id equivalence (`git cherry`) and/or the ancestry
    /// safety net. Robust to a caller-side rebase. `landed` may be `true`
    /// (integrated) or `false` (a genuine unlanded commit git could see).
    GitVerified,
    /// Git verification was unavailable (branch torn down, no repo/branch
    /// recorded, or git errored), so the `landed: true` verdict came from the
    /// durable confirmed-`run merge` terminal-report marker (a typed
    /// `ReportOrigin::RunMerge` origin, or a legacy `via: "explicit-merge"`).
    /// This is the normal post-teardown case.
    ReportMarker,
    /// Neither git verification nor a merge marker was available — `landed` is
    /// `false` because nothing *confirms* a landing, NOT because one was
    /// disproven. A caller must treat this as "could not verify" (fall back to a
    /// content check on the actual target), never as proof the work is missing.
    Unverified,
}

impl LandedMethod {
    /// Wire string for the `landed_method` field.
    pub(crate) fn wire(self) -> &'static str {
        match self {
            LandedMethod::GitVerified => "git-verified",
            LandedMethod::ReportMarker => "report-marker",
            LandedMethod::Unverified => "unverified",
        }
    }
}

/// The computed landing verdict for one run's reporting node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LandingSignal {
    /// True when the worker's committed work is integrated into the target —
    /// git-confirmed (patch-present or reachable via the ancestry net), or, when
    /// git could not run, attested by the durable `success: true` merge marker.
    pub landed: bool,
    /// How [`Self::landed`] was decided.
    pub method: LandedMethod,
}

/// The already-read run/node fields [`landing_signal`] needs. The caller reads
/// these under the run's shared lock (state-integrity invariant 3) and then
/// calls `landing_signal` *outside* the lock — the git shell-out must not run
/// while the flock is held.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct LandingInputs<'a> {
    /// `manifest.source_repo` — the repo the target branch lives in. Survives
    /// worktree teardown, so it is preferred over the worktree path.
    pub source_repo: Option<&'a str>,
    /// `manifest.source_branch` — the merge target (`main` for a worktree, an
    /// integration branch for an orchestrated child).
    pub source_branch: Option<&'a str>,
    /// `node.worktree_path` — a fallback repo to run git in while the worktree
    /// still exists and no `source_repo` was recorded.
    pub worktree_path: Option<&'a str>,
    /// `node.branch` — the worker branch ref.
    pub branch: Option<&'a str>,
    /// `node.base_sha` — the fork point, used as `git cherry`'s limit so only the
    /// branch's own commits are examined.
    pub base_sha: Option<&'a str>,
    /// The reporting node's terminal `node.report`, for the marker fallback.
    pub report: Option<&'a Value>,
}

/// Compute the rebase-robust `landed` signal from already-read fields.
///
/// Precedence (see the module docs for the rationale):
/// 1. Git (patch-id + ancestry) confirms every branch commit is integrated →
///    `git-verified` true.
/// 2. Git positively finds an unlanded commit (not patch-present, not in ancestry)
///    → `git-verified` false. Git's live view is authoritative over a possibly
///    stale marker, so post-merge work added on the branch is not hidden.
/// 3. else (git could not run) a durable `success: true` merge marker is present →
///    `report-marker` true.
/// 4. else nothing confirms a landing → `unverified` false.
pub(crate) fn landing_signal(inputs: &LandingInputs<'_>, git: &str) -> LandingSignal {
    match git_verify_landed(inputs, git) {
        Some(true) => LandingSignal {
            landed: true,
            method: LandedMethod::GitVerified,
        },
        Some(false) => LandingSignal {
            landed: false,
            method: LandedMethod::GitVerified,
        },
        None if report_has_merge_marker(inputs.report) => LandingSignal {
            landed: true,
            method: LandedMethod::ReportMarker,
        },
        None => LandingSignal {
            landed: false,
            method: LandedMethod::Unverified,
        },
    }
}

/// True when a terminal report is a confirmed **successful** merge — the only
/// success-completion marker in the thin model. Delegates to
/// [`ReportOrigin::report_is_confirmed_merge`], so it reads the SAME merge truth
/// as the supervisor's typed
/// [`TerminalOutcome::Merged`](crate::supervise::outcome::TerminalOutcome) gate,
/// the reducer's adoption gate, and `run wait`'s `merged` flag: the typed
/// [`ReportOrigin::RunMerge`] origin (issue `retire-via-string`), with the legacy
/// `via: "explicit-merge"` string honored only for a legacy report carrying NO
/// `origin` field. An agent-authored report (normalized to an `Agent` origin by
/// `node report`) never earns the landed marker on a forged `via` alone. The
/// `success: true` requirement means a merge marker with `success: false`
/// (malformed/spoofed) is not a marker; a blocked handoff reads false.
fn report_has_merge_marker(report: Option<&Value>) -> bool {
    report.is_some_and(ReportOrigin::report_is_confirmed_merge)
}

/// Git-verified landing check. Two rungs (see module docs), sound for the
/// rebase-then-`--ff-only` integration this codebase uses:
/// - `Some(true)`  — every branch commit is integrated: all patch-present
///   (`git cherry` all `-`), OR the branch tip is reachable from the target and
///   the branch advanced past its fork point (the ancestry safety net for a
///   fast-forward / plain-merge landing `cherry` could not range over).
/// - `Some(false)` — a branch commit is genuinely unlanded: no patch-equivalent
///   in the target (`+`) AND the branch is not an ancestor of the target. Git's
///   authoritative negative.
/// - `None`        — cannot tell: a required input missing/`-`-prefixed (option
///   injection guard), the branch ref is gone (deleted at teardown), git errored,
///   or nothing is decidable. The caller falls back to the report marker.
///
/// Conservative: any spawn failure, non-zero exit, or unexpected line reads as
/// `None` (never a fabricated verdict), so a transient git hiccup degrades to the
/// marker fallback.
fn git_verify_landed(inputs: &LandingInputs<'_>, git: &str) -> Option<bool> {
    let repo = safe_arg(inputs.source_repo).or_else(|| safe_arg(inputs.worktree_path))?;
    let source = safe_arg(inputs.source_branch)?;
    let branch = safe_arg(inputs.branch)?;
    // `base_sha` is optional; drop it if it fails the option-injection guard
    // rather than failing the whole check (cherry then defaults its limit to
    // `source`, and the ancestry net still covers the fast-forward case).
    let base = safe_arg(inputs.base_sha);

    // Rung 1: patch-id equivalence. Robust to a caller-side rebase.
    let mut cmd = Command::new(git);
    cmd.arg("-C").arg(repo).args(["cherry", source, branch]);
    // Limit `git cherry` to the branch's own commits when we know the fork point;
    // without it, cherry defaults the limit to `source` (examining `source..branch`),
    // which is empty for a fast-forwarded branch — rung 2 then covers that case.
    if let Some(base) = base {
        cmd.arg(base);
    }
    let out = cmd.stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        // Branch ref gone / bad revision → cannot tell. Defer to the marker.
        return None;
    }

    let text = String::from_utf8_lossy(&out.stdout);
    let mut saw_commit = false;
    let mut all_present = true;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        saw_commit = true;
        if line.starts_with('+') {
            // `+ <sha>`: no patch-equivalent upstream — not (yet) confirmed.
            all_present = false;
        } else if line.starts_with('-') {
            // `- <sha>`: a patch-equivalent commit exists upstream → landed.
        } else {
            // Unexpected shape — refuse to guess.
            return None;
        }
    }
    if saw_commit && all_present {
        // Every branch commit is patch-present in the target. Landed, and immune
        // to the rebase replay (the discriminating case). Return before the
        // ancestry net so the rebase case never touches it.
        return Some(true);
    }

    // Rung 2: ancestry safety net. Reached only when rung 1 did NOT confirm — a
    // `+` line (a landing whose patch-id changed, e.g. a plain merge) or an empty
    // range (`base_sha` absent + fast-forward). If the branch tip is reachable
    // from the target AND advanced past its fork point, it landed. Only after
    // rung 1, so it can never reintroduce the rebase false negative.
    if git_is_ancestor(git, repo, branch, source)
        && branch_advanced_past_base(git, repo, base, branch)
    {
        return Some(true);
    }

    // A `+` line that ancestry could not rescue is a genuine unlanded commit.
    // An empty range with no ancestry (never-advanced / rewound branch, or an
    // unresolvable base) is undecidable → defer to the marker.
    if saw_commit {
        Some(false)
    } else {
        None
    }
}

/// `git -C <repo> merge-base --is-ancestor <ancestor> <descendant>` — true iff
/// the command exits 0 (`ancestor` reachable from `descendant`). Any non-zero
/// exit (not an ancestor, unknown ref) or spawn failure → false. Mirrors
/// `supervise::cleanup::git_is_ancestor`.
fn git_is_ancestor(git: &str, repo: &str, ancestor: &str, descendant: &str) -> bool {
    Command::new(git)
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// True when `branch` carries at least one commit not reachable from `base`
/// (`git rev-list --count <base>..<branch> > 0`) — proof it advanced *forward*
/// past its fork point, rejecting a never-committed or rewound branch that is a
/// trivial ancestor of the target yet merged nothing. When `base` is unknown we
/// cannot make this distinction, so we accept (matching the old bare
/// `--is-ancestor` behaviour that the ancestry net stands in for). A git error
/// reads as "not advanced" (`false`), so the net declines rather than guesses.
fn branch_advanced_past_base(git: &str, repo: &str, base: Option<&str>, branch: &str) -> bool {
    let Some(base) = base else {
        return true;
    };
    let out = Command::new(git)
        .arg("-C")
        .arg(repo)
        .args(["rev-list", "--count", &format!("{base}..{branch}")])
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse::<u64>()
            .is_ok_and(|count| count > 0),
        _ => false,
    }
}

/// `Some(trimmed)` for a value safe to pass as a positional git argument; `None`
/// for absent, blank, or **option-injection-shaped** input. `git cherry` (and
/// `merge-base`) take their revisions positionally with no `--` separator, so a
/// ref that begins with `-` (e.g. a branch literally named `-v` or `--abbrev`)
/// would be parsed as a FLAG. Recorded branch/source strings are
/// Taskfleet-controlled (`wt/...`), but rejecting a leading dash keeps a
/// malformed or adversarial projection from steering git's argv.
fn safe_arg(s: Option<&str>) -> Option<&str> {
    s.map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;
    use std::process::Command;

    use crate::supervise::cleanup::git_bin;

    fn git(repo: &Path, args: &[&str]) -> String {
        let out = Command::new(git_bin())
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Build a repo where a worker branch merged into `main`, then the caller
    /// rebases local `main` (replaying the worker's merge under a new hash while
    /// the worker branch ref stays at its pre-rebase tip) — the exact
    /// false-negative trap `merge-base --is-ancestor` falls into. Returns
    /// `(repo, base_sha, worker_branch)`.
    fn repo_with_rebase_replayed_merge() -> (tempfile::TempDir, String, String) {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.email", "t@t"]);
        git(repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("f"), "base\n").unwrap();
        git(repo, &["add", "f"]);
        git(repo, &["commit", "-qm", "base"]);
        let base = git(repo, &["rev-parse", "HEAD"]);

        // Worker branch off base commits real work.
        git(repo, &["checkout", "-q", "-b", "wt/worker"]);
        std::fs::write(repo.join("f"), "base\nwork\n").unwrap();
        git(repo, &["commit", "-qam", "worker change"]);

        // Another session advances main under us.
        git(repo, &["checkout", "-q", "main"]);
        std::fs::write(repo.join("g"), "other\n").unwrap();
        git(repo, &["add", "g"]);
        git(repo, &["commit", "-qm", "other session"]);

        // Merge worker via rebase-then-ff (like merge.sh --rebase). The worker
        // branch ref is left at its pre-rebase tip on purpose (mirrors reality:
        // the ref is not moved to the replayed commit).
        let worker_tip = git(repo, &["rev-parse", "wt/worker"]);
        git(repo, &["checkout", "-q", "-b", "replay", &worker_tip]);
        git(repo, &["rebase", "-q", "main"]);
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["merge", "-q", "--ff-only", "replay"]);
        git(repo, &["branch", "-q", "-D", "replay"]);

        // The caller rebases local main onto a further-moved origin — replaying
        // the merge under yet another new hash.
        git(repo, &["checkout", "-q", "-b", "tmp", &base]);
        std::fs::write(repo.join("h"), "upstream\n").unwrap();
        git(repo, &["add", "h"]);
        git(repo, &["commit", "-qm", "origin moved"]);
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["rebase", "-q", "tmp"]);

        (dir, base, "wt/worker".to_string())
    }

    #[test]
    fn git_verified_landed_survives_caller_rebase() {
        let (dir, base, branch) = repo_with_rebase_replayed_merge();
        let repo = dir.path().to_str().unwrap();

        // Precondition: the OLD ancestry check the skills warned about lies here.
        let is_ancestor = Command::new(git_bin())
            .arg("-C")
            .arg(repo)
            .args(["merge-base", "--is-ancestor", &branch, "main"])
            .status()
            .unwrap()
            .success();
        assert!(
            !is_ancestor,
            "the rebase-replay case must make --is-ancestor lie (return false)"
        );

        // The content-based signal reports the truth: landed, git-verified.
        let inputs = LandingInputs {
            source_repo: Some(repo),
            source_branch: Some("main"),
            branch: Some(&branch),
            base_sha: Some(&base),
            ..Default::default()
        };
        let sig = landing_signal(&inputs, &git_bin());
        assert_eq!(
            sig,
            LandingSignal {
                landed: true,
                method: LandedMethod::GitVerified
            },
            "patch-id equivalence must confirm the landing after a caller rebase"
        );
    }

    #[test]
    fn git_verified_not_landed_for_unmerged_branch() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.email", "t@t"]);
        git(repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("f"), "base\n").unwrap();
        git(repo, &["add", "f"]);
        git(repo, &["commit", "-qm", "base"]);
        let base = git(repo, &["rev-parse", "HEAD"]);
        // Worker commits but never merges.
        git(repo, &["checkout", "-q", "-b", "wt/worker"]);
        std::fs::write(repo.join("f"), "base\nwork\n").unwrap();
        git(repo, &["commit", "-qam", "unmerged work"]);
        git(repo, &["checkout", "-q", "main"]);

        let repo_s = repo.to_str().unwrap();
        let inputs = LandingInputs {
            source_repo: Some(repo_s),
            source_branch: Some("main"),
            branch: Some("wt/worker"),
            base_sha: Some(&base),
            ..Default::default()
        };
        let sig = landing_signal(&inputs, &git_bin());
        assert_eq!(
            sig,
            LandingSignal {
                landed: false,
                method: LandedMethod::GitVerified
            },
            "a branch with a commit absent from the target is not landed"
        );
    }

    #[test]
    fn falls_back_to_report_marker_when_branch_gone() {
        // No repo/branch resolvable → git verification yields None → the durable
        // merge marker decides. This is the real post-teardown case: the branch
        // ref was force-deleted, but `via: explicit-merge` remains.
        let report = json!({ "success": true, "via": "explicit-merge" });
        let inputs = LandingInputs {
            report: Some(&report),
            ..Default::default()
        };
        let sig = landing_signal(&inputs, "git");
        assert_eq!(
            sig,
            LandingSignal {
                landed: true,
                method: LandedMethod::ReportMarker
            }
        );

        // The removed `merge-reconciled` marker is NO LONGER a merge signal (the
        // git-reconcile heuristic was deleted in the A6 thin-supervisor cut) — a
        // report carrying it does not confirm a landing.
        let reconciled = json!({ "success": true, "via": "merge-reconciled" });
        let inputs = LandingInputs {
            report: Some(&reconciled),
            ..Default::default()
        };
        assert_eq!(
            landing_signal(&inputs, "git").method,
            LandedMethod::Unverified
        );
    }

    #[test]
    fn unverified_when_nothing_confirms() {
        // A blocked handoff: success false, no merge marker, no git inputs.
        let report = json!({ "success": false, "summary": "blocked on X" });
        let inputs = LandingInputs {
            report: Some(&report),
            ..Default::default()
        };
        let sig = landing_signal(&inputs, "git");
        assert_eq!(
            sig,
            LandingSignal {
                landed: false,
                method: LandedMethod::Unverified
            }
        );
    }

    #[test]
    fn git_positive_beats_absent_marker_and_marker_beats_git_none() {
        let (dir, base, branch) = repo_with_rebase_replayed_merge();
        let repo = dir.path().to_str().unwrap();

        // Git confirms even with no report at all.
        let inputs = LandingInputs {
            source_repo: Some(repo),
            source_branch: Some("main"),
            branch: Some(&branch),
            base_sha: Some(&base),
            worktree_path: None,
            report: None,
        };
        assert_eq!(
            landing_signal(&inputs, &git_bin()).method,
            LandedMethod::GitVerified
        );

        // With a bogus branch (git → None) but a merge marker, marker wins true.
        let report = json!({ "success": true, "via": "explicit-merge" });
        let inputs = LandingInputs {
            source_repo: Some(repo),
            source_branch: Some("main"),
            branch: Some("does/not/exist"),
            base_sha: Some(&base),
            worktree_path: None,
            report: Some(&report),
        };
        assert_eq!(
            landing_signal(&inputs, &git_bin()),
            LandingSignal {
                landed: true,
                method: LandedMethod::ReportMarker
            }
        );
    }

    /// A fast-forwarded / plain-merge landing whose branch ref IS an ancestor of
    /// the target but whose commits `git cherry` cannot range over when `base_sha`
    /// is absent — the ancestry safety net (rung 2) must still confirm it, with no
    /// report marker in play. (Regression for review finding: `git cherry` alone
    /// returns None on an empty range and would false-negative a real FF merge.)
    #[test]
    fn ancestry_net_confirms_fast_forward_landing_without_base_or_marker() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.email", "t@t"]);
        git(repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("f"), "base\n").unwrap();
        git(repo, &["add", "f"]);
        git(repo, &["commit", "-qm", "base"]);
        // Worker branch commits, then fast-forwards into main (branch ref stays,
        // and IS now an ancestor of main).
        git(repo, &["checkout", "-q", "-b", "wt/worker"]);
        std::fs::write(repo.join("f"), "base\nwork\n").unwrap();
        git(repo, &["commit", "-qam", "worker change"]);
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["merge", "-q", "--ff-only", "wt/worker"]);

        let repo_s = repo.to_str().unwrap();
        // No base_sha and no report marker: only rung 2 can decide.
        let inputs = LandingInputs {
            source_repo: Some(repo_s),
            source_branch: Some("main"),
            branch: Some("wt/worker"),
            base_sha: None,
            report: None,
            ..Default::default()
        };
        assert_eq!(
            landing_signal(&inputs, &git_bin()),
            LandingSignal {
                landed: true,
                method: LandedMethod::GitVerified
            },
            "the ancestry net must confirm a fast-forward landing cherry can't range"
        );
    }

    /// A branch at exactly its fork point (never committed) is a trivial ancestor
    /// of the target but merged nothing — the advanced-past-base guard must stop
    /// the ancestry net from reporting it landed.
    #[test]
    fn ancestry_net_declines_never_advanced_branch() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.email", "t@t"]);
        git(repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("f"), "base\n").unwrap();
        git(repo, &["add", "f"]);
        git(repo, &["commit", "-qm", "base"]);
        let base = git(repo, &["rev-parse", "HEAD"]);
        // Branch forked but never advanced; main moves forward.
        git(repo, &["branch", "wt/worker"]);
        std::fs::write(repo.join("g"), "more\n").unwrap();
        git(repo, &["add", "g"]);
        git(repo, &["commit", "-qm", "main advances"]);

        let repo_s = repo.to_str().unwrap();
        let inputs = LandingInputs {
            source_repo: Some(repo_s),
            source_branch: Some("main"),
            branch: Some("wt/worker"),
            base_sha: Some(&base),
            report: None,
            ..Default::default()
        };
        // cherry range base..branch is empty AND the advanced guard fails → the
        // net declines → undecidable → unverified (no marker).
        assert_eq!(
            landing_signal(&inputs, &git_bin()),
            LandingSignal {
                landed: false,
                method: LandedMethod::Unverified
            },
            "a never-advanced branch that merged nothing must not read as landed"
        );
    }

    /// Post-merge work on the branch: the merge marker is present (an earlier
    /// `run merge`) but git sees a `+` commit the branch added afterward that is
    /// NOT in the target. Git's authoritative negative must win over the stale
    /// marker so the extra work is not silently counted as landed.
    #[test]
    fn git_negative_overrides_stale_marker_on_post_merge_work() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.email", "t@t"]);
        git(repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("f"), "base\n").unwrap();
        git(repo, &["add", "f"]);
        git(repo, &["commit", "-qm", "base"]);
        let base = git(repo, &["rev-parse", "HEAD"]);
        // Worker lands commit A into main (ff), then adds commit B afterward that
        // never merges. Branch is NOT an ancestor of main (B not in main).
        git(repo, &["checkout", "-q", "-b", "wt/worker"]);
        std::fs::write(repo.join("f"), "base\nA\n").unwrap();
        git(repo, &["commit", "-qam", "commit A"]);
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["merge", "-q", "--ff-only", "wt/worker"]);
        git(repo, &["checkout", "-q", "wt/worker"]);
        std::fs::write(repo.join("f"), "base\nA\nB\n").unwrap();
        git(repo, &["commit", "-qam", "commit B (unmerged)"]);
        git(repo, &["checkout", "-q", "main"]);

        let repo_s = repo.to_str().unwrap();
        let report = json!({ "success": true, "via": "explicit-merge" });
        let inputs = LandingInputs {
            source_repo: Some(repo_s),
            source_branch: Some("main"),
            branch: Some("wt/worker"),
            base_sha: Some(&base),
            report: Some(&report),
            ..Default::default()
        };
        assert_eq!(
            landing_signal(&inputs, &git_bin()),
            LandingSignal {
                landed: false,
                method: LandedMethod::GitVerified
            },
            "git's live view of unlanded post-merge work must override a stale marker"
        );
    }

    #[test]
    fn report_marker_requires_success_true() {
        // A merge `via` with success:false (malformed/spoofed) is NOT a marker.
        assert!(!report_has_merge_marker(Some(&json!({
            "success": false, "via": "explicit-merge"
        }))));
        // Missing success is not enough either.
        assert!(!report_has_merge_marker(Some(
            &json!({ "via": "explicit-merge" })
        )));
        // success:true + an explicit-merge via is a marker.
        assert!(report_has_merge_marker(Some(&json!({
            "success": true, "via": "explicit-merge"
        }))));
        // A cancelled report with a legacy merge via is NOT a marker — the helper
        // requires `cancelled` absent/false (tightened from the old `via`-only
        // check; matches classify's cancel-wins stance, issue `retire-via-string`).
        assert!(!report_has_merge_marker(Some(&json!({
            "success": false, "cancelled": true, "reason": "x", "via": "explicit-merge"
        }))));
        // success:true but a non-merge via is not — including the removed
        // `merge-reconciled` (the git-reconcile heuristic was deleted in A6).
        assert!(!report_has_merge_marker(Some(&json!({
            "success": true, "via": "watchdog"
        }))));
        assert!(!report_has_merge_marker(Some(&json!({
            "success": true, "via": "merge-reconciled"
        }))));
    }

    /// Regression (issue `retire-via-string`): the report marker now keys on the
    /// typed `RunMerge` origin, with `via` honored only for a legacy report that
    /// carries NO origin field. A present-but-Agent or present-but-malformed
    /// origin with a forged `via` is NOT a marker, so `landed` does not fall back
    /// to it when git can't verify.
    #[test]
    fn report_marker_prefers_typed_origin() {
        use taskfleet_core::ReportOrigin;

        // A RunMerge origin is a marker even with NO `via` string.
        let mut merged = json!({ "success": true });
        ReportOrigin::RunMerge {
            op_id: Some("op-1".into()),
            worker_oid: Some("abc".into()),
            integrated_oid: None,
        }
        .stamp(&mut merged);
        assert!(report_has_merge_marker(Some(&merged)));

        // Agent origin + a forged `via` is NOT a marker.
        let mut agent = json!({ "success": true, "via": "explicit-merge" });
        ReportOrigin::Agent.stamp(&mut agent);
        assert!(
            !report_has_merge_marker(Some(&agent)),
            "an Agent-origin report must not be a landed marker on a forged via"
        );

        // Malformed origin + a forged `via` is NOT a marker.
        assert!(!report_has_merge_marker(Some(&json!({
            "success": true, "via": "explicit-merge", "origin": "garbage-not-an-object"
        }))));

        // A legacy report (no origin field) with `via` IS a marker (compat).
        assert!(report_has_merge_marker(Some(&json!({
            "success": true, "via": "explicit-merge"
        }))));
    }

    /// End-to-end `landing_signal`: with no git inputs, an Agent-origin report
    /// carrying a forged `via` reads as `Unverified` (not `report-marker`), while a
    /// genuine `RunMerge`-origin report reads as `report-marker`.
    #[test]
    fn landed_fallback_gated_on_typed_origin() {
        use taskfleet_core::ReportOrigin;

        let mut agent = json!({ "success": true, "via": "explicit-merge" });
        ReportOrigin::Agent.stamp(&mut agent);
        let inputs = LandingInputs {
            report: Some(&agent),
            ..Default::default()
        };
        assert_eq!(
            landing_signal(&inputs, "git"),
            LandingSignal {
                landed: false,
                method: LandedMethod::Unverified
            },
            "a forged via on an Agent-origin report must not confirm a landing"
        );

        let mut merged = json!({ "success": true });
        ReportOrigin::RunMerge {
            op_id: None,
            worker_oid: None,
            integrated_oid: None,
        }
        .stamp(&mut merged);
        let inputs = LandingInputs {
            report: Some(&merged),
            ..Default::default()
        };
        assert_eq!(
            landing_signal(&inputs, "git"),
            LandingSignal {
                landed: true,
                method: LandedMethod::ReportMarker
            }
        );
    }

    #[test]
    fn safe_arg_rejects_option_injection() {
        assert_eq!(safe_arg(Some("wt/worker")), Some("wt/worker"));
        assert_eq!(safe_arg(Some("  main  ")), Some("main"));
        assert_eq!(safe_arg(Some("-v")), None);
        assert_eq!(safe_arg(Some("--abbrev=1")), None);
        assert_eq!(safe_arg(Some("")), None);
        assert_eq!(safe_arg(None), None);
    }
}
