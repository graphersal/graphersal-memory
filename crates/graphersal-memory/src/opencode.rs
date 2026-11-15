//! opencode, both lines: 1.x (verified against v1.19.5) and 2.x (verified against v2.2.4,
//! github.com/anomalyco/opencode): where it keeps its files, what graphersal-memory writes there,
//! and how its sessions are read. [`Major`] says which line is installed ([`detect`]); the
//! differences are kept here and in `opencode/install.rs`, behind one [`OpenCode`] agent.
//!
//! - **Configuration**: the global directory is `$XDG_CONFIG_HOME/opencode`, else
//!   `~/.config/opencode` (1.x `packages/core/src/global.ts`; 2.x
//!   `packages/util/src/global-roots.ts`, where `OPENCODE_CONFIG_DIR` replaces it). 1.x merges
//!   `config.json`, `opencode.json` and `opencode.jsonc`; 2.x reads only `opencode.json` and
//!   `opencode.jsonc` (`packages/core/src/config/discovery.ts`, `names`), so a `config.json`
//!   install wrote under 1.x is no longer read after an upgrade. All are JSONC. install edits
//!   the first that exists of the line's files ([`config_files`]), else creates
//!   `opencode.json`, as text (`crate::jsonedit`).
//! - **MCP servers**: 1.x: members of `mcp`, a local stdio server being `{"type": "local",
//!   "command": [program, args...], "enabled": true}` (`packages/core/src/v1/config/mcp.ts`).
//!   2.x: members of `mcp.servers` with `"disabled"` instead of `"enabled"`
//!   (`packages/schema/src/mcp.ts`, `LocalConfig`); it still reads the 1.x shape as a legacy
//!   entry (`config/normalize.ts`, `normalizeMcp`), which install migrates. 2.x exposes MCP
//!   tools through "Code Mode" (callable only as JavaScript inside its `execute` tool) unless a
//!   server sets `"codemode": false`; our entries set it, so the tools stay direct calls named
//!   `graphersal-memory_<function>` (`packages/core/src/tool/mcp.ts`, `name`), as in 1.x and
//!   as the rule block says. The develop server is registered switched off (both lines have a
//!   switched-off state).
//! - **Rules**: 1.x loads the first of `~/.config/opencode/AGENTS.md` and `~/.claude/CLAUDE.md`
//!   that exists (`session/instruction.ts`), so creating `AGENTS.md` would silently stop it
//!   from loading a Claude Code user's `CLAUDE.md`; 2.x loads `AGENTS.md` only (no `CLAUDE.md`
//!   fallback, `config/plugin/instruction.ts`). Both load every file listed in
//!   `instructions`. The managed rule block therefore goes into a file of our own,
//!   [`RULES_FILE`] in the configuration directory, listed in `instructions` (user decision,
//!   kept for 2.x). The user's `AGENTS.md` is never touched.
//! - **Session start and end**: a plugin file of our own, [`PLUGIN_FILE`] in `plugins/` (both
//!   lines auto-load `{plugin,plugins}/*.{ts,js}` of the configuration directory: 1.x
//!   `config/plugin.ts`, 2.x `packages/core/src/plugin/source-directory.ts`). The plugin APIs
//!   differ: 1.x calls a function export (or a default export's `server`), 2.x a default
//!   export's `setup` (`packages/core/src/plugin/module.ts`). One file serves both: its default
//!   export carries `id`, `server` (1.x) and `setup` (2.x), so it keeps working across an
//!   upgrade. It runs `graphersal-memory hook session-start|session-end --harness opencode`
//!   detached when a session starts (1.x `session.created`; 2.x a session's first
//!   `session.execution.started`, as its plugins do not see the creation of a session made by
//!   `opencode run`) and when opencode shuts down (1.x `dispose`, 2.x the cleanup `setup`
//!   returns; neither has a session-end event). 1.x shows the start hook's line as a toast
//!   (`client.tui.showToast`); 2.x has no toast for server plugins, so the line is dropped there.
//! - **Sessions**: one SQLite database, `$XDG_DATA_HOME/opencode/opencode.db` (default
//!   `~/.local/share/opencode`; `OPENCODE_DB` when set), read by [`sessions`]: 1.x tables
//!   `session`/`message`/`part`, 2.x tables `session_v2`/`session_message`/`session_part`. 2.x
//!   upgrades the same file in place and copies the 1.x sessions with their ids, keeping the
//!   1.x rows; the reader reads both and takes each session once.
//! - **Headless runs**: `opencode run --format json --model <provider/model>` on both lines,
//!   with the same event lines (`text`, `step_finish` with `cost` and `tokens`, `error`). 2.x's
//!   `run` connects to a shared background server by default; [`headless_args`] adds
//!   `--standalone` so the run gets its own server with the sandbox's environment.
//! - **Memory**: neither line has a memory feature of its own (nothing to switch off) or a file
//!   memory: the export of `uninstall` writes readable Markdown files into
//!   `~/graphersal-memory-export/opencode/` instead ([`OpenCode::memory_dir`]).

pub mod install;
pub mod sessions;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::agents::{Harness, Kind};

/// The managed rules file in opencode's configuration directory.
pub const RULES_FILE: &str = "graphersal-memory.md";

/// The plugin file in opencode's `plugins/` directory.
pub const PLUGIN_FILE: &str = "graphersal-memory.js";

/// The configuration files opencode 1.x reads from its global directory, in the order install
/// prefers them for its edit (the one opencode merges last first).
pub const CONFIG_FILES: [&str; 3] = ["opencode.jsonc", "opencode.json", "config.json"];

