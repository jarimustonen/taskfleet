//! `skill` subcommand — list / show / print / install companion AI-skills.
//!
//! Skill files (`SKILL.template.md`) live under
//! `crates/taskfleet/skills/<name>/`. At build time, `build.rs` substitutes
//! `{{CLI_VERSION}}` with the crate's Cargo version and writes the
//! result to `$OUT_DIR/skills/<name>/SKILL.md`. The generated files are
//! embedded into the binary at compile time via `include_str!`, so they
//! version with the CLI. See `AGENTS-AI-FIRST-CLI.md` §15-§17.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::error::CliError;
use crate::home;
use crate::output::{self, OutputFormat, OutputSpec};

/// One embedded skill: name and full SKILL.md text. The description is
/// parsed lazily from the body's frontmatter so the catalog stays a
/// single source of truth (the SKILL.md file).
struct EmbeddedSkill {
    name: &'static str,
    body: &'static str,
    path_in_repo: &'static str,
}

/// A companion reference file that ships alongside a skill's `SKILL.md`.
/// Claude and pi install it as a sibling in the skill directory. Codex embeds
/// it into the single prompt so that artifact remains self-contained.
struct EmbeddedResource {
    filename: &'static str,
    body: &'static str,
    /// The markdown link targets (the payload inside `](…)`) that reference
    /// this companion in the Claude/pi skill bodies. On a Codex install each
    /// is rewritten to the resource's in-document anchor. Anchoring the
    /// rewrite on the full `](target)` form keeps the shorter sibling target
    /// from matching inside a longer `../owner/target` one.
    claude_link_targets: &'static [&'static str],
}

/// Legacy Codex provenance/companion subdir. New Codex installs are
/// self-contained prompts, but the marker remains here so upgrades can safely
/// detect and prune companion files written by older Taskfleet releases.
const CODEX_SHARED_SUBDIR: &str = "_shared";

/// Companion resources for a skill, keyed by skill name. Most skills have
/// none. `build.rs` renders every non-`SKILL.template.md` `*.md` file in a
/// skill's directory into `$OUT_DIR/skills/<name>/`, and the matching
/// `include_str!` below embeds it. `cmd_install` writes each resource as a
/// sibling of Claude/pi `SKILL.md`; Codex embeds the same bytes in its prompt.
fn resources_for(_name: &str) -> &'static [EmbeddedResource] {
    &[]
}

/// Render a skill body for one target agent. Claude and pi receive the
/// byte-identical Agent Skill body and sibling resource tree. Codex receives
/// one self-contained prompt: every referenced resource is appended verbatim
/// under a stable heading, and links are rewritten to that in-document anchor.
fn render_body_for_agent(agent: &str, skill_name: &str, body: &'static str) -> Cow<'static, str> {
    if agent != "codex" {
        return Cow::Borrowed(body);
    }
    render_codex_prompt(body, resources_for(skill_name))
}

fn render_codex_prompt(
    body: &'static str,
    resources: &'static [EmbeddedResource],
) -> Cow<'static, str> {
    if resources.is_empty() {
        return Cow::Borrowed(body);
    }

    let mut rendered = body.to_string();
    for resource in resources {
        let anchor = format!("#taskfleet-resource-{}", resource_anchor(resource.filename));
        for claude_target in resource.claude_link_targets {
            rendered = rendered.replace(&format!("]({claude_target})"), &format!("]({anchor})"));
        }
        write!(
            rendered,
            "\n\n## Taskfleet resource: {}\n<a id=\"taskfleet-resource-{}\"></a>\n\n{}",
            resource.filename,
            resource_anchor(resource.filename),
            resource.body
        )
        .expect("writing to a String cannot fail");
        if !rendered.ends_with('\n') {
            rendered.push('\n');
        }
    }
    Cow::Owned(rendered)
}

fn resource_anchor(filename: &str) -> String {
    filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

const SKILLS: &[EmbeddedSkill] = &[
    EmbeddedSkill {
        name: "taskfleet-overview",
        body: include_str!(concat!(
            env!("OUT_DIR"),
            "/skills/taskfleet-overview/SKILL.md"
        )),
        path_in_repo: "crates/taskfleet/skills/taskfleet-overview/SKILL.template.md",
    },
    EmbeddedSkill {
        name: "taskfleet-run-overview",
        body: include_str!(concat!(
            env!("OUT_DIR"),
            "/skills/taskfleet-run-overview/SKILL.md"
        )),
        path_in_repo: "crates/taskfleet/skills/taskfleet-run-overview/SKILL.template.md",
    },
    EmbeddedSkill {
        name: "taskfleet-spawn-spinoff",
        body: include_str!(concat!(
            env!("OUT_DIR"),
            "/skills/taskfleet-spawn-spinoff/SKILL.md"
        )),
        path_in_repo: "crates/taskfleet/skills/taskfleet-spawn-spinoff/SKILL.template.md",
    },
    EmbeddedSkill {
        name: "worktree-spinoff",
        body: include_str!(concat!(
            env!("OUT_DIR"),
            "/skills/worktree-spinoff/SKILL.md"
        )),
        path_in_repo: "crates/taskfleet/skills/worktree-spinoff/SKILL.template.md",
    },
    EmbeddedSkill {
        name: "worktree-merge",
        body: include_str!(concat!(env!("OUT_DIR"), "/skills/worktree-merge/SKILL.md")),
        path_in_repo: "crates/taskfleet/skills/worktree-merge/SKILL.template.md",
    },
    EmbeddedSkill {
        name: "worktree-research",
        body: include_str!(concat!(
            env!("OUT_DIR"),
            "/skills/worktree-research/SKILL.md"
        )),
        path_in_repo: "crates/taskfleet/skills/worktree-research/SKILL.template.md",
    },
    EmbeddedSkill {
        name: "worktree-technical-decision",
        body: include_str!(concat!(
            env!("OUT_DIR"),
            "/skills/worktree-technical-decision/SKILL.md"
        )),
        path_in_repo: "crates/taskfleet/skills/worktree-technical-decision/SKILL.template.md",
    },
    EmbeddedSkill {
        name: "worktree-bug-analysis",
        body: include_str!(concat!(
            env!("OUT_DIR"),
            "/skills/worktree-bug-analysis/SKILL.md"
        )),
        path_in_repo: "crates/taskfleet/skills/worktree-bug-analysis/SKILL.template.md",
    },
    EmbeddedSkill {
        name: "worktree",
        body: include_str!(concat!(env!("OUT_DIR"), "/skills/worktree/SKILL.md")),
        path_in_repo: "crates/taskfleet/skills/worktree/SKILL.template.md",
    },
    EmbeddedSkill {
        name: "worktree-status",
        body: include_str!(concat!(env!("OUT_DIR"), "/skills/worktree-status/SKILL.md")),
        path_in_repo: "crates/taskfleet/skills/worktree-status/SKILL.template.md",
    },
    EmbeddedSkill {
        name: "stint-start",
        body: include_str!(concat!(env!("OUT_DIR"), "/skills/stint-start/SKILL.md")),
        path_in_repo: "crates/taskfleet/skills/stint-start/SKILL.template.md",
    },
    EmbeddedSkill {
        name: "stint-handoff",
        body: include_str!(concat!(env!("OUT_DIR"), "/skills/stint-handoff/SKILL.md")),
        path_in_repo: "crates/taskfleet/skills/stint-handoff/SKILL.template.md",
    },
    EmbeddedSkill {
        name: "fan-out",
        body: include_str!(concat!(env!("OUT_DIR"), "/skills/fan-out/SKILL.md")),
        path_in_repo: "crates/taskfleet/skills/fan-out/SKILL.template.md",
    },
];

/// Binary version embedded at build time. `build.rs` substitutes this
/// into every shipped SKILL.md's `cli_version:` frontmatter, so `skill
/// print` always returns a body whose declared `cli_version` matches the
/// binary that emitted it (AGENTS-AI-FIRST-CLI §17).
const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Skill-format schema version (the version of the SKILL.md frontmatter +
/// body contract itself, distinct from the envelope `schema_version`).
pub const SKILL_SCHEMA_VERSION: u32 = 1;

/// Provenance marker filename. `cmd_install` drops this hidden file into
/// every claude-layout skill directory it writes (`~/.claude/skills/
/// <name>/.Taskfleet-managed`). Its *presence* is the ONLY signal
/// `prune` and the `skill.orphan.*` doctor check use to decide a directory
/// is safe to delete: a user's own hand-authored skill of the same name
/// never carries it, so it is never touched. See `is_managed_skill_dir`.
const MANAGED_MARKER_FILENAME: &str = ".taskfleet-managed";
/// Public catalog entry used by `version --json` to expose the bundled
/// skill set (AGENTS-AI-FIRST-CLI §17). The on-disk skill is the source
/// of truth for `cli_version`; emit it directly from the parsed
/// frontmatter so the version payload cannot quietly disagree with what
/// `skill print` returns.
#[derive(Debug, Serialize)]
pub struct SkillCatalogEntry {
    pub name: &'static str,
    pub cli_version: String,
    pub schema_version: u32,
}

pub fn catalog() -> Vec<SkillCatalogEntry> {
    SKILLS
        .iter()
        .map(|s| SkillCatalogEntry {
            name: s.name,
            cli_version: parse_frontmatter_field(s.body, "cli_version")
                .unwrap_or_else(|| CLI_VERSION.to_string()),
            schema_version: parse_frontmatter_field(s.body, "schema_version")
                .and_then(|v| v.parse().ok())
                .unwrap_or(SKILL_SCHEMA_VERSION),
        })
        .collect()
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum AgentTarget {
    Claude,
    Pi,
    Codex,
    All,
}

pub struct InstallOptions {
    pub target: Option<PathBuf>,
    pub dest: Option<PathBuf>,
    pub dry_run: bool,
    pub force: bool,
}

/// Names of every skill bundled in this binary. Consumed by `doctor`'s
/// `skill.sync` check so it audits the exact catalog the binary ships.
pub fn bundled_skill_names() -> Vec<&'static str> {
    SKILLS.iter().map(|s| s.name).collect()
}

/// The running binary's version — the authority `skill.sync` compares
/// each on-disk skill's `cli_version` against.
pub fn binary_cli_version() -> &'static str {
    CLI_VERSION
}

/// Default `claude` install path for a skill (`~/.claude/skills/<name>/
/// SKILL.md`). `None` when `HOME` is unset. Used by `doctor` to locate
/// the on-disk copy to compare against the binary.
pub fn claude_default_path(name: &str) -> Option<PathBuf> {
    default_path("claude", name).ok()
}

/// Default `codex` install path for a skill (`~/.codex/prompts/<name>.md`).
/// `None` when `HOME` is unset. The codex layout is FLAT — a skill is a
/// single top-level prompt file, not a per-skill directory. Used by
/// `doctor` to locate the on-disk codex copy to compare against the binary.
pub fn codex_default_path(name: &str) -> Option<PathBuf> {
    default_path("codex", name).ok()
}

/// Read the `cli_version` frontmatter field from an on-disk SKILL.md.
/// `None` when the file is unreadable or has no parseable `cli_version`.
pub fn read_on_disk_cli_version(path: &Path) -> Option<String> {
    let body = fs::read_to_string(path).ok()?;
    parse_frontmatter_field(&body, "cli_version")
}

/// Parse the `cli_version` frontmatter field from an in-memory body (e.g.
/// a companion resource already read off disk). Sibling of
/// [`read_on_disk_cli_version`] for content the caller has in hand.
pub fn cli_version_of(body: &str) -> Option<String> {
    parse_frontmatter_field(body, "cli_version")
}

/// One companion resource bundled alongside a skill's `SKILL.md`, surfaced
/// for the `doctor` `skill.sync.<name>.<file>` companion sub-check: the
/// filename and the embedded (authoritative) body. The expected install
/// path is a sibling of the skill's `SKILL.md` — the doctor derives it from
/// the resolved `SKILL.md` path it already holds.
pub struct CompanionSource {
    pub filename: &'static str,
    pub bundled_body: &'static str,
}

/// Every companion resource bundled for skill `name` (empty for skills that
/// ship none). Consumed by `doctor` to audit that each companion is present
/// and version-synced with the binary, mirroring the SKILL.md `skill.sync`
/// check.
pub fn companion_sources(name: &str) -> Vec<CompanionSource> {
    resources_for(name)
        .iter()
        .map(|r| CompanionSource {
            filename: r.filename,
            bundled_body: r.body,
        })
        .collect()
}

/// External companion resources in the current Codex layout. Always empty:
/// Codex embeds resources in each self-contained prompt. Retained as the
/// doctor compatibility seam so legacy `_shared` marker rows become orphans.
pub fn all_companion_sources() -> Vec<CompanionSource> {
    // Codex's §15 artifact is now self-contained. This compatibility-facing
    // catalog intentionally stays empty so every old `_shared` marker record
    // is classified as an orphan and a forced install can retire it.
    Vec::new()
}

#[derive(Serialize)]
struct SkillSummary {
    name: &'static str,
    description: String,
    cli_version: String,
    skill_schema_version: u32,
}

#[derive(Serialize)]
struct ListPayload {
    supported_agents: [&'static str; 3],
    install: InstallCapabilities,
    skills: Vec<SkillSummary>,
}

#[derive(Serialize)]
struct InstallCapabilities {
    selection_flag: &'static str,
    default: &'static str,
    accepted_values: [&'static str; 4],
    target_flag: &'static str,
    dry_run_flag: &'static str,
    force_flag: &'static str,
    interactive: bool,
    no_clobber_default: bool,
    overwrite_requires_force: bool,
    layouts: [InstallLayout; 3],
}

#[derive(Serialize)]
struct InstallLayout {
    agent: &'static str,
    path: &'static str,
    form: &'static str,
}

#[derive(Serialize)]
struct InstallPayload {
    installed: Vec<InstalledFile>,
    /// Names of de-registered managed skill directories that this install
    /// pruned from `~/.claude/skills/`. Empty in every install form except
    /// the full-catalog default-path claude install (see `cmd_install`).
    pruned: Vec<String>,
    /// Orphan companion files this `--force` install removed: companions a
    /// prior binary recorded in a still-registered skill's provenance marker
    /// that the current binary no longer bundles. Reported as
    /// `<skill>/<filename>` so the offending sibling is unambiguous. Empty
    /// unless a default-path claude `--force` install found stale companions.
    pruned_companions: Vec<String>,
}

#[derive(Serialize)]
struct InstalledFile {
    name: &'static str,
    agent: &'static str,
    path: String,
}

pub fn cmd_list(spec: &OutputSpec, warnings: &[String]) -> Result<(), CliError> {
    let skills: Vec<SkillSummary> = SKILLS
        .iter()
        .map(|s| SkillSummary {
            name: s.name,
            description: parse_description(s.body).unwrap_or_default(),
            cli_version: parse_frontmatter_field(s.body, "cli_version")
                .unwrap_or_else(|| CLI_VERSION.to_string()),
            skill_schema_version: parse_frontmatter_field(s.body, "schema_version")
                .and_then(|value| value.parse().ok())
                .unwrap_or(SKILL_SCHEMA_VERSION),
        })
        .collect();
    match spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => {
            output::emit_discoverable_envelope(
                &ListPayload {
                    supported_agents: ["claude", "pi", "codex"],
                    install: InstallCapabilities {
                        selection_flag: "--agent",
                        default: "all",
                        accepted_values: ["claude", "pi", "codex", "all"],
                        target_flag: "--target",
                        dry_run_flag: "--dry-run",
                        force_flag: "--force",
                        interactive: false,
                        no_clobber_default: true,
                        overwrite_requires_force: true,
                        layouts: [
                            InstallLayout {
                                agent: "claude",
                                path: ".claude/skills/<name>/...",
                                form: "agent-skill-tree",
                            },
                            InstallLayout {
                                agent: "pi",
                                path: ".pi/agent/skills/<name>/...",
                                form: "agent-skill-tree",
                            },
                            InstallLayout {
                                agent: "codex",
                                path: ".codex/prompts/<name>.md",
                                form: "self-contained-prompt",
                            },
                        ],
                    },
                    skills,
                },
                spec,
                warnings,
            )?;
        }
        OutputFormat::Text => {
            for s in &skills {
                println!("{}\t{}", s.name, s.description);
            }
            output::emit_text_warnings(warnings);
        }
    }
    Ok(())
}

pub fn cmd_show(name: &str, spec: &OutputSpec, warnings: &[String]) -> Result<(), CliError> {
    let skill = lookup(name)?;
    match spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => {
            #[derive(Serialize)]
            struct ShowPayload<'a> {
                name: &'a str,
                content: &'a str,
            }
            output::emit_envelope(
                &ShowPayload {
                    name: skill.name,
                    content: skill.body,
                },
                spec,
                warnings,
            )?;
        }
        OutputFormat::Text => {
            print!("{}", skill.body);
            if !skill.body.ends_with('\n') {
                println!();
            }
            output::emit_text_warnings(warnings);
        }
    }
    Ok(())
}

/// `skill print <name>` — stream the canonical embedded SKILL.md text
/// to stdout, byte-identical to what `skill install` would persist
/// (AGENTS-AI-FIRST-CLI §16). Pure read; no filesystem mutation.
pub fn cmd_print(name: &str, spec: &OutputSpec, warnings: &[String]) -> Result<(), CliError> {
    let skill = lookup(name)?;
    let cli_version = parse_frontmatter_field(skill.body, "cli_version")
        .unwrap_or_else(|| CLI_VERSION.to_string());
    let schema_version_skill = parse_frontmatter_field(skill.body, "schema_version")
        .and_then(|v| v.parse().ok())
        .unwrap_or(SKILL_SCHEMA_VERSION);
    match spec.format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            struct PrintPayload<'a> {
                /// Skill-print payload schema; bumps independently from
                /// the envelope's `schema_version`.
                schema_version: u32,
                name: &'a str,
                cli_version: &'a str,
                schema_version_skill: u32,
                content: &'a str,
                path_in_repo: &'a str,
            }
            output::emit_envelope(
                &PrintPayload {
                    schema_version: SKILL_SCHEMA_VERSION,
                    name: skill.name,
                    cli_version: &cli_version,
                    schema_version_skill,
                    content: skill.body,
                    path_in_repo: skill.path_in_repo,
                },
                spec,
                warnings,
            )?;
        }
        // §16 contract: text and jsonl both stream the SKILL.md
        // byte-identically. The structured form is opt-in via `--output
        // json`; the default (jsonl) is byte-identity so `skill print`
        // composes with `cat`, `tee`, and shell redirection without any
        // un-wrapping step.
        OutputFormat::Text | OutputFormat::Jsonl => {
            use std::io::Write as _;
            let mut out = std::io::stdout().lock();
            out.write_all(skill.body.as_bytes())
                .map_err(|e| CliError::system("io_error", format!("write stdout: {e}")))?;
            out.flush()
                .map_err(|e| CliError::system("io_error", format!("flush stdout: {e}")))?;
            output::emit_text_warnings(warnings);
        }
    }
    Ok(())
}

