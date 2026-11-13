//! Coexisting with other memory (M15): what else on this machine keeps the agent's memory, and
//! what install, `status` and `doctor` do about it. Detection only reads files; nothing of the
//! user's is changed unless the user chose it (`docs/harnesses.md`, "Other memory").
//!
//! - **Built-in memory** of each harness ([`BuiltIn`]): Claude Code's auto-memory (on/off, its
//!   memory files), Codex's memories (`[features] memories`, `$CODEX_HOME/memories_v2/`),
//!   Cursor's Memories (kept by Cursor itself, not readable from files), opencode (none).
//! - **Other memory tools** ([`Server`]): the MCP entries of every harness configuration
//!   (Claude Code's user and local scopes and a project's `.mcp.json`, opencode's `mcp` /
//!   `mcp.servers`, Codex's `[mcp_servers.*]`, Cursor's `mcpServers` and a project's
//!   `.cursor/mcp.json`) whose name, command or package names a known memory tool ([`TOOLS`]), or
//!   whose name says memory, or whose tool names (from the entry itself, or Claude Code's
//!   `permissions.allow` rules `mcp__<server>__<tool>`) look like memory tools
//!   ([`MEMORY_TOOL_NAMES`]). Other servers are never started to find out. Memory plugins
//!   ([`Plugin`]): Claude Code plugins and hooks, opencode plugins, by known names.
//! - **Instruction files** ([`Mention`]): the user and project instruction files of each harness
//!   (`CLAUDE.md`, `CLAUDE.local.md`, rules, `AGENTS.md`, Cursor rules, opencode's
//!   `instructions`) with a line about keeping memory elsewhere (`MEMORY.md`, another tool's
//!   name, "keep/store/save ... memory in ..."). Never edited: `doctor` shows the line and the
//!   edit it suggests.
//! - **External writers** ([`Writer`]): an imported memory directory whose `MEMORY.md` carries a
//!   generated-file header, or whose imported files changed while no Claude Code session of that
//!   project was running (their modification time outside every transcript's time span).
//! - **Returning facts** ([`Returning`]): files whose import brought back a memory that was
//!   forgotten or changed in the graph (`crate::import` records them), with the fix (stop the
//!   program, edit the file, or `import --ignore` it).
//!
//! The user's choice per other tool: graphersal-memory is the main memory and the rule block
//! names the tool read-only ([`read_only_line`]; the default for a known memory tool), switch its
//! MCP entry off ([`disable_edit`]; only with an explicit flag or a yes in the wizard, recorded
//! in the harness's install state as [`OFF_RECORD`] and undone by uninstall, [`restore_edit`]), or
//! keep both as they are (the default for a server recognised only by its name or tools, which
//! may be more than a memory). The choices live in `<data dir>/install/other-memory.json`
//! ([`Choices`]).

pub mod other_import;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::agents::Kind;
use crate::install::{DEVELOP_SERVER, Finding, Setup, State, USE_SERVER, read_text};
use crate::{fsutil, jsonedit, tomledit};

/// The record of a switched-off MCP entry in a harness's install state (the entry of the
/// configuration file): `{"<name>": {"path": [...], "how": "removed" | "flag" | "toml", ...}}`.
pub const OFF_RECORD: &str = "other_memory_off";

/// The choices file in the install state directory.
const CHOICES_FILE: &str = "other-memory.json";

/// The format version of the choices file.
const FORMAT: u64 = 1;

/// The memory formats graphersal-memory can import from another tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// The reference memory server's knowledge graph file (`memory.jsonl` / `memory.json`).
    MemoryServer,
    /// Basic Memory's folder of Markdown notes.
    BasicMemory,
}

/// A known memory tool.
#[derive(Debug)]
pub struct Tool {
    /// A short id.
    pub id: &'static str,
    /// The name people know.
    pub name: &'static str,
    /// Lower-case words that name it in an MCP entry's name, command, arguments, URL or a
    /// plugin's name.
    tokens: &'static [&'static str],
    /// Whether memory is what it is for (then "graphersal-memory is the main memory" is the
    /// safe default); false for a toolkit whose memory is one feature among others.
    pub primary: bool,
    /// Its format, when graphersal-memory can import it.
    pub format: Option<Format>,
}

/// The memory tools graphersal-memory recognises.
pub const TOOLS: &[Tool] = &[
    Tool {
        id: "server-memory",
        name: "the reference memory server (@modelcontextprotocol/server-memory)",
        tokens: &["@modelcontextprotocol/server-memory", "server-memory"],
        primary: true,
        format: Some(Format::MemoryServer),
    },
    Tool {
        id: "mem0",
        name: "mem0 / OpenMemory",
        tokens: &["mem0", "openmemory"],
        primary: true,
        format: None,
    },
    Tool {
        id: "claude-mem",
        name: "claude-mem",
        tokens: &["claude-mem"],
        primary: true,
        format: None,
    },
    Tool {
        id: "basic-memory",
        name: "Basic Memory",
        tokens: &["basic-memory", "basic_memory", "basicmemory"],
        primary: true,
        format: Some(Format::BasicMemory),
    },
    Tool {
        id: "serena",
        name: "Serena (a coding toolkit with memories)",
        tokens: &["serena"],
        primary: false,
        format: None,
    },
    Tool {
        id: "supermemory",
        name: "Supermemory",
        tokens: &["supermemory"],
        primary: true,
        format: None,
    },
    Tool {
        id: "byterover",
        name: "ByteRover / Cipher",
        tokens: &["byterover", "@byterover/cipher"],
        primary: true,
        format: None,
    },
    Tool {
        id: "graphiti",
        name: "Graphiti",
        tokens: &["graphiti"],
        primary: true,
        format: None,
    },
    Tool {
        id: "letta",
        name: "Letta",
        tokens: &["letta"],
        primary: true,
        format: None,
    },
    Tool {
        id: "cognee",
        name: "cognee",
        tokens: &["cognee"],
        primary: true,
        format: None,
    },
    Tool {
        id: "pieces",
        name: "Pieces",
        tokens: &["pieces"],
        primary: true,
        format: None,
    },
];

/// Tool names that say a server is a memory (seen in an entry or in permission rules; the
/// server itself is never started to list its tools).
pub const MEMORY_TOOL_NAMES: &[&str] = &[
    "remember",
    "recall",
    "memorize",
    "add_memory",
    "add_memories",
    "save_memory",
    "store_memory",
    "search_memory",
    "search_memories",
    "list_memories",
    "delete_memory",
    "create_entities",
    "add_observations",
    "read_graph",
];

/// The known tool `haystack` (lower-case name, command, arguments, URL) names; an entry named
/// exactly `cipher` is ByteRover's.
pub fn known_tool(haystack: &str, name: &str) -> Option<&'static Tool> {
    if name.eq_ignore_ascii_case("cipher") {
        return TOOLS.iter().find(|t| t.id == "byterover");
    }
    TOOLS
        .iter()
        .find(|tool| tool.tokens.iter().any(|token| haystack.contains(token)))
}

/// The state of another tool's MCP entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerState {
    /// Registered and on.
    On,
    /// Switched off by the user (`enabled = false`, `"disabled": true`).
    Off,
    /// Switched off by graphersal-memory's install (uninstall switches it on again).
    OffByUs,
}

