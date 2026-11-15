//! Codex's part of other memory beside graphersal-memory (M15, `crate::install::coexist`): its
//! `[mcp_servers.*]` tables, its memories, the instruction files it loads, and its switch.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::Codex;
use crate::install::coexist::{BuiltIn, OtherMemory, Server, ServerSource};
use crate::install::tomledit;
use crate::install::{Setup, read_text};

impl OtherMemory for Codex {
    fn server_sources(&self, _setup: &Setup, _project: Option<&Path>) -> Vec<ServerSource> {
        let file = &self.paths.config_file;
        match read_text(file)
            .ok()
            .flatten()
            .map(|text| tomledit::parse(&text))
        {
            Some(Ok(doc)) => vec![ServerSource::of(file, &["mcp_servers"], &doc, None)],
            _ => Vec::new(),
        }
    }

    /// Its memories (`[features] memories`, the files of `memories_v2`/`memories`), when on
    /// or when files are there.
    fn built_in(&self, setup: &Setup) -> Option<BuiltIn> {
        let on = read_text(&self.paths.config_file)
            .ok()
            .flatten()
            .and_then(|text| tomledit::parse(&text).ok())
            .is_some_and(|doc| super::install::memories_on(&doc));
        let mut files = 0;
        for dir in ["memories_v2", "memories"] {
            files += crate::install::coexist::count_files(&self.paths.codex_home.join(dir), 2);
        }
        (on || files > 0).then(|| BuiltIn {
            harness: crate::harnesses::agent::Kind::Codex,
            text: format!(
                "Codex's memories: {}; {files} file{} in {}",
                if on { "on" } else { "off" },
                if files == 1 { "" } else { "s" },
                setup.show(
                    &self
                        .paths
                        .codex_home
                        .join("memories_v2")
                        .display()
                        .to_string()
                )
            ),
            entries: files,
            notable: true,
        })
    }

    /// `AGENTS.override.md` and `AGENTS.md` of its home; a project's `AGENTS.md`,
    /// `AGENTS.override.md`, else the first of `project_doc_fallback_filenames` there.
    fn instruction_files(&self, _setup: &Setup, project: Option<&Path>) -> Vec<PathBuf> {
        let home = &self.paths.codex_home;
        let mut out = vec![home.join("AGENTS.override.md"), home.join("AGENTS.md")];
        if let Some(root) = project {
            out.push(root.join("AGENTS.md"));
            out.push(root.join("AGENTS.override.md"));
            let fallbacks: Vec<String> = std::fs::read_to_string(&self.paths.config_file)
                .ok()
                .and_then(|text| tomledit::parse(&text).ok())
                .map(|config| {
                    config["project_doc_fallback_filenames"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .filter(|name| !name.contains(['/', '\\']) && !name.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if !root.join("AGENTS.md").is_file()
                && !root.join("AGENTS.override.md").is_file()
                && let Some(name) = fallbacks.iter().find(|n| root.join(n).is_file())
            {
                out.push(root.join(name));
            }
        }
        out
    }

    /// `enabled = false` in its table (`how: toml`, with the value before).
    fn disable_edit(&self, server: &Server, text: &str) -> Result<Option<(String, Value)>, String> {
        let previous = server.entry.get("enabled").cloned().unwrap_or(Value::Null);
        Ok(
            tomledit::set_table_bool(text, &["mcp_servers", &server.name], "enabled", false)?.map(
                |new| {
                    (
                        new,
                        json!({"how": "toml", "path": server.path, "key": "enabled",
                               "previous": previous}),
                    )
                },
            ),
        )
    }

    fn off_by_hand(&self, server: &Server) -> String {
        format!("set `enabled = false` in [mcp_servers.{}]", server.name)
    }
}