pub fn cmd_install(
    name: Option<&str>,
    agent: AgentTarget,
    options: InstallOptions,
    spec: &OutputSpec,
    warnings: &[String],
) -> Result<(), CliError> {
    let InstallOptions {
        target,
        dest,
        dry_run,
        force,
    } = options;
    // §15: `install [<name>]` installs all skills when no name is given.
    let skills: Vec<&'static EmbeddedSkill> = match name {
        Some(n) => vec![lookup(n)?],
        None => SKILLS.iter().collect(),
    };

    // `--dest` is incompatible with `--agent all` and with the implicit
    // install-all form: a single path cannot host multiple installations.
    if dest.is_some() && matches!(agent, AgentTarget::All) {
        return Err(CliError::user(
            "invalid_arguments",
            "--dest cannot be combined with --agent all",
        ));
    }
    if dest.is_some() && skills.len() > 1 {
        return Err(CliError::user(
            "invalid_arguments",
            "--dest requires a skill name; omit --dest to install all skills",
        ));
    }

    // A default-path pi install maintains its out-of-band provenance record.
    // `--target` is an isolated caller-owned base and never mutates Taskfleet's
    // normal state root.
    let pi_default =
        target.is_none() && dest.is_none() && matches!(agent, AgentTarget::Pi | AgentTarget::All);
    let mut pi_provenance: Option<(PathBuf, PiProvenance)> = None;
    if pi_default {
        if let Some(record_path) = pi_provenance_path() {
            let prov = load_pi_provenance_for_write(&record_path)?;
            pi_provenance = Some((record_path, prov));
        }
    }

    // Build the complete plan before checking or writing any destination.
    // Claude and pi keep native resource trees; Codex embeds resources in its
    // one prompt and therefore contributes exactly one file per skill.
    let mut plan: Vec<PlanItem> = Vec::new();
    let mut mark_dirs: Vec<(&'static str, PathBuf)> = Vec::new();
    for skill in &skills {
        let selected: Vec<&'static str> = match agent {
            AgentTarget::Claude => vec!["claude"],
            AgentTarget::Pi => vec!["pi"],
            AgentTarget::Codex => vec!["codex"],
            AgentTarget::All => vec!["claude", "pi", "codex"],
        };
        for agent_name in selected {
            let path = if let Some(exact) = dest.as_ref() {
                exact.clone()
            } else {
                install_path(agent_name, skill.name, target.as_deref())?
            };
            if agent_name == "claude" && target.is_none() && dest.is_none() {
                if let Some(parent) = path.parent() {
                    mark_dirs.push((skill.name, parent.to_path_buf()));
                }
            }
            if agent_name != "codex" {
                for resource in resources_for(skill.name) {
                    plan.push(PlanItem {
                        agent: agent_name,
                        path: sibling_path(&path, resource.filename),
                        content: Cow::Borrowed(resource.body),
                        kind: PlanKind::Companion {
                            owner: skill.name,
                            filename: resource.filename,
                        },
                    });
                }
            }
            plan.push(PlanItem {
                agent: agent_name,
                path,
                content: render_body_for_agent(agent_name, skill.name, skill.body),
                kind: PlanKind::Skill { name: skill.name },
            });
        }
    }

    // Validate the complete install plan before any write.
    let initial_preflight = preflight(&plan, force)?;
    if dry_run {
        #[derive(Serialize)]
        struct DryRunPayload {
            dry_run: bool,
            would: Vec<DryRunMutation>,
        }
        #[derive(Serialize)]
        struct DryRunMutation {
            action: &'static str,
            resource: &'static str,
            agent: &'static str,
            name: &'static str,
            path: String,
        }
        let would = plan
            .iter()
            .map(|item| DryRunMutation {
                action: if initial_preflight.overwrite_allowed.contains(&item.path) {
                    "overwrite"
                } else {
                    "create"
                },
                resource: match item.kind {
                    PlanKind::Skill { .. } => "skill",
                    PlanKind::Companion { .. } => "skill-resource",
                },
                agent: item.agent,
                name: item.kind.display_name(),
                path: item.path.display().to_string(),
            })
            .collect();
        let mut all_warnings = warnings.to_vec();
        all_warnings.extend(initial_preflight.warnings);
        let payload = DryRunPayload {
            dry_run: true,
            would,
        };
        match spec.format {
            OutputFormat::Json | OutputFormat::Jsonl => {
                output::emit_envelope(&payload, spec, &all_warnings)?;
            }
            OutputFormat::Text => {
                for item in &payload.would {
                    println!(
                        "would {} {} ({}) -> {}",
                        item.action, item.name, item.agent, item.path
                    );
                }
                output::emit_text_warnings(&all_warnings);
            }
        }
        return Ok(());
    }

    let preflight_result = preflight(&plan, force)?;

    // Combine caller-provided warnings (logging init, etc.) with
    // drift-detected ones so the success envelope surfaces both.
    let mut all_warnings: Vec<String> = warnings.to_vec();
    all_warnings.extend(preflight_result.warnings);

    let mut installed = Vec::with_capacity(plan.len());
    // Pi files actually written this run are recorded independently in the
    // provenance record below.
    let mut pi_written: Vec<PiWrite> = Vec::new();
    for item in plan {
        // The set of paths approved for overwrite is decided exclusively
        // by preflight — never recomputed from `path.exists()` in this
        // loop. That keeps the persist_noclobber TOCTOU guarantee intact:
        // a file that did not exist at preflight time will refuse to
        // overwrite, even if a concurrent process created it in the
        // window. (Review finding #1.)
        let allow_overwrite = preflight_result.overwrite_allowed.contains(&item.path);
        write_atomic(&item.path, &item.content, allow_overwrite)?;
        if item.agent == "pi" {
            let hash = sha256_hex(item.content.as_bytes());
            match item.kind {
                PlanKind::Skill { name } => {
                    let cli_version = parse_frontmatter_field(&item.content, "cli_version")
                        .unwrap_or_else(|| CLI_VERSION.to_string());
                    pi_written.push(PiWrite::Skill {
                        name,
                        hash,
                        cli_version,
                    });
                }
                PlanKind::Companion { owner, filename } => {
                    pi_written.push(PiWrite::Companion {
                        owner,
                        filename,
                        hash,
                    });
                }
            }
        }
        installed.push(InstalledFile {
            name: item.kind.display_name(),
            agent: item.agent,
            path: item.path.display().to_string(),
        });
    }

    // Stamp every freshly-installed claude-layout directory with the
    // provenance marker (idempotent, always overwritten — it's ours). The
    // marker is what makes later pruning safe, so a write failure here is
    // fatal rather than silent: without it a genuine orphan would never be
    // recognised as managed. Deduplicated because `--agent all` and the
    // install-all form can enumerate the same dir once per skill.
    //
    // The marker also records the exact companion files this binary wrote
    // (`companion:` lines), so a later binary that drops a companion can
    // recognise the lingering file as an ORPHAN it once managed rather than
    // as a user's own note. Before rewriting the marker we read the copy the
    // prior binary left, diff its recorded companions against what this
    // binary bundles, and act on the leftover ORPHANS:
    //
    // - `--force`: remove the orphan companion file (the redeploy intends the
    //   on-disk catalog to mirror the binary) and drop it from the marker.
    // - non-`--force`: leave the file in place but CARRY IT FORWARD in the new
    //   marker, so `doctor` still recognises it as an orphan (and can suggest
    //   the `--force` fix). Rewriting the marker without it would forget the
    //   file forever while it lingers on disk.
    mark_dirs.sort();
    mark_dirs.dedup();
    let mut pruned_companions: Vec<String> = Vec::new();
    for (skill_name, dir) in &mark_dirs {
        let marker_path = dir.join(MANAGED_MARKER_FILENAME);
        let bundled: Vec<&'static str> = resources_for(skill_name)
            .iter()
            .map(|r| r.filename)
            .collect();
        // Start the new record with everything this binary bundles, then
        // reconcile the companions the prior marker recorded.
        let mut recorded: Vec<String> = bundled.iter().copied().map(String::from).collect();
        let prior_companions = read_managed_companions(&marker_path);
        for prev in prior_companions {
            if bundled.iter().any(|b| *b == prev) {
                continue; // still bundled — already in `recorded`
            }
            // A managed companion this binary no longer ships: an orphan.
            let orphan_path = dir.join(&prev);
            // Only ever touch a regular file we actually wrote — never follow a
            // symlink or recurse into a directory squatting at that name.
            let is_regular =
                fs::symlink_metadata(&orphan_path).is_ok_and(|m| m.file_type().is_file());
            if !is_regular {
                // Nothing safe to clean and nothing on disk to keep tracking:
                // drop it from the marker.
                continue;
            }
            if force {
                match fs::remove_file(&orphan_path) {
                    Ok(()) => {
                        all_warnings.push(format!(
                            "skill_companion_pruned: removed orphan companion '{prev}' for skill '{skill_name}' at {}",
                            orphan_path.display()
                        ));
                        pruned_companions.push(format!("{skill_name}/{prev}"));
                        // dropped from `recorded` → no longer tracked
                    }
                    Err(e) => {
                        all_warnings.push(format!(
                            "skill_companion_prune_failed: could not remove orphan companion '{prev}' for skill '{skill_name}' at {}: {e}",
                            orphan_path.display()
                        ));
                        recorded.push(prev); // still on disk — keep it tracked
                    }
                }
            } else {
                recorded.push(prev); // preserve tracking for doctor
            }
        }
        recorded.sort();
        recorded.dedup();
        write_marker(&marker_path, skill_name, &recorded)?;
    }

    // Prune de-registered managed skills. Scoped to the full-catalog
    // (`name.is_none()`), default-path (`dest.is_none()`), `--force`,
    // claude-targeting install: that is exactly the `skill install --force`
    // redeploy where the caller intends the on-disk catalog to mirror the
    // binary's. `--force` is required so a plain, non-destructive-looking
    // `skill install` can never delete a directory as a side effect. A
    // targeted `skill install <name>` must NEVER nuke the rest of the
    // catalog, so it is excluded too. Only directories carrying a VALID
    // marker (see `is_managed_skill_dir`) AND absent from the shipped
    // catalog are removed — a user's hand-authored or copied skill is
    // spared. A prune failure is a maintenance hiccup, not an install
    // failure: the catalog already landed, so we warn and carry on rather
    // than erroring out with the success payload unreported.
    let mut pruned: Vec<String> = Vec::new();
    let prune_eligible = name.is_none()
        && target.is_none()
        && dest.is_none()
        && force
        && matches!(agent, AgentTarget::Claude | AgentTarget::All);
    if prune_eligible {
        if let Some(root) = claude_skills_root() {
            let registered: HashSet<&str> = SKILLS.iter().map(|s| s.name).collect();
            for (orphan_name, orphan_path) in managed_orphan_dirs(&root, &registered) {
                match prune_managed_claude_dir(&orphan_path) {
                    Ok(true) => {
                        all_warnings.push(format!(
                            "skill_pruned: removed de-registered managed skill '{orphan_name}' at {}",
                            orphan_path.display()
                        ));
                        pruned.push(orphan_name);
                    }
                    Ok(false) => all_warnings.push(format!(
                        "skill_prune_preserved: de-registered skill '{orphan_name}' contains edited or user-owned bytes at {}; left unchanged",
                        orphan_path.display()
                    )),
                    Err(e) => all_warnings.push(format!(
                        "skill_prune_failed: could not remove de-registered skill '{orphan_name}' at {}: {e}",
                        orphan_path.display()
                    )),
                }
            }
        }
    }

    // Codex flat-layout provenance + prune. Codex's prompts dir is flat (no
    // per-skill directory), so a single shared marker at
    // `_shared/.Taskfleet-managed` records which prompts + companions
    // Taskfleet installed there — the only signal that makes codex-side
    // pruning + orphan detection safe. Maintained whenever the codex layout
    // is installed to its DEFAULT path (never for a caller-managed `--dest`).
    // The marker MERGES with any prior record (union) so a targeted
    // single-skill install never forgets the rest of the managed set. Orphan
    // pruning (removing a de-registered prompt/companion) is gated to the
    // full-catalog `--force` redeploy, symmetric with the claude dir prune
    // above; a plain `skill install` never deletes anything as a side effect.
    let codex_default = target.is_none()
        && dest.is_none()
        && matches!(agent, AgentTarget::Codex | AgentTarget::All);
    if codex_default {
        if let (Some(prompts_root), Some(shared_root)) = (codex_prompts_root(), codex_shared_root())
        {
            let marker_path = shared_root.join(MANAGED_MARKER_FILENAME);

            // Everything this install just wrote to the Codex layout: one
            // self-contained prompt per skill. Companion records come only
            // from older markers and remain long enough for a forced full
            // install to prune those retired external files safely.
            let mut recorded_prompts: HashSet<String> =
                skills.iter().map(|s| s.name.to_string()).collect();
            let mut recorded_companions: HashSet<String> = HashSet::new();
            // Union with the prior marker so a targeted install (or a prune
            // that must recognise a de-registered entry) keeps the full set.
            recorded_prompts.extend(read_marker_records(&marker_path, "prompt"));
            recorded_companions.extend(read_marker_records(&marker_path, "companion"));
            let codex_prune_eligible = name.is_none() && force;
            if codex_prune_eligible {
                let registered: HashSet<&str> = SKILLS.iter().map(|s| s.name).collect();
                // Codex now embeds every resource, so no external companion
                // remains bundled in this layout.
                let bundled_companions: HashSet<&str> = HashSet::new();

                // Orphan codex prompts: recorded but no longer in the catalog.
                let orphan_prompts: Vec<String> = recorded_prompts
                    .iter()
                    .filter(|p| !registered.contains(p.as_str()))
                    .cloned()
                    .collect();
                for orphan in orphan_prompts {
                    let prompt_path = prompts_root.join(format!("{orphan}.md"));
                    match prune_codex_file(
                        &prompt_path,
                        &format!("skill_pruned: removed de-registered managed codex prompt '{orphan}'"),
                        &format!("skill_prune_failed: could not remove de-registered codex prompt '{orphan}'"),
                        &mut all_warnings,
                    ) {
                        CodexPruneOutcome::Removed => {
                            pruned.push(orphan.clone());
                            recorded_prompts.remove(&orphan);
                        }
                        CodexPruneOutcome::Dropped => {
                            recorded_prompts.remove(&orphan);
                        }
                        CodexPruneOutcome::Kept => {}
                    }
                }

                // Orphan codex companions: recorded but no bundled skill ships
                // them any more (the last referrer was removed).
                let orphan_companions: Vec<String> = recorded_companions
                    .iter()
                    .filter(|c| !bundled_companions.contains(c.as_str()))
                    .cloned()
                    .collect();
                for orphan in orphan_companions {
                    let companion_path = shared_root.join(&orphan);
                    match prune_codex_file(
                        &companion_path,
                        &format!("skill_companion_pruned: removed orphan codex companion '_shared/{orphan}'"),
                        &format!("skill_companion_prune_failed: could not remove orphan codex companion '_shared/{orphan}'"),
                        &mut all_warnings,
                    ) {
                        CodexPruneOutcome::Removed => {
                            pruned_companions.push(format!("{CODEX_SHARED_SUBDIR}/{orphan}"));
                            recorded_companions.remove(&orphan);
                        }
                        CodexPruneOutcome::Dropped => {
                            recorded_companions.remove(&orphan);
                        }
                        CodexPruneOutcome::Kept => {}
                    }
                }
            }

            // Persist the reconciled marker. Create `_shared/` first: a codex
            // install of a companion-less skill would not otherwise materialise
            // it, but we still need the marker for later pruning of the flat
            // prompt file. A marker-write failure is fatal (like the claude
            // marker): without it a genuine orphan is never recognised.
            let mut prompts: Vec<String> = recorded_prompts.into_iter().collect();
            prompts.sort();
            let mut companions: Vec<String> = recorded_companions.into_iter().collect();
            companions.sort();
            fs::create_dir_all(&shared_root).map_err(|e| {
                CliError::system(
                    "create_dir_failed",
                    format!("could not create {}: {}", shared_root.display(), e),
                )
            })?;
            write_codex_marker(&marker_path, &prompts, &companions)?;
        }
    }

    // pi.dev mirror lifecycle (out-of-band provenance). Maintained whenever
    // this install dual-homed into pi — i.e. the same condition `cmd_install`
    // used to push the pi `PlanItem`s (default path, claude-format target).
    // Two steps, both keyed SOLELY on the out-of-band record (the pi dir has no
    // in-dir marker):
    //
    //   1. Record every pi mirror we just wrote (union-merged with the prior
    //      record so a targeted single-skill install never forgets the rest of
    //      the managed set — symmetric with the codex marker union).
    //   2. On the full-catalog `--force` redeploy (the same gate as the claude
    //      prune), prune pi mirrors of de-registered skills — but only ones the
    //      record names AND whose on-disk bytes still hash to the recorded value
    //      (strong evidence it is our unmodified copy). A user-taken-over or
    //      hand-authored pi dir is never recorded, so it is not touched.
    //
    // Like the claude/codex marker updates, the record read-modify-write is
    // unlocked: two concurrent `skill install` runs can lose one another's
    // additions (parity debt, not introduced here). Mutation commands are not
    // meant to run concurrently — see `crates/taskfleet/AGENTS.md`.
    if let Some((record_path, mut prov)) = pi_provenance {
        prov.schema_version = PI_PROVENANCE_SCHEMA_VERSION;
        // Record every pi file we just wrote as an independent `files` entry
        // under its owning skill. In the flat per-file model a companion no
        // longer has to attach to a pre-existing body record: it files itself
        // directly (creating the skill entry if the body write was skipped),
        // which removes the pre-flat `pi_companion_unrecorded` edge entirely
        // (issue `pi-provenance-flat-file-model`). A `SKILL.md` write also
        // refreshes the skill's `cli_version`. `skipped` (present, non-force) pi
        // files are absent from `pi_written`, so their prior `files` entries are
        // carried forward untouched.
        for w in &pi_written {
            match w {
                PiWrite::Skill {
                    name,
                    hash,
                    cli_version,
                } => {
                    let rec = prov.skills.entry((*name).to_string()).or_default();
                    rec.cli_version.clone_from(cli_version);
                    rec.files.insert(
                        PI_SKILL_FILENAME.to_string(),
                        PiFileRecord {
                            sha256: hash.clone(),
                            kind: PiFileKind::Skill,
                        },
                    );
                }
                PiWrite::Companion {
                    owner,
                    filename,
                    hash,
                } => {
                    let rec = prov.skills.entry((*owner).to_string()).or_default();
                    rec.files.insert(
                        (*filename).to_string(),
                        PiFileRecord {
                            sha256: hash.clone(),
                            kind: PiFileKind::Companion,
                        },
                    );
                }
            }
        }

        // Reconcile each STILL-REGISTERED installed skill's recorded companions
        // against what this binary now bundles: a companion a prior binary
        // mirrored that the current one dropped is an orphan. Without this, the
        // `skill.orphan.<name>.pi.<file>` doctor check would flag a state its only
        // suggested fix (`skill install <name> --force`) could never clear — a
        // permanent unfixable warning loop (review finding F1). Mirrors the claude
        // `mark_dirs` companion reconciliation. De-registered skills are handled
        // by the prune block below; this handles skills that survive but shed a
        // companion.
        for skill in &skills {
            reconcile_pi_companions(
                skill.name,
                &mut prov,
                force,
                &mut pruned_companions,
                &mut all_warnings,
            );
        }

        if prune_eligible {
            let registered: HashSet<&str> = SKILLS.iter().map(|s| s.name).collect();
            // De-registered names the record still tracks — the only prune
            // candidates. Collected first (from `BTreeMap::keys`, so sorted +
            // deterministic) so we don't mutate `prov.skills` while iterating it.
            // The registered check is case-insensitive as well as exact, symmetric
            // with `managed_orphan_dirs`: on a case-insensitive filesystem (APFS) a
            // corrupt record key that is a case variant of a registered skill would
            // otherwise resolve to that skill's live dir and, if the hash matched,
            // delete a registered mirror (review finding F5).
            let orphan_names: Vec<String> = prov
                .skills
                .keys()
                .filter(|n| {
                    !registered.contains(n.as_str())
                        && !registered.iter().any(|r| r.eq_ignore_ascii_case(n))
                })
                .cloned()
                .collect();
            for orphan in orphan_names {
                // Never let a record-sourced key that is not a single normal path
                // component reach the filesystem (review finding E). It stays in
                // the record (inert — doctor skips it too) but is never acted on.
                if !is_simple_skill_name(&orphan) {
                    all_warnings.push(format!(
                        "pi_provenance_bad_name: ignoring pi provenance entry '{orphan}' (not a simple skill name)"
                    ));
                    continue;
                }
                // `orphan` came straight from `prov.skills.keys()`, so the entry
                // is present — mutate it in place. The prune removes each
                // successfully-handled (deleted / relinquished / absent) file
                // from the record's `files` map and reports whether the body was
                // deleted; a file whose delete FAILED is left in `files` so the
                // entry survives for a retry.
                let rec = prov.skills.get_mut(&orphan).expect("orphan key present");
                let outcome = prune_pi_mirror(&orphan, rec, &mut all_warnings);
                if outcome.body_removed {
                    pruned.push(orphan.clone());
                }
                // Drop the skill entry once every file is accounted for (nothing
                // left tracked); a Kept file keeps the entry so a later redeploy
                // retries and `doctor` still flags the leftover.
                if rec.files.is_empty() {
                    prov.skills.remove(&orphan);
                }
            }
        }

        write_pi_provenance(&record_path, &prov)?;
    }

    // De-registered skills can be pruned from more than one layout (claude,
    // codex, pi) in a single `--force` redeploy, each pushing the same name here.
    // Deduplicate so the `pruned` payload is a set (review finding B).
    pruned.sort();
    pruned.dedup();
    // Symmetric with `pruned`: a `<skill>/<file>` entry can be reported by more
    // than one layout (claude + pi both key companions that way), so normalise
    // the payload to a set.
    pruned_companions.sort();
    pruned_companions.dedup();

    let payload = InstallPayload {
        installed,
        pruned,
        pruned_companions,
    };
    match spec.format {
        OutputFormat::Json | OutputFormat::Jsonl => {
            output::emit_envelope(&payload, spec, &all_warnings)?;
        }
        OutputFormat::Text => {
            for f in &payload.installed {
                println!("installed {} ({}) -> {}", f.name, f.agent, f.path);
            }
            for name in &payload.pruned {
                println!("pruned {name} (de-registered)");
            }
            for entry in &payload.pruned_companions {
                println!("pruned {entry} (orphan companion)");
            }
            output::emit_text_warnings(&all_warnings);
        }
    }
    Ok(())
}

