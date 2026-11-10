//! Running a command. In this pre-release `install`, `uninstall`, `status`, `doctor`, `import`
//! (also `--sessions`), `export --to-files`, `projects`, `summaries` and `bench` (Claude Code), `mcp` (the shim, or
//! `--no-daemon`), `daemon` and the hidden `hook` work; every other command reports which stage
//! delivers it.

use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cli::{
    BenchAction, BenchArm, Cli, Command, Harness, Profile, ProjectsAction, SummariesAction,
};
use crate::config::{self, Config};
use crate::daemon::{self, DaemonOptions};
use crate::import::{self, Caller, ImportState};
use crate::install::{self, ClaudePaths, InstallOptions, Setup, UninstallOptions};
use crate::shim::{self, DaemonLocation, ShimOptions};
use crate::{bench, client, hook, ipc, mcp, protocol, sessions, store, summaries};

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
        Command::Projects { .. } => ("M6b", "project identity"),
        Command::Import { .. } => ("M3", "import of the file memory"),
        Command::Summaries { .. } => ("M4", "sessions and facts"),
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
        Command::Summaries { .. } => "summaries",
        Command::Export { .. } => "export",
        Command::Projects { .. } => "projects",
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
        Harness::All => "all",
        Harness::Codex => "codex",
        Harness::Cursor => "cursor",
    };
    format!(
        "graphersal-memory {command} --harness {name}: not implemented yet (planned in stage \
         M7: other harnesses); Claude Code, opencode, Codex and Cursor are supported one at a \
         time."
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
            language,
            model,
            no_import,
            keep_auto_memory,
            enable_develop,
            summaries,
        } => {
            let harnesses = match harness {
                None => None,
                Some(h) => match h.kinds() {
                    Some(kinds) => Some(kinds),
                    None => {
                        let _ = writeln!(stderr, "{}", harness_not_implemented(name, *h));
                        return EXIT_NOT_IMPLEMENTED;
                    }
                },
            };
            let terminal = terminal();
            let options = InstallOptions {
                harnesses,
                dry_run: *dry_run,
                yes: *yes,
                language: language.map(crate::cli::LanguageArg::language),
                model: model.clone(),
                no_import: *no_import,
                keep_auto_memory: *keep_auto_memory,
                enable_develop: *enable_develop,
                summaries: *summaries,
                interactive: terminal,
                color: terminal && crate::prompt::color_allowed(&|name| std::env::var(name).ok()),
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
            no_export,
        } => {
            let harnesses = match harness {
                None => None,
                Some(h) => match h.kinds() {
                    Some(kinds) => Some(kinds),
                    None => {
                        let _ = writeln!(stderr, "{}", harness_not_implemented(name, *h));
                        return EXIT_NOT_IMPLEMENTED;
                    }
                },
            };
            let options = UninstallOptions {
                harnesses,
                dry_run: *dry_run,
                yes: *yes,
                purge: *purge,
                disable_develop_only: *disable_develop,
                no_export: *no_export,
                interactive: terminal(),
            };
            setup(cli).and_then(|setup| {
                let mut source = DaemonSource(setup_location(&setup));
                install::uninstall_with(
                    &setup,
                    &options,
                    Some(&mut source),
                    &mut BufReader::new(stdin),
                    &mut stdout,
                )
            })
        }
        Command::Export {
            harness,
            dry_run,
            to_files: true,
            project,
        } => {
            let kind = match harness {
                Harness::ClaudeCode => crate::agents::Kind::ClaudeCode,
                Harness::Opencode => crate::agents::Kind::OpenCode,
                Harness::Codex => crate::agents::Kind::Codex,
                Harness::Cursor => crate::agents::Kind::Cursor,
                other => {
                    let _ = writeln!(stderr, "{}", harness_not_implemented(name, *other));
                    return EXIT_NOT_IMPLEMENTED;
                }
            };
            setup(cli).and_then(|setup| {
                let mut source = DaemonSource(setup_location(&setup));
                export_files(
                    &setup,
                    kind,
                    project.as_deref(),
                    *dry_run,
                    &mut source,
                    &mut stdout,
                )
            })
        }
        Command::Projects { action } => setup(cli).and_then(|setup| {
            run_projects(&setup, action, &mut BufReader::new(stdin), &mut stdout)
        }),
        Command::Doctor => setup(cli).map(|setup| doctor(&setup, &mut stdout)),
        Command::Import {
            dry_run,
            sessions: true,
        } => setup(cli).and_then(|setup| import_sessions(&setup, *dry_run, &mut stdout)),
        Command::Import {
            dry_run,
            sessions: false,
        } => setup(cli).and_then(|setup| {
            let mut connect = daemon_connector(&setup);
            run_import(&setup, *dry_run, &mut connect, &mut stdout)
        }),
        Command::Summaries { action } => setup(cli).and_then(|setup| {
            run_summaries(&setup, action, &mut BufReader::new(stdin), &mut stdout)
        }),
        Command::Hook { event, harness } => {
            return run_hook(cli, *event, *harness, stdin, &mut stdout, stderr);
        }
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
        Command::Bench {
            action,
            arms,
            questions,
            repeats,
            budget,
            project: project_flag,
            claude_dir,
            dry_run,
            yes,
        } => {
            let request = match action {
                Some(BenchAction::Questions {
                    questions,
                    review,
                    budget,
                    project,
                    claude_dir,
                    dry_run,
                    yes,
                }) => BenchRequest {
                    questions_only: true,
                    review: *review,
                    arms: Vec::new(),
                    questions: *questions,
                    repeats: 1,
                    budget: *budget,
                    project: project.clone(),
                    claude_dir: claude_dir.clone(),
                    dry_run: *dry_run,
                    yes: *yes,
                },
                None => BenchRequest {
                    questions_only: false,
                    review: false,
                    arms: arms.clone(),
                    questions: *questions,
                    repeats: *repeats,
                    budget: *budget,
                    project: project_flag.clone(),
                    claude_dir: claude_dir.clone(),
                    dry_run: *dry_run,
                    yes: *yes,
                },
            };
            run_bench(cli, &request, &mut BufReader::new(stdin), &mut stdout)
        }
        command @ (Command::Ui
        | Command::Export {
            to_files: false, ..
        }) => {
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
    setup_with(cli, true)
}

/// [`setup`]; `probe`: ask `opencode --version` for opencode's line (the hooks do not: they
/// need only its database, and a session start should not wait for a second program).
fn setup_with(cli: &Cli, probe: bool) -> Result<Setup, String> {
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let (data_dir, run_dir) = dirs(cli)?;
    let paths = ClaudePaths::resolve(&env)?;
    let detected = crate::opencode::detect(&env, None, probe)?;
    let opencode = crate::opencode::OpenCodePaths::resolve_for(&env, detected)?;
    let codex = crate::codex::CodexPaths::resolve(&env)?;
    let cursor = crate::cursor::CursorPaths::resolve(&env, &data_dir)?;
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
        opencode,
        codex,
        cursor,
        search_path: None,
        data_dir,
        run_dir,
        exe: mcp::canonical(&exe),
        dir_args,
        source: crate::download::Source::from_env(&env),
    })
}