/// The configuration files opencode 2.x reads (it no longer reads `config.json`).
pub const CONFIG_FILES_V2: [&str; 2] = ["opencode.jsonc", "opencode.json"];

/// The configuration's JSON schema (what a file install creates names; the same URL in 2.x).
pub const SCHEMA: &str = "https://opencode.ai/config.json";

/// The default headless command of opencode's summaries and bench: a model of opencode Zen
/// that is free (for a limited time, opencode docs "Zen"; still the first free model in 2.x,
/// `packages/core/src/model.ts`, `PREFERRED_FREE`) and needs no login.
pub const DEFAULT_HEADLESS_COMMAND: &str = "opencode run --model opencode/big-pickle";

/// The line of opencode (major version).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Major {
    /// opencode 1.x.
    V1,
    /// opencode 2.x, the current release line (the default when nothing says otherwise).
    #[default]
    V2,
}

impl Major {
    /// `1.x` or `2.x`.
    pub fn label(self) -> &'static str {
        match self {
            Major::V1 => "1.x",
            Major::V2 => "2.x",
        }
    }
}

/// The configuration files `major` reads, in install's order of preference.
pub fn config_files(major: Major) -> &'static [&'static str] {
    match major {
        Major::V1 => &CONFIG_FILES,
        Major::V2 => &CONFIG_FILES_V2,
    }
}

/// Which opencode line is installed, and how that was found out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    /// The line.
    pub major: Major,
    /// The version `opencode --version` printed (`None` when the binary was not asked).
    pub version: Option<String>,
    /// How it was found out (for `status` and `doctor`).
    pub source: String,
}

impl Detected {
    /// A line given without detection (tests, and callers that only need the database).
    pub fn given(major: Major) -> Self {
        Self {
            major,
            version: None,
            source: "given".to_string(),
        }
    }

    /// `opencode 2.x (2.2.4, from `opencode --version`)`.
    pub fn describe(&self) -> String {
        match &self.version {
            Some(version) => format!(
                "opencode {} ({version}, {})",
                self.major.label(),
                self.source
            ),
            None => format!("opencode {} ({})", self.major.label(), self.source),
        }
    }
}

/// The version in `opencode --version`'s output (`1.18.35`, `opencode v2.2.4`): the last word,
/// without a leading `v`, with its major version.
pub fn parse_version(output: &str) -> Option<(Major, String)> {
    let word = output.split_whitespace().last()?;
    let version = word.strip_prefix('v').unwrap_or(word);
    let major: u64 = version.split('.').next()?.parse().ok()?;
    if !version.contains('.') {
        return None;
    }
    let line = match major {
        0 | 1 => Major::V1,
        _ => Major::V2,
    };
    Some((line, version.to_string()))
}

/// Asks `program --version` (at most 10 seconds; once per program and process). opencode
/// creates its directories even for `--version`, so it runs with a home and XDG directories of
/// its own in a temporary directory, removed afterwards: asking leaves nothing behind.
pub fn version_of(program: &Path) -> Option<(Major, String)> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    type Answers = HashMap<PathBuf, Option<(Major, String)>>;
    static CACHE: OnceLock<Mutex<Answers>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Ok(known) = cache.lock()
        && let Some(found) = known.get(program)
    {
        return found.clone();
    }
    static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let run = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scratch = std::env::temp_dir().join(format!(
        "graphersal-memory-opencode-version-{}-{run}",
        std::process::id()
    ));
    let found = std::fs::create_dir_all(&scratch).ok().and_then(|()| {
        let dir = scratch.display().to_string();
        let env: Vec<(&str, &str)> = [
            "HOME",
            "USERPROFILE",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
        ]
        .into_iter()
        .map(|name| (name, dir.as_str()))
        .chain([
            (crate::summaries::HOOK_ENV, "off"),
            ("OPENCODE_DISABLE_AUTOUPDATE", "1"),
        ])
        .collect();
        crate::harness::run(
            &[program.display().to_string(), "--version".to_string()],
            "",
            &scratch,
            &env,
            std::time::Duration::from_secs(10),
        )
        .ok()
        .and_then(|output| parse_version(&output.stdout))
    });
    let _ = std::fs::remove_dir_all(&scratch);
    if let Ok(mut known) = cache.lock() {
        known.insert(program.to_path_buf(), found.clone());
    }
    found
}