/// Compare two `cli_version` strings via the `semver` crate. Returns
/// `None` if either side fails to parse as semver — callers treat that
/// as "unversioned / legacy" rather than guessing an ordering.
/// (Review finding #2 — the previous ad-hoc parser inverted prerelease
/// ordering: `1.0.0-alpha > 1.0.0` instead of `<`.)
fn compare_versions(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    let av = semver::Version::parse(a).ok()?;
    let bv = semver::Version::parse(b).ok()?;
    Some(av.cmp(&bv))
}

/// Outcome of the install preflight pass.
///
/// `overwrite_allowed` is the *authoritative* set of paths the write
/// loop is permitted to clobber. Computed once, then never recomputed —
/// see `cmd_install` for the TOCTOU rationale.
struct PreflightResult {
    warnings: Vec<String>,
    overwrite_allowed: HashSet<PathBuf>,
}

/// What one [`PlanItem`] writes: a skill's `SKILL.md` body or one of its
/// companion resources. Replaces the pre-flat-model stringly-typed
/// `agent == "pi"` + `pi_companion_of: Option<_>` inference — the pi write loop
/// now matches this enum directly, and any agent's item carries the same
/// classification (a companion always knows its owning skill).
enum PlanKind {
    /// A skill body (`SKILL.md`) for `name`.
    Skill { name: &'static str },
    /// A companion resource `filename` owned by skill `owner`.
    Companion {
        owner: &'static str,
        filename: &'static str,
    },
}

impl PlanKind {
    /// The name the install payload and drift warnings report: the skill name
    /// for a body, the resource filename for a companion.
    fn display_name(&self) -> &'static str {
        match self {
            PlanKind::Skill { name } => name,
            PlanKind::Companion { filename, .. } => filename,
        }
    }
}

/// One file the install will write: a skill's `SKILL.md` or one of its
/// companion resources.
struct PlanItem {
    agent: &'static str,
    path: PathBuf,
    /// Bytes to write. Borrowed for the claude layout and companions (the
    /// embedded source verbatim); owned when a codex body needed companion
    /// links rewritten (see `render_body_for_agent`).
    content: Cow<'static, str>,
    /// Whether this item is a skill body or a companion, and its identifying
    /// name(s). The pi provenance update matches this to file the write under
    /// the right skill. Codex plans never carry companion items because their
    /// bytes are embedded in the prompt.
    kind: PlanKind,
}

/// Resolve a companion resource's destination: a file named `filename` in
/// the same directory as the skill's `SKILL.md` destination `skill_path`.
/// A bare relative `skill_path` (empty parent, e.g. `--dest SKILL.md`)
/// places the resource in the current directory.
fn sibling_path(skill_path: &Path, filename: &str) -> PathBuf {
    match skill_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.join(filename),
        _ => PathBuf::from(filename),
    }
}

/// Reject the whole install plan before touching the filesystem when any
/// destination already exists (without `--force`) or appears twice in
/// the plan. Catches the partial-install retry trap noted by the review:
/// without preflight, writing N targets sequentially can leave the user
/// with a half-installed catalog and an ambiguous error on retry.
/// Inspect a destination without following symlinks. Unlike [`Path::exists`],
/// this reports a dangling symlink as present, so `--force` can replace the
/// link itself rather than letting the later atomic rename fail unexpectedly.
fn destination_metadata(path: &Path) -> Result<Option<fs::Metadata>, CliError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(CliError::system(
            "metadata_failed",
            format!("could not inspect {}: {error}", path.display()),
        )),
    }
}

fn preflight(plan: &[PlanItem], force: bool) -> Result<PreflightResult, CliError> {
    use std::cmp::Ordering;
    let mut seen: HashSet<&Path> = HashSet::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut overwrite_allowed: HashSet<PathBuf> = HashSet::new();

    for PlanItem {
        agent: _,
        path,
        content: _,
        kind,
    } in plan
    {
        let name = kind.display_name();
        if !seen.insert(path.as_path()) {
            return Err(CliError::user(
                "duplicate_destination",
                format!("destination appears more than once: {}", path.display()),
            )
            .with_invalid_value(path.display().to_string()));
        }
        let destination = destination_metadata(path)?;
        // `symlink_metadata` intentionally classifies a symlink to a directory
        // as a symlink, not a directory: it is a replaceable file destination
        // when `--force` is passed.
        if destination
            .as_ref()
            .is_some_and(|metadata| metadata.file_type().is_dir())
        {
            return Err(CliError::user(
                "invalid_dest",
                format!("destination is a directory: {}", path.display()),
            )
            .with_invalid_value(path.display().to_string()));
        }
        if destination.is_none() {
            // No file → the write loop will refuse to clobber via
            // persist_noclobber. Do not insert into `overwrite_allowed`.
            continue;
        }
        // Existing target: classify by semver-correct `cli_version`
        // drift relative to the running binary (AGENTS-AI-FIRST-CLI §17).
        // An unreadable or unparseable `cli_version` lands in the
        // "legacy / unversioned" arm so we never invent an ordering.
        let on_disk_raw = fs::read_to_string(path)
            .ok()
            .and_then(|s| parse_frontmatter_field(&s, "cli_version"));
        let drift = on_disk_raw
            .as_deref()
            .and_then(|v| compare_versions(v, CLI_VERSION).map(|ord| (v, ord)));
        match drift {
            Some((v, Ordering::Greater)) if !force => {
                return Err(CliError::system(
                    "skill_version_too_new",
                    format!(
                        "{}: on-disk skill is cli_version {} but binary is {}; pass --force to overwrite anyway",
                        path.display(), v, CLI_VERSION
                    ),
                )
                .with_invalid_value(path.display().to_string()));
            }
            _ if !force => {
                // §15's no-clobber boundary is uniform across every runtime:
                // even a recognisably older managed copy requires the caller's
                // explicit `--force` authorization.
                return Err(CliError::system(
                    "refused_overwrite",
                    format!("{} already exists; pass --force to overwrite", path.display()),
                )
                .with_invalid_value(path.display().to_string()));
            }
            Some((v, Ordering::Less)) => warnings.push(format!(
                "skill_version_drift: {name} on disk is {v}; binary ships {CLI_VERSION}; --force overwriting"
            )),
            Some((v, Ordering::Greater)) => warnings.push(format!(
                "skill_version_drift: {name} on disk is {v} (newer than binary {CLI_VERSION}); --force overwriting"
            )),
            Some((_, Ordering::Equal)) | None => {}
        }
        overwrite_allowed.insert(path.clone());
    }
    Ok(PreflightResult {
        warnings,
        overwrite_allowed,
    })
}

fn lookup(name: &str) -> Result<&'static EmbeddedSkill, CliError> {
    SKILLS.iter().find(|s| s.name == name).ok_or_else(|| {
        let available: Vec<&str> = SKILLS.iter().map(|s| s.name).collect();
        CliError::user(
            "skill_not_found",
            format!(
                "no skill named '{}'; available: {}",
                name,
                available.join(", ")
            ),
        )
        .with_invalid_value(name.to_string())
        .with_expected(serde_json::json!({ "one_of": available }))
    })
}

fn default_path(agent: &str, name: &str) -> Result<PathBuf, CliError> {
    install_path(agent, name, None)
}

/// Resolve one canonical §15 layout. `--target` replaces only the install
/// base; the runtime-specific relative layout remains unchanged.
fn install_path(agent: &str, name: &str, target: Option<&Path>) -> Result<PathBuf, CliError> {
    let base = match target {
        Some(path) => path.to_path_buf(),
        None => PathBuf::from(std::env::var("HOME").map_err(|_| {
            CliError::system(
                "home_unset",
                "HOME is not set; cannot resolve the install base (pass --target)",
            )
        })?),
    };
    Ok(match agent {
        "claude" => base.join(".claude/skills").join(name).join("SKILL.md"),
        "pi" => base.join(".pi/agent/skills").join(name).join("SKILL.md"),
        "codex" => base.join(".codex/prompts").join(format!("{name}.md")),
        other => {
            return Err(CliError::user(
                "invalid_agent",
                format!("unknown agent '{other}'"),
            ))
        }
    })
}

/// Root of the claude skill-install layout (`~/.claude/skills`). `None`
/// when `HOME` is unset. Both `prune` and the `skill.orphan.*` doctor
/// check scan this directory for managed-but-de-registered skills.
pub fn claude_skills_root() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".claude/skills"))
}

/// Root of the codex flat prompts layout (`~/.codex/prompts`), where each
/// skill installs as a single top-level `<name>.md`. `None` when `HOME` is
/// unset. Both the codex prune path and the `skill.orphan.codex.*` doctor
/// check resolve prompt files against this root.
pub fn codex_prompts_root() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".codex/prompts"))
}

/// The codex shared-companion dir (`~/.codex/prompts/_shared`). `None` when
/// `HOME` is unset. Holds the `_shared/<file>` companions AND the single
/// codex provenance marker.
pub fn codex_shared_root() -> Option<PathBuf> {
    codex_prompts_root().map(|p| p.join(CODEX_SHARED_SUBDIR))
}

/// Path to the single codex provenance marker
/// (`~/.codex/prompts/_shared/.Taskfleet-managed`). `None` when `HOME`
/// is unset. Codex's prompts dir is flat, so — unlike claude's per-skill
/// directory marker — ONE shared marker records every prompt + companion
/// Taskfleet installed there. Its presence is what makes codex-side
/// pruning + orphan detection safe: a user's own prompt of the same name is
/// never recorded, so it is never touched.
fn codex_marker_path() -> Option<PathBuf> {
    codex_shared_root().map(|p| p.join(MANAGED_MARKER_FILENAME))
}

/// Codex prompt names the shared provenance marker records as
/// Taskfleet-managed (sorted, deduped). Empty when `HOME` is unset or
/// the marker is absent/unreadable — which is precisely the signal that
/// Taskfleet does not manage codex on this host (e.g. a claude-only
/// install), so `doctor` emits no codex checks and a claude-primary tree
/// stays 0-warn.
pub fn codex_managed_prompts() -> Vec<String> {
    let Some(marker) = codex_marker_path() else {
        return Vec::new();
    };
    let mut v = read_marker_records(&marker, "prompt");
    v.sort();
    v.dedup();
    v
}

/// Companion filenames the shared codex provenance marker records as
/// managed — the `_shared/<file>` companions Taskfleet installed
/// (sorted, deduped). Empty under the same conditions as
/// [`codex_managed_prompts`].
pub fn codex_managed_companions() -> Vec<String> {
    let Some(marker) = codex_marker_path() else {
        return Vec::new();
    };
    let mut v = read_marker_records(&marker, "companion");
    v.sort();
    v.dedup();
    v
}

// ---------------------------------------------------------------------------
// pi.dev mirror lifecycle (out-of-band provenance).
//
// The pi mirror (`~/.pi/agent/skills/<name>/SKILL.md`, written by `cmd_install`)
// deliberately carries NO in-dir `.Taskfleet-managed` marker — the
// `pidev-dual-home-skills` contract forbids one so the pi corpus stays a pure
// skill-body mirror. Without an in-dir provenance signal we cannot tell an
// Taskfleet-written pi dir from a user's own hand-authored pi skill, so a
// naive "pi orphan" prune/warn would false-positive on every user skill.
//
// The provenance therefore lives OUT-OF-BAND, in a single JSON record under the
// taskfleet state root (`<root>/state/pi-installed-skills.json`), keyed by
// skill name → a flat per-file map (`files: { <relpath>: { sha256, kind } }`)
// of every mirrored file we last wrote (the `SKILL.md` body AND each companion
// sibling), plus the skill's `cli_version`. It is the SOLE authority for two
// safety-critical decisions:
//
//   - prune (issue task 2): a pi mirror is a prune candidate only if its name is
//     recorded here AND the on-disk bytes still hash to the recorded value (proof
//     it is our unmodified copy). A user-taken-over or hand-authored pi dir is
//     never recorded, so it is never touched.
//   - `doctor` drift (issue task 3): the recorded set gates the `skill.sync.
//     <name>.pi` / `skill.orphan.<name>.pi` checks, so a host that never dual-
//     homed into pi emits no pi checks and stays 0-warn.

