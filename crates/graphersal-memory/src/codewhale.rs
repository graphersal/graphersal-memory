//! CodeWhale (github.com/Hmbown/CodeWhale; verified against the source of release `v0.10.2`,
//! each fact names its file): where it keeps its files, what graphersal-memory writes there, and
//! how its sessions are read. CodeWhale was called "DeepSeek TUI" before its rename: many
//! variables are still `DEEPSEEK_*`, and `~/.deepseek` is read where CodeWhale still reads it.
//!
//! - **Home**: `$CODEWHALE_HOME` when set (absolute, `~` expanded), else `~/.codewhale`, the
//!   home directory being `HOME`, then `USERPROFILE` (`crates/paths/src/lib.rs`,
//!   `codewhale_home`, `user_home`). Without `CODEWHALE_HOME`, a file or directory that is
//!   missing under `~/.codewhale` but present under `~/.deepseek` is read from there
//!   (`crates/config/src/lib.rs` `resolve_state_dir`, `crates/tui/src/config/paths.rs`
//!   `default_user_state_path`), so install writes into the file CodeWhale reads: creating
//!   `~/.codewhale/mcp.json` beside a legacy `~/.deepseek/mcp.json` would hide the legacy one.
//! - **Configuration**: `config.toml` there (`--config`, `CODEWHALE_CONFIG_PATH`,
//!   `DEEPSEEK_CONFIG_PATH` move it; `default_config_path`). CodeWhale edits it itself through
//!   `toml_edit` (comments kept, `crates/tui/src/config_persistence.rs`).
//! - **MCP servers** (`crates/tui/src/mcp.rs`, `docs/MCP.md`): `mcp.json` in the home (moved
//!   by `CODEWHALE_MCP_CONFIG`/`DEEPSEEK_MCP_CONFIG` or `mcp_config_path` in `config.toml`),
//!   strict JSON (no comments), the servers under `servers` or `mcpServers` (exactly one: a
//!   file with both does not parse), each `{command, args, env, disabled}` (`enabled` and
//!   `disabled` both honoured). The file must be a regular file, not a symbolic link. The model
//!   sees a tool as `mcp_<server>_<tool>`, the server name unchanged
//!   (`mcp_model_tool_name`), so ours are `mcp_graphersal-memory_<function>` ([`tool_names`]).
//!   Every MCP tool is deferred (the model must `tool_search` it first) unless its exact name
//!   is in `[tools] always_load` of `config.toml` (`core/engine/tool_catalog.rs`
//!   `apply_mcp_tool_deferral`): install adds our `use` tools there. The develop server is
//!   registered with `"disabled": true` until `--enable-develop`.
//! - **Rules** (`crates/tui/src/project_context.rs`): one global instruction file, the first
//!   that exists of `~/.codewhale/AGENTS.md`, `~/.agents/AGENTS.md`, `~/.deepseek/AGENTS.md`,
//!   then the same three `instructions.md`, always under the home directory (not
//!   `CODEWHALE_HOME`), loaded before the project's `AGENTS.md` files. All instructions share
//!   one budget of 48 KiB that drops text from the front, the global file first
//!   (`MAX_PROJECT_INSTRUCTION_BYTES`, `keep_tail_within`): `doctor` warns when a project's
//!   files leave no room for the block ([`INSTRUCTION_BUDGET`]).
//! - **Session start and end**: no hooks. CodeWhale's `session_start`/`session_end` hooks run
//!   only with `[hooks] enabled` (a missing `[hooks]` table means every hook off) and carry a
//!   per-launch id that is not the saved session's id (`crates/tui/src/hooks/executor.rs`).
//!   Its stdio MCP servers start with the TUI and stay for its whole run (`docs/MCP.md`,
//!   "Connection Lifecycle"), so the connection does the work: the shim queues the indexing at
//!   its start (`crate::session_sync`); CodeWhale kills the server when it exits (seen live), so
//!   the daemon, seeing the connection close, indexes the project's sessions written meanwhile as
//!   ended (`crate::daemon`, `end_codewhale_sessions`).
//! - **Approvals**: CodeWhale asks before each call of a user-configured MCP tool, at most once
//!   per tool and run with "Allow for this conversation"; `permissions.toml` rules do not apply
//!   to MCP tools in 0.10.2 (`core/engine/turn_loop.rs`). Install changes nothing there.
//! - **Sessions** (`crates/tui/src/session_manager.rs`): one JSON document per session,
//!   `<home>/sessions/<uuid>.json`, rewritten whole and atomically after every turn; read by
//!   [`sessions`].
//! - **Headless runs**: `codewhale exec --json --prompt-file -` reads the prompt from stdin and
//!   prints one summary object (`output`, `usage`, `status`; no cost). A one-shot run has no
//!   tools, fires no hooks and saves no session (`crates/tui/src/exec_agent.rs`), so the
//!   summaries run it in a working directory of their own with the user's configuration. The
//!   bench does not run through CodeWhale yet ([`NO_BENCH`]).
//! - **Memory** (`crates/runtime/src/native_memory.rs`, `crates/memory/src/sql/schema.sql`): an
//!   opt-in native memory (`[memory] enabled = true`, `[memory] backend = "native"`, or
//!   `CODEWHALE_MEMORY`/`DEEPSEEK_MEMORY`), a SQLite store `<memory root>/store.sqlite3`
//!   (`<directory of memory_path>/memory`, default `~/.codewhale/memory`); the `MEMORY.md` paths
//!   its docs name are labels only. The wizard offers to switch it off (restored by uninstall),
//!   and its reviewed entries are imported read-only ([`memory`]). The export of `uninstall`
//!   writes readable Markdown files into `~/graphersal-memory-export/codewhale/`.

