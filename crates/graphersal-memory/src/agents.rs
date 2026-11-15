//! The harness abstraction: what `install`, `uninstall`, `import` and `export` need to know
//! about one AI agent harness, behind one interface. Claude Code ([`ClaudeCode`]), opencode
//! (`crate::opencode::OpenCode`), Codex (`crate::codex::Codex`), Cursor
//! (`crate::cursor::Cursor`) and CodeWhale (`crate::codewhale::CodeWhale`) implement it;
//! `crate::harnesses` is the one place that names them all. Shared code asks the [`Harness`]
//! for its program, the files it edits, the JSON keys of its MCP registrations and the names
//! the wizard shows, and never matches on its [`Kind`].

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::install::{Change, ClaudePaths, Finding, HarnessPlan, PlanInput, Setup, State};

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
}

impl Kind {
    /// Every supported harness, in the order the wizard lists them.
    pub const ALL: [Kind; 5] = [
        Kind::ClaudeCode,
        Kind::OpenCode,
        Kind::Codex,
        Kind::Cursor,
        Kind::CodeWhale,
    ];

    /// The id of the command line and the state files (`claude-code`, `opencode`).
    pub fn id(self) -> &'static str {
        match self {
            Kind::ClaudeCode => "claude-code",
            Kind::OpenCode => "opencode",
            Kind::Codex => "codex",
            Kind::Cursor => "cursor",
            Kind::CodeWhale => "codewhale",
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
        }
    }

    /// The harness of an id.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == id)
    }
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
    /// The settings key that switches the harness's own memory off (`false` = off), when it
    /// has one.
    fn auto_memory_key(&self) -> Option<&'static str>;
    /// The headless command line the summaries and the A/B run (a prompt on stdin); `None`
    /// when the harness has none graphersal-memory can use (Cursor).
    fn headless_command(&self) -> Option<&'static str>;
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

/// Claude Code (2.1.x): `~/.claude.json`, `~/.claude/settings.json`, `~/.claude/CLAUDE.md`,
/// `~/.claude/projects/<encoded root>/memory/`, `~/.claude/rules/` (or `$CLAUDE_CONFIG_DIR`).
#[derive(Debug, Clone)]
pub struct ClaudeCode {
    paths: ClaudePaths,
}

impl ClaudeCode {
    /// Claude Code with its files at `paths`.
    pub fn new(paths: ClaudePaths) -> Self {
        Self { paths }
    }
}

/// The file name of the exported global memories in Claude Code's user rules directory.
pub const GLOBAL_RULES_FILE: &str = "graphersal-memory-global.md";

impl Harness for ClaudeCode {
    fn kind(&self) -> Kind {
        Kind::ClaudeCode
    }

    /// On the search path, else `~/.local/bin/claude` (the native installer) or
    /// `~/.claude/local/claude` (the old local install).
    fn find_program(&self, home: &Path, search: Option<&OsStr>) -> Option<PathBuf> {
        let path = match search {
            Some(path) => Some(path.to_os_string()),
            None => std::env::var_os("PATH"),
        };
        let mut candidates: Vec<PathBuf> = path
            .iter()
            .flat_map(std::env::split_paths)
            .flat_map(|dir| ["claude", "claude.exe", "claude.cmd"].map(|name| dir.join(name)))
            .collect();
        candidates.push(home.join(".local").join("bin").join("claude"));
        candidates.push(home.join(".claude").join("local").join("claude"));
        candidates.into_iter().find(|p| p.is_file())
    }

    fn config_roots(&self) -> Vec<PathBuf> {
        vec![
            self.paths.config_dir.clone(),
            self.paths.user_config.clone(),
        ]
    }

    fn has_our_parts(&self, setup: &Setup) -> bool {
        !crate::install::inspect(setup).nothing_installed()
    }

    fn plan(&self, setup: &Setup, input: &PlanInput<'_>) -> Result<HarnessPlan, String> {
        crate::install::claude_plan(setup, input)
    }

    fn removal(
        &self,
        setup: &Setup,
        state: &State,
        develop_only: bool,
        conflicts: &mut Vec<String>,
    ) -> Result<Vec<(Change, &'static str)>, String> {
        crate::install::claude_removal(setup, develop_only, state, conflicts)
    }

    fn own_memory_restored(&self, setup: &Setup, state: &State) -> Option<(PathBuf, String)> {
        crate::install::claude_own_memory_restored(setup, state)
    }

    fn status_lines(&self, setup: &Setup) -> Vec<String> {
        crate::install::status_lines(setup)
    }

    fn doctor(&self, setup: &Setup, _project: Option<&Path>) -> Vec<Finding> {
        crate::install::claude_doctor(setup)
    }

    fn mcp_config_file(&self) -> &Path {
        &self.paths.user_config
    }

    fn mcp_servers_key(&self) -> &'static str {
        "mcpServers"
    }

    /// `{"type": "stdio", "command", "args", "env": {}}` (Claude Code docs, "Connect Claude
    /// Code to tools via MCP": the user-scope `mcpServers` of `~/.claude.json`).
    fn mcp_entry(&self, exe: &Path, args: &[String]) -> Value {
        json!({"type": "stdio", "command": exe.display().to_string(), "args": args, "env": {}})
    }

    fn remove_server_hint(&self, name: &str) -> String {
        format!("`claude mcp remove -s user {name}`")
    }

    fn settings_file(&self) -> &Path {
        &self.paths.settings
    }

    fn instruction_file(&self) -> &Path {
        &self.paths.instructions
    }

    fn projects_dir(&self) -> &Path {
        &self.paths.projects
    }

    fn has_file_memory(&self) -> bool {
        true
    }

    fn memory_dir(&self, root: &Path) -> PathBuf {
        self.paths
            .projects
            .join(crate::import::claude_slug(&root.display().to_string()))
            .join("memory")
    }

    /// `~/.claude/rules/graphersal-memory-global.md`: Claude Code loads every file of the user
    /// rules directory in every project, and nobody else writes this file (the managed block
    /// of `CLAUDE.md` is not used: uninstall removes it).
    fn global_memory_file(&self) -> PathBuf {
        self.paths.config_dir.join("rules").join(GLOBAL_RULES_FILE)
    }

    /// `autoMemoryEnabled` in the user settings (Claude Code docs, "How Claude remembers your
    /// project", section "Enable or disable auto memory": the `/memory` toggle saves
    /// `autoMemoryEnabled` to `~/.claude/settings.json`).
    fn auto_memory_key(&self) -> Option<&'static str> {
        Some("autoMemoryEnabled")
    }

    fn headless_command(&self) -> Option<&'static str> {
        Some(crate::config::DEFAULT_HARNESS_COMMAND)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_code_names_its_files() {
        let paths =
            ClaudePaths::resolve(&|name| (name == "HOME").then(|| "/h".to_string())).unwrap();
        let agent = ClaudeCode::new(paths);
        assert_eq!(agent.id(), "claude-code");
        assert_eq!(agent.settings_file(), Path::new("/h/.claude/settings.json"));
        assert_eq!(
            agent.memory_dir(Path::new("/w/my app")),
            PathBuf::from("/h/.claude/projects/-w-my-app/memory")
        );
        assert_eq!(
            agent.global_memory_file(),
            PathBuf::from("/h/.claude/rules/graphersal-memory-global.md")
        );
        assert_eq!(agent.auto_memory_key(), Some("autoMemoryEnabled"));
    }
}