/// Schema version of the pi provenance record. Bumped independently of the
/// SKILL.md and envelope schema versions if the record's shape ever changes.
///
/// **v3** flattened the record to a per-file model: each skill entry became
/// `{ cli_version, files: { <relpath>: { sha256, kind } } }`, where every
/// mirrored file — the `SKILL.md` body AND each companion sibling — is tracked
/// as one independent `PiFileRecord`. Before v3 the body was a privileged
/// ownership root (`{ sha256, cli_version, companions: { <file>: sha } }`) that
/// nested companions under it, which forced several lifecycle edge-case
/// point-fixes (a companion written while the body write was skipped had no
/// record to attach to; prune coupled companion cleanup to body divergence).
/// The flat model makes ownership/relinquish/retry decisions per file and
/// removes those couplings (issue `pi-provenance-flat-file-model`).
///
/// **v2** (superseded) added the per-skill `companions` map to v1's bare
/// `{ sha256, cli_version }`. The v3 upgrade reads both legacy shapes: on load a
/// record whose `files` map is empty is reconstructed from the legacy
/// `sha256`/`companions` fields (see `RawPiSkillRecord`), so v1 and v2 records
/// keep working. The bump is deliberate: keeping the number at 2 would let an
/// OLDER binary read a v3 record, silently drop the unknown `files` field on its
/// next rewrite, and — still seeing a schema it accepts — overwrite it, erasing
/// tracking for every mirror. Writing v3 makes that older binary reject the
/// record via `load_pi_provenance_for_write`'s `schema_too_new` guard (fail
/// closed) instead of laundering the field away. The `<=` load check keeps old
/// v1/v2 records readable here.
const PI_PROVENANCE_SCHEMA_VERSION: u32 = 3;

/// Relpath key under which a skill's `SKILL.md` body is tracked in a
/// [`PiSkillRecord`]'s `files` map. It is the filename component of every
/// `pi_default_path` (`~/.pi/agent/skills/<name>/SKILL.md`), so the on-disk
/// sibling a `files` entry names resolves by joining it to the skill's pi dir —
/// identical to how a companion relpath resolves.
const PI_SKILL_FILENAME: &str = "SKILL.md";

/// Whether one tracked pi file is the skill's `SKILL.md` body or a companion
/// sibling. In the flat model the body is no longer an ownership root — it is
/// one `PiFileRecord` like any other — but the kind is still recorded so the
/// prune orders companion deletes before the body (keeping the per-skill dir
/// emptyable) and `doctor` can classify a body-vs-companion drift.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum PiFileKind {
    Skill,
    Companion,
}

/// One mirrored pi file Taskfleet wrote (the `SKILL.md` body OR a companion
/// sibling), tracked independently in its owning skill's `files` map.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PiFileRecord {
    /// Lowercase-hex SHA-256 of the bytes we wrote to
    /// `~/.pi/agent/skills/<name>/<relpath>`. Divergence from the on-disk bytes
    /// means the user (or a newer/older binary) has since changed the file — the
    /// prune/reconcile paths refuse to delete such a copy.
    sha256: String,
    /// Whether this file is the skill body (`SKILL.md`) or a companion sibling.
    kind: PiFileKind,
}

/// Flat per-file provenance for one pi mirror: which files Taskfleet wrote
/// under `~/.pi/agent/skills/<name>/` and their content hash + kind. Replaces
/// the pre-v3 body-owns-companions nesting so every file's
/// ownership/relinquish/retry decision is independent.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(from = "RawPiSkillRecord")]
struct PiSkillRecord {
    /// The `cli_version` frontmatter of the `SKILL.md` bytes we last wrote,
    /// retained for human/debug inspection of the record. `doctor` classifies
    /// drift from the ON-DISK `cli_version` (more accurate than the last-written
    /// one), so it does not read this field today. Empty when only a companion
    /// was ever recorded for the skill (the body write was skipped).
    #[serde(skip_serializing_if = "String::is_empty")]
    cli_version: String,
    /// Every mirrored file, keyed relpath (`SKILL.md` or a companion filename) →
    /// its content hash + kind. The body carries no special status; it is the
    /// `PI_SKILL_FILENAME` entry with `kind: Skill`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    files: BTreeMap<String, PiFileRecord>,
}

/// Deserialize shim that reads BOTH the v3 `files` shape and the legacy v1/v2
/// `sha256`/`companions` fields, so an older on-disk record upgrades in place on
/// load (see [`PI_PROVENANCE_SCHEMA_VERSION`]). `From<RawPiSkillRecord>` folds
/// the legacy fields into the flat `files` map only when `files` is empty (a
/// genuine v1/v2 record), so a v3 record round-trips untouched.
#[derive(Debug, Clone, Default, Deserialize)]
struct RawPiSkillRecord {
    #[serde(default)]
    cli_version: String,
    #[serde(default)]
    files: BTreeMap<String, PiFileRecord>,
    /// Legacy v1/v2 body hash. `Option` so an absent field (v3) is distinguished
    /// from a legacy empty-string body hash.
    #[serde(default)]
    sha256: Option<String>,
    /// Legacy v2 companions map (filename → hash).
    #[serde(default)]
    companions: BTreeMap<String, String>,
}

impl From<RawPiSkillRecord> for PiSkillRecord {
    fn from(raw: RawPiSkillRecord) -> Self {
        // A v3 record already carries `files`; take it verbatim. A legacy v1/v2
        // record has an empty `files` — reconstruct it from the body `sha256`
        // and the `companions` map so tracking survives the upgrade.
        if !raw.files.is_empty() {
            return PiSkillRecord {
                cli_version: raw.cli_version,
                files: raw.files,
            };
        }
        let mut files: BTreeMap<String, PiFileRecord> = BTreeMap::new();
        // Drop an EMPTY legacy body hash rather than minting a fake `SKILL.md`
        // entry with `sha256: ""` — a pre-flat `pi_companion_unrecorded` edge
        // (or a hand-edit) could leave one, and an empty-hash body entry would
        // make `doctor`'s same-version content check spuriously report the real
        // on-disk body as "differs from the copy Taskfleet wrote".
        if let Some(sha256) = raw.sha256.filter(|s| !s.is_empty()) {
            files.insert(
                PI_SKILL_FILENAME.to_string(),
                PiFileRecord {
                    sha256,
                    kind: PiFileKind::Skill,
                },
            );
        }
        for (filename, sha256) in raw.companions {
            // Same empty-hash guard as the body; also never let a legacy
            // companion keyed `SKILL.md` (any case) alias/overwrite the body
            // entry or mis-kind it as a companion (which the prune's
            // stale-companion path could then delete as the actual body).
            if sha256.is_empty() || filename.eq_ignore_ascii_case(PI_SKILL_FILENAME) {
                continue;
            }
            files.insert(
                filename,
                PiFileRecord {
                    sha256,
                    kind: PiFileKind::Companion,
                },
            );
        }
        PiSkillRecord {
            cli_version: raw.cli_version,
            files,
        }
    }
}

impl PiSkillRecord {
    /// The recorded hash of the skill's `SKILL.md` body, if it is tracked. `None`
    /// when only a companion was ever recorded (the body write was skipped).
    fn body_hash(&self) -> Option<&str> {
        self.files
            .get(PI_SKILL_FILENAME)
            .filter(|f| f.kind == PiFileKind::Skill)
            .map(|f| f.sha256.as_str())
    }

    /// The tracked companion relpaths (every `files` entry that is not the body),
    /// sorted (the `BTreeMap` iterates in key order).
    fn companion_names(&self) -> Vec<String> {
        self.files
            .iter()
            .filter(|(_, f)| f.kind == PiFileKind::Companion)
            .map(|(name, _)| name.clone())
            .collect()
    }
}

/// The out-of-band pi provenance record: which pi mirrors Taskfleet wrote
/// and their content hash + version. A `BTreeMap` keeps the on-disk JSON
/// deterministic (sorted keys) so a redeploy that changes nothing produces
/// byte-identical output.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PiProvenance {
    schema_version: u32,
    skills: BTreeMap<String, PiSkillRecord>,
}

/// Lowercase-hex SHA-256 of `bytes`. Used to fingerprint a pi `SKILL.md` for
/// the provenance record and to verify a prune candidate is still our copy.
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Root of the pi.dev skill-mirror layout (`~/.pi/agent/skills`). `None` when
/// `HOME` is unset. Sibling of [`claude_skills_root`]; the pi prune uses it to
/// assert a per-skill dir it is about to `remove_dir` really sits directly under
/// the corpus root before touching it.
fn pi_skills_root() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".pi/agent/skills"))
}

/// Default pi mirror path for a skill (`~/.pi/agent/skills/<name>/SKILL.md`).
/// `None` when `HOME` is unset. Used by `doctor` to locate the on-disk pi copy
/// to compare against the binary, and by the prune path to resolve a
/// de-registered mirror.
pub fn pi_default_path(name: &str) -> Option<PathBuf> {
    default_path("pi", name).ok()
}

/// True when `name` is a single normal path component — no `/`, no `.`/`..`, not
/// absolute, not empty. Every real catalog skill name has this shape. The
/// provenance record is persisted, mutable JSON: a corrupt/hand-edited key like
/// `"../../.bashrc"` or an absolute path deserialized and then `join`ed into a
/// filesystem path would let the prune/doctor act OUTSIDE the pi corpus (the one
/// `record-key → fs-path → remove_file` path that has no other binding check —
/// claude/codex instead enumerate real directory entries, which are
/// single-component for free). Validating here gives the pi path the same rigor
/// as `managed_orphan_dirs` (review finding E). Non-matching names are skipped,
/// never acted on.
///
/// Reused for record-sourced companion FILENAMES too (e.g. `AGENTS-EXECUTION-
/// DAG.md`): the contract is the same "single normal path component", so the
/// name reads skill-specific but the check is exactly what a companion filename
/// needs before it is joined into a per-skill dir.
pub fn is_simple_skill_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(_)), None)
    )
}

/// Path to the single out-of-band pi provenance record
/// (`<taskfleet-root>/state/pi-installed-skills.json`). `None` when the
/// resolved Taskfleet root cannot be established.
/// Deliberately rooted at the Taskfleet state dir, not `~/.pi` — the pi
/// corpus must stay a pure body mirror with no Taskfleet bookkeeping in it.
fn pi_provenance_path() -> Option<PathBuf> {
    home::root_dir()
        .ok()
        .map(|root| root.join("state").join(home::PI_PROVENANCE_FILE_NAME))
}

/// LENIENT read for the READ-ONLY doctor path: a missing, unreadable,
/// unparseable, OR future-schema record yields an empty [`PiProvenance`], so
/// `doctor` simply emits no pi checks rather than auditing a record it cannot
/// trust. NEVER use this on the install mutation path — that must fail loudly
/// instead of laundering a corrupt record into an empty one it then overwrites
/// (see [`load_pi_provenance_for_write`]).
fn read_pi_provenance(path: &Path) -> PiProvenance {
    let Ok(body) = fs::read_to_string(path) else {
        return PiProvenance::default();
    };
    match serde_json::from_str::<PiProvenance>(&body) {
        Ok(p) if p.schema_version <= PI_PROVENANCE_SCHEMA_VERSION => p,
        _ => PiProvenance::default(),
    }
}

/// STRICT load for the install MUTATION path. Distinguishes:
///
///   - absent (`NotFound`) → `Ok(default)` — a first install starts fresh.
///   - unreadable / unparseable / `schema_version` NEWER than this binary
///     understands → `Err`.
///
/// so an install NEVER silently launders a corrupt or future-schema record into
/// an empty one and then overwrites it — which would erase tracking for EVERY
/// other managed pi mirror (the record is the sole authority; there is no in-dir
/// fallback). The record is loaded and validated BEFORE any file is written, so
/// a corrupt record fails the install fast rather than after mutating the tree
/// (review finding A). The trusted state root does not rescue this: a partial
/// write (power loss / ENOSPC mid-persist), a version rollback, or a manual edit
/// are all ordinary causes.
fn load_pi_provenance_for_write(path: &Path) -> Result<PiProvenance, CliError> {
    let body = match fs::read_to_string(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(PiProvenance::default()),
        Err(e) => {
            return Err(CliError::system(
                "pi_provenance_unreadable",
                format!(
                    "could not read pi provenance record {}: {e}; back it up and remove it to re-initialise",
                    path.display()
                ),
            ))
        }
    };
    let prov: PiProvenance = serde_json::from_str(&body).map_err(|e| {
        CliError::system(
            "pi_provenance_corrupt",
            format!(
                "pi provenance record {} is not valid JSON ({e}); refusing to overwrite it (that would erase tracking for every managed pi mirror). Back it up and remove it to re-initialise.",
                path.display()
            ),
        )
    })?;
    if prov.schema_version > PI_PROVENANCE_SCHEMA_VERSION {
        return Err(CliError::system(
            "pi_provenance_schema_too_new",
            format!(
                "pi provenance record {} uses schema {} but this binary understands only {}; refusing to overwrite it. Upgrade taskfleet.",
                path.display(),
                prov.schema_version,
                PI_PROVENANCE_SCHEMA_VERSION
            ),
        ));
    }
    Ok(prov)
}

/// Write the pi provenance record atomically. The `state/` dir is created if
/// absent. `write_atomic` with `force = true` renames a fresh tempfile over the
/// destination, which atomically replaces whatever name is there — including a
/// squatting symlink — with the regular file, so no symlink target is ever
/// clobbered. A write failure is fatal to the install (symmetric with the
/// claude/codex marker writes): without a current record a genuine pi orphan is
/// never recognised.
fn write_pi_provenance(path: &Path, prov: &PiProvenance) -> Result<(), CliError> {
    let body = serde_json::to_string_pretty(prov).map_err(|e| {
        CliError::system(
            "pi_provenance_serialize_failed",
            format!("could not serialize pi provenance record: {e}"),
        )
    })?;
    write_atomic(path, &body, true)
}

/// The skill names the pi provenance record lists as Taskfleet-managed,
/// each with its recorded content hash (sorted by name). Empty
/// when `HOME`/root is unset or the record is absent — precisely the signal
/// that Taskfleet does not manage a pi mirror on this host, so `doctor`
/// emits no pi checks. Consumed by the `skill.sync.<name>.pi` /
/// `skill.orphan.<name>.pi` doctor checks.
pub fn pi_managed_skills() -> Vec<PiManagedSkill> {
    let Some(path) = pi_provenance_path() else {
        return Vec::new();
    };
    let prov = read_pi_provenance(&path);
    let mut out: Vec<PiManagedSkill> = prov
        .skills
        .into_iter()
        .map(|(name, rec)| {
            // The doctor's same-version-edit check compares the on-disk
            // `SKILL.md` hash against the recorded body hash. `None` when only a
            // companion was ever recorded (the body write was skipped) — doctor
            // must then NOT fabricate a body hash and claim the real on-disk body
            // "differs from the copy Taskfleet wrote". Companions are
            // surfaced regardless.
            PiManagedSkill {
                name,
                sha256: rec.body_hash().map(str::to_string),
                companions: rec.companion_names(),
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// One managed pi mirror surfaced to `doctor`: the skill name plus the content
/// hash Taskfleet recorded when it last wrote the mirror. The hash lets the
/// drift check detect a same-version local edit (on-disk bytes no longer match
/// what we wrote) without holding the bundled body. (The record also carries the
/// written `cli_version`, but the doctor reads the on-disk `cli_version` for a
/// more accurate drift classification, so it is not surfaced here.)
pub struct PiManagedSkill {
    pub name: String,
    /// The recorded `SKILL.md` body hash, or `None` when the record tracks only
    /// companions (the body write was skipped). `doctor` skips the same-version
    /// content-edit sub-check when this is `None` rather than comparing the real
    /// on-disk body against a fabricated empty hash.
    pub sha256: Option<String>,
    /// Companion filenames the provenance record lists as mirrored beside this
    /// skill's pi `SKILL.md` (sorted). Used by `doctor` to detect a companion
    /// the binary dropped but the record still tracks (`skill.orphan.<name>.pi.
    /// <file>`); the forward drift check compares on-disk companions to the
    /// bundled bodies directly, so it does not need this list.
    pub companions: Vec<String>,
}

/// SHA-256 (lowercase hex) of the file at `path`, or `None` if it cannot be
/// read. Exposed for `doctor` to compare an on-disk pi `SKILL.md` against the
/// hash the provenance record holds.
pub fn file_sha256(path: &Path) -> Option<String> {
    fs::read(path).ok().map(|b| sha256_hex(&b))
}

/// True when `dir` is a claude-layout skill directory that Taskfleet
/// installed. The guard is deliberately strict — its `true` is the SOLE
/// authorization for a recursive `remove_dir_all`, so it must never yield
/// a false positive on a user's own directory. Three conditions must ALL
/// hold:
///
/// 1. The marker is a **regular file** (`symlink_metadata`, which does not
///    follow links) — a planted `.Taskfleet-managed` *symlink* cannot
///    make a directory look managed.
/// 2. The marker carries the `managed-by: taskfleet` stamp.
/// 3. The marker's recorded `skill_name` equals this directory's name.
///    This binding is what makes `cp -r managed-skill my-copy` safe: the
///    copy's marker still names the ORIGINAL skill, so it never matches
///    `my-copy` and the copy is spared.
fn is_managed_skill_dir(dir: &Path) -> bool {
    let Some(dir_name) = dir.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let marker = dir.join(MANAGED_MARKER_FILENAME);
    let Ok(meta) = fs::symlink_metadata(&marker) else {
        return false;
    };
    if !meta.file_type().is_file() {
        return false;
    }
    let Ok(content) = fs::read_to_string(&marker) else {
        return false;
    };
    let mut has_stamp = false;
    let mut name_matches = false;
    let mut recorded_hash = None;
    for line in content.lines() {
        let line = line.trim();
        if line == "managed-by: taskfleet" {
            has_stamp = true;
        } else if let Some(rest) = line.strip_prefix("skill_name:") {
            name_matches = rest.trim() == dir_name;
        } else if let Some(rest) = line.strip_prefix("sha256:") {
            recorded_hash = Some(rest.trim());
        }
    }
    has_stamp
        && name_matches
        && recorded_hash
            .is_some_and(|hash| file_sha256(&dir.join(PI_SKILL_FILENAME)).as_deref() == Some(hash))
}

/// Prune only bytes whose canonical marker hash proves Taskfleet wrote them.
/// Unknown siblings and edited bodies are never recursively deleted.
fn dir_contains_only(dir: &Path, allowed: &[&str]) -> bool {
    fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().all(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| allowed.contains(&name))
        })
    })
}

fn prune_managed_claude_dir(dir: &Path) -> std::io::Result<bool> {
    if !is_managed_skill_dir(dir)
        || !dir_contains_only(dir, &[PI_SKILL_FILENAME, MANAGED_MARKER_FILENAME])
    {
        return Ok(false);
    }
    fs::remove_file(dir.join(PI_SKILL_FILENAME))?;
    fs::remove_file(dir.join(MANAGED_MARKER_FILENAME))?;
    fs::remove_dir(dir)?;
    Ok(true)
}