pub mod install;
pub mod memory;
pub mod other;
pub mod sessions;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::agents::{Harness, Kind};

/// The default headless command of CodeWhale's summaries (the model and provider of the user's
/// own configuration).
pub const DEFAULT_HEADLESS_COMMAND: &str = "codewhale exec";

/// The aggregate instruction budget of CodeWhale (`MAX_PROJECT_INSTRUCTION_BYTES`, 48 KiB): the
/// global file is dropped first when the project's files fill it.
pub const INSTRUCTION_BUDGET: u64 = 48 * 1024;

/// How long CodeWhale waits for our MCP server's start (seconds; its default is 30, and the
/// first connection may start the daemon and open the store).
pub const CONNECT_TIMEOUT_SECS: u64 = 60;

/// The providers that run a model on this machine (`docs/PROVIDERS.md`, "Local Models"):
/// summaries through them cost nothing.
pub const LOCAL_PROVIDERS: [&str; 5] = ["ollama", "vllm", "sglang", "ds4", "lm-studio"];

/// Why the bench does not run through CodeWhale (yet).
pub const NO_BENCH: &str = "CodeWhale saves every run that uses tools as a session in your \
    CodeWhale home, and a run in a home of its own would need a copy of your provider \
    credentials.";

/// The tool names CodeWhale shows the model for our `use` server's functions.
pub fn tool_names() -> Vec<String> {
    crate::catalog::FUNCTIONS
        .iter()
        .map(|function| format!("mcp_{}_{function}", crate::install::USE_SERVER))
        .collect()
}

/// Where CodeWhale keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeWhalePaths {
    /// The home directory (CodeWhale's: `HOME`, then `USERPROFILE`).
    pub home: PathBuf,
    /// The CodeWhale home (`$CODEWHALE_HOME` or `~/.codewhale`).
    pub codewhale_home: PathBuf,
    /// Whether `CODEWHALE_HOME` set it (then nothing is read from `~/.deepseek`).
    pub explicit_home: bool,
    /// `config.toml` (the one CodeWhale reads).
    pub config_file: PathBuf,
    /// `mcp.json` (the one CodeWhale reads).
    pub mcp_file: PathBuf,
    /// The global instruction file install writes the rule block into.
    pub instruction_file: PathBuf,
    /// The saved sessions.
    pub sessions: PathBuf,
    /// The native memory's store.
    pub memory_store: PathBuf,
    /// Whether the environment switches the native memory on (`CODEWHALE_MEMORY`,
    /// `DEEPSEEK_MEMORY`).
    pub memory_env_on: bool,
    /// Where `uninstall` exports the memories as Markdown.
    pub export_dir: PathBuf,
}

/// `~/x` expanded against `home`; other paths as they are.
fn expand(value: &str, home: &Path) -> PathBuf {
    match value.strip_prefix("~/").or(value.strip_prefix("~\\")) {
        Some(rest) => rest
            .split(['/', '\\'])
            .filter(|part| !part.is_empty())
            .fold(home.to_path_buf(), |path, part| path.join(part)),
        None if value == "~" => home.to_path_buf(),
        None => PathBuf::from(value),
    }
}

/// Whether an environment value switches the native memory on (`config.rs`: `1`, `on`,
/// `true`, `yes`, `y`, `enabled`).
fn truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "on" | "true" | "yes" | "y" | "enabled"
    )
}

