//! DeepSeek Harness (`dsh`, github.com/deepseek-ai/deepseek-harness, the official DeepSeek agent
//! on Cordis; not CodeWhale, which has a module of its own and whose `.deepseek` directory and
//! `DEEPSEEK_*` variables are never read here). Verified against the source of tag
//! `dsh-v0.2.2-rc.3` (commit 38a6a5c); each fact names its file. dsh is a developer preview that
//! announces compatibility-breaking changes (`README.md:11-13`), and it was not run by any test:
//! this harness is a preview.
//!
//! - **Home**: `$DSH_HOME` (empty or whitespace-only counts as unset; a leading `~`, `~/` or
//!   `~\` expanded), else `<home>/.dsh`, the same on every OS, no XDG
//!   (`packages/util/home-paths/src/index.ts:11-27, 70-72, 87-110, 135-137`). The home directory
//!   is `crate::runtime::store::home_dir`'s (Node's `os.homedir()`). A relative `DSH_HOME` is not
//!   followed here (dsh would resolve it against its working directory, which install does not
//!   know). The second root `$DSH_AGENTS_HOME|~/.agents` is never written (CodeWhale reads
//!   `~/.agents/AGENTS.md` too).
//! - **Layers** (later wins): the bundle's layers, the profile's `profiles/<name>/
//!   cordis.patch.yml` (dsh's Settings rewrite it), the home layer `$DSH_HOME/cordis.patch.yml`
//!   (nothing in dsh writes it), then `--patch` overlays
//!   (`packages/boot/app-boot/src/profile-context.ts:74-85`). Every profile (`acp`, `web`,
//!   `headless`, `sdk`, `sdk-minimal`, Desktop) boots through the same path and applies the home
//!   layer (`apps/cli/src/profile-boot.ts:282-336`). `profiles/<name>/cordis.yml` is rewritten on
//!   every boot (`apps/cli/src/profile-boot.ts:81-88, 194-199, 231`): install writes neither it
//!   nor a profile layer, only the home layer.
//! - **Patch files** (`vendor/include/src/patch.ts:5-16, 62-106`, `vendor/include/src/index.ts:8,
//!   16-25`): a top-level YAML sequence of patch operations, read by js-yaml 4 with the JSON
//!   schema plus `!!js`. A new row must be inside `- insert: [rows]` (a bare `- id: x` overrides
//!   an existing row, and is skipped with a warning when there is none); a row is switched off
//!   by `disabled: true` beside its `id` (`vendor/loader/src/config/entry.ts:19-20, 76-95`). A
//!   missing file is no layer; an empty or comments-only file fails every boot
//!   (`packages/boot/app-boot/src/index.ts:591-608`), so uninstall never leaves one (dsh's own
//!   empty template is a flow `[]` under a comment header,
//!   `packages/boot/app-boot/src/profile.ts:174-178`). The edits are `crate::install::yamledit`.
//! - **MCP servers**: one `@deepseek-ai/dsh-mcp-client` row per server, shipped with the CLI
//!   (`packages/mcp/mcp-client/src/index.ts:1-14, 161-190`; example
//!   `packages/preset/agent-preset/skills/dsh-doc/templates/mcp/cordis.patch.yml`): `config`
//!   `{serverName (/^[A-Za-z0-9_-]{1,32}$/), transport: stdio, command, args, env, cwd,
//!   startupTimeoutMs, ...}`. Without `startupTimeoutMs` dsh's start waits for the server
//!   forever (`index.ts:254-262`), so ours sets [`STARTUP_TIMEOUT_MS`]. The child's environment
//!   is dsh's minus `/KEY|PASSWORD|SECRET|TOKEN/i` and `DSH_*`, then the row's `env` merged
//!   (`packages/subprocess/subprocess/src/index.ts:54, 73-76`,
//!   `packages/mcp/mcp-client/src/transport.ts:21-23, 34-39`): the row's `env` names the harness
//!   (`crate::runtime::shim::HARNESS_ENV`). A second row with the same `serverName` throws
//!   "already in use" (`index.ts:212-224`), also against a server of the web and Desktop
//!   profiles' `mcp-manager` row (`packages/bundle/web-app/cordis.patch.yml:148-154`): `doctor`
//!   names one. The model sees a tool as `mcp__<serverName>__<tool>`, every tool loaded
//!   (`packages/mcp/mcp-client/src/tools.ts:75-97, 139-177`), so ours are
//!   `mcp__graphersal-memory__<function>` ([`tool_names`]). Install writes one managed block in
//!   the home layer holding one `- insert:` item with both rows; the develop row has `disabled:
//!   true` until `--enable-develop`. There is no `dsh mcp add` (`apps/cli/src/args.ts:187-202`).
//! - **Server instructions**: `### MCP server: <serverName>\n\n` + the instructions (trimmed at
//!   the end) join the system prompt; above `maxInstructionBytes` (UTF-8, default 32768) the
//!   connection is rejected, never cut (`packages/mcp/mcp-client/src/connection.ts:54-55,
//!   316-319`, `index.ts:161-190`). The start context is therefore the short one
//!   ([`start_context`]): at most `CLAUDE_CODE_INSTRUCTIONS_CHARS` characters of instructions,
//!   far below [`MAX_INSTRUCTION_BYTES`] whatever the characters (a test proves the bound).
//! - **Rules**: the first request carries `$DSH_HOME/AGENTS.md`, then `~/.agents/AGENTS.md`, then
//!   the project's `AGENTS.md`/`CLAUDE.md` chain, 64 KiB per chain, the harness-home file dropped
//!   first when the user chain is over (`packages/context/agent-instructions/README.md`,
//!   `packages/bundle/base/cordis.patch.yml:307-310`). The managed rule block goes into
//!   `$DSH_HOME/AGENTS.md`. The `sdk-minimal` profile loads no instruction files
//!   (`packages/bundle/sdk-minimal/cordis.patch.yml`): there only the MCP server's instructions
//!   reach the model.
//! - **Sessions root**: `$DSH_HOME/sessions` (`packages/bundle/base/cordis.patch.yml:138-141`),
//!   read by [`sessions`].
//! - **Layout**: `<root>/<projectKey(cwd)>/<encodeSegment(id)>/session.v4.jsonl.zstd`, or
//!   `session.v4.jsonl` uncompressed (the `sdk-minimal` profile, `compression: none`, same root)
//!   (`packages/session/session-persistence-jsonl/src/format.ts:199-304`,
//!   `packages/session/session-format/src/filename.ts:14-17`); a session without a working
//!   directory sits under `_no-cwd/`. The project key is lossy (runs of `/ \ :` become `-`,
//!   other characters `~XXXX`, cut to 251, `--<slug>--`): it is never decoded, the project is
//!   the header's `cwd`. Older generations (`session.jsonl`, `session.v1.jsonl` ..
//!   `session.v3.jsonl[.zstd]`) may sit beside the v4 file and are not read.
//! - **Format**: JSON Lines; line 1 the header `{"type": "session", "version": 4, "id":
//!   "session-<uuid>", "createdAt": <ms>, "cwd"?, "parentSession"?, "isSeeded", "origin"?:
//!   "subagent", "delegationDepth", "agentPreset"?}`, then one event per line `{type, seq, time
//!   (ms), data, surfaceOp?, sourceEventSeqs?, ignorable?}`: `turn/start`/`turn/end` (`turn`),
//!   `user/message` (no turn; `source.kind` `user` for the human, other kinds injected),
//!   `assistant/message` (`turn`, `message.content` parts), `tool/call` (`turn`, `name`,
//!   `arguments` as JSON text; MCP tools as `mcp__<server>__<tool>`), `tool/result`. Version 4
//!   may gain event types without a version bump. There is no session end event.
//! - **Compression**: the `.zstd` file is a concatenation of independent standard zstd frames
//!   (one with the header line, one per append batch), each with a content checksum, no
//!   dictionary, no skippable frames; the last frame can be torn by a crash or a concurrent
//!   write.
//! - **Session start and end**: no hooks (the hook bridges are experimental packages no bundle
//!   mounts, without a session end and with an empty `transcript_path`,
//!   `packages/experimental/hooks-claude-code/README.md:93, 101, 175-176, 182`). dsh keeps one MCP
//!   child for its whole run, so the connection does the work: the shim's start queues the
//!   indexing (`crate::memory::session_sync`), and the daemon ends the sessions written during a
//!   `use` connection when it closes (`SessionSource::ended_by_connection`). The child runs in
//!   dsh's launch directory and serves every session of that dsh process (no MCP roots,
//!   `connection.ts:283`; `transport.ts:38`): in `web`, Desktop and `acp` the project is the
//!   launch directory's, right in `headless` and in a dsh started from the project (a known limit
//!   of the preview).
//! - **Memory**: none of its own (no memory package; the "memory" rows are SQLite `:memory:`
//!   indexes, `packages/bundle/base/cordis.patch.yml:160`). The export of `uninstall` writes
//!   readable Markdown files into `~/graphersal-memory-export/dsh/`.
//! - **Headless runs**: `dsh --profile headless --json -` (`packages/bundle/headless/src/
//!   startup.ts:53-75`) has no way to run without tools (no flag; the tool rows change per
//!   version, `docs/upgrade-guide/`), no cost and saves every run as a session: the summaries
//!   and the bench refuse it ([`NO_HEADLESS`]).
//! - **Privacy**: the `session-log-deepseek` row is on by default in every bundle and sends the
//!   session log (MCP tool results, so memories, included) with official DeepSeek API requests
//!   (`packages/session/session-log-deepseek/src/index.ts:59-61`, its README "Request field").
//!   Install never changes it; `status` and `doctor` say when no layer switches it off
//!   (`- id: session-log-deepseek` with `config: {enabled: false}` or `disabled: true`).
//! - **Program**: npm `@deepseek-ai/dsh`, bin `dsh` (`apps/cli/package.json:14-16`; on Windows
//!   npm's `dsh.cmd`); Desktop links `/usr/local/bin/dsh` on macOS and puts its `dsh.cmd`
//!   directory on the Windows user `PATH` (`apps/desktop/src/command-manager-entry.ts:17-31`).
//!   `npx @deepseek-ai/dsh` leaves no program: such a configuration is a leftover, and `doctor`
//!   says how to install it globally.