/// Whether stdin and stdout are terminals (the wizard asks then).
fn terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// The memories of the daemon's store (`export_memories` control line).
pub struct DaemonSource(pub DaemonLocation);

impl crate::export::Source for DaemonSource {
    fn memories(&mut self, project: &str) -> Result<Vec<crate::export::Row>, String> {
        let answer = shim::request(
            &self.0,
            &serde_json::json!({"protocol": protocol::PROTOCOL, "control": "export_memories",
                                "project": project}),
        )?;
        if answer["ok"] != true {
            return Err(answer["error"]["message"]
                .as_str()
                .unwrap_or("the daemon refused the export")
                .to_string());
        }
        Ok(crate::export::rows(&answer["memories"]))
    }
}

/// `export --to-files [--project ROOT]`.
pub fn export_files(
    setup: &Setup,
    kind: crate::agents::Kind,
    project: Option<&Path>,
    dry_run: bool,
    source: &mut dyn crate::export::Source,
    out: &mut dyn Write,
) -> Result<u8, String> {
    let id = match project {
        Some(root) => {
            let key = mcp::project_key(Some(root), &absolute(root));
            let projects = crate::projects::Projects::load(&setup.data_dir)?;
            match projects.find(&key) {
                Some(found) => found.id,
                None => {
                    return Err(format!(
                        "the memory knows no project at {key} (`graphersal-memory projects \
                         list` shows them)"
                    ));
                }
            }
        }
        None => String::new(),
    };
    let rows = source.memories(&id)?;
    let agent = setup.agent_of(kind);
    let agent = agent.as_ref();
    if dry_run {
        let projects: std::collections::BTreeSet<&str> = rows
            .iter()
            .map(|r| r.project.as_str())
            .filter(|p| !p.is_empty())
            .collect();
        let _ = writeln!(
            out,
            "Dry run: {} memories of {} project(s) and {} global one(s) would be written into \
             {}; nothing was written.",
            rows.iter().filter(|r| !r.project.is_empty()).count(),
            projects.len(),
            rows.iter().filter(|r| r.project.is_empty()).count(),
            export_target(agent)
        );
        return Ok(0);
    }
    if rows.is_empty() {
        let _ = writeln!(out, "Nothing to export: the graph holds no memories.");
        return Ok(0);
    }
    let report = crate::export::write(agent, &setup.data_dir, &rows)?;
    for line in report.lines() {
        let _ = writeln!(out, "{line}");
    }
    if !agent.has_file_memory() {
        let _ = writeln!(out, "{}", export_note(agent));
    }
    Ok(if report.problems.is_empty() {
        0
    } else {
        EXIT_FAILED
    })
}