impl CodeWhalePaths {
    /// Resolves the paths from the environment (`env` reads one variable) and the files present.
    pub fn resolve(env: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let set = |name: &str| {
            env(name)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let home = set("HOME")
            .or_else(|| set("USERPROFILE"))
            .map(PathBuf::from)
            .ok_or("cannot find the home directory (HOME is not set)")?;
        let explicit = set("CODEWHALE_HOME")
            .map(|dir| expand(&dir, &home))
            .filter(|dir| dir.is_absolute());
        let mut paths = Self::with(home.clone(), explicit.clone());
        let absolute = |name: &str| {
            set(name)
                .map(|value| expand(&value, &home))
                .filter(|path| path.is_absolute())
        };
        if let Some(config) =
            absolute("CODEWHALE_CONFIG_PATH").or_else(|| absolute("DEEPSEEK_CONFIG_PATH"))
        {
            paths.config_file = config;
        }
        let doc = std::fs::read_to_string(&paths.config_file)
            .ok()
            .and_then(|text| crate::tomledit::parse(&text).ok())
            .unwrap_or(Value::Null);
        let from_config = |key: &str| {
            doc[key]
                .as_str()
                .map(|value| expand(value.trim(), &home))
                .filter(|path| path.is_absolute())
        };
        if let Some(mcp) = absolute("CODEWHALE_MCP_CONFIG")
            .or_else(|| absolute("DEEPSEEK_MCP_CONFIG"))
            .or_else(|| from_config("mcp_config_path"))
        {
            paths.mcp_file = mcp;
        }
        if let Some(memory) = absolute("CODEWHALE_MEMORY_PATH")
            .or_else(|| absolute("DEEPSEEK_MEMORY_PATH"))
            .or_else(|| from_config("memory_path"))
        {
            paths.memory_store = memory_store_of(&memory);
        }
        paths.memory_env_on = set("CODEWHALE_MEMORY")
            .or_else(|| set("DEEPSEEK_MEMORY"))
            .is_some_and(|value| truthy(&value));
        Ok(paths)
    }

    fn with(home: PathBuf, explicit: Option<PathBuf>) -> Self {
        let explicit_home = explicit.is_some();
        let codewhale_home = explicit.unwrap_or_else(|| home.join(".codewhale"));
        let legacy = home.join(".deepseek");
        // What CodeWhale reads: the CodeWhale home's entry, or the legacy one when only that
        // exists (never with an explicit home).
        let state = |name: &str| {
            let primary = codewhale_home.join(name);
            let old = legacy.join(name);
            if explicit_home || primary.exists() || !old.exists() {
                primary
            } else {
                old
            }
        };
        Self {
            config_file: state("config.toml"),
            mcp_file: state("mcp.json"),
            instruction_file: instruction_file(&home),
            sessions: state("sessions"),
            memory_store: memory_store_of(&codewhale_home.join("memory.md")),
            memory_env_on: false,
            export_dir: home.join("graphersal-memory-export").join("codewhale"),
            explicit_home,
            codewhale_home,
            home,
        }
    }

    /// The paths of a home directory without environment overrides (tests).
    #[cfg(test)]
    pub fn of_home(home: &Path) -> Self {
        Self::with(home.to_path_buf(), None)
    }
}

/// The native memory's store of a `memory_path` (`<its directory>/memory/store.sqlite3`).
fn memory_store_of(memory_path: &Path) -> PathBuf {
    memory_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("memory")
        .join("store.sqlite3")
}

/// The global instruction files CodeWhale looks for, in its order (the first that exists is
/// loaded).
pub fn instruction_candidates(home: &Path) -> Vec<PathBuf> {
    let dirs = [".codewhale", ".agents", ".deepseek"];
    ["AGENTS.md", "instructions.md"]
        .iter()
        .flat_map(|name| dirs.iter().map(move |dir| home.join(dir).join(name)))
        .collect()
}

/// The global instruction file CodeWhale loads: the first that exists
/// ([`instruction_candidates`]), else `~/.codewhale/AGENTS.md` (install creates it).
pub fn instruction_file(home: &Path) -> PathBuf {
    instruction_candidates(home)
        .into_iter()
        .find(|path| path.is_file())
        .unwrap_or_else(|| home.join(".codewhale").join("AGENTS.md"))
}

/// The `codewhale` program (or its alias `codew`) on `search_path` (`None`: this process's
/// `PATH`), else in the install locations of its installers (`docs/INSTALL.md`: the curl
/// installer's `~/.local/bin`, cargo's `~/.cargo/bin`, Scoop's shims, the Windows archive's
/// `%USERPROFILE%\bin` and `%LOCALAPPDATA%\Programs\CodeWhale\bin`); those only for this
/// process's own `PATH` (tests pass their search path).
pub fn find_program(home: &Path, search_path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let path = match search_path {
        Some(path) => Some(path.to_os_string()),
        None => std::env::var_os("PATH"),
    };
    let mut dirs: Vec<PathBuf> = path.iter().flat_map(std::env::split_paths).collect();
    if search_path.is_none() {
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join(".cargo").join("bin"));
        if cfg!(windows) {
            dirs.push(home.join("scoop").join("shims"));
            dirs.push(home.join("bin"));
            dirs.push(
                home.join("AppData")
                    .join("Local")
                    .join("Programs")
                    .join("CodeWhale")
                    .join("bin"),
            );
        }
    }
    let names = [
        "codewhale",
        "codewhale.exe",
        "codewhale.cmd",
        "codew",
        "codew.exe",
        "codew.cmd",
    ];
    dirs.iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

/// The version in `codewhale --version`'s output (`codewhale 0.10.2 (1be1a703b975)`, or
/// without the commit): the first word that looks like a version.
pub fn parse_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .map(|word| word.strip_prefix('v').unwrap_or(word))
        .find(|word| word.contains('.') && word.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(str::to_string)
}

/// Asks `program --version` (at most 10 seconds; clap answers before CodeWhale reads any
/// file).
pub fn version_of(program: &Path) -> Option<String> {
    let dir = std::env::temp_dir();
    crate::harness::run(
        &[program.display().to_string(), "--version".to_string()],
        "",
        &dir,
        &[(crate::summaries::HOOK_ENV, "off")],
        std::time::Duration::from_secs(10),
    )
    .ok()
    .and_then(|output| parse_version(&output.stdout))
}

/// Whether `command` runs CodeWhale (`codewhale`, or its alias `codew`).
pub fn is_codewhale(command: &[String]) -> bool {
    command.first().is_some_and(|program| {
        Path::new(program)
            .file_stem()
            .is_some_and(|stem| stem == "codewhale" || stem == "codew")
    })
}

/// The provider a CodeWhale command names (`--provider X` or `--provider=X`).
fn provider_of(command: &[String]) -> Option<String> {
    command
        .windows(2)
        .find(|pair| pair[0] == "--provider")
        .map(|pair| pair[1].clone())
        .or_else(|| {
            command
                .iter()
                .find_map(|arg| arg.strip_prefix("--provider=").map(str::to_string))
        })
}

/// Whether a CodeWhale command runs a model on this machine (costs nothing): its `--provider`
/// is a local one ([`LOCAL_PROVIDERS`]). A local provider chosen only in `config.toml` is not
/// seen, so its runs count the estimate against the budget (name it in the command to count
/// $0).
pub fn local_model(command: &[String]) -> bool {
    is_codewhale(command)
        && provider_of(command)
            .is_some_and(|p| LOCAL_PROVIDERS.contains(&p.trim().to_ascii_lowercase().as_str()))
}

/// The arguments of a headless CodeWhale run: `command`, plus `--json` (one summary object on
/// stdout) and `--prompt-file -` (the prompt on stdin), each unless the command has it. No
/// tool flag is added: a one-shot run has no tools and saves no session.
pub fn exec_args(command: &[String]) -> Vec<String> {
    let mut args = command.to_vec();
    if !args.iter().any(|arg| arg == "--json") {
        args.push("--json".to_string());
    }
    if !args
        .iter()
        .any(|arg| arg == "--prompt-file" || arg.starts_with("--prompt-file="))
    {
        args.extend(["--prompt-file".to_string(), "-".to_string()]);
    }
    args
}

/// `codewhale exec --json`'s summary object (`{mode, provider, model, output, status, error,
/// usage: {input_tokens, output_tokens, ...}}`, `crates/tui/src/lib.rs` `ExecSummary`) turned
/// into the object `claude -p --output-format json` answers (what the summaries read):
/// `result` = `output`, `usage`, `is_error` for a failed status or no output. CodeWhale reports
/// no cost: `total_cost_usd` is 0 for a local model (`local`) and absent otherwise (the caller
/// then counts its estimate).
pub fn normalize_exec_output(stdout: &str, local: bool) -> Value {
    let summary = crate::summaries::json_in(stdout).unwrap_or(Value::Null);
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    let usage = &summary["usage"];
    let mut value = json!({
        "type": "result", "num_turns": 1,
        "usage": {
            "input_tokens": n(&usage["input_tokens"]),
            "output_tokens": n(&usage["output_tokens"]) + n(&usage["reasoning_tokens"]),
        },
    });
    if local {
        value["total_cost_usd"] = json!(0.0);
    }
    let output = summary["output"].as_str().unwrap_or_default();
    let failed = summary["success"] == Value::Bool(false)
        || summary["status"]
            .as_str()
            .is_some_and(|s| matches!(s, "failed" | "error" | "cancelled"));
    if summary.is_null() {
        value["is_error"] = json!(true);
        value["result"] = json!("codewhale answered no JSON");
    } else if output.trim().is_empty() || (failed && output.trim().is_empty()) {
        value["is_error"] = json!(true);
        value["result"] = json!(
            summary["error"]
                .as_str()
                .filter(|e| !e.is_empty())
                .unwrap_or("codewhale answered no output")
        );
    } else {
        value["result"] = json!(output);
        if let Some(object) = crate::summaries::json_in(output).filter(Value::is_object) {
            value["structured_output"] = object;
        }
    }
    value
}

/// Whether CodeWhale's native memory is on: `[memory] backend = "native"` (it decides when
/// set), else `[memory] enabled = true`, else the environment.
pub fn memory_on(doc: &Value, env_on: bool) -> bool {
    match doc["memory"]["backend"].as_str() {
        Some(backend) => backend == "native",
        None => doc["memory"]["enabled"] == Value::Bool(true) || env_on,
    }
}

/// CodeWhale, as the installation sees it.
#[derive(Debug, Clone)]
pub struct CodeWhale {
    paths: CodeWhalePaths,
    servers_key: &'static str,
}

impl CodeWhale {
    /// CodeWhale with its files at `paths` (the servers key from its `mcp.json`).
    pub fn new(paths: CodeWhalePaths) -> Self {
        let servers_key = servers_key_of(&paths.mcp_file);
        Self { paths, servers_key }
    }
}

/// The key of the MCP servers in `mcp.json`: `mcpServers` when the file uses it, else
/// `servers` (what `codewhale mcp init` writes).
pub fn servers_key_of(mcp_file: &Path) -> &'static str {
    let uses_mcp_servers = std::fs::read_to_string(mcp_file)
        .ok()
        .and_then(|text| crate::jsonedit::value_of(&text).ok())
        .is_some_and(|value| value.get("mcpServers").is_some() && value.get("servers").is_none());
    if uses_mcp_servers {
        "mcpServers"
    } else {
        "servers"
    }
}

