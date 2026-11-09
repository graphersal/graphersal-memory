//! Running a command. In this pre-release every command reports which stage delivers it.

use crate::cli::Command;

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
        Command::Mcp { .. } => ("M1", "memory core served over stdio"),
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
    format!(
        "graphersal-memory {}: not implemented yet (planned in stage {stage}: {what}).\n\
         graphersal-memory is a pre-release; see docs/roadmap.md at \
         https://github.com/graphersal/graphersal-memory",
        command_name(command)
    )
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
            ("mcp", "M1"),
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
}
