//! `graphersal-memory`: local graph memory for AI coding agents.
//!
//! Pre-release: `install` (a wizard in a terminal), `uninstall`, `status`, `doctor`, `import`
//! (also `--sessions`), `export --to-files`, `projects` and `summaries` set graphersal-memory up
//! in Claude Code, opencode, Codex and Cursor, `mcp` serves the memory over stdio (through the shared daemon, or in this
//! process with `--no-daemon`), `daemon` manages the daemon; every other command reports the
//! roadmap stage that delivers it and exits with code 2.

mod agents;
mod bench;
#[cfg(test)]
mod bench_tests;
mod catalog;
mod cli;
mod client;
mod codex;
mod coexist;
#[cfg(test)]
mod coexist_tests;
mod commands;
mod config;
mod cursor;
mod daemon;
mod download;
mod engine;
mod export;
mod fsutil;
mod harness;
mod hook;
mod import;
mod install;
#[cfg(test)]
mod install_tests;
mod ipc;
mod jsonedit;
mod language;
mod maintenance;
#[cfg(test)]
mod maintenance_tests;
mod mcp;
#[cfg(test)]
mod mcp_tests;
#[cfg(test)]
mod model_tests;
mod models;
mod opencode;
mod projects;
mod prompt;
mod protocol;
mod registry;
mod session_sync;
mod sessions;
mod shim;
mod start_context;
mod store;
mod summaries;
mod tomledit;
mod transcript;
mod winshell;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    // clap prints help/version to stdout (exit 0) and usage errors to stderr (exit 2).
    let cli = cli::Cli::parse();
    ExitCode::from(commands::run_cli(
        &cli,
        std::io::stdin(),
        std::io::stdout(),
        &mut std::io::stderr(),
    ))
}