/// The `opencode` program on `search_path` (`None`: this process's `PATH`), else the one its
/// install script puts into `~/.opencode/bin`.
pub fn find_program(home: &Path, search_path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let path = match search_path {
        Some(path) => Some(path.to_os_string()),
        None => std::env::var_os("PATH"),
    };
    let names = ["opencode", "opencode.exe", "opencode.cmd"];
    path.iter()
        .flat_map(std::env::split_paths)
        .chain(std::iter::once(home.join(".opencode").join("bin")))
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

/// What opencode's own data says about its line: a database with 2.x's `session_v2` table
/// means 2.x ran (it upgrades the file in place); one with only 1.x's `session` table means
/// 1.x.
pub fn major_from_data(db: &Path) -> Option<Major> {
    let tables = sessions::tables(db).ok()?;
    if tables.iter().any(|t| t == "session_v2") {
        Some(Major::V2)
    } else if tables.iter().any(|t| t == "session") {
        Some(Major::V1)
    } else {
        None
    }
}

/// Which opencode line is installed: `opencode --version` when `probe` and the binary is found,
/// else opencode's database ([`major_from_data`]), else 2.x (the current release).
pub fn detect(
    env: &dyn Fn(&str) -> Option<String>,
    search_path: Option<&std::ffi::OsStr>,
    probe: bool,
) -> Result<Detected, String> {
    let paths = OpenCodePaths::resolve_for(env, Detected::given(Major::V2))?;
    if probe
        && let Some(program) = find_program(&paths.home, search_path)
        && let Some((major, version)) = version_of(&program)
    {
        return Ok(Detected {
            major,
            version: Some(version),
            source: format!("from `{} --version`", program.display()),
        });
    }
    if let Some(major) = major_from_data(&paths.db) {
        return Ok(Detected {
            major,
            version: None,
            source: format!("from its data in {}", paths.db.display()),
        });
    }
    Ok(Detected {
        major: Major::V2,
        version: None,
        source: "opencode not found; the current release line assumed".to_string(),
    })
}

/// Where opencode keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodePaths {
    /// Which line these paths are for.
    pub detected: Detected,
    /// The home directory.
    pub home: PathBuf,
    /// The global configuration directory.
    pub config_dir: PathBuf,
    /// The configuration file install edits (it may not exist yet).
    pub config_file: PathBuf,
    /// Configuration files of the directory this line does not read (2.x: `config.json`),
    /// where an install under an earlier line may have left our parts.
    pub stale_config_files: Vec<PathBuf>,
    /// The managed rules file.
    pub rules_file: PathBuf,
    /// The plugin file.
    pub plugin_file: PathBuf,
    /// The session database.
    pub db: PathBuf,
    /// Where `uninstall` exports the memories as Markdown.
    pub export_dir: PathBuf,
}

impl OpenCodePaths {
    /// Resolves the paths from the environment (`env` reads one variable) and detects the line
    /// (`opencode --version` on `PATH`, else its data): `XDG_CONFIG_HOME`, `XDG_DATA_HOME`,
    /// `OPENCODE_DB`, 2.x `OPENCODE_CONFIG_DIR`, home [`crate::store::home_dir`].
    pub fn resolve(env: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        Self::resolve_for(env, detect(env, None, true)?)
    }

    /// The paths of the line `detected`.
    pub fn resolve_for(
        env: &dyn Fn(&str) -> Option<String>,
        detected: Detected,
    ) -> Result<Self, String> {
        let set = |name: &str| env(name).filter(|value| !value.is_empty());
        let home = crate::store::home_dir(env)?;
        let xdg_config = set("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("opencode");
        let config_dir = match (detected.major, set("OPENCODE_CONFIG_DIR")) {
            (Major::V2, Some(dir)) if Path::new(&dir).is_absolute() => PathBuf::from(dir),
            _ => xdg_config,
        };
        let data_dir = set("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local").join("share"))
            .join("opencode");
        let db = match set("OPENCODE_DB") {
            Some(db) if Path::new(&db).is_absolute() => PathBuf::from(db),
            Some(db) if db != ":memory:" => data_dir.join(db),
            _ => data_dir.join("opencode.db"),
        };
        let files = config_files(detected.major);
        let config_file = files
            .iter()
            .map(|name| config_dir.join(name))
            .find(|path| path.is_file())
            .unwrap_or_else(|| config_dir.join("opencode.json"));
        let stale_config_files = CONFIG_FILES
            .iter()
            .filter(|name| !files.contains(name))
            .map(|name| config_dir.join(name))
            .collect();
        Ok(Self {
            rules_file: config_dir.join(RULES_FILE),
            plugin_file: config_dir.join("plugins").join(PLUGIN_FILE),
            export_dir: home.join("graphersal-memory-export").join("opencode"),
            detected,
            config_file,
            stale_config_files,
            config_dir,
            db,
            home,
        })
    }

    /// The paths of a home directory for opencode 1.x, without environment overrides (tests).
    #[cfg(test)]
    pub fn of_home(home: &Path) -> Self {
        Self::of_home_for(home, Major::V1)
    }

    /// The paths of a home directory for `major`, without environment overrides (tests).
    #[cfg(test)]
    pub fn of_home_for(home: &Path, major: Major) -> Self {
        let text = home.display().to_string();
        // `resolve_for` fails only without a home directory, which is given here.
        Self::resolve_for(
            &|name| (name == "HOME").then(|| text.clone()),
            Detected::given(major),
        )
        .unwrap_or_else(|err| panic!("{err}"))
    }

    /// The line these paths are for.
    pub fn major(&self) -> Major {
        self.detected.major
    }

    /// Where the MCP servers are in the configuration (1.x `mcp`, 2.x `mcp.servers`).
    pub fn servers_path(&self) -> &'static [&'static str] {
        match self.major() {
            Major::V1 => &["mcp"],
            Major::V2 => &["mcp", "servers"],
        }
    }
}

/// opencode, as the installation sees it.
#[derive(Debug, Clone)]
pub struct OpenCode {
    paths: OpenCodePaths,
}

impl OpenCode {
    /// opencode with its files at `paths`.
    pub fn new(paths: OpenCodePaths) -> Self {
        Self { paths }
    }
}

impl Harness for OpenCode {
    fn kind(&self) -> Kind {
        Kind::OpenCode
    }

    fn find_program(&self, home: &Path, search: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
        find_program(home, search)
    }

    fn config_roots(&self) -> Vec<PathBuf> {
        vec![self.paths.config_dir.clone()]
    }

    fn has_our_parts(&self, setup: &crate::install::Setup) -> bool {
        !install::inspect(setup).nothing_installed()
    }

