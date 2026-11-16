//! OpenAI Codex CLI (verified against the source of release `rust-v0.167.0`,
//! github.com/openai/codex, and its documentation): where it keeps its files, what
//! graphersal-memory writes there, and how its sessions are read.
//!
//! - **Home**: `$CODEX_HOME` when set (it must exist), else `~/.codex`
//!   (`codex-rs/utils/home-dir/src/lib.rs`). The user configuration is `config.toml` there
//!   (TOML; Codex itself edits it, for example for project trust and hook trust).
//! - **MCP servers**: tables `[mcp_servers.<name>]` with `command`, `args`, `enabled` (default
//!   true; `false` = not started), `startup_timeout_sec` (default 10)
//!   (`codex-rs/config/src/mcp_types.rs`). The model sees a tool as `mcp__<server>__<tool>`
//!   with every character outside `[A-Za-z0-9_]` replaced by `_`
//!   (`codex-rs/codex-mcp/src/mcp/mod.rs`, `qualified_mcp_tool_name_prefix`), so our tools are
//!   `mcp__graphersal_memory__<function>`. install writes both servers in one managed block of
//!   `config.toml` (`crate::install::tomledit`): `use` enabled, `develop` with `enabled = false` until
//!   `--enable-develop` (Codex has a switched-off state). `codex mcp add` is not used: it
//!   rewrites the file.
//! - **Rules**: Codex loads one global instruction file, the first non-empty of
//!   `$CODEX_HOME/AGENTS.override.md` and `$CODEX_HOME/AGENTS.md`
//!   (`codex-rs/codex-home/src/instructions/mod.rs`), before the project's `AGENTS.md` files.
//!   No setting adds a second global file (`developer_instructions` is one string in
//!   `config.toml` that a project's configuration replaces), so the managed rule block goes
//!   into the global file Codex loads, between the fingerprinted marker lines Claude Code's
//!   `CLAUDE.md` block uses; the user's text around it stays as it is.
//! - **Session start and end**: lifecycle hooks (feature `hooks`, stable, on by default;
//!   `codex-rs/features/src/lib.rs`) in `$CODEX_HOME/hooks.json`, the shape of Claude Code's
//!   settings (`{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command",
//!   "timeout"}]}]}}`; `codex-rs/config/src/hook_config.rs`). The input on stdin carries
//!   `session_id`, `transcript_path` (the rollout file) and `cwd`; plain stdout of
//!   `SessionStart` becomes model context; `SessionEnd` may take at most 3 seconds
//!   (`codex-rs/hooks/src/events/session_end.rs`). Codex runs a user hook only after the user
//!   reviewed and trusted it in `/hooks` (the trust is recorded in `config.toml`,
//!   `hooks.state`); install does not trust its own hooks for the user: the report says so,
//!   and without the hooks the sessions are still indexed once unchanged for 30 minutes.
//! - **Sessions**: one JSON Lines rollout file per session,
//!   `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<timestamp>-<thread id>.jsonl`
//!   (`codex-rs/rollout/src/recorder.rs`), read by [`sessions`]. Rollouts older than 7 days may
//!   be compressed to `.jsonl.zst` by a feature that is off by default
//!   (`local_thread_store_compression`); those are not read.
//! - **Headless runs**: `codex exec --json` reads the prompt from stdin and prints JSON Lines
//!   events (`item.completed` with an `agent_message`, `turn.completed` with token `usage`, no
//!   cost; `codex-rs/exec/src/exec_events.rs`). The summaries and the bench run it with a
//!   `CODEX_HOME` of their own ([`build_sandbox`]). `--oss --local-provider lmstudio` runs a
//!   local model (LM Studio at `localhost:1234`, default model `openai/gpt-oss-20b`).
//! - **Memory**: Codex has a memory feature of its own (`[features] memories`, off by default;
//!   generated files under `$CODEX_HOME/memories_v2/`). When the user switched it on, the
//!   wizard offers to switch it off (restored by uninstall). Codex has no file memory that we
//!   could write back into: the export of `uninstall` writes readable Markdown files into
//!   `~/graphersal-memory-export/codex/`.

pub mod bench;
pub mod hook;
pub mod install;
pub mod other;
pub mod sessions;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::harnesses::agent::{Harness, Kind};

/// The user configuration file in the Codex home.
pub const CONFIG_FILE: &str = "config.toml";

/// The hooks file in the Codex home.
pub const HOOKS_FILE: &str = "hooks.json";

/// The default headless command of Codex's summaries and bench (the model of the user's own
/// configuration; `--oss --local-provider lmstudio` for a local model).
pub const DEFAULT_HEADLESS_COMMAND: &str = "codex exec";

