//! Running a command. In this pre-release `mcp` (the shim, or `--no-daemon`), `daemon` and
//! the daemon part of `status` work; every other command reports which stage delivers it.

use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use crate::cli::{Cli, Command, Profile};
use crate::daemon::{self, DaemonOptions};
use crate::shim::{self, DaemonLocation, ShimOptions};
use crate::{ipc, mcp, protocol, store};

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
        Command::Mcp { .. } | Command::Daemon { .. } => {
            ("M2", "the shared daemon and the stdio shim")
        }
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
        Command::Daemon { .. } => "daemon",
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

/// Runs the command line: `mcp` serves MCP on `stdin`/`stdout` (through the daemon, or in this
/// process with `--no-daemon`), `daemon` runs or starts the daemon, `status` reports it, every
/// other command reports its stage. Diagnostics go to `stderr`; returns the process exit code.
pub fn run_cli(
    cli: &Cli,
    stdin: impl Read + Send + 'static,
    stdout: impl Write + Send + 'static,
    stderr: &mut dyn Write,
) -> u8 {
    let name = command_name(&cli.command);
    let result = match &cli.command {
        Command::Mcp {
            profile,
            no_daemon: true,
            project,
        } => serve_options(cli, *profile, project.as_deref())
            .and_then(|options| mcp::serve(&options, BufReader::new(stdin), stdout)),
        Command::Mcp {
            profile,
            no_daemon: false,
            project,
        } => shim_options(cli, *profile, project.as_deref())
            .and_then(|options| shim::run(&options, stdin, stdout)),
        Command::Daemon {
            idle,
            foreground,
            version_as,
        } => run_daemon(cli, *idle, *foreground, version_as.as_deref(), stdout),
        Command::Status => status(cli, stdout),
        command => return run(command, stderr),
    };
    match result {
        Ok(()) => 0,
        Err(err) => {
            let _ = writeln!(stderr, "graphersal-memory {name}: {err}");
            EXIT_FAILED
        }
    }
}

/// The data directory (flag, environment, platform) and the run directory of `cli`.
fn dirs(cli: &Cli) -> Result<(PathBuf, PathBuf), String> {
    let env = |name: &str| std::env::var(name).ok();
    let platform = store::Platform::current();
    let data_dir = store::resolve_data_dir(cli.data_dir.as_deref(), &env, platform)?;
    let run_dir = ipc::resolve_run_dir(
        cli.run_dir.as_deref(),
        cli.data_dir.as_deref(),
        &data_dir,
        &env,
        platform,
    )?;
    Ok((data_dir, run_dir))
}

/// The project of this process: the flag, else the git root of the current directory.
fn project(flag: Option<&Path>) -> Result<String, String> {
    let cwd = std::env::current_dir()
        .map_err(|err| format!("cannot read the current directory: {err}"))?;
    Ok(mcp::project_key(flag, &cwd))
}

/// What `mcp --no-daemon` serves.
fn serve_options(
    cli: &Cli,
    profile: Profile,
    project_flag: Option<&Path>,
) -> Result<mcp::ServeOptions, String> {
    let (data_dir, _) = dirs(cli)?;
    Ok(mcp::ServeOptions {
        data_dir,
        profile,
        project: project(project_flag)?,
    })
}

/// Where the daemon of `cli` is and how to start it.
fn location(cli: &Cli) -> Result<DaemonLocation, String> {
    let (data_dir, run_dir) = dirs(cli)?;
    let exe = std::env::current_exe()
        .map_err(|err| format!("cannot find this program's path to start the daemon: {err}"))?;
    Ok(DaemonLocation {
        data_dir,
        run_dir,
        exe,
        start_timeout: shim::START_TIMEOUT,
    })
}

/// What the shim connects as.
fn shim_options(
    cli: &Cli,
    profile: Profile,
    project_flag: Option<&Path>,
) -> Result<ShimOptions, String> {
    Ok(ShimOptions {
        location: location(cli)?,
        profile,
        project: project(project_flag)?,
        harness: shim::harness_name(&|name| std::env::var(name).ok()).to_string(),
    })
}