    fn mcp_config_file(&self) -> &Path {
        &self.paths.config_file
    }

    fn mcp_servers_key(&self) -> &'static str {
        "mcp"
    }

    /// 1.x: `{"type": "local", "command": [exe, args...], "enabled": true}` (opencode docs "MCP
    /// servers", `packages/core/src/v1/config/mcp.ts`); 2.x: `{"type": "local", "command",
    /// "disabled": false, "codemode": false}` (`packages/schema/src/mcp.ts`; Code Mode off, so
    /// the tools stay direct calls).
    fn mcp_entry(&self, exe: &Path, args: &[String]) -> Value {
        let mut command = vec![exe.display().to_string()];
        command.extend(args.iter().cloned());
        match self.paths.major() {
            Major::V1 => json!({"type": "local", "command": command, "enabled": true}),
            Major::V2 => {
                json!({"type": "local", "command": command, "disabled": false, "codemode": false})
            }
        }
    }

    fn remove_server_hint(&self, name: &str) -> String {
        format!(
            "the member {}.{name} of {}",
            self.paths.servers_path().join("."),
            self.paths.config_file.display()
        )
    }

    /// The configuration file (opencode keeps its settings and MCP servers in one file).
    fn settings_file(&self) -> &Path {
        &self.paths.config_file
    }

    fn instruction_file(&self) -> &Path {
        &self.paths.rules_file
    }

    fn projects_dir(&self) -> &Path {
        &self.paths.db
    }

    fn has_file_memory(&self) -> bool {
        false
    }

    /// `~/graphersal-memory-export/opencode/<encoded root>/` (MEMORY.md and one file per
    /// memory, readable Markdown; opencode loads none of it by itself).
    fn memory_dir(&self, root: &Path) -> PathBuf {
        self.paths
            .export_dir
            .join(crate::import::claude_slug(&root.display().to_string()))
    }

    fn global_memory_file(&self) -> PathBuf {
        self.paths.export_dir.join("global.md")
    }

    /// opencode has no memory feature of its own.
    fn auto_memory_key(&self) -> Option<&'static str> {
        None
    }

    fn headless_command(&self) -> Option<&'static str> {
        Some(DEFAULT_HEADLESS_COMMAND)
    }
}

/// Whether `command` runs opencode.
pub fn is_opencode(command: &[String]) -> bool {
    command.first().is_some_and(|program| {
        Path::new(program)
            .file_stem()
            .is_some_and(|stem| stem == "opencode")
    })
}

/// The model of an opencode command (`--model`/`-m provider/model`), when it names one.
pub fn model_of(command: &[String]) -> Option<&str> {
    command
        .windows(2)
        .find(|pair| pair[0] == "--model" || pair[0] == "-m")
        .map(|pair| pair[1].as_str())
        .or_else(|| command.iter().find_map(|arg| arg.strip_prefix("--model=")))
}

/// Whether an opencode command uses a free model of opencode Zen (`opencode/big-pickle` and
/// the `opencode/*-free` models, opencode docs "Zen", "free for a limited time"): opencode
/// prices them at 0, so the summaries and the bench cost nothing.
pub fn free_model(command: &[String]) -> bool {
    is_opencode(command)
        && model_of(command).is_some_and(|model| {
            model
                .strip_prefix("opencode/")
                .is_some_and(|id| id == "big-pickle" || id.ends_with("-free"))
        })
}

/// The arguments of a headless opencode run: `command` itself, plus `--standalone` after `run`
/// for opencode 2.x (asked with `--version`, [`version_of`]). Without it 2.x's `run` connects
/// to the shared background server (`packages/cli/src/services/server-connection.ts`), which
/// has the user's configuration and session database; `--standalone` starts a server of the
/// run's own that inherits the sandbox's environment (`services/standalone.ts`). 1.x has no
/// such flag (and refuses unknown ones). A program that does not answer `--version` is taken
/// for 1.x.
pub fn headless_args(command: &[String]) -> Vec<String> {
    let mut args = command.to_vec();
    if !is_opencode(command) || command.iter().any(|arg| arg == "--standalone") {
        return args;
    }
    let Some(run) = command.iter().position(|arg| arg == "run") else {
        return args;
    };
    let program = crate::harness::program_path(&command[0]);
    if matches!(version_of(&program), Some((Major::V2, _))) {
        args.insert(run + 1, "--standalone".to_string());
    }
    args
}

/// The environment of a headless opencode run in a sandbox directory: its own configuration,
/// data and state directories (so the user's global configuration, plugins, rules, session
/// database and 2.x background server registration are neither loaded nor written), no
/// project configuration, nothing read from Claude Code's files, no update check, and
/// graphersal-memory's hooks off. The same variables exist in 1.x and 2.x.
pub fn sandbox_env(config_home: &Path, data_home: &Path) -> Vec<(String, String)> {
    let state_home = data_home.with_file_name("state");
    vec![
        ("XDG_CONFIG_HOME".into(), config_home.display().to_string()),
        ("XDG_DATA_HOME".into(), data_home.display().to_string()),
        ("XDG_STATE_HOME".into(), state_home.display().to_string()),
        ("OPENCODE_DISABLE_PROJECT_CONFIG".into(), "1".into()),
        ("OPENCODE_DISABLE_CLAUDE_CODE".into(), "1".into()),
        ("OPENCODE_DISABLE_AUTOUPDATE".into(), "1".into()),
        (crate::summaries::HOOK_ENV.into(), "off".into()),
    ]
}

