//! Claude Code (2.1.x): the `Harness` and its capabilities. Its files are `ClaudePaths`
//! (`crate::install`): `~/.claude.json`, `~/.claude/settings.json`, `~/.claude/CLAUDE.md`,
//! `~/.claude/projects/<encoded root>/memory/`, `~/.claude/rules/` (or `$CLAUDE_CONFIG_DIR`).
//! Its install parts still live in `crate::install` (with the shared wizard), its file-memory
//! import in `crate::import`, its transcript parser in `crate::transcript` and its hook in
//! `crate::hook`.

pub mod install;
pub mod other;
pub mod sessions;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::agents::{Harness, HookHandler, Kind, OwnMemory, OwnMemoryNow, OwnMemoryQuestion};
use crate::install::{Change, ClaudePaths, Finding, HarnessPlan, PlanInput, Setup, State};

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

    fn develop_on(&self, setup: &Setup) -> bool {
        crate::install::claude_develop_on(setup)
    }

    fn has_sessions(&self) -> bool {
        self.paths.projects.is_dir()
    }

    fn hook(&self) -> Option<&dyn HookHandler> {
        Some(self)
    }

    fn own_memory(&self) -> Option<&dyn OwnMemory> {
        Some(self)
    }

    fn other_memory(&self) -> Option<&dyn crate::coexist::OtherMemory> {
        Some(self)
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

impl HookHandler for ClaudeCode {
    /// The memory files of the session's project imported, the indexing queued, the start
    /// context at the start. Claude Code's hooks run by Cursor (its third-party hooks) do
    /// nothing: the Cursor hooks do Cursor's part.
    fn run(
        &self,
        setup: &Setup,
        event: crate::hook::HookEvent,
        input: Vec<u8>,
        stdout: &mut dyn std::io::Write,
        stderr: &mut dyn std::io::Write,
    ) -> u8 {
        if crate::hook::is_cursor_input(&String::from_utf8_lossy(&input)) {
            let _ = writeln!(
                stderr,
                "graphersal-memory hook: run by Cursor for Claude Code's settings; nothing to do \
                 (graphersal-memory's own Cursor hooks handle Cursor: `graphersal-memory install \
                 --harness cursor`)"
            );
            return 0;
        }
        crate::hook::run_with_context(
            event,
            Ok(self.paths.clone()),
            setup.data_dir.clone(),
            Box::new(crate::commands::daemon_connector(setup)),
            Some(crate::commands::hook_enqueue(setup)),
            Some(crate::commands::start_fetch(setup, Kind::ClaudeCode)),
            std::io::Cursor::new(input),
            stdout,
            stderr,
        )
    }
}

impl OwnMemory for ClaudeCode {
    /// The auto-memory: off when `autoMemoryEnabled` is false; asked about unless the user
    /// switched it off themselves.
    fn now(&self, setup: &Setup) -> Result<OwnMemoryNow, String> {
        crate::install::claude_own_memory_now(setup)
    }

    fn question(&self, setup: &Setup, start_sentence: &str) -> OwnMemoryQuestion {
        let name = self.display_name();
        OwnMemoryQuestion {
            title: format!("{name}'s auto-memory"),
            lines: vec![
                format!(
                    "{name} also writes its own memory files and loads them into every session;"
                ),
                "with graphersal-memory both would keep the same facts twice.".to_string(),
                start_sentence.to_string(),
                crate::install::NOTHING_IS_LOST.to_string(),
            ],
            yes: format!(
                "switch it off ({} false in {})",
                self.auto_memory_key().unwrap_or_default(),
                setup.show(&self.settings_file().display().to_string())
            ),
            no: "keep it on (the files it writes are imported at each session start)".to_string(),
        }
    }

    fn also(&self, setup: &Setup) -> Vec<String> {
        vec![format!(
            "The same answer switches {}'s auto-memory off ({} in {}).",
            self.display_name(),
            self.auto_memory_key().unwrap_or_default(),
            setup.show(&self.settings_file().display().to_string())
        )]
    }

    fn recap_title(&self) -> Option<&'static str> {
        Some("Auto-memory")
    }
}

/// Claude Code's `claude -p` (its flags added by `crate::summaries::harness_args`).
pub struct ClaudeHeadless;

impl crate::harness::Headless for ClaudeHeadless {
    fn matches(&self, command: &[String]) -> bool {
        command.first().is_some_and(|program| {
            Path::new(program)
                .file_stem()
                .is_some_and(|stem| stem == "claude")
        })
    }

    fn default_command(&self) -> Option<&'static str> {
        Some(crate::config::DEFAULT_HARNESS_COMMAND)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

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
