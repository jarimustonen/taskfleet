//! Read-only worker display discovery and deliberate native terminal attachment.
//!
//! Names, cwd and socket conventions carry no authority. Discovery reads the
//! manifest and nodes under `LOCK_SH`; execution revalidates the recorded server
//! generation and exact pane through the same checks used by retention. No
//! server creation, input injection, supervisor recovery or worker cleanup lives here.
//! Native viewing is outside tmux only: switching a caller's client would risk
//! changing an unrelated server/session. Machine output is a validated descriptor,
//! never terminal bytes. The final tmux command also guards pid/window/pane on its
//! own connection, closing the probe-to-connect server replacement window.
//! An ephemeral session links only the owned window (no session grouping, which
//! could activate the source's destroy-unattached policy). active-pane gives this
//! client its own selected pane; -Z preserves the shared zoom. A target hidden by
//! another pane's zoom is refused rather than unzooming somebody else's window.
//! destroy-unattached is enabled after create-and-attach, and a tmux wait-for
//! signal confirms setup: post-attach errors must not look like successful views.

use std::io::IsTerminal;
use std::process::Command;
use std::time::Duration;

use serde::Serialize;
use taskfleet_core::{Manifest, Node, RunLock, RunPaths, TmuxIdentity};

use crate::error::CliError;
use crate::output::{self, OutputFormat, OutputSpec};

#[derive(Debug, Serialize)]
pub(crate) struct Attachment {
    /// recorded is discovery only; validated is a point-in-time tmux probe.
    pub state: &'static str,
    pub reason: Option<&'static str>,
    pub node_id: Option<String>,
    pub display: Option<&'static str>,
    pub identity: Option<TmuxIdentity>,
    pub command: Option<String>,
    #[serde(skip)]
    retained: Option<taskfleet_core::RetainedDisplay>,
}

impl Attachment {
    pub(crate) fn not_created() -> Self {
        Self::unavailable("not-created-dry-run")
    }
    fn unavailable(reason: &'static str) -> Self {
        Self {
            state: "unavailable",
            reason: Some(reason),
            node_id: None,
            display: None,
            identity: None,
            command: None,
            retained: None,
        }
    }
}

fn id_valid(value: &str, prefix: char) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
}