/// Another memory tool's MCP entry.
#[derive(Debug, Clone)]
pub struct Server {
    /// The harness whose configuration holds it.
    pub harness: Kind,
    /// Its name in that configuration.
    pub name: String,
    /// The configuration file.
    pub file: PathBuf,
    /// The object that holds the entries (`mcpServers`, `mcp`, `mcp.servers`, `mcp_servers`).
    pub path: Vec<String>,
    /// `None` at user scope; else the project whose own registration it is (never changed by
    /// install).
    pub project: Option<String>,
    /// The known tool, when it is one.
    pub tool: Option<&'static Tool>,
    /// Why it counts as memory (for an unknown tool: its name or its tools).
    pub why: String,
    /// Whether it is on.
    pub state: ServerState,
    /// The entry itself.
    pub entry: Value,
}

impl Server {
    /// What it is, in a few words.
    pub fn what(&self) -> String {
        match self.tool {
            Some(tool) => tool.name.to_string(),
            None => format!("a memory tool? ({})", self.why),
        }
    }

    /// Whether "graphersal-memory is the main memory, this one read-only" is the default: for
    /// a known tool made for memory. A server recognised only by its name or tools may do more
    /// than memory (a home-made knowledge base), so it is kept as it is unless the user says.
    pub fn main_by_default(&self) -> bool {
        self.tool.is_some_and(|tool| tool.primary)
    }

    /// Where it is, for the output: the harness and the file (and the project for a project's
    /// own registration).
    pub fn place(&self, setup: &Setup) -> String {
        let file = setup.show(&self.file.display().to_string());
        match &self.project {
            Some(project) => format!(
                "{}, {file}, project {}",
                self.harness.display_name(),
                setup.show(project)
            ),
            None => format!("{}, {file}", self.harness.display_name()),
        }
    }
}

/// A memory plugin or hook of another tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plugin {
    /// The harness.
    pub harness: Kind,
    /// Its name (or command).
    pub name: String,
    /// Where it is configured.
    pub file: PathBuf,
}

/// A line of an instruction file about keeping memory elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    /// The file.
    pub file: PathBuf,
    /// The line number (1-based).
    pub line: usize,
    /// The line (trimmed, clipped).
    pub text: String,
    /// What it names: `MEMORY.md`, another tool, or `None` for a general memory instruction.
    pub target: Option<String>,
    /// The harnesses that load the file.
    pub loaded_by: Vec<Kind>,
}

impl Mention {
    /// The edit `doctor` suggests (the file is never changed by graphersal-memory).
    pub fn suggestion(&self) -> String {
        let keep = match &self.target {
            Some(target) if target == "MEMORY.md" => {
                "\"Memory lives in graphersal-memory (the MCP server `graphersal-memory`); do not \
                 write MEMORY.md.\""
                    .to_string()
            }
            Some(target) => format!(
                "\"Memory lives in graphersal-memory (the MCP server `graphersal-memory`); {target} \
                 is read-only: search it if needed, never write there.\""
            ),
            None => "\"Memory lives in graphersal-memory (the MCP server `graphersal-memory`).\""
                .to_string(),
        };
        format!(
            "if it tells the agent to keep memory there, it disagrees with graphersal-memory's \
             rule block (the agent is told to point that out). graphersal-memory never edits \
             your instruction files: delete the line, or change it to {keep}"
        )
    }
}

/// A harness's own memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltIn {
    /// The harness.
    pub harness: Kind,
    /// Its state and where it lives, in plain words.
    pub text: String,
    /// How many entries it holds (files), 0 when unknown.
    pub entries: usize,
}

/// A memory directory that another program (or the user by hand) writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Writer {
    /// The memory directory.
    pub dir: PathBuf,
    /// The generated-file header of its `MEMORY.md`, when it has one.
    pub header: Option<String>,
    /// How many imported files changed while no session of the project ran.
    pub outside: usize,
}

/// A file whose import brought back a memory forgotten or changed in the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Returning {
    /// The file.
    pub file: String,
    /// The memory.
    pub slug: String,
    /// `forgotten` or `changed`.
    pub what: String,
    /// When.
    pub at: String,
}

/// Everything found.
#[derive(Debug, Clone, Default)]
pub struct Findings {
    /// The harnesses' own memory.
    pub built_in: Vec<BuiltIn>,
    /// Other memory tools' MCP entries.
    pub servers: Vec<Server>,
    /// Memory plugins and hooks.
    pub plugins: Vec<Plugin>,
    /// Instruction file lines about memory elsewhere.
    pub mentions: Vec<Mention>,
    /// Memory directories another program writes.
    pub writers: Vec<Writer>,
    /// Files that brought back a memory.
    pub returning: Vec<Returning>,
    /// The files and directories the import ignores.
    pub ignored: Vec<String>,
}

impl Findings {
    /// Whether anything beyond the harnesses' own memory was found (then status and doctor
    /// show their "Other memory" section).
    pub fn anything(&self) -> bool {
        !self.servers.is_empty()
            || !self.plugins.is_empty()
            || !self.mentions.is_empty()
            || !self.writers.is_empty()
            || !self.returning.is_empty()
            || !self.ignored.is_empty()
            || self.built_in.iter().any(|b| b.harness != Kind::ClaudeCode)
    }

    /// The user-scope servers of the harnesses `kinds` (what install can name or switch off).
    pub fn user_servers(&self, kinds: &[Kind]) -> Vec<&Server> {
        self.servers
            .iter()
            .filter(|s| s.project.is_none() && kinds.contains(&s.harness))
            .collect()
    }

    /// The distinct names of the user-scope servers of `kinds`, in order of appearance.
    pub fn names(&self, kinds: &[Kind]) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for server in self.user_servers(kinds) {
            if !names.contains(&server.name) {
                names.push(server.name.clone());
            }
        }
        names
    }
}

/// The user's choices about other memory tools (`<data dir>/install/other-memory.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Choices {
    /// The servers the rule block names read-only (graphersal-memory is the main memory).
    pub read_only: Vec<String>,
    /// The servers kept as they are (not named).
    pub kept: Vec<String>,
}

impl Choices {
    fn path(setup: &Setup) -> PathBuf {
        setup.state_dir().join(CHOICES_FILE)
    }

