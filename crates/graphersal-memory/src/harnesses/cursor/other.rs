//! Cursor's part of other memory beside graphersal-memory (M15, `crate::install::coexist`): its
//! `mcp.json` files, its Memories, the rule files it loads, and how an entry is switched off.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::Cursor;
use crate::install::Setup;
use crate::install::coexist::{BuiltIn, OtherMemory, Server, ServerSource, files_with_extension};

impl OtherMemory for Cursor {
    /// `~/.cursor/mcp.json` and the project's `.cursor/mcp.json`.
    fn server_sources(&self, _setup: &Setup, project: Option<&Path>) -> Vec<ServerSource> {
        let file = &self.paths.mcp_file;
        let value = crate::install::read_json(file, &mut Vec::new());
        let mut out = vec![ServerSource::of(file, &["mcpServers"], &value, None)];
        if let Some(root) = project {
            let file = root.join(".cursor").join("mcp.json");
            let value = crate::install::read_json(&file, &mut Vec::new());
            out.push(ServerSource::of(
                &file,
                &["mcpServers"],
                &value,
                Some(root.display().to_string()),
            ));
        }
        out
    }

    fn built_in(&self, _setup: &Setup) -> Option<BuiltIn> {
        Some(BuiltIn {
            harness: crate::harnesses::agent::Kind::Cursor,
            text: "Cursor's Memories: kept by Cursor itself, not readable from files (Cursor \
                   Settings, Rules & Memories, shows and switches them)"
                .to_string(),
            entries: 0,
            notable: true,
        })
    }

    /// The user rules other than ours; a project's `AGENTS.md`, `.cursorrules` and
    /// `.cursor/rules/`.
    fn instruction_files(&self, _setup: &Setup, project: Option<&Path>) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> =
            files_with_extension(&self.paths.cursor_dir.join("rules"), "mdc")
                .into_iter()
                .filter(|file| *file != self.paths.rules_file)
                .collect();
        if let Some(root) = project {
            out.push(root.join("AGENTS.md"));
            out.push(root.join(".cursorrules"));
            out.extend(files_with_extension(
                &root.join(".cursor").join("rules"),
                "mdc",
            ));
        }
        out
    }

    /// A user-scope entry has no switched-off state: removed, and kept in the record.
    fn disable_edit(&self, server: &Server, text: &str) -> Result<Option<(String, Value)>, String> {
        crate::install::coexist::remove_entry(server, text)
    }

    fn off_by_hand(&self, server: &Server) -> String {
        format!("switch {} off in Cursor Settings, MCP", server.name)
    }
}