/// `daemon`: in this process with `--foreground`, else started detached (or found running).
fn run_daemon(
    cli: &Cli,
    idle: Option<std::time::Duration>,
    foreground: bool,
    version_as: Option<&str>,
    mut stdout: impl Write,
) -> Result<(), String> {
    if foreground {
        let (data_dir, run_dir) = dirs(cli)?;
        let idle = match idle {
            Some(idle) => idle,
            None => match std::env::var(daemon::IDLE_ENV) {
                Ok(text) if !text.is_empty() => daemon::parse_duration(&text)
                    .map_err(|err| format!("{}: {err}", daemon::IDLE_ENV))?,
                _ => daemon::DEFAULT_IDLE,
            },
        };
        return daemon::run(DaemonOptions {
            data_dir,
            run_dir,
            idle,
            version: version_as.unwrap_or(protocol::VERSION).to_string(),
        });
    }
    if idle.is_some() || version_as.is_some() {
        return Err(format!(
            "--idle and --version-as need --foreground (a detached daemon reads {})",
            daemon::IDLE_ENV
        ));
    }
    let location = location(cli)?;
    drop(shim::connect_or_start(&location)?);
    let status = shim::control(&location.run_dir, "status")?
        .ok_or_else(|| "the daemon stopped right after its start".to_string())?;
    let _ = writeln!(stdout, "{}", describe(&status));
    Ok(())
}

/// One line about a running daemon.
fn describe(status: &serde_json::Value) -> String {
    let stores: Vec<&str> = status["stores"]
        .as_array()
        .map(|stores| stores.iter().filter_map(|s| s.as_str()).collect())
        .unwrap_or_default();
    format!(
        "daemon: running (pid {}, version {}, up {}s, {} connection(s), stores open: {}, run \
         directory {}){}",
        status["pid"],
        status["daemon_version"].as_str().unwrap_or("?"),
        status["uptime_secs"],
        status["connections"],
        if stores.is_empty() {
            "none".to_string()
        } else {
            stores.join(", ")
        },
        status["run_dir"].as_str().unwrap_or("?"),
        if status["draining"] == serde_json::Value::Bool(true) {
            ", draining for an upgrade"
        } else {
            ""
        }
    )
}

/// `status`: whether the daemon runs; the installation part arrives in M3.
fn status(cli: &Cli, mut stdout: impl Write) -> Result<(), String> {
    let (data_dir, run_dir) = dirs(cli)?;
    let daemon = match shim::control(&run_dir, "status")? {
        Some(status) => describe(&status),
        None => format!(
            "daemon: not running (run directory {}; the next agent session starts it)",
            run_dir.display()
        ),
    };
    let (stage, _) = planned_stage(&Command::Status);
    let _ = writeln!(
        stdout,
        "{daemon}\ndata directory: {}\ninstallation: not checked yet (planned in stage {stage})",
        data_dir.display()
    );
    Ok(())
}

/// Runs a command that is not implemented yet: writes its stage to `stderr` and returns the
/// process exit code.
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
            ("doctor", "M3"),
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
    fn status_without_a_daemon_says_so() {
        let data = crate::mcp_tests::TempDir::new("st");
        let data_dir = data.path().display().to_string();
        let cli =
            Cli::try_parse_from(["graphersal-memory", "status", "--data-dir", &data_dir]).unwrap();
        let out = SharedBuffer::default();
        let mut err = Vec::new();
        assert_eq!(
            run_cli(&cli, std::io::empty(), out.clone(), &mut err),
            0,
            "{}",
            String::from_utf8_lossy(&err)
        );
        let text = out.text();
        assert!(text.contains("daemon: not running"), "{text}");
        assert!(text.contains(&data_dir), "{text}");
        assert!(text.contains("stage M3"), "{text}");
    }

    #[test]
    fn a_detached_daemon_refuses_foreground_options() {
        let cli = Cli::try_parse_from([
            "graphersal-memory",
            "daemon",
            "--idle",
            "1s",
            "--data-dir",
            "/x",
        ])
        .unwrap();
        let mut err = Vec::new();
        assert_eq!(
            run_cli(&cli, std::io::empty(), Vec::new(), &mut err),
            EXIT_FAILED
        );
        let text = String::from_utf8(err).unwrap();
        assert!(text.contains("--foreground"), "{text}");
    }

    /// An output buffer the test keeps a handle of.
    #[derive(Clone, Default)]
    struct SharedBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl SharedBuffer {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for SharedBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
