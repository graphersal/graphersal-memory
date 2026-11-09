//! Command-line interface: the command set of the product and its help text.
//!
//! Every command exists from the first stage on, so the help text shows the whole product; a
//! command that is not built yet says so and names the stage that delivers it
//! (`crate::commands`).

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// The long `--version` text: the product version and the engine revision it is built on.
const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (pre-release; engine: graphersal git ",
    crate::engine::engine_rev_short!(),
    ")"
);

/// Local graph memory for AI coding agents.
#[derive(Debug, Parser)]
#[command(
    name = "graphersal-memory",
    version,
    long_version = LONG_VERSION,
    about = "Local graph memory for AI coding agents: rules, decisions and past sessions, \
             searchable through MCP.",
    long_about = "Local graph memory for AI coding agents.\n\n\
        graphersal-memory keeps your agent's rules, decisions, past sessions and facts in a local \
        graph database and lets the agent search them through MCP, instead of loading a growing \
        memory file on every turn. Everything stays on your machine.\n\n\
        PRE-RELEASE: `install`, `uninstall`, `status`, `doctor` and `import` (for Claude Code), \
        `mcp` (the stdio server a harness starts; it shares one background daemon with every \
        other session) and `daemon` work. Every other command says which stage delivers it; \
        the plan is in docs/roadmap.md.",
    after_help = "Start with `graphersal-memory install`. \
        Docs: https://github.com/graphersal/graphersal-memory",
    propagate_version = true,
    subcommand_required = true,
    arg_required_else_help = true
)]
pub struct Cli {
    /// The command to run.
    #[command(subcommand)]
    pub command: Command,
    /// The data directory holding the stores (default: the per-user data directory; the
    /// environment variable GRAPHERSAL_MEMORY_DIR overrides it too).
    #[arg(long, global = true, value_name = "DIR")]
    pub data_dir: Option<PathBuf>,
    /// The run directory holding the daemon's socket, lock files and log (default: `run` in an
    /// explicitly chosen data directory, else the per-user run directory; the environment
    /// variable GRAPHERSAL_MEMORY_RUN_DIR overrides it too).
    #[arg(long, global = true, value_name = "DIR")]
    pub run_dir: Option<PathBuf>,
}