/// How long Codex waits for our MCP server to start (seconds; its default is 10, and the first
/// connection may start the daemon and open the store).
pub const STARTUP_TIMEOUT_SECS: u64 = 30;

/// How long Codex lets the session start hook run (seconds; the hook itself stops earlier).
pub const START_HOOK_TIMEOUT_SECS: u64 = 30;

/// How long Codex lets the session end hook run (seconds; Codex allows at most 3).
pub const END_HOOK_TIMEOUT_SECS: u64 = 3;

/// The settings that pick the model, copied from the user's configuration into a headless
/// sandbox (so the summaries run on the user's own model).
pub const MODEL_KEYS: [&str; 5] = [
    "model",
    "model_provider",
    "model_reasoning_effort",
    "oss_provider",
    "model_providers",
];

/// Where Codex keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexPaths {
    /// The home directory.
    pub home: PathBuf,
    /// The Codex home (`$CODEX_HOME` or `~/.codex`).
    pub codex_home: PathBuf,
    /// `config.toml`.
    pub config_file: PathBuf,
    /// `hooks.json`.
    pub hooks_file: PathBuf,
    /// The global instruction file install writes the rule block into.
    pub instruction_file: PathBuf,
    /// The rollout directory.
    pub sessions: PathBuf,
    /// Where `uninstall` exports the memories as Markdown.
    pub export_dir: PathBuf,
}

impl CodexPaths {
    /// Resolves the paths from the environment (`env` reads one variable): `CODEX_HOME`
    /// (canonicalized like Codex does, `codex-rs/utils/home-dir`, so the hook trust keys Codex
    /// records under the hooks file's path match ours), else the home directory
    /// ([`crate::runtime::store::home_dir`]).
    pub fn resolve(env: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let set = |name: &str| env(name).filter(|value| !value.is_empty());
        let home = crate::runtime::store::home_dir(env)?;
        let codex_home = match set("CODEX_HOME") {
            Some(dir) if Path::new(&dir).is_absolute() => {
                crate::runtime::mcp::canonical(Path::new(&dir))
            }
            _ => home.join(".codex"),
        };
        Ok(Self::with(home, codex_home))
    }

    fn with(home: PathBuf, codex_home: PathBuf) -> Self {
        Self {
            config_file: codex_home.join(CONFIG_FILE),
            hooks_file: codex_home.join(HOOKS_FILE),
            instruction_file: instruction_file(&codex_home),
            sessions: codex_home.join("sessions"),
            export_dir: home.join("graphersal-memory-export").join("codex"),
            codex_home,
            home,
        }
    }

    /// The paths of a home directory without environment overrides (tests).
    #[cfg(test)]
    pub fn of_home(home: &Path) -> Self {
        Self::with(home.to_path_buf(), home.join(".codex"))
    }

    /// The user's login file (copied into headless sandboxes).
    pub fn auth_file(&self) -> PathBuf {
        self.codex_home.join("auth.json")
    }
}

/// The global instruction file Codex loads: `AGENTS.override.md` when it holds text, else
/// `AGENTS.md`.
pub fn instruction_file(codex_home: &Path) -> PathBuf {
    let preferred = codex_home.join("AGENTS.override.md");
    let has_text = std::fs::read_to_string(&preferred).is_ok_and(|text| !text.trim().is_empty());
    if has_text {
        preferred
    } else {
        codex_home.join("AGENTS.md")
    }
}