    /// The recorded choices; `None` before an install recorded any.
    pub fn load(setup: &Setup) -> Option<Self> {
        let bytes = std::fs::read(Self::path(setup)).ok()?;
        let value: Value = serde_json::from_slice(&bytes).ok()?;
        let list = |key: &str| -> Vec<String> {
            value[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        };
        Some(Self {
            read_only: list("read_only"),
            kept: list("kept"),
        })
    }

    /// Writes the choices (removes the file when there are none).
    pub fn save(&self, setup: &Setup) -> Result<(), String> {
        let path = Self::path(setup);
        if self.read_only.is_empty() && self.kept.is_empty() {
            return crate::install::remove_file(&path);
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
        }
        let value = json!({"version": FORMAT, "read_only": self.read_only, "kept": self.kept});
        fsutil::write_atomic(
            &path,
            format!("{}\n", jsonedit::render(&value, Some("  "), 0)).as_bytes(),
        )
    }

    /// Removes the choices file (uninstall).
    pub fn remove(setup: &Setup) -> Result<(), String> {
        crate::install::remove_file(&Self::path(setup))
    }
}

/// The servers the installed rule block names read-only (for `inspect`).
pub fn read_only_names(setup: &Setup) -> Vec<String> {
    Choices::load(setup)
        .map(|choices| choices.read_only)
        .unwrap_or_default()
}

/// The first characters of the rule block's line about other memory tools.
pub const READ_ONLY_PREFIX: &str = "- Other memory on this machine:";

/// The rule block's line naming other memory tools read-only (empty for none).
pub fn read_only_line(names: &[String]) -> String {
    if names.is_empty() {
        return String::new();
    }
    let listed: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
    format!(
        "{READ_ONLY_PREFIX} the MCP server{} {}. graphersal-memory is the main memory: search {} \
         only when you need to, never write there; save with `remember` here.\n",
        if names.len() == 1 { "" } else { "s" },
        crate::install::join_and(&listed.iter().map(String::as_str).collect::<Vec<_>>()),
        if names.len() == 1 { "it" } else { "them" }
    )
}

/// A rule block text with the line naming `names` read-only appended.
pub fn rule_with(base: &str, names: &[String]) -> String {
    format!("{base}{}", read_only_line(names))
}

/// A rule block text without its line about other memory tools (to recognise a block of ours
/// whatever tools it named).
pub fn strip_read_only(text: &str) -> &str {
    match text.find(&format!("\n{READ_ONLY_PREFIX}")) {
        Some(at) => &text[..at + 1],
        None => text,
    }
}

// ----------------------------------------------------------------------------------------
// Detection
// ----------------------------------------------------------------------------------------

/// Lower-case text of an entry's name, command, arguments, URL and package fields.
fn haystack(name: &str, entry: &Value) -> String {
    let mut parts = vec![name.to_lowercase()];
    let mut push = |value: &Value| match value {
        Value::String(text) => parts.push(text.to_lowercase()),
        Value::Array(items) => {
            for item in items {
                if let Some(text) = item.as_str() {
                    parts.push(text.to_lowercase());
                }
            }
        }
        _ => {}
    };
    for key in ["command", "args", "url", "serverUrl", "httpUrl", "package"] {
        push(&entry[key]);
    }
    parts.join(" ")
}

/// The tool names an entry lists itself (allow lists, enabled/disabled tools).
fn entry_tools(entry: &Value) -> Vec<String> {
    let mut tools = Vec::new();
    for key in [
        "alwaysAllow",
        "autoApprove",
        "tools",
        "enabled_tools",
        "disabled_tools",
        "includeTools",
    ] {
        if let Some(items) = entry[key].as_array() {
            tools.extend(items.iter().filter_map(Value::as_str).map(str::to_string));
        }
    }
    tools
}

/// Whether a server name says memory (`memory`, `my-memories`, `kb-memory`).
fn name_says_memory(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("memory") || lower.contains("memories")
}

/// Classifies one entry: `Some((tool, why))` when it is memory.
fn classify(
    name: &str,
    entry: &Value,
    extra_tools: &[String],
) -> Option<(Option<&'static Tool>, String)> {
    if let Some(tool) = known_tool(&haystack(name, entry), name) {
        return Some((Some(tool), tool.id.to_string()));
    }
    let mut tools: Vec<String> = entry_tools(entry);
    tools.extend(extra_tools.iter().cloned());
    let memory_tools: Vec<String> = tools
        .into_iter()
        .filter(|t| MEMORY_TOOL_NAMES.contains(&t.to_lowercase().as_str()))
        .collect();
    if !memory_tools.is_empty() {
        let mut unique: Vec<String> = Vec::new();
        for tool in memory_tools {
            if !unique.contains(&tool) {
                unique.push(tool);
            }
        }
        return Some((None, format!("its tools: {}", unique.join(", "))));
    }
    if name_says_memory(name) {
        return Some((None, "its name says memory".to_string()));
    }
    None
}

/// Whether an entry is off by its own switch.
fn entry_off(entry: &Value) -> bool {
    entry["enabled"] == Value::Bool(false) || entry["disabled"] == Value::Bool(true)
}

/// The tools Claude Code's permission rules name per server (`mcp__<server>__<tool>`).
fn claude_permission_tools(setup: &Setup) -> BTreeMap<String, Vec<String>> {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let settings = crate::install::read_json(&setup.paths.settings, &mut Vec::new());
    for list in ["allow", "ask", "deny"] {
        for rule in settings["permissions"][list]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if let Some(rest) = rule.strip_prefix("mcp__")
                && let Some((server, tool)) = rest.split_once("__")
            {
                map.entry(server.to_string())
                    .or_default()
                    .push(tool.to_string());
            }
        }
    }
    map
}

/// The servers recorded as switched off by install, per (harness, file, name).
fn recorded_off(setup: &Setup) -> Vec<(Kind, PathBuf, String, Value)> {
    let mut out = Vec::new();
    for kind in Kind::ALL {
        let Ok(state) = State::load_of(setup, kind) else {
            continue;
        };
        for (file, entry) in state.files() {
            if let Some(records) = entry.get(OFF_RECORD).and_then(Value::as_object) {
                for (name, record) in records {
                    out.push((kind, PathBuf::from(file), name.clone(), record.clone()));
                }
            }
        }
    }
    out
}

/// Reads the servers of one JSON object of entries.
#[allow(clippy::too_many_arguments)]
fn servers_in(
    setup: &Setup,
    harness: Kind,
    file: &Path,
    path: &[&str],
    entries: &Value,
    project: Option<String>,
    permission_tools: &BTreeMap<String, Vec<String>>,
    off_by_us: &[(Kind, PathBuf, String, Value)],
    out: &mut Vec<Server>,
) {
    let Some(entries) = entries.as_object() else {
        return;
    };
    let ours = setup.ours();
    for (name, entry) in entries {
        if !entry.is_object()
            || name == USE_SERVER
            || name == DEVELOP_SERVER
            || ours.entry(entry)
            || (harness == Kind::OpenCode && name == "servers" && path == ["mcp"])
        {
            continue;
        }
        let ours_off = off_by_us
            .iter()
            .any(|(k, f, n, _)| *k == harness && f == file && n == name);
        let empty = Vec::new();
        let extra = permission_tools.get(name).unwrap_or(&empty);
        let Some((tool, why)) = classify(name, entry, extra) else {
            continue;
        };
        out.push(Server {
            harness,
            name: name.clone(),
            file: file.to_path_buf(),
            path: path.iter().map(|p| p.to_string()).collect(),
            project: project.clone(),
            tool,
            why,
            state: if ours_off {
                ServerState::OffByUs
            } else if entry_off(entry) {
                ServerState::Off
            } else {
                ServerState::On
            },
            entry: entry.clone(),
        });
    }
}