/// The product's commands.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Command {
    /// Set up graph memory for an agent harness: MCP servers, hooks, import of the existing
    /// memory, the managed rule block.
    Install {
        /// The harness to set up.
        #[arg(long, value_enum, default_value_t = Harness::ClaudeCode)]
        harness: Harness,
        /// Show what would change without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Do not ask before changing anything.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Also download a better embedding model (planned: today the built-in embedder is
        /// used and this only says so).
        #[arg(long)]
        with_model: bool,
        /// Also register the develop MCP server (structure tools; off by default).
        #[arg(long)]
        enable_develop: bool,
    },
    /// Remove exactly what `install` added; your own settings stay untouched.
    Uninstall {
        /// The harness to remove graph memory from.
        #[arg(long, value_enum, default_value_t = Harness::ClaudeCode)]
        harness: Harness,
        /// Show what would change without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Do not ask before changing anything.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Also delete the data directory with every memory in it (asks first unless --yes).
        #[arg(long, conflicts_with = "disable_develop")]
        purge: bool,
        /// Only unregister the develop MCP server; everything else stays installed.
        #[arg(long)]
        disable_develop: bool,
    },
    /// Show the daemon, the stores and what is installed where.
    Status,
    /// Check the installation and explain how to fix what is wrong.
    Doctor,
    /// Serve MCP over stdio for one harness session (the harness starts this, not you).
    Mcp {
        /// Which MCP server this is: `use` (memory tools) or `develop` (structure tools).
        #[arg(long, value_enum, default_value_t = Profile::Use)]
        profile: Profile,
        /// Serve the store in this process instead of connecting to the shared daemon.
        #[arg(long)]
        no_daemon: bool,
        /// The project this session works on (default: the git repository root of the current
        /// directory, else the current directory).
        #[arg(long, value_name = "ROOT")]
        project: Option<PathBuf>,
    },
    /// Run the background daemon that owns the stores (started on demand by `mcp`).
    Daemon {
        /// Exit after this long without a connection (examples: 90s, 30m, 2h; default 2h; the
        /// environment variable GRAPHERSAL_MEMORY_IDLE sets it too).
        #[arg(long, value_name = "DURATION", value_parser = parse_idle)]
        idle: Option<std::time::Duration>,
        /// Run in this process (logging to stderr) instead of starting a detached daemon.
        #[arg(long)]
        foreground: bool,
        /// The version the daemon reports (tests of the upgrade path).
        #[arg(long, hide = true, value_name = "VERSION")]
        version_as: Option<String>,
    },
    /// Open the memory in the browser (develop mode); runs while this terminal runs.
    Ui,
    /// Measure whether the memory helps: the built-in A/B on your own data.
    Bench {
        /// Plan the run and estimate its cost without calling any model.
        #[arg(long)]
        dry_run: bool,
    },
    /// Import the harness's file memory (MEMORY.md and the memory files; new and changed files
    /// only).
    Import {
        /// Show what would be imported without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Export rules from the graph into the harness's instruction files (managed block only).
    Export {
        /// The harness to export rules for.
        #[arg(long, value_enum, default_value_t = Harness::ClaudeCode)]
        harness: Harness,
        /// Show the managed block without writing it.
        #[arg(long)]
        dry_run: bool,
    },
    /// Run a harness hook (the hooks `install` adds call this; not for you).
    #[command(hide = true)]
    Hook {
        /// The hook event.
        #[arg(value_enum)]
        event: crate::hook::HookEvent,
    },
}

/// Parses `--idle`.
fn parse_idle(text: &str) -> Result<std::time::Duration, String> {
    crate::daemon::parse_duration(text)
}

/// An AI agent harness graphersal-memory can be installed into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Harness {
    /// Claude Code (first supported harness).
    ClaudeCode,
    /// opencode (planned).
    Opencode,
    /// OpenAI Codex CLI (planned).
    Codex,
    /// Cursor (planned).
    Cursor,
}

