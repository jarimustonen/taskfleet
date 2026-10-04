//! CI gate: every `taskfleet …` example in every bundled SKILL must
//! match the binary's actual CLI surface.
//!
//! # Why this exists
//!
//! Agents treat a bundled SKILL as the operating manual: the skill-catalog
//! version-check tells them "same version, proceed normally", so they run the
//! commands the SKILL documents verbatim. When a SKILL drifts from the binary
//! — a renamed flag, a positional written as `--flag`, an enum value that no
//! longer exists — the agent hits `unknown_subcommand_or_flag` or
//! `invalid_value` mid-workflow with no way to recover, because the manual
//! itself is wrong. Issue `skill-binary-doc-sync` catalogued a batch of such
//! drift found in production. This test is the long-term prevention: it
//! mechanically validates every documented invocation so the same class of
//! drift cannot ship again.
//!
//! # What it checks (v1 — shape only)
//!
//! For every `taskfleet …` command in a fenced code block of every
//! `crates/taskfleet/skills/<name>/SKILL.template.md`, the test reconstructs the
//! argv and runs it against the real binary with `--help` appended. `--help`
//! makes clap validate the whole argv during parsing — unknown flags
//! (`unknown_subcommand_or_flag`), unknown subcommands, and out-of-range enum
//! values (`invalid_value`) all error here — and then short-circuit with exit 0
//! *before any command handler runs*. So a structurally-valid invocation exits
//! 0 with no side effects (no run is created, no supervisor is spawned), and a
//! drifted one exits non-zero with the shape error in its JSON envelope.
//!
//! Because `--help` bypasses clap's required-argument and mutually-required
//! group checks, a SKILL may show a partial command (only the flags relevant to
//! the point it is making) without tripping the gate. We validate the *shape*
//! of what is written, not completeness.
//!
//! Envelope-shape diffing (does the SKILL's example JSON output still match the
//! binary's?) is out of scope for v1 — that is tracked separately as v2.
//!
//! # Allow-listing genuinely illustrative examples
//!
//! Some fenced examples are deliberately *not* literal invocations — they show
//! a verb in the abstract, or a shape that is not meant to parse. Mark such a
//! command so the gate skips it with the magic comment:
//!
//! ```text
//! # skill-example-ci: skip
//! ```
//!
//! Place it either on its own line immediately above the `taskfleet`
//! command, or as a trailing comment on the command's first line. The skip
//! applies to that single command (the next `taskfleet …` in the block).
//! Prefer fixing the example over allow-listing it — the allow-list is for
//! commands that genuinely cannot be made to parse, not for hiding drift.
//!
//! # Normalization applied before validation
//!
//! Real SKILL command blocks use a few documentation conventions that are not
//! literal shell. The extractor normalizes them so the underlying invocation
//! can be checked:
//!
//! * **Line continuations** — a trailing `\` joins the next line into one
//!   command, exactly as a shell would.
//! * **Optional-flag brackets** — `[--source-branch <branch>]` documents an
//!   optional flag. The surrounding `[` / `]` are stripped and the flag is
//!   validated like any other; a bracketed flag that does not exist is still
//!   drift the gate must catch.
//! * **Placeholders** — `<kind>` (the only enum-valued placeholder clap
//!   validates before `--help` short-circuits) is substituted with a real
//!   kind, and `{{CLI_VERSION}}` with the crate version. Every other `<…>` /
//!   `$var` placeholder is left as an opaque string value: clap accepts it as a
//!   positional/flag value and never reaches it before exiting on `--help`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

/// Magic comment that allow-lists a single illustrative example.
const SKIP_MARKER: &str = "# skill-example-ci: skip";

/// A single `taskfleet …` invocation extracted from a SKILL, with enough
/// provenance to point a maintainer straight at the offending line.
#[derive(Debug)]
struct Invocation {
    skill: String,
    /// 1-based line number of the command's first line in the SKILL file.
    line: usize,
    /// argv *after* the leading `taskfleet` token — i.e. the args we
    /// hand to the binary, before appending `--help`.
    argv: Vec<String>,
}