/// Builds a sandbox for headless opencode runs in `dir`: `config/opencode/opencode.json` with
/// `config` (the arm's MCP servers, permissions and instructions, in the 1.x shape, which 2.x
/// reads as legacy configuration), and `data/opencode/` with a copy of the user's `auth.json`
/// (their provider logins; read, never changed) when there is one. 2.x imports `auth.json`
/// into its database when it creates one (migration `import_legacy_credentials`), so the
/// sandbox gets the logins kept there; logins made in 2.x itself live only in its database (it
/// no longer writes `auth.json`) and are not copied. Returns the environment of the runs.
pub fn build_sandbox(
    dir: &Path,
    config: &Value,
    user_db: Option<&Path>,
) -> Result<Vec<(String, String)>, String> {
    let config_home = dir.join("config");
    let data_home = dir.join("data");
    let config_dir = config_home.join("opencode");
    let data_dir = data_home.join("opencode");
    for d in [&config_dir, &data_dir] {
        std::fs::create_dir_all(d)
            .map_err(|err| format!("cannot create {}: {err}", d.display()))?;
    }
    let mut config = config.clone();
    config["$schema"] = json!(SCHEMA);
    crate::fsutil::write_atomic(
        &config_dir.join("opencode.json"),
        format!("{}\n", crate::jsonedit::render(&config, Some("  "), 0)).as_bytes(),
    )?;
    if let Some(auth) = user_db.and_then(Path::parent).map(|d| d.join("auth.json"))
        && auth.is_file()
    {
        let target = data_dir.join("auth.json");
        std::fs::copy(&auth, &target)
            .map_err(|err| format!("cannot copy {}: {err}", auth.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600));
        }
    }
    Ok(sandbox_env(&config_home, &data_home))
}

/// The permissions of a sandboxed run: everything denied, then `allow` (tool names or
/// patterns) allowed (opencode docs "Permissions": the last matching rule wins).
pub fn permissions(allow: &[&str]) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("*".into(), json!("deny"));
    for tool in allow {
        map.insert((*tool).to_string(), json!("allow"));
    }
    Value::Object(map)
}

/// `opencode run --format json`'s event lines turned into the object `claude -p
/// --output-format json` answers (what the summaries and the bench read): `result` = the text
/// parts, `total_cost_usd` and `usage` = the sums of the `step_finish` parts, `num_turns` =
/// the steps, `is_error` for an `error` event or no text, `structured_output` = the JSON
/// object in the text when there is one.
pub fn normalize_run_output(stdout: &str) -> Value {
    let mut text = Vec::new();
    let (mut cost, mut input, mut output, mut steps) = (0.0, 0u64, 0u64, 0u64);
    let mut error: Option<String> = None;
    let mut events = 0;
    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        events += 1;
        let part = &event["part"];
        match event["type"].as_str().unwrap_or_default() {
            "text" => {
                if let Some(t) = part["text"].as_str()
                    && part["synthetic"] != true
                {
                    text.push(t.to_string());
                }
            }
            "step_finish" => {
                steps += 1;
                cost += part["cost"].as_f64().unwrap_or(0.0);
                let tokens = &part["tokens"];
                let n = |v: &Value| v.as_u64().unwrap_or(0);
                input += n(&tokens["input"])
                    + n(&tokens["cache"]["read"])
                    + n(&tokens["cache"]["write"]);
                output += n(&tokens["output"]) + n(&tokens["reasoning"]);
            }
            "error" => {
                let e = &event["error"];
                error = Some(
                    e["data"]["message"]
                        .as_str()
                        .or_else(|| e["message"].as_str())
                        .or_else(|| e["name"].as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| e.to_string()),
                );
            }
            _ => {}
        }
    }
    let result = text.join("\n\n");
    let mut value = json!({
        "type": "result", "total_cost_usd": cost, "num_turns": steps,
        "usage": {"input_tokens": input, "output_tokens": output},
    });
    if let Some(error) = error {
        value["is_error"] = json!(true);
        value["result"] = json!(error);
    } else if result.trim().is_empty() {
        value["is_error"] = json!(true);
        value["result"] = json!(if events == 0 {
            "opencode answered no JSON events".to_string()
        } else {
            "opencode answered no text".to_string()
        });
    } else {
        value["is_error"] = json!(false);
        if let Some(object) = crate::bench::json_in(&result) {
            value["structured_output"] = object;
        }
        value["result"] = json!(result);
    }
    value
}