pub(crate) fn discover(paths: &RunPaths, manifest: &Manifest, nodes: &[Node]) -> Attachment {
    if manifest.node_count != 1 || nodes.len() != 1 {
        return Attachment::unavailable("no-unique-node-display");
    }
    let node = &nodes[0];
    let (identity, display) = if let Some(retained) = &node.retained_display {
        if retained.expired_at.is_some() {
            return Attachment::unavailable("display-cleaned");
        }
        if retained.attempt != node.retry_attempts
            || retained.ownership_marker
                != format!(
                    "taskfleet:{}:{}:{}",
                    paths.run_id, node.node_id, node.retry_attempts
                )
        {
            return Attachment::unavailable("invalid-retained-identity");
        }
        (
            TmuxIdentity {
                socket: Some(retained.socket.clone()),
                session: retained.session.clone(),
                window_id: retained.window_id.clone(),
                pane_id: Some(retained.pane_id.clone()),
                server_pid: Some(retained.server_pid),
                server_pid_start_secs: Some(retained.server_pid_start_secs),
                server_marker: Some(retained.server_marker.clone()),
            },
            "retained",
        )
    } else if let Some(identity) = &node.tmux_identity {
        ((**identity).clone(), "worker")
    } else {
        return Attachment::unavailable("display-not-recorded");
    };
    if !identity
        .socket
        .as_deref()
        .is_some_and(|s| std::path::Path::new(s).is_absolute())
        || identity.session.is_empty()
        || !id_valid(&identity.window_id, '@')
        || !identity
            .pane_id
            .as_deref()
            .is_some_and(|s| id_valid(s, '%'))
        || identity.server_pid.is_none_or(|p| p == 0)
        || identity.server_pid_start_secs.is_none()
    {
        return Attachment::unavailable("incomplete-display-identity");
    }
    // Re-enter the same executable and state home, not a PATH version or a raw
    // tmux command that would bypass validation on later copy/paste.
    let Ok(exe) = std::env::current_exe() else {
        return Attachment::unavailable("executable-unavailable");
    };
    let Some(home) = paths.root.parent().and_then(std::path::Path::parent) else {
        return Attachment::unavailable("state-home-unavailable");
    };
    let command = format!(
        "TASKFLEET_HOME={} {} run attach {}",
        quote(&home.to_string_lossy()),
        quote(&exe.to_string_lossy()),
        quote(paths.run_id.as_str())
    );
    Attachment {
        state: "recorded",
        reason: None,
        node_id: Some(node.node_id.to_string()),
        display: Some(display),
        identity: Some(identity),
        command: Some(command),
        retained: node.retained_display.as_deref().cloned(),
    }
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Safe for create replays as well as new creates; no tmux calls or mutations.
pub(crate) fn read(paths: &RunPaths) -> Result<Attachment, CliError> {
    RunLock::with_shared_lock(&paths.lock(), || {
        let manifest = taskfleet_core::read_manifest_opt(paths)?;
        let Some(manifest) = manifest else {
            return Ok(Attachment::unavailable("run-not-found"));
        };
        let nodes = super::retained::read_nodes(paths, super::retained::MissingNodesDir::Empty)?;
        Ok(discover(paths, &manifest, &nodes))
    })
    .map_err(super::from_core)
}

pub fn run(run_id: &str, spec: &OutputSpec, warnings: &[String]) -> Result<(), CliError> {
    let id = super::parse_run_id(run_id)?; // Full identity only, never prefix lookup.
    let root = crate::home::root_dir()?;
    let paths = super::run_paths_exact(&root, &id)?;
    let mut attachment = read(&paths)?;
    if attachment.state == "unavailable" {
        return Err(CliError::user(
            "display_unavailable",
            format!("run {run_id}: {}", attachment.reason.unwrap_or("unknown")),
        ));
    }
    let native = spec.format == OutputFormat::Text;
    if native && std::env::var_os("TMUX").is_some() {
        return Err(CliError::user("attach_inside_tmux", "Open a terminal outside tmux and run the attach command there; use --json here to validate/discover without switching your client."));
    }
    // Reuse the retention layer's exact ownership checks, including OS process
    // start identity. No fallback to a default socket or a window's active pane.
    crate::session::validate_attachment(
        attachment.identity.as_ref().expect("discovered identity"),
        attachment.retained.as_ref(),
    )
    .map_err(|e| e.with_invalid_value(run_id))?;
    attachment.state = "validated";
    if !native {
        return output::emit_envelope(&attachment, spec, warnings);
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(CliError::user(
            "attach_requires_terminal",
            "Native attach requires terminal stdin/stdout; use --json for a validated descriptor.",
        ));
    }
    let identity = attachment.identity.as_ref().expect("discovered identity");
    let viewer = format!("taskfleet-view-{}", uuid::Uuid::new_v4().simple());
    let mut client = native_command(identity, &viewer)
        .spawn()
        .map_err(|e| CliError::system("tmux_attach_failed", format!("start tmux viewer: {e}")))?;
    let mut ready = Command::new(crate::multiplexer::tmux::tmux_bin());
    ready.env_remove("TMUX").args([
        "-N",
        "-S",
        identity.socket.as_deref().expect("validated socket"),
        "wait-for",
        &viewer,
    ]);
    if !matches!(crate::proc::run_with_timeout(ready, Duration::from_secs(5), 8192),
        crate::proc::TimedOutcome::Exited { status, .. } if status.success())
    {
        // This is our own command client, not a worker PID or another user's
        // tmux client. Closing it expires only its destroy-unattached view.
        let _ = client.kill();
        let _ = client.wait();
        return Err(CliError::user("tmux_attach_failed", format!("run {run_id}: viewer setup was not confirmed; display changed, target is hidden by another zoomed pane, or tmux setup failed. No worker or other client was changed.")));
    }
    let status = client.wait().map_err(|e| {
        CliError::system("tmux_attach_failed", format!("wait for tmux viewer: {e}"))
    })?;
    if !status.success() {
        return Err(CliError::user(
            "tmux_attach_failed",
            format!("tmux viewer exited {status}; inspect run {run_id} with --json"),
        ));
    }
    output::emit_text_warnings(warnings);
    Ok(())
}

fn native_command(identity: &TmuxIdentity, viewer: &str) -> Command {
    let pane = identity.pane_id.as_deref().expect("validated pane");
    let pid = identity.server_pid.expect("validated pid");
    let guard = format!("#{{&&:#{{==:#{{pid}},{pid}}},#{{&&:#{{==:#{{window_id}},{}}},#{{&&:#{{==:#{{pane_id}},{pane}}},#{{||:#{{==:#{{window_zoomed_flag}},0}},#{{==:#{{pane_active}},1}}}}}}}}}}", identity.window_id);
    let mut command = Command::new(crate::multiplexer::tmux::tmux_bin());
    // Empty-command new-session creates an inert placeholder, not a shell.
    // link-window replaces only that placeholder with the recorded owned
    // window. It does NOT group sessions or change any source selection.
    let view = format!(
        "new-session -E -s {viewer} -f read-only,ignore-size,active-pane '' ; set-option -t {viewer} destroy-unattached on ; link-window -k -s {} -t ={viewer}: ; select-pane -Z -t {pane} ; wait-for -S {viewer}",
        identity.window_id
    );
    command.env_remove("TMUX").args([
        "-N",
        "-S",
        identity.socket.as_deref().expect("validated socket"),
        "if-shell",
        "-F",
        "-t",
        pane,
        &guard,
        &view,
        // Session IDs are u32. This read-only runtime error (not an unknown
        // command parse error) prints the useful message, then fails closed.
        "display-message -p 'Taskfleet display changed or target hidden by zoom'; has-session -t '$4294967296'",
    ]);
    command
}
