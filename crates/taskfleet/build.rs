use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR must be set when running the build script"),
    );
    generate_skill_files(&manifest_dir);

    let repo_root = manifest_dir
        .ancestors()
        .find(|p| p.join(".git").exists())
        .map(Path::to_path_buf);

    let commit = Command::new("git")
        .arg("-C")
        .arg(repo_root.as_deref().unwrap_or(&manifest_dir))
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=TASKFLEET_GIT_COMMIT={commit}");

    // Set up rerun triggers so the commit env var refreshes when HEAD
    // moves. In a worktree, `.git` is a *file* whose contents are
    // `gitdir: <path>` pointing at the real per-worktree directory
    // under the main repo's `.git/worktrees/<name>/`. Both layouts are
    // handled.
    if let Some(root) = repo_root {
        let git_path = root.join(".git");
        let git_dir = if git_path.is_dir() {
            Some(git_path)
        } else if git_path.is_file() {
            std::fs::read_to_string(&git_path)
                .ok()
                .and_then(|s| {
                    s.lines()
                        .find_map(|l| l.strip_prefix("gitdir:").map(str::trim).map(str::to_string))
                })
                .map(PathBuf::from)
                .map(|p| if p.is_absolute() { p } else { root.join(p) })
        } else {
            None
        };

        if let Some(git_dir) = git_dir {
            let head_path = git_dir.join("HEAD");
            if let Ok(head) = std::fs::read_to_string(&head_path) {
                println!("cargo:rerun-if-changed={}", head_path.display());
                if let Some(rest) = head.strip_prefix("ref: ") {
                    let ref_path = git_dir.join(rest.trim());
                    println!("cargo:rerun-if-changed={}", ref_path.display());
                }
            }
        }
    }
}

/// Substitute `{{CLI_VERSION}}` in every `skills/<name>/SKILL.template.md`
/// with the crate's Cargo version, writing the result to
/// `$OUT_DIR/skills/<name>/SKILL.md`. Embedded into the binary via
/// `include_str!` at compile time so the shipped skill text always matches
/// the binary it ships with (AGENTS-AI-FIRST-CLI §17).
fn generate_skill_files(manifest_dir: &Path) {
    let cli_version = env!("CARGO_PKG_VERSION");
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let skills_dir = manifest_dir.join("skills");
    // Watch the catalog directory itself so adding or removing a
    // `skills/<name>/` subdirectory re-triggers this build script.
    // Without this, a new skill landing in the tree won't be picked up
    // until a `cargo clean`. (Review finding #4.)
    println!("cargo:rerun-if-changed={}", skills_dir.display());

    let entries = std::fs::read_dir(&skills_dir)
        .unwrap_or_else(|e| panic!("read {}: {}", skills_dir.display(), e));
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let template = path.join("SKILL.template.md");
        if !template.exists() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).expect("dirname");
        let body = std::fs::read_to_string(&template)
            .unwrap_or_else(|e| panic!("read {}: {}", template.display(), e));
        let rendered = body.replace("{{CLI_VERSION}}", cli_version);

        let out_skill_dir = out_dir.join("skills").join(name);
        std::fs::create_dir_all(&out_skill_dir)
            .unwrap_or_else(|e| panic!("mkdir {}: {}", out_skill_dir.display(), e));
        let out_file = out_skill_dir.join("SKILL.md");
        std::fs::write(&out_file, &rendered)
            .unwrap_or_else(|e| panic!("write {}: {}", out_file.display(), e));
        println!("cargo:rerun-if-changed={}", template.display());

        // Companion reference files: every other `*.md` in the skill dir
        // (not `SKILL.template.md`, and not a stray rendered `SKILL.md`
        // which would clobber the output written just above) is rendered
        // through the same `{{CLI_VERSION}}` substitution into
        // `$OUT_DIR/skills/<name>/`, so `skill.rs` can `include_str!` it
        // and `skill install` writes it alongside the installed `SKILL.md`.
        // See `EmbeddedResource`. Watch the skill dir itself so adding or
        // removing a companion file re-triggers this build script (the
        // per-file `rerun-if-changed` below only covers files that already
        // exist on this run).
        println!("cargo:rerun-if-changed={}", path.display());
        let files =
            std::fs::read_dir(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
        for file in files.flatten() {
            let fpath = file.path();
            if !fpath.is_file() {
                continue;
            }
            let is_md = fpath.extension().and_then(|e| e.to_str()) == Some("md");
            let fname = match fpath.file_name().and_then(|n| n.to_str()) {
                Some(n) if is_md && n != "SKILL.template.md" && n != "SKILL.md" => n.to_string(),
                _ => continue,
            };
            let fbody = std::fs::read_to_string(&fpath)
                .unwrap_or_else(|e| panic!("read {}: {}", fpath.display(), e));
            let frendered = fbody.replace("{{CLI_VERSION}}", cli_version);
            let out_resource = out_skill_dir.join(&fname);
            std::fs::write(&out_resource, &frendered)
                .unwrap_or_else(|e| panic!("write {}: {}", out_resource.display(), e));
            println!("cargo:rerun-if-changed={}", fpath.display());
        }
    }
}
