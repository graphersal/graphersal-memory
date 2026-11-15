//! Cursor (the editor and its agent), verified against Cursor's documentation as of 2026-11-10
//! (cursor.com/docs; each fact names its page). Cursor is not run by any test: everything here
//! is checked against fixtures in the documented formats, and `docs/cursor-live-check.md` is the
//! checklist of a live run.
//!
//! - **Home**: `~/.cursor` (`%USERPROFILE%\.cursor` on Windows). No environment variable moves
//!   it (`CURSOR_CONFIG_DIR` moves only the CLI's `cli-config.json`,
//!   cursor.com/docs/cli/reference/configuration).
//! - **MCP servers** (cursor.com/docs/mcp, "Configuration locations"): the global file
//!   `~/.cursor/mcp.json`, `{"mcpServers": {"<name>": {"type": "stdio", "command", "args",
//!   "env"}}}`. The documented file has no switched-off state: a server is toggled off in the
//!   app (Customize) or with `agent mcp disable`, which keep their own list. So, as for Claude
//!   Code, the develop server is not registered until `install --enable-develop`. Cursor asks
//!   before each MCP tool call by default ("Tool approval"); install never writes an allowlist
//!   (the approval stays the user's). Global servers load without a per-workspace prompt (CLI
//!   changelog, "Global MCP servers auto-approved"). The file is edited as text
//!   (`crate::jsonedit`, comments kept).
//! - **Rules** (cursor.com/help/customization/rules, "Where are rules stored?"): "User rule
//!   files in `~/.cursor/rules` ... stay on the machine and do not sync"; project rules are
//!   `.mdc` files with `description`/`globs`/`alwaysApply` front matter (cursor.com/docs/rules).
//!   The User Rules of Customize live in the account, not in a file, and are never touched.
//!   install writes its own file `~/.cursor/rules/graphersal-memory.mdc` (`alwaysApply: true`,
//!   the managed rule block); no file of the user is edited. The MCP server's instructions say
//!   the same (whether Cursor shows them to the model is not documented). Caveat: a forum report
//!   of 2025-12 (Cursor 2.2.43) says files in `~/.cursor/rules` were not applied; the live check
//!   asks for it.
//! - **Session start and end** (cursor.com/docs/hooks): `~/.cursor/hooks.json`, `{"version": 1,
//!   "hooks": {"sessionStart": [{"command", "timeout"}], "sessionEnd": [...]}}`; user hooks run
//!   from `~/.cursor/` and need no approval (only project hooks need a trusted workspace);
//!   Cursor reloads the file on save. Every agent hook gets `conversation_id`,
//!   `workspace_roots` and `transcript_path` (null when transcripts are off); `sessionStart`
//!   also `session_id` and may answer `{"additional_context": ...}`; both are fire-and-forget.
//!   Cursor also runs Claude Code's hooks of `~/.claude/settings.json` ("Third Party Hooks", on
//!   by default): our Claude Code hook recognises Cursor's input (`cursor_version`) and leaves
//!   the work to these hooks (`crate::commands`).
//! - **Sessions**: Cursor keeps its chats in an internal database whose format is not
//!   documented; it is not read. The documented way in is the hooks' `transcript_path`, a JSON
//!   Lines file under `agent-transcripts/` (CLI changelog 2026-10-17: sub-agent transcripts
//!   under `agent-transcripts/<parent>/subagents/`). The hooks record each transcript with its
//!   workspace in `<data dir>/sessions/cursor-transcripts.json` ([`sessions`]); only those are
//!   indexed. The line format of a transcript is not documented, so the reader takes what is
//!   recognisable (user and assistant text, tool names) and skips the rest; a transcript it
//!   cannot read stays unindexed and is counted as empty.
//! - **Headless runs**: the CLI `agent -p --output-format json` reports no cost
//!   (cursor.com/docs/cli/reference/output-format) and needs its own login or `CURSOR_API_KEY`;
//!   it reads the user's `~/.cursor` (MCP servers, hooks) with no documented way to isolate a
//!   run. The summaries and the bench therefore do not run through Cursor; they use another
//!   harness's command when one is installed.
//! - **Memory**: the documentation of 2026-11 describes no memory feature of Cursor's own
//!   (Cursor's earlier "Memories" lost its settings UI in 2.1 per forum answers of Cursor staff;
//!   nothing in a file switches it). The wizard has no step for it; `docs/harnesses.md` says how
//!   to export old ones. Cursor has no file memory to write back into: the export of
//!   `uninstall` writes readable Markdown files into `~/graphersal-memory-export/cursor/`.