pub mod install;
pub mod sessions;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::harnesses::agent::{Harness, Kind};

/// The patch file of the home layer (and of each profile).
pub const PATCH_FILE: &str = "cordis.patch.yml";

/// The plugin of an MCP server row.
pub const MCP_CLIENT: &str = "@deepseek-ai/dsh-mcp-client";

/// How long dsh waits for our server's start (milliseconds; unset means forever, and the first
/// connection may start the daemon and open the store).
pub const STARTUP_TIMEOUT_MS: u64 = 60_000;

/// dsh rejects a server whose attribution plus instructions are longer than this many UTF-8
/// bytes (`maxInstructionBytes`' default, `packages/mcp/mcp-client/src/index.ts:161-190`;
/// checked in `packages/mcp/mcp-client/src/connection.ts:316-319`). [`start_context`] keeps
/// ours far below it (a test proves the bound).
#[cfg(test)]
pub const MAX_INSTRUCTION_BYTES: usize = 32768;

/// The row of dsh's session log upload.
pub const SESSION_LOG_ROW: &str = "session-log-deepseek";

/// The row of the web and Desktop profiles' MCP server manager.
pub const MCP_MANAGER_ROW: &str = "mcp-manager";

/// The profile that loads no instruction files.
pub const MINIMAL_PROFILE: &str = "sdk-minimal";