/// Every other memory tool's MCP entry on this machine (and in `project`'s own files).
pub fn servers(setup: &Setup, project: Option<&Path>) -> Vec<Server> {
    let mut out = Vec::new();
    let off_by_us = recorded_off(setup);
    let permission_tools = claude_permission_tools(setup);
    let none = BTreeMap::new();
    // Claude Code: user scope, local scopes, the project's .mcp.json.
    let config = crate::install::read_json(&setup.paths.user_config, &mut Vec::new());
    let user_file = setup.paths.user_config.clone();
    servers_in(
        setup,
        Kind::ClaudeCode,
        &user_file,
        &["mcpServers"],
        &config["mcpServers"],
        None,
        &permission_tools,
        &off_by_us,
        &mut out,
    );
    if let Some(projects) = config["projects"].as_object() {
        for (path, value) in projects {
            servers_in(
                setup,
                Kind::ClaudeCode,
                &user_file,
                &["projects", path, "mcpServers"],
                &value["mcpServers"],
                Some(path.clone()),
                &permission_tools,
                &[],
                &mut out,
            );
        }
    }
    if let Some(root) = project {
        let file = root.join(".mcp.json");
        let value = crate::install::read_json(&file, &mut Vec::new());
        servers_in(
            setup,
            Kind::ClaudeCode,
            &file,
            &["mcpServers"],
            &value["mcpServers"],
            Some(root.display().to_string()),
            &permission_tools,
            &[],
            &mut out,
        );
    }
    // opencode: mcp (1.x, and 2.x's legacy entries) and mcp.servers (2.x).
    let file = setup.opencode.config_file.clone();
    let value = crate::install::read_json(&file, &mut Vec::new());
    servers_in(
        setup,
        Kind::OpenCode,
        &file,
        &["mcp"],
        &value["mcp"],
        None,
        &none,
        &off_by_us,
        &mut out,
    );
    servers_in(
        setup,
        Kind::OpenCode,
        &file,
        &["mcp", "servers"],
        &value["mcp"]["servers"],
        None,
        &none,
        &off_by_us,
        &mut out,
    );
    // Codex: [mcp_servers.*].
    let file = setup.codex.config_file.clone();
    if let Ok(Some(text)) = read_text(&file)
        && let Ok(doc) = tomledit::parse(&text)
    {
        servers_in(
            setup,
            Kind::Codex,
            &file,
            &["mcp_servers"],
            &doc["mcp_servers"],
            None,
            &none,
            &off_by_us,
            &mut out,
        );
    }
    // Cursor: ~/.cursor/mcp.json and the project's .cursor/mcp.json.
    let file = setup.cursor.mcp_file.clone();
    let value = crate::install::read_json(&file, &mut Vec::new());
    servers_in(
        setup,
        Kind::Cursor,
        &file,
        &["mcpServers"],
        &value["mcpServers"],
        None,
        &none,
        &off_by_us,
        &mut out,
    );
    if let Some(root) = project {
        let file = root.join(".cursor").join("mcp.json");
        let value = crate::install::read_json(&file, &mut Vec::new());
        servers_in(
            setup,
            Kind::Cursor,
            &file,
            &["mcpServers"],
            &value["mcpServers"],
            Some(root.display().to_string()),
            &none,
            &[],
            &mut out,
        );
    }
    // Entries install switched off by removing them (Claude Code, Cursor): from the records.
    for (kind, file, name, record) in off_by_us {
        if out
            .iter()
            .any(|s| s.harness == kind && s.file == file && s.name == name)
        {
            continue;
        }
        let entry = record["entry"].clone();
        let (tool, why) = classify(&name, &entry, &[]).unwrap_or((
            None,
            "switched off by graphersal-memory install".to_string(),
        ));
        out.push(Server {
            harness: kind,
            name,
            file,
            path: record["path"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            project: None,
            tool,
            why,
            state: ServerState::OffByUs,
            entry,
        });
    }
    out
}

/// Memory plugins and hooks of other tools (by known names).
pub fn plugins(setup: &Setup) -> Vec<Plugin> {
    let mut out = Vec::new();
    let named = |text: &str| -> bool {
        let lower = text.to_lowercase();
        known_tool(&lower, text).is_some()
    };
    let settings = crate::install::read_json(&setup.paths.settings, &mut Vec::new());
    if let Some(enabled) = settings["enabledPlugins"].as_object() {
        for (name, on) in enabled {
            if *on != Value::Bool(false) && named(name) {
                out.push(Plugin {
                    harness: Kind::ClaudeCode,
                    name: name.clone(),
                    file: setup.paths.settings.clone(),
                });
            }
        }
    }
    let installed_file = setup
        .paths
        .config_dir
        .join("plugins")
        .join("installed_plugins.json");
    let installed = crate::install::read_json(&installed_file, &mut Vec::new());
    if let Some(plugins) = installed["plugins"].as_object() {
        for name in plugins.keys() {
            if named(name) && !out.iter().any(|p| p.name == *name) {
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
                    if !ours.hook(hook) && named(command) && !out.iter().any(|p| p.name == command)
                    {
                        out.push(Plugin {
                            harness: Kind::ClaudeCode,
                            name: command.to_string(),
                            file: setup.paths.settings.clone(),
                        });
                    }
                }
            }
        }
    }
    let config = crate::install::read_json(&setup.opencode.config_file, &mut Vec::new());
    for item in config["plugin"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if named(item) {
            out.push(Plugin {
                harness: Kind::OpenCode,
                name: item.to_string(),
                file: setup.opencode.config_file.clone(),
            });
        }
    }
    for dir in ["plugin", "plugins"] {
        let dir = setup.opencode.config_dir.join(dir);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name != crate::opencode::PLUGIN_FILE && named(name))
            .collect();
        names.sort();
        for name in names {
            out.push(Plugin {
                harness: Kind::OpenCode,
                name: name.clone(),
                file: dir.join(name),
            });
        }
    }
    out
}

/// The harnesses' own memory, for the harnesses whose program or configuration is here.
pub fn built_in(setup: &Setup) -> Vec<BuiltIn> {
    let mut out = Vec::new();
    let present = |kind: Kind| {
        setup.program(kind).is_some()
            || setup.leftovers().iter().any(|(k, _)| *k == kind)
            || State::exists_of(setup, kind)
    };
    if present(Kind::ClaudeCode) {
        let settings = crate::install::read_json(&setup.paths.settings, &mut Vec::new());
        let off = settings["autoMemoryEnabled"] == Value::Bool(false);
        let dirs = crate::import::scan(&setup.paths.projects).unwrap_or_default();
        let files: usize = dirs.iter().map(|d| d.files.len()).sum();
        out.push(BuiltIn {
            harness: Kind::ClaudeCode,
            text: format!(
                "Claude Code's auto-memory: {}; {files} memory file{} in {} project{} ({})",
                if off { "off" } else { "on" },
                if files == 1 { "" } else { "s" },
                dirs.len(),
                if dirs.len() == 1 { "" } else { "s" },
                setup.show(&setup.paths.projects.display().to_string())
            ),
            entries: files,
        });
    }
    if present(Kind::Codex) {
        let on = read_text(&setup.codex.config_file)
            .ok()
            .flatten()
            .and_then(|text| tomledit::parse(&text).ok())
            .is_some_and(|doc| crate::codex::install::memories_on(&doc));
        let mut files = 0;
        for dir in ["memories_v2", "memories"] {
            files += count_files(&setup.codex.codex_home.join(dir), 2);
        }
        if on || files > 0 {
            out.push(BuiltIn {
                harness: Kind::Codex,
                text: format!(
                    "Codex's memories: {}; {files} file{} in {}",
                    if on { "on" } else { "off" },
                    if files == 1 { "" } else { "s" },
                    setup.show(
                        &setup
                            .codex
                            .codex_home
                            .join("memories_v2")
                            .display()
                            .to_string()
                    )
                ),
                entries: files,
            });
        }
    }
    if present(Kind::Cursor) {
        out.push(BuiltIn {
            harness: Kind::Cursor,
            text: "Cursor's Memories: kept by Cursor itself, not readable from files (Cursor \
                   Settings, Rules & Memories, shows and switches them)"
                .to_string(),
            entries: 0,
        });
    }
    out
}