pub mod install;
pub mod other;
pub mod sessions;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::agents::{Harness, Kind};

/// The MCP configuration file in the Cursor home.
pub const MCP_FILE: &str = "mcp.json";

/// The hooks file in the Cursor home.
pub const HOOKS_FILE: &str = "hooks.json";

/// Our user rule file in `~/.cursor/rules`.
pub const RULES_FILE: &str = "graphersal-memory.mdc";

/// How long Cursor lets a hook run (seconds; the hook itself stops after 15).
pub const HOOK_TIMEOUT_SECS: u64 = 30;

/// The registry of the transcripts the hooks reported, in the data directory's `sessions/`.
pub const REGISTRY_FILE: &str = "cursor-transcripts.json";

/// Where Cursor keeps its files, and where graphersal-memory records Cursor's transcripts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorPaths {
    /// The home directory.
    pub home: PathBuf,
    /// `~/.cursor`.
    pub cursor_dir: PathBuf,
    /// `~/.cursor/mcp.json`.
    pub mcp_file: PathBuf,
    /// `~/.cursor/hooks.json`.
    pub hooks_file: PathBuf,
    /// `~/.cursor/rules/graphersal-memory.mdc`.
    pub rules_file: PathBuf,
    /// `<data dir>/sessions/cursor-transcripts.json`.
    pub registry: PathBuf,
    /// Where `uninstall` exports the memories as Markdown.
    pub export_dir: PathBuf,
}

impl CursorPaths {
    /// Resolves the paths from the environment (`env` reads one variable: the home directory,
    /// [`crate::store::home_dir`]) and the data directory.
    pub fn resolve(env: &dyn Fn(&str) -> Option<String>, data_dir: &Path) -> Result<Self, String> {
        let home = crate::store::home_dir(env)?;
        Ok(Self::of_home(&home, data_dir))
    }

    /// The paths of a home directory and a data directory.
    pub fn of_home(home: &Path, data_dir: &Path) -> Self {
        let cursor_dir = home.join(".cursor");
        Self {
            mcp_file: cursor_dir.join(MCP_FILE),
            hooks_file: cursor_dir.join(HOOKS_FILE),
            rules_file: cursor_dir.join("rules").join(RULES_FILE),
            registry: data_dir.join("sessions").join(REGISTRY_FILE),
            export_dir: home.join("graphersal-memory-export").join("cursor"),
            cursor_dir,
            home: home.to_path_buf(),
        }
    }
}

/// Cursor's programs on `search_path` (`None`: this process's `PATH`): the editor's shell
/// command `cursor` and the CLI `cursor-agent` (its newer name `agent` is too common a name to
/// count as a sign of Cursor).
pub fn find_program(search_path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let path = match search_path {
        Some(path) => Some(path.to_os_string()),
        None => std::env::var_os("PATH"),
    };
    let names = [
        "cursor",
        "cursor.exe",
        "cursor.cmd",
        "cursor-agent",
        "cursor-agent.exe",
        "cursor-agent.cmd",
    ];
    path.iter()
        .flat_map(std::env::split_paths)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

/// Whether `command` runs Cursor's CLI (`agent`, `cursor-agent`) or the editor (`cursor`).
pub fn is_cursor_cli(command: &[String]) -> bool {
    command.first().is_some_and(|program| {
        Path::new(program)
            .file_stem()
            .is_some_and(|stem| stem == "agent" || stem == "cursor-agent" || stem == "cursor")
    })
}

/// Why the summaries and the bench do not run through Cursor's CLI.
pub const NO_HEADLESS: &str = "Cursor's CLI (`agent -p`) is not used for summaries or the \
    bench: it reports no cost, so the monthly budget could not be kept, and it reads your \
    ~/.cursor (MCP servers, hooks) with no way to isolate a run. Use Claude Code's `claude -p`, \
    opencode's `opencode run` or Codex's `codex exec` (summaries.harness_command in \
    config.json)";

/// Where Cursor's Windows installers put `Cursor.exe` when it is not on `PATH`: the per-user
/// install (`%LOCALAPPDATA%\Programs\cursor`, under the home directory) and, for this process's
/// own machine (`system`), the per-machine one (`%ProgramFiles%\cursor`) and `%LOCALAPPDATA%`
/// itself when it is elsewhere.
pub fn windows_locations(home: &Path, system: bool) -> Vec<PathBuf> {
    let exe = |base: PathBuf| base.join("Programs").join("cursor").join("Cursor.exe");
    let mut candidates = vec![exe(home.join("AppData").join("Local"))];
    if system && cfg!(windows) {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            candidates.push(exe(PathBuf::from(local)));
        }
        if let Some(programs) = std::env::var_os("ProgramFiles") {
            candidates.push(PathBuf::from(programs).join("cursor").join("Cursor.exe"));
        }
    }
    candidates
}

