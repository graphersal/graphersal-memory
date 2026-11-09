//! Running a command. In this pre-release only `mcp --no-daemon` works; every other command
//! reports which stage delivers it.

use std::io::{BufRead, Write};

use crate::cli::{Cli, Command};
use crate::{mcp, store};

/// Exit code of a command that failed.
pub const EXIT_FAILED: u8 = 1;

/// Exit code of a command that is not implemented yet (the same code as invalid usage: the
/// invocation cannot be served).
pub const EXIT_NOT_IMPLEMENTED: u8 = 2;

/// The roadmap stage that delivers a command, and what that stage is about.
pub fn planned_stage(command: &Command) -> (&'static str, &'static str) {
    match command {
        Command::Install { .. } | Command::Uninstall { .. } => {
            ("M3", "one-click install for Claude Code")
        }
        Command::Status | Command::Doctor => ("M3", "one-click install for Claude Code"),
        Command::Import { .. } | Command::Export { .. } => {
            ("M3", "import of the file memory and rule export")
        }
        Command::Mcp { .. } => ("M2", "the shared daemon and the stdio shim"),
        Command::Daemon => ("M2", "the shared daemon and the stdio shim"),
        Command::Bench { .. } => ("M5", "the built-in A/B"),
        Command::Ui => ("M7", "the browser UI for develop mode"),
    }
}

/// The command's name as typed on the command line.
pub fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Install { .. } => "install",
        Command::Uninstall { .. } => "uninstall",
        Command::Status => "status",
        Command::Doctor => "doctor",
        Command::Mcp { .. } => "mcp",
        Command::Daemon => "daemon",
        Command::Ui => "ui",
        Command::Bench { .. } => "bench",
        Command::Import { .. } => "import",
        Command::Export { .. } => "export",
    }
}

/// The message printed to stderr for a command that is not implemented yet.
pub fn not_implemented_message(command: &Command) -> String {
    let (stage, what) = planned_stage(command);
    let hint = match command {
        Command::Mcp { .. } => {
            "\nTo serve the memory in this process (one session at a time), add --no-daemon."
        }
        _ => "",
    };
    format!(
        "graphersal-memory {}: not implemented yet (planned in stage {stage}: {what}).{hint}\n\
         graphersal-memory is a pre-release; see docs/roadmap.md at \
         https://github.com/graphersal/graphersal-memory",
        command_name(command)
    )
}

/// Runs the command line: `mcp --no-daemon` serves MCP on `stdin`/`stdout`, every other
/// command reports its stage. Diagnostics go to `stderr`; returns the process exit code.
pub fn run_cli(
    cli: &Cli,
    stdin: impl BufRead,
    stdout: impl Write + Send,
    stderr: &mut dyn Write,
) -> u8 {
    match &cli.command {
        Command::Mcp {
            profile,
            no_daemon: true,
            project,
        } => {
            let served = serve_options(cli.data_dir.as_deref(), *profile, project.as_deref())
                .and_then(|options| mcp::serve(&options, stdin, stdout));
            match served {
                Ok(()) => 0,
                Err(err) => {
                    let _ = writeln!(stderr, "graphersal-memory mcp: {err}");
                    EXIT_FAILED
                }
            }
        }
        command => run(command, stderr),
    }
}

/// What `mcp --no-daemon` serves: the data directory (flag, environment, platform) and the
/// project (flag, else the git root of the current directory).
fn serve_options(
    data_dir: Option<&std::path::Path>,
    profile: crate::cli::Profile,
    project: Option<&std::path::Path>,
) -> Result<mcp::ServeOptions, String> {
    let data_dir = store::resolve_data_dir(
        data_dir,
        &|name| std::env::var(name).ok(),
        store::Platform::current(),
    )?;
    let cwd = std::env::current_dir()
        .map_err(|err| format!("cannot read the current directory: {err}"))?;
    Ok(mcp::ServeOptions {
        data_dir,
        profile,
        project: mcp::project_key(project, &cwd),
    })
}

/// Runs a command: writes its diagnostics to `stderr` and returns the process exit code.
pub fn run(command: &Command, stderr: &mut dyn std::io::Write) -> u8 {
    // A closed or failing stderr must not turn into a panic; the exit code still tells the story.
    let _ = writeln!(stderr, "{}", not_implemented_message(command));
    EXIT_NOT_IMPLEMENTED
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    #[test]
    fn every_command_reports_its_stage_and_exits_2() {
        for (name, stage) in [
            ("install", "M3"),
            ("uninstall", "M3"),
            ("status", "M3"),
            ("doctor", "M3"),
            ("mcp", "M2"),
            ("daemon", "M2"),
            ("ui", "M7"),
            ("bench", "M5"),
            ("import", "M3"),
            ("export", "M3"),
        ] {
            let cli = Cli::try_parse_from(["graphersal-memory", name]).unwrap();
            let mut err = Vec::new();
            assert_eq!(run(&cli.command, &mut err), 2);
            let text = String::from_utf8(err).unwrap();
            assert!(
                text.starts_with(&format!("graphersal-memory {name}: not implemented yet")),
                "{text}"
            );
            assert!(text.contains(&format!("stage {stage}")), "{text}");
        }
    }

    #[test]
    fn mcp_without_no_daemon_names_the_flag() {
        let cli = Cli::try_parse_from(["graphersal-memory", "mcp"]).unwrap();
        let mut err = Vec::new();
        let code = run_cli(&cli, std::io::empty(), Vec::new(), &mut err);
        assert_eq!(code, EXIT_NOT_IMPLEMENTED);
        let text = String::from_utf8(err).unwrap();
        assert!(text.contains("planned in stage M2"), "{text}");
        assert!(text.contains("--no-daemon"), "{text}");
    }
}