/// The number of files under `dir`, `depth` levels deep at most.
fn count_files(dir: &Path, depth: usize) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                if depth > 1 {
                    count_files(&path, depth - 1)
                } else {
                    0
                }
            } else {
                1
            }
        })
        .sum()
}

/// The instruction files of every harness (user scope, and `project`'s) with the harnesses
/// that load each one.
pub fn instruction_files(setup: &Setup, project: Option<&Path>) -> Vec<(PathBuf, Vec<Kind>)> {
    let mut files: Vec<(PathBuf, Vec<Kind>)> = Vec::new();
    let mut add = |path: PathBuf, kind: Kind| {
        if !path.is_file() {
            return;
        }
        match files.iter_mut().find(|(p, _)| *p == path) {
            Some((_, kinds)) => {
                if !kinds.contains(&kind) {
                    kinds.push(kind);
                }
            }
            None => files.push((path, vec![kind])),
        }
    };
    let md_in = |dir: &Path, ext: &str| -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == ext))
                    .collect()
            })
            .unwrap_or_default();
        found.sort();
        found
    };
    // Claude Code.
    add(setup.paths.instructions.clone(), Kind::ClaudeCode);
    for file in md_in(&setup.paths.config_dir.join("rules"), "md") {
        if file
            .file_name()
            .is_some_and(|n| n != crate::agents::GLOBAL_RULES_FILE)
        {
            add(file, Kind::ClaudeCode);
        }
    }
    // Codex.
    add(
        setup.codex.codex_home.join("AGENTS.override.md"),
        Kind::Codex,
    );
    add(setup.codex.codex_home.join("AGENTS.md"), Kind::Codex);
    // opencode: its AGENTS.md and the files listed in `instructions` (not ours, no globs).
    add(setup.opencode.config_dir.join("AGENTS.md"), Kind::OpenCode);
    let config = crate::install::read_json(&setup.opencode.config_file, &mut Vec::new());
    for item in config["instructions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if item.contains('*') || item.ends_with(crate::opencode::RULES_FILE) {
            continue;
        }
        let path = if let Some(rest) = item.strip_prefix("~/") {
            setup.opencode.home.join(rest)
        } else if Path::new(item).is_absolute() {
            PathBuf::from(item)
        } else {
            setup.opencode.config_dir.join(item)
        };
        add(path, Kind::OpenCode);
    }
    // opencode also loads Claude Code's files where it has none of its own (its "Claude Code
    // compatibility"): ~/.claude/CLAUDE.md when it has no global AGENTS.md, unless switched off
    // by OPENCODE_DISABLE_CLAUDE_CODE(_PROMPT).
    let opencode_reads_claude = setup.opencode.config_dir.is_dir()
        && std::env::var_os("OPENCODE_DISABLE_CLAUDE_CODE").is_none();
    if opencode_reads_claude
        && std::env::var_os("OPENCODE_DISABLE_CLAUDE_CODE_PROMPT").is_none()
        && !setup.opencode.config_dir.join("AGENTS.md").is_file()
    {
        add(setup.paths.instructions.clone(), Kind::OpenCode);
    }
    // Codex: the extra file names of `project_doc_fallback_filenames` (CLAUDE.md, for example).
    let codex_fallbacks: Vec<String> = std::fs::read_to_string(&setup.codex.config_file)
        .ok()
        .and_then(|text| crate::tomledit::parse(&text).ok())
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
    // Cursor: user rules other than ours.
    for file in md_in(&setup.cursor.cursor_dir.join("rules"), "mdc") {
        if file != setup.cursor.rules_file {
            add(file, Kind::Cursor);
        }
    }
    if let Some(root) = project {
        add(root.join("CLAUDE.md"), Kind::ClaudeCode);
        add(root.join("CLAUDE.local.md"), Kind::ClaudeCode);
        add(root.join(".claude").join("CLAUDE.md"), Kind::ClaudeCode);
        for file in md_in(&root.join(".claude").join("rules"), "md") {
            add(file, Kind::ClaudeCode);
        }
        for kind in [Kind::Codex, Kind::OpenCode, Kind::Cursor] {
            add(root.join("AGENTS.md"), kind);
        }
        add(root.join("AGENTS.override.md"), Kind::Codex);
        // opencode: the project's CLAUDE.md when the project has no AGENTS.md.
        if opencode_reads_claude && !root.join("AGENTS.md").is_file() {
            add(root.join("CLAUDE.md"), Kind::OpenCode);
        }
        // Codex: its fallback names, when the project has no AGENTS(.override).md.
        if !root.join("AGENTS.md").is_file()
            && !root.join("AGENTS.override.md").is_file()
            && let Some(name) = codex_fallbacks.iter().find(|n| root.join(n).is_file())
        {
            add(root.join(name), Kind::Codex);
        }
        add(root.join(".cursorrules"), Kind::Cursor);
        for file in md_in(&root.join(".cursor").join("rules"), "mdc") {
            add(file, Kind::Cursor);
        }
    }
    files
}

/// The places of `text` that point the agent at other memory, as (line, paragraph, target).
///
/// Language-neutral on purpose (an instruction file may be in any language): only names count
/// (`MEMORY.md`, a known memory tool, the name of another memory server found on this machine,
/// the tool names of memory servers), never words such as "keep" or "do not". The text is read
/// as Markdown paragraphs and list items (a wrapped line belongs to its item); a paragraph that
/// names graphersal-memory is about graphersal-memory and skipped. Whether a mention really
/// tells the agent to keep memory elsewhere is left to the agent, whose rule block tells it to
/// point such an instruction out to the user; this only shows the place.
pub fn mentions_in(text: &str, names: &[String]) -> Vec<(usize, String, Option<String>)> {
    let mut out = Vec::new();
    for unit in units(text) {
        let joined: String = unit
            .iter()
            .map(|(_, line)| line.trim())
            .collect::<Vec<_>>()
            .join(" ");
        let lower = joined.to_lowercase();
        if lower.contains("graphersal-memory") || lower.contains("graphersal memory") {
            continue;
        }
        let found = unit.iter().find_map(|(n, line)| {
            anchor_in(&line.to_lowercase(), names).map(|target| (*n, target))
        });
        if let Some((line, target)) = found {
            out.push((line, clip(&joined, 160), Some(target)));
        }
    }
    out
}