/// THE gate. Every documented invocation must validate against the binary.
#[test]
fn every_skill_taskfleet_example_matches_the_binary() {
    let skills_dir = skills_dir();
    let home = TempDir::new().expect("temp TASKFLEET_HOME");

    let mut invocations = Vec::new();
    for skill_md in skill_template_paths(&skills_dir) {
        let skill_name = skill_md
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("<unknown>")
            .to_string();
        let body = std::fs::read_to_string(&skill_md)
            .unwrap_or_else(|e| panic!("read {}: {e}", skill_md.display()));
        invocations.extend(extract_invocations(&skill_name, &body));
    }

    assert!(
        !invocations.is_empty(),
        "extracted zero taskfleet invocations — the extractor or the \
         skills directory ({}) is broken; this gate would be silently \
         vacuous",
        skills_dir.display()
    );

    let mut failures = Vec::new();
    for inv in &invocations {
        if let Err(reason) = validate(&inv.argv, &home) {
            failures.push(format!(
                "  {} (line {}):\n    $ taskfleet {}\n    → {}",
                inv.skill,
                inv.line,
                inv.argv.join(" "),
                reason
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} documented taskfleet invocation(s) do not match the \
         binary's CLI surface.\n\nEach failing example below documents a flag, \
         subcommand, positional, or enum value the binary does not accept. Fix \
         the SKILL to match the binary (run the command with `--help` to see \
         the real surface), or — if the example is genuinely illustrative and \
         cannot parse — add a `{SKIP_MARKER}` comment above it.\n\n{}",
        failures.len(),
        invocations.len(),
        failures.join("\n")
    );
}

/// Locks the gate's own failure-detection so a future refactor cannot quietly
/// turn it into a no-op: a deliberately-bogus flag MUST be reported as a shape
/// error. This is the automated form of Done-criteria #3 (manually inserting
/// `--bogus-flag` and confirming the gate trips).
#[test]
fn gate_rejects_a_bogus_flag() {
    let home = TempDir::new().expect("temp TASKFLEET_HOME");
    let argv = [
        "run".to_string(),
        "create".to_string(),
        "--kind".to_string(),
        "fan-out".to_string(),
        "--definitely-not-a-real-flag".to_string(),
    ];
    let result = validate(&argv, &home);
    let err = result.expect_err("gate must reject an unknown flag");
    assert!(
        err.contains("unknown_subcommand_or_flag"),
        "expected an unknown-flag shape error, got: {err}"
    );
}

/// A valid, fully-formed `run create` must validate *and* leave no trace —
/// `--help` short-circuits before the handler runs, so no run directory is
/// ever created. Guards the "zero side effects" property the whole gate
/// relies on.
#[test]
fn validation_has_no_side_effects() {
    let home = TempDir::new().expect("temp TASKFLEET_HOME");
    let argv = [
        "run".to_string(),
        "create".to_string(),
        "--kind".to_string(),
        "fan-out".to_string(),
        "--title".to_string(),
        "t".to_string(),
        "--task".to_string(),
        "do the thing".to_string(),
    ];
    validate(&argv, &home).expect("a well-formed run create must validate");
    assert!(
        !home.path().join("runs").exists(),
        "validation created a runs/ directory — `--help` did not short-circuit \
         before the handler; the gate is no longer side-effect-free"
    );
}

/// Documentation-contract smoke test for every bundled workflow that creates a
/// worker brief. Assertions are scoped to the copyable section so unrelated
/// prose cannot mask a broken blocked-handoff recipe.
#[test]
fn worker_skill_failure_disclosure_sections_are_self_contained() {
    const WORKER_SKILLS: [&str; 6] = [
        "fan-out",
        "taskfleet-spawn-spinoff",
        "worktree-bug-analysis",
        "worktree-research",
        "worktree-spinoff",
        "worktree-technical-decision",
    ];

    for skill in WORKER_SKILLS {
        let path = skills_dir().join(skill).join("SKILL.template.md");
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let section = markdown_section(&body, "## Tool and sub-workflow failure disclosure")
            .unwrap_or_else(|| panic!("{} is missing the disclosure section", path.display()));
        let required = [
            // Required failure takes the existing non-merge terminal path.
            "**required**",
            "`success: false`",
            "Do not call `run merge`",
            "taskfleet node report",
            "--from-file",
            // Optional failure can continue only through a full disclosure.
            "**optional/advisory**",
            "may continue only",
            "`success: true`",
            "--report-file",
            // Detectable incompleteness and bounded retry exhaustion.
            "Requested completeness is a contract",
            "incomplete",
            "Retry only",
            "finite bound",
            "exhaustion",
            // Existing report fields carry bounded, secret-safe context.
            "discussion_items[]",
            "Tool/sub-workflow failure —",
            "`options`",
            "do not put them inside the",
            "KiB",
            "secrets, credentials",
            "artifact/log",
            "do not add a schema or",
        ];
        for phrase in required {
            assert!(
                section.contains(phrase),
                "{} disclosure section is missing {phrase:?}",
                path.display()
            );
        }
        assert!(
            body.contains("copy the disclosure contract below"),
            "{} does not require the spawning brief to carry the contract",
            path.display()
        );
        assert!(
            body.contains("exactly one terminal path"),
            "{} does not make merge and blocked handoff exclusive",
            path.display()
        );
    }

    // Panel completeness is workflow-specific, not boilerplate forced into
    // generic bug-analysis or fan-out prompts.
    for skill in [
        "taskfleet-spawn-spinoff",
        "worktree-research",
        "worktree-spinoff",
        "worktree-technical-decision",
    ] {
        let path = skills_dir().join(skill).join("SKILL.template.md");
        let body = std::fs::read_to_string(&path).expect("read panel workflow");
        let section = markdown_section(&body, "## Tool and sub-workflow failure disclosure")
            .expect("panel workflow disclosure section");
        assert!(section.contains("model"));
        assert!(section.contains("not representative consensus"));
    }
}

#[test]
fn failure_disclosure_command_shapes_match_the_binary() {
    let home = TempDir::new().expect("temp TASKFLEET_HOME");
    validate(
        &[
            "node".into(),
            "report".into(),
            "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
            "n-0001".into(),
            "--from-file".into(),
            "/tmp/report.json".into(),
        ],
        &home,
    )
    .expect("blocked report command shape");
    validate(
        &[
            "run".into(),
            "merge".into(),
            "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
            "--report-file".into(),
            "/tmp/report.json".into(),
        ],
        &home,
    )
    .expect("successful report command shape");
}

fn markdown_section<'a>(body: &'a str, heading: &str) -> Option<&'a str> {
    let start = body.find(heading)? + heading.len();
    let rest = &body[start..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Run `taskfleet <argv…> --help` against the real binary, sandboxed into
/// `home`. `Ok(())` iff the invocation's shape is accepted (exit 0); otherwise
/// `Err` carries the binary's error `code` + `message` for the report.
fn validate(argv: &[String], home: &TempDir) -> Result<(), String> {
    let out = Command::new(env!("CARGO_BIN_EXE_taskfleet"))
        .env("TASKFLEET_HOME", home.path())
        .env("HOME", home.path())
        .args(argv)
        .arg("--help")
        .output()
        .expect("spawn taskfleet");

    if out.status.success() {
        return Ok(());
    }

    // Non-zero: a shape error. Surface the structured envelope when present so
    // the failure message names the exact problem (and code) the agent would
    // hit, falling back to raw stderr for anything unparseable.
    let stderr = String::from_utf8_lossy(&out.stderr);
    let detail = serde_json::from_str::<Value>(stderr.trim())
        .ok()
        .and_then(|v| {
            let err = v.get("error")?;
            let code = err.get("code").and_then(Value::as_str).unwrap_or("error");
            let msg = err.get("message").and_then(Value::as_str).unwrap_or("");
            Some(format!("{code}: {msg}"))
        })
        .unwrap_or_else(|| stderr.trim().to_string());
    Err(detail)
}

/// `crates/taskfleet/skills`, resolved from the crate manifest dir so the test
/// is independent of the working directory `cargo test` is invoked from.
fn skills_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("skills")
}

/// Every `SKILL.template.md` under `skills/<name>/`, sorted for stable output.
fn skill_template_paths(skills_dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(skills_dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", skills_dir.display()))
        .flatten()
        .map(|e| e.path().join("SKILL.template.md"))
        .filter(|p| p.is_file())
        .collect();
    paths.sort();
    paths
}

/// Extract every fenced `taskfleet …` command from one SKILL body.
///
/// A command is a line inside a fenced code block whose first non-whitespace
/// token is `taskfleet`, plus any following lines joined onto it via a
/// trailing `\`. Lines outside fences (prose, inline-backtick mentions) are
/// ignored — only fenced blocks are meant to be literal, runnable commands.
fn extract_invocations(skill: &str, body: &str) -> Vec<Invocation> {
    let lines: Vec<&str> = body.lines().collect();
    let mut out = Vec::new();
    let mut in_fence = false;
    // The most recent non-blank line seen *inside the current fence*, used to
    // detect a `# skill-example-ci: skip` marker sitting above a command.
    let mut prev_nonblank: &str = "";
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            prev_nonblank = "";
            i += 1;
            continue;
        }

        if in_fence && line.trim_start().starts_with("taskfleet ") {
            let first_line_no = i + 1; // 1-based
                                       // Join `\`-continuations into a single logical command line.
            let mut joined = String::new();
            loop {
                let cur = lines[i];
                let trimmed_end = cur.trim_end();
                if let Some(stripped) = trimmed_end.strip_suffix('\\') {
                    joined.push_str(stripped.trim_end());
                    joined.push(' ');
                    i += 1;
                    if i >= lines.len() {
                        break;
                    }
                } else {
                    joined.push_str(trimmed_end);
                    i += 1;
                    break;
                }
            }

            let skip = joined.contains(SKIP_MARKER) || prev_nonblank.contains(SKIP_MARKER);
            if !skip {
                if let Some(argv) = to_argv(&joined) {
                    out.push(Invocation {
                        skill: skill.to_string(),
                        line: first_line_no,
                        argv,
                    });
                }
            }
            // `i` already advanced past the whole command; refresh the
            // marker-tracking context to the command line and continue.
            prev_nonblank = "";
            continue;
        }

        if in_fence && !line.trim().is_empty() {
            prev_nonblank = line;
        }
        i += 1;
    }

    out
}