/// Cursor, as the installation sees it.
#[derive(Debug, Clone)]
pub struct Cursor {
    paths: CursorPaths,
}

impl Cursor {
    /// Cursor with its files at `paths`.
    pub fn new(paths: CursorPaths) -> Self {
        Self { paths }
    }
}

impl crate::agents::HookHandler for Cursor {
    /// The transcript recorded in the registry and queued; the start answer carries the start
    /// context of the workspace.
    fn run(
        &self,
        setup: &crate::install::Setup,
        event: crate::hook::HookEvent,
        input: Vec<u8>,
        stdout: &mut dyn std::io::Write,
        stderr: &mut dyn std::io::Write,
    ) -> u8 {
        crate::hook::run_cursor_with_context(
            event,
            self.paths.registry.clone(),
            std::env::var("CURSOR_PROJECT_DIR").ok(),
            Some(crate::commands::hook_enqueue(setup)),
            Some(crate::commands::start_fetch(setup, Kind::Cursor)),
            std::io::Cursor::new(input),
            stdout,
            stderr,
        )
    }
}

impl Harness for Cursor {
    fn kind(&self) -> Kind {
        Kind::Cursor
    }

    /// The CLI on the search path, else the app: `~/Applications/Cursor.app` (and
    /// `/Applications/Cursor.app` for this process's own `PATH`), or Windows' install
    /// locations.
    fn find_program(&self, home: &Path, search: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
        find_program(search)
            .or_else(|| {
                let mut bundles = vec![home.join("Applications").join("Cursor.app")];
                if search.is_none() {
                    bundles.push(PathBuf::from("/Applications/Cursor.app"));
                }
                bundles.into_iter().find(|bundle| bundle.is_dir())
            })
            .or_else(|| {
                windows_locations(home, search.is_none())
                    .into_iter()
                    .find(|p| p.is_file())
            })
    }

    fn config_roots(&self) -> Vec<PathBuf> {
        vec![self.paths.cursor_dir.clone()]
    }

    fn has_our_parts(&self, setup: &crate::install::Setup) -> bool {
        !install::inspect(setup).nothing_installed()
    }

    fn develop_on(&self, setup: &crate::install::Setup) -> bool {
        install::inspect(setup).develop_server != crate::install::PartState::Missing
    }

    fn hook(&self) -> Option<&dyn crate::agents::HookHandler> {
        Some(self)
    }

    fn export_hint(&self) -> &'static str {
        "to give the agent a file, paste it into a project's AGENTS.md or into a rule file \
         under ~/.cursor/rules (a .mdc file with `alwaysApply: true`)"
    }

    fn has_sessions(&self) -> bool {
        self.paths.registry.is_file()
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

    fn mcp_config_file(&self) -> &Path {
        &self.paths.mcp_file
    }

    fn mcp_servers_key(&self) -> &'static str {
        "mcpServers"
    }

    /// `{"type": "stdio", "command", "args"}` (cursor.com/docs/mcp, "STDIO server
    /// configuration": `type` is listed as required).
    fn mcp_entry(&self, exe: &Path, args: &[String]) -> Value {
        json!({"type": "stdio", "command": exe.display().to_string(), "args": args})
    }

    fn remove_server_hint(&self, name: &str) -> String {
        format!(
            "the member mcpServers.{name} of {}",
            self.paths.mcp_file.display()
        )
    }

    /// `hooks.json` (Cursor has no settings file of its own that install changes).
    fn settings_file(&self) -> &Path {
        &self.paths.hooks_file
    }

    fn instruction_file(&self) -> &Path {
        &self.paths.rules_file
    }

    /// The registry of the transcripts the hooks reported ([`sessions`]).
    fn projects_dir(&self) -> &Path {
        &self.paths.registry
    }

    fn has_file_memory(&self) -> bool {
        false
    }

    /// `~/graphersal-memory-export/cursor/<encoded root>/`.
    fn memory_dir(&self, root: &Path) -> PathBuf {
        self.paths
            .export_dir
            .join(crate::import::claude_slug(&root.display().to_string()))
    }

    fn global_memory_file(&self) -> PathBuf {
        self.paths.export_dir.join("global.md")
    }

    fn auto_memory_key(&self) -> Option<&'static str> {
        None
    }

    fn headless_command(&self) -> Option<&'static str> {
        None
    }
}