/// Write (or overwrite) the provenance marker for skill `skill_name` at
/// `path`. The recorded `skill_name` is what `is_managed_skill_dir` binds
/// against, so a copied-and-renamed skill is never mistaken for an orphan.
/// `companions` are the companion filenames this install manages in the
/// directory, each recorded on its own `companion:` line so a later binary
/// that drops one can recognise the leftover file as an orphan it once
/// installed (see `orphan_companions` and the `cmd_install` prune loop).
/// The parent directory already exists (the SKILL.md write created it), so
/// this is a plain overwrite; the marker is always ours. If a symlink is
/// squatting at the marker path we unlink it first so `fs::write` cannot
/// clobber the link's target (an arbitrary-file overwrite within the
/// user's permissions).
fn write_marker(path: &Path, skill_name: &str, companions: &[String]) -> Result<(), CliError> {
    clear_marker_symlink(path)?;
    let skill_hash = lookup(skill_name)
        .map(|skill| sha256_hex(skill.body.as_bytes()))
        .unwrap_or_default();
    let mut body = format!(
        "managed-by: taskfleet\ncli_version: {CLI_VERSION}\nskill_name: {skill_name}\nsha256: {skill_hash}\n"
    );
    for companion in companions {
        body.push_str("companion: ");
        body.push_str(companion);
        body.push('\n');
    }
    fs::write(path, body).map_err(|e| {
        CliError::system(
            "marker_write_failed",
            format!(
                "could not write provenance marker {}: {}",
                path.display(),
                e
            ),
        )
    })
}

/// Write (or overwrite) the SINGLE codex provenance marker at `path`
/// (`~/.codex/prompts/_shared/.Taskfleet-managed`). Unlike the claude
/// per-skill marker, this one records the flat layout's whole managed set:
/// a `prompt:` line per installed codex skill and a `companion:` line per
/// installed `_shared/<file>`. That is the only signal that makes codex
/// pruning safe (a user's own prompt is never listed). A squatting symlink
/// is unlinked first so `fs::write` cannot clobber the link's target.
fn write_codex_marker(
    path: &Path,
    prompts: &[String],
    companions: &[String],
) -> Result<(), CliError> {
    clear_marker_symlink(path)?;
    let mut body = format!("managed-by: taskfleet\ncli_version: {CLI_VERSION}\n");
    for prompt in prompts {
        body.push_str("prompt: ");
        body.push_str(prompt);
        body.push('\n');
    }
    for companion in companions {
        body.push_str("companion: ");
        body.push_str(companion);
        body.push('\n');
    }
    fs::write(path, body).map_err(|e| {
        CliError::system(
            "marker_write_failed",
            format!(
                "could not write codex provenance marker {}: {}",
                path.display(),
                e
            ),
        )
    })
}

/// If a symlink squats at the marker `path`, unlink it so a subsequent
/// `fs::write` cannot clobber the link's target (an arbitrary-file
/// overwrite within the user's permissions). A regular file is left for the
/// write to overwrite in place — the marker is always ours.
fn clear_marker_symlink(path: &Path) -> Result<(), CliError> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            fs::remove_file(path).map_err(|e| {
                CliError::system(
                    "marker_write_failed",
                    format!(
                        "could not clear stale marker symlink {}: {}",
                        path.display(),
                        e
                    ),
                )
            })?;
        }
    }
    Ok(())
}

/// Scan `skills_root` for managed skill directories whose name is NOT in
/// `registered`. These are directories Taskfleet installed (they
/// carry a valid provenance marker naming that same directory) but the
/// running binary no longer ships — renamed or removed bundled skills,
/// safe to prune. Directories without a valid marker (a user's own skills)
/// are never returned. Result is sorted by name for deterministic output.
/// An unreadable root, or an unreadable entry, is skipped rather than
/// guessed at — the prune path must always err toward NOT deleting.
fn managed_orphan_dirs(skills_root: &Path, registered: &HashSet<&str>) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(skills_root) else {
        return Vec::new();
    };
    let mut orphans: Vec<(String, PathBuf)> = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        // `file_type()` is an lstat: it never follows a symlink. A
        // symlinked entry — even one pointing at a real directory — is
        // rejected so `remove_dir_all` can never traverse a link out of
        // the skills root and delete an unrelated tree.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Never prune a directory that matches a registered skill —
        // exactly, OR case-insensitively (a differently-cased alias on a
        // case-insensitive filesystem, e.g. macOS APFS, resolves to the
        // same on-disk directory a fresh install just wrote).
        if registered
            .iter()
            .any(|r| *r == dir_name || r.eq_ignore_ascii_case(dir_name))
        {
            continue;
        }
        if is_managed_skill_dir(&path) {
            orphans.push((dir_name.to_string(), path.clone()));
        }
    }
    orphans.sort();
    orphans
}

/// What [`prune_codex_file`] did with an orphan file, so the caller can
/// keep the marker's recorded set in step: `Removed` (deleted → drop it +
/// report it pruned), `Dropped` (nothing safe on disk to delete → stop
/// tracking it), or `Kept` (delete failed → still on disk → keep tracking).
enum CodexPruneOutcome {
    Removed,
    Dropped,
    Kept,
}

/// Delete one orphaned codex file (a de-registered prompt or companion),
/// mirroring the claude orphan-companion prune's safety: only ever remove a
/// REGULAR file we manage — never follow a symlink or recurse into a
/// directory squatting at that name. An absent/symlink/dir target yields
/// `Dropped` (nothing to clean, so stop tracking it); a successful unlink
/// yields `Removed`; a failed unlink warns and yields `Kept` so the marker
/// keeps tracking the still-present file.
fn prune_codex_file(
    path: &Path,
    removed_warning: &str,
    failed_warning: &str,
    warnings: &mut Vec<String>,
) -> CodexPruneOutcome {
    let is_regular = fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file());
    if !is_regular {
        return CodexPruneOutcome::Dropped;
    }
    match fs::remove_file(path) {
        Ok(()) => {
            warnings.push(format!("{removed_warning} at {}", path.display()));
            CodexPruneOutcome::Removed
        }
        Err(e) => {
            warnings.push(format!("{failed_warning} at {}: {e}", path.display()));
            CodexPruneOutcome::Kept
        }
    }
}

/// One pi file the install's write loop actually persisted this run, tagged so
/// the provenance update files it correctly under its owning skill's flat
/// `files` map: a `SKILL.md` body (`kind: Skill`, also refreshing the skill's
/// `cli_version`) or a companion (`kind: Companion`). Only persisted files
/// appear here — a `skipped` (present, non-force) pi file is absent, so its
/// prior `files` entry is carried forward untouched.
enum PiWrite {
    Skill {
        name: &'static str,
        hash: String,
        cli_version: String,
    },
    Companion {
        owner: &'static str,
        filename: &'static str,
        hash: String,
    },
}

/// Summary of a de-registered skill's flat-file prune, so the caller can update
/// the record + `pruned` payload. Every file the prune handled (deleted /
/// relinquished / absent) is removed from the record's `files` map in place; a
/// file whose delete FAILED is left in `files` so the entry survives for a
/// retry. `body_removed` is true when the skill's `SKILL.md` was our unmodified
/// copy and was deleted — the signal to report the skill in the `pruned` list.
#[derive(Default)]
struct PiPruneSummary {
    body_removed: bool,
}

/// Prune the pi mirror for a de-registered skill `name`, keyed SOLELY on the
/// out-of-band provenance record (the pi dir carries no in-dir marker). Flat
/// per-file model: each tracked file in `rec.files` is handled independently —
/// its own ownership/relinquish/retry decision, no privileged body — so a
/// diverged `SKILL.md` no longer forces the companions to be left behind (issue
/// `pi-provenance-flat-file-model`). Safety is layered exactly like the
/// claude/codex orphan prunes:
///
///   - Resolve `~/.pi/agent/skills/<name>/`; a `HOME`-unset root clears the
///     record's `files` (nothing we can locate) so the caller drops the entry.
///   - Each file: `symlink_metadata` (never follows a link) → only a REGULAR
///     file whose bytes hash to the recorded value is deleted; a symlink, a
///     squatting dir, or a user-edited (diverged) copy is left untouched and
///     relinquished (dropped from tracking); a failed delete keeps the file
///     tracked for a retry.
///   - Companions are handled BEFORE the `SKILL.md` so the per-skill dir can
///     empty out; the body no longer defers on a companion failure (that
///     coupling is gone — a Kept companion simply stays tracked). The body is
///     cleaned even when a companion delete failed (never stranded, since it
///     stays in the record). Finally the now-possibly-empty per-skill dir is
///     best-effort removed (never a recursive `remove_dir_all` — a user sibling,
///     or a surviving diverged/Kept file, keeps the dir).
fn prune_pi_mirror(
    name: &str,
    rec: &mut PiSkillRecord,
    warnings: &mut Vec<String>,
) -> PiPruneSummary {
    let Some(dir) = pi_default_path(name).and_then(|p| p.parent().map(Path::to_path_buf)) else {
        // Cannot locate the mirror (HOME unset — usually transient, e.g. a
        // misconfigured cron/sudo invocation). We cannot prove the files are
        // gone or diverged, so we must NOT delete tracking: leave `rec.files`
        // intact so a later run with HOME set can still prune/audit them. The
        // caller keeps the entry (files non-empty) and reports nothing pruned.
        return PiPruneSummary::default();
    };
    prune_pi_mirror_at(name, &dir, pi_skills_root().as_deref(), rec, warnings)
}

/// Path-taking core of [`prune_pi_mirror`], split out so the safety logic is
/// unit-testable against a tempdir without touching `$HOME`. `dir` is the pi
/// per-skill directory; `skills_root` is the expected pi corpus root, and the
/// empty-dir cleanup only fires when `dir` is exactly `<skills_root>/<name>/`.
fn prune_pi_mirror_at(
    name: &str,
    dir: &Path,
    skills_root: Option<&Path>,
    rec: &mut PiSkillRecord,
    warnings: &mut Vec<String>,
) -> PiPruneSummary {
    // Companions FIRST (before the body) so the per-skill dir can empty out.
    // Collect relpaths up front so we can mutate `rec.files` inside the loop.
    for filename in rec.companion_names() {
        if !is_simple_skill_name(&filename) {
            warnings.push(format!(
                "pi_provenance_bad_name: ignoring pi companion entry '{filename}' for skill '{name}' (not a simple filename)"
            ));
            rec.files.remove(&filename);
            continue;
        }
        let recorded_hash = rec.files[&filename].sha256.clone();
        match prune_pi_companion(name, &dir.join(&filename), &recorded_hash, warnings) {
            // Delete failed while the file is still present: keep it tracked so a
            // later redeploy retries and `doctor` keeps flagging it.
            PiCompanionOutcome::Kept => {}
            // Deleted, absent, relinquished (symlink/dir/diverged): stop tracking.
            _ => {
                rec.files.remove(&filename);
            }
        }
    }

    // Then the body (`SKILL.md`), if it is still tracked.
    let mut body_removed = false;
    if let Some(recorded_hash) = rec.body_hash().map(str::to_string) {
        let body_path = dir.join(PI_SKILL_FILENAME);
        match prune_pi_body(name, &body_path, &recorded_hash, warnings) {
            PiCompanionOutcome::Removed => {
                body_removed = true;
                rec.files.remove(PI_SKILL_FILENAME);
            }
            PiCompanionOutcome::Kept => {}
            // Absent / symlink / dir / diverged: relinquish tracking.
            _ => {
                rec.files.remove(PI_SKILL_FILENAME);
            }
        }
    }

    // Best-effort clean the now-possibly-empty per-skill dir (a user sibling or a
    // surviving diverged/Kept file keeps it via the non-recursive `remove_dir`).
    remove_empty_pi_skill_dir(Some(dir), skills_root, name);

    PiPruneSummary { body_removed }
}

/// Prune ONE de-registered pi `SKILL.md` body, mirroring [`prune_pi_companion`]'s
/// safety (regular-file + hash-match before delete) but with body-specific
/// warning strings. Returns the shared [`PiCompanionOutcome`] so the caller
/// classifies it identically to a companion: `Removed` (our copy, deleted),
/// `Absent` (nothing on disk), `NonRegular` (symlink/dir left), `Diverged`
/// (user-edited, left), `Kept` (delete/read failed while present → retry).
fn prune_pi_body(
    name: &str,
    path: &Path,
    recorded_hash: &str,
    warnings: &mut Vec<String>,
) -> PiCompanionOutcome {
    match fs::symlink_metadata(path) {
        // Only a genuine NotFound relinquishes tracking. A transient metadata
        // error (PermissionDenied, I/O) must NOT be mistaken for absence — that
        // would permanently drop tracking of a file that may still exist — so it
        // defers the whole prune (`Kept`) for a retry.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return PiCompanionOutcome::Absent,
        Err(e) => {
            warnings.push(format!(
                "pi_mirror_prune_failed: could not inspect de-registered pi mirror '{name}' at {}: {e}",
                path.display()
            ));
            return PiCompanionOutcome::Kept;
        }
        Ok(m) if !m.file_type().is_file() => {
            // A symlink or squatting dir at the body path: never followed/deleted.
            // Narrate it (mirrors the companion path) so a leftover is visible.
            warnings.push(format!(
                "pi_mirror_left: de-registered pi mirror '{name}' at {} is not a regular file (symlink or directory); leaving it in place",
                path.display()
            ));
            return PiCompanionOutcome::NonRegular;
        }
        Ok(_) => {}
    }
    match fs::read(path) {
        Ok(bytes) if sha256_hex(&bytes) == recorded_hash => match fs::remove_file(path) {
            Ok(()) => {
                warnings.push(format!(
                    "pi_mirror_pruned: removed de-registered pi mirror '{name}' at {}",
                    path.display()
                ));
                PiCompanionOutcome::Removed
            }
            Err(e) => {
                warnings.push(format!(
                    "pi_mirror_prune_failed: could not remove de-registered pi mirror '{name}' at {}: {e}",
                    path.display()
                ));
                PiCompanionOutcome::Kept
            }
        },
        Ok(_) => {
            warnings.push(format!(
                "pi_mirror_diverged: de-registered pi mirror '{name}' at {} was modified since Taskfleet wrote it; leaving it in place and no longer tracking it",
                path.display()
            ));
            PiCompanionOutcome::Diverged
        }
        Err(e) => {
            warnings.push(format!(
                "pi_mirror_prune_failed: could not read de-registered pi mirror '{name}' at {}: {e}",
                path.display()
            ));
            PiCompanionOutcome::Kept
        }
    }
}

/// Best-effort removal of a now-orphaned per-skill pi dir if it is empty. We only
/// ever wrote `SKILL.md` + our companions into it, but a user may have added
/// their own sibling (or a diverged companion may remain) — `remove_dir`
/// (non-recursive) fails on a non-empty dir, so anything we did not clean is
/// preserved and the dir left be. Guarded on `parent` sitting DIRECTLY under the
/// pi skills root (`<root>/<name>/`), so even an unexpected `pi_default_path`
/// result can never point `remove_dir` at an arbitrary directory — the pi
/// analogue of claude's `is_managed_skill_dir` name-binding (review finding D).
fn remove_empty_pi_skill_dir(parent: Option<&Path>, skills_root: Option<&Path>, name: &str) {
    if let (Some(parent), Some(root)) = (parent, skills_root) {
        if parent.parent() == Some(root)
            && parent.file_name().and_then(|n| n.to_str()) == Some(name)
        {
            let _ = fs::remove_dir(parent);
        }
    }
}

/// What [`prune_pi_companion`] / [`prune_pi_body`] did with ONE tracked file, so
/// the flat prune can update the record per file: `Removed` (our unmodified
/// copy, deleted → drop from tracking), `Absent` (positively `NotFound` → drop),
/// `NonRegular` (a symlink / squatting dir left in place → drop), `Diverged` (a
/// user-edited copy left in place → drop), or `Kept` (a metadata/read/delete
/// error while the file may still be present → KEEP tracking so a later redeploy
/// retries and `doctor` keeps flagging it). In the flat model a `Kept` companion
/// no longer defers the body delete — it simply stays tracked (never stranded,
/// since it remains in the record).
#[derive(PartialEq, Eq)]
enum PiCompanionOutcome {
    Removed,
    Absent,
    NonRegular,
    Diverged,
    Kept,
}

/// Best-effort removal of ONE recorded pi companion sibling during a
/// de-registered skill's prune. Mirrors the SKILL.md safety: only a REGULAR
/// file whose bytes hash to `recorded_hash` is deleted — a symlink, a squatting
/// dir, an unreadable file, or a user-edited copy is left untouched (and thus
/// keeps the parent dir non-empty so `remove_dir` spares it). Warnings narrate
/// every case where a file is LEFT behind (a squatting non-regular path, a
/// diverged copy, or a failed delete) so a leftover is visible; a plain-absent
/// companion is silent (nothing to narrate). Returns the outcome so the caller
/// can defer the body delete on `Kept`.
fn prune_pi_companion(
    skill: &str,
    path: &Path,
    recorded_hash: &str,
    warnings: &mut Vec<String>,
) -> PiCompanionOutcome {
    match fs::symlink_metadata(path) {
        // NotFound relinquishes tracking; a transient metadata error keeps the
        // companion tracked for a retry rather than mistaking it for absence.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return PiCompanionOutcome::Absent,
        Err(e) => {
            warnings.push(format!(
                "pi_companion_prune_failed: could not inspect companion of de-registered pi skill '{skill}' at {}: {e}",
                path.display()
            ));
            return PiCompanionOutcome::Kept;
        }
        Ok(m) if !m.file_type().is_file() => {
            warnings.push(format!(
                "pi_companion_left: companion of de-registered pi skill '{skill}' at {} is not a regular file (symlink or directory); leaving it in place",
                path.display()
            ));
            return PiCompanionOutcome::NonRegular;
        }
        Ok(_) => {}
    }
    match fs::read(path) {
        Ok(bytes) if sha256_hex(&bytes) == recorded_hash => match fs::remove_file(path) {
            Ok(()) => {
                warnings.push(format!(
                    "pi_companion_pruned: removed companion of de-registered pi skill '{skill}' at {}",
                    path.display()
                ));
                PiCompanionOutcome::Removed
            }
            Err(e) => {
                warnings.push(format!(
                    "pi_companion_prune_failed: could not remove companion of de-registered pi skill '{skill}' at {}: {e}",
                    path.display()
                ));
                PiCompanionOutcome::Kept
            }
        },
        Ok(_) => {
            warnings.push(format!(
                "pi_companion_diverged: companion of de-registered pi skill '{skill}' at {} was modified since Taskfleet wrote it; leaving it in place",
                path.display()
            ));
            PiCompanionOutcome::Diverged
        }
        Err(e) => {
            warnings.push(format!(
                "pi_companion_prune_failed: could not read companion of de-registered pi skill '{skill}' at {}: {e}",
                path.display()
            ));
            PiCompanionOutcome::Kept
        }
    }
}

