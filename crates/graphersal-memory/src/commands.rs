//! Running a command. In this pre-release `install`, `uninstall`, `status`, `doctor` and
//! `import` (Claude Code), `mcp` (the shim, or `--no-daemon`), `daemon` and the hidden `hook`
//! work; every other command reports which stage delivers it.

use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cli::{Cli, Command, Harness, Profile};
use crate::config::{self, Config};
use crate::daemon::{self, DaemonOptions};
use crate::import::{self, Caller, ImportState};
use crate::install::{self, ClaudePaths, InstallOptions, Setup, UninstallOptions};
use crate::shim::{self, DaemonLocation, ShimOptions};
use crate::{client, hook, ipc, mcp, protocol, store};

/// How long the product's own memory calls (import) wait for one answer.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

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
        Command::Status | Command::Doctor | Command::Hook { .. } => {
            ("M3", "one-click install for Claude Code")
        }
        Command::Import { .. } => ("M3", "import of the file memory"),
        Command::Export { .. } => (
            "M7",
            "the export of rules kept in the graph into the harness's instruction files",
        ),
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
        Command::Hook { .. } => "hook",
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

/// The message for a harness that is not supported yet.
fn harness_not_implemented(command: &str, harness: Harness) -> String {
    let name = match harness {
        Harness::ClaudeCode => "claude-code",
        Harness::Opencode => "opencode",
        Harness::Codex => "codex",
        Harness::Cursor => "cursor",
    };
    format!(
        "graphersal-memory {command} --harness {name}: not implemented yet (planned in stage \
         M7: other harnesses); Claude Code is supported."
    )
}

/// Runs the command line: `mcp` serves MCP on `stdin`/`stdout` (through the daemon, or in this
/// process with `--no-daemon`), `daemon` runs or starts the daemon, `install`, `uninstall`,
/// `status`, `doctor`, `import` and `hook` work on Claude Code, every other command reports its
/// stage. Diagnostics go to `stderr`; returns the process exit code.
pub fn run_cli(
    cli: &Cli,
    stdin: impl Read + Send + 'static,
    stdout: impl Write + Send + 'static,
    stderr: &mut dyn Write,
) -> u8 {
    let name = command_name(&cli.command);
    let mut stdout = stdout;
    let result: Result<u8, String> = match &cli.command {
        Command::Install {
            harness,
            dry_run,
            yes,
            with_model,
            enable_develop,
        } => {
            if *harness != Harness::ClaudeCode {
                let _ = writeln!(stderr, "{}", harness_not_implemented(name, *harness));
                return EXIT_NOT_IMPLEMENTED;
            }
            let options = InstallOptions {
                dry_run: *dry_run,
                yes: *yes,
                with_model: *with_model,
                enable_develop: *enable_develop,
            };
            setup(cli).and_then(|setup| {
                let mut connect = daemon_connector(&setup);
                install::install(
                    &setup,
                    &options,
                    &mut connect,
                    &mut BufReader::new(stdin),
                    &mut stdout,
                )
            })
        }
        Command::Uninstall {
            harness,
            dry_run,
            yes,
            purge,
            disable_develop,
        } => {
            if *harness != Harness::ClaudeCode {
                let _ = writeln!(stderr, "{}", harness_not_implemented(name, *harness));
                return EXIT_NOT_IMPLEMENTED;
            }
            let options = UninstallOptions {
                dry_run: *dry_run,
                yes: *yes,
                purge: *purge,
                disable_develop_only: *disable_develop,
            };
            setup(cli).and_then(|setup| {
                install::uninstall(&setup, &options, &mut BufReader::new(stdin), &mut stdout)
            })
        }
        Command::Doctor => setup(cli).map(|setup| doctor(&setup, &mut stdout)),
        Command::Import { dry_run } => setup(cli).and_then(|setup| {
            let mut connect = daemon_connector(&setup);
            run_import(&setup, *dry_run, &mut connect, &mut stdout)
        }),
        Command::Hook { event } => return run_hook(cli, *event, stdin, &mut stdout, stderr),
        Command::Mcp {
            profile,
            no_daemon: true,
            project,
        } => serve_options(cli, *profile, project.as_deref())
            .and_then(|options| mcp::serve(&options, BufReader::new(stdin), stdout))
            .map(|()| 0),
        Command::Mcp {
            profile,
            no_daemon: false,
            project,
        } => shim_options(cli, *profile, project.as_deref())
            .and_then(|options| shim::run(&options, stdin, stdout))
            .map(|()| 0),
        Command::Daemon {
            idle,
            foreground,
            version_as,
        } => run_daemon(cli, *idle, *foreground, version_as.as_deref(), stdout).map(|()| 0),
        Command::Status => status(cli, stdout).map(|()| 0),
        command @ (Command::Ui | Command::Bench { .. } | Command::Export { .. }) => {
            return run(command, stderr);
        }
    };
    match result {
        Ok(code) => code,
        Err(err) => {
            let _ = writeln!(stderr, "graphersal-memory {name}: {err}");
            EXIT_FAILED
        }
    }
}