/// Turn a joined command line into the argv after the leading `taskfleet`
/// token, applying the documented normalizations. Returns `None` if the line
/// does not actually start with `taskfleet` (defensive — the caller only
/// passes lines that do).
fn to_argv(command: &str) -> Option<Vec<String>> {
    let mut tokens = shell_split(command).into_iter();
    let head = tokens.next()?;
    if head != "taskfleet" {
        return None;
    }
    let argv: Vec<String> = tokens
        .map(|t| normalize_token(&t))
        .filter(|t| !t.is_empty())
        .collect();
    Some(argv)
}

/// Apply the documentation-convention normalizations to one token:
/// strip optional-flag square brackets, then substitute the placeholders clap
/// would otherwise reject.
fn normalize_token(token: &str) -> String {
    // Strip a single layer of optional-flag brackets: `[--flag` → `--flag`,
    // `<val>]` → `<val>`. A bare `[` or `]` collapses to empty and is dropped.
    let unbracketed = token.trim_start_matches('[').trim_end_matches(']');

    match unbracketed {
        // The only enum-valued placeholder clap validates before `--help`
        // short-circuits. Any real kind works; `fan-out` is arbitrary.
        "<kind>" => "fan-out".to_string(),
        other => other.replace("{{CLI_VERSION}}", env!("CARGO_PKG_VERSION")),
    }
}