/// The name in a lower-case line that points at other memory: `MEMORY.md`, a known memory tool,
/// another memory server's name, or a memory tool name with an underscore (`create_entities`;
/// plain words like "remember" are left out: they are words of a language).
fn anchor_in(lower: &str, names: &[String]) -> Option<String> {
    let words: Vec<&str> = lower
        .split(|c: char| {
            !(c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '@' || c == '/')
        })
        .map(|w| w.trim_end_matches('.'))
        .filter(|w| !w.is_empty())
        .collect();
    if words
        .iter()
        .any(|w| *w == "memory.md" || w.ends_with("/memory.md"))
    {
        return Some("MEMORY.md".to_string());
    }
    // "pieces" is an English word: Pieces counts only as a server found on this machine.
    if let Some(tool) = TOOLS.iter().filter(|t| t.id != "pieces").find(|tool| {
        tool.tokens.iter().any(|token| {
            words
                .iter()
                .any(|w| w == token || w.contains(token) && token.len() > 5)
        })
    }) {
        return Some(tool.tokens[0].to_string());
    }
    if let Some(name) = names
        .iter()
        .filter(|name| !name_is_generic(name))
        .find(|name| words.contains(&name.to_lowercase().as_str()))
    {
        return Some(name.clone());
    }
    MEMORY_TOOL_NAMES
        .iter()
        .filter(|tool| tool.contains('_'))
        .find(|tool| {
            words
                .iter()
                .any(|w| w == *tool || w.ends_with(&format!("__{tool}")))
        })
        .map(|tool| tool.to_string())
}

/// The Markdown units of `text`: paragraphs and list items as their (1-based line, text) lines;
/// headings, table rows, code-block lines and the managed block's lines are units of one line
/// (the managed block's own lines are left out).
fn units(text: &str) -> Vec<Vec<(usize, &str)>> {
    fn flush<'a>(current: &mut Vec<(usize, &'a str)>, units: &mut Vec<Vec<(usize, &'a str)>>) {
        if !current.is_empty() {
            units.push(std::mem::take(current));
        }
    }
    let mut units: Vec<Vec<(usize, &str)>> = Vec::new();
    let mut current: Vec<(usize, &str)> = Vec::new();
    let (mut in_block, mut in_code) = (false, false);
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        let trimmed = line.trim();
        if trimmed.starts_with(crate::install::BLOCK_BEGIN) {
            flush(&mut current, &mut units);
            in_block = true;
            continue;
        }
        if trimmed.starts_with(crate::install::BLOCK_END) {
            in_block = false;
            continue;
        }
        if in_block {
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush(&mut current, &mut units);
            in_code = !in_code;
            continue;
        }
        if in_code || trimmed.starts_with('#') || trimmed.starts_with('|') {
            flush(&mut current, &mut units);
            if !trimmed.is_empty() {
                units.push(vec![(n, line)]);
            }
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut current, &mut units);
            continue;
        }
        if starts_list_item(trimmed) || trimmed.starts_with('>') {
            flush(&mut current, &mut units);
        }
        current.push((n, line));
    }
    flush(&mut current, &mut units);
    units
}

/// Whether a trimmed line starts a list item (`- `, `* `, `+ `, `1. `, `1) `).
pub(crate) fn starts_list_item(trimmed: &str) -> bool {
    if ["- ", "* ", "+ "].iter().any(|m| trimmed.starts_with(m)) {
        return true;
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && (trimmed[digits..].starts_with(". ") || trimmed[digits..].starts_with(") "))
}

/// Whether a server name is a common word (`memory`), which says nothing in a text.
fn name_is_generic(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "memory" | "memories" | "notes" | "kb" | "knowledge"
    )
}

/// `text` cut to `max` characters (at a character boundary).
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}...")
}

/// Instruction file lines about keeping memory elsewhere.
pub fn mentions(setup: &Setup, project: Option<&Path>, names: &[String]) -> Vec<Mention> {
    let mut out = Vec::new();
    for (file, loaded_by) in instruction_files(setup, project) {
        let Ok(Some(text)) = read_text(&file) else {
            continue;
        };
        if text.len() > 1 << 20 {
            continue;
        }
        for (line, text, target) in mentions_in(&text, names) {
            out.push(Mention {
                file: file.clone(),
                line,
                text,
                target,
                loaded_by: loaded_by.clone(),
            });
        }
    }
    out
}

/// Header words of a generated file.
const GENERATED: &[&str] = &[
    "generated by",
    "auto-generated",
    "autogenerated",
    "do not edit",
    "don't edit",
    "this file is generated",
    "exported by",
    "exported from",
];

/// The generated-file header among the first lines of `text`, when there is one.
pub fn generated_header(text: &str) -> Option<String> {
    text.lines()
        .take(6)
        .find(|line| {
            let lower = line.to_lowercase();
            GENERATED.iter().any(|w| lower.contains(w))
        })
        .map(|line| clip(line.trim(), 120))
}

/// Seconds since the epoch of a `YYYY-MM-DDTHH:MM:SS...` text (UTC).
pub fn parse_utc(text: &str) -> Option<u64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let num = |a: usize, b: usize| text.get(a..b)?.parse::<i64>().ok();
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    // Days from civil (H. Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86400 + hh * 3600 + mm * 60 + ss).ok()
}

/// A file's modification time in seconds since the epoch.
fn mtime(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// The time spans of the Claude Code sessions of a project directory: from each transcript's
/// first record with a timestamp to its last change.
pub fn session_spans(project_dir: &Path) -> Vec<(u64, u64)> {
    let Ok(entries) = std::fs::read_dir(project_dir) else {
        return Vec::new();
    };
    let mut spans = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "jsonl") {
            continue;
        }
        let Some(end) = mtime(&path) else { continue };
        let start = first_timestamp(&path).unwrap_or(end);
        spans.push((start.min(end), end));
    }
    spans
}

/// The first `timestamp` among a transcript's first lines.
fn first_timestamp(path: &Path) -> Option<u64> {
    use std::io::{BufRead, BufReader};
    let file = std::fs::File::open(path).ok()?;
    for line in BufReader::new(file).lines().take(50) {
        let line = line.ok()?;
        if let Ok(value) = serde_json::from_str::<Value>(&line)
            && let Some(at) = value["timestamp"].as_str()
        {
            return parse_utc(at);
        }
    }
    None
}

/// How far a memory file's change may lie outside a session's span and still count as the
/// session's (seconds).
const SPAN_SLACK: u64 = 120;

