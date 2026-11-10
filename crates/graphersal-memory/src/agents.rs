//! The harness abstraction: what `install`, `uninstall`, `import` and `export` need to know
//! about one AI agent harness, behind one interface. Claude Code ([`ClaudeCode`]) and opencode
//! (`crate::opencode::OpenCode`) implement it; `crate::install` asks the [`Agent`] for the
//! files it edits, the JSON keys of its MCP registrations and the names the wizard shows.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::install::ClaudePaths;

/// The harnesses graphersal-memory supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Kind {
    /// Claude Code.
    #[default]
    ClaudeCode,
    /// opencode.
    OpenCode,
}

impl Kind {
    /// Every supported harness, in the order the wizard lists them.
    pub const ALL: [Kind; 2] = [Kind::ClaudeCode, Kind::OpenCode];

    /// The id of the command line and the state files (`claude-code`, `opencode`).
    pub fn id(self) -> &'static str {
        match self {
            Kind::ClaudeCode => "claude-code",
            Kind::OpenCode => "opencode",
        }
    }

    /// The name people know.
    pub fn display_name(self) -> &'static str {
        match self {
            Kind::ClaudeCode => "Claude Code",
            Kind::OpenCode => "opencode",
        }
    }

    /// The harness of an id.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == id)
    }
}

/// One harness, as the installation sees it.
pub trait Agent {
    /// Which harness this is.
    fn kind(&self) -> Kind;
    /// The harness id of the command line (`claude-code`, `opencode`).
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
    /// The headless command line the summaries and the A/B run (a prompt on stdin).
    fn headless_command(&self) -> &'static str;
}

/// Whether an MCP entry is one of ours: it starts a `graphersal-memory` binary with `mcp`
/// (Claude Code: `command` + `args`; opencode: `command` as one array).
pub fn is_our_entry(entry: &Value) -> bool {
    let (command, first) = match &entry["command"] {
        Value::Array(parts) => (
            parts.first().and_then(Value::as_str).unwrap_or_default(),
            parts.get(1).cloned().unwrap_or(Value::Null),
        ),
        other => (other.as_str().unwrap_or_default(), entry["args"][0].clone()),
    };
    // The file name on any platform (an entry may come from another system's path style).
    let name = command.rsplit(['/', '\\']).next().unwrap_or_default();
    let name = name.strip_suffix(".exe").unwrap_or(name);
    name == "graphersal-memory" && first == "mcp"
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

impl Agent for ClaudeCode {
    fn kind(&self) -> Kind {
        Kind::ClaudeCode
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

    fn headless_command(&self) -> &'static str {
        crate::config::DEFAULT_HARNESS_COMMAND
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