/// Reconcile ONE still-registered skill's recorded pi companions against what
/// this binary now bundles: a companion a prior binary mirrored that the current
/// one no longer ships is an orphan. Without this, the `skill.orphan.<name>.pi.
/// <file>` doctor check would flag a state its only suggested fix (`skill install
/// <name> --force`) could never clear — a permanent, unfixable warning loop
/// (review finding F1). Symmetric with the claude `mark_dirs` companion
/// reconciliation, but keyed on the out-of-band record (no in-dir marker):
///
///   - non-`--force`: the stale entry is LEFT in the record so `doctor` keeps
///     surfacing it (and its `--force` fix now genuinely clears it).
///   - `--force`: remove the on-disk file only when it is our unmodified copy
///     (regular + hash match); report it in `pruned_companions` and drop it from
///     the record. A diverged / non-regular / absent file is left on disk but
///     dropped from tracking (we relinquish a copy we no longer recognise). A
///     failed delete keeps the entry tracked so a later redeploy retries.
fn reconcile_pi_companions(
    skill_name: &str,
    prov: &mut PiProvenance,
    force: bool,
    pruned_companions: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let Some(rec) = prov.skills.get_mut(skill_name) else {
        return;
    };
    let Some(dir) = pi_default_path(skill_name).and_then(|p| p.parent().map(Path::to_path_buf))
    else {
        return;
    };
    reconcile_pi_companions_at(skill_name, rec, &dir, force, pruned_companions, warnings);
}

/// Path-taking core of [`reconcile_pi_companions`], split out so the logic is
/// unit-testable against a tempdir without touching `$HOME`. `dir` is the pi
/// per-skill directory the companions live in.
fn reconcile_pi_companions_at(
    skill_name: &str,
    rec: &mut PiSkillRecord,
    dir: &Path,
    force: bool,
    pruned_companions: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let bundled: HashSet<&str> = resources_for(skill_name)
        .iter()
        .map(|r| r.filename)
        .collect();
    // Stale = a tracked COMPANION file the binary no longer bundles. The body
    // (`kind: Skill`) is never stale here — it is reconciled by the write loop
    // (a still-registered skill always re-writes its `SKILL.md`).
    let stale: Vec<String> = rec
        .files
        .iter()
        .filter(|(f, r)| r.kind == PiFileKind::Companion && !bundled.contains(f.as_str()))
        .map(|(f, _)| f.clone())
        .collect();
    if stale.is_empty() {
        return;
    }
    // Non-force: keep every stale entry so `doctor` keeps flagging it; the
    // `--force` fix it suggests is what actually clears it.
    if !force {
        return;
    }
    for filename in stale {
        if !is_simple_skill_name(&filename) {
            warnings.push(format!(
                "pi_provenance_bad_name: ignoring pi companion entry '{filename}' for skill '{skill_name}' (not a simple filename)"
            ));
            rec.files.remove(&filename);
            continue;
        }
        let recorded_hash = rec.files[&filename].sha256.clone();
        let path = dir.join(&filename);
        // Classify the on-disk file. A metadata/read ERROR (PermissionDenied,
        // I/O) is NOT the same as absence or divergence — relinquishing on a
        // transient error would permanently drop tracking of a file that may
        // still be ours. So a hard error KEEPS the entry tracked for a retry;
        // only a positively-determined absent / non-regular / diverged file is
        // relinquished, and only a verified-our-copy is deleted.
        enum Classified {
            OurCopy,
            NotOurs,
            Error(std::io::Error),
        }
        let classified = match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Classified::NotOurs,
            Err(e) => Classified::Error(e),
            Ok(m) if !m.file_type().is_file() => Classified::NotOurs,
            Ok(_) => match fs::read(&path) {
                Ok(b) if sha256_hex(&b) == recorded_hash => Classified::OurCopy,
                Ok(_) => Classified::NotOurs,
                Err(e) => Classified::Error(e),
            },
        };
        if let Classified::Error(e) = classified {
            warnings.push(format!(
                "skill_companion_prune_failed: could not inspect orphan pi companion '{filename}' for skill '{skill_name}' at {}: {e}",
                path.display()
            ));
            continue; // keep the entry tracked so a later redeploy retries
        }
        let is_our_copy = matches!(classified, Classified::OurCopy);
        if is_our_copy {
            match fs::remove_file(&path) {
                Ok(()) => {
                    warnings.push(format!(
                        "skill_companion_pruned: removed orphan pi companion '{filename}' for skill '{skill_name}' at {}",
                        path.display()
                    ));
                    pruned_companions.push(format!("{skill_name}/{filename}"));
                    rec.files.remove(&filename);
                }
                Err(e) => {
                    // Still on disk — keep tracking so a later redeploy retries and
                    // `doctor` keeps flagging it.
                    warnings.push(format!(
                        "skill_companion_prune_failed: could not remove orphan pi companion '{filename}' for skill '{skill_name}' at {}: {e}",
                        path.display()
                    ));
                }
            }
        } else {
            // Absent / non-regular / diverged: not our unmodified copy. Leave any
            // file in place and relinquish tracking rather than delete something we
            // no longer recognise as ours.
            warnings.push(format!(
                "pi_companion_relinquished: orphan pi companion '{filename}' for skill '{skill_name}' at {} is not our unmodified copy; no longer tracking it",
                path.display()
            ));
            rec.files.remove(&filename);
        }
    }
}

/// Managed-but-de-registered skill directories under the claude install
/// root, as `(name, path)` pairs sorted by name. Consumed by the
/// `skill.orphan.*` doctor check. Empty when `HOME` is unset or the root
/// is unreadable.
pub fn managed_orphans() -> Vec<(String, PathBuf)> {
    let Some(root) = claude_skills_root() else {
        return Vec::new();
    };
    let registered: HashSet<&str> = SKILLS.iter().map(|s| s.name).collect();
    managed_orphan_dirs(&root, &registered)
}

/// The companion filenames a provenance marker records as managed (the
/// `companion:` lines). Empty when the marker is unreadable or records
/// none. Blank values are dropped. Sibling of the `skill_name:`/stamp
/// parsing in `is_managed_skill_dir`, but for the companion sub-records.
fn read_managed_companions(marker_path: &Path) -> Vec<String> {
    read_marker_records(marker_path, "companion")
}

/// The trimmed values of every `<key>:` line in a marker file (blanks
/// dropped). Shared by the claude marker's `companion:` reader and the
/// codex marker's `prompt:` / `companion:` readers, so all three parse the
/// same line shape identically. Unreadable / absent marker → empty.
fn read_marker_records(marker_path: &Path, key: &str) -> Vec<String> {
    if !fs::symlink_metadata(marker_path).is_ok_and(|m| m.file_type().is_file()) {
        return Vec::new();
    }
    let Ok(content) = fs::read_to_string(marker_path) else {
        return Vec::new();
    };
    let owned = content
        .lines()
        .any(|line| matches!(line.trim(), "managed-by: taskfleet"));
    if !owned {
        return Vec::new();
    }
    let prefix = format!("{key}:");
    content
        .lines()
        .filter_map(|line| line.trim().strip_prefix(prefix.as_str()))
        .map(|rest| rest.trim().to_string())
        .filter(|value| is_simple_skill_name(value))
        .collect()
}

/// Companion files recorded as managed in `<skill_dir>`'s provenance marker
/// that the current binary no longer bundles AND that still exist on disk —
/// the orphan companions a prior binary installed and this one dropped.
/// Filenames only (sorted, deduped); the caller joins `skill_dir` to report
/// or remove them. Consumed by the `skill.orphan.<name>.<file>` doctor
/// check. A still-bundled companion is never returned (it is audited by the
/// `skill.sync.<name>.<file>` forward check instead), and a user's own file
/// that the marker never recorded is never returned (that is what keeps this
/// from false-positiving on a hand-dropped note). Presence is probed with
/// `symlink_metadata` so a planted symlink is not followed.
pub fn orphan_companions(skill_name: &str, skill_dir: &Path) -> Vec<String> {
    let bundled: HashSet<&str> = resources_for(skill_name)
        .iter()
        .map(|r| r.filename)
        .collect();
    let marker_path = skill_dir.join(MANAGED_MARKER_FILENAME);
    let mut orphans: Vec<String> = read_managed_companions(&marker_path)
        .into_iter()
        .filter(|name| !bundled.contains(name.as_str()))
        .filter(|name| fs::symlink_metadata(skill_dir.join(name)).is_ok())
        .collect();
    orphans.sort();
    orphans.dedup();
    orphans
}

/// Empty parent from `PathBuf::parent()` means a bare relative filename
/// (e.g. `--dest SKILL.md`). Treat that as the current directory rather
/// than failing `create_dir_all("")`.
fn normalized_parent(path: &Path) -> Option<&Path> {
    match path.parent() {
        Some(p) if p.as_os_str().is_empty() => Some(Path::new(".")),
        Some(p) => Some(p),
        None => None,
    }
}

fn write_atomic(path: &Path, content: &str, force: bool) -> Result<(), CliError> {
    let parent = normalized_parent(path).ok_or_else(|| {
        CliError::user(
            "invalid_dest",
            format!("destination has no parent directory: {}", path.display()),
        )
    })?;
    fs::create_dir_all(parent).map_err(|e| {
        CliError::system(
            "create_dir_failed",
            format!("could not create {}: {}", parent.display(), e),
        )
    })?;

    let mut tmp = NamedTempFile::new_in(parent).map_err(|e| {
        CliError::system(
            "tempfile_failed",
            format!("could not create tempfile in {}: {}", parent.display(), e),
        )
    })?;
    tmp.write_all(content.as_bytes())
        .map_err(|e| CliError::system("write_failed", format!("could not write tempfile: {e}")))?;
    tmp.as_file_mut()
        .sync_all()
        .map_err(|e| CliError::system("fsync_failed", format!("could not fsync tempfile: {e}")))?;

    // `persist_noclobber` makes the non-force case TOCTOU-safe: the rename
    // refuses to clobber via the kernel rather than via an earlier
    // `path.exists()` check. The preflight pass above still runs so we
    // surface the friendly `refused_overwrite` envelope early; this is
    // the belt-and-braces guard against a race between preflight and
    // persist.
    let persist_result = if force {
        tmp.persist(path).map(|_| ())
    } else {
        tmp.persist_noclobber(path).map(|_| ())
    };
    persist_result.map_err(|e| {
        // tempfile's PersistError wraps EEXIST; surface the canonical
        // refused_overwrite envelope so callers can branch on the code.
        let kind = e.error.kind();
        if !force && kind == std::io::ErrorKind::AlreadyExists {
            CliError::system(
                "refused_overwrite",
                format!(
                    "{} already exists; pass --force to overwrite",
                    path.display()
                ),
            )
            .with_invalid_value(path.display().to_string())
        } else {
            CliError::system(
                "rename_failed",
                format!("could not rename into place {}: {}", path.display(), e),
            )
        }
    })
}

/// Extract the `description:` field from YAML-ish frontmatter at the top
/// of a SKILL.md.
///
/// Frontmatter is everything between a leading `---` line and the next
/// `---` line. We parse line-by-line (handling both `\n` and `\r\n` line
/// endings via `str::lines`) and accept `key: value` / `key : value`
/// pairs. Quoted values have their surrounding `"` or `'` stripped. This
/// is not a full YAML parser — multi-line scalars and nested maps are
/// out of scope — but it covers every shape our SKILL.md frontmatter is
/// allowed to take.
fn parse_description(body: &str) -> Option<String> {
    parse_frontmatter_field(body, "description")
}