/// `path` made absolute against the current directory (not canonicalized: the directory may
/// not exist yet, and the entries install writes must not change once it does).
fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

/// What install, uninstall, status, doctor, import and the hooks work on.
fn setup(cli: &Cli) -> Result<Setup, String> {
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let (data_dir, run_dir) = dirs(cli)?;
    let paths = ClaudePaths::resolve(&env)?;
    let exe =
        std::env::current_exe().map_err(|err| format!("cannot find this program's path: {err}"))?;
    let mut dir_args = Vec::new();
    if cli.data_dir.is_some() || env(store::DATA_DIR_ENV).is_some() {
        dir_args.push("--data-dir".to_string());
        dir_args.push(absolute(&data_dir).display().to_string());
    }
    if cli.run_dir.is_some() || env(ipc::RUN_DIR_ENV).is_some() {
        dir_args.push("--run-dir".to_string());
        dir_args.push(absolute(&run_dir).display().to_string());
    }
    Ok(Setup {
        paths,
        data_dir,
        run_dir,
        exe: mcp::canonical(&exe),
        dir_args,
    })
}

/// Connections of the product's own calls: through the daemon, `use` profile.
fn daemon_connector(
    setup: &Setup,
) -> impl FnMut(&str) -> Result<Box<dyn Caller>, String> + Send + 'static {
    let location = DaemonLocation {
        data_dir: setup.data_dir.clone(),
        run_dir: setup.run_dir.clone(),
        exe: setup.exe.clone(),
        start_timeout: shim::START_TIMEOUT,
    };
    move |project: &str| -> Result<Box<dyn Caller>, String> {
        Ok(Box::new(client::connect(&location, project, CALL_TIMEOUT)?))
    }
}

/// `import`: the file memory of every project, new and changed files only.
pub fn run_import(
    setup: &Setup,
    dry_run: bool,
    connect: &mut import::Connect<'_>,
    out: &mut dyn Write,
) -> Result<u8, String> {
    let state = ImportState::load(&setup.data_dir)?;
    let plans = import::plan(import::scan(&setup.paths.projects)?, &state);
    let (with_project, skipped, new, changed, unchanged) = import::totals(&plans);
    let _ = writeln!(
        out,
        "{} memory director{} under {}: {new} new, {changed} changed, {unchanged} unchanged \
         memory files{}",
        with_project + skipped,
        if with_project + skipped == 1 {
            "y"
        } else {
            "ies"
        },
        setup.paths.projects.display(),
        if skipped > 0 {
            format!("; {skipped} skipped (the project's directory no longer exists)")
        } else {
            String::new()
        }
    );
    if dry_run {
        for plan in &plans {
            let project = plan
                .dir
                .project
                .as_deref()
                .unwrap_or("(skipped: not found)");
            let _ = writeln!(
                out,
                "  {} -> {project}: {} new, {} changed, {} unchanged",
                plan.dir.name,
                plan.new.len(),
                plan.changed.len(),
                plan.unchanged
            );
        }
        let _ = writeln!(out, "Dry run: nothing was imported.");
        return Ok(0);
    }
    if new + changed == 0 {
        let _ = writeln!(out, "Nothing to import.");
        return Ok(0);
    }
    let report = import::with_lock(&setup.data_dir, Duration::from_secs(60), || {
        let mut state = ImportState::load(&setup.data_dir)?;
        Ok::<_, String>(import::apply(
            &plans,
            connect,
            &mut state,
            &setup.data_dir,
            None,
        ))
    })?
    .ok_or("another import is running; try again later")??;
    let _ = writeln!(out, "{}", install::import_summary(&report));
    for warning in &report.warnings {
        let _ = writeln!(out, "  warning: {warning}");
    }
    for problem in &report.problems {
        let _ = writeln!(out, "  failed: {problem}");
    }
    Ok(if report.problems.is_empty() {
        0
    } else {
        EXIT_FAILED
    })
}

/// `doctor`: every check with its fix; exit code 1 when a problem was found.
fn doctor(setup: &Setup, out: &mut dyn Write) -> u8 {
    let findings = install::doctor(setup);
    let _ = writeln!(out, "graphersal-memory doctor (Claude Code)\n");
    let mut problems = 0;
    for finding in &findings {
        if finding.problem {
            problems += 1;
            let _ = writeln!(out, "  PROBLEM {}", finding.text);
            if let Some(fix) = &finding.fix {
                let _ = writeln!(out, "          fix: {fix}");
            }
        } else {
            let _ = writeln!(out, "  ok      {}", finding.text);
        }
    }
    let _ = writeln!(
        out,
        "\n{}",
        if problems == 0 {
            "No problems found.".to_string()
        } else {
            format!("{problems} problem(s) found.")
        }
    );
    if problems == 0 { 0 } else { EXIT_FAILED }
}

