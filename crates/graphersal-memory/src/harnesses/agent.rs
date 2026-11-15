//! The harness abstraction: what `install`, `uninstall`, `import` and `export` need to know
//! about one AI agent harness, behind one interface. Claude Code (`crate::harnesses::claude_code::ClaudeCode`), opencode
//! (`crate::harnesses::opencode::OpenCode`), Codex (`crate::harnesses::codex::Codex`), Cursor
//! (`crate::harnesses::cursor::Cursor`), CodeWhale (`crate::harnesses::codewhale::CodeWhale`) and
//! DeepSeek Harness (`crate::harnesses::dsh::Dsh`) implement it;
//! `crate::harnesses` is the one place that names them all. Shared code asks the [`Harness`]
//! for its program, the files it edits, the JSON keys of its MCP registrations and the names
//! the wizard shows, and never matches on its [`Kind`].

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::install::{Change, Finding, HarnessPlan, PlanInput, Setup, State};

/// The harnesses graphersal-memory supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Kind {
    /// Claude Code.
    #[default]
    ClaudeCode,
    /// opencode.
    OpenCode,
    /// OpenAI Codex CLI.
    Codex,
    /// Cursor.
    Cursor,
    /// CodeWhale.
    CodeWhale,
    /// DeepSeek Harness (`dsh`).
    Dsh,
}

impl Kind {
    /// The default harness (what install sets up when nothing else is found), as a constant.
    pub const DEFAULT: Kind = Kind::ClaudeCode;

    /// Every supported harness, in the order the wizard lists them.
    pub const ALL: [Kind; 6] = [
        Kind::ClaudeCode,
        Kind::OpenCode,
        Kind::Codex,
        Kind::Cursor,
        Kind::CodeWhale,
        Kind::Dsh,
    ];

    /// The id of the command line and the state files (`claude-code`, `opencode`).
    pub fn id(self) -> &'static str {
        match self {
            Kind::ClaudeCode => "claude-code",
            Kind::OpenCode => "opencode",
            Kind::Codex => "codex",
            Kind::Cursor => "cursor",
            Kind::CodeWhale => "codewhale",
            Kind::Dsh => "dsh",
        }
    }

    /// The name of its program, for messages (`claude`, `codewhale`).
    pub fn program_name(self) -> &'static str {
        match self {
            Kind::ClaudeCode => "claude",
            Kind::OpenCode => "opencode",
            Kind::Codex => "codex",
            Kind::Cursor => "cursor",
            Kind::CodeWhale => "codewhale",
            Kind::Dsh => "dsh",
        }
    }

    /// The directory names of its files, to tell which harness an error is about.
    pub fn dir_names(self) -> &'static [&'static str] {
        match self {
            Kind::ClaudeCode => &[".claude"],
            Kind::OpenCode => &["opencode"],
            Kind::Codex => &[".codex", "codex"],
            Kind::Cursor => &[".cursor"],
            Kind::CodeWhale => &[".codewhale", ".deepseek", ".agents"],
            Kind::Dsh => &[".dsh"],
        }
    }

    /// Its line in the command line's help (`--harness`).
    pub fn cli_help(self) -> &'static str {
        match self {
            Kind::ClaudeCode => "Claude Code",
            Kind::OpenCode => "opencode (1.x or 2.x: `opencode --version` says which)",
            Kind::Codex => "OpenAI Codex CLI",
            Kind::Cursor => "Cursor (the editor's agent)",
            Kind::CodeWhale => "CodeWhale (the terminal agent, formerly DeepSeek TUI; preview)",
            Kind::Dsh => "DeepSeek Harness (`dsh`, the official DeepSeek agent; preview)",
        }
    }

    /// The name people know.
    pub fn display_name(self) -> &'static str {
        match self {
            Kind::ClaudeCode => "Claude Code",
            Kind::OpenCode => "opencode",
            Kind::Codex => "Codex",
            Kind::Cursor => "Cursor",
            Kind::CodeWhale => "CodeWhale",
            Kind::Dsh => "DeepSeek Harness",
        }
    }

    /// The harness of an id.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == id)
    }
}

/// A harness's own memory as it is now ([`OwnMemory::now`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnMemoryNow {
    /// It is on.
    pub on: bool,
    /// The wizard asks about it.
    pub askable: bool,
    /// The user switched it off themselves (not install).
    pub off_by_user: bool,
    /// An earlier install recorded an answer for it (its state's `auto_memory_off`).
    pub answered: bool,
}

/// The wizard's question about a harness's own memory.
pub struct OwnMemoryQuestion {
    /// The step's title.
    pub title: String,
    /// What the step explains.
    pub lines: Vec<String>,
    /// The answer that switches it off.
    pub yes: String,
    /// The answer that keeps it on.
    pub no: String,
}