/// Minimal POSIX-ish word splitter: whitespace-separated, honouring `'…'` and
/// `"…"` quoting, and treating an unquoted `#` at a token boundary as the start
/// of a line comment (so trailing `# skill-example-ci: skip` and other shell
/// comments are dropped). Sufficient for the literal command forms SKILLs use;
/// it is not a full shell parser.
fn shell_split(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut has_token = false;
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if has_token {
                    tokens.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            '#' if !has_token => break, // start-of-token unquoted `#` → comment
            '\'' => {
                has_token = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    cur.push(q);
                }
            }
            '"' => {
                has_token = true;
                while let Some(q) = chars.next() {
                    match q {
                        '"' => break,
                        '\\' => {
                            // In double quotes only \" and \\ are escapes; keep
                            // everything else (including the backslash) verbatim.
                            match chars.peek() {
                                Some('"' | '\\') => cur.push(chars.next().unwrap()),
                                _ => cur.push('\\'),
                            }
                        }
                        other => cur.push(other),
                    }
                }
            }
            other => {
                has_token = true;
                cur.push(other);
            }
        }
    }
    if has_token {
        tokens.push(cur);
    }
    tokens
}

#[cfg(test)]
mod extractor_tests {
    use super::*;

    #[test]
    fn joins_continuations_and_strips_brackets() {
        let body = "```\ntaskfleet run create \\\n  --kind fan-out \\\n  [--source-branch <branch>]\n```\n";
        let inv = extract_invocations("x", body);
        assert_eq!(inv.len(), 1);
        assert_eq!(
            inv[0].argv,
            vec![
                "run",
                "create",
                "--kind",
                "fan-out",
                "--source-branch",
                "<branch>"
            ]
        );
    }

    #[test]
    fn honours_skip_marker_above_and_inline() {
        let above = "```\n# skill-example-ci: skip\ntaskfleet frobnicate --wat\n```\n";
        assert_eq!(extract_invocations("x", above).len(), 0);
        let inline = "```\ntaskfleet frobnicate --wat # skill-example-ci: skip\n```\n";
        assert_eq!(extract_invocations("x", inline).len(), 0);
    }

    #[test]
    fn ignores_taskfleet_outside_fences() {
        let prose = "Run `taskfleet run list` to see runs.\n";
        assert_eq!(extract_invocations("x", prose).len(), 0);
    }

    #[test]
    fn substitutes_kind_and_version_placeholders() {
        let body = "```\ntaskfleet run create --kind <kind>\n```\n";
        let inv = extract_invocations("x", body);
        assert_eq!(inv[0].argv, vec!["run", "create", "--kind", "fan-out"]);
    }
}