impl crate::agents::OwnMemory for CodeWhale {
    /// Its memory: on by `[memory] enabled`/`backend` in `config.toml` or the environment.
    fn now(&self, setup: &crate::install::Setup) -> Result<crate::agents::OwnMemoryNow, String> {
        let on = memory_on(
            &crate::install::read_text(&self.paths.config_file)
                .ok()
                .flatten()
                .and_then(|text| crate::tomledit::parse(&text).ok())
                .unwrap_or(serde_json::Value::Null),
            self.paths.memory_env_on,
        );
        let answered = crate::install::State::load_of(setup, Kind::CodeWhale)?
            .meta
            .contains_key("auto_memory_off");
        Ok(crate::agents::OwnMemoryNow {
            on,
            askable: on,
            off_by_user: false,
            answered,
        })
    }

    fn question(
        &self,
        setup: &crate::install::Setup,
        _start_sentence: &str,
    ) -> crate::agents::OwnMemoryQuestion {
        crate::agents::OwnMemoryQuestion {
            title: "CodeWhale's memory".to_string(),
            lines: vec![
                "CodeWhale's own memory is on: it keeps notes in its own store and loads them"
                    .to_string(),
                "into sessions; with graphersal-memory both would keep the same facts twice."
                    .to_string(),
                install::NOTHING_IS_LOST.to_string(),
            ],
            yes: format!(
                "switch it off ([memory] enabled = false in {})",
                setup.show(&self.paths.config_file.display().to_string())
            ),
            no: "keep it on".to_string(),
        }
    }

