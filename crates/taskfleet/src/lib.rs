//! The sole Taskfleet command-line engine.

mod cli;
// User-facing configuration file (`~/.taskfleet/config.toml`), the `file`
// layer of the flag > env > file > default precedence (AGENTS-AI-FIRST-CLI §8).
mod config;
mod doctor;
mod error;
mod event;
mod git;
mod harness;
mod help;
mod home;
mod idempotency;
mod multiplexer;
mod node;
mod output;
mod proc;
mod run;
mod run_worker;
mod self_exec;
mod session;
mod skill;
mod supervise;
mod worker_handshake;

use std::process::ExitCode;

/// Dispatch the current process through the Taskfleet parser and engine.
pub fn dispatch() -> ExitCode {
    cli::run()
}
