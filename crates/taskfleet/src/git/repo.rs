//! The typed [`Git`] backend — one method per git subprocess the supervisor /
//! merge paths issue. See the [module docs](super) for provenance and the
//! fork-and-own drift policy.
//!
//! Every method is a mechanical extraction of a `Command::new(git)` call site
//! that used to live inline in [`crate::supervise::cleanup`]: same subcommand,
//! same args (including `-C <repo>`, `--end-of-options`, and the `--` before a
//! branch name), same stream redirection, and the same conservative-on-error
//! contract (a git error or non-zero exit reads as the *safe* verdict for that
//! call — "nothing unmerged" / "not merged" / "not clean" — never the reverse).
//! Do not soften these: they encode the branch-preservation and ancestry
//! invariants (root CLAUDE.md "State integrity invariants").

use std::process::{Command, Stdio};

use tracing::{info, warn};

/// One registered worktree row from `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRegistration {
    pub path: String,
    /// Short local branch name, or `None` for a detached worktree.
    pub branch: Option<String>,
}

fn parse_worktree_registrations(bytes: &[u8]) -> Option<Vec<WorktreeRegistration>> {
    let mut rows = Vec::new();
    let mut current: Option<WorktreeRegistration> = None;
    for field in bytes.split(|b| *b == 0) {
        if field.is_empty() {
            if let Some(row) = current.take() {
                rows.push(row);
            }
            continue;
        }
        let field = std::str::from_utf8(field).ok()?;
        if let Some(path) = field.strip_prefix("worktree ") {
            if current.is_some() || path.is_empty() {
                return None;
            }
            current = Some(WorktreeRegistration {
                path: path.to_string(),
                branch: None,
            });
        } else if let Some(branch) = field.strip_prefix("branch refs/heads/") {
            let row = current.as_mut()?;
            if branch.is_empty() {
                return None;
            }
            row.branch = Some(branch.to_string());
        }
    }
    if let Some(row) = current {
        rows.push(row);
    }
    (!rows.is_empty()).then_some(rows)
}

/// Typed git backend, pinned to a specific binary. Construct with
/// [`Git::with_bin`], threading the caller's already-resolved binary name (the
/// supervisor resolves it once via [`crate::supervise::cleanup::git_bin`], which
/// honors the `GIT_BIN` test override) — the same seam tests inject a fake git
/// through.
pub struct Git {
    bin: String,
}

impl Git {
    /// A backend pinned to `bin` (the supervisor's already-resolved binary; also
    /// the seam tests inject a fake/real git through).
    pub fn with_bin(bin: impl Into<String>) -> Self {
        Self { bin: bin.into() }
    }

    /// A `Command` for `self.bin` rooted at `repo` (`git -C <repo> …`), matching
    /// how every extracted call site scoped its operation to a repo/worktree.
    fn at(&self, repo: &str) -> Command {
        let mut cmd = Command::new(&self.bin);
        cmd.arg("-C").arg(repo);
        cmd
    }

    /// `git -C <repo> rev-list --count <from>..<to>` → the number of commits
    /// reachable from `to` but not from `from`. `None` on a git error, a
    /// non-zero exit, or unparseable output, so a caller declines/decides
    /// conservatively rather than guess. This is the primitive behind both the
    /// source-relative unmerged-work check and the forward-advance check.
    pub fn rev_list_count(&self, repo: &str, from: &str, to: &str) -> Option<u64> {
        // Reject an empty endpoint: `git rev-list --count ..<to>` (or `<from>..`)
        // silently defaults the omitted side to the repo's ambient `HEAD`, so a
        // malformed empty `source`/`branch` would be measured against the wrong
        // base — a fail-OPEN a teardown safety check must never accept (issue
        // `detached-head-teardown-commit-loss`, review finding A). `--end-of-options`
        // additionally stops a ref that begins with `-` from being parsed as a flag.
        if from.is_empty() || to.is_empty() {
            return None;
        }
        let out = self
            .at(repo)
            .args([
                "rev-list",
                "--count",
                "--end-of-options",
                &format!("{from}..{to}"),
            ])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<u64>()
            .ok()
    }