/// Where the export of `agent` goes, for messages.
pub fn export_target(agent: &dyn crate::agents::Agent) -> String {
    if agent.has_file_memory() {
        format!("{}'s file memory", agent.display_name())
    } else {
        format!(
            "readable Markdown files under {}",
            agent
                .global_memory_file()
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        )
    }
}

/// What to do with an export for a harness without file memory.
pub fn export_note(agent: &dyn crate::agents::Agent) -> String {
    let how = match agent.kind() {
        crate::agents::Kind::Codex => {
            "to give the agent a file, paste it into ~/.codex/AGENTS.md or a project's AGENTS.md"
        }
        crate::agents::Kind::Cursor => {
            "to give the agent a file, paste it into a project's AGENTS.md or into a rule file \
             under ~/.cursor/rules (a .mdc file with `alwaysApply: true`)"
        }
        _ => {
            "to give the agent a file, add its path to `instructions` in the agent's configuration"
        }
    };
    format!(
        "{} has no file memory: the export is plain Markdown (per project MEMORY.md and one file \
         per memory, the global ones in {}). It is not loaded automatically; {how}. \
         `graphersal-memory export --to-files --harness {}` writes it again.",
        agent.display_name(),
        agent.global_memory_file().display(),
        agent.id()
    )
}

/// `projects list` and `projects merge`.
fn run_projects(
    setup: &Setup,
    action: &ProjectsAction,
    input: &mut dyn std::io::BufRead,
    out: &mut dyn Write,
) -> Result<u8, String> {
    let projects = crate::projects::Projects::load(&setup.data_dir)?;
    match action {
        ProjectsAction::List => {
            let list = projects.list();
            if list.is_empty() {
                let _ = writeln!(
                    out,
                    "No projects yet: a project is recorded when a session starts in it."
                );
            }
            for project in &list {
                let _ = writeln!(out, "{}", project.id);
                let _ = writeln!(out, "  paths: {}", project.paths.join(", "));
                if !project.remotes.is_empty() {
                    let _ = writeln!(out, "  remotes: {}", project.remotes.join(", "));
                }
                if let Some(root) = &project.root_commit {
                    let _ = writeln!(out, "  first commit: {root}");
                }
            }
            for (what, how) in crate::projects::look_alike_lines(&setup.data_dir) {
                let _ = writeln!(out, "\nnote: {what}\n      {how}");
            }
            Ok(0)
        }
        ProjectsAction::Merge { from, into, yes } => {
            let find = |name: &str| {
                projects.find(name).ok_or_else(|| {
                    format!("no project {name} (`graphersal-memory projects list` shows them)")
                })
            };
            let (source, target) = (find(from)?, find(into)?);
            if source.id == target.id {
                return Err(format!("{from} and {into} are the same project"));
            }
            let _ = writeln!(
                out,
                "Merge project {} ({}) into {} ({}): its memories, sessions, turns and facts \
                 move, a memory of the same name keeps the more recently updated text, and {} \
                 is found by {}'s paths and remotes from then on.",
                source.id,
                source.paths.join(", "),
                target.id,
                target.paths.join(", "),
                target.id,
                source.id
            );
            if !yes && !install::confirm("Merge?", input, out) {
                let _ = writeln!(out, "Nothing was merged.");
                return Ok(1);
            }
            let answer = shim::request(
                &setup_location(setup),
                &serde_json::json!({"protocol": protocol::PROTOCOL, "control": "merge_projects",
                                    "from": source.id, "into": target.id}),
            )?;
            if answer["ok"] != true {
                return Err(answer["error"]["message"]
                    .as_str()
                    .unwrap_or("the daemon refused the merge")
                    .to_string());
            }
            let merged = &answer["merged"];
            let _ = writeln!(
                out,
                "Merged: {} memories moved, {} kept from {} (newer there), {} replaced, {} \
                 sessions, turns and facts moved.",
                merged["memories_moved"],
                merged["memories_kept"],
                target.id,
                merged["memories_replaced"],
                merged["others"]
            );
            Ok(0)
        }
    }
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

/// Where the daemon of `setup` is.
fn setup_location(setup: &Setup) -> DaemonLocation {
    DaemonLocation {
        data_dir: setup.data_dir.clone(),
        run_dir: setup.run_dir.clone(),
        exe: setup.exe.clone(),
        start_timeout: shim::START_TIMEOUT,
    }
}

/// `import --sessions`: queues the indexing of every finished session in the daemon (starting
/// it) and shows the progress until the run is done.
pub fn import_sessions(setup: &Setup, dry_run: bool, out: &mut dyn Write) -> Result<u8, String> {
    let mut requests = Vec::new();
    for kind in session_harnesses(setup) {
        let dir = setup.agent_of(kind).projects_dir().to_path_buf();
        let request = sessions::Request::for_harness(kind, &dir);
        let (found, active, todo, unchanged) = sessions::plan(&request, &setup.data_dir)?;
        let _ = writeln!(
            out,
            "{}: {found} session{} in {}: {todo} new or changed, {unchanged} unchanged, \
             {active} still being written (indexed once unchanged for {} minutes)",
            kind.display_name(),
            if found == 1 { "" } else { "s" },
            dir.display(),
            sessions::IDLE.as_secs() / 60
        );
        requests.push(request);
    }
    let settings = Config::load(&setup.data_dir)?;
    let _ = writeln!(
        out,
        "summaries: {}",
        if settings.summaries.enabled {
            "on (through your harness, within the monthly budget)"
        } else {
            "off (raw turns only; `graphersal-memory summaries estimate` shows what they would \
             cost)"
        }
    );
    if dry_run {
        let _ = writeln!(out, "Dry run: nothing was indexed.");
        return Ok(0);
    }
    let mut totals = [0u64; 4];
    let mut failed = false;
    for request in &requests {
        let answer = shim::request(&setup_location(setup), &request.to_control())?;
        let Some(run) = answer["run"].as_u64() else {
            return Err(format!(
                "the daemon did not take the request: {}",
                answer["error"]["message"]
                    .as_str()
                    .unwrap_or("unknown answer")
            ));
        };
        let _ = writeln!(out, "{}:", request.harness.display_name());
        let mut shown = String::new();
        loop {
            let status = shim::request_running(
                &setup.run_dir,
                &serde_json::json!({"protocol": protocol::PROTOCOL, "control": sessions::CONTROL_STATUS}),
            )?
            .ok_or("the daemon stopped while indexing; see its log")?;
            let report = &status["report"];
            let done = status["completed"].as_u64().unwrap_or(0) >= run;
            let line = format!(
                "  {}: {}/{} ({} indexed, {} unchanged, {} summarized)",
                report["phase"].as_str().unwrap_or("queued"),
                report["done"],
                report["total"],
                report["indexed"],
                report["unchanged"],
                report["summarized"]
            );
            if line != shown && (done || report["phase"].is_string()) {
                let _ = writeln!(out, "{line}");
                let _ = out.flush();
                shown = line;
            }
            if done {
                for problem in report["problems"].as_array().into_iter().flatten() {
                    let _ = writeln!(out, "  problem: {}", problem.as_str().unwrap_or_default());
                }
                for (total, key) in
                    totals
                        .iter_mut()
                        .zip(["indexed", "turns", "unchanged", "summarized"])
                {
                    *total += report[key].as_u64().unwrap_or(0);
                }
                failed |= report["problems"].as_array().is_some_and(|p| {
                    p.iter().any(|p| {
                        !p.as_str()
                            .unwrap_or_default()
                            .contains("monthly budget is reached")
                    })
                });
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    let _ = writeln!(
        out,
        "Done: {} sessions indexed ({} turns), {} unchanged, {} summarized.",
        totals[0], totals[1], totals[2], totals[3]
    );
    Ok(if failed { EXIT_FAILED } else { 0 })
}

/// The harnesses whose sessions are indexed: those graphersal-memory is installed in, else
/// those found on this machine, else Claude Code.
pub fn session_harnesses(setup: &Setup) -> Vec<crate::agents::Kind> {
    let installed = setup.installed();
    if !installed.is_empty() {
        return installed;
    }
    // Found but not set up: only a harness that has sessions to read.
    let detected: Vec<_> = setup
        .detected()
        .into_iter()
        .filter(|kind| match kind {
            crate::agents::Kind::OpenCode => setup.opencode.db.is_file(),
            crate::agents::Kind::Codex => setup.codex.sessions.is_dir(),
            crate::agents::Kind::Cursor => setup.cursor.registry.is_file(),
            crate::agents::Kind::ClaudeCode => true,
        })
        .collect();
    if detected.is_empty() {
        vec![crate::agents::Kind::ClaudeCode]
    } else {
        detected
    }
}

/// The estimate of summarizing the sessions of `kinds`.
pub fn estimate_all(
    setup: &Setup,
    kinds: &[crate::agents::Kind],
    since: Option<&str>,
) -> Result<sessions::Estimate, String> {
    let mut total = sessions::Estimate::default();
    for kind in kinds {
        let dir = setup.agent_of(*kind).projects_dir().to_path_buf();
        let e = sessions::estimate_for(*kind, &dir, &setup.data_dir, sessions::IDLE, since)?;
        total.sessions += e.sessions;
        total.pending += e.pending;
        total.too_short += e.too_short;
        total.usd += e.usd;
    }
    Ok(total)
}

/// Indexed and summarized sessions over every harness's state.
fn session_counts(data_dir: &Path) -> Result<(usize, usize), String> {
    let mut counts = (0, 0);
    for kind in crate::agents::Kind::ALL {
        let state = sessions::State::load_for(data_dir, kind)?;
        counts.0 += state.indexed();
        counts.1 += state.summarized();
    }
    Ok(counts)
}

/// One estimate line for summaries through `command`: $0 for a free model of opencode Zen or a
/// local model of Codex, else [`estimate_line`].
pub fn estimate_line_for(estimate: &sessions::Estimate, command: &[String]) -> String {
    if crate::codex::local_model(command) {
        format!(
            "{} finished session{}, {} without a summary: $0.00 (`{}` runs a local model on this \
             machine)",
            estimate.sessions,
            if estimate.sessions == 1 { "" } else { "s" },
            estimate.pending,
            command.join(" ")
        )
    } else if crate::opencode::free_model(command) {
        format!(
            "{} finished session{}, {} without a summary: $0.00 (`{}` uses a free model of \
             opencode Zen, free for a limited time; the monthly budget still applies)",
            estimate.sessions,
            if estimate.sessions == 1 { "" } else { "s" },
            estimate.pending,
            command.join(" ")
        )
    } else {
        estimate_line(estimate)
    }
}

/// One estimate line.
pub fn estimate_line(estimate: &sessions::Estimate) -> String {
    format!(
        "{} finished session{}, {} without a summary{}: about ${:.2} at list price \
         (an estimate: ~4 characters per token, $3/$15 per million input/output tokens, plus \
         the harness's own prompt; your harness reports the real cost)",
        estimate.sessions,
        if estimate.sessions == 1 { "" } else { "s" },
        estimate.pending,
        if estimate.too_short > 0 {
            format!(" ({} more too short to summarize)", estimate.too_short)
        } else {
            String::new()
        },
        estimate.usd
    )
}

/// `summaries estimate|status|enable|disable`.
fn run_summaries(
    setup: &Setup,
    action: &SummariesAction,
    input: &mut dyn std::io::BufRead,
    out: &mut dyn Write,
) -> Result<u8, String> {
    let settings = Config::load(&setup.data_dir)?;
    let kinds = session_harnesses(setup);
    let estimate = || estimate_all(setup, &kinds, settings.summaries.since.as_deref());
    let command = settings.summaries.harness_command.join(" ");
    let line = |estimate: &sessions::Estimate| {
        estimate_line_for(estimate, &settings.summaries.harness_command)
    };
    match action {
        SummariesAction::Estimate => {
            let _ = writeln!(out, "{}", line(&estimate()?));
            let _ = writeln!(
                out,
                "Summaries run through `{command}` with a monthly budget of ${:.2}; nothing was \
                 sent anywhere.",
                settings.summaries.monthly_budget_usd
            );
        }
        SummariesAction::Status => {
            let spend = summaries::Spend::load(&setup.data_dir)?;
            let month = summaries::this_month();
            let (indexed, summarized) = session_counts(&setup.data_dir)?;
            let _ = writeln!(
                out,
                "summaries: {}\nthis month ({month}): ${:.2} spent of ${:.2}, {} summaries\n\
                 sessions: {} indexed, {} with a summary of their current version",
                if settings.summaries.enabled {
                    format!("on, through `{command}`")
                } else {
                    "off".to_string()
                },
                spend.usd(&month),
                settings.summaries.monthly_budget_usd,
                spend.calls(&month),
                indexed,
                summarized
            );
        }
        SummariesAction::Enable { budget, yes } => {
            let _ = writeln!(out, "{}", line(&estimate()?));
            let cap = budget.unwrap_or(settings.summaries.monthly_budget_usd);
            let _ = writeln!(
                out,
                "Each finished session is sent once (again when it grows) to `{command}`: its \
                 title, times, branches, edited file paths and the text of every prompt and \
                 reply, never tool output. At most ${cap:.2} per month; above it only raw turns \
                 are indexed."
            );
            if !yes && !crate::install::confirm("Switch summaries on?", input, out) {
                let _ = writeln!(out, "Summaries stay off.");
                return Ok(1);
            }
            write_summaries(setup, true, *budget)?;
            let _ = writeln!(
                out,
                "Summaries are on. They run in the background after the next session (or now: \
                 `graphersal-memory import --sessions`)."
            );
        }
        SummariesAction::Disable => {
            write_summaries(setup, false, None)?;
            let _ = writeln!(out, "Summaries are off; raw turns are still indexed.");
        }
    }
    Ok(0)
}

/// Switches the summaries in the settings file.
pub fn write_summaries(setup: &Setup, enabled: bool, budget: Option<f64>) -> Result<(), String> {
    let path = config::path(&setup.data_dir);
    let current = crate::fsutil::read_optional(&path)?
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    let text = config::with_summaries(current.as_deref(), enabled, budget)?;
    std::fs::create_dir_all(&setup.data_dir)
        .map_err(|err| format!("cannot create {}: {err}", setup.data_dir.display()))?;
    crate::fsutil::write_atomic(&path, text.as_bytes())
}

/// `doctor`: every check with its fix; exit code 1 when a problem was found.
fn doctor(setup: &Setup, out: &mut dyn Write) -> u8 {
    let kinds = session_harnesses(setup);
    let mut findings = Vec::new();
    if kinds.contains(&crate::agents::Kind::OpenCode) {
        findings.extend(crate::opencode::install::doctor(setup));
    }
    if kinds.contains(&crate::agents::Kind::Codex) {
        findings.extend(crate::codex::install::doctor(setup));
    }
    if kinds.contains(&crate::agents::Kind::Cursor) {
        findings.extend(crate::cursor::install::doctor(setup));
    }
    // Claude Code's checks end with the shared ones (settings, model, daemon, registry).
    if kinds.contains(&crate::agents::Kind::ClaudeCode) {
        findings.extend(install::doctor(setup));
    } else {
        findings.extend(install::shared_doctor(setup));
    }
    let _ = writeln!(
        out,
        "graphersal-memory doctor ({})\n",
        kinds
            .iter()
            .map(|k| k.display_name())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut problems = 0;
    for finding in &findings {
        if finding.problem {
            problems += 1;
            let _ = writeln!(out, "  PROBLEM {}", finding.text);
            if let Some(fix) = &finding.fix {
                let _ = writeln!(out, "          fix: {fix}");
            }
        } else {
            match &finding.fix {
                Some(hint) => {
                    let _ = writeln!(out, "  note    {}", finding.text);
                    let _ = writeln!(out, "          {hint}");
                }
                None => {
                    let _ = writeln!(out, "  ok      {}", finding.text);
                }
            }
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
    harness: Harness,
    stdin: impl Read + Send + 'static,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    // The summaries' own harness runs: nothing to do.
    if std::env::var(summaries::HOOK_ENV).is_ok_and(|v| v == "off") {
        return 0;
    }
    // Claude Code's hooks run by Cursor (its third-party hooks): the Cursor hooks do Cursor's
    // part. The input is read here to tell (a harness closes stdin after writing it).
    let input = hook::read_input(stdin);
    if harness == Harness::ClaudeCode && hook::is_cursor_input(&String::from_utf8_lossy(&input)) {
        let _ = writeln!(
            stderr,
            "graphersal-memory hook: run by Cursor for Claude Code's settings; nothing to do \
             (graphersal-memory's own Cursor hooks handle Cursor: `graphersal-memory install \
             --harness cursor`)"
        );
        return 0;
    }
    let stdin = std::io::Cursor::new(input);
    match setup_with(cli, false) {
        Ok(setup) => {
            let connect = daemon_connector(&setup);
            let location = setup_location(&setup);
            let enqueue: hook::Enqueue = Box::new(move |request: sessions::Request| {
                let answer = shim::request(&location, &request.to_control())?;
                if answer["ok"] == serde_json::Value::Bool(true) {
                    Ok(())
                } else {
                    Err(answer["error"]["message"]
                        .as_str()
                        .unwrap_or("refused")
                        .to_string())
                }
            });
            if harness == Harness::Cursor {
                return hook::run_cursor(
                    event,
                    setup.cursor.registry.clone(),
                    std::env::var("CURSOR_PROJECT_DIR").ok(),
                    Some(enqueue),
                    stdin,
                    stdout,
                    stderr,
                );
            }
            if harness == Harness::Codex {
                return hook::run_codex(
                    event,
                    setup.codex.sessions.clone(),
                    Some(enqueue),
                    stdin,
                    stdout,
                    stderr,
                );
            }
            if harness == Harness::Opencode {
                return hook::run_opencode(
                    event,
                    setup.opencode.db.clone(),
                    Some(enqueue),
                    stdin,
                    stdout,
                    stderr,
                );
            }
            hook::run_with(
                event,
                Ok(setup.paths.clone()),
                setup.data_dir.clone(),
                Box::new(connect),
                Some(enqueue),
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

/// A `bench` or `bench questions` invocation.
#[derive(Debug, Clone)]
pub struct BenchRequest {
    /// `bench questions`.
    pub questions_only: bool,
    /// `bench questions --review`.
    pub review: bool,
    /// The arms.
    pub arms: Vec<BenchArm>,
    /// Questions to generate.
    pub questions: u32,
    /// Repeats.
    pub repeats: u32,
    /// The budget, USD.
    pub budget: f64,
    /// `--project`.
    pub project: Option<PathBuf>,
    /// `--claude-dir`.
    pub claude_dir: Option<PathBuf>,
    /// `--dry-run`.
    pub dry_run: bool,
    /// `--yes`.
    pub yes: bool,
}

/// The bench options of `request` on this machine: the data directory, the sources (Claude
/// Code's directory, or `--claude-dir`), this binary, the harness command of the settings.
pub fn bench_options(
    data_dir: &Path,
    claude: ClaudePaths,
    opencode: crate::opencode::OpenCodePaths,
    codex: crate::codex::CodexPaths,
    exe: PathBuf,
    project: String,
    request: &BenchRequest,
) -> Result<bench::Options, String> {
    let settings = Config::load(data_dir)?;
    Ok(bench::Options {
        arms: request.arms.clone(),
        questions: request.questions as usize,
        repeats: request.repeats,
        budget_usd: request.budget,
        dry_run: request.dry_run,
        yes: request.yes,
        project,
        claude,
        opencode,
        codex,
        data_dir: data_dir.to_path_buf(),
        exe,
        harness_command: settings.summaries.harness_command,
    })
}

/// `bench` and `bench questions`.
fn run_bench(
    cli: &Cli,
    request: &BenchRequest,
    input: &mut dyn std::io::BufRead,
    out: &mut dyn Write,
) -> Result<u8, String> {
    let (data_dir, _) = dirs(cli)?;
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let claude = match &request.claude_dir {
        Some(dir) => {
            let dir = absolute(dir);
            ClaudePaths::resolve(&|name: &str| {
                (name == "CLAUDE_CONFIG_DIR").then(|| dir.display().to_string())
            })?
        }
        None => ClaudePaths::resolve(&env)?,
    };
    let exe =
        std::env::current_exe().map_err(|err| format!("cannot find this program's path: {err}"))?;
    let options = bench_options(
        &data_dir,
        claude,
        crate::opencode::OpenCodePaths::resolve(&env)?,
        crate::codex::CodexPaths::resolve(&env)?,
        mcp::canonical(&exe),
        project(request.project.as_deref())?,
        request,
    )?;
    let mut harness: Box<dyn bench::Harness> = if request.dry_run {
        Box::new(bench::DryRun::default())
    } else {
        Box::new(bench::Process)
    };
    if request.questions_only {
        bench::questions_command(&options, request.review, harness.as_mut(), input, out)
    } else {
        bench::run(&options, harness.as_mut(), input, out)
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
    let running = shim::control(run_dir, "status")?;
    let daemon = match &running {
        Some(status) => describe(status),
        None => format!(
            "daemon: not running (run directory {}; the next agent session starts it)",
            run_dir.display()
        ),
    };
    let stores = store_lines(data_dir, running.as_ref());
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
    let sessions = match (session_counts(data_dir), Config::load(data_dir)) {
        (Ok((indexed, summarized)), Ok(config)) => {
            let spend = summaries::Spend::load(data_dir)
                .map(|spend| spend.usd(&summaries::this_month()))
                .unwrap_or(0.0);
            format!(
                "sessions: {indexed} indexed, {summarized} summarized; summaries {}",
                if config.summaries.enabled {
                    format!(
                        "on (${spend:.2} of ${:.2} spent this month)",
                        config.summaries.monthly_budget_usd
                    )
                } else {
                    "off".to_string()
                }
            )
        }
        (Err(err), _) | (_, Err(err)) => format!("sessions: {err}"),
    };
    let mut text = format!(
        "{daemon}\ndata directory: {}\n{settings}\n{sessions}\n",
        data_dir.display()
    );
    for line in stores {
        text.push_str(&line);
        text.push('\n');
    }
    match setup {
        Ok(setup) => {
            let kinds = setup.installed();
            let mut lines = Vec::new();
            if kinds.is_empty() || kinds.contains(&crate::agents::Kind::ClaudeCode) {
                lines.extend(install::status_lines(setup));
            } else {
                lines.extend(install::shared_status_lines(setup));
            }
            if kinds.contains(&crate::agents::Kind::OpenCode) {
                lines.extend(crate::opencode::install::status_lines(setup));
            } else if setup.detected().contains(&crate::agents::Kind::OpenCode) {
                lines.push(
                    "opencode: found, not set up (`graphersal-memory install --harness \
                     opencode`)"
                        .to_string(),
                );
            }
            if kinds.contains(&crate::agents::Kind::Codex) {
                lines.extend(crate::codex::install::status_lines(setup));
            } else if setup.detected().contains(&crate::agents::Kind::Codex) {
                lines.push(
                    "Codex: found, not set up (`graphersal-memory install --harness codex`)"
                        .to_string(),
                );
            }
            if kinds.contains(&crate::agents::Kind::Cursor) {
                lines.extend(crate::cursor::install::status_lines(setup));
            } else if setup.detected().contains(&crate::agents::Kind::Cursor) {
                lines.push(
                    "Cursor: found, not set up (`graphersal-memory install --harness cursor`)"
                        .to_string(),
                );
            }
            for line in lines {
                text.push_str(&line);
                text.push('\n');
            }
        }
        Err(err) => text.push_str(&format!("Claude Code: cannot check ({err})\n")),
    }
    Ok(text)
}

/// The maintenance lines of the stores: each open store's numbers from the running daemon's
/// status, else the default store's from its files (when it exists).
fn store_lines(data_dir: &Path, daemon: Option<&serde_json::Value>) -> Vec<String> {
    let mut lines = Vec::new();
    let open = daemon
        .and_then(|status| status["maintenance"].as_object())
        .filter(|stores| !stores.is_empty());
    match open {
        Some(stores) => {
            for (name, status) in stores {
                lines.extend(crate::maintenance::status_lines(name, status));
            }
        }
        None => {
            let dir = crate::store::default_store_dir(data_dir);
            if dir.join("GRAPH").exists() {
                let settings = crate::maintenance::Settings::load(data_dir);
                let status = crate::maintenance::files_status(&dir, &settings);
                lines.extend(crate::maintenance::status_lines(
                    crate::registry::DEFAULT_STORE,
                    &status,
                ));
            }
        }
    }
    lines
}

/// The `doctor` findings about the stores' WAL (only the running daemon knows the WAL since
/// the last snapshot).
pub fn store_findings(run_dir: &Path) -> Vec<install::Finding> {
    let Ok(Some(status)) = shim::control(run_dir, "status") else {
        return Vec::new();
    };
    status["maintenance"]
        .as_object()
        .map(|stores| {
            stores
                .iter()
                .filter_map(|(name, status)| crate::maintenance::wal_finding(name, status))
                .collect()
        })
        .unwrap_or_default()
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
        for (name, stage) in [("ui", "M7"), ("export", "M7")] {
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
    fn every_harness_choice_is_implemented_and_export_takes_one() {
        // Cursor was the last planned harness (M7d): every choice names supported harnesses.
        use clap::ValueEnum;
        for harness in Harness::value_variants() {
            assert!(harness.kinds().is_some(), "{harness:?}");
        }
        assert!(harness_not_implemented("export", Harness::All).contains("one at a time"));
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