/// Generic line-oriented frontmatter field extractor. Same constraints
/// as `parse_description`: top-level `---` fence, `key: value` shape,
/// single-line scalars only. Strips surrounding `"` / `'` quotes from
/// the value.
fn parse_frontmatter_field(body: &str, field: &str) -> Option<String> {
    let body = body.strip_prefix('\u{feff}').unwrap_or(body);
    let mut lines = body.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    for line in lines {
        if line.trim_end() == "---" {
            return None;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() == field {
            let v = value.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                .unwrap_or(v);
            return Some(v.to_string());
        }
    }
    None
}

/// Extract the `name:` field from frontmatter, same rules as
/// `parse_description`. Only consumed by the build-time consistency
/// test that pins catalog name == frontmatter name.
#[cfg(test)]
fn parse_name(body: &str) -> Option<String> {
    parse_frontmatter_field(body, "name")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `SKILL.md` body file record with the given hash (`kind: Skill`).
    fn skill_file(hash: String) -> PiFileRecord {
        PiFileRecord {
            sha256: hash,
            kind: PiFileKind::Skill,
        }
    }

    /// A companion file record with the given hash (`kind: Companion`).
    fn companion_file(hash: String) -> PiFileRecord {
        PiFileRecord {
            sha256: hash,
            kind: PiFileKind::Companion,
        }
    }

    /// Build a `PiSkillRecord` from a body hash + companion `(filename, hash)`
    /// pairs — the flat-model test constructor.
    fn skill_record(
        cli_version: &str,
        body_hash: Option<String>,
        companions: &[(&str, String)],
    ) -> PiSkillRecord {
        let mut files: BTreeMap<String, PiFileRecord> = BTreeMap::new();
        if let Some(h) = body_hash {
            files.insert(PI_SKILL_FILENAME.to_string(), skill_file(h));
        }
        for (name, hash) in companions {
            files.insert((*name).to_string(), companion_file(hash.clone()));
        }
        PiSkillRecord {
            cli_version: cli_version.to_string(),
            files,
        }
    }

    #[test]
    fn every_embedded_skill_is_valid_portable_yaml_frontmatter() {
        for skill in SKILLS {
            let mut lines = skill.body.lines();
            assert_eq!(
                lines.next(),
                Some("---"),
                "{} frontmatter start",
                skill.name
            );
            let mut frontmatter = Vec::new();
            let mut closed = false;
            for line in lines.by_ref() {
                if line == "---" {
                    closed = true;
                    break;
                }
                frontmatter.push(line);
            }
            assert!(
                closed,
                "{} frontmatter has no closing delimiter",
                skill.name
            );
            let yaml = frontmatter.join("\n");
            let value: serde_norway::Value =
                serde_norway::from_str(&yaml).unwrap_or_else(|error| {
                    panic!("skill {} has invalid YAML frontmatter: {error}", skill.name)
                });
            let mapping = value
                .as_mapping()
                .unwrap_or_else(|| panic!("skill {} frontmatter is not a mapping", skill.name));
            for required in ["name", "description"] {
                let value = mapping
                    .get(serde_norway::Value::String(required.into()))
                    .unwrap_or_else(|| panic!("skill {} missing {required}", skill.name));
                assert!(
                    value.as_str().is_some_and(|text| !text.is_empty()),
                    "skill {} {required} must be a non-empty string",
                    skill.name
                );
            }
        }
    }

    #[test]
    fn every_embedded_skill_has_a_description_and_matching_name() {
        // Guards against frontmatter drift: if someone edits a SKILL.md
        // and breaks the `description:` line, `skill list` would silently
        // emit an empty string. Fail the build instead. Also pin
        // `name` equality between the catalog entry and the frontmatter
        // so a rename in one place can't quietly desync from the other.
        // (Review finding #5: extended to pin `cli_version` ==
        // CLI_VERSION and parseable `schema_version` so silent fallbacks
        // in `catalog()` and `cmd_print()` can't mask a broken template.)
        for s in SKILLS {
            let d = parse_description(s.body)
                .unwrap_or_else(|| panic!("skill {} missing description", s.name));
            assert!(!d.is_empty(), "skill {} has empty description", s.name);
            let n = parse_name(s.body).unwrap_or_else(|| panic!("skill {} missing name", s.name));
            assert_eq!(
                n, s.name,
                "catalog name {:?} does not match frontmatter name {:?}",
                s.name, n
            );
            let cli = parse_frontmatter_field(s.body, "cli_version")
                .unwrap_or_else(|| panic!("skill {} missing cli_version", s.name));
            assert_eq!(
                cli, CLI_VERSION,
                "skill {} cli_version {:?} does not match binary {:?}",
                s.name, cli, CLI_VERSION
            );
            let schema = parse_frontmatter_field(s.body, "schema_version")
                .unwrap_or_else(|| panic!("skill {} missing schema_version", s.name));
            let parsed: u32 = schema.parse().unwrap_or_else(|_| {
                panic!(
                    "skill {} has unparseable schema_version {:?}",
                    s.name, schema
                )
            });
            assert_eq!(
                parsed, SKILL_SCHEMA_VERSION,
                "skill {} schema_version {} != {}",
                s.name, parsed, SKILL_SCHEMA_VERSION
            );
            assert!(
                !s.body.contains("{{CLI_VERSION}}"),
                "skill {} still contains unrendered {{{{CLI_VERSION}}}} placeholder",
                s.name
            );
        }
    }

    #[test]
    fn every_companion_resource_is_rendered_and_version_pinned() {
        // The doctor `skill.sync.<name>.<file>` OK arm reports a companion
        // as "matching the bundled content for binary <CLI_VERSION>". That
        // claim only holds if the embedded companion body is fully rendered
        // (no leftover `{{CLI_VERSION}}` placeholder) and — when it carries
        // `cli_version` frontmatter at all — pins it to this binary's
        // version. Guard both here so a companion template can't silently
        // ship stale or unrendered.
        for name in SKILLS.iter().map(|s| s.name) {
            for r in resources_for(name) {
                assert!(
                    !r.body.contains("{{CLI_VERSION}}"),
                    "companion {} for skill {} still contains an unrendered {{{{CLI_VERSION}}}} placeholder",
                    r.filename,
                    name
                );
                if let Some(v) = parse_frontmatter_field(r.body, "cli_version") {
                    assert_eq!(
                        v, CLI_VERSION,
                        "companion {} for skill {} declares cli_version {:?}, binary is {:?}",
                        r.filename, name, v, CLI_VERSION
                    );
                }
            }
        }
    }

    #[test]
    fn every_claude_link_target_appears_in_some_skill_body() {
        // The codex rewrite is a literal `](target)` string replacement. If a
        // companion's `claude_link_target` ever drifts from the actual link
        // text in the skill bodies (a rename, a reflow), the rewrite silently
        // no-ops and codex ships a body still pointing at the un-resolvable
        // claude-layout path. Pin every declared target to real link text so
        // that drift fails the build instead of shipping a broken prompt.
        for skill in SKILLS {
            for r in resources_for(skill.name) {
                for target in r.claude_link_targets {
                    let needle = format!("]({target})");
                    assert!(
                        SKILLS.iter().any(|s| s.body.contains(&needle)),
                        "no skill body contains link {needle:?} declared for companion {} of {}",
                        r.filename,
                        skill.name
                    );
                }
            }
        }
    }

    #[test]
    fn render_body_for_claude_is_byte_identical_and_borrowed() {
        // The claude layout must be entirely unaffected by the codex rewrite:
        // every claude body is the embedded source, returned borrowed (no
        // reallocation, no byte change).
        for s in SKILLS {
            let rendered = render_body_for_agent("claude", s.name, s.body);
            assert!(
                matches!(rendered, Cow::Borrowed(_)),
                "claude body for {} was reallocated",
                s.name
            );
            assert_eq!(
                &*rendered, s.body,
                "claude body for {} was modified",
                s.name
            );
        }
    }

    #[test]
    fn render_body_for_codex_without_companion_links_is_noop() {
        // A codex body that references no companion is returned borrowed and
        // unchanged — the global rewrite table only touches bodies that carry
        // a declared link form.
        let no_links = SKILLS
            .iter()
            .find(|s| s.name == "taskfleet-spawn-spinoff")
            .unwrap();
        let rendered = render_body_for_agent("codex", no_links.name, no_links.body);
        assert!(matches!(rendered, Cow::Borrowed(_)));
        assert_eq!(&*rendered, no_links.body);
    }

    #[test]
    fn codex_prompt_embeds_resource_content_and_rewrites_links() {
        static RESOURCES: &[EmbeddedResource] = &[EmbeddedResource {
            filename: "REFERENCE.md",
            body: "resource payload\n",
            claude_link_targets: &["REFERENCE.md"],
        }];
        let rendered = render_codex_prompt("Read [the reference](REFERENCE.md).\n", RESOURCES);
        assert!(rendered.contains("[the reference](#taskfleet-resource-reference-md)"));
        assert!(rendered.contains("## Taskfleet resource: REFERENCE.md"));
        assert!(rendered.contains("resource payload"));
        assert!(!rendered.contains("](REFERENCE.md)"));
    }

    #[test]
    fn stint_skills_pin_issuectl_dag_cutover() {
        for name in ["stint-start", "stint-handoff"] {
            let body = SKILLS.iter().find(|s| s.name == name).unwrap().body;
            assert!(body.contains("issuectl dag --json"), "{name}");
            for forbidden in [
                "AGENTS-EXECUTION-DAG.md",
                "execution-dag:begin",
                "execution-dag:end",
                "comm -3",
                "GLOBAL HEAD-OF-LINE",
                "collision:",
            ] {
                assert!(
                    !body.contains(forbidden),
                    "{name} still contains retired DAG notation {forbidden:?}"
                );
            }
        }
        for name in ["stint-start", "stint-handoff"] {
            let skill = SKILLS.iter().find(|s| s.name == name).unwrap();
            assert!(skill.body.contains("--reservations"), "{name}");
        }
    }

    #[test]
    fn bundled_worker_closing_recipes_never_use_short_prefix_discovery() {
        for skill in SKILLS {
            for forbidden in ["grep -m1", "ls -1 ~/.taskfleet/runs/", "([0-9a-z]{10})"] {
                assert!(
                    !skill.body.contains(forbidden),
                    "{} contains vulnerable run-id discovery {forbidden:?}",
                    skill.name
                );
            }
        }
        for name in [
            "worktree-merge",
            "worktree-spinoff",
            "worktree-research",
            "worktree-bug-analysis",
            "worktree-technical-decision",
            "fan-out",
        ] {
            let skill = SKILLS.iter().find(|s| s.name == name).unwrap();
            assert!(skill.body.contains("run show --current"), "{name}");
            assert!(
                skill.body.contains("failed to resolve exact owning run id"),
                "{name}"
            );
        }
    }

    #[test]
    fn worktree_and_handoff_skills_pin_unlaned_review_filing_boundary() {
        let spinoff = SKILLS
            .iter()
            .find(|s| s.name == "worktree-spinoff")
            .unwrap()
            .body;
        assert!(spinoff.contains("issuectl intake file"));
        assert!(spinoff.contains("born unlaned"));
        assert!(spinoff.contains("AI-review provenance"));
        assert!(spinoff.contains("`/assess-findings`"));

        let handoff = SKILLS
            .iter()
            .find(|s| s.name == "stint-handoff")
            .unwrap()
            .body;
        assert!(handoff.contains("issuectl intake file"));
        assert!(handoff.contains("human lane-or-close sweep owns scheduling"));
        assert!(handoff.contains("Named model agreement stays a list"));
        assert!(handoff.contains("severity/confidence metadata"));
    }

    #[test]
    fn write_marker_records_companions_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join(MANAGED_MARKER_FILENAME);
        write_marker(
            &marker,
            "stint-start",
            &["A.md".to_string(), "B.md".to_string()],
        )
        .unwrap();
        let recorded = read_managed_companions(&marker);
        assert_eq!(recorded, vec!["A.md".to_string(), "B.md".to_string()]);
        // A markerless dir records nothing.
        assert!(read_managed_companions(&dir.path().join("nope")).is_empty());
    }

    #[test]
    fn orphan_companions_flags_dropped_but_not_bundled() {
        // Simulate a prior binary that installed a companion this binary no
        // longer ships. The dropped file lingers and the marker still records it.
        let skill = "stint-start";
        let dir = tempfile::tempdir().unwrap();
        write_marker(
            &dir.path().join(MANAGED_MARKER_FILENAME),
            skill,
            &["OLD-COMPANION.md".to_string()],
        )
        .unwrap();
        fs::write(dir.path().join("OLD-COMPANION.md"), "stale").unwrap();

        let orphans = orphan_companions(skill, dir.path());
        assert_eq!(
            orphans,
            vec!["OLD-COMPANION.md".to_string()],
            "only the dropped-but-recorded companion is an orphan"
        );
    }

    #[test]
    fn orphan_companions_ignores_unrecorded_user_file() {
        // A file a user dropped into the managed dir that the marker never
        // recorded must NOT be flagged — that is the false-positive the
        // marker-based design exists to avoid.
        let skill = "stint-start";
        let bundled: Vec<String> = resources_for(skill)
            .iter()
            .map(|r| r.filename.to_string())
            .collect();
        let dir = tempfile::tempdir().unwrap();
        // Marker records only the bundled companions (a clean install).
        write_marker(&dir.path().join(MANAGED_MARKER_FILENAME), skill, &bundled).unwrap();
        // User drops their own note — never recorded in the marker.
        fs::write(dir.path().join("my-note.md"), "mine").unwrap();

        assert!(
            orphan_companions(skill, dir.path()).is_empty(),
            "an unrecorded user file is not an orphan"
        );
    }

    #[test]
    fn orphan_companions_ignores_recorded_but_absent_file() {
        // The marker records an orphan whose file was already removed: nothing
        // to clean, so it is not reported (a WARN with no fixable target would
        // be noise).
        let skill = "stint-start";
        let bundled: Vec<String> = resources_for(skill)
            .iter()
            .map(|r| r.filename.to_string())
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let mut recorded = bundled.clone();
        recorded.push("GONE.md".to_string());
        write_marker(&dir.path().join(MANAGED_MARKER_FILENAME), skill, &recorded).unwrap();
        // `GONE.md` is NOT written to disk.
        assert!(orphan_companions(skill, dir.path()).is_empty());
    }

    #[test]
    fn codex_marker_records_prompts_and_companions_read_back() {
        // The single codex marker records both `prompt:` and `companion:`
        // lines; each key's reader returns only its own records.
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join(MANAGED_MARKER_FILENAME);
        write_codex_marker(
            &marker,
            &["stint-start".to_string(), "worktree-code".to_string()],
            &["REFERENCE.md".to_string()],
        )
        .unwrap();
        assert_eq!(
            read_marker_records(&marker, "prompt"),
            vec!["stint-start".to_string(), "worktree-code".to_string()]
        );
        assert_eq!(
            read_marker_records(&marker, "companion"),
            vec!["REFERENCE.md".to_string()]
        );
        // An absent marker records nothing for either key.
        let missing = dir.path().join("nope");
        assert!(read_marker_records(&missing, "prompt").is_empty());
        assert!(read_marker_records(&missing, "companion").is_empty());
    }

    #[test]
    fn all_companion_sources_dedupes_by_filename() {
        // Every declared companion filename is unique and the list is sorted;
        // the shared `_shared/` layout depends on one entry per filename.
        let sources = all_companion_sources();
        let mut names: Vec<&str> = sources.iter().map(|c| c.filename).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(
            names, sorted,
            "companion sources must be sorted by filename"
        );
        names.dedup();
        assert_eq!(
            names.len(),
            sources.len(),
            "companion filenames must be unique across skills"
        );
    }

    #[test]
    fn prune_codex_file_outcomes() {
        let dir = tempfile::tempdir().unwrap();
        let mut warnings: Vec<String> = Vec::new();

        // A regular file is removed.
        let regular = dir.path().join("gone.md");
        fs::write(&regular, "stale").unwrap();
        assert!(matches!(
            prune_codex_file(&regular, "removed", "failed", &mut warnings),
            CodexPruneOutcome::Removed
        ));
        assert!(!regular.exists(), "file must be gone after Removed");
        assert!(warnings[0].starts_with("removed at "));

        // An absent file yields Dropped (nothing to clean).
        let absent = dir.path().join("never.md");
        assert!(matches!(
            prune_codex_file(&absent, "removed", "failed", &mut warnings),
            CodexPruneOutcome::Dropped
        ));

        // A directory squatting at the path is never removed (Dropped, not a
        // recursive delete).
        let squat = dir.path().join("squat.md");
        fs::create_dir(&squat).unwrap();
        assert!(matches!(
            prune_codex_file(&squat, "removed", "failed", &mut warnings),
            CodexPruneOutcome::Dropped
        ));
        assert!(squat.is_dir(), "a squatting dir must be left intact");
    }

    #[test]
    fn compare_versions_handles_semver_ordering() {
        use std::cmp::Ordering;
        // Pre-release is *less than* the release.
        assert_eq!(
            compare_versions("1.0.0-alpha", "1.0.0"),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare_versions("1.0.0", "1.0.0-alpha"),
            Some(Ordering::Greater)
        );
        // Standard numeric ordering.
        assert_eq!(compare_versions("1.10.0", "1.9.0"), Some(Ordering::Greater));
        assert_eq!(compare_versions("0.0.1", "0.0.1"), Some(Ordering::Equal));
        // Unparseable → None so callers route to the legacy arm.
        assert_eq!(compare_versions("banana", "1.0.0"), None);
        assert_eq!(compare_versions("1.0.0", "1.x"), None);
        assert_eq!(compare_versions("{{CLI_VERSION}}", "1.0.0"), None);
    }

    #[test]
    fn parse_description_extracts_value() {
        let body = "---\nname: foo\ndescription: a short blurb\nversion: 1\n---\n\n# body\n";
        assert_eq!(parse_description(body).as_deref(), Some("a short blurb"));
    }

    #[test]
    fn parse_description_handles_crlf() {
        let body = "---\r\nname: foo\r\ndescription: blurb\r\n---\r\n";
        assert_eq!(parse_description(body).as_deref(), Some("blurb"));
    }

    #[test]
    fn parse_description_strips_quotes() {
        let body = "---\ndescription: \"quoted blurb\"\n---\n";
        assert_eq!(parse_description(body).as_deref(), Some("quoted blurb"));
    }

    #[test]
    fn parse_description_returns_none_without_frontmatter() {
        assert_eq!(parse_description("# just a heading\n"), None);
    }

    #[test]
    fn parse_description_returns_none_when_field_absent() {
        let body = "---\nname: foo\nversion: 1\n---\n";
        assert_eq!(parse_description(body), None);
    }

    /// Every bundled skill's `description:` frontmatter must fit pi.dev's
    /// 1024-*character* limit — pi warns on load past it and drops the
    /// overflow, degrading skill selection (issue
    /// `stint-skill-desc-over-pi-limit`). Counted in Unicode scalar values,
    /// not bytes, because the descriptions carry multi-byte glyphs (ä, →).
    #[test]
    fn bundled_descriptions_fit_pi_char_limit() {
        const PI_DESCRIPTION_LIMIT: usize = 1024;
        for skill in SKILLS {
            let desc = parse_description(skill.body)
                .unwrap_or_else(|| panic!("skill {} has no description", skill.name));
            let chars = desc.chars().count();
            assert!(
                chars <= PI_DESCRIPTION_LIMIT,
                "skill {} description is {chars} chars (> {PI_DESCRIPTION_LIMIT} pi.dev limit)",
                skill.name
            );
        }
    }

    #[test]
    fn sha256_hex_is_deterministic_and_lowercase() {
        let a = sha256_hex(b"hello");
        assert_eq!(a, sha256_hex(b"hello"));
        assert_ne!(a, sha256_hex(b"world"));
        assert_eq!(a.len(), 64);
        assert!(a
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        // Known vector for "hello".
        assert_eq!(
            a,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn pi_provenance_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("pi-installed-skills.json");
        let mut prov = PiProvenance {
            schema_version: PI_PROVENANCE_SCHEMA_VERSION,
            skills: BTreeMap::new(),
        };
        prov.skills.insert(
            "stint-start".to_string(),
            skill_record("0.1.7", Some(sha256_hex(b"body-a")), &[]),
        );
        // Parent dir does not exist yet — the write must create `state/`.
        write_pi_provenance(&path, &prov).unwrap();
        let read_back = read_pi_provenance(&path);
        assert_eq!(read_back.schema_version, PI_PROVENANCE_SCHEMA_VERSION);
        assert_eq!(
            read_back
                .skills
                .get("stint-start")
                .and_then(|r| r.body_hash().map(str::to_string)),
            Some(sha256_hex(b"body-a"))
        );
    }

    #[test]
    fn read_pi_provenance_tolerates_missing_and_garbage() {
        let dir = tempfile::tempdir().unwrap();
        // Missing file → empty default.
        let missing = dir.path().join("nope.json");
        assert!(read_pi_provenance(&missing).skills.is_empty());
        // Unparseable content → empty default (err toward NOT managing).
        let garbage = dir.path().join("garbage.json");
        fs::write(&garbage, "{ not json").unwrap();
        assert!(read_pi_provenance(&garbage).skills.is_empty());
        // Future-schema record → lenient reader treats it as empty (doctor
        // must not audit a record a newer binary wrote).
        let future = dir.path().join("future.json");
        fs::write(&future, r#"{"schema_version":999,"skills":{}}"#).unwrap();
        assert!(read_pi_provenance(&future).skills.is_empty());
    }

    #[test]
    fn load_pi_provenance_for_write_fails_closed_on_corruption() {
        let dir = tempfile::tempdir().unwrap();
        // Missing → Ok(empty): a first install starts fresh.
        let missing = dir.path().join("nope.json");
        assert!(load_pi_provenance_for_write(&missing)
            .unwrap()
            .skills
            .is_empty());
        // Unparseable → Err (never launder + overwrite, which would erase all
        // tracking).
        let garbage = dir.path().join("garbage.json");
        fs::write(&garbage, "{ not json").unwrap();
        let err = load_pi_provenance_for_write(&garbage).unwrap_err();
        assert_eq!(err.code, "pi_provenance_corrupt");
        // Future schema → Err (an older binary must not downgrade-overwrite it).
        let future = dir.path().join("future.json");
        fs::write(&future, r#"{"schema_version":999,"skills":{}}"#).unwrap();
        let err = load_pi_provenance_for_write(&future).unwrap_err();
        assert_eq!(err.code, "pi_provenance_schema_too_new");
    }

    #[test]
    fn is_simple_skill_name_rejects_traversal_and_absolute() {
        assert!(is_simple_skill_name("stint-start"));
        assert!(is_simple_skill_name("worktree-code"));
        assert!(!is_simple_skill_name("../../.bashrc"));
        assert!(!is_simple_skill_name("a/b"));
        assert!(!is_simple_skill_name("/etc/passwd"));
        assert!(!is_simple_skill_name(".."));
        assert!(!is_simple_skill_name(""));
    }

    #[test]
    fn write_pi_provenance_replaces_squatting_symlink() {
        // A symlink squatting at the record path must be replaced by the atomic
        // rename, never followed to clobber its target.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("victim.txt");
        fs::write(&target, "precious").unwrap();
        let record = dir.path().join("pi-installed-skills.json");
        std::os::unix::fs::symlink(&target, &record).unwrap();

        let prov = PiProvenance {
            schema_version: PI_PROVENANCE_SCHEMA_VERSION,
            skills: BTreeMap::new(),
        };
        write_pi_provenance(&record, &prov).unwrap();

        // The victim is untouched; the record path is now a regular file.
        assert_eq!(fs::read_to_string(&target).unwrap(), "precious");
        assert!(fs::symlink_metadata(&record).unwrap().file_type().is_file());
    }

    #[test]
    fn prune_pi_mirror_removes_our_unmodified_copy_and_empty_dir() {
        let root = tempfile::tempdir().unwrap();
        let skill_dir = root.path().join("old-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let mirror = skill_dir.join("SKILL.md");
        let body = b"---\nname: old-skill\n---\nbody\n";
        fs::write(&mirror, body).unwrap();

        let mut rec = skill_record("0.1.0", Some(sha256_hex(body)), &[]);
        let mut warnings = Vec::new();
        let summary = prune_pi_mirror_at(
            "old-skill",
            &skill_dir,
            Some(root.path()),
            &mut rec,
            &mut warnings,
        );
        assert!(summary.body_removed);
        assert!(rec.files.is_empty(), "record fully cleared");
        assert!(!mirror.exists(), "mirror file must be gone");
        assert!(
            !skill_dir.exists(),
            "empty per-skill dir must be cleaned up"
        );
        assert!(warnings.iter().any(|w| w.starts_with("pi_mirror_pruned:")));
    }

    #[test]
    fn prune_pi_mirror_removes_recorded_companions_then_empty_dir() {
        // A de-registered skill's recorded companions are removed alongside its
        // SKILL.md so the per-skill dir empties out. A companion that has since
        // diverged from what we wrote is LEFT in place (and keeps the dir).
        let root = tempfile::tempdir().unwrap();
        let skill_dir = root.path().join("old-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let mirror = skill_dir.join("SKILL.md");
        let body = b"---\nname: old-skill\n---\nbody\n";
        fs::write(&mirror, body).unwrap();
        let comp_body = b"companion payload\n";
        fs::write(skill_dir.join("REFERENCE.md"), comp_body).unwrap();
        // A second recorded companion the user has since edited: must survive.
        fs::write(skill_dir.join("EDITED.md"), b"user changed this").unwrap();

        let mut rec = skill_record(
            "0.1.0",
            Some(sha256_hex(body)),
            &[
                ("REFERENCE.md", sha256_hex(comp_body)),
                ("EDITED.md", sha256_hex(b"original edited body")),
            ],
        );

        let mut warnings = Vec::new();
        let summary = prune_pi_mirror_at(
            "old-skill",
            &skill_dir,
            Some(root.path()),
            &mut rec,
            &mut warnings,
        );
        assert!(summary.body_removed);
        assert!(!mirror.exists(), "SKILL.md removed");
        assert!(
            !skill_dir.join("REFERENCE.md").exists(),
            "our unmodified companion is removed"
        );
        assert!(
            skill_dir.join("EDITED.md").exists(),
            "a diverged companion is preserved"
        );
        // The dir is NOT empty (the diverged companion remains), so it survives.
        assert!(skill_dir.exists(), "dir with a surviving companion is kept");
        assert!(warnings
            .iter()
            .any(|w| w.starts_with("pi_companion_pruned:")));
        assert!(warnings
            .iter()
            .any(|w| w.starts_with("pi_companion_diverged:")));
    }

    #[test]
    fn prune_pi_mirror_removes_all_matching_companions_and_empty_dir() {
        // When every recorded companion is our unmodified copy, both it and the
        // SKILL.md are removed and the now-empty dir is cleaned up.
        let root = tempfile::tempdir().unwrap();
        let skill_dir = root.path().join("old-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let mirror = skill_dir.join("SKILL.md");
        let body = b"body\n";
        fs::write(&mirror, body).unwrap();
        let c1 = b"c1\n";
        let c2 = b"c2\n";
        fs::write(skill_dir.join("A.md"), c1).unwrap();
        fs::write(skill_dir.join("B.md"), c2).unwrap();
        let mut rec = skill_record(
            "0.1.0",
            Some(sha256_hex(body)),
            &[("A.md", sha256_hex(c1)), ("B.md", sha256_hex(c2))],
        );

        let mut warnings = Vec::new();
        let summary = prune_pi_mirror_at(
            "old-skill",
            &skill_dir,
            Some(root.path()),
            &mut rec,
            &mut warnings,
        );
        assert!(summary.body_removed);
        assert!(rec.files.is_empty(), "record fully cleared");
        assert!(!skill_dir.exists(), "fully-cleaned dir must be removed");
    }

    #[test]
    fn prune_pi_mirror_cleans_companions_when_skill_md_absent() {
        // A prior partial prune left the SKILL.md gone but a recorded companion
        // behind. The prune must still clean the companion (never strand it) and
        // remove the now-empty dir. The body was already gone (absent), so
        // `body_removed` is false and the record clears out.
        let root = tempfile::tempdir().unwrap();
        let skill_dir = root.path().join("old-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let comp = b"companion\n";
        fs::write(skill_dir.join("C.md"), comp).unwrap();
        // Record still tracks the (now-absent) body plus the leftover companion.
        let mut rec = skill_record(
            "0.1.0",
            Some("irrelevant-body-hash".to_string()),
            &[("C.md", sha256_hex(comp))],
        );

        let mut warnings = Vec::new();
        let summary = prune_pi_mirror_at(
            "old-skill",
            &skill_dir,
            Some(root.path()),
            &mut rec,
            &mut warnings,
        );
        assert!(!summary.body_removed, "absent body was not deleted by us");
        assert!(
            rec.files.is_empty(),
            "record cleared (companion + absent body)"
        );
        assert!(
            !skill_dir.join("C.md").exists(),
            "recorded companion cleaned even with an absent SKILL.md"
        );
        assert!(!skill_dir.exists(), "now-empty dir removed");
        assert!(warnings
            .iter()
            .any(|w| w.starts_with("pi_companion_pruned:")));
    }

    #[test]
    fn prune_pi_mirror_diverged_body_still_prunes_unmodified_companion() {
        // Flat per-file model: a user-edited SKILL.md is relinquished (left in
        // place, dropped from tracking), but an UNMODIFIED companion is still our
        // copy and IS pruned — the body's divergence no longer shields the
        // companions (issue `pi-provenance-flat-file-model`). The surviving
        // diverged body keeps the dir.
        let root = tempfile::tempdir().unwrap();
        let skill_dir = root.path().join("old-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let mirror = skill_dir.join("SKILL.md");
        fs::write(&mirror, b"user edited body").unwrap();
        let comp = b"companion\n";
        fs::write(skill_dir.join("C.md"), comp).unwrap();
        let mut rec = skill_record(
            "0.1.0",
            Some(sha256_hex(b"the body we originally wrote")),
            &[("C.md", sha256_hex(comp))],
        );

        let mut warnings = Vec::new();
        let summary = prune_pi_mirror_at(
            "old-skill",
            &skill_dir,
            Some(root.path()),
            &mut rec,
            &mut warnings,
        );
        assert!(!summary.body_removed, "diverged body is not deleted");
        assert!(mirror.exists(), "diverged body is left in place");
        assert!(
            !skill_dir.join("C.md").exists(),
            "an unmodified companion is pruned even though the body diverged"
        );
        assert!(
            skill_dir.exists(),
            "the surviving diverged body keeps the dir"
        );
        assert!(
            rec.files.is_empty(),
            "both files relinquished/pruned from tracking"
        );
        assert!(warnings
            .iter()
            .any(|w| w.starts_with("pi_mirror_diverged:")));
        assert!(warnings
            .iter()
            .any(|w| w.starts_with("pi_companion_pruned:")));
    }

    #[test]
    fn reconcile_pi_companions_force_removes_dropped_companion() {
        // A still-registered skill whose record tracks companions the binary no
        // longer bundles: under --force every orphan is removed from disk and record.
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        // The fixture carries two companions from the prior catalog.
        let dag = b"dag body\n";
        let old = b"old body\n";
        fs::write(dir.join("REFERENCE.md"), dag).unwrap();
        fs::write(dir.join("OLD.md"), old).unwrap();

        let mut rec = skill_record(
            CLI_VERSION,
            Some(sha256_hex(b"skill body")),
            &[
                ("REFERENCE.md", sha256_hex(dag)),
                ("OLD.md", sha256_hex(old)),
            ],
        );

        let mut pruned = Vec::new();
        let mut warnings = Vec::new();
        reconcile_pi_companions_at(
            "stint-start",
            &mut rec,
            dir,
            true,
            &mut pruned,
            &mut warnings,
        );

        assert_eq!(
            pruned,
            vec![
                "stint-start/OLD.md".to_string(),
                "stint-start/REFERENCE.md".to_string(),
            ]
        );
        assert!(!rec.files.contains_key("REFERENCE.md"));
        assert!(!rec.files.contains_key("OLD.md"));
        assert!(!dir.join("OLD.md").exists());
        assert!(!dir.join("REFERENCE.md").exists());
    }

    #[test]
    fn reconcile_pi_companions_non_force_keeps_orphan_tracked() {
        // Without --force the stale companion is LEFT tracked so `doctor` keeps
        // flagging it (its --force fix is what clears it).
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        fs::write(dir.join("OLD.md"), b"old\n").unwrap();

        let mut rec = skill_record(
            CLI_VERSION,
            Some(sha256_hex(b"body")),
            &[("OLD.md", sha256_hex(b"old\n"))],
        );

        let mut pruned = Vec::new();
        let mut warnings = Vec::new();
        reconcile_pi_companions_at(
            "stint-start",
            &mut rec,
            dir,
            false,
            &mut pruned,
            &mut warnings,
        );

        assert!(pruned.is_empty(), "non-force prunes nothing");
        assert!(
            rec.files.contains_key("OLD.md"),
            "orphan stays tracked without --force"
        );
        assert!(dir.join("OLD.md").exists(), "orphan file left on disk");
    }

    #[test]
    fn pi_provenance_v1_record_reads_and_upgrades_to_v3() {
        // A legacy v1 record (bare `sha256`/`cli_version`, no companions) upgrades
        // in place on load: the body `sha256` becomes the `SKILL.md` file entry,
        // and a fresh write stamps the current schema (v3, flat `files`).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("pi-installed-skills.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"schema_version":1,"skills":{"stint-start":{"sha256":"aa","cli_version":"0.1.0"}}}"#,
        )
        .unwrap();
        let mut prov = load_pi_provenance_for_write(&path).unwrap();
        assert_eq!(prov.schema_version, 1, "read preserves the on-disk version");
        let rec = &prov.skills["stint-start"];
        assert_eq!(
            rec.body_hash(),
            Some("aa"),
            "legacy body hash upgraded to a Skill file"
        );
        assert!(rec.companion_names().is_empty(), "v1 had no companions");
        // A write stamps the current (v3) schema with the flat `files` shape.
        prov.schema_version = PI_PROVENANCE_SCHEMA_VERSION;
        write_pi_provenance(&path, &prov).unwrap();
        let reread: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(reread["schema_version"], 3);
        assert_eq!(
            reread["skills"]["stint-start"]["files"]["SKILL.md"]["sha256"],
            "aa"
        );
        assert_eq!(
            reread["skills"]["stint-start"]["files"]["SKILL.md"]["kind"],
            "skill"
        );
    }

    #[test]
    fn pi_provenance_v2_record_upgrades_companions_to_files() {
        // A v2 record (body `sha256` + `companions` map) upgrades so the body and
        // each companion become independent `files` entries with the right kind.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pi.json");
        fs::write(
            &path,
            r#"{"schema_version":2,"skills":{"stint-start":{"sha256":"bb","cli_version":"0.1.0","companions":{"REFERENCE.md":"cc"}}}}"#,
        )
        .unwrap();
        let prov = load_pi_provenance_for_write(&path).unwrap();
        let rec = &prov.skills["stint-start"];
        assert_eq!(rec.body_hash(), Some("bb"));
        assert_eq!(rec.companion_names(), vec!["REFERENCE.md".to_string()]);
        assert_eq!(rec.files["REFERENCE.md"].kind, PiFileKind::Companion);
    }

    #[test]
    fn pi_upgrade_drops_empty_legacy_hashes_and_skill_md_companion() {
        // A legacy record with an empty body hash must NOT mint a fake `SKILL.md`
        // entry (which would make doctor spuriously flag the real body as
        // "differs"); an empty companion hash is dropped; and a legacy companion
        // keyed `SKILL.md` (any case) must never alias/overwrite the body entry.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pi.json");
        fs::write(
            &path,
            r#"{"schema_version":2,"skills":{"s":{"sha256":"","cli_version":"0.1.0","companions":{"REFERENCE.md":"aa","EMPTY.md":"","SKILL.md":"bb","skill.md":"cc"}}}}"#,
        )
        .unwrap();
        let prov = load_pi_provenance_for_write(&path).unwrap();
        let rec = &prov.skills["s"];
        assert_eq!(
            rec.body_hash(),
            None,
            "empty legacy body hash → no body entry"
        );
        assert_eq!(
            rec.companion_names(),
            vec!["REFERENCE.md".to_string()],
            "empty-hash and SKILL.md-aliasing companions are dropped"
        );
    }

    #[test]
    fn pi_managed_skills_body_hash_is_none_for_companion_only_record() {
        // A companion-only record (body write skipped) surfaces `sha256: None`
        // rather than an empty string, so doctor skips the content-edit check.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("pi-installed-skills.json");
        let mut prov = PiProvenance {
            schema_version: PI_PROVENANCE_SCHEMA_VERSION,
            skills: BTreeMap::new(),
        };
        prov.skills.insert(
            "s".to_string(),
            skill_record("", None, &[("C.md", sha256_hex(b"c"))]),
        );
        write_pi_provenance(&path, &prov).unwrap();
        // Point pi_managed_skills at this record via the resolved home.
        let read = read_pi_provenance(&path);
        let rec = &read.skills["s"];
        assert_eq!(rec.body_hash(), None);
        assert_eq!(rec.companion_names(), vec!["C.md".to_string()]);
    }

    #[test]
    fn pi_provenance_v3_record_round_trips_untouched() {
        // A native v3 record with a `files` map is taken verbatim (the legacy
        // upgrade only fires when `files` is empty).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pi.json");
        fs::write(
            &path,
            r#"{"schema_version":3,"skills":{"s":{"cli_version":"1.0.0","files":{"SKILL.md":{"sha256":"dd","kind":"skill"},"C.md":{"sha256":"ee","kind":"companion"}}}}}"#,
        )
        .unwrap();
        let prov = load_pi_provenance_for_write(&path).unwrap();
        let rec = &prov.skills["s"];
        assert_eq!(rec.body_hash(), Some("dd"));
        assert_eq!(rec.files["C.md"].sha256, "ee");
        assert_eq!(rec.files["C.md"].kind, PiFileKind::Companion);
    }

    #[test]
    fn prune_pi_mirror_preserves_dir_with_user_sibling() {
        let root = tempfile::tempdir().unwrap();
        let skill_dir = root.path().join("old-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let mirror = skill_dir.join("SKILL.md");
        let body = b"our body\n";
        fs::write(&mirror, body).unwrap();
        // A user-added sibling in the same dir.
        fs::write(skill_dir.join("notes.md"), "mine").unwrap();

        let mut rec = skill_record("0.1.0", Some(sha256_hex(body)), &[]);
        let mut warnings = Vec::new();
        let summary = prune_pi_mirror_at(
            "old-skill",
            &skill_dir,
            Some(root.path()),
            &mut rec,
            &mut warnings,
        );
        assert!(summary.body_removed);
        assert!(rec.files.is_empty(), "our body relinquished from tracking");
        assert!(!mirror.exists(), "our SKILL.md is removed");
        assert!(skill_dir.exists(), "non-empty dir is preserved");
        assert!(skill_dir.join("notes.md").exists(), "user sibling survives");
    }

    #[test]
    fn prune_pi_mirror_refuses_diverged_copy() {
        let root = tempfile::tempdir().unwrap();
        let skill_dir = root.path().join("old-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let mirror = skill_dir.join("SKILL.md");
        fs::write(&mirror, b"user has edited this").unwrap();

        // Recorded hash is of the ORIGINAL body we wrote, which no longer matches.
        let mut rec = skill_record("0.1.0", Some(sha256_hex(b"original body")), &[]);
        let mut warnings = Vec::new();
        let summary = prune_pi_mirror_at(
            "old-skill",
            &skill_dir,
            Some(root.path()),
            &mut rec,
            &mut warnings,
        );
        assert!(!summary.body_removed, "a diverged copy is not deleted");
        assert!(
            rec.files.is_empty(),
            "the diverged body is relinquished from tracking"
        );
        assert!(
            mirror.exists(),
            "a diverged (user-owned) copy is NOT deleted"
        );
        assert!(warnings
            .iter()
            .any(|w| w.starts_with("pi_mirror_diverged:")));
    }

    #[test]
    fn prune_pi_mirror_drops_absent_symlink_and_dir() {
        let root = tempfile::tempdir().unwrap();
        let mut warnings = Vec::new();

        // Absent body: a per-skill dir that does not exist. Nothing to delete;
        // the record clears and nothing is reported removed.
        let mut rec = skill_record("0.1.0", Some("anyhash".to_string()), &[]);
        let summary = prune_pi_mirror_at(
            "gone",
            &root.path().join("gone"),
            Some(root.path()),
            &mut rec,
            &mut warnings,
        );
        assert!(!summary.body_removed);
        assert!(rec.files.is_empty(), "absent body dropped from tracking");

        // A directory squatting where SKILL.md should be is never removed.
        let squat_dir = root.path().join("squat");
        fs::create_dir_all(squat_dir.join(PI_SKILL_FILENAME)).unwrap();
        let mut rec = skill_record("0.1.0", Some("anyhash".to_string()), &[]);
        let summary = prune_pi_mirror_at(
            "squat",
            &squat_dir,
            Some(root.path()),
            &mut rec,
            &mut warnings,
        );
        assert!(!summary.body_removed);
        assert!(
            squat_dir.join(PI_SKILL_FILENAME).is_dir(),
            "a squatting dir must be left intact"
        );

        // A symlink at the body path is never followed/deleted.
        let link_dir = root.path().join("linked");
        fs::create_dir_all(&link_dir).unwrap();
        let real = root.path().join("real.md");
        fs::write(&real, b"body").unwrap();
        std::os::unix::fs::symlink(&real, link_dir.join(PI_SKILL_FILENAME)).unwrap();
        let mut rec = skill_record("0.1.0", Some(sha256_hex(b"body")), &[]);
        let summary = prune_pi_mirror_at(
            "linked",
            &link_dir,
            Some(root.path()),
            &mut rec,
            &mut warnings,
        );
        assert!(!summary.body_removed);
        assert!(
            link_dir.join(PI_SKILL_FILENAME).exists(),
            "the symlink is left intact"
        );
        assert!(real.exists(), "the symlink target is untouched");
    }
}