    /// `git -C <dir> rev-parse HEAD` → the worktree's current HEAD commit oid.
    /// `None` on a git error, a non-zero exit, or empty output — so a caller
    /// treats an unreadable HEAD conservatively (preserve rather than remove).
    ///
    /// This resolves the ACTUAL checked-out commit, independent of any recorded
    /// branch name: it is the ground truth the detached-HEAD teardown guard
    /// (issue `detached-head-teardown-commit-loss`) measures against source, so a
    /// worktree on a detached HEAD (or a stale `Node.branch`) can still be proven
    /// safe-or-unsafe to remove.
    pub fn head_oid(&self, dir: &str) -> Option<String> {
        let out = self
            .at(dir)
            .args(["rev-parse", "HEAD"])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    /// `git -C <dir> symbolic-ref --quiet --short HEAD` → the branch HEAD is
    /// currently on, or `None` when HEAD is DETACHED or the ref cannot be read.
    ///
    /// A caller distinguishes the two `None` causes by pairing this with
    /// [`Self::head_oid`]: when `head_oid` succeeds but this returns `None`, HEAD
    /// is a valid-but-detached commit; when `head_oid` itself fails, HEAD is
    /// unreadable (a git error). The teardown guard treats those differently
    /// (detached → reachability check; unreadable → fail closed).
    pub fn head_branch(&self, dir: &str) -> Option<String> {
        let out = self
            .at(dir)
            .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    /// `git -C <repo> merge-base --is-ancestor <ancestor> <descendant>` — true
    /// when the command exits 0 (`ancestor` is reachable from `descendant`). A
    /// non-zero exit (not an ancestor, or exit 128 for an unknown ref) or a spawn
    /// failure → false. Parameters are named by their TOPOLOGICAL role: the
    /// "merged into source?" check passes `(branch, source)` while the
    /// "fast-forwards?" check passes `(source, branch)` — argument ORDER is what
    /// distinguishes them.
    pub fn is_ancestor(&self, repo: &str, ancestor: &str, descendant: &str) -> bool {
        self.at(repo)
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// `git -C <repo> merge-tree --write-tree <source> <branch>` — an in-memory
    /// three-way merge that writes only to the object store (never a worktree);
    /// exit 0 = clean, 1 = conflicts, ≥2 = git error. `true` only on a clean
    /// exit; any spawn failure or non-zero exit reads as "does not merge
    /// cleanly", so a transient git error never over-reports recoverability.
    /// Requires git ≥ 2.38 (`--write-tree`, Oct 2022).
    pub fn merge_tree_clean(&self, repo: &str, source: &str, branch: &str) -> bool {
        self.at(repo)
            .args(["merge-tree", "--write-tree", source, branch])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// `git -C <dir> status --porcelain --untracked-files=all` → the cleanliness
    /// of the tree as a THREE-state answer:
    ///
    /// - `Some(true)` — status ran and the output is empty: no tracked, staged, or
    ///   untracked changes.
    /// - `Some(false)` — status ran and reported changes: the tree is dirty.
    /// - `None` — status could not be run (spawn failure / non-zero exit): the
    ///   tree's cleanliness is UNVERIFIABLE. The caller decides how to treat that
    ///   (teardown fails closed → preserve).
    ///
    /// `--untracked-files=all` is load-bearing: without it a repo (or the user's
    /// global `~/.gitconfig`) setting `status.showUntrackedFiles=no` would hide an
    /// agent's untracked new files from the check, and a non-force
    /// [`Self::worktree_remove`] would then delete them (issue
    /// `non-merge-teardown-dirty-worktree`). Ignored files are deliberately NOT
    /// reported — they are disposable by policy, exactly as git's own non-force
    /// `worktree remove` treats them.
    pub fn worktree_status_clean(&self, dir: &str) -> Option<bool> {
        match self
            .at(dir)
            .args(["status", "--porcelain", "--untracked-files=all"])
            .stderr(Stdio::null())
            .output()
        {
            Ok(out) if out.status.success() => Some(out.stdout.iter().all(u8::is_ascii_whitespace)),
            _ => None,
        }
    }

    /// Every registered worktree, including its checked-out local branch.
    /// `None` means Git could not provide a trustworthy registration list.
    pub fn worktree_registrations(&self, repo: &str) -> Option<Vec<WorktreeRegistration>> {
        let out = self
            .at(repo)
            .args(["worktree", "list", "--porcelain", "-z"])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        parse_worktree_registrations(&out.stdout)
    }

    /// Whether an exact local branch ref exists. `None` on any Git error.
    pub fn branch_exists(&self, repo: &str, branch: &str) -> Option<bool> {
        if branch.is_empty() {
            return None;
        }
        let status = self
            .at(repo)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        match status.code() {
            Some(0) => Some(true),
            Some(1) => Some(false),
            _ => None,
        }
    }

    /// The main worktree path for a linked worktree. Kept as the small legacy
    /// convenience over the exact registration observer above.
    pub fn main_worktree(&self, dir: &str) -> Option<String> {
        self.worktree_registrations(dir)
            .and_then(|rows| rows.into_iter().next().map(|row| row.path))
    }

    /// `git -C <repo> worktree remove [--force] <worktree_path>` — lenient. The
    /// `force` flag is a data-loss boundary (issue
    /// `non-merge-teardown-dirty-worktree`):
    ///
    /// - `force == true` — used only after an explicit durable authorization:
    ///   either a confirmed `run merge` (`Teardown::Full`) or `run discard
    ///   --force` after exact ownership and dirty-state verification. It
    ///   bulldozes untracked/modified scratch, so callers own that boundary.
    /// - `force == false` — every non-explicit-merge (`SourceRelative`) teardown.
    ///   `cleanup_node` has already preserved a dirty tree upstream, so the tree
    ///   reaching here is expected clean and non-force removal succeeds. But
    ///   non-force is ALSO the atomic safety net: git re-checks cleanliness as part
    ///   of the removal, so a tree that went dirty in the TOCTOU window between the
    ///   upstream `worktree_status_clean` check and here makes git REFUSE (returns
    ///   `false`) instead of silently discarding the new work. The caller treats a
    ///   refusal-with-an-existing-dir as a preservation, not a removal.
    ///
    /// Returns `true` on success so the caller can distinguish an already-gone
    /// worktree (removal fails, dir absent) from a genuine refusal (removal fails,
    /// dir present).
    pub fn worktree_remove(&self, repo: &str, worktree_path: &str, force: bool) -> bool {
        let mut cmd = self.at(repo);
        if force {
            cmd.args(["worktree", "remove", "--force", worktree_path]);
            run_lenient(cmd, &format!("git worktree remove --force {worktree_path}"))
        } else {
            cmd.args(["worktree", "remove", worktree_path]);
            run_lenient(cmd, &format!("git worktree remove {worktree_path}"))
        }
    }

    /// `git -C <repo> branch -{d|D} -- <branch>` — lenient. `force` selects the
    /// flag and is the defense-in-depth safety net against the silent data loss
    /// of issue `blocked-report-deletes-branch`:
    ///
    /// - `force == true` (a confirmed `run merge`, report carries
    ///   `via: "explicit-merge"`) → `-D`, force-delete. The merge is confirmed
    ///   and the branch may already be gone from the main worktree's vantage
    ///   point, so the force is safe and necessary.
    /// - `force == false` → `-d`, the LAST-resort backstop. The caller has
    ///   already preserved a branch with source-unmerged commits via its stronger
    ///   source-relative check, so a branch reaching here is expected clean; `-d`
    ///   still refuses a branch not merged into `HEAD`/upstream, catching the
    ///   residual case where the source check could not run. `-d`'s check is
    ///   ambient-`HEAD`-relative and weaker, which is why it is only the fallback.
    ///
    /// The branch name is passed after `--` so a name beginning with `-` can
    /// never be misparsed as a flag. Returns the captured failure detail (`Some`)
    /// when the delete did not succeed, `None` on success, so the caller can both
    /// record an audit event and surface the incompletion as a warning.
    pub fn branch_delete(&self, repo: &str, branch: &str, force: bool) -> Option<String> {
        let flag = if force { "-D" } else { "-d" };
        let mut cmd = self.at(repo);
        cmd.args(["branch", flag, "--", branch]);
        run_lenient_detail(cmd, &format!("git branch {flag} -- {branch}"))
    }
}

/// Run a best-effort git command, logging its outcome to both `tracing` and
/// stderr (captured to `supervisor.stderr.log`) so the teardown is auditable.
/// Returns `true` only when the command exited successfully; a non-zero exit or
/// spawn error is logged at `warn`, swallowed, and reported as `false` — cleanup
/// is best-effort by contract, but the boolean lets a caller fall back rather
/// than leak. Mirrors the tmux-side lenient runner in [`crate::multiplexer`];
/// git ops live here so the git module owns its own execution.
fn run_lenient(cmd: Command, label: &str) -> bool {
    run_lenient_detail(cmd, label).is_none()
}

/// Like [`run_lenient`] but returns the captured failure detail on a non-zero
/// exit or spawn error (`None` on success), so a caller can record it in an
/// audit event (e.g. `cleanup.branch_remove_failed`). Logging is identical.
fn run_lenient_detail(mut cmd: Command, label: &str) -> Option<String> {
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    match cmd.output() {
        Ok(out) if out.status.success() => {
            info!(target: "taskfleet::supervise", step = label, "cleanup step ok");
            eprintln!("supervisor cleanup: {label}: ok");
            None
        }
        Ok(out) => {
            let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
            warn!(
                target: "taskfleet::supervise",
                step = label,
                code = out.status.code(),
                detail = %detail,
                "cleanup step non-zero (treated as already-done/refused; continuing)"
            );
            eprintln!("supervisor cleanup: {label}: non-zero exit (continuing): {detail}");
            Some(detail)
        }
        Err(e) => {
            warn!(
                target: "taskfleet::supervise",
                step = label,
                error = %e,
                "cleanup step could not spawn (continuing)"
            );
            eprintln!("supervisor cleanup: {label}: spawn failed (continuing): {e}");
            Some(format!("spawn failed: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn nul_worktree_parser_preserves_path_whitespace_exactly() {
        let bytes = b"worktree /tmp/trailing \0HEAD abc\0branch refs/heads/wt/test\0\0";
        assert_eq!(
            parse_worktree_registrations(bytes),
            Some(vec![WorktreeRegistration {
                path: "/tmp/trailing ".to_string(),
                branch: Some("wt/test".to_string()),
            }])
        );
    }
    use tempfile::TempDir;

    /// Run real `git <args>` in `cwd`, asserting success. Sets up fixture repos
    /// for the tests below against a real git binary.
    fn git(cwd: &std::path::Path, args: &[&str]) {
        let ok = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?} failed in {cwd:?}");
    }

    /// A real repo with one commit on `main` and a linked worktree on `wt/foo`,
    /// returning `(repo, worktree)`.
    fn init_repo_with_worktree(tmp: &TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("README"), "x").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "init"]);
        let wt = tmp.path().join("wt");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "wt/foo",
                wt.to_str().unwrap(),
            ],
        );
        (repo, wt)
    }

    fn branch_exists(repo: &std::path::Path, branch: &str) -> bool {
        Command::new("git")
            .current_dir(repo)
            .args(["rev-parse", "--verify", "--quiet", branch])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    /// Write an executable fake `git` that always exits non-zero, to exercise the
    /// conservative-on-error branches without a real repo.
    fn failing_git(dir: &std::path::Path) -> String {
        let p = dir.join("fake-git.sh");
        std::fs::write(&p, "#!/bin/sh\nexit 3\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.to_str().unwrap().to_string()
    }

    #[test]
    fn rev_list_count_counts_commits_ahead() {
        let tmp = TempDir::new().unwrap();
        let (repo, wt) = init_repo_with_worktree(&tmp);
        let g = Git::with_bin("git");
        let repo_s = repo.to_str().unwrap();
        // wt/foo == main → 0 ahead of main.
        assert_eq!(g.rev_list_count(repo_s, "main", "wt/foo"), Some(0));
        // Add a commit on wt/foo → 1 ahead of main.
        std::fs::write(wt.join("f"), "y").unwrap();
        git(&wt, &["add", "-A"]);
        git(&wt, &["commit", "-qm", "work"]);
        assert_eq!(g.rev_list_count(repo_s, "main", "wt/foo"), Some(1));
    }

    #[test]
    fn rev_list_count_none_on_git_error() {
        let tmp = TempDir::new().unwrap();
        let g = Git::with_bin(failing_git(tmp.path()));
        assert_eq!(g.rev_list_count("/nonexistent", "main", "wt/foo"), None);
    }

    /// An empty endpoint must NOT be spawned as `git rev-list --count ..<oid>`
    /// (which git resolves against ambient `HEAD`). It fails closed → `None`
    /// WITHOUT running git (issue `detached-head-teardown-commit-loss`, review
    /// finding A). Proven against a real repo: even with valid refs available, an
    /// empty side returns `None`, never a 0 count from an ambient-HEAD fallback.
    #[test]
    fn rev_list_count_rejects_empty_endpoint() {
        let tmp = TempDir::new().unwrap();
        let (repo, _wt) = init_repo_with_worktree(&tmp);
        let g = Git::with_bin("git");
        let repo_s = repo.to_str().unwrap();
        assert_eq!(g.rev_list_count(repo_s, "", "wt/foo"), None);
        assert_eq!(g.rev_list_count(repo_s, "main", ""), None);
        assert_eq!(g.rev_list_count(repo_s, "", ""), None);
    }

    #[test]
    fn head_oid_and_branch_track_the_worktree() {
        let tmp = TempDir::new().unwrap();
        let (_repo, wt) = init_repo_with_worktree(&tmp);
        let g = Git::with_bin("git");
        let wt_s = wt.to_str().unwrap();
        // On a branch: head_branch names it; head_oid resolves the tip.
        assert_eq!(g.head_branch(wt_s).as_deref(), Some("wt/foo"));
        let on_branch = g.head_oid(wt_s).expect("HEAD resolves on a branch");
        // A full object id is 40 hex chars (SHA-1) or 64 (SHA-256) — accept both
        // so the test is portable to a SHA-256-configured git.
        assert!(
            (on_branch.len() == 40 || on_branch.len() == 64)
                && on_branch.bytes().all(|b| b.is_ascii_hexdigit()),
            "a full object id is 40 or 64 hex chars, got {on_branch:?}"
        );
        // Detach HEAD: head_branch is now None (detached), head_oid still resolves.
        git(&wt, &["checkout", "--detach", "-q"]);
        assert_eq!(
            g.head_branch(wt_s),
            None,
            "a detached HEAD has no symbolic branch"
        );
        assert_eq!(
            g.head_oid(wt_s).as_deref(),
            Some(on_branch.as_str()),
            "detach does not move the commit"
        );
    }

    #[test]
    fn head_oid_and_branch_none_on_git_error() {
        let tmp = TempDir::new().unwrap();
        // A real dir that is not a git repo → both probes fail (non-zero exit).
        let g = Git::with_bin("git");
        let bare = tmp.path().join("bare");
        std::fs::create_dir_all(&bare).unwrap();
        let bare_s = bare.to_str().unwrap();
        assert_eq!(g.head_oid(bare_s), None);
        assert_eq!(g.head_branch(bare_s), None);
        // A failing git binary declines the same way.
        let fg = Git::with_bin(failing_git(tmp.path()));
        assert_eq!(fg.head_oid("/nonexistent"), None);
        assert_eq!(fg.head_branch("/nonexistent"), None);
    }

    #[test]
    fn is_ancestor_reflects_topology() {
        let tmp = TempDir::new().unwrap();
        let (repo, wt) = init_repo_with_worktree(&tmp);
        let g = Git::with_bin("git");
        let repo_s = repo.to_str().unwrap();
        // Fresh branch == main: each is an ancestor of the other.
        assert!(g.is_ancestor(repo_s, "main", "wt/foo"));
        assert!(g.is_ancestor(repo_s, "wt/foo", "main"));
        // Advance wt/foo: main is still an ancestor of wt/foo, but not vice versa.
        std::fs::write(wt.join("f"), "y").unwrap();
        git(&wt, &["add", "-A"]);
        git(&wt, &["commit", "-qm", "work"]);
        assert!(g.is_ancestor(repo_s, "main", "wt/foo"));
        assert!(!g.is_ancestor(repo_s, "wt/foo", "main"));
    }

    #[test]
    fn is_ancestor_false_on_unknown_ref() {
        let tmp = TempDir::new().unwrap();
        let (repo, _wt) = init_repo_with_worktree(&tmp);
        let g = Git::with_bin("git");
        assert!(!g.is_ancestor(repo.to_str().unwrap(), "main", "does/not/exist"));
    }

    #[test]
    fn main_worktree_resolves_from_linked() {
        let tmp = TempDir::new().unwrap();
        let (repo, wt) = init_repo_with_worktree(&tmp);
        let g = Git::with_bin("git");
        let main = g.main_worktree(wt.to_str().unwrap()).unwrap();
        assert_eq!(
            std::fs::canonicalize(&main).unwrap(),
            std::fs::canonicalize(&repo).unwrap()
        );
    }

    #[test]
    fn worktree_remove_removes_and_is_lenient() {
        let tmp = TempDir::new().unwrap();
        let (repo, wt) = init_repo_with_worktree(&tmp);
        let g = Git::with_bin("git");
        let repo_s = repo.to_str().unwrap();
        assert!(g.worktree_remove(repo_s, wt.to_str().unwrap(), true));
        assert!(!wt.exists());
        // Second remove of an already-gone worktree is a lenient no-op (false).
        assert!(!g.worktree_remove(repo_s, wt.to_str().unwrap(), true));
    }

    /// Non-force removal is the atomic dirty-check: it removes a genuinely CLEAN
    /// worktree but REFUSES a dirty one (an untracked file), leaving it in place —
    /// so a TOCTOU race that dirties the tree between the pre-check and removal
    /// cannot silently discard the work (issue `non-merge-teardown-dirty-worktree`).
    #[test]
    fn worktree_remove_nonforce_refuses_dirty_but_removes_clean() {
        let tmp = TempDir::new().unwrap();
        let (repo, wt) = init_repo_with_worktree(&tmp);
        let g = Git::with_bin("git");
        let repo_s = repo.to_str().unwrap();
        // An untracked file makes non-force removal refuse; the worktree survives.
        std::fs::write(wt.join("scratch"), "dirt").unwrap();
        assert!(!g.worktree_remove(repo_s, wt.to_str().unwrap(), false));
        assert!(wt.exists(), "non-force must not delete a dirty worktree");
        // Clean it up; now non-force removal succeeds.
        std::fs::remove_file(wt.join("scratch")).unwrap();
        assert!(g.worktree_remove(repo_s, wt.to_str().unwrap(), false));
        assert!(!wt.exists());
    }

    /// `worktree_status_clean` is a three-state answer, and `--untracked-files=all`
    /// makes an untracked file register as dirty even under
    /// `status.showUntrackedFiles=no` — the config that would otherwise hide it and
    /// let a non-force removal delete it (issue `non-merge-teardown-dirty-worktree`).
    #[test]
    fn worktree_status_clean_tristate_and_uall_defeats_showuntrackedfiles_no() {
        let tmp = TempDir::new().unwrap();
        let (_repo, wt) = init_repo_with_worktree(&tmp);
        let g = Git::with_bin("git");
        let wt_s = wt.to_str().unwrap();
        assert_eq!(g.worktree_status_clean(wt_s), Some(true), "clean tree");
        // Hide untracked files via config — the -uall override must still see them.
        git(&wt, &["config", "status.showUntrackedFiles", "no"]);
        std::fs::write(wt.join("agent-new.rs"), "work").unwrap();
        assert_eq!(
            g.worktree_status_clean(wt_s),
            Some(false),
            "untracked file must count as dirty despite showUntrackedFiles=no"
        );
        // A non-repo dir → status errors → Unverifiable (None).
        assert_eq!(g.worktree_status_clean(tmp.path().to_str().unwrap()), None);
    }

    /// The crux invariant: `-d` (force == false) REFUSES an unmerged branch,
    /// `-D` (force == true) force-deletes it.
    #[test]
    fn branch_delete_d_refuses_unmerged_but_big_d_forces() {
        let tmp = TempDir::new().unwrap();
        let (repo, wt) = init_repo_with_worktree(&tmp);
        let g = Git::with_bin("git");
        let repo_s = repo.to_str().unwrap();
        // Give wt/foo an unmerged commit, then detach the worktree so the branch
        // is deletable-in-principle from the main repo.
        std::fs::write(wt.join("f"), "y").unwrap();
        git(&wt, &["add", "-A"]);
        git(&wt, &["commit", "-qm", "unmerged work"]);
        assert!(g.worktree_remove(repo_s, wt.to_str().unwrap(), true));
        assert!(branch_exists(&repo, "wt/foo"));

        // `-d` refuses: the branch has commits not merged into HEAD (main).
        let detail = g.branch_delete(repo_s, "wt/foo", false);
        assert!(detail.is_some(), "-d must refuse an unmerged branch");
        assert!(
            branch_exists(&repo, "wt/foo"),
            "branch preserved by -d refusal"
        );

        // `-D` force-deletes it.
        assert_eq!(g.branch_delete(repo_s, "wt/foo", true), None);
        assert!(!branch_exists(&repo, "wt/foo"));
    }

    #[test]
    fn branch_delete_d_succeeds_on_merged_branch() {
        let tmp = TempDir::new().unwrap();
        let (repo, wt) = init_repo_with_worktree(&tmp);
        let g = Git::with_bin("git");
        let repo_s = repo.to_str().unwrap();
        // wt/foo is at main (fully merged); detach worktree then `-d` succeeds.
        assert!(g.worktree_remove(repo_s, wt.to_str().unwrap(), true));
        assert_eq!(g.branch_delete(repo_s, "wt/foo", false), None);
        assert!(!branch_exists(&repo, "wt/foo"));
    }
}
