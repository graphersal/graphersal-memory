//! DeepSeek Harness's part of other memory beside graphersal-memory (M15,
//! `crate::install::coexist`): the MCP servers of its patch layers, the instruction files it
//! loads, and how a server is switched off by hand. dsh has no memory of its own.
//!
//! - **Servers**: the `@deepseek-ai/dsh-mcp-client` rows inserted by the home layer
//!   `$DSH_HOME/cordis.patch.yml` and by every profile layer `$DSH_HOME/profiles/*/
//!   cordis.patch.yml` (an `- insert:` item's rows, named by `config.serverName`), and the
//!   servers of the web and Desktop profiles' `mcp-manager` row (`- id: mcp-manager` with
//!   `config.servers.<name>`, in any of those files; `packages/mcp/mcp-manager/README.md:52-75`).
//!   A row is off by its own `disabled: true`, or by an override `- id: <its id>` with
//!   `disabled: true` in its file or in the home layer (which outranks the profile layers).
//!   Desktop's own profile layer lives in an Electron-managed directory that was not traced, so
//!   its servers are not seen.
//! - **Switching off**: never done by install, only reported with how to do it by hand. The safe
//!   place would be an override `- id: <their id>` / `disabled: true` inside our managed block
//!   of the home layer, but the shared switch-off edits the file that holds the server (a
//!   profile layer, which dsh's Settings rewrite) and records it under that file, and our block
//!   is rewritten whole by every install; folding an override into it would need the shared
//!   flow to edit and record another file than the server's. A row without an `id` gets a
//!   random one from dsh (`vendor/include/src/patch.ts:37-41`), so no override can name it; an
//!   `mcp-manager` server is Settings' to change.
//! - **Instruction files**: `$DSH_HOME/AGENTS.md`, `<$DSH_AGENTS_HOME or ~/.agents>/AGENTS.md`,
//!   then the project's `AGENTS.md`, `CLAUDE.md`, `AGENTS.local.md` and `CLAUDE.local.md`
//!   (`packages/context/agent-instructions/README.md`, "What the agent gets").

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::Dsh;
use crate::install::coexist::{OtherMemory, Server, ServerSource};
use crate::install::{Setup, read_text, yamledit};

/// The key path of inserted rows in a [`ServerSource`] (by server name).
const ROWS: &str = "insert";

/// The items of a patch file, empty when it is missing or dsh could not read it either.
fn items_of(file: &Path) -> Vec<Value> {
    read_text(file)
        .ok()
        .flatten()
        .and_then(|text| yamledit::items(&text).ok())
        .unwrap_or_default()
}

/// The ids that `items` switch off by an override (`- id: X` with `disabled: true`).
fn overridden_off(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .filter(|item| item.get("insert").is_none() && item["disabled"] == Value::Bool(true))
        .filter_map(|item| item["id"].as_str().map(str::to_string))
        .collect()
}

/// The MCP client rows inserted by `items`, by server name: each row's `config` with its `id`
/// and its `disabled` beside (`off`: the ids overrides switch off).
fn rows(items: &[Value], off: &[String]) -> Value {
    let mut out = Map::new();
    for row in super::install::inserted(items) {
        if row["name"] != super::MCP_CLIENT {
            continue;
        }
        let Some(name) = row["config"]["serverName"].as_str() else {
            continue;
        };
        let mut entry = match &row["config"] {
            Value::Object(config) => config.clone(),
            _ => Map::new(),
        };
        if let Some(id) = row["id"].as_str() {
            entry.insert("id".to_string(), json!(id));
        }
        let disabled = row["disabled"] == Value::Bool(true)
            || row["id"]
                .as_str()
                .is_some_and(|id| off.iter().any(|o| o == id));
        if disabled {
            entry.insert("disabled".to_string(), json!(true));
        }
        out.insert(name.to_string(), Value::Object(entry));
    }
    Value::Object(out)
}

/// The servers of the `mcp-manager` overrides among `items`.
fn manager_servers(items: &[Value]) -> Value {
    let mut out = Map::new();
    for item in items.iter().filter(|i| i["id"] == super::MCP_MANAGER_ROW) {
        if let Some(servers) = item["config"]["servers"].as_object() {
            out.extend(servers.clone());
        }
    }
    Value::Object(out)
}

impl OtherMemory for Dsh {
    /// The inserted MCP client rows and the `mcp-manager` servers of the home layer and of
    /// every profile layer (user scope: dsh has no project configuration).
    fn server_sources(&self, _setup: &Setup, _project: Option<&Path>) -> Vec<ServerSource> {
        let home = items_of(&self.paths.patch_file);
        let home_off = overridden_off(&home);
        let mut layers = vec![(self.paths.patch_file.clone(), home)];
        layers.extend(
            self.paths
                .profile_patches()
                .into_iter()
                .map(|(_, file)| (file.clone(), items_of(&file))),
        );
        let mut out = Vec::new();
        for (file, items) in layers {
            let mut off = overridden_off(&items);
            off.extend(home_off.iter().cloned());
            out.push(ServerSource::of(
                &file,
                &[ROWS],
                &json!({ROWS: rows(&items, &off)}),
                None,
            ));
            out.push(ServerSource::of(
                &file,
                &[super::MCP_MANAGER_ROW, "servers"],
                &json!({super::MCP_MANAGER_ROW: {"servers": manager_servers(&items)}}),
                None,
            ));
        }
        out
    }

    /// `$DSH_HOME/AGENTS.md`, the agents home's `AGENTS.md`, then the project's chain; none
    /// without a dsh home (no dsh on this machine loads them).
    fn instruction_files(&self, _setup: &Setup, project: Option<&Path>) -> Vec<PathBuf> {
        if !self.paths.dsh_home.is_dir() {
            return Vec::new();
        }
        let mut out = vec![
            self.paths.instruction_file.clone(),
            self.paths.agents_home.join("AGENTS.md"),
        ];
        if let Some(root) = project {
            for name in [
                "AGENTS.md",
                "CLAUDE.md",
                "AGENTS.local.md",
                "CLAUDE.local.md",
            ] {
                out.push(root.join(name));
            }
        }
        out
    }

    /// Never in place (module docs): `doctor` and install say how.
    fn disable_edit(
        &self,
        _server: &Server,
        _text: &str,
    ) -> Result<Option<(String, Value)>, String> {
        Ok(None)
    }

    fn off_by_hand(&self, server: &Server) -> String {
        let file = server.file.display();
        if server.path.first().map(String::as_str) == Some(super::MCP_MANAGER_ROW) {
            return format!(
                "remove the server `{}` (or switch it off) in dsh's Settings (MCP); it is in the \
                 `mcp-manager` row of {file}",
                server.name
            );
        }
        match server.entry["id"].as_str() {
            Some(id) => format!(
                "add `- id: {id}` with `disabled: true` to {} (outside the graphersal-memory \
                 block; the home layer outranks the profiles), or set `disabled: true` on its row \
                 in {file}",
                self.paths.patch_file.display()
            ),
            None => format!(
                "set `disabled: true` on the row of `serverName: {}` in {file} (beside its `name`; \
                 the row has no `id`, so no override can name it)",
                server.name
            ),
        }
    }
}