/// The npm package of the `dsh` program.
pub const NPM_PACKAGE: &str = "@deepseek-ai/dsh";

/// Why the summaries and the bench do not run through dsh.
pub const NO_HEADLESS: &str = "DeepSeek Harness (`dsh --profile headless`) is not used for \
    summaries or the bench: it has no way to run without tools (no flag, and its tool rows \
    change from version to version), reports no cost, so the monthly budget could not be kept, \
    and saves every run as a session. Use Claude Code's `claude -p`, opencode's `opencode run`, \
    Codex's `codex exec` or CodeWhale's `codewhale exec` (summaries.harness_command in \
    config.json)";

/// The text dsh puts before a server's instructions (counted in its limit).
#[cfg(test)]
pub fn attribution(server: &str) -> String {
    format!("### MCP server: {server}\n\n")
}

/// The tool names dsh shows the model for our `use` server's functions (the rule block names
/// them).
#[cfg(test)]
pub fn tool_names() -> Vec<String> {
    crate::runtime::catalog::FUNCTIONS
        .iter()
        .map(|function| format!("mcp__{}__{function}", crate::install::USE_SERVER))
        .collect()
}

/// Where dsh keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DshPaths {
    /// The home directory.
    pub home: PathBuf,
    /// The dsh home (`$DSH_HOME` or `~/.dsh`).
    pub dsh_home: PathBuf,
    /// Whether `DSH_HOME` set it.
    pub explicit_home: bool,
    /// The home layer's patch file, where install writes the MCP rows.
    pub patch_file: PathBuf,
    /// The global instruction file of dsh's home (the rule block goes there).
    pub instruction_file: PathBuf,
    /// The sessions.
    pub sessions: PathBuf,
    /// The profiles (`<name>/cordis.patch.yml` each): read by `status` and `doctor` only.
    pub profiles: PathBuf,
    /// dsh's `.env` file (never read).
    pub env_file: PathBuf,
    /// Where `uninstall` exports the memories as Markdown.
    pub export_dir: PathBuf,
}

