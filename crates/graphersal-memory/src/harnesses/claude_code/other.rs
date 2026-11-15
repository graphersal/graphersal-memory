//! Claude Code's part of other memory beside graphersal-memory (M15, `crate::install::coexist`): its MCP
//! entries (user and local scopes, a project's `.mcp.json`) with the tools its permission rules
//! name, its auto-memory, plugins and hooks of other memory tools, the instruction files it
//! loads, and how an entry is switched off.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::ClaudeCode;
use crate::harnesses::agent::Kind;
use crate::install::Setup;
use crate::install::coexist::{
    BuiltIn, OtherMemory, Plugin, Server, ServerSource, claude_permission_tools,
    files_with_extension, names_memory_tool, remove_entry,
};

impl OtherMemory for ClaudeCode {
    /// User scope, the local scopes of `~/.claude.json`, and the project's `.mcp.json`.
    fn server_sources(&self, setup: &Setup, project: Option<&Path>) -> Vec<ServerSource> {
        let permission_tools = claude_permission_tools(setup);
        let file = setup.paths().user_config.clone();
        let config = crate::install::read_json(&file, &mut Vec::new());
        let mut out = vec![ServerSource::of(&file, &["mcpServers"], &config, None)];
        if let Some(projects) = config["projects"].as_object() {
            for path in projects.keys() {
                out.push(ServerSource::of(
                    &file,
                    &["projects", path, "mcpServers"],
                    &config,
                    Some(path.clone()),
                ));
            }
        }
        if let Some(root) = project {
            let file = root.join(".mcp.json");
            let value = crate::install::read_json(&file, &mut Vec::new());
            out.push(ServerSource::of(
                &file,
                &["mcpServers"],
                &value,
                Some(root.display().to_string()),
            ));
        }
        for source in &mut out {
            source.permission_tools = permission_tools.clone();
        }
        out
    }

    fn built_in(&self, setup: &Setup) -> Option<BuiltIn> {
        let settings = crate::install::read_json(&setup.paths().settings, &mut Vec::new());
        let off = settings["autoMemoryEnabled"] == Value::Bool(false);
        let dirs = crate::harnesses::claude_code::import::scan(&setup.paths().projects)
            .unwrap_or_default();
        let files: usize = dirs.iter().map(|d| d.files.len()).sum();
        Some(BuiltIn {
            harness: Kind::ClaudeCode,
            text: format!(
                "Claude Code's auto-memory: {}; {files} memory file{} in {} project{} ({})",
                if off { "off" } else { "on" },
                if files == 1 { "" } else { "s" },
                dirs.len(),
                if dirs.len() == 1 { "" } else { "s" },
                setup.show(&setup.paths().projects.display().to_string())
            ),
            entries: files,
            notable: false,
        })
    }

    /// Enabled and installed plugins, and hooks other than ours, that name a memory tool.
    fn plugins(&self, setup: &Setup) -> Vec<Plugin> {
        let mut out = Vec::new();
        let settings = crate::install::read_json(&setup.paths().settings, &mut Vec::new());
        if let Some(enabled) = settings["enabledPlugins"].as_object() {
            for (name, on) in enabled {
                if *on != Value::Bool(false) && names_memory_tool(name) {
                    out.push(Plugin {
                        harness: Kind::ClaudeCode,
                        name: name.clone(),
                        file: setup.paths().settings.clone(),
                    });
                }
            }
        }
        let installed_file = setup
            .paths()
            .config_dir
            .join("plugins")
            .join("installed_plugins.json");
        let installed = crate::install::read_json(&installed_file, &mut Vec::new());
        if let Some(plugins) = installed["plugins"].as_object() {
            for name in plugins.keys() {
                if names_memory_tool(name) && !out.iter().any(|p| p.name == *name) {
                    out.push(Plugin {
                        harness: Kind::ClaudeCode,
                        name: name.clone(),
                        file: installed_file.clone(),
                    });
                }
            }
        }
        let ours = setup.ours();
        if let Some(events) = settings["hooks"].as_object() {
            for groups in events.values() {
                for group in groups.as_array().into_iter().flatten() {
                    for hook in group["hooks"].as_array().into_iter().flatten() {
                        let command = hook["command"].as_str().unwrap_or_default();
                        if !ours.hook(hook)
                            && names_memory_tool(command)
                            && !out.iter().any(|p| p.name == command)
                        {
                            out.push(Plugin {
                                harness: Kind::ClaudeCode,
                                name: command.to_string(),
                                file: setup.paths().settings.clone(),
                            });
                        }
                    }
                }
            }
        }
        out
    }

    /// `~/.claude/CLAUDE.md`, the user rules (not our exported global memories); a project's
    /// `CLAUDE.md`, `CLAUDE.local.md`, `.claude/CLAUDE.md` and `.claude/rules/`.
    fn instruction_files(&self, setup: &Setup, project: Option<&Path>) -> Vec<PathBuf> {
        let mut out = vec![setup.paths().instructions.clone()];
        for file in files_with_extension(&setup.paths().config_dir.join("rules"), "md") {
            if file
                .file_name()
                .is_some_and(|n| n != super::GLOBAL_RULES_FILE)
            {
                out.push(file);
            }
        }
        if let Some(root) = project {
            out.push(root.join("CLAUDE.md"));
            out.push(root.join("CLAUDE.local.md"));
            out.push(root.join(".claude").join("CLAUDE.md"));
            out.extend(files_with_extension(
                &root.join(".claude").join("rules"),
                "md",
            ));
        }
        out
    }

    /// A user-scope entry has no switched-off state: removed, and kept in the record.
    fn disable_edit(&self, server: &Server, text: &str) -> Result<Option<(String, Value)>, String> {
        remove_entry(server, text)
    }

    fn off_by_hand(&self, server: &Server) -> String {
        match &server.project {
            Some(_) if server.file.ends_with(".mcp.json") => format!(
                "remove `{}` from that project's .mcp.json, or deny it in its .claude/settings.json",
                server.name
            ),
            Some(_) => format!(
                "in that project: `claude mcp remove -s local {}`",
                server.name
            ),
            None => format!("`claude mcp remove -s user {}`", server.name),
        }
    }
}
