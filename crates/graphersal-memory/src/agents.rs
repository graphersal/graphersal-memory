//! The harness abstraction: what `install`, `uninstall`, `import` and `export` need to know
//! about one AI agent harness, behind one interface, so a second harness (opencode is next)
//! is a new implementation instead of a new code path. Claude Code is the only one today
//! ([`ClaudeCode`]); `crate::install` asks the [`Agent`] for the files it edits and the
//! wizard for the names and the steps that apply.

use std::path::{Path, PathBuf};

use crate::install::ClaudePaths;

/// One harness, as the installation sees it.
pub trait Agent {
    /// The harness id of the command line (`claude-code`).
    #[allow(dead_code)] // the harness interface; the second harness (opencode) uses it
    fn id(&self) -> &'static str;
    /// The name people know (`Claude Code`).
    fn display_name(&self) -> &'static str;
    /// The file holding the user-scope MCP server registrations.
    fn mcp_config_file(&self) -> &Path;
    /// The user settings file (hooks, the auto-memory switch).
    fn settings_file(&self) -> &Path;
    /// The user instruction file every session loads (the managed rule block goes there).
    fn instruction_file(&self) -> &Path;
    /// The directory of the per-project transcripts and file memory.
    fn projects_dir(&self) -> &Path;
    /// The file-memory directory of the project rooted at `root`.
    fn memory_dir(&self, root: &Path) -> PathBuf;
    /// A file every session of every project loads, where exported global memories go.
    fn global_memory_file(&self) -> PathBuf;
    /// The settings key that switches the harness's own memory off (`false` = off), when it
    /// has one.
    fn auto_memory_key(&self) -> Option<&'static str>;
    /// The headless command line the summaries and the A/B run (a prompt on stdin).
    #[allow(dead_code)] // the harness interface; the second harness (opencode) uses it
    fn headless_command(&self) -> &'static str;
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
    fn id(&self) -> &'static str {
        "claude-code"
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    fn mcp_config_file(&self) -> &Path {
        &self.paths.user_config
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
