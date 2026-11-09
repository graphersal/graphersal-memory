//! `graphersal-memory`: local graph memory for AI coding agents.
//!
//! Pre-release: `mcp --no-daemon` serves the memory over stdio; every other command reports
//! the roadmap stage that delivers it and exits with code 2.

mod catalog;
mod cli;
mod commands;
mod engine;
mod mcp;
#[cfg(test)]
mod mcp_tests;
mod store;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    // clap prints help/version to stdout (exit 0) and usage errors to stderr (exit 2).
    let cli = cli::Cli::parse();
    ExitCode::from(commands::run_cli(
        &cli,
        std::io::stdin().lock(),
        std::io::stdout(),
        &mut std::io::stderr(),
    ))
}