    fn also(&self, setup: &crate::install::Setup) -> Vec<String> {
        vec![
            format!(
                "The same answer switches CodeWhale's own memory off ([memory] enabled in {}),",
                setup.show(&self.paths.config_file.display().to_string())
            ),
            "after importing its reviewed entries; uninstall switches it on again.".to_string(),
        ]
    }
}

impl Harness for CodeWhale {
    fn kind(&self) -> Kind {
        Kind::CodeWhale
    }

    fn find_program(&self, home: &Path, search: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
        find_program(home, search)
    }

    fn config_roots(&self) -> Vec<PathBuf> {
        vec![self.paths.codewhale_home.clone()]
    }

    fn has_our_parts(&self, setup: &crate::install::Setup) -> bool {
        !install::inspect(setup).nothing_installed()
    }

    fn own_memory(&self) -> Option<&dyn crate::agents::OwnMemory> {
        Some(self)
    }

    /// Its SQLite store, once CodeWhale is on this machine (or was installed into).
    fn native_memory(
        &self,
        setup: &crate::install::Setup,
        project: Option<&Path>,
    ) -> Option<Box<dyn crate::agents::NativeMemory>> {
        let present = self
            .find_program(&self.paths.home, setup.search_path.as_deref())
            .is_some()
            || crate::install::State::exists_of(setup, Kind::CodeWhale);
        if !present {
            return None;
        }
        memory::Native::load(&self.paths.memory_store, &setup.data_dir, project)
            .map(|native| Box::new(native) as Box<dyn crate::agents::NativeMemory>)
    }