/// The memory directories another program (or the user by hand) writes: a generated-file
/// header in `MEMORY.md`, or imported files changed while no session of the project ran (only
/// changes after the oldest transcript count: older sessions are gone).
pub fn writers(setup: &Setup) -> Vec<Writer> {
    let Ok(state) = crate::import::ImportState::load(&setup.data_dir) else {
        return Vec::new();
    };
    let mut by_dir: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for (path, _) in state.entries() {
        let path = PathBuf::from(path);
        if let Some(dir) = path.parent() {
            by_dir.entry(dir.to_path_buf()).or_default().push(path);
        }
    }
    let Ok(dirs) = crate::import::scan(&setup.paths.projects) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for dir in dirs {
        let header = read_text(&dir.dir.join("MEMORY.md"))
            .ok()
            .flatten()
            .and_then(|text| generated_header(&text));
        let mut outside = 0;
        if let (Some(files), Some(project_dir)) = (by_dir.get(&dir.dir), dir.dir.parent()) {
            let spans = session_spans(project_dir);
            if let Some(oldest) = spans.iter().map(|(start, _)| *start).min() {
                for file in files {
                    let Some(at) = mtime(file) else { continue };
                    let in_session = spans
                        .iter()
                        .any(|(s, e)| at + SPAN_SLACK >= *s && at <= e + SPAN_SLACK);
                    if at >= oldest && !in_session {
                        outside += 1;
                    }
                }
            }
        }
        if header.is_some() || outside > 0 {
            out.push(Writer {
                dir: dir.dir.clone(),
                header,
                outside,
            });
        }
    }
    out
}