/// A memory of the harness's own that install can switch off (and uninstall on again): Claude
/// Code's auto-memory, Codex's memories, CodeWhale's memory. One wizard question answers for
/// every chosen harness: the first asks it, the others add [`OwnMemory::also`].
pub trait OwnMemory {
    /// How it is now.
    fn now(&self, setup: &Setup) -> Result<OwnMemoryNow, String>;
    /// The question when this harness asks it (`start_sentence`: how the always-loaded
    /// memories reach a session).
    fn question(&self, setup: &Setup, start_sentence: &str) -> OwnMemoryQuestion;
    /// The lines another harness's question adds: the same answer switches this one too.
    fn also(&self, setup: &Setup) -> Vec<String>;
    /// The title of its line in the wizard's recap, when it has one.
    fn recap_title(&self) -> Option<&'static str> {
        None
    }
}

/// A store of the harness's own memory whose entries install imports, reading it only
/// (CodeWhale's).
pub trait NativeMemory {
    /// Its name in the wizard and the report (`CodeWhale's memory`).
    fn name(&self) -> &'static str;
    /// How many entries are new or changed since their import.
    fn pending(&self, data_dir: &Path) -> usize;
    /// What it holds and what of it was imported.
    fn describe(&self, data_dir: &Path) -> String;
    /// Whether the store could not be read.
    fn failed(&self) -> bool;
    /// Imports its new and changed entries.
    fn import(
        &self,
        connect: &mut crate::memory::import::Connect<'_>,
        data_dir: &Path,
    ) -> NativeImport;
}

/// What an import of a native memory did.
pub struct NativeImport {
    /// The report line.
    pub line: String,
    /// Changed entries whose memory was edited in graphersal-memory (kept), one line each.
    pub kept_edited: Vec<String>,
    /// Failures.
    pub problems: Vec<String>,
}

/// A harness's hooks of ours (`graphersal-memory hook ... --harness <id>`): what one does with
/// the harness's input. Always exit 0: a hook never blocks a session.
pub trait HookHandler {
    /// Runs the hook of `event` on `input` (read already).
    fn run(
        &self,
        setup: &Setup,
        event: crate::memory::hook::HookEvent,
        input: Vec<u8>,
        stdout: &mut dyn std::io::Write,
        stderr: &mut dyn std::io::Write,
    ) -> u8;
}