    fn develop_on(&self, setup: &crate::install::Setup) -> bool {
        let (state, enabled) = install::inspect(setup).develop_server;
        state != crate::install::PartState::Missing && enabled
    }

    fn other_memory(&self) -> Option<&dyn crate::coexist::OtherMemory> {
        Some(self)
    }

    fn status_lines(&self, setup: &crate::install::Setup) -> Vec<String> {
        install::status_lines(setup)
    }

    fn doctor(
        &self,
        setup: &crate::install::Setup,
        project: Option<&Path>,
    ) -> Vec<crate::install::Finding> {
        install::doctor(setup, project)
    }

    fn removal(
        &self,
        setup: &crate::install::Setup,
        state: &crate::install::State,
        develop_only: bool,
        conflicts: &mut Vec<String>,
    ) -> Result<Vec<(crate::install::Change, &'static str)>, String> {
        install::removal(setup, state, develop_only, conflicts)
    }

    fn own_memory_restored(
        &self,
        _setup: &crate::install::Setup,
        state: &crate::install::State,
    ) -> Option<(PathBuf, String)> {
        let config = &self.paths.config_file;
        state
            .file(config)
            .is_some_and(|entry| entry.contains_key(install::MEMORY_RECORD))
            .then(|| {
                (
                    config.clone(),
                    "CodeWhale's memory: switched on again, as before install".to_string(),
                )
            })
    }

    /// With the records of `[tools] always_load` and the memory switch, which go with the
    /// write of `config.toml`.
    fn plan(
        &self,
        setup: &crate::install::Setup,
        input: &crate::install::PlanInput<'_>,
    ) -> Result<crate::install::HarnessPlan, String> {
        let plan = install::plan(
            setup,
            input.language,
            input.read_only,
            input.develop,
            input.own_memory_off,
        )?;
        let mut out = crate::install::HarnessPlan::of(plan.steps, plan.changes, plan.conflicts);
        out.own_memory_off = Some(input.own_memory_off);
        if plan
            .records
            .iter()
            .any(|(key, value)| *key == install::MEMORY_RECORD && value.is_some())
        {
            out.switched_off = Some(
                "CodeWhale's memory: off (uninstall exports every memory as Markdown and \
                 switches it on again)"
                    .to_string(),
            );
        }
        let config = self.paths.config_file.clone();
        out.records = plan
            .records
            .into_iter()
            .map(|(key, value)| (config.clone(), key, value))
            .collect();
        Ok(out)
    }

    fn mcp_config_file(&self) -> &Path {
        &self.paths.mcp_file
    }

