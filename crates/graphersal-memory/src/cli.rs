//! Command-line interface: the command set of the product and its help text.
//!
//! Every command exists from the first stage on, so the help text shows the whole product; a
//! command that is not built yet says so and names the stage that delivers it
//! (`crate::commands`).

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// The long `--version` text: the product version and the engine version it is built on.
const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (pre-release; engine: graphersal ",
    crate::engine::engine_version!(),
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
        PRE-RELEASE: `install`, `uninstall`, `status`, `doctor`, `import`, `summaries` and \
        `bench` (for Claude Code, opencode 1.x and 2.x, and Codex; Cursor without summaries and \
        bench), `mcp` (the stdio server a harness starts; it shares one background daemon \
        with every other session) and `daemon` work. Every other command says which stage delivers it; \
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
#[derive(Debug, Subcommand, PartialEq)]
pub enum Command {
    /// Set up graph memory for an agent harness: a short wizard (memory language, embedding
    /// model, import of the existing memory, the harness's own memory, summaries, the develop
    /// server), then the MCP servers, hooks and the managed rule block.
    #[command(long_about = "Set up graph memory for an agent harness.\n\n\
            In a terminal, install asks a few questions (Step N of M; Enter takes the default) \
            and shows the full plan before it changes anything. With --yes, or without a \
            terminal, every question takes its default (or the flag that answers it; on an \
            installed system: the current answer); without --yes it still asks once before \
            applying. Running install again changes only what differs.\n\n\
            The embedding model download (Hugging Face, once, into the data directory) is the \
            one network access of install; if it fails, the built-in embedder stays.")]
    Install {
        /// The harness(es) to set up: claude-code, opencode, codex, cursor or all (default: the
        /// wizard asks, offering every harness whose program is found on this machine; without
        /// questions, and with all: every harness found).
        #[arg(long, value_enum)]
        harness: Option<Harness>,
        /// Show what would change without changing anything (no questions).
        #[arg(long)]
        dry_run: bool,
        /// Do not ask: every question takes its default or its flag.
        #[arg(long, short = 'y')]
        yes: bool,
        /// The memory language: english (default: stored and searched in English) or any
        /// (stored in the language of the conversation).
        #[arg(long, value_enum, value_name = "LANGUAGE")]
        language: Option<LanguageArg>,
        /// The embedding model: a Hugging Face repository of a Model2Vec model (default:
        /// minishlab/potion-retrieval-32M for English, minishlab/potion-multilingual-128M for
        /// any language) or `builtin` (no download, weaker search).
        #[arg(long, value_name = "REPO|builtin", value_parser = parse_model)]
        model: Option<String>,
        /// Do not import the existing file memory now.
        #[arg(long)]
        no_import: bool,
        /// Keep the harness's own auto-memory on (default: switched off; uninstall switches it
        /// back on).
        #[arg(long)]
        keep_auto_memory: bool,
        /// Also register the develop MCP server (structure tools; off by default).
        #[arg(long)]
        enable_develop: bool,
        /// Switch session summaries on without asking (they run through your own harness and
        /// cost model usage; off by default, also with --yes).
        #[arg(long)]
        summaries: bool,
    },
    /// Remove exactly what `install` added; your own settings stay untouched.
    Uninstall {
        /// The harness(es) to remove graph memory from: claude-code, opencode, codex, cursor or all
        /// (default and all: every harness it is installed in).
        #[arg(long, value_enum)]
        harness: Option<Harness>,
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
        /// Do not write the memories back into the harness's file memory first (default: every
        /// memory is exported before anything is removed).
        #[arg(long, conflicts_with = "disable_develop")]
        no_export: bool,
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
    /// Measure whether the memory helps: the built-in A/B on your own data (through
    /// `summaries.harness_command`: Claude Code's `claude -p`, opencode's `opencode run` or
    /// Codex's `codex exec --oss` with a local model).
    #[command(args_conflicts_with_subcommands = true)]
    Bench {
        /// `questions`: generate, review or add to the question set only.
        #[command(subcommand)]
        action: Option<BenchAction>,
        /// The arms to compare, comma-separated: none (no memory), files (your file memory),
        /// graph (graphersal-memory on a copy of your store).
        #[arg(long, value_enum, value_delimiter = ',', default_values_t = [BenchArm::None, BenchArm::Files, BenchArm::Graph])]
        arms: Vec<BenchArm>,
        /// How many generated questions to add to your own (generated when the project has
        /// none yet); 0 asks only your own questions (`bench questions add`).
        #[arg(long, value_name = "N", default_value_t = 12, value_parser = clap::value_parser!(u32).range(0..=60))]
        questions: u32,
        /// How often every question is answered per arm (the spread between runs).
        #[arg(long, value_name = "R", default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=10))]
        repeats: u32,
        /// The most this run may spend, USD at list price: a run whose estimate is higher is
        /// refused, and the run stops when its real spend reaches it.
        #[arg(long, value_name = "USD", default_value_t = 3.0, value_parser = parse_budget)]
        budget: f64,
        /// The project to measure (default: the git repository root of the current directory).
        #[arg(long, value_name = "ROOT")]
        project: Option<PathBuf>,
        /// Read the memory files, instruction files and transcripts from this Claude Code
        /// configuration directory instead of your own (the harness keeps its own).
        #[arg(long, value_name = "DIR")]
        claude_dir: Option<PathBuf>,
        /// Plan the run and build the sandboxes, with a fake harness that calls no model and
        /// spends nothing.
        #[arg(long)]
        dry_run: bool,
        /// Do not ask: accept the estimate and an unconfirmed question set.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Import the harness's file memory (MEMORY.md and the memory files; new and changed files
    /// only), or with --sessions its past sessions.
    Import {
        /// Show what would be imported without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Index the finished past sessions instead (in the daemon, with progress; summaries
        /// only when switched on).
        #[arg(long)]
        sessions: bool,
    },
    /// Session summaries through your own harness: estimate, status, enable, disable.
    Summaries {
        /// What to do.
        #[command(subcommand)]
        action: SummariesAction,
    },
    /// Export the memories back into the harness's file memory (--to-files; what uninstall
    /// does first; opencode, Codex and Cursor, which have none: readable Markdown files under
    /// ~/graphersal-memory-export/<harness>). The export of rules into instruction files is
    /// planned.
    Export {
        /// The harness to export for.
        #[arg(long, value_enum, default_value_t = Harness::ClaudeCode)]
        harness: Harness,
        /// Show what would be written without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Write every memory as the harness's memory files (MEMORY.md and one file per
        /// memory per project; the global ones into one file every project loads). A file
        /// that differs is kept and the graph's version written beside it.
        #[arg(long)]
        to_files: bool,
        /// Only this project (its root directory) and the global memories.
        #[arg(long, value_name = "ROOT", requires = "to_files")]
        project: Option<PathBuf>,
    },
    /// The projects the memory knows (their ids and lookup keys), and merging one into another.
    Projects {
        /// What to do.
        #[command(subcommand)]
        action: ProjectsAction,
    },
    /// Print the start context: the memories put into the start of every session of a project
    /// (every pinned memory, every feedback and user memory, of the project and global),
    /// exactly as the agent gets it.
    StartContext {
        /// The project (a directory in it; default: the current directory).
        #[arg(long, value_name = "DIR")]
        project: Option<PathBuf>,
    },
    /// Run a harness hook (the hooks `install` adds call this; not for you).
    #[command(hide = true)]
    Hook {
        /// The hook event.
        #[arg(value_enum)]
        event: crate::hook::HookEvent,
        /// The harness that runs the hook (Claude Code's, Codex's or Cursor's hooks, opencode's
        /// plugin).
        #[arg(long, value_enum, default_value_t = Harness::ClaudeCode)]
        harness: Harness,
    },
}

/// What `projects` does.
#[derive(Debug, Subcommand, PartialEq)]
pub enum ProjectsAction {
    /// List the projects: id, paths, remotes, root commit, and look-alikes that may have moved.
    List,
    /// Move every memory, session and fact of project FROM into project INTO (both an id or a
    /// path `projects list` shows), and FROM's lookup keys with them; FROM is gone afterwards.
    Merge {
        /// The project that goes away.
        from: String,
        /// The project that stays.
        into: String,
        /// Do not ask.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

/// The memory language of `install --language`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LanguageArg {
    /// Store and search in English.
    English,
    /// Store in the language of the conversation.
    Any,
}

impl LanguageArg {
    /// The language.
    pub fn language(self) -> crate::language::Language {
        match self {
            LanguageArg::English => crate::language::Language::English,
            LanguageArg::Any => crate::language::Language::Any,
        }
    }
}

/// Parses `--model`: a repository name, or `builtin` (stored as `hash-ngram/1`).
fn parse_model(text: &str) -> Result<String, String> {
    if text == "builtin" || text == crate::models::BUILTIN {
        Ok(crate::models::BUILTIN.to_string())
    } else if crate::download::is_repo_name(text) && text.contains('/') {
        Ok(text.to_string())
    } else {
        Err(format!(
            "'{text}' is neither `builtin` nor a Hugging Face repository (owner/name, for \
             example minishlab/potion-retrieval-32M)"
        ))
    }
}

/// What `bench questions` does.
#[derive(Debug, Subcommand, PartialEq)]
pub enum BenchAction {
    /// Generate a new question set from your raw transcripts (asks before spending; your own
    /// questions are kept and their missing references looked up), with --review show the
    /// current one and confirm it, or add your own questions (`add`, `import`, `remove`).
    #[command(args_conflicts_with_subcommands = true)]
    Questions {
        /// Add, import or remove your own questions (no model is called).
        #[command(subcommand)]
        edit: Option<QuestionsEdit>,
        /// How many questions to generate (0: only look up the references of your own).
        #[arg(long, value_name = "N", default_value_t = 12, value_parser = clap::value_parser!(u32).range(0..=60))]
        questions: u32,
        /// Show the current question set with its references and sources, and confirm it
        /// (edit the file first to fix or drop questions).
        #[arg(long)]
        review: bool,
        /// The most the generation may spend, USD at list price.
        #[arg(long, value_name = "USD", default_value_t = 3.0, value_parser = parse_budget)]
        budget: f64,
        /// The project (default: the git repository root of the current directory).
        #[arg(long, value_name = "ROOT")]
        project: Option<PathBuf>,
        /// Read the transcripts from this Claude Code configuration directory instead of yours.
        #[arg(long, value_name = "DIR")]
        claude_dir: Option<PathBuf>,
        /// Generate with a fake harness (no model, no spend) and keep the result out of the
        /// project's question set.
        #[arg(long)]
        dry_run: bool,
        /// Do not ask.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

/// What `bench questions add|import|remove` does.
#[derive(Debug, Subcommand, PartialEq)]
pub enum QuestionsEdit {
    /// Add a question of your own: something you want the assistant to remember.
    Add {
        /// The question, as you would ask it.
        question: String,
        /// The right answer (the judge scores against it); without it, the bench looks the
        /// answer up in your past sessions.
        #[arg(long, value_name = "TEXT")]
        answer: Option<String>,
        /// The category in the report (default: other).
        #[arg(long, value_enum)]
        category: Option<QuestionCategory>,
        /// The project (default: the git repository root of the current directory).
        #[arg(long, value_name = "ROOT")]
        project: Option<PathBuf>,
    },
    /// Add the questions of a file: one per line, `Q:`/`A:` pairs (an answer under its
    /// question), or JSON entries like the question file's; duplicates are skipped.
    Import {
        /// The file.
        file: PathBuf,
        /// The project (default: the git repository root of the current directory).
        #[arg(long, value_name = "ROOT")]
        project: Option<PathBuf>,
    },
    /// Remove a question by its id (U2, Q3; `bench questions --review` lists them).
    Remove {
        /// The question's id.
        id: String,
        /// The project (default: the git repository root of the current directory).
        #[arg(long, value_name = "ROOT")]
        project: Option<PathBuf>,
    },
}

/// The category of a question of your own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum QuestionCategory {
    /// A standing instruction (how to work, what never to do).
    #[value(name = "rules", alias = "rule")]
    Rules,
    /// A choice made, with its reason.
    #[value(name = "decisions", alias = "decision")]
    Decisions,
    /// What happened in a session.
    History,
}

impl QuestionCategory {
    /// The category's name in the question file.
    pub fn name(self) -> &'static str {
        match self {
            QuestionCategory::Rules => "rule",
            QuestionCategory::Decisions => "decision",
            QuestionCategory::History => "history",
        }
    }
}

/// One arm of the A/B.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum BenchArm {
    /// No memory at all.
    None,
    /// Your file memory as it is: CLAUDE.md files, MEMORY.md and the memory files.
    Files,
    /// graphersal-memory's `use` tools on a copy of your store.
    Graph,
    /// Your current MCP setup copied (planned: not built yet).
    Current,
}

impl BenchArm {
    /// The arm's name in reports.
    pub fn name(self) -> &'static str {
        match self {
            BenchArm::None => "none",
            BenchArm::Files => "files",
            BenchArm::Graph => "graph",
            BenchArm::Current => "current",
        }
    }
}

/// What `summaries` does.
#[derive(Debug, Subcommand, PartialEq)]
pub enum SummariesAction {
    /// Estimate the cost of summarizing the sessions without a summary (calls no model).
    Estimate,
    /// Show whether summaries are on, the month's spend and the sessions waiting.
    Status,
    /// Switch summaries on (shows the estimate and asks unless --yes).
    Enable {
        /// The monthly budget in USD (default: the settings file's, else 5).
        #[arg(long, value_name = "USD", value_parser = parse_budget)]
        budget: Option<f64>,
        /// Do not ask.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Switch summaries off (raw turns are still indexed).
    Disable,
}

/// Parses `--budget`.
fn parse_budget(text: &str) -> Result<f64, String> {
    text.trim()
        .trim_start_matches('$')
        .parse::<f64>()
        .ok()
        .filter(|usd| usd.is_finite() && *usd >= 0.0)
        .ok_or_else(|| format!("'{text}' is not an amount of dollars (example: 5)"))
}

/// Parses `--idle`.
fn parse_idle(text: &str) -> Result<std::time::Duration, String> {
    crate::daemon::parse_duration(text)
}

/// An AI agent harness graphersal-memory can be installed into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Harness {
    /// Claude Code.
    ClaudeCode,
    /// opencode (1.x or 2.x: `opencode --version` says which).
    Opencode,
    /// OpenAI Codex CLI.
    Codex,
    /// Cursor (the editor's agent).
    Cursor,
    /// Every supported harness (install: those whose program is found on this machine;
    /// uninstall: those it is installed in).
    All,
}

impl Harness {
    /// The supported harnesses this choice names (`None` for a planned one).
    pub fn kinds(self) -> Option<Vec<crate::agents::Kind>> {
        use crate::agents::Kind;
        match self {
            Harness::ClaudeCode => Some(vec![Kind::ClaudeCode]),
            Harness::Opencode => Some(vec![Kind::OpenCode]),
            Harness::Codex => Some(vec![Kind::Codex]),
            Harness::All => Some(Kind::ALL.to_vec()),
            Harness::Cursor => Some(vec![Kind::Cursor]),
        }
    }
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
                harness: None,
                dry_run: false,
                yes: false,
                language: None,
                model: None,
                no_import: false,
                keep_auto_memory: false,
                enable_develop: false,
                summaries: false
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
                harness: Some(Harness::Cursor),
                dry_run: true,
                yes: false,
                purge: false,
                disable_develop: false,
                no_export: false
            }
        );
        assert_eq!(
            parse(&["bench", "--dry-run"]).unwrap().command,
            Command::Bench {
                action: None,
                arms: vec![BenchArm::None, BenchArm::Files, BenchArm::Graph],
                questions: 12,
                repeats: 1,
                budget: 3.0,
                project: None,
                claude_dir: None,
                dry_run: true,
                yes: false
            }
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
    fn version_names_the_engine_version() {
        let err = parse(&["--version"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
        assert!(LONG_VERSION.starts_with(env!("CARGO_PKG_VERSION")));
        assert!(LONG_VERSION.contains(concat!("graphersal ", crate::engine::engine_version!())));
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
            parse(&["install", "--yes", "--model", "builtin", "--enable-develop"])
                .unwrap()
                .command,
            Command::Install {
                harness: None,
                dry_run: false,
                yes: true,
                language: None,
                model: Some(crate::models::BUILTIN.to_string()),
                no_import: false,
                keep_auto_memory: false,
                enable_develop: true,
                summaries: false
            }
        );
        assert_eq!(
            parse(&["uninstall", "-y", "--purge"]).unwrap().command,
            Command::Uninstall {
                harness: None,
                dry_run: false,
                yes: true,
                purge: true,
                disable_develop: false,
                no_export: false
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
                event: crate::hook::HookEvent::SessionStart,
                harness: Harness::ClaudeCode
            }
        );
        let help = Cli::command().render_long_help().to_string();
        assert!(!help.contains("\n  hook"), "the hook command is hidden");
    }

    #[test]
    fn session_and_summary_options_parse() {
        assert_eq!(
            parse(&["import", "--sessions"]).unwrap().command,
            Command::Import {
                dry_run: false,
                sessions: true
            }
        );
        assert_eq!(
            parse(&["summaries", "enable", "--budget", "$2.5", "-y"])
                .unwrap()
                .command,
            Command::Summaries {
                action: SummariesAction::Enable {
                    budget: Some(2.5),
                    yes: true
                }
            }
        );
        for action in ["estimate", "status", "disable"] {
            assert!(parse(&["summaries", action]).is_ok(), "{action}");
        }
        assert!(parse(&["summaries"]).is_err());
        assert_eq!(
            parse(&["summaries", "enable", "--budget=-1"])
                .unwrap_err()
                .kind(),
            ErrorKind::ValueValidation
        );
        match parse(&["install", "--yes", "--summaries"]).unwrap().command {
            Command::Install { summaries, yes, .. } => assert!(summaries && yes),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bench_options_parse() {
        match parse(&[
            "bench",
            "--arms",
            "files,graph",
            "--questions",
            "6",
            "--repeats",
            "2",
            "--budget",
            "$4",
            "--yes",
        ])
        .unwrap()
        .command
        {
            Command::Bench {
                arms,
                questions,
                repeats,
                budget,
                yes,
                ..
            } => {
                assert_eq!(arms, [BenchArm::Files, BenchArm::Graph]);
                assert_eq!((questions, repeats, yes), (6, 2, true));
                assert!((budget - 4.0).abs() < 1e-9);
            }
            other => panic!("{other:?}"),
        }
        match parse(&["bench", "questions", "--review", "-y"])
            .unwrap()
            .command
        {
            Command::Bench {
                action: Some(BenchAction::Questions { review, yes, .. }),
                ..
            } => assert!(review && yes),
            other => panic!("{other:?}"),
        }
        assert!(parse(&["bench", "--arms", "files,mem0"]).is_err());
        // 0 generated questions: only the user's own are asked.
        match parse(&["bench", "--questions", "0"]).unwrap().command {
            Command::Bench { questions, .. } => assert_eq!(questions, 0),
            other => panic!("{other:?}"),
        }
        assert!(parse(&["bench", "--questions", "61"]).is_err());
        assert!(parse(&["bench", "--budget", "-1"]).is_err());
    }

    #[test]
    fn bench_questions_add_import_remove_parse() {
        let edit = |args: &[&str]| match parse(args).unwrap().command {
            Command::Bench {
                action: Some(BenchAction::Questions { edit, .. }),
                ..
            } => edit,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            edit(&[
                "bench",
                "questions",
                "add",
                "Which database?",
                "--answer",
                "SQLite",
                "--category",
                "decisions",
            ]),
            Some(QuestionsEdit::Add {
                question: "Which database?".into(),
                answer: Some("SQLite".into()),
                category: Some(QuestionCategory::Decisions),
                project: None,
            })
        );
        assert_eq!(
            edit(&["bench", "questions", "add", "Q?", "--category", "rule"]),
            Some(QuestionsEdit::Add {
                question: "Q?".into(),
                answer: None,
                category: Some(QuestionCategory::Rules),
                project: None,
            })
        );
        assert_eq!(QuestionCategory::Rules.name(), "rule");
        assert_eq!(QuestionCategory::Decisions.name(), "decision");
        assert_eq!(QuestionCategory::History.name(), "history");
        assert_eq!(
            edit(&[
                "bench",
                "questions",
                "import",
                "mine.txt",
                "--project",
                "/w/p"
            ]),
            Some(QuestionsEdit::Import {
                file: PathBuf::from("mine.txt"),
                project: Some(PathBuf::from("/w/p")),
            })
        );
        assert_eq!(
            edit(&["bench", "questions", "remove", "U2"]),
            Some(QuestionsEdit::Remove {
                id: "U2".into(),
                project: None,
            })
        );
        assert_eq!(edit(&["bench", "questions", "--questions", "0"]), None);
        assert!(parse(&["bench", "questions", "add"]).is_err());
        assert!(parse(&["bench", "questions", "add", "Q?", "--category", "gossip"]).is_err());
        // An edit takes no generation options.
        assert!(parse(&["bench", "questions", "--review", "add", "Q?"]).is_err());
    }

    #[test]
    fn wizard_flags_export_and_projects_parse() {
        match parse(&[
            "install",
            "--language",
            "any",
            "--model",
            "acme/model-1",
            "--no-import",
            "--keep-auto-memory",
        ])
        .unwrap()
        .command
        {
            Command::Install {
                language,
                model,
                no_import,
                keep_auto_memory,
                ..
            } => {
                assert_eq!(language, Some(LanguageArg::Any));
                assert_eq!(model.as_deref(), Some("acme/model-1"));
                assert!(no_import && keep_auto_memory);
            }
            other => panic!("{other:?}"),
        }
        assert!(parse(&["install", "--model", "not a repo"]).is_err());
        assert!(parse(&["install", "--model", "plain"]).is_err());
        assert!(
            parse(&["install", "--with-model"]).is_err(),
            "--with-model became --model"
        );
        assert!(parse(&["install", "--language", "klingon"]).is_err());
        match parse(&["export", "--to-files", "--project", "/w"])
            .unwrap()
            .command
        {
            Command::Export {
                to_files, project, ..
            } => assert!(to_files && project == Some(PathBuf::from("/w"))),
            other => panic!("{other:?}"),
        }
        assert!(parse(&["export", "--project", "/w"]).is_err());
        assert_eq!(
            parse(&["projects", "merge", "/a", "/b", "-y"])
                .unwrap()
                .command,
            Command::Projects {
                action: ProjectsAction::Merge {
                    from: "/a".into(),
                    into: "/b".into(),
                    yes: true
                }
            }
        );
        assert!(parse(&["projects", "list"]).is_ok());
        match parse(&["uninstall", "--no-export"]).unwrap().command {
            Command::Uninstall { no_export, .. } => assert!(no_export),
            other => panic!("{other:?}"),
        }
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

    #[test]
    fn uninstall_help_lists_the_four_harnesses_then_all() {
        let help = Cli::command()
            .find_subcommand_mut("uninstall")
            .unwrap()
            .render_long_help()
            .to_string();
        assert!(
            help.contains("claude-code, opencode, codex, cursor or all"),
            "{help}"
        );
        let at = |value: &str| help.find(&format!("- {value}:")).unwrap();
        assert!(
            at("claude-code") < at("opencode")
                && at("opencode") < at("codex")
                && at("codex") < at("cursor")
                && at("cursor") < at("all"),
            "{help}"
        );
    }

    #[test]
    fn harness_choices_parse() {
        use crate::agents::Kind;
        for (arg, kinds) in [
            ("claude-code", vec![Kind::ClaudeCode]),
            ("opencode", vec![Kind::OpenCode]),
            ("codex", vec![Kind::Codex]),
            ("cursor", vec![Kind::Cursor]),
            (
                "all",
                vec![Kind::ClaudeCode, Kind::OpenCode, Kind::Codex, Kind::Cursor],
            ),
        ] {
            match parse(&["install", "--harness", arg]).unwrap().command {
                Command::Install {
                    harness: Some(h), ..
                } => assert_eq!(h.kinds(), Some(kinds)),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(
            parse(&["hook", "session-end", "--harness", "opencode"])
                .unwrap()
                .command,
            Command::Hook {
                event: crate::hook::HookEvent::SessionEnd,
                harness: Harness::Opencode
            }
        );
        let help = Cli::command()
            .find_subcommand_mut("install")
            .unwrap()
            .render_long_help()
            .to_string();
        assert!(
            help.contains("claude-code, opencode, codex, cursor or all"),
            "{help}"
        );
    }
}