/// The plugin file's text: for `exe` with `dir_args` (the data and run directory flags, when
/// they were chosen explicitly). One default export serves both lines: `server` is 1.x's
/// plugin function, `setup` 2.x's (the other line ignores the member it does not know). It only
/// starts our hook and never waits for it, so a session is never blocked;
/// `GRAPHERSAL_MEMORY_HOOK=off` (the summaries' and the bench's own runs) makes it do nothing.
pub fn plugin_text(exe: &Path, dir_args: &[String]) -> String {
    let mut command = vec![exe.display().to_string()];
    command.extend(dir_args.iter().cloned());
    let command = Value::from(command).to_string();
    format!(
        "// {MARKER}\n\
// Written by `graphersal-memory install`; `graphersal-memory uninstall` removes it. Edits are\n\
// kept: install then reports a conflict instead of overwriting the file.\n\
// opencode loads every file of this directory at startup (1.x calls `server`, 2.x `setup`).\n\
// When a session starts it runs `graphersal-memory hook session-start --harness opencode`\n\
// (queues the indexing of past sessions; 1.x shows its line as a toast); when opencode shuts\n\
// down it runs `session-end` for the sessions it saw. Both are detached: a session never\n\
// waits for them (and on Windows no console window opens).\n\
import {{ spawn }} from \"node:child_process\";\n\
\n\
const COMMAND = {command};\n\
const off = () => (process.env.GRAPHERSAL_MEMORY_HOOK ?? \"\") === \"off\";\n\
const args = (event) => [\"hook\", event, \"--harness\", \"opencode\", ...COMMAND.slice(1)];\n\
\n\
// opencode 1.x: `session.created` and `dispose`; the start line becomes a toast.\n\
const server = async ({{ client }}) => {{\n\
  const seen = new Map();\n\
  const run = (event, input, show) => {{\n\
    if (off()) return;\n\
    try {{\n\
      const child = Bun.spawn([COMMAND[0], ...args(event)], {{\n\
        stdin: new Blob([JSON.stringify(input)]),\n\
        stdout: show ? \"pipe\" : \"ignore\",\n\
        stderr: \"ignore\",\n\
        windowsHide: true,\n\
      }});\n\
      if (show) {{\n\
        new Response(child.stdout).text().then((line) => {{\n\
          const message = line.trim();\n\
          if (message) client.tui.showToast({{ body: {{ message, variant: \"info\" }} }}).catch(() => {{}});\n\
        }}).catch(() => {{}});\n\
      }} else {{\n\
        child.unref();\n\
      }}\n\
    }} catch {{\n\
      // The memory never breaks a session.\n\
    }}\n\
  }};\n\
  return {{\n\
    event: async ({{ event }}) => {{\n\
      if (event.type !== \"session.created\") return;\n\
      const info = event.properties?.info ?? {{}};\n\
      if (!info.id || info.parentID) return;\n\
      seen.set(info.id, info.directory ?? \"\");\n\
      run(\"session-start\", {{ session_id: info.id, cwd: info.directory ?? \"\" }}, true);\n\
    }},\n\
    dispose: async () => {{\n\
      for (const [id, cwd] of seen) run(\"session-end\", {{ session_id: id, cwd }}, false);\n\
    }},\n\
  }};\n\
}};\n\
\n\
// opencode 2.x: a session's first `session.execution.started` (or `session.created`), and the\n\
// cleanup when the plugin is unloaded. Server plugins have no toast in 2.x.\n\
const setup = async (ctx) => {{\n\
  const seen = new Map();\n\
  const run = (event, input) => {{\n\
    if (off()) return;\n\
    try {{\n\
      const child = spawn(COMMAND[0], args(event), {{ stdio: [\"pipe\", \"ignore\", \"ignore\"], detached: true, windowsHide: true }});\n\
      child.on(\"error\", () => {{}});\n\
      child.stdin.on(\"error\", () => {{}});\n\
      child.stdin.end(JSON.stringify(input));\n\
      child.unref();\n\
    }} catch {{\n\
      // The memory never breaks a session.\n\
    }}\n\
  }};\n\
  const abort = new AbortController();\n\
  const types = [\"session.created\", \"session.execution.started\"];\n\
  (async () => {{\n\
    for await (const event of ctx.event.subscribe({{ types, signal: abort.signal }})) {{\n\
      const data = event.data ?? {{}};\n\
      const id = data.sessionID;\n\
      if (!id || seen.has(id)) continue;\n\
      if (data.parentID) {{\n\
        seen.set(id, null);\n\
        continue;\n\
      }}\n\
      const cwd = data.location?.directory ?? event.location?.directory ?? ctx.location?.directory ?? \"\";\n\
      seen.set(id, cwd);\n\
      run(\"session-start\", {{ session_id: id, cwd }});\n\
    }}\n\
  }})().catch(() => {{}});\n\
  return () => {{\n\
    abort.abort();\n\
    for (const [id, cwd] of seen) if (cwd !== null) run(\"session-end\", {{ session_id: id, cwd }});\n\
  }};\n\
}};\n\
\n\
export default {{ id: \"graphersal-memory\", server, setup }};\n"
    )
}

/// The first line of our plugin file (how it is recognised).
pub const MARKER: &str = "graphersal-memory plugin (managed by `graphersal-memory install`)";