/// The MCP server a connection belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Profile {
    /// Everyday memory tools: search, read, remember, forget. On by default.
    Use,
    /// Tools to improve the memory's structure. Off by default.
    Develop,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use clap::error::ErrorKind;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("graphersal-memory").chain(args.iter().copied()))
    }

    #[test]
    fn command_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn every_command_parses() {
        for name in [
            "install",
            "uninstall",
            "status",
            "doctor",
            "mcp",
            "daemon",
            "ui",
            "bench",
            "import",
            "export",
        ] {
            assert!(parse(&[name]).is_ok(), "{name} does not parse");
        }
    }

    #[test]
    fn defaults_are_claude_code_and_use() {
        assert_eq!(
            parse(&["install"]).unwrap().command,
            Command::Install {
                harness: Harness::ClaudeCode,
                dry_run: false,
                yes: false,
                with_model: false,
                enable_develop: false
            }
        );
        assert_eq!(
            parse(&["mcp"]).unwrap().command,
            Command::Mcp {
                profile: Profile::Use,
                no_daemon: false,
                project: None
            }
        );
    }

    #[test]
    fn options_parse() {
        assert_eq!(
            parse(&["mcp", "--profile", "develop", "--no-daemon"])
                .unwrap()
                .command,
            Command::Mcp {
                profile: Profile::Develop,
                no_daemon: true,
                project: None
            }
        );
        assert_eq!(
            parse(&["uninstall", "--harness", "cursor", "--dry-run"])
                .unwrap()
                .command,
            Command::Uninstall {
                harness: Harness::Cursor,
                dry_run: true,
                yes: false,
                purge: false,
                disable_develop: false
            }
        );
        assert_eq!(
            parse(&["bench", "--dry-run"]).unwrap().command,
            Command::Bench { dry_run: true }
        );
    }

    #[test]
    fn project_and_data_dir_parse() {
        let cli = parse(&[
            "mcp",
            "--no-daemon",
            "--project",
            "/w/a",
            "--data-dir",
            "/d",
        ])
        .unwrap();
        assert_eq!(cli.data_dir, Some(PathBuf::from("/d")));
        assert_eq!(
            cli.command,
            Command::Mcp {
                profile: Profile::Use,
                no_daemon: true,
                project: Some(PathBuf::from("/w/a"))
            }
        );
        assert_eq!(parse(&["status"]).unwrap().data_dir, None);
    }

    #[test]
    fn invalid_input_is_rejected() {
        assert_eq!(
            parse(&["frobnicate"]).unwrap_err().kind(),
            ErrorKind::InvalidSubcommand
        );
        assert_eq!(
            parse(&["mcp", "--profile", "admin"]).unwrap_err().kind(),
            ErrorKind::InvalidValue
        );
        assert_eq!(
            parse(&["status", "--bogus"]).unwrap_err().kind(),
            ErrorKind::UnknownArgument
        );
    }

    #[test]
    fn no_command_shows_help() {
        assert_eq!(
            parse(&[]).unwrap_err().kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }

    #[test]
    fn version_names_the_engine_revision() {
        let err = parse(&["--version"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
        assert!(LONG_VERSION.starts_with(env!("CARGO_PKG_VERSION")));
        assert!(LONG_VERSION.contains(crate::engine::engine_rev_short!()));
    }

    #[test]
    fn help_mentions_every_command_and_the_pre_release() {
        let help = Cli::command().render_long_help().to_string();
        for name in [
            "install",
            "uninstall",
            "status",
            "doctor",
            "mcp",
            "daemon",
            "ui",
            "bench",
            "import",
            "export",
        ] {
            assert!(help.contains(name), "help lacks {name}");
        }
        assert!(help.contains("PRE-RELEASE"));
    }

    #[test]
    fn install_and_uninstall_options_parse() {
        assert_eq!(
            parse(&["install", "--yes", "--with-model", "--enable-develop"])
                .unwrap()
                .command,
            Command::Install {
                harness: Harness::ClaudeCode,
                dry_run: false,
                yes: true,
                with_model: true,
                enable_develop: true
            }
        );
        assert_eq!(
            parse(&["uninstall", "-y", "--purge"]).unwrap().command,
            Command::Uninstall {
                harness: Harness::ClaudeCode,
                dry_run: false,
                yes: true,
                purge: true,
                disable_develop: false
            }
        );
        assert_eq!(
            parse(&["uninstall", "--purge", "--disable-develop"])
                .unwrap_err()
                .kind(),
            ErrorKind::ArgumentConflict
        );
        assert_eq!(
            parse(&["hook", "session-start"]).unwrap().command,
            Command::Hook {
                event: crate::hook::HookEvent::SessionStart
            }
        );
        let help = Cli::command().render_long_help().to_string();
        assert!(!help.contains("\n  hook"), "the hook command is hidden");
    }

    #[test]
    fn daemon_options_parse() {
        assert_eq!(
            parse(&["daemon", "--idle", "90s", "--foreground"])
                .unwrap()
                .command,
            Command::Daemon {
                idle: Some(std::time::Duration::from_secs(90)),
                foreground: true,
                version_as: None
            }
        );
        assert_eq!(
            parse(&["daemon", "--idle", "soon"]).unwrap_err().kind(),
            ErrorKind::ValueValidation
        );
        let cli = parse(&["status", "--run-dir", "/r"]).unwrap();
        assert_eq!(cli.run_dir, Some(PathBuf::from("/r")));
    }
}
