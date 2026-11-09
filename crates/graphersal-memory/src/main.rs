//! `graphersal-memory`: local graph memory for AI coding agents.
//!
//! Pre-release: `install`, `uninstall`, `status`, `doctor`, `import` (also `--sessions`) and
//! `summaries` set graphersal-memory up in Claude Code, `mcp` serves the memory over stdio (through the shared daemon, or in this
//! process with `--no-daemon`), `daemon` manages the daemon; every other command reports the
//! roadmap stage that delivers it and exits with code 2.

mod catalog;
mod cli;
mod client;
mod commands;
mod config;
mod daemon;
mod engine;
mod fsutil;
mod harness;
mod hook;
mod import;
mod install;
#[cfg(test)]
mod install_tests;
mod ipc;
mod jsonedit;
mod mcp;
#[cfg(test)]
mod mcp_tests;
mod projects;
mod protocol;
mod registry;
mod sessions;
mod shim;
mod store;
mod summaries;
mod transcript;

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
