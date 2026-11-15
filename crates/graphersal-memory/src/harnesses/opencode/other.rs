//! opencode's part of other memory beside graphersal-memory (M15, `crate::install::coexist`): its MCP
//! entries (`mcp`, and 2.x's `mcp.servers`), its plugins, the instruction files it loads (its
//! own and, by its Claude Code compatibility, Claude Code's), and its switch.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::OpenCode;
use crate::install::Setup;
use crate::install::coexist::{OtherMemory, Plugin, Server, ServerSource};

/// The switch of an entry under `path`: `"disabled": true` in 2.x's `mcp.servers`,
/// `"enabled": false` in `mcp` (1.x, and 2.x's legacy entries).
pub fn switch(path: &[String]) -> (&'static str, bool) {
    if path.len() == 2 && path[1] == "servers" {
        ("disabled", true)
    } else {
        ("enabled", false)
    }
}

impl OtherMemory for OpenCode {
    fn server_sources(&self, _setup: &Setup, _project: Option<&Path>) -> Vec<ServerSource> {
        let file = &self.paths.config_file;
        let value = crate::install::read_json(file, &mut Vec::new());
        // `mcp` (1.x, and 2.x's legacy entries) holds 2.x's `servers` object: not a server.
        let mut legacy = ServerSource::of(file, &["mcp"], &value, None);
        legacy.skip = vec!["servers".to_string()];
        vec![
            legacy,
            ServerSource::of(file, &["mcp", "servers"], &value, None),
        ]
    }

    /// The entries of `plugin` and the files in `plugin/` and `plugins/` (ours left out).
    fn plugins(&self, _setup: &Setup) -> Vec<Plugin> {
        let mut out = Vec::new();
        let config = crate::install::read_json(&self.paths.config_file, &mut Vec::new());
        for item in config["plugin"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if crate::install::coexist::names_memory_tool(item) {
                out.push(Plugin {
                    harness: crate::harnesses::agent::Kind::OpenCode,
                    name: item.to_string(),
                    file: self.paths.config_file.clone(),
                });
            }
        }
        for dir in ["plugin", "plugins"] {
            let dir = self.paths.config_dir.join(dir);
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut names: Vec<String> = entries
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| {
                    name != super::PLUGIN_FILE && crate::install::coexist::names_memory_tool(name)
                })
                .collect();
            names.sort();
            for name in names {
                out.push(Plugin {
                    harness: crate::harnesses::agent::Kind::OpenCode,
                    name: name.clone(),
                    file: dir.join(name),
                });
            }
        }
        out
    }

    /// Its `AGENTS.md` and the files listed in `instructions` (not ours, no globs); a
    /// project's `AGENTS.md`. Its Claude Code compatibility loads Claude Code's files where it
    /// has none of its own: `~/.claude/CLAUDE.md` without a global `AGENTS.md`, a project's
    /// `CLAUDE.md` without its `AGENTS.md` (off by `OPENCODE_DISABLE_CLAUDE_CODE(_PROMPT)`).
    fn instruction_files(&self, setup: &Setup, project: Option<&Path>) -> Vec<PathBuf> {
        let paths = &self.paths;
        let mut out = vec![paths.config_dir.join("AGENTS.md")];
        let config = crate::install::read_json(&paths.config_file, &mut Vec::new());
        for item in config["instructions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if item.contains('*') || item.ends_with(super::RULES_FILE) {
                continue;
            }
            out.push(if let Some(rest) = item.strip_prefix("~/") {
                paths.home.join(rest)
            } else if Path::new(item).is_absolute() {
                PathBuf::from(item)
            } else {
                paths.config_dir.join(item)
            });
        }
        let reads_claude =
            paths.config_dir.is_dir() && std::env::var_os("OPENCODE_DISABLE_CLAUDE_CODE").is_none();
        if reads_claude
            && std::env::var_os("OPENCODE_DISABLE_CLAUDE_CODE_PROMPT").is_none()
            && !paths.config_dir.join("AGENTS.md").is_file()
        {
            out.push(setup.paths().instructions.clone());
        }
        if let Some(root) = project {
            out.push(root.join("AGENTS.md"));
            if reads_claude && !root.join("AGENTS.md").is_file() {
                out.push(root.join("CLAUDE.md"));
            }
        }
        out
    }

    /// `"enabled": false` (1.x) or `"disabled": true` (2.x).
    fn disable_edit(&self, server: &Server, text: &str) -> Result<Option<(String, Value)>, String> {
        let (key, off) = switch(&server.path);
        crate::install::coexist::flag_entry(server, text, key, off, false)
    }

    fn off_by_hand(&self, server: &Server) -> String {
        let (key, off) = switch(&server.path);
        format!("set \"{key}\": {off} in its entry")
    }
}