/// The files whose import brought back a memory (from the import's records).
pub fn returning(setup: &Setup) -> Vec<Returning> {
    let Ok(state) = crate::import::ImportState::load(&setup.data_dir) else {
        return Vec::new();
    };
    state
        .returned()
        .into_iter()
        .map(|(file, entry)| Returning {
            file,
            slug: entry["slug"].as_str().unwrap_or_default().to_string(),
            what: entry["returned"]["what"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            at: entry["returned"]["at"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        })
        .collect()
}

/// Everything about other memory on this machine (`project`: the project whose own files are
/// read too; `None`: user scope only).
pub fn detect(setup: &Setup, project: Option<&Path>) -> Findings {
    let servers = servers(setup, project);
    let mut names: Vec<String> = Vec::new();
    for server in &servers {
        if !names.contains(&server.name) {
            names.push(server.name.clone());
        }
    }
    Findings {
        built_in: built_in(setup),
        mentions: mentions(setup, project, &names),
        plugins: plugins(setup),
        writers: writers(setup),
        returning: returning(setup),
        ignored: crate::import::ImportState::load(&setup.data_dir)
            .map(|state| state.ignored().to_vec())
            .unwrap_or_default(),
        servers,
    }
}

// ----------------------------------------------------------------------------------------
// Switching another tool's entry off and on again
// ----------------------------------------------------------------------------------------

/// The opencode switch of an entry in `path`: 2.x entries (`mcp.servers`) have `"disabled"`,
/// 1.x and legacy ones `"enabled"`. Returns (key, value when off).
fn opencode_switch(path: &[String]) -> (&'static str, bool) {
    if path.len() == 2 && path[1] == "servers" {
        ("disabled", true)
    } else {
        ("enabled", false)
    }
}

/// `text` (the server's configuration file) with `server` switched off, and the record that
/// undoes it; `None` when it cannot be done in place (then the user is told how).
/// - Claude Code and Cursor have no switched-off state for a user-scope entry: it is removed
///   and kept in the record (`how: removed`, with the entry).
/// - opencode: `"enabled": false` (1.x) or `"disabled": true` (2.x) (`how: flag`, with the
///   value before, `null` when absent).
/// - Codex: `enabled = false` in its table (`how: toml`, with the value before).
pub fn disable_edit(server: &Server, text: &str) -> Result<Option<(String, Value)>, String> {
    let path: Vec<&str> = server.path.iter().map(String::as_str).collect();
    match server.harness {
        Kind::ClaudeCode | Kind::Cursor => Ok(jsonedit::remove_member(text, &path, &server.name)?
            .map(|new| {
                (
                    new,
                    json!({"how": "removed", "path": server.path, "entry": server.entry}),
                )
            })),
        Kind::OpenCode => {
            let (key, off) = opencode_switch(&server.path);
            let previous = server.entry.get(key).cloned().unwrap_or(Value::Null);
            let mut entry_path = path.clone();
            entry_path.push(&server.name);
            Ok(jsonedit::set_member(text, &entry_path, key, &json!(off))?.map(|(new, _)| {
                (
                    new,
                    json!({"how": "flag", "path": server.path, "key": key, "previous": previous}),
                )
            }))
        }
        Kind::Codex => {
            let previous = server.entry.get("enabled").cloned().unwrap_or(Value::Null);
            Ok(
                tomledit::set_table_bool(text, &["mcp_servers", &server.name], "enabled", false)?
                    .map(|new| {
                        (
                            new,
                            json!({"how": "toml", "path": server.path, "key": "enabled",
                                   "previous": previous}),
                        )
                    }),
            )
        }
    }
}

/// `text` with the entry `name` that install switched off (`record`) switched on again, or
/// `None` when nothing is to do (it is back already: the user re-added or switched it on, or
/// it is gone).
pub fn restore_edit(
    _harness: Kind,
    text: &str,
    name: &str,
    record: &Value,
) -> Result<Option<String>, String> {
    let path: Vec<String> = record["path"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let path: Vec<&str> = path.iter().map(String::as_str).collect();
    let key = record["key"].as_str().unwrap_or("enabled");
    let previous = &record["previous"];
    match record["how"].as_str() {
        Some("removed") => {
            let parsed = jsonedit::value_of(text).map_err(|err| err.to_string())?;
            let at = path.iter().fold(&parsed, |v, k| &v[*k]);
            if !at[name].is_null() {
                return Ok(None);
            }
            Ok(jsonedit::set_member(text, &path, name, &record["entry"])?.map(|(new, _)| new))
        }
        Some("flag") => {
            let parsed = jsonedit::value_of(text).map_err(|err| err.to_string())?;
            let entry = &path.iter().fold(&parsed, |v, k| &v[*k])[name];
            let (_, off) = opencode_switch(&path.iter().map(|p| p.to_string()).collect::<Vec<_>>());
            if entry[key] != json!(off) {
                return Ok(None);
            }
            let mut entry_path = path.clone();
            entry_path.push(name);
            if previous.is_null() {
                jsonedit::remove_member(text, &entry_path, key)
            } else {
                Ok(jsonedit::set_member(text, &entry_path, key, previous)?.map(|(new, _)| new))
            }
        }
        Some("toml") => {
            let doc = tomledit::parse(text)?;
            if doc["mcp_servers"][name][key] != Value::Bool(false) {
                return Ok(None);
            }
            match previous {
                Value::Bool(value) => {
                    tomledit::set_table_bool(text, &["mcp_servers", name], key, *value)
                }
                _ => tomledit::remove_table_key(text, &["mcp_servers", name], key),
            }
        }
        _ => Ok(None),
    }
}

/// How to switch `server` off by hand (when install cannot, or for a project's registration).
pub fn off_by_hand(server: &Server) -> String {
    match (server.harness, &server.project) {
        (Kind::ClaudeCode, Some(_)) if server.file.ends_with(".mcp.json") => format!(
            "remove `{}` from that project's .mcp.json, or deny it in its .claude/settings.json",
            server.name
        ),
        (Kind::ClaudeCode, Some(_)) => {
            format!(
                "in that project: `claude mcp remove -s local {}`",
                server.name
            )
        }
        (Kind::ClaudeCode, None) => format!("`claude mcp remove -s user {}`", server.name),
        (Kind::OpenCode, _) => {
            let (key, off) = opencode_switch(&server.path);
            format!("set \"{key}\": {off} in its entry")
        }
        (Kind::Codex, _) => format!("set `enabled = false` in [mcp_servers.{}]", server.name),
        (Kind::Cursor, _) => format!("switch {} off in Cursor Settings, MCP", server.name),
    }
}

// ----------------------------------------------------------------------------------------
// Output
// ----------------------------------------------------------------------------------------

/// What happens to one server under `choices`, in a few words.
fn choice_words(server: &Server, choices: Option<&Choices>) -> String {
    match server.state {
        ServerState::OffByUs => "switched off by graphersal-memory install (uninstall switches it \
            on again; `graphersal-memory install --enable-other NAME` now)"
            .replace("NAME", &server.name),
        ServerState::Off => "switched off (by you)".to_string(),
        ServerState::On if server.project.is_some() => {
            "a project's own registration (install leaves it as it is)".to_string()
        }
        ServerState::On => match choices {
            Some(c) if c.read_only.contains(&server.name) => {
                "named read-only in the rule block (graphersal-memory is the main memory)"
                    .to_string()
            }
            Some(c) if c.kept.contains(&server.name) => {
                "kept as it is (your choice; the rule block does not name it)".to_string()
            }
            _ => "not chosen yet (`graphersal-memory install` asks)".to_string(),
        },
    }
}

/// The "Other memory" lines of `status`: a heading, then what was found and the choice made.
pub fn status_lines(setup: &Setup, findings: &Findings) -> Vec<String> {
    if !findings.anything() {
        return Vec::new();
    }
    let choices = Choices::load(setup);
    let mut lines = vec!["Other memory:".to_string()];
    for built_in in &findings.built_in {
        lines.push(format!("  {}", built_in.text));
    }
    for server in &findings.servers {
        lines.push(format!(
            "  {} ({}): {}; {}",
            server.name,
            server.place(setup),
            server.what(),
            choice_words(server, choices.as_ref())
        ));
    }
    for plugin in &findings.plugins {
        lines.push(format!(
            "  {} plugin or hook {} ({}): another memory beside graphersal-memory",
            plugin.harness.display_name(),
            plugin.name,
            setup.show(&plugin.file.display().to_string())
        ));
    }
    for mention in &findings.mentions {
        lines.push(format!(
            "  {}:{} says: \"{}\" (never edited; `graphersal-memory doctor` suggests an edit)",
            setup.show(&mention.file.display().to_string()),
            mention.line,
            mention.text
        ));
    }
    for writer in &findings.writers {
        lines.push(format!("  {}", writer_text(setup, writer)));
    }
    for returning in &findings.returning {
        lines.push(format!("  {}", returning_text(setup, returning)));
    }
    for ignored in &findings.ignored {
        lines.push(format!(
            "  not imported (ignored): {} (`graphersal-memory import --unignore {}` imports it again)",
            setup.show(ignored),
            setup.show(ignored)
        ));
    }
    lines
}

/// One writer, in plain words.
fn writer_text(setup: &Setup, writer: &Writer) -> String {
    let dir = setup.show(&writer.dir.display().to_string());
    match (&writer.header, writer.outside) {
        (Some(header), 0) => format!("{dir}: written by another program (MEMORY.md: \"{header}\")"),
        (Some(header), n) => format!(
            "{dir}: written by another program (MEMORY.md: \"{header}\"); {n} file{} changed \
             outside any Claude Code session",
            if n == 1 { "" } else { "s" }
        ),
        (None, n) => format!(
            "{dir}: {n} imported file{} changed outside any Claude Code session (by another \
             program, or by hand)",
            if n == 1 { "" } else { "s" }
        ),
    }
}

/// One returning fact, in plain words.
fn returning_text(setup: &Setup, returning: &Returning) -> String {
    format!(
        "{} brought back the memory {} ({} in the graph; {})",
        setup.show(&returning.file),
        returning.slug,
        if returning.what == "forgotten" {
            "forgotten"
        } else {
            "changed by an agent"
        },
        returning.at.get(..10).unwrap_or(&returning.at)
    )
}

/// The "Other memory" findings of `doctor`: notes with what to do (never problems: nothing
/// here breaks graphersal-memory; never ACTION NEEDED: the user decides).
pub fn doctor_findings(setup: &Setup, findings: &Findings) -> Vec<Finding> {
    let choices = Choices::load(setup);
    let mut out = Vec::new();
    for built_in in &findings.built_in {
        out.push(crate::install::ok(built_in.text.clone()));
    }
    for server in &findings.servers {
        let text = format!(
            "{} ({}): {}; {}",
            server.name,
            server.place(setup),
            server.what(),
            choice_words(server, choices.as_ref())
        );
        let undecided = server.state == ServerState::On
            && server.project.is_none()
            && !choices
                .as_ref()
                .is_some_and(|c| c.read_only.contains(&server.name));
        if server.state == ServerState::On && server.project.is_some() {
            out.push(crate::install::note(
                format!("{text}; the agent sees two memories in that project"),
                format!("to switch it off: {}", off_by_hand(server)),
            ));
        } else if undecided {
            out.push(crate::install::note(
                format!("{text}; the agent sees two memories and may split facts between them"),
                format!(
                    "name it read-only in the rule block: `graphersal-memory install \
                     --other-read-only {}`; or switch it off (uninstall switches it on again): \
                     `graphersal-memory install --disable-other {}`",
                    server.name, server.name
                ),
            ));
        } else {
            out.push(crate::install::ok(text));
        }
    }
    for plugin in &findings.plugins {
        out.push(crate::install::note(
            format!(
                "{} plugin or hook {} ({}) keeps memory beside graphersal-memory",
                plugin.harness.display_name(),
                plugin.name,
                setup.show(&plugin.file.display().to_string())
            ),
            "two memories split facts; switch the plugin off in its harness if graphersal-memory \
             is to be the only one (graphersal-memory does not change plugins)",
        ));
    }
    for mention in &findings.mentions {
        out.push(crate::install::note(
            format!(
                "{}:{} says: \"{}\"",
                setup.show(&mention.file.display().to_string()),
                mention.line,
                mention.text
            ),
            mention.suggestion(),
        ));
    }
    for writer in &findings.writers {
        out.push(crate::install::note(
            writer_text(setup, writer),
            format!(
                "graphersal-memory imports these files at each session start, so what is written \
                 there comes back into the graph, also over a memory changed or forgotten there. \
                 If a program of yours writes them, stop it; or stop importing them: \
                 `graphersal-memory import --ignore {}`",
                setup.show(&writer.dir.display().to_string())
            ),
        ));
    }
    for returning in &findings.returning {
        out.push(crate::install::note(
            returning_text(setup, returning),
            format!(
                "edit or delete the file (or stop the program that writes it), or stop importing \
                 it: `graphersal-memory import --ignore {}`",
                setup.show(&returning.file)
            ),
        ));
    }
    for ignored in &findings.ignored {
        out.push(crate::install::ok(format!(
            "not imported (ignored): {}",
            setup.show(ignored)
        )));
    }
    out
}
