//! `graphersal-memory`: local graph memory for AI coding agents.
//!
//! Pre-release: `mcp` serves the memory over stdio (through the shared daemon, or in this
//! process with `--no-daemon`), `daemon` and `status` manage the daemon; every other command
//! reports the roadmap stage that delivers it and exits with code 2.

mod catalog;
mod cli;
mod commands;
mod daemon;
mod engine;
mod ipc;
mod mcp;
#[cfg(test)]
mod mcp_tests;
mod protocol;
mod registry;
mod shim;
mod store;

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
