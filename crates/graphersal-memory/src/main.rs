//! `graphersal-memory`: local graph memory for AI coding agents.
//!
//! Pre-release skeleton: the command set and its help text exist; every command reports the
//! roadmap stage that delivers it and exits with code 2.

mod cli;
mod commands;
mod engine;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    // clap prints help/version to stdout (exit 0) and usage errors to stderr (exit 2).
    let cli = cli::Cli::parse();
    ExitCode::from(commands::run(&cli.command, &mut std::io::stderr()))
}