    fn mcp_servers_key(&self) -> &'static str {
        self.servers_key
    }

    /// `{"command", "args", "env", "connect_timeout"}` (`docs/MCP.md`, "Server Fields"); `env`
    /// names the harness (CodeWhale sets no variable of its own that would tell the shim), so
    /// the start context is cut to CodeWhale's limit for server instructions.
    fn mcp_entry(&self, exe: &Path, args: &[String]) -> Value {
        json!({"command": exe.display().to_string(), "args": args,
               "env": {crate::shim::HARNESS_ENV: Kind::CodeWhale.id()},
               "connect_timeout": CONNECT_TIMEOUT_SECS})
    }

    fn remove_server_hint(&self, name: &str) -> String {
        format!(
            "`codewhale mcp remove {name}` (or the member {}.{name} of {})",
            self.servers_key,
            self.paths.mcp_file.display()
        )
    }

    /// `config.toml` (`always_load` and the memory switch are there).
    fn settings_file(&self) -> &Path {
        &self.paths.config_file
    }

    fn instruction_file(&self) -> &Path {
        &self.paths.instruction_file
    }

    fn projects_dir(&self) -> &Path {
        &self.paths.sessions
    }

    fn has_file_memory(&self) -> bool {
        false
    }

    /// `~/graphersal-memory-export/codewhale/<encoded root>/`.
    fn memory_dir(&self, root: &Path) -> PathBuf {
        self.paths
            .export_dir
            .join(crate::import::claude_slug(&root.display().to_string()))
    }

    fn global_memory_file(&self) -> PathBuf {
        self.paths.export_dir.join("global.md")
    }

    /// `memory.enabled` in `config.toml`.
    fn auto_memory_key(&self) -> Option<&'static str> {
        Some("memory.enabled")
    }

    fn headless_command(&self) -> Option<&'static str> {
        Some(DEFAULT_HEADLESS_COMMAND)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, String)]) -> impl Fn(&str) -> Option<String> + use<> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        }
    }

    #[test]
    fn paths_follow_codewhale_and_its_legacy_directory() {
        let tmp = crate::mcp_tests::TempDir::new("cw-paths");
        let home = tmp.path().to_path_buf();
        let env = env_of(&[("HOME", home.display().to_string())]);
        let paths = CodeWhalePaths::resolve(&env).unwrap();
        assert_eq!(paths.codewhale_home, home.join(".codewhale"));
        assert_eq!(paths.mcp_file, home.join(".codewhale").join("mcp.json"));
        assert_eq!(
            paths.config_file,
            home.join(".codewhale").join("config.toml")
        );
        assert_eq!(
            paths.instruction_file,
            home.join(".codewhale").join("AGENTS.md")
        );
        assert_eq!(
            paths.memory_store,
            home.join(".codewhale").join("memory").join("store.sqlite3")
        );
        assert_eq!(
            paths.export_dir,
            home.join("graphersal-memory-export").join("codewhale")
        );
        // Only the legacy files exist: CodeWhale reads them, so install writes them.
        let legacy = home.join(".deepseek");
        std::fs::create_dir_all(legacy.join("sessions")).unwrap();
        std::fs::write(legacy.join("mcp.json"), "{}").unwrap();
        std::fs::write(legacy.join("config.toml"), "").unwrap();
        std::fs::write(legacy.join("AGENTS.md"), "x").unwrap();
        let paths = CodeWhalePaths::resolve(&env).unwrap();
        assert_eq!(paths.mcp_file, legacy.join("mcp.json"));
        assert_eq!(paths.config_file, legacy.join("config.toml"));
        assert_eq!(paths.sessions, legacy.join("sessions"));
        assert_eq!(paths.instruction_file, legacy.join("AGENTS.md"));
        // ~/.agents/AGENTS.md comes before the legacy one.
        std::fs::create_dir_all(home.join(".agents")).unwrap();
        std::fs::write(home.join(".agents").join("AGENTS.md"), "y").unwrap();
        assert_eq!(
            CodeWhalePaths::resolve(&env).unwrap().instruction_file,
            home.join(".agents").join("AGENTS.md")
        );
        // CODEWHALE_HOME reads nothing from ~/.deepseek, but the global instructions stay
        // under the home directory.
        let own = home.join("cw");
        let env = env_of(&[
            ("HOME", home.display().to_string()),
            ("CODEWHALE_HOME", own.display().to_string()),
        ]);
        let paths = CodeWhalePaths::resolve(&env).unwrap();
        assert!(paths.explicit_home);
        assert_eq!(paths.mcp_file, own.join("mcp.json"));
        assert_eq!(paths.sessions, own.join("sessions"));
        assert_eq!(
            paths.instruction_file,
            home.join(".agents").join("AGENTS.md")
        );
    }

    #[test]
    fn overrides_move_the_files() {
        let tmp = crate::mcp_tests::TempDir::new("cw-overrides");
        let home = tmp.path().to_path_buf();
        let config = home.join("conf").join("cw.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            "mcp_config_path = \"~/m/mcp.json\"\nmemory_path = \"~/mem/memory.md\"\n",
        )
        .unwrap();
        let env = env_of(&[
            ("HOME", home.display().to_string()),
            ("CODEWHALE_CONFIG_PATH", config.display().to_string()),
            ("DEEPSEEK_MEMORY", "on".to_string()),
        ]);
        let paths = CodeWhalePaths::resolve(&env).unwrap();
        assert_eq!(paths.config_file, config);
        assert_eq!(paths.mcp_file, home.join("m").join("mcp.json"));
        assert_eq!(
            paths.memory_store,
            home.join("mem").join("memory").join("store.sqlite3")
        );
        assert!(paths.memory_env_on);
        let env = env_of(&[
            ("HOME", home.display().to_string()),
            ("CODEWHALE_CONFIG_PATH", config.display().to_string()),
            (
                "CODEWHALE_MCP_CONFIG",
                home.join("x").join("mcp.json").display().to_string(),
            ),
            ("CODEWHALE_MEMORY", "off".to_string()),
        ]);
        let paths = CodeWhalePaths::resolve(&env).unwrap();
        assert_eq!(paths.mcp_file, home.join("x").join("mcp.json"));
        assert!(!paths.memory_env_on);
        assert!(CodeWhalePaths::resolve(&|_| None).is_err());
    }

    #[test]
    fn tool_names_keep_the_server_name() {
        let names = tool_names();
        assert!(names.contains(&"mcp_graphersal-memory_remember".to_string()));
        assert_eq!(names.len(), crate::catalog::FUNCTIONS.len());
        for name in &names {
            assert!(
                crate::transcript::memory_tool(name).is_some()
                    || !["remember", "forget", "pin", "settle"]
                        .iter()
                        .any(|t| name.ends_with(t)),
                "{name}"
            );
        }
    }

    #[test]
    fn the_servers_key_follows_the_file() {
        let tmp = crate::mcp_tests::TempDir::new("cw-key");
        let file = tmp.path().join("mcp.json");
        assert_eq!(servers_key_of(&file), "servers");
        std::fs::write(&file, r#"{"mcpServers": {}}"#).unwrap();
        assert_eq!(servers_key_of(&file), "mcpServers");
        std::fs::write(&file, r#"{"servers": {"a": {}}}"#).unwrap();
        assert_eq!(servers_key_of(&file), "servers");
    }

    #[test]
    fn programs_and_versions_are_found() {
        let tmp = crate::mcp_tests::TempDir::new("cw-program");
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        assert_eq!(find_program(tmp.path(), Some(bin.as_os_str())), None);
        std::fs::write(bin.join("codew"), "").unwrap();
        assert_eq!(
            find_program(tmp.path(), Some(bin.as_os_str())),
            Some(bin.join("codew"))
        );
        assert_eq!(
            parse_version("codewhale 0.10.2 (1be1a703b975)\n").as_deref(),
            Some("0.10.2")
        );
        assert_eq!(parse_version("codewhale 0.10.2").as_deref(), Some("0.10.2"));
        assert_eq!(parse_version("codewhale"), None);
        let words = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        assert!(is_codewhale(&words("codewhale exec")));
        assert!(is_codewhale(&words("/opt/bin/codew exec")));
        assert!(!is_codewhale(&words("codex exec")));
    }

    #[test]
    fn headless_runs_answer_like_claude() {
        let words = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        assert_eq!(
            exec_args(&words("codewhale exec --provider ollama")),
            words("codewhale exec --provider ollama --json --prompt-file -")
        );
        assert_eq!(
            exec_args(&words("codewhale exec --json --prompt-file x")),
            words("codewhale exec --json --prompt-file x")
        );
        assert!(local_model(&words("codewhale exec --provider ollama")));
        assert!(local_model(&words("codewhale exec --provider=vllm")));
        assert!(!local_model(&words("codewhale exec")));
        assert!(!local_model(&words("codewhale exec --provider deepseek")));
        assert!(!local_model(&words("codex exec --provider ollama")));

        let stdout = r#"{"mode":"one-shot","provider":"ollama","model":"m","prompt":"p",
            "output":"```json\n{\"headline\":\"h\",\"summary\":\"s\"}\n```","tools":[],
            "status":"completed","usage":{"input_tokens":100,"output_tokens":20,"reasoning_tokens":5}}"#;
        let value = normalize_exec_output(stdout, true);
        assert_eq!(value["usage"]["input_tokens"], 100);
        assert_eq!(value["usage"]["output_tokens"], 25);
        assert_eq!(value["total_cost_usd"], 0.0);
        assert_eq!(value["structured_output"]["headline"], "h");
        let summary = crate::summaries::parse_answer(&value.to_string()).unwrap();
        assert_eq!(summary.cost_usd, Some(0.0));
        let paid = normalize_exec_output(stdout, false);
        assert!(paid.get("total_cost_usd").is_none(), "no cost reported");
        let failed = normalize_exec_output(
            r#"{"output":"","status":"failed","error":"no API key"}"#,
            false,
        );
        assert_eq!(failed["is_error"], true);
        assert_eq!(failed["result"], "no API key");
        assert_eq!(normalize_exec_output("oops", false)["is_error"], true);
    }

    #[test]
    fn the_memory_switch_reads_backend_first() {
        let doc = |s: &str| crate::tomledit::parse(s).unwrap();
        assert!(!memory_on(&doc(""), false));
        assert!(memory_on(&doc(""), true));
        assert!(memory_on(&doc("[memory]\nenabled = true\n"), false));
        assert!(!memory_on(
            &doc("[memory]\nenabled = true\nbackend = \"off\"\n"),
            true
        ));
        assert!(memory_on(&doc("[memory]\nbackend = \"native\"\n"), false));
    }

    #[test]
    fn the_agent_names_its_files() {
        let agent = CodeWhale::new(CodeWhalePaths::of_home(Path::new("/h")));
        assert_eq!(agent.id(), "codewhale");
        assert_eq!(agent.display_name(), "CodeWhale");
        assert_eq!(agent.mcp_servers_key(), "servers");
        let entry = agent.mcp_entry(Path::new("/x/graphersal-memory"), &["mcp".into()]);
        assert!(crate::agents::is_our_entry(&entry));
        assert_eq!(
            entry["env"],
            json!({"GRAPHERSAL_MEMORY_HARNESS": "codewhale"})
        );
        assert_eq!(
            crate::shim::harness_name(&|name| entry["env"][name].as_str().map(str::to_string)),
            "codewhale"
        );
        assert!(!agent.has_file_memory());
        assert_eq!(agent.headless_command(), Some("codewhale exec"));
    }
}
