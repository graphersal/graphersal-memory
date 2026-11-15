//! CodeWhale's part of other memory beside graphersal-memory (M15, `crate::install::coexist`): its
//! `mcp.json` files, its own memory, the instruction file it loads, and its switch.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::CodeWhale;
use crate::install::coexist::{BuiltIn, OtherMemory, Server, ServerSource};
use crate::install::{Setup, read_text};

impl OtherMemory for CodeWhale {
    /// Its `mcp.json` (`servers` or `mcpServers`) and the project's `.codewhale/mcp.json`.
    fn server_sources(&self, _setup: &Setup, project: Option<&Path>) -> Vec<ServerSource> {
        let mut files = vec![(self.paths.mcp_file.clone(), None)];
        if let Some(root) = project {
            files.push((
                root.join(".codewhale").join("mcp.json"),
                Some(root.display().to_string()),
            ));
        }
        let mut out = Vec::new();
        for (file, owner) in files {
            let value = crate::install::read_json(&file, &mut Vec::new());
            for key in ["servers", "mcpServers"] {
                out.push(ServerSource::of(&file, &[key], &value, owner.clone()));
            }
        }
        out
    }

    /// Its memory (on or off, its reviewed entries), when on or when its store is there.
    fn built_in(&self, setup: &Setup) -> Option<BuiltIn> {
        let on = super::memory_on(
            &read_text(&self.paths.config_file)
                .ok()
                .flatten()
                .and_then(|text| crate::install::tomledit::parse(&text).ok())
                .unwrap_or(Value::Null),
            self.paths.memory_env_on,
        );
        let store = &self.paths.memory_store;
        let entries = super::memory::read(store).map(|e| e.len()).unwrap_or(0);
        (on || store.is_file()).then(|| BuiltIn {
            harness: crate::harnesses::agent::Kind::CodeWhale,
            text: format!(
                "CodeWhale's memory: {}; {entries} reviewed entr{} in {}",
                if on { "on" } else { "off" },
                if entries == 1 { "y" } else { "ies" },
                setup.show(&store.display().to_string())
            ),
            entries,
            notable: true,
        })
    }

    /// The global file it loads (the first of its candidates); a project's `AGENTS.md`.
    fn instruction_files(&self, _setup: &Setup, project: Option<&Path>) -> Vec<PathBuf> {
        let mut out = vec![super::instruction_file(&self.paths.home)];
        if let Some(root) = project {
            out.push(root.join("AGENTS.md"));
        }
        out
    }

    /// `"disabled": true` (`how: flag`, `off: true`, with the value before).
    fn disable_edit(&self, server: &Server, text: &str) -> Result<Option<(String, Value)>, String> {
        crate::install::coexist::flag_entry(server, text, "disabled", true, true)
    }

    fn off_by_hand(&self, server: &Server) -> String {
        format!("`codewhale mcp disable {}`", server.name)
    }
}