/// `~`, `~/x` or `~\x` expanded against `home`; other paths as they are.
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

impl DshPaths {
    /// Resolves the paths from the environment (`env` reads one variable): the home directory
    /// ([`crate::runtime::store::home_dir`]) and `DSH_HOME`.
    pub fn resolve(env: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let home = crate::runtime::store::home_dir(env)?;
        let explicit = env("DSH_HOME")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(|value| expand(&value, &home))
            .filter(|dir| dir.is_absolute());
        Ok(Self::with(home, explicit))
    }

    /// The paths of a home directory without environment overrides (tests).
    #[cfg(test)]
    pub fn of_home(home: &Path) -> Self {
        Self::with(home.to_path_buf(), None)
    }

    fn with(home: PathBuf, explicit: Option<PathBuf>) -> Self {
        let explicit_home = explicit.is_some();
        let dsh_home = explicit.unwrap_or_else(|| home.join(".dsh"));
        Self {
            patch_file: dsh_home.join(PATCH_FILE),
            instruction_file: dsh_home.join("AGENTS.md"),
            sessions: dsh_home.join("sessions"),
            profiles: dsh_home.join("profiles"),
            env_file: dsh_home.join(".env"),
            export_dir: home.join("graphersal-memory-export").join("dsh"),
            explicit_home,
            dsh_home,
            home,
        }
    }

    /// The profiles' patch files that exist, by profile name.
    pub fn profile_patches(&self) -> Vec<(String, PathBuf)> {
        let Ok(dirs) = std::fs::read_dir(&self.profiles) else {
            return Vec::new();
        };
        let mut out: Vec<(String, PathBuf)> = dirs
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|dir| dir.is_dir())
            .filter_map(|dir| {
                let file = dir.join(PATCH_FILE);
                let name = dir.file_name()?.to_string_lossy().to_string();
                file.is_file().then_some((name, file))
            })
            .collect();
        out.sort();
        out
    }
}

