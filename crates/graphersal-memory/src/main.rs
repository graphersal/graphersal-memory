//! `graphersal-memory`: local graph memory for AI coding agents.
//!
//! `install` (a wizard in a terminal), `uninstall`, `status`, `doctor`, `import` (also
//! `--sessions`), `export --to-files`, `projects` and `summaries` set graphersal-memory up in
//! Claude Code, opencode, Codex and Cursor; `mcp` serves the memory over stdio (through the shared
//! daemon, or in this process with `--no-daemon`), `daemon` manages the daemon, `ui` opens the
//! browser UI and `bench` measures it. `export` without `--to-files` (the export of rules into
//! instruction files) is planned and exits with code 2.

mod bench;
mod cli;
mod commands;
mod config;
mod fsutil;
mod harnesses;
mod install;
mod memory;
mod runtime;
mod ui;

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