/// One harness, as the installation sees it.
pub trait Harness {
    /// Which harness this is.
    fn kind(&self) -> Kind;
    /// Its program on this machine: on `search` (`None`: this process's `PATH`, and then the
    /// system-wide install locations too), or in a known install location under `home` that
    /// is not on it.
    fn find_program(&self, home: &Path, search: Option<&OsStr>) -> Option<PathBuf>;
    /// Its configuration directories or files: one that exists without the program is a
    /// leftover (`Setup::leftovers`).
    fn config_roots(&self) -> Vec<PathBuf>;
    /// Whether a part of ours (an MCP entry, a hook, the rule block, ...) is in its files.
    fn has_our_parts(&self, setup: &Setup) -> bool;
    /// Its part of an install: steps, file changes, conflicts and the records that go with
    /// them.
    fn plan(&self, setup: &Setup, input: &PlanInput<'_>) -> Result<HarnessPlan, String>;
    /// What uninstall changes in its files (`develop_only`: only the develop server goes),
    /// each change with what it is; parts it leaves are added to `conflicts`.
    fn removal(
        &self,
        setup: &Setup,
        state: &State,
        develop_only: bool,
        conflicts: &mut Vec<String>,
    ) -> Result<Vec<(Change, &'static str)>, String>;
    /// Whether its develop server is registered and on, as it is now.
    fn develop_on(&self, setup: &Setup) -> bool;
    /// What the user still has to do in the harness for the install to work fully (Codex:
    /// trust the hooks), as one line; `None` when nothing.
    fn action_needed(&self, _setup: &Setup) -> Option<String> {
        None
    }
    /// Its own memory, when it has one install can switch off.
    fn own_memory(&self) -> Option<&dyn OwnMemory> {
        None
    }
    /// Its own memory's store, when install imports it (`project`: the current directory's).
    fn native_memory(
        &self,
        _setup: &Setup,
        _project: Option<&Path>,
    ) -> Option<Box<dyn NativeMemory>> {
        None
    }
    /// What it tells about other memory beside graphersal-memory (M15).
    fn other_memory(&self) -> Option<&dyn crate::install::coexist::OtherMemory> {
        None
    }
    /// Its `hook session-start|session-end --harness <id>`, when it runs hooks of ours.
    fn hook(&self) -> Option<&dyn HookHandler> {
        None
    }
    /// Its memory directories with their projects, when it has a file memory of its own
    /// (`has_file_memory`; the import reads them, `crate::memory::import`).
    fn memory_dirs(&self) -> Result<Vec<crate::memory::import::MemoryDir>, String> {
        Ok(Vec::new())
    }
    /// Whether its own start hook delivers the start context now (then the MCP instructions
    /// leave it out: Claude Code's SessionStart hook).
    fn start_hook_installed(&self, _setup: &Setup) -> bool {
        false
    }
    /// For a harness without file memory: how to give the agent an exported file.
    fn export_hint(&self) -> &'static str {
        "to give the agent a file, add its path to `instructions` in the agent's configuration"
    }
    /// Writes the harness's configuration as a user has it into `home` (their own entries,
    /// comments, rules): what the conformance tests (`crate::harnesses::conformance_tests`) start from.
    #[cfg(test)]
    fn user_files(&self, home: &Path);
    /// The text of a fake program of this harness that its `find_program` accepts (tests; most
    /// harnesses accept any file of the program's name).
    #[cfg(test)]
    fn fake_program(&self) -> &'static str {
        ""
    }
    /// What the bench needs of it, when the bench runs through it (`crate::bench`).
    fn bench(&self) -> Option<std::sync::Arc<dyn crate::bench::BenchSupport>> {
        None
    }
    /// Whether its sessions are on this machine (its transcripts directory, database or
    /// registry).
    fn has_sessions(&self) -> bool;
    /// Whether it gets a session plugin rather than hooks (opencode).
    fn has_plugin(&self) -> bool {
        false
    }
    /// Its lines of `status`: each part of ours and its state.
    fn status_lines(&self, setup: &Setup) -> Vec<String>;
    /// Its findings of `doctor` (`project`: the current directory's, for checks of a project's
    /// own files).
    fn doctor(&self, setup: &Setup, project: Option<&Path>) -> Vec<Finding>;
    /// When install switched the harness's own memory off (its state records it): the file
    /// uninstall switches it back on in, and the report line.
    fn own_memory_restored(&self, _setup: &Setup, _state: &State) -> Option<(PathBuf, String)> {
        None
    }
    /// The harness id of the command line (`claude-code`, `opencode`, `codex`).
    fn id(&self) -> &'static str {
        self.kind().id()
    }
    /// The name people know (`Claude Code`, `opencode`).
    fn display_name(&self) -> &'static str {
        self.kind().display_name()
    }
    /// The file holding the user-scope MCP server registrations.
    fn mcp_config_file(&self) -> &Path;
    /// The object of the MCP registrations in that file (`mcpServers`, `mcp`; opencode 2.x
    /// keeps them one level deeper, in `mcp.servers`).
    fn mcp_servers_key(&self) -> &'static str;
    /// The MCP registration that starts `exe` with `args` (a local stdio server).
    fn mcp_entry(&self, exe: &Path, args: &[String]) -> Value;
    /// How to remove a registration of another program under `name` by hand.
    fn remove_server_hint(&self, name: &str) -> String;
    /// The user settings file (hooks, the auto-memory switch).
    fn settings_file(&self) -> &Path;
    /// The user instruction file every session loads (the managed rule block goes there).
    fn instruction_file(&self) -> &Path;
    /// Where the harness keeps its sessions (Claude Code: the per-project transcripts and file
    /// memory; opencode: its data directory with `opencode.db`).
    fn projects_dir(&self) -> &Path;
    /// Where the export puts the memories of the project rooted at `root`: the harness's
    /// file-memory directory (Claude Code), or a directory of readable Markdown files
    /// (opencode, which has no file memory).
    fn memory_dir(&self, root: &Path) -> PathBuf;
    /// Where the export puts the global memories: a file every session of every project loads
    /// (Claude Code), or a Markdown file beside the projects' (opencode).
    fn global_memory_file(&self) -> PathBuf;
    /// Whether the harness has a file memory of its own (one the import reads and the export
    /// writes back into).
    fn has_file_memory(&self) -> bool;
}

/// Whether an MCP entry is one of ours: it starts a `graphersal-memory` binary with `mcp`
/// (Claude Code: `command` + `args`; opencode: `command` as one array).
#[cfg(test)]
pub fn is_our_entry(entry: &Value) -> bool {
    let (command, first) = match &entry["command"] {
        Value::Array(parts) => (
            parts.first().and_then(Value::as_str).unwrap_or_default(),
            parts.get(1).cloned().unwrap_or(Value::Null),
        ),
        other => (other.as_str().unwrap_or_default(), entry["args"][0].clone()),
    };
    is_our_program(command) && first == "mcp"
}

/// Whether `command` starts a `graphersal-memory` binary (its file name, on any platform: an
/// entry may come from another system's path style).
pub fn is_our_program(command: &str) -> bool {
    let name = command.rsplit(['/', '\\']).next().unwrap_or_default();
    let name = name.strip_suffix(".exe").unwrap_or(name);
    name == "graphersal-memory"
}

/// The program an MCP entry starts (either shape).
pub fn entry_program(entry: &Value) -> &str {
    match &entry["command"] {
        Value::Array(parts) => parts.first().and_then(Value::as_str).unwrap_or_default(),
        other => other.as_str().unwrap_or_default(),
    }
}