/// The `dsh` program on `search_path` (`None`: this process's `PATH`), else, for this process's
/// own `PATH` only (tests pass their search path), where its installers put it off `PATH`:
/// npm's default global directory on Windows (`%APPDATA%\npm`, under the home directory) and
/// the Desktop app's link `/usr/local/bin/dsh` on macOS.
pub fn find_program(home: &Path, search_path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let path = match search_path {
        Some(path) => Some(path.to_os_string()),
        None => std::env::var_os("PATH"),
    };
    let mut dirs: Vec<PathBuf> = path.iter().flat_map(std::env::split_paths).collect();
    if search_path.is_none() {
        if cfg!(windows) {
            dirs.push(home.join("AppData").join("Roaming").join("npm"));
        } else {
            dirs.push(Path::new("/usr").join("local").join("bin"));
        }
    }
    let names = ["dsh", "dsh.cmd", "dsh.exe"];
    dirs.iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

/// Whether `command` runs dsh.
pub fn is_dsh(command: &[String]) -> bool {
    command
        .first()
        .is_some_and(|program| Path::new(program).file_stem().is_some_and(|s| s == "dsh"))
}

/// DeepSeek Harness, as the installation sees it.
#[derive(Debug, Clone)]
pub struct Dsh {
    paths: DshPaths,
}

impl Dsh {
    /// dsh with its files at `paths`.
    pub fn new(paths: DshPaths) -> Self {
        Self { paths }
    }

    /// Its files.
    pub fn paths(&self) -> &DshPaths {
        &self.paths
    }
}

impl Harness for Dsh {
    fn kind(&self) -> Kind {
        Kind::Dsh
    }

    fn find_program(&self, home: &Path, search: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
        find_program(home, search)
    }

    fn config_roots(&self) -> Vec<PathBuf> {
        vec![self.paths.dsh_home.clone()]
    }

    fn has_our_parts(&self, setup: &crate::install::Setup) -> bool {
        !install::inspect(setup).nothing_installed()
    }

    fn plan(
        &self,
        setup: &crate::install::Setup,
        input: &crate::install::PlanInput<'_>,
    ) -> Result<crate::install::HarnessPlan, String> {
        let plan = install::plan(setup, input.language, input.read_only, input.develop)?;
        Ok(crate::install::HarnessPlan::of(
            plan.steps,
            plan.changes,
            plan.conflicts,
        ))
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

    fn develop_on(&self, setup: &crate::install::Setup) -> bool {
        let (state, enabled) = install::inspect(setup).develop_row;
        state != crate::install::PartState::Missing && enabled
    }

    fn export_hint(&self) -> &'static str {
        "to give the agent a file, paste it into ~/.dsh/AGENTS.md or a project's AGENTS.md"
    }

    #[cfg(test)]
    fn user_files(&self, home: &Path) {
        fixture::user_files(home);
    }

    fn has_sessions(&self) -> bool {
        self.paths.sessions.is_dir()
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

    fn mcp_config_file(&self) -> &Path {
        &self.paths.patch_file
    }

    /// The patch operation the rows sit in.
    fn mcp_servers_key(&self) -> &'static str {
        "insert"
    }

    /// A row's `config` (`packages/mcp/mcp-client/src/index.ts:161-190`) without its
    /// `serverName`: `env` names the harness (dsh sets no variable of its own that would tell
    /// the shim), so the start context is the short one.
    fn mcp_entry(&self, exe: &Path, args: &[String]) -> Value {
        json!({"transport": "stdio", "command": exe.display().to_string(), "args": args,
               "env": {crate::runtime::shim::HARNESS_ENV: Kind::Dsh.id()},
               "startupTimeoutMs": STARTUP_TIMEOUT_MS})
    }

    fn remove_server_hint(&self, name: &str) -> String {
        format!(
            "the row `id: {name}` in an `- insert:` item of {} (or `disabled: true` on it)",
            self.paths.patch_file.display()
        )
    }

    /// The home layer's patch file (dsh has no other settings file install changes).
    fn settings_file(&self) -> &Path {
        &self.paths.patch_file
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

    /// `~/graphersal-memory-export/dsh/<encoded root>/`.
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

/// dsh's start context: the short one. dsh rejects a server whose attribution and instructions
/// are over [`MAX_INSTRUCTION_BYTES`] UTF-8 bytes; `Short` keeps the whole instructions within
/// `CLAUDE_CODE_INSTRUCTIONS_CHARS` characters (at most four bytes each), while `Full` follows
/// the user's `memory.start_context_chars`, which nothing bounds.
pub fn start_context(_hook: bool) -> crate::runtime::protocol::StartContextMode {
    crate::runtime::protocol::StartContextMode::Short
}

/// dsh's headless command (`crate::harnesses::headless::Headless`): recognised only to be
/// refused.
pub struct HeadlessRun;

impl crate::harnesses::headless::Headless for HeadlessRun {
    fn matches(&self, command: &[String]) -> bool {
        is_dsh(command)
    }

    fn summaries_refusal(&self) -> Option<&'static str> {
        Some(NO_HEADLESS)
    }

    fn wizard_summaries_note(&self) -> Option<&'static [&'static str]> {
        Some(&[
            "DeepSeek Harness is not used for summaries (it cannot run without tools); say yes",
            "only if the command above is installed and logged in.",
        ])
    }

    fn bench_refusal(&self, _command: &[String]) -> Option<(&'static str, &'static str)> {
        Some((
            "the bench does not run through DeepSeek Harness.",
            "dsh has no way to run without tools, reports no cost, so the budget could not \
             stop the run, and saves every run as a session.",
        ))
    }
}

/// What the conformance tests (`crate::harnesses::conformance_tests`) and the install tests
/// start from: a home with dsh's files as a user has them.
#[cfg(test)]
pub(crate) mod fixture {
    use std::path::Path;

    /// dsh's own empty home patch template (`packages/boot/app-boot/src/profile.ts:174-178`): a
    /// comment header and a flow `[]`.
    pub(crate) const HOME_PATCH: &str = "# DeepSeek Harness home patch.\n# Entries here apply to every profile and override profile settings.\n[]\n";

    /// A home patch with the user's own overrides, an insert of theirs and a `!!js` value.
    pub(crate) const USER_PATCH: &str = "# my overrides\n- id: agent-default-model\n  config:\n    provider: deepseek-official\n    model: deepseek-pro\n- insert:\n    - id: git-mcp\n      name: '@deepseek-ai/dsh-mcp-client'\n      config:\n        serverName: git\n        transport: stdio\n        command: mcp-git\n- id: hmr\n  disabled: !!js \"process.env.CI === 'true'\"\n";

    /// The web profile's layer as dsh's Settings write it: a model and an MCP server of the
    /// manager.
    pub(crate) const WEB_PATCH: &str = "- id: agent-default-model\n  config:\n    provider: deepseek-official\n    model: deepseek-flash\n- id: mcp-manager\n  config:\n    servers:\n      github:\n        transport: stdio\n        command: npx\n        args: [-y, github-mcp]\n";

    pub(crate) fn user_files(home: &Path) {
        let dir = home.join(".dsh");
        let web = dir.join("profiles").join("web");
        std::fs::create_dir_all(&web).unwrap();
        std::fs::write(dir.join("cordis.patch.yml"), HOME_PATCH).unwrap();
        std::fs::write(web.join("cordis.patch.yml"), WEB_PATCH).unwrap();
        std::fs::write(dir.join("AGENTS.md"), "# My rules\n\nTabs, not spaces.\n").unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::mcp_tests::{TempDir, abs};

    /// A Unix-style fixture path, absolute on this platform.
    fn pb(unix: &str) -> PathBuf {
        PathBuf::from(abs(unix))
    }

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
    fn paths_follow_dsh_home() {
        let home = pb("/h");
        let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let env = env_of(&[(var, home.display().to_string())]);
        let paths = DshPaths::resolve(&env).unwrap();
        let dsh = home.join(".dsh");
        assert_eq!(paths, DshPaths::of_home(&home));
        assert!(!paths.explicit_home);
        assert_eq!(paths.patch_file, dsh.join("cordis.patch.yml"));
        assert_eq!(paths.instruction_file, dsh.join("AGENTS.md"));
        assert_eq!(paths.sessions, dsh.join("sessions"));
        assert_eq!(paths.profiles, dsh.join("profiles"));
        assert_eq!(paths.env_file, dsh.join(".env"));
        assert_eq!(
            paths.export_dir,
            home.join("graphersal-memory-export").join("dsh")
        );
        // An empty or blank DSH_HOME is unset; a relative one is not followed.
        for value in ["", "   ", "relative/dir"] {
            let env = env_of(&[
                (var, home.display().to_string()),
                ("DSH_HOME", value.to_string()),
            ]);
            assert_eq!(DshPaths::resolve(&env).unwrap().dsh_home, dsh, "{value:?}");
        }
        // DSH_HOME moves every file, `~` expanded.
        let own = pb("/elsewhere/dsh");
        for (value, want) in [
            (own.display().to_string(), own.clone()),
            ("~/x/y".to_string(), home.join("x").join("y")),
            ("~\\z".to_string(), home.join("z")),
            ("~".to_string(), home.clone()),
        ] {
            let env = env_of(&[(var, home.display().to_string()), ("DSH_HOME", value)]);
            let paths = DshPaths::resolve(&env).unwrap();
            assert!(paths.explicit_home);
            assert_eq!(paths.dsh_home, want);
            assert_eq!(paths.patch_file, want.join("cordis.patch.yml"));
            assert_eq!(paths.instruction_file, want.join("AGENTS.md"));
            assert_eq!(paths.sessions, want.join("sessions"));
            // The export stays under the home directory.
            assert_eq!(
                paths.export_dir,
                home.join("graphersal-memory-export").join("dsh")
            );
        }
        assert!(DshPaths::resolve(&|_| None).is_err());
    }

    #[test]
    fn profile_patches_are_listed_by_name() {
        let tmp = TempDir::new("dsh-profiles");
        fixture::user_files(tmp.path());
        let paths = DshPaths::of_home(tmp.path());
        std::fs::create_dir_all(paths.profiles.join("empty")).unwrap();
        assert_eq!(
            paths.profile_patches(),
            vec![(
                "web".to_string(),
                paths.profiles.join("web").join("cordis.patch.yml")
            )]
        );
    }

    #[test]
    fn the_short_start_context_always_fits_dshs_instruction_limit() {
        use crate::runtime::protocol::{CLAUDE_CODE_INSTRUCTIONS_CHARS, StartContextMode};
        // The attribution dsh counts: 35 bytes for our server.
        let attribution = attribution(crate::install::USE_SERVER);
        assert_eq!(attribution.len(), 35);
        assert_eq!(start_context(false), StartContextMode::Short);
        assert_eq!(start_context(true), StartContextMode::Short);
        assert_eq!(
            crate::commands::start_context_mode("dsh", false),
            StartContextMode::Short
        );
        // `Short` keeps the whole instructions within CLAUDE_CODE_INSTRUCTIONS_CHARS characters
        // (the `use` instructions leave room for some start context), each at most 4 UTF-8
        // bytes: the worst case is far below dsh's limit.
        for language in [
            crate::install::language::Language::English,
            crate::install::language::Language::Any,
        ] {
            let own = crate::runtime::mcp::use_instructions(language)
                .chars()
                .count();
            assert!(own + 2 < CLAUDE_CODE_INSTRUCTIONS_CHARS, "{own}");
        }
        let worst = CLAUDE_CODE_INSTRUCTIONS_CHARS * 4 + attribution.len();
        assert!(worst < MAX_INSTRUCTION_BYTES, "{worst}");
        // About half of it: a margin for a larger Claude Code limit before this must change.
        assert!(worst < MAX_INSTRUCTION_BYTES * 2 / 3, "{worst}");
    }

    #[test]
    fn the_agent_names_its_files_and_its_entry() {
        let home = pb("/h");
        let agent = Dsh::new(DshPaths::of_home(&home));
        assert_eq!(agent.id(), "dsh");
        assert_eq!(agent.display_name(), "DeepSeek Harness");
        assert_eq!(
            agent.mcp_config_file(),
            home.join(".dsh").join("cordis.patch.yml")
        );
        assert_eq!(agent.settings_file(), agent.mcp_config_file());
        assert_eq!(
            agent.instruction_file(),
            home.join(".dsh").join("AGENTS.md")
        );
        assert!(!agent.has_file_memory());
        assert!(agent.hook().is_none());
        assert!(agent.own_memory().is_none());
        let entry = agent.mcp_entry(&pb("/x/graphersal-memory"), &["mcp".into()]);
        assert!(crate::harnesses::agent::is_our_entry(&entry));
        assert_eq!(entry["startupTimeoutMs"], 60_000);
        assert_eq!(
            crate::runtime::shim::harness_name(&|name| entry["env"][name]
                .as_str()
                .map(str::to_string)),
            "dsh"
        );
        let names = tool_names();
        assert!(names.contains(&"mcp__graphersal-memory__remember".to_string()));
        for name in &names {
            assert!(name.len() <= 64, "dsh would rename {name}");
        }
        assert!(
            crate::memory::transcript::memory_tool("mcp__graphersal-memory__remember").is_some()
        );
        assert_eq!(
            crate::harnesses::driver(Kind::Dsh)
                .headless
                .and_then(|headless| headless.default_command()),
            None
        );
        assert!(crate::harnesses::driver(Kind::Dsh).native_slugs.is_none());
    }

    #[test]
    fn summaries_and_bench_refuse_dsh_before_running_anything() {
        let words = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        assert!(is_dsh(&words("dsh --profile headless --json -")));
        assert!(is_dsh(&words("/usr/local/bin/dsh")));
        assert!(!is_dsh(&words("codewhale exec")));
        let command = words("dsh --profile headless");
        let err = crate::bench::check_harness(&command).unwrap_err();
        assert!(err.contains("DeepSeek Harness"), "{err}");
        let tmp = TempDir::new("dsh-summary");
        let err = crate::memory::summaries::run_harness_with(
            &command,
            "prompt",
            tmp.path(),
            std::time::Duration::from_secs(1),
            None,
        )
        .unwrap_err();
        assert_eq!(err, NO_HEADLESS);
    }

    #[test]
    fn programs_are_found_on_the_search_path() {
        let tmp = TempDir::new("dsh-program");
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        assert_eq!(find_program(tmp.path(), Some(bin.as_os_str())), None);
        std::fs::write(bin.join("dsh.cmd"), "").unwrap();
        assert_eq!(
            find_program(tmp.path(), Some(bin.as_os_str())),
            Some(bin.join("dsh.cmd"))
        );
        std::fs::write(bin.join("dsh"), "").unwrap();
        assert_eq!(
            find_program(tmp.path(), Some(bin.as_os_str())),
            Some(bin.join("dsh"))
        );
    }

    #[test]
    fn the_end_of_a_connection_ends_sessions_under_dsh_home() {
        use crate::memory::sessions::SessionSource;
        let home = pb("/h");
        let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let env = env_of(&[(var, home.display().to_string())]);
        assert_eq!(
            sessions::Sessions.ended_by_connection(&env),
            Some(home.join(".dsh").join("sessions"))
        );
        let own = pb("/d");
        let env = env_of(&[
            (var, home.display().to_string()),
            ("DSH_HOME", own.display().to_string()),
        ]);
        assert_eq!(
            sessions::Sessions.ended_by_connection(&env),
            Some(own.join("sessions"))
        );
    }
}