/// The `codex` program on `search_path` (`None`: this process's `PATH`).
pub fn find_program(search_path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let path = match search_path {
        Some(path) => Some(path.to_os_string()),
        None => std::env::var_os("PATH"),
    };
    let names = ["codex", "codex.exe", "codex.cmd"];
    path.iter()
        .flat_map(std::env::split_paths)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

/// The version in `codex --version`'s output (`codex-cli 0.167.0`): its last word.
pub fn parse_version(output: &str) -> Option<String> {
    let word = output.split_whitespace().last()?;
    let version = word.strip_prefix('v').unwrap_or(word);
    (version.contains('.') && version.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .then(|| version.to_string())
}

/// Asks `program --version` (at most 10 seconds), with a `CODEX_HOME` of its own in a
/// temporary directory (removed afterwards), so asking leaves nothing behind.
pub fn version_of(program: &Path) -> Option<String> {
    static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let run = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scratch = std::env::temp_dir().join(format!(
        "graphersal-memory-codex-version-{}-{run}",
        std::process::id()
    ));
    let found = std::fs::create_dir_all(&scratch).ok().and_then(|()| {
        let dir = scratch.display().to_string();
        let env = [
            ("CODEX_HOME", dir.as_str()),
            (crate::memory::summaries::HOOK_ENV, "off"),
        ];
        crate::harnesses::headless::run(
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
    found
}

/// Codex, as the installation sees it.
#[derive(Debug, Clone)]
pub struct Codex {
    paths: CodexPaths,
}

impl Codex {
    /// Codex with its files at `paths`.
    pub fn new(paths: CodexPaths) -> Self {
        Self { paths }
    }

    /// Its files.
    pub fn paths(&self) -> &CodexPaths {
        &self.paths
    }
}

impl crate::harnesses::agent::OwnMemory for Codex {
    /// Its memories: on with `[features] memories = true` in `config.toml`.
    fn now(
        &self,
        setup: &crate::install::Setup,
    ) -> Result<crate::harnesses::agent::OwnMemoryNow, String> {
        let on = crate::install::read_text(&self.paths.config_file)
            .ok()
            .flatten()
            .and_then(|text| crate::install::tomledit::parse(&text).ok())
            .is_some_and(|doc| install::memories_on(&doc));
        let answered = crate::install::State::load_of(setup, Kind::Codex)?
            .meta
            .contains_key("auto_memory_off");
        Ok(crate::harnesses::agent::OwnMemoryNow {
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
    ) -> crate::harnesses::agent::OwnMemoryQuestion {
        crate::harnesses::agent::OwnMemoryQuestion {
            title: "Codex's memories".to_string(),
            lines: vec![
                "Codex's own memory feature is on: it generates memories from your chats and"
                    .to_string(),
                "loads them into sessions; with graphersal-memory both would keep the same facts"
                    .to_string(),
                "twice.".to_string(),
                install::NOTHING_IS_LOST.to_string(),
            ],
            yes: format!(
                "switch it off ([features] memories = false in {})",
                setup.show(&self.paths.config_file.display().to_string())
            ),
            no: "keep it on".to_string(),
        }
    }

    fn also(&self, setup: &crate::install::Setup) -> Vec<String> {
        vec![
            "The same answer switches Codex's own memories off ([features] memories in".to_string(),
            format!(
                "{}); uninstall switches them on again.",
                setup.show(&self.paths.config_file.display().to_string())
            ),
        ]
    }
}

impl crate::harnesses::agent::HookHandler for Codex {
    /// Its rollout queued (active at the start, ended at the end); the start line becomes model
    /// context.
    fn run(
        &self,
        setup: &crate::install::Setup,
        event: crate::memory::hook::HookEvent,
        input: Vec<u8>,
        stdout: &mut dyn std::io::Write,
        stderr: &mut dyn std::io::Write,
    ) -> u8 {
        hook::run_codex(
            event,
            self.paths.sessions.clone(),
            Some(crate::commands::hook_enqueue(setup)),
            std::io::Cursor::new(input),
            stdout,
            stderr,
        )
    }
}

impl Harness for Codex {
    fn kind(&self) -> Kind {
        Kind::Codex
    }

    fn find_program(&self, _home: &Path, search: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
        find_program(search)
    }

    fn config_roots(&self) -> Vec<PathBuf> {
        vec![self.paths.codex_home.clone()]
    }

    fn has_our_parts(&self, setup: &crate::install::Setup) -> bool {
        !install::inspect(setup).nothing_installed()
    }

    fn own_memory(&self) -> Option<&dyn crate::harnesses::agent::OwnMemory> {
        Some(self)
    }

    fn develop_on(&self, setup: &crate::install::Setup) -> bool {
        let (state, enabled) = install::inspect(setup).develop_server;
        state != crate::install::PartState::Missing && enabled
    }

    /// Codex runs our hooks only once the user trusts them in `/hooks`.
    fn action_needed(&self, setup: &crate::install::Setup) -> Option<String> {
        (!install::inspect(setup).hooks_trusted).then(install::hook_approval_line)
    }

    fn hook(&self) -> Option<&dyn crate::harnesses::agent::HookHandler> {
        Some(self)
    }

    fn export_hint(&self) -> String {
        "to give the agent a file, paste it into ~/.codex/AGENTS.md or a project's AGENTS.md"
            .to_string()
    }

    #[cfg(test)]
    fn user_files(&self, home: &Path) {
        fixture::user_files(home);
    }

    fn bench(&self) -> Option<std::sync::Arc<dyn crate::bench::BenchSupport>> {
        Some(std::sync::Arc::new(self.clone()))
    }

    fn has_sessions(&self) -> bool {
        self.paths.sessions.is_dir()
    }

    fn other_memory(&self) -> Option<&dyn crate::install::coexist::OtherMemory> {
        Some(self)
    }

    fn status_lines(&self, setup: &crate::install::Setup) -> Vec<String> {
        install::status_lines(setup)
    }

    fn doctor(
        &self,
        setup: &crate::install::Setup,
        _project: Option<&Path>,
    ) -> Vec<crate::install::Finding> {
        install::doctor(setup)
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
            .is_some_and(|entry| entry.contains_key(install::MEMORIES_RECORD))
            .then(|| {
                (
                    config.clone(),
                    "Codex's memories: switched on again, as before install".to_string(),
                )
            })
    }

    /// With the record of the memories switch, which goes with the write of `config.toml`.
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
        let config = self.paths.config_file.clone();
        match plan.memories {
            Some(true) => {
                out.records.push((
                    config,
                    install::MEMORIES_RECORD,
                    Some(serde_json::json!({"previous": true})),
                ));
                out.switched_off = Some(
                    "Codex's memories: off (uninstall exports every memory as Markdown and \
                     switches them on again)"
                        .to_string(),
                );
                out.nothing_is_lost = Some(install::NOTHING_IS_LOST);
            }
            Some(false) => out.records.push((config, install::MEMORIES_RECORD, None)),
            None => {}
        }
        Ok(out)
    }

    fn mcp_config_file(&self) -> &Path {
        &self.paths.config_file
    }

    fn mcp_servers_key(&self) -> &'static str {
        "mcp_servers"
    }

    /// `{"command", "args", "startup_timeout_sec"}`: the keys of `[mcp_servers.<name>]`.
    fn mcp_entry(&self, exe: &Path, args: &[String]) -> Value {
        json!({"command": exe.display().to_string(), "args": args,
               "startup_timeout_sec": STARTUP_TIMEOUT_SECS})
    }

    fn remove_server_hint(&self, name: &str) -> String {
        format!(
            "the table [mcp_servers.{name}] of {}",
            self.paths.config_file.display()
        )
    }

    /// `config.toml` (the memories switch is there).
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

    /// `~/graphersal-memory-export/codex/<encoded root>/`.
    fn memory_dir(&self, root: &Path) -> PathBuf {
        self.paths
            .export_dir
            .join(crate::harnesses::claude_code::import::claude_slug(
                &root.display().to_string(),
            ))
    }

    fn global_memory_file(&self) -> PathBuf {
        self.paths.export_dir.join("global.md")
    }
}

/// Whether `command` runs Codex.
pub fn is_codex(command: &[String]) -> bool {
    command.first().is_some_and(|program| {
        Path::new(program)
            .file_stem()
            .is_some_and(|stem| stem == "codex")
    })
}

/// Whether a Codex command runs a local model (`--oss`: LM Studio or Ollama on this machine),
/// which costs nothing.
pub fn local_model(command: &[String]) -> bool {
    is_codex(command) && command.iter().any(|arg| arg == "--oss")
}

/// The arguments of a headless Codex run: `command`, plus `--json` (events on stdout),
/// `--ephemeral` (no rollout file), `--skip-git-repo-check` (the sandbox's directory is no
/// repository) and `--sandbox read-only`, each unless the command has it. The prompt comes on
/// stdin (no prompt argument).
pub fn exec_args(command: &[String]) -> Vec<String> {
    let mut args = command.to_vec();
    for flag in ["--json", "--ephemeral", "--skip-git-repo-check"] {
        if !args.iter().any(|arg| arg == flag) {
            args.push(flag.to_string());
        }
    }
    if !args
        .iter()
        .any(|arg| arg == "--sandbox" || arg == "-s" || arg.starts_with("--sandbox="))
    {
        args.extend(["--sandbox".to_string(), "read-only".to_string()]);
    }
    // `--oss --local-provider X` alone left 0.167.0 on the `openai` provider in the smoke run
    // ("This chat uses lmstudio, but your current provider is openai"): name it explicitly.
    if let Some(provider) = local_provider(command)
        && !args.iter().any(|arg| arg.starts_with("model_provider="))
    {
        args.extend([
            "-c".to_string(),
            format!(
                "model_provider={}",
                crate::install::tomledit::string(&provider)
            ),
        ]);
    }
    args
}

/// The provider of `--local-provider X` (or `--local-provider=X`) in a `--oss` command.
pub fn local_provider(command: &[String]) -> Option<String> {
    if !local_model(command) {
        return None;
    }
    command
        .windows(2)
        .find(|pair| pair[0] == "--local-provider")
        .map(|pair| pair[1].clone())
        .or_else(|| {
            command
                .iter()
                .find_map(|arg| arg.strip_prefix("--local-provider=").map(str::to_string))
        })
}

/// A JSON value as a TOML value (inline tables for objects; `None` for null).
pub fn toml_value(value: &Value) -> Option<String> {
    Some(match value {
        Value::Null => return None,
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => crate::install::tomledit::string(s),
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .filter_map(toml_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(map) => format!(
            "{{ {} }}",
            map.iter()
                .filter_map(|(k, v)| Some(format!(
                    "{} = {}",
                    crate::install::tomledit::string(k),
                    toml_value(v)?
                )))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    })
}

/// The model settings of the user's `config.toml` ([`MODEL_KEYS`]) as top-level TOML lines.
pub fn model_lines(user_config: Option<&Path>) -> String {
    let Some(doc) = user_config
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| crate::install::tomledit::parse(&text).ok())
    else {
        return String::new();
    };
    MODEL_KEYS
        .iter()
        .filter_map(|key| Some(format!("{key} = {}\n", toml_value(doc.get(*key)?)?)))
        .collect()
}

/// The `config.toml` of a headless sandbox: the user's model settings, no approval prompts,
/// no hooks, no Codex memories, no history, no project instruction files, every built-in tool
/// off (shell, unified exec, image viewing, web search, sub-agents, apps, plugins: the
/// `[features]` of `codex-rs/features/src/lib.rs`), then `tables` (whole tables, for example
/// the graph arm's MCP server).
pub fn sandbox_config(user_config: Option<&Path>, tables: &str) -> String {
    format!(
        "# A sandbox of graphersal-memory for headless Codex runs.\n{}\
approval_policy = \"never\"\nproject_doc_max_bytes = 0\nweb_search = \"disabled\"\n\n\
[features]\nhooks = false\nmemories = false\nshell_tool = false\nunified_exec = false\n\
view_image = false\nmulti_agent = false\napps = false\nplugins = false\n\n\
[history]\npersistence = \"none\"\n{}{tables}",
        model_lines(user_config),
        if tables.is_empty() { "" } else { "\n" }
    )
}

/// Builds a sandbox for headless Codex runs in `dir`: `home/` as its `CODEX_HOME` with
/// `config` as `config.toml`, `agents` (when given) as its global `AGENTS.md`, and a copy of
/// the user's login `auth.json` (read, never changed) when there is one. Returns the
/// environment of the runs.
pub fn build_sandbox(
    dir: &Path,
    config: &str,
    agents: Option<&str>,
    user_auth: Option<&Path>,
) -> Result<Vec<(String, String)>, String> {
    let home = dir.join("home");
    std::fs::create_dir_all(&home)
        .map_err(|err| format!("cannot create {}: {err}", home.display()))?;
    crate::fsutil::write_atomic(&home.join(CONFIG_FILE), config.as_bytes())?;
    if let Some(agents) = agents {
        crate::fsutil::write_atomic(&home.join("AGENTS.md"), agents.as_bytes())?;
    }
    if let Some(auth) = user_auth.filter(|auth| auth.is_file()) {
        let target = home.join("auth.json");
        std::fs::copy(auth, &target)
            .map_err(|err| format!("cannot copy {}: {err}", auth.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600));
        }
    }
    Ok(vec![
        ("CODEX_HOME".into(), home.display().to_string()),
        (crate::memory::summaries::HOOK_ENV.into(), "off".into()),
    ])
}

/// `codex exec --json`'s event lines turned into the object `claude -p --output-format json`
/// answers (what the summaries and the bench read): `result` = the last agent message,
/// `usage` = the sums of the `turn.completed` usages (input with the cached part, output with
/// the reasoning part as Codex reports them separately), `num_turns` = the agent messages and
/// MCP calls, `is_error` for a `turn.failed`/`error` event or no message, `structured_output`
/// = the JSON object in the message. Codex reports no cost: `total_cost_usd` is 0 for a local
/// model (`local`) and absent otherwise (the caller then counts its estimate).
pub fn normalize_exec_output(stdout: &str, local: bool) -> Value {
    let mut last_message: Option<String> = None;
    let (mut input, mut output, mut steps) = (0u64, 0u64, 0u64);
    let mut error: Option<String> = None;
    let mut events = 0;
    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        events += 1;
        let n = |v: &Value| v.as_u64().unwrap_or(0);
        match event["type"].as_str().unwrap_or_default() {
            "item.completed" => match event["item"]["type"].as_str().unwrap_or_default() {
                "agent_message" => {
                    steps += 1;
                    if let Some(text) = event["item"]["text"].as_str() {
                        last_message = Some(text.to_string());
                    }
                }
                "mcp_tool_call" => steps += 1,
                _ => {}
            },
            "turn.completed" => {
                let usage = &event["usage"];
                input += n(&usage["input_tokens"]) + n(&usage["cache_write_input_tokens"]);
                output += n(&usage["output_tokens"]) + n(&usage["reasoning_output_tokens"]);
            }
            "turn.failed" => {
                error = event["error"]["message"].as_str().map(str::to_string);
            }
            "error" => {
                error = event["message"].as_str().map(str::to_string);
            }
            _ => {}
        }
    }
    let mut value = json!({
        "type": "result", "num_turns": steps,
        "usage": {"input_tokens": input, "output_tokens": output},
    });
    if local {
        value["total_cost_usd"] = json!(0.0);
    }
    let result = last_message.unwrap_or_default();
    if let Some(error) = error.filter(|_| result.trim().is_empty()) {
        value["is_error"] = json!(true);
        value["result"] = json!(error);
    } else if result.trim().is_empty() {
        value["is_error"] = json!(true);
        value["result"] = json!(if events == 0 {
            "codex answered no JSON events".to_string()
        } else {
            "codex answered no message".to_string()
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

/// Codex's headless runs (`crate::harnesses::headless::Headless`): `codex exec --json --ephemeral` in a
/// `CODEX_HOME` of its own.
pub struct HeadlessRun;

impl crate::harnesses::headless::Headless for HeadlessRun {
    fn matches(&self, command: &[String]) -> bool {
        is_codex(command)
    }

    fn default_command(&self) -> Option<&'static str> {
        Some(DEFAULT_HEADLESS_COMMAND)
    }

    fn bench_command(&self) -> Option<&'static str> {
        Some("codex exec --oss --local-provider lmstudio")
    }

    fn default_command_note(&self) -> &'static str {
        "Codex only; the model of your Codex configuration"
    }

    fn costs_nothing(&self, command: &[String]) -> bool {
        local_model(command)
    }

    fn free_note(&self, command: &[String]) -> Option<String> {
        local_model(command)
            .then(|| format!("`{}` runs a local model on this machine", command.join(" ")))
    }

    fn summaries_run(
        &self,
        args: Vec<String>,
        work_dir: &Path,
        user_env: crate::harnesses::headless::UserEnv<'_>,
    ) -> Result<crate::harnesses::headless::Prepared, String> {
        // The user's model settings and login (copies): from their Codex home.
        let codex_home = user_env
            .and_then(|env| CodexPaths::resolve(env).ok())
            .map(|paths| paths.codex_home);
        let args = exec_args(&args);
        let sandbox = work_dir.join("codex");
        let _ = std::fs::remove_dir_all(&sandbox);
        let config = sandbox_config(
            codex_home.as_ref().map(|d| d.join(CONFIG_FILE)).as_deref(),
            "",
        );
        let env = build_sandbox(
            &sandbox,
            &config,
            None,
            codex_home.as_ref().map(|d| d.join("auth.json")).as_deref(),
        )?;
        Ok((args, env))
    }

    fn answer(&self, stdout: &str, command: &[String]) -> String {
        normalize_exec_output(stdout, local_model(command)).to_string()
    }

    fn bench_args(&self, args: &[String]) -> Vec<String> {
        exec_args(args)
    }

    /// Codex reports a failed start (a local server that does not answer, for example) on
    /// stderr only: added to the error.
    fn bench_answer(
        &self,
        output: &crate::harnesses::headless::Output,
        command: &[String],
    ) -> Option<Value> {
        let mut answer = normalize_exec_output(&output.stdout, local_model(command));
        if answer["is_error"] == true && !output.stderr.trim().is_empty() {
            answer["result"] = json!(format!(
                "{} ({})",
                answer["result"].as_str().unwrap_or_default(),
                crate::memory::transcript::clip(output.stderr.trim(), 300)
            ));
        }
        Some(answer)
    }

    fn bench_refusal(&self, command: &[String]) -> Option<(&'static str, &'static str)> {
        (!local_model(command)).then_some((
            "the bench through Codex needs a local model (`--oss`).",
            "`codex exec` reports tokens but no cost, so the budget could not stop a run on a \
             paid model.",
        ))
    }
}

/// What the conformance tests (`crate::harnesses::conformance_tests`) and the install tests start from: a
/// home with Codex's configuration as a user has it (their own entries, comments, rules).
#[cfg(test)]
pub(crate) mod fixture {
    use std::path::Path;

    /// The user's own Codex configuration: comments, top-level keys, a project table, an MCP server
    /// of another program; their own AGENTS.md and a hook of theirs.
    pub(crate) const CODEX_CONFIG: &str = "# my Codex settings\nmodel = \"gpt-5.5\"\napproval_policy = \"on-request\"\n\n[mcp_servers.other]\ncommand = \"npx\"\nargs = [\"-y\", \"other\"]  # keep\n\n[projects.\"/w/app\"]\ntrust_level = \"trusted\"\n";

    pub(crate) fn user_files(home: &Path) {
        let dir = home.join(".codex");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), CODEX_CONFIG).unwrap();
        std::fs::write(
            dir.join("AGENTS.md"),
            "# My Codex rules\n\nTabs, not spaces.\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("hooks.json"),
            "{\n  \"hooks\": {\n    \"Stop\": [{\"hooks\": [{\"type\": \"command\", \"command\": \"say done\"}]}]\n  }\n}\n",
        )
        .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_follow_codex_home_then_home() {
        let paths =
            CodexPaths::resolve(&|name| (name == "HOME").then(|| "/h".to_string())).unwrap();
        assert_eq!(paths.codex_home, PathBuf::from("/h/.codex"));
        assert_eq!(paths.config_file, PathBuf::from("/h/.codex/config.toml"));
        assert_eq!(paths.hooks_file, PathBuf::from("/h/.codex/hooks.json"));
        assert_eq!(paths.instruction_file, PathBuf::from("/h/.codex/AGENTS.md"));
        assert_eq!(paths.sessions, PathBuf::from("/h/.codex/sessions"));
        assert_eq!(
            paths.export_dir,
            PathBuf::from("/h/graphersal-memory-export/codex")
        );
        let codex_home = crate::runtime::mcp_tests::abs("/c");
        let custom = CodexPaths::resolve(&|name| match name {
            "HOME" => Some("/h".into()),
            "CODEX_HOME" => Some(codex_home.clone()),
            _ => None,
        })
        .unwrap();
        assert_eq!(
            custom.config_file,
            PathBuf::from(&codex_home).join("config.toml")
        );
        assert!(CodexPaths::resolve(&|_| None).is_err());
    }

    #[test]
    fn an_existing_codex_home_is_canonical_like_codex_makes_it() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("codex-canon");
        let given = tmp.path().join("x").join("..").join("home");
        std::fs::create_dir_all(tmp.path().join("x")).unwrap();
        std::fs::create_dir_all(tmp.path().join("home")).unwrap();
        let text = given.display().to_string();
        let paths = CodexPaths::resolve(&|name| match name {
            "HOME" | "USERPROFILE" => Some("/h".into()),
            "CODEX_HOME" => Some(text.clone()),
            _ => None,
        })
        .unwrap();
        let canonical = crate::runtime::mcp::canonical(&tmp.path().join("home"));
        assert_eq!(paths.codex_home, canonical);
        assert!(!paths.codex_home.display().to_string().contains(".."));
        assert!(!paths.codex_home.display().to_string().starts_with(r"\\?\"));
        assert_eq!(paths.hooks_file, canonical.join("hooks.json"));
    }

    #[test]
    fn a_non_empty_override_file_is_the_one_codex_loads() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("codex-agents");
        let home = tmp.path().join(".codex");
        std::fs::create_dir_all(&home).unwrap();
        assert_eq!(instruction_file(&home), home.join("AGENTS.md"));
        std::fs::write(home.join("AGENTS.override.md"), " \n").unwrap();
        assert_eq!(
            instruction_file(&home),
            home.join("AGENTS.md"),
            "empty: skipped"
        );
        std::fs::write(home.join("AGENTS.override.md"), "# mine\n").unwrap();
        assert_eq!(instruction_file(&home), home.join("AGENTS.override.md"));
    }

    #[test]
    fn entries_versions_and_commands_are_recognised() {
        let agent = Codex::new(CodexPaths::of_home(Path::new("/h")));
        let entry = agent.mcp_entry(Path::new("/x/graphersal-memory"), &["mcp".into()]);
        assert_eq!(entry["command"], "/x/graphersal-memory");
        assert!(crate::harnesses::agent::is_our_entry(&entry));
        assert_eq!(parse_version("codex-cli 0.167.0\n"), Some("0.167.0".into()));
        assert_eq!(parse_version("codex-cli"), None);
        let words = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        assert!(is_codex(&words("/bin/codex exec")));
        assert!(!is_codex(&words("claude -p")));
        assert!(local_model(&words(
            "codex exec --oss --local-provider lmstudio"
        )));
        assert!(!local_model(&words("codex exec -m gpt-5.5")));
        assert_eq!(
            exec_args(&words("codex exec --oss --local-provider lmstudio"))[10..],
            words("-c model_provider=\"lmstudio\"")[..]
        );
        assert_eq!(
            exec_args(&words("codex exec --oss")),
            words("codex exec --oss --json --ephemeral --skip-git-repo-check --sandbox read-only")
        );
        assert_eq!(
            exec_args(&words("codex exec --json -s workspace-write")),
            words("codex exec --json -s workspace-write --ephemeral --skip-git-repo-check")
        );
    }

    #[test]
    fn exec_events_become_the_answer_object() {
        let stdout = [
            json!({"type": "thread.started", "thread_id": "t"}),
            json!({"type": "turn.started"}),
            json!({"type": "item.completed", "item": {"id": "i0", "type": "agent_message", "text": "Thinking aloud"}}),
            json!({"type": "item.completed", "item": {"id": "i1", "type": "mcp_tool_call", "server": "s", "tool": "t", "status": "completed"}}),
            json!({"type": "item.completed", "item": {"id": "i2", "type": "agent_message", "text": "{\"headline\": \"h\", \"summary\": \"s\"}"}}),
            json!({"type": "turn.completed", "usage": {"input_tokens": 100, "cached_input_tokens": 80, "output_tokens": 20, "reasoning_output_tokens": 5}}),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let value = normalize_exec_output(&stdout, false);
        assert_eq!(value["is_error"], false);
        assert_eq!(value["structured_output"]["headline"], "h");
        assert_eq!(value["usage"]["input_tokens"], 100);
        assert_eq!(value["usage"]["output_tokens"], 25);
        assert_eq!(value["num_turns"], 3);
        assert!(value.get("total_cost_usd").is_none(), "no cost reported");
        assert_eq!(normalize_exec_output(&stdout, true)["total_cost_usd"], 0.0);
        let failed = normalize_exec_output(
            &json!({"type": "turn.failed", "error": {"message": "LM Studio is not responding"}})
                .to_string(),
            true,
        );
        assert_eq!(failed["is_error"], true);
        assert_eq!(failed["result"], "LM Studio is not responding");
        assert_eq!(normalize_exec_output("", false)["is_error"], true);
    }

    #[test]
    fn the_sandbox_copies_the_model_settings_only() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("codex-sandbox");
        let user = tmp.path().join("config.toml");
        std::fs::write(
            &user,
            "model = \"openai/gpt-oss-20b\"\nmodel_provider = \"lm\"\nnotify = [\"x\"]\n\n\
             [model_providers.lm]\nname = \"LM Studio\"\nbase_url = \"http://localhost:1234/v1\"\n\n\
             [mcp_servers.other]\ncommand = \"x\"\n",
        )
        .unwrap();
        let config = sandbox_config(Some(&user), "[mcp_servers.m]\ncommand = \"/m\"\n");
        let doc = crate::install::tomledit::parse(&config).unwrap();
        assert_eq!(doc["model"], "openai/gpt-oss-20b");
        assert_eq!(
            doc["model_providers"]["lm"]["base_url"],
            "http://localhost:1234/v1"
        );
        assert_eq!(doc["approval_policy"], "never");
        assert_eq!(doc["features"]["hooks"], false);
        assert_eq!(doc["features"]["memories"], false);
        assert_eq!(doc["features"]["shell_tool"], false);
        assert_eq!(doc["web_search"], "disabled");
        assert_eq!(doc["mcp_servers"]["m"]["command"], "/m");
        assert!(doc["mcp_servers"]["other"].is_null(), "{config}");
        assert!(doc["notify"].is_null(), "{config}");
        let auth = tmp.path().join("auth.json");
        std::fs::write(&auth, "{}").unwrap();
        let env = build_sandbox(
            &tmp.path().join("s"),
            &config,
            Some("# rules\n"),
            Some(&auth),
        )
        .unwrap();
        let home = tmp.path().join("s").join("home");
        assert_eq!(
            env[0],
            ("CODEX_HOME".to_string(), home.display().to_string())
        );
        assert!(home.join("auth.json").is_file());
        assert_eq!(
            std::fs::read_to_string(home.join("AGENTS.md")).unwrap(),
            "# rules\n"
        );
        assert_eq!(
            std::fs::read_to_string(home.join("config.toml")).unwrap(),
            config
        );
    }
}