/// Cursor's CLI (`crate::harness::Headless`): recognised only to be refused.
pub struct HeadlessRun;

impl crate::harness::Headless for HeadlessRun {
    fn matches(&self, command: &[String]) -> bool {
        is_cursor_cli(command)
    }

    fn summaries_refusal(&self) -> Option<&'static str> {
        Some(NO_HEADLESS)
    }

    fn wizard_summaries_note(&self) -> Option<&'static [&'static str]> {
        Some(&[
            "Cursor's own CLI is not used for summaries (it reports no cost); say yes only if",
            "the command above is installed and logged in.",
        ])
    }

    fn bench_refusal(&self, _command: &[String]) -> Option<(&'static str, &'static str)> {
        Some((
            "the bench does not run through Cursor's CLI.",
            "Cursor's CLI reports no cost, so the budget could not stop the run, and it reads \
             your ~/.cursor (MCP servers, hooks) with no way to isolate a run.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_under_the_home_and_the_data_directory() {
        let paths = CursorPaths::resolve(
            &|name| (name == "HOME").then(|| "/h".to_string()),
            Path::new("/d"),
        )
        .unwrap();
        assert_eq!(paths.mcp_file, PathBuf::from("/h/.cursor/mcp.json"));
        assert_eq!(paths.hooks_file, PathBuf::from("/h/.cursor/hooks.json"));
        assert_eq!(
            paths.rules_file,
            PathBuf::from("/h/.cursor/rules/graphersal-memory.mdc")
        );
        assert_eq!(
            paths.registry,
            PathBuf::from("/d/sessions/cursor-transcripts.json")
        );
        assert_eq!(
            paths.export_dir,
            PathBuf::from("/h/graphersal-memory-export/cursor")
        );
        let windows = CursorPaths::resolve(
            &|name| (name == "USERPROFILE").then(|| "/u".to_string()),
            Path::new("/d"),
        )
        .unwrap();
        assert_eq!(windows.mcp_file, PathBuf::from("/u/.cursor/mcp.json"));
        assert!(CursorPaths::resolve(&|_| None, Path::new("/d")).is_err());
    }

    #[test]
    fn the_entry_is_ours_and_cursor_commands_are_recognised() {
        let agent = Cursor::new(CursorPaths::of_home(Path::new("/h"), Path::new("/d")));
        let entry = agent.mcp_entry(Path::new("/x/graphersal-memory"), &["mcp".into()]);
        assert_eq!(entry["type"], "stdio");
        assert!(crate::agents::is_our_entry(&entry));
        assert_eq!(agent.auto_memory_key(), None);
        assert_eq!(agent.headless_command(), None);
        assert!(!agent.has_file_memory());
        let words = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        assert!(is_cursor_cli(&words("agent -p")));
        assert!(is_cursor_cli(&words("/usr/local/bin/cursor-agent -p")));
        assert!(!is_cursor_cli(&words("claude -p")));
    }

    #[test]
    fn summaries_and_bench_refuse_cursors_cli_before_running_anything() {
        let command = vec!["agent".to_string(), "-p".to_string()];
        let err = crate::bench::check_harness(&command).unwrap_err();
        assert!(err.contains("Cursor's CLI"), "{err}");
        let tmp = crate::mcp_tests::TempDir::new("cursor-summary");
        let err = crate::summaries::run_harness_with(
            &command,
            "prompt",
            tmp.path(),
            std::time::Duration::from_secs(1),
            None,
            None,
        )
        .unwrap_err();
        assert_eq!(err, NO_HEADLESS);
    }

    #[test]
    fn programs_are_found_on_the_search_path() {
        let tmp = crate::mcp_tests::TempDir::new("cursor-program");
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        assert_eq!(find_program(Some(bin.as_os_str())), None);
        std::fs::write(bin.join("agent"), "").unwrap();
        assert_eq!(
            find_program(Some(bin.as_os_str())),
            None,
            "`agent` alone is no sign of Cursor"
        );
        std::fs::write(bin.join("cursor-agent"), "").unwrap();
        assert_eq!(
            find_program(Some(bin.as_os_str())),
            Some(bin.join("cursor-agent"))
        );
    }
}