/// Whether a file's text is our plugin.
pub fn is_our_plugin(text: &str) -> bool {
    text.lines()
        .next()
        .is_some_and(|line| line.trim_start_matches("// ") == MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `resolve_for` with opencode 1.x (no `opencode --version` in unit tests).
    fn resolve_v1(env: &dyn Fn(&str) -> Option<String>) -> Result<OpenCodePaths, String> {
        OpenCodePaths::resolve_for(env, Detected::given(Major::V1))
    }

    #[test]
    fn paths_follow_xdg_then_home() {
        let paths = resolve_v1(&|name| match name {
            "HOME" => Some("/h".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(paths.config_dir, PathBuf::from("/h/.config/opencode"));
        assert_eq!(
            paths.config_file,
            PathBuf::from("/h/.config/opencode/opencode.json")
        );
        assert_eq!(
            paths.plugin_file,
            PathBuf::from("/h/.config/opencode/plugins/graphersal-memory.js")
        );
        assert_eq!(
            paths.db,
            PathBuf::from("/h/.local/share/opencode/opencode.db")
        );
        let custom = resolve_v1(&|name| match name {
            "HOME" => Some("/h".into()),
            "XDG_CONFIG_HOME" => Some("/c".into()),
            "XDG_DATA_HOME" => Some("/d".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(
            custom.rules_file,
            PathBuf::from("/c/opencode/graphersal-memory.md")
        );
        assert_eq!(custom.db, PathBuf::from("/d/opencode/opencode.db"));
        let db = resolve_v1(&|name| match name {
            "HOME" => Some("/h".into()),
            "OPENCODE_DB" => Some("/x/my.db".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(db.db, PathBuf::from("/x/my.db"));
        assert!(resolve_v1(&|_| None).is_err());
    }

    #[test]
    fn an_existing_jsonc_file_is_the_one_edited() {
        let home = crate::mcp_tests::TempDir::new("oc-paths");
        let dir = home.path().join(".config/opencode");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), "{}").unwrap();
        assert_eq!(
            OpenCodePaths::of_home(home.path()).config_file,
            dir.join("config.json")
        );
        std::fs::write(dir.join("opencode.jsonc"), "{}").unwrap();
        assert_eq!(
            OpenCodePaths::of_home(home.path()).config_file,
            dir.join("opencode.jsonc")
        );
    }

    #[test]
    fn entries_free_models_and_the_plugin_are_recognised() {
        let agent = OpenCode::new(OpenCodePaths::of_home(Path::new("/h")));
        let entry = agent.mcp_entry(Path::new("/x/graphersal-memory"), &["mcp".into()]);
        assert_eq!(entry["command"], json!(["/x/graphersal-memory", "mcp"]));
        assert!(crate::agents::is_our_entry(&entry));
        let words = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        assert!(free_model(&words(
            "opencode run --model opencode/big-pickle"
        )));
        assert!(free_model(&words("/bin/opencode run -m opencode/exo-free")));
        assert!(!free_model(&words(
            "opencode run --model anthropic/claude-sonnet-4"
        )));
        assert!(!free_model(&words("claude -p --model opencode/big-pickle")));
        let plugin = plugin_text(
            Path::new("/x/graphersal-memory"),
            &["--data-dir".into(), "/d".into()],
        );
        assert!(is_our_plugin(&plugin));
        assert!(
            plugin.contains("const COMMAND = [\"/x/graphersal-memory\",\"--data-dir\",\"/d\"];")
        );
        assert!(!is_our_plugin("export const X = 1\n"));
    }

    #[test]
    fn versions_are_parsed_from_both_lines() {
        assert_eq!(
            parse_version("1.18.35\n"),
            Some((Major::V1, "1.18.35".into()))
        );
        assert_eq!(
            parse_version("opencode v2.2.4\n"),
            Some((Major::V2, "2.2.4".into()))
        );
        assert_eq!(parse_version("0.15.2"), Some((Major::V1, "0.15.2".into())));
        assert_eq!(
            parse_version("3.0.0-beta.1"),
            Some((Major::V2, "3.0.0-beta.1".into()))
        );
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("{\"type\": \"text\"}"), None);
        assert_eq!(parse_version("opencode 2"), None);
    }

    #[test]
    fn paths_of_2x_read_no_config_json_and_honour_opencode_config_dir() {
        let home = crate::mcp_tests::TempDir::new("oc2-paths");
        let dir = home.path().join(".config/opencode");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), "{}").unwrap();
        let v1 = OpenCodePaths::of_home_for(home.path(), Major::V1);
        assert_eq!(v1.config_file, dir.join("config.json"));
        assert!(v1.stale_config_files.is_empty());
        assert_eq!(v1.servers_path(), &["mcp"]);
        let v2 = OpenCodePaths::of_home_for(home.path(), Major::V2);
        assert_eq!(
            v2.config_file,
            dir.join("opencode.json"),
            "2.x ignores config.json"
        );
        assert_eq!(v2.stale_config_files, vec![dir.join("config.json")]);
        assert_eq!(v2.servers_path(), &["mcp", "servers"]);
        assert_eq!(v2.db, v1.db, "one database for both lines");
        let text = home.path().display().to_string();
        let env = |name: &str| match name {
            "HOME" => Some(text.clone()),
            "OPENCODE_CONFIG_DIR" => Some(crate::mcp_tests::abs("/oc")),
            _ => None,
        };
        let custom = OpenCodePaths::resolve_for(&env, Detected::given(Major::V2)).unwrap();
        let oc = PathBuf::from(crate::mcp_tests::abs("/oc"));
        assert_eq!(custom.config_dir, oc);
        assert_eq!(
            custom.plugin_file,
            oc.join("plugins").join("graphersal-memory.js")
        );
        let old = OpenCodePaths::resolve_for(&env, Detected::given(Major::V1)).unwrap();
        assert_eq!(
            old.config_dir, dir,
            "1.x: OPENCODE_CONFIG_DIR is an extra directory"
        );
    }

    #[test]
    fn entries_of_2x_are_switched_by_disabled_and_keep_code_mode_off() {
        let agent = OpenCode::new(OpenCodePaths::of_home_for(Path::new("/h"), Major::V2));
        let entry = agent.mcp_entry(Path::new("/x/graphersal-memory"), &["mcp".into()]);
        assert_eq!(
            entry,
            json!({"type": "local", "command": ["/x/graphersal-memory", "mcp"],
                   "disabled": false, "codemode": false})
        );
        assert!(crate::agents::is_our_entry(&entry));
        assert!(
            agent
                .remove_server_hint("x")
                .starts_with("the member mcp.servers.x of ")
        );
    }

    #[test]
    fn the_plugin_serves_both_lines_from_one_default_export() {
        let plugin = plugin_text(Path::new("/x/graphersal-memory"), &[]);
        assert!(is_our_plugin(&plugin));
        assert!(
            plugin.contains("const server = async ({ client })"),
            "{plugin}"
        );
        assert!(plugin.contains("const setup = async (ctx)"), "{plugin}");
        assert!(plugin.contains("session.execution.started"), "{plugin}");
        assert!(
            plugin.ends_with("export default { id: \"graphersal-memory\", server, setup };\n"),
            "{plugin}"
        );
        // Only the default export: 1.x would call every other exported function as a plugin.
        assert_eq!(plugin.matches("export ").count(), 1, "{plugin}");
    }

    /// A fake `opencode` printing `version` for `--version`.
    #[cfg(unix)]
    fn fake_program(dir: &Path, version: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = bin.join("opencode");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nmkdir -p \"$HOME/.config/opencode\"\n[ \"$1\" = --version ] && echo '{version}'\nexit 0\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    #[cfg(unix)]
    #[test]
    fn the_line_is_asked_from_the_binary_then_read_from_the_data() {
        let tmp = crate::mcp_tests::TempDir::new("oc-detect");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let text = home.display().to_string();
        let env = |name: &str| (name == "HOME").then(|| text.clone());
        // No binary, no data: the current line.
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let none = detect(&env, Some(empty.as_os_str()), true).unwrap();
        assert_eq!(none.major, Major::V2);
        assert!(none.source.contains("not found"), "{}", none.source);
        // Data of 1.x, then of 2.x after an upgrade.
        let db = home.join(".local/share/opencode/opencode.db");
        sessions::tests::fixture(&db, "/w");
        let found = detect(&env, Some(empty.as_os_str()), true).unwrap();
        assert_eq!((found.major, found.version.clone()), (Major::V1, None));
        assert!(found.source.contains("opencode.db"), "{}", found.source);
        sessions::tests::fixture_v2(&db, "/w");
        assert_eq!(
            detect(&env, Some(empty.as_os_str()), true).unwrap().major,
            Major::V2
        );
        // The binary wins over the data, and asking it leaves nothing behind.
        let program = fake_program(&tmp.path().join("one"), "1.19.5");
        let bin = program.parent().unwrap().as_os_str().to_os_string();
        let asked = detect(&env, Some(&bin), true).unwrap();
        assert_eq!(
            (asked.major, asked.version.as_deref()),
            (Major::V1, Some("1.19.5"))
        );
        assert!(
            asked.describe().starts_with("opencode 1.x (1.19.5, from `"),
            "{}",
            asked.describe()
        );
        assert!(
            !home.join(".config").exists(),
            "--version ran in a scratch home"
        );
        // Without probing (the hooks), only the data counts.
        assert_eq!(detect(&env, Some(&bin), false).unwrap().major, Major::V2);
        // The install script's place.
        let script_home = tmp.path().join("script-home");
        let installed = fake_program(&script_home.join(".opencode"), "opencode v2.2.4");
        assert!(installed.ends_with(".opencode/bin/opencode"));
        let text2 = script_home.display().to_string();
        let env2 = |name: &str| (name == "HOME").then(|| text2.clone());
        let found = detect(&env2, Some(empty.as_os_str()), true).unwrap();
        assert_eq!(
            (found.major, found.version.as_deref()),
            (Major::V2, Some("2.2.4"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn headless_runs_of_2x_get_their_own_server() {
        let tmp = crate::mcp_tests::TempDir::new("oc-headless");
        let words = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        let two = fake_program(&tmp.path().join("two"), "opencode v2.2.4");
        let command = words(&format!(
            "{} run --model opencode/big-pickle",
            two.display()
        ));
        assert_eq!(
            headless_args(&command)[1..],
            words("run --standalone --model opencode/big-pickle")[..]
        );
        let already = words(&format!("{} run --standalone -m x/y", two.display()));
        assert_eq!(headless_args(&already), already);
        let one = fake_program(&tmp.path().join("one"), "1.18.35");
        let command = words(&format!(
            "{} run --model opencode/big-pickle",
            one.display()
        ));
        assert_eq!(
            headless_args(&command),
            command,
            "1.x refuses unknown flags"
        );
        let claude = words("claude -p --model sonnet");
        assert_eq!(headless_args(&claude), claude);
    }

    #[test]
    fn run_events_become_the_answer_object() {
        let stdout = [
            json!({"type": "step_start", "part": {"type": "step-start"}}),
            json!({"type": "text", "part": {"type": "text", "text": "{\"headline\": \"h\", \"summary\": \"s\"}"}}),
            json!({"type": "step_finish", "part": {"type": "step-finish", "cost": 0.25,
                "tokens": {"input": 100, "output": 20, "reasoning": 5, "cache": {"read": 7, "write": 3}}}}),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let value = normalize_run_output(&stdout);
        assert_eq!(value["is_error"], false);
        assert_eq!(value["total_cost_usd"], 0.25);
        assert_eq!(value["usage"]["input_tokens"], 110);
        assert_eq!(value["usage"]["output_tokens"], 25);
        assert_eq!(value["num_turns"], 1);
        assert_eq!(value["structured_output"]["headline"], "h");
        let failed = normalize_run_output(
            &json!({"type": "error", "error": {"name": "APIError", "data": {"message": "no key"}}})
                .to_string(),
        );
        assert_eq!(
            (failed["is_error"].clone(), failed["result"].clone()),
            (json!(true), json!("no key"))
        );
        assert_eq!(normalize_run_output("not json")["is_error"], true);
    }
}