/// `hook`: never fails (exit code 0, notes on stderr).
fn run_hook(
    cli: &Cli,
    event: hook::HookEvent,
    stdin: impl Read + Send + 'static,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    match setup(cli) {
        Ok(setup) => {
            let connect = daemon_connector(&setup);
            hook::run(
                event,
                Ok(setup.paths.clone()),
                setup.data_dir.clone(),
                Box::new(connect),
                stdin,
                stdout,
                stderr,
            )
        }
        Err(err) => {
            let _ = writeln!(stderr, "graphersal-memory hook: {err}");
            0
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
        let settings = Config::load(&data_dir)?;
        let idle = config::idle(idle, std::env::var(daemon::IDLE_ENV).ok(), &settings)?;
        return daemon::run(DaemonOptions {
            data_dir,
            run_dir,
            idle,
            version: version_as.unwrap_or(protocol::VERSION).to_string(),
        });
    }
    if idle.is_some() || version_as.is_some() {
        return Err(format!(
            "--idle and --version-as need --foreground (a detached daemon reads {} or the \
             settings file {})",
            daemon::IDLE_ENV,
            config::CONFIG_FILE
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

/// `status`: the daemon, the data directory, the settings and what is installed in Claude Code.
fn status(cli: &Cli, mut stdout: impl Write) -> Result<(), String> {
    let (data_dir, run_dir) = dirs(cli)?;
    let setup = setup(cli);
    let text = status_text(&data_dir, &run_dir, setup.as_ref().map_err(String::as_str))?;
    let _ = write!(stdout, "{text}");
    Ok(())
}

/// The text of `status`.
pub fn status_text(
    data_dir: &Path,
    run_dir: &Path,
    setup: Result<&Setup, &str>,
) -> Result<String, String> {
    let daemon = match shim::control(run_dir, "status")? {
        Some(status) => describe(&status),
        None => format!(
            "daemon: not running (run directory {}; the next agent session starts it)",
            run_dir.display()
        ),
    };
    let settings = match Config::load(data_dir) {
        Ok(settings) => {
            let idle = config::idle(None, std::env::var(daemon::IDLE_ENV).ok(), &settings)
                .map(|idle| format!("{}s", idle.as_secs()))
                .unwrap_or_else(|err| err);
            format!(
                "settings: {} (daemon idle time {idle})",
                if config::path(data_dir).exists() {
                    config::path(data_dir).display().to_string()
                } else {
                    "defaults".to_string()
                }
            )
        }
        Err(err) => format!("settings: {err}"),
    };
    let mut text = format!(
        "{daemon}\ndata directory: {}\n{settings}\n",
        data_dir.display()
    );
    match setup {
        Ok(setup) => {
            for line in install::status_lines(setup) {
                text.push_str(&line);
                text.push('\n');
            }
        }
        Err(err) => text.push_str(&format!("Claude Code: cannot check ({err})\n")),
    }
    Ok(text)
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
        for (name, stage) in [("ui", "M7"), ("bench", "M5"), ("export", "M7")] {
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
    fn status_without_a_daemon_says_so_and_shows_the_installation() {
        let data = crate::mcp_tests::TempDir::new("st");
        let home = crate::mcp_tests::TempDir::new("sthome");
        let setup = crate::install_tests::setup_in(home.path(), data.path());
        let text = status_text(data.path(), &setup.run_dir, Ok(&setup)).unwrap();
        assert!(text.contains("daemon: not running"), "{text}");
        assert!(text.contains(&data.path().display().to_string()), "{text}");
        assert!(
            text.contains("settings: defaults (daemon idle time 7200s)"),
            "{text}"
        );
        assert!(
            text.contains("MCP server graphersal-memory (use): not installed"),
            "{text}"
        );
        assert!(
            text.contains("Not installed: run `graphersal-memory install`"),
            "{text}"
        );
        let text = status_text(data.path(), &setup.run_dir, Err("no home")).unwrap();
        assert!(
            text.contains("Claude Code: cannot check (no home)"),
            "{text}"
        );
    }

    #[test]
    fn other_harnesses_are_not_implemented_yet() {
        for command in ["install", "uninstall"] {
            let cli =
                Cli::try_parse_from(["graphersal-memory", command, "--harness", "cursor"]).unwrap();
            let mut err = Vec::new();
            assert_eq!(run_cli(&cli, std::io::empty(), Vec::new(), &mut err), 2);
            let text = String::from_utf8(err).unwrap();
            assert!(text.contains("not implemented yet"), "{text}");
            assert!(text.contains("M7"), "{text}");
        }
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
}
