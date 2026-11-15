//! `install`, `uninstall`, `status` and `doctor` for Claude Code.
//!
//! What `install` writes, and nothing else:
//!
//! - **MCP servers**, user scope, in Claude Code's user configuration (`~/.claude.json`, or
//!   `$CLAUDE_CONFIG_DIR/.claude.json`): the member `mcpServers.graphersal-memory` (the `use`
//!   profile) and, only with `--enable-develop`, `mcpServers.graphersal-memory-develop`. Claude
//!   Code has no switched-off state for a user-scope server (it can disable a server per project
//!   only), so the develop server stays unregistered until it is enabled, and
//!   `uninstall --disable-develop` unregisters it again.
//! - **Hooks** in Claude Code's user settings (`~/.claude/settings.json`): one entry in
//!   `hooks.SessionStart` and one in `hooks.SessionEnd`, each a group with the single command
//!   `"<this binary>" hook session-start|session-end` (on Windows the exec form: `command` =
//!   this binary, `args` = `["hook", "session-start", ...]`, run without a shell; see
//!   `crate::winshell`). Our entries are recognised by that command, in either form.
//! - **The managed rule block** in the user instruction file (`~/.claude/CLAUDE.md`), between
//!   `<!-- graphersal-memory:begin ... -->` and `<!-- graphersal-memory:end -->`; the begin line
//!   carries the fingerprint of the block's text, so a hand edit inside is detected and reported
//!   as a conflict, never overwritten. A block an earlier version wrote (its fingerprint is in
//!   `PREVIOUS_BLOCK_FINGERPRINTS`) and left untouched is updated by `install` and removed by
//!   `uninstall` like the current one.
//!
//! The JSON files are edited in place (`crate::jsonedit`): only our members are inserted or
//! removed, the rest of the text keeps its bytes. (`claude mcp add` is not used: it rewrites the
//! whole file, reorders and adds keys, so `uninstall` could not give the file back byte for
//! byte.) Before its first change a file is backed up to `<data dir>/install/backups/`, and
//! `<data dir>/install/claude-code.json` records what was changed. `uninstall` puts a file back
//! from its backup when nobody changed it since `install` wrote it (or deletes it when install
//! created it); otherwise it removes our members and our block from the current text.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::agents::{Agent, ClaudeCode, Kind};
use crate::config::Config;
use crate::import::{self, Connect, DirPlan, ImportState};
use crate::language::Language;
use crate::prompt::{Choice, Console, paint};
use crate::{download, export, fsutil, jsonedit, models};

/// The name of the `use` server in Claude Code.
pub const USE_SERVER: &str = "graphersal-memory";

/// The name of the `develop` server in Claude Code.
pub const DEVELOP_SERVER: &str = "graphersal-memory-develop";

/// The hook events install adds, with the hook subcommand each one runs.
pub const HOOK_EVENTS: [(&str, &str); 2] = [
    ("SessionStart", "session-start"),
    ("SessionEnd", "session-end"),
];

/// How long Claude Code lets a hook run (seconds); the hook itself stops earlier.
pub const HOOK_TIMEOUT_SECS: u64 = 30;

/// The first characters of the managed block's begin line.
pub const BLOCK_BEGIN: &str = "<!-- graphersal-memory:begin";

/// The managed block's end line.
pub const BLOCK_END: &str = "<!-- graphersal-memory:end -->";

/// The install state directory in the data directory.
const STATE_DIR: &str = "install";

/// The format version of the state file.
const FORMAT: u64 = 1;

/// The text of the managed rule block (between its marker lines) with the English memory
/// language rule (the default; [`RULE_BLOCK_ANY`] is the "any language" text).
pub const RULE_BLOCK: &str = "\
## Memory: graphersal-memory

Your long-term memory of this user lives in a local graph, served by the MCP server
`graphersal-memory`, not in memory files.

- Store and search memories in English. Translate a user's words into English before saving
  them or searching; quote exact wording only when the exact words matter.
- Search it before you answer from memory or ask the user something they may have told you
  before: `memories(text)` for rules, decisions and preferences, `recall(text)` for everything,
  `memory(slug)` to read one in full.
- Save with `remember`: search with `memories(text)` first, so you replace a memory instead of
  duplicating it; scope \"global\" for rules and preferences that hold in every project. Remove
  with `forget`.
- Save what will help in later sessions and in other tasks like this one: rules for how to work
  (always / never / only when, with the why), lasting facts about the user and the project, where
  things are. Write it as a lasting rule or fact, not as what happened today. Leave out what only
  the current task needs: its plan, steps, progress, branch, findings.
- Do not write memory files (`MEMORY.md` and the files beside it) any more. Files written anyway
  are imported into the graph at the next session start.
- If another instruction (a CLAUDE.md, AGENTS.md or rules file, in any language) tells you to
  keep memory elsewhere (`MEMORY.md`, another tool or place), memory still lives here: tell the
  user once that the two instructions disagree and which file says it, so they can fix it.
- Past sessions of this project: `sessions(text)` finds what was decided, corrected or left
  open in them (from their summaries, when summaries are on), `session(id)` reads one, and
  `recall(text, k, \"turn\")` finds what was literally said.
";

/// The rule block with the "any language" memory language rule.
pub const RULE_BLOCK_ANY: &str = "\
## Memory: graphersal-memory

Your long-term memory of this user lives in a local graph, served by the MCP server
`graphersal-memory`, not in memory files.

- Store memories in the language the user uses; search in that language and in English.
- Search it before you answer from memory or ask the user something they may have told you
  before: `memories(text)` for rules, decisions and preferences, `recall(text)` for everything,
  `memory(slug)` to read one in full.
- Save with `remember`: search with `memories(text)` first, so you replace a memory instead of
  duplicating it; scope \"global\" for rules and preferences that hold in every project. Remove
  with `forget`.
- Save what will help in later sessions and in other tasks like this one: rules for how to work
  (always / never / only when, with the why), lasting facts about the user and the project, where
  things are. Write it as a lasting rule or fact, not as what happened today. Leave out what only
  the current task needs: its plan, steps, progress, branch, findings.
- Do not write memory files (`MEMORY.md` and the files beside it) any more. Files written anyway
  are imported into the graph at the next session start.
- If another instruction (a CLAUDE.md, AGENTS.md or rules file, in any language) tells you to
  keep memory elsewhere (`MEMORY.md`, another tool or place), memory still lives here: tell the
  user once that the two instructions disagree and which file says it, so they can fix it.
- Past sessions of this project: `sessions(text)` finds what was decided, corrected or left
  open in them (from their summaries, when summaries are on), `session(id)` reads one, and
  `recall(text, k, \"turn\")` finds what was literally said.
";

/// The rule block text of a memory language.
pub fn rule_block(language: Language) -> &'static str {
    match language {
        Language::English => RULE_BLOCK,
        Language::Any => RULE_BLOCK_ANY,
    }
}

/// The fingerprints of the rule block texts earlier versions wrote (oldest first; add the
/// current one here whenever `RULE_BLOCK` changes). An untouched block with one of them, or
/// with the text of the other memory language, is updated by `install` and removed by
/// `uninstall`; one with an unknown fingerprint whose text still matches it (a newer version's
/// block) is kept by `install` and reported.
pub const PREVIOUS_BLOCK_FINGERPRINTS: &[&str] = &[
    // 0.0.0 (stages M3-M5): "Past sessions ... are not indexed yet in this version."
    "e255edadf6bd5788",
    // 0.1.0 before the memory language (stages M5-M6): no language rule.
    "80779cc2f75cc8fc",
    // 0.1.0 with the memory language, English and any language (stages M6-M15): no line about
    // instructions that keep memory elsewhere.
    "b5b70fbe26fde82a",
    "5a6b75fa55b96838",
    // 0.1.0 and 0.2.0 before the line about what to save (English, any language).
    "9e81846ca7328d34",
    "1cb7a5e674a86eea",
];

/// Where Claude Code keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudePaths {
    /// The configuration directory (`~/.claude` or `$CLAUDE_CONFIG_DIR`).
    pub config_dir: PathBuf,
    /// The user configuration with the user-scope MCP servers.
    pub user_config: PathBuf,
    /// The user settings with the hooks.
    pub settings: PathBuf,
    /// The user instruction file.
    pub instructions: PathBuf,
    /// The per-project directories (transcripts, file memory).
    pub projects: PathBuf,
}

impl ClaudePaths {
    /// Resolves the paths from the environment (`env` reads one variable): `CLAUDE_CONFIG_DIR`
    /// when set, else `~/.claude` (home: [`crate::store::home_dir`]).
    pub fn resolve(env: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let set = |name: &str| env(name).filter(|value| !value.is_empty());
        if let Some(dir) = set("CLAUDE_CONFIG_DIR") {
            let dir = PathBuf::from(dir);
            return Ok(Self::with(dir.join(".claude.json"), dir));
        }
        let home = crate::store::home_dir(env)?;
        Ok(Self::with(home.join(".claude.json"), home.join(".claude")))
    }

    fn with(user_config: PathBuf, config_dir: PathBuf) -> Self {
        Self {
            settings: config_dir.join("settings.json"),
            instructions: config_dir.join("CLAUDE.md"),
            projects: config_dir.join("projects"),
            user_config,
            config_dir,
        }
    }
}

/// Everything install needs to know about this machine.
#[derive(Debug, Clone)]
pub struct Setup {
    /// Claude Code's files.
    pub paths: ClaudePaths,
    /// opencode's files.
    pub opencode: crate::opencode::OpenCodePaths,
    /// Codex's files.
    pub codex: crate::codex::CodexPaths,
    /// Cursor's files (and the registry of its transcripts in the data directory).
    pub cursor: crate::cursor::CursorPaths,
    /// CodeWhale's files.
    pub codewhale: crate::codewhale::CodeWhalePaths,
    /// The program search path the harness detection looks in (`None`: this process's `PATH`;
    /// tests pass an empty one).
    pub search_path: Option<std::ffi::OsString>,
    /// The data directory.
    pub data_dir: PathBuf,
    /// The run directory.
    pub run_dir: PathBuf,
    /// This binary (what the MCP entries and hooks start).
    pub exe: PathBuf,
    /// Arguments that point a started process at an explicitly chosen data or run directory
    /// (`--data-dir X`, `--run-dir Y`); empty for the defaults.
    pub dir_args: Vec<String>,
    /// How hook commands are written on this machine (`crate::winshell`).
    pub hooks: HookForm,
    /// Where an embedding model download connects (Hugging Face, or
    /// `GRAPHERSAL_MEMORY_HF_ENDPOINT`).
    pub source: download::Source,
}

impl Setup {
    /// Claude Code with this setup's paths.
    pub fn agent(&self) -> ClaudeCode {
        ClaudeCode::new(self.paths.clone())
    }

    /// opencode with this setup's paths.
    pub fn opencode_agent(&self) -> crate::opencode::OpenCode {
        crate::opencode::OpenCode::new(self.opencode.clone())
    }

    /// Codex with this setup's paths.
    pub fn codex_agent(&self) -> crate::codex::Codex {
        crate::codex::Codex::new(self.codex.clone())
    }

    /// Cursor with this setup's paths.
    pub fn cursor_agent(&self) -> crate::cursor::Cursor {
        crate::cursor::Cursor::new(self.cursor.clone())
    }

    /// CodeWhale with this setup's paths.
    pub fn codewhale_agent(&self) -> crate::codewhale::CodeWhale {
        crate::codewhale::CodeWhale::new(self.codewhale.clone())
    }

    /// `text` as output shows it: the home directory as `~` ([`tilde`]).
    pub fn show(&self, text: &str) -> String {
        tilde(text, &self.opencode.home)
    }

    /// The agent of a harness.
    pub fn agent_of(&self, kind: Kind) -> Box<dyn Agent> {
        match kind {
            Kind::ClaudeCode => Box::new(self.agent()),
            Kind::OpenCode => Box::new(self.opencode_agent()),
            Kind::Codex => Box::new(self.codex_agent()),
            Kind::Cursor => Box::new(self.cursor_agent()),
            Kind::CodeWhale => Box::new(self.codewhale_agent()),
        }
    }

    /// The program of a harness on this machine: on the search path, or in a known install
    /// location that is not on it (`~/.local/bin/claude`, `~/.claude/local/claude`,
    /// `~/.opencode/bin/opencode`, Cursor's app bundle). System-wide locations (`/Applications`)
    /// are looked at only for this process's own `PATH` (tests pass their search path).
    pub fn program(&self, kind: Kind) -> Option<PathBuf> {
        let home = &self.opencode.home;
        let search = self.search_path.as_deref();
        let first_file = |candidates: Vec<PathBuf>| candidates.into_iter().find(|p| p.is_file());
        match kind {
            Kind::ClaudeCode => {
                let path = match search {
                    Some(path) => Some(path.to_os_string()),
                    None => std::env::var_os("PATH"),
                };
                let mut candidates: Vec<PathBuf> = path
                    .iter()
                    .flat_map(std::env::split_paths)
                    .flat_map(|dir| {
                        ["claude", "claude.exe", "claude.cmd"].map(|name| dir.join(name))
                    })
                    .collect();
                candidates.push(home.join(".local").join("bin").join("claude"));
                candidates.push(home.join(".claude").join("local").join("claude"));
                first_file(candidates)
            }
            Kind::OpenCode => crate::opencode::find_program(home, search),
            Kind::Codex => crate::codex::find_program(search),
            Kind::Cursor => crate::cursor::find_program(search)
                .or_else(|| {
                    let mut bundles = vec![home.join("Applications").join("Cursor.app")];
                    if search.is_none() {
                        bundles.push(PathBuf::from("/Applications/Cursor.app"));
                    }
                    bundles.into_iter().find(|bundle| bundle.is_dir())
                })
                .or_else(|| first_file(cursor_windows_locations(home, search.is_none()))),
            Kind::CodeWhale => crate::codewhale::find_program(home, search),
        }
    }

    /// Whether a harness's configuration is on this machine (its configuration directory or
    /// file), whether or not its program is.
    fn config_found(&self, kind: Kind) -> Option<PathBuf> {
        let candidates: Vec<PathBuf> = match kind {
            Kind::ClaudeCode => vec![
                self.paths.config_dir.clone(),
                self.paths.user_config.clone(),
            ],
            Kind::OpenCode => vec![self.opencode.config_dir.clone()],
            Kind::Codex => vec![self.codex.codex_home.clone()],
            Kind::Cursor => vec![self.cursor.cursor_dir.clone()],
            Kind::CodeWhale => vec![self.codewhale.codewhale_home.clone()],
        };
        candidates.into_iter().find(|path| path.exists())
    }

    /// The harnesses found on this machine, in wizard order: those whose program is found
    /// ([`Setup::program`]). A configuration directory alone is not enough
    /// ([`Setup::leftovers`]); `--harness` installs into another one after a warning.
    pub fn detected(&self) -> Vec<Kind> {
        Kind::ALL
            .into_iter()
            .filter(|kind| self.program(*kind).is_some())
            .collect()
    }

    /// The harnesses whose configuration (or our installation in it) is on this machine but
    /// whose program is not: reported, never offered. Each with the path found.
    pub fn leftovers(&self) -> Vec<(Kind, PathBuf)> {
        Kind::ALL
            .into_iter()
            .filter(|kind| self.program(*kind).is_none())
            .filter_map(|kind| {
                self.config_found(kind)
                    .or_else(|| State::exists_of(self, kind).then(|| State::path_of(self, kind)))
                    .map(|path| (kind, path))
            })
            .collect()
    }

    /// The name of a harness's program, for messages.
    pub fn program_name(kind: Kind) -> &'static str {
        match kind {
            Kind::ClaudeCode => "claude",
            Kind::OpenCode => "opencode",
            Kind::Codex => "codex",
            Kind::Cursor => "cursor",
            Kind::CodeWhale => "codewhale",
        }
    }

    /// The harnesses graphersal-memory is installed in (their state file exists, or one of
    /// our parts is in their files).
    pub fn installed(&self) -> Vec<Kind> {
        Kind::ALL
            .into_iter()
            .filter(|kind| {
                State::exists_of(self, *kind)
                    || match kind {
                        Kind::ClaudeCode => !inspect(self).nothing_installed(),
                        Kind::OpenCode => {
                            !crate::opencode::install::inspect(self).nothing_installed()
                        }
                        Kind::Codex => !crate::codex::install::inspect(self).nothing_installed(),
                        Kind::Cursor => !crate::cursor::install::inspect(self).nothing_installed(),
                        Kind::CodeWhale => {
                            !crate::codewhale::install::inspect(self).nothing_installed()
                        }
                    }
            })
            .collect()
    }

    /// The MCP entry of a profile (`use` or `develop`).
    pub fn mcp_entry(&self, profile: &str) -> Value {
        let mut args = vec![
            "mcp".to_string(),
            "--profile".to_string(),
            profile.to_string(),
        ];
        args.extend(self.dir_args.iter().cloned());
        self.agent().mcp_entry(&self.exe, &args)
    }

    /// The hook command of an event (`session-start`, `session-end`) as one shell command line.
    /// On Windows, the words every shell runs unquoted when there are such words
    /// ([`HookForm::plain`]); else the quoted program, which a POSIX shell (Git Bash on
    /// Windows) and `cmd.exe` run, PowerShell not.
    pub fn hook_command(&self, event: &str) -> String {
        match &self.hooks.plain {
            Some(words) => {
                let mut command = format!("{} hook {event}", words[0]);
                for word in &words[1..] {
                    command.push(' ');
                    command.push_str(word);
                }
                command
            }
            None => self.hook_command_with(event, self.quote_fn(), ""),
        }
    }

    /// Claude Code's hook command of an event as `install` shows it: the shell command line,
    /// or on Windows the exec form's program and arguments (run without a shell).
    pub fn claude_hook_text(&self, event: &str) -> String {
        if self.hooks.windows {
            format!(
                "{} (run without a shell)",
                self.hook_command_with(event, quote_double, "")
            )
        } else {
            self.hook_command(event)
        }
    }

    /// How this machine's hook command lines quote a word: double quotes for Windows (`cmd.exe`
    /// and Git Bash run them), single quotes for `sh` elsewhere.
    fn quote_fn(&self) -> fn(&str) -> String {
        if self.hooks.windows {
            quote_double
        } else {
            quote_single
        }
    }

    /// [`Setup::hook_command`] for PowerShell: `& 'program' hook <event> ...` (a quoted string
    /// alone is an expression there, `&` runs it; single quotes keep `$` and backticks literal).
    pub fn powershell_hook_command(&self, event: &str) -> String {
        self.hook_command_with(event, quote_powershell, "& ")
    }

    fn hook_command_with(&self, event: &str, quote: fn(&str) -> String, call: &str) -> String {
        let mut command = format!(
            "{call}{} hook {event}",
            quote(&self.exe.display().to_string())
        );
        for arg in &self.dir_args {
            command.push(' ');
            if arg.starts_with("--") {
                command.push_str(arg);
            } else {
                command.push_str(&quote(arg));
            }
        }
        command
    }

    /// The hook group of an event, as it goes into Claude Code's settings. On Windows the exec
    /// form (`command` = this binary, `args`; Claude Code 2.1.139 and later spawn it without a
    /// shell, so it runs with Git Bash, with PowerShell, and whatever the path holds); elsewhere
    /// the shell command line.
    pub fn hook_group(&self, event: &str) -> Value {
        if self.hooks.windows {
            let mut args = vec!["hook".to_string(), event.to_string()];
            args.extend(self.dir_args.iter().cloned());
            return json!({"hooks": [{"type": "command",
                                     "command": self.exe.display().to_string(),
                                     "args": args, "timeout": HOOK_TIMEOUT_SECS}]});
        }
        json!({"hooks": [{"type": "command", "command": self.hook_command(event),
                          "timeout": HOOK_TIMEOUT_SECS}]})
    }

    pub(crate) fn state_dir(&self) -> PathBuf {
        self.data_dir.join(STATE_DIR)
    }

    /// What recognises our entries on this machine ([`Ours`]): this binary, and every binary
    /// an install recorded in a state file.
    pub fn ours(&self) -> Ours {
        let mut programs = vec![self.exe.display().to_string()];
        for kind in Kind::ALL {
            if let Ok(state) = State::load_of(self, kind)
                && let Some(exe) = state.meta.get("exe").and_then(Value::as_str)
                && !programs.iter().any(|p| p == exe)
            {
                programs.push(exe.to_string());
            }
        }
        Ours { programs }
    }
}

/// Where Cursor's Windows installers put `Cursor.exe` when it is not on `PATH`: the per-user
/// install (`%LOCALAPPDATA%\Programs\cursor`, under the home directory) and, for this process's
/// own machine (`system`), the per-machine one (`%ProgramFiles%\cursor`) and `%LOCALAPPDATA%`
/// itself when it is elsewhere.
fn cursor_windows_locations(home: &Path, system: bool) -> Vec<PathBuf> {
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

/// How the hook commands are written on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookForm {
    /// Windows: Claude Code's hooks in exec form, the other harnesses' commands in
    /// [`HookForm::plain`] words when there are such.
    pub windows: bool,
    /// The program and the directory arguments as words bash, `cmd.exe` and PowerShell all run
    /// unquoted (`crate::winshell::all_shell_words`); `None` outside Windows, or when one of
    /// them has no such form.
    pub plain: Option<Vec<String>>,
}

impl HookForm {
    /// macOS and Linux: the quoted program for `sh -c`.
    pub fn posix() -> Self {
        Self::default()
    }

    /// The form for `exe` and `dir_args` on this machine.
    pub fn here(exe: &Path, dir_args: &[String]) -> Self {
        if cfg!(windows) {
            Self {
                windows: true,
                plain: crate::winshell::all_shell_words_here(exe, dir_args),
            }
        } else {
            Self::posix()
        }
    }
}

/// Names joined for a sentence: "a", "a and b", "a, b and c".
pub fn join_and(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The display names of `kinds` for a sentence ([`join_and`]).
pub fn names_of(kinds: &[Kind]) -> String {
    join_and(&kinds.iter().map(|k| k.display_name()).collect::<Vec<_>>())
}

/// `text` with the home directory written as `~` (every path that starts with it), except in
/// commands shown between backticks: a shell does not expand `~` inside quotes, so a command
/// copied from the output must carry the full path.
pub fn tilde(text: &str, home: &Path) -> String {
    if text.matches('`').count() >= 2 {
        let mut out = String::new();
        for (n, part) in text.split('`').enumerate() {
            if n > 0 {
                out.push('`');
            }
            if n % 2 == 1 && text.split('`').count() > n + 1 {
                out.push_str(part);
            } else {
                out.push_str(&tilde_prose(part, home));
            }
        }
        return out;
    }
    tilde_prose(text, home)
}

/// [`tilde`] of text without commands.
fn tilde_prose(text: &str, home: &Path) -> String {
    let home = home.display().to_string();
    if home.is_empty() || home == "/" {
        return text.to_string();
    }
    let sep = std::path::MAIN_SEPARATOR;
    let mut out = text.replace(&format!("{home}{sep}"), &format!("~{sep}"));
    if sep != '/' {
        out = out.replace(&format!("{home}/"), "~/");
    }
    // The home directory itself, at the end of the text or before a space or punctuation.
    for end in [" ", ")", ",", ";", ":"] {
        out = out.replace(&format!("{home}{end}"), &format!("~{end}"));
    }
    if out.ends_with(&home) {
        let cut = out.len() - home.len();
        out = format!("{}~", &out[..cut]);
    }
    out
}

/// What install and uninstall print when they would ask but nothing answers (no terminal: a
/// pipe, a harness's shell command).
pub const NO_TERMINAL: &str = "there is no terminal to ask in; run it in a terminal, or add --yes";

/// An answer to a yes/no question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reply {
    Yes,
    No,
    /// The input ended before an answer (no terminal).
    NoInput,
}

/// Asks `question` in the style install and uninstall share: `[Y/n]` (Enter = yes) or `[y/N]`
/// (Enter = no) after it.
pub(crate) fn ask_yes_no(
    question: &str,
    default_yes: bool,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Reply {
    let _ = write!(
        out,
        "{question} {} ",
        if default_yes { "[Y/n]" } else { "[y/N]" }
    );
    let _ = out.flush();
    let mut answer = String::new();
    match input.read_line(&mut answer) {
        Ok(0) | Err(_) => {
            let _ = writeln!(out);
            Reply::NoInput
        }
        Ok(_) => match answer.trim().to_ascii_lowercase().as_str() {
            "" if default_yes => Reply::Yes,
            "y" | "yes" => Reply::Yes,
            _ => Reply::No,
        },
    }
}

/// `text` in single quotes for `sh`.
fn quote_single(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// `text` in double quotes (what earlier versions wrote on every platform).
fn quote_double(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\\\""))
}

/// The program word of a command line and the rest: a word in double quotes (`\"` inside), in
/// single quotes (`sh`'s `'\''` and PowerShell's `''` inside), after PowerShell's `& `, or a
/// bare word.
pub(crate) fn split_program(command: &str) -> Option<(String, &str)> {
    let text = command.trim_start();
    let text = text.strip_prefix("& ").unwrap_or(text).trim_start();
    let bytes = text.as_bytes();
    let mut word = String::new();
    let mut i = 0;
    match bytes.first()? {
        b'"' => {
            i = 1;
            loop {
                let c = text[i..].chars().next()?;
                if c == '\\' && text[i + 1..].starts_with(['"', '\\']) {
                    word.push(text[i + 1..].chars().next()?);
                    i += 2;
                } else if c == '"' {
                    i += 1;
                    break;
                } else {
                    word.push(c);
                    i += c.len_utf8();
                }
            }
        }
        b'\'' => loop {
            // One single-quoted run, then `\''` (sh) or `''` (PowerShell) continue the word.
            i += 1;
            let end = i + text[i..].find('\'')?;
            word.push_str(&text[i..end]);
            i = end + 1;
            if text[i..].starts_with("\\''") {
                word.push('\'');
                i += 2;
            } else if text[i..].starts_with('\'') {
                word.push('\'');
            } else {
                break;
            }
        },
        _ => {
            let end = text.find(char::is_whitespace).unwrap_or(text.len());
            word.push_str(&text[..end]);
            i = end;
        }
    }
    Some((word, &text[i..]))
}

/// What recognises our entries: the program a `graphersal-memory` binary by its file name, or
/// exactly this binary or one an install recorded (a binary renamed or installed under another
/// name). The same rule for MCP entries and hook commands of every harness.
#[derive(Debug, Clone, Default)]
pub struct Ours {
    programs: Vec<String>,
}

impl Ours {
    /// Whether `program` (a path or a bare name) is ours.
    pub fn program(&self, program: &str) -> bool {
        crate::agents::is_our_program(program) || self.programs.iter().any(|p| p == program)
    }

    /// Whether an MCP entry (either shape) starts one of our programs with `mcp`.
    pub fn entry(&self, entry: &Value) -> bool {
        let first = match &entry["command"] {
            Value::Array(parts) => parts.get(1).cloned().unwrap_or(Value::Null),
            _ => entry["args"][0].clone(),
        };
        self.program(crate::agents::entry_program(entry)) && first == "mcp"
    }

    /// Whether a hook command line runs one of our programs with `hook session-...`.
    pub fn command(&self, command: &str) -> bool {
        split_program(command).is_some_and(|(program, rest)| {
            self.program(&program) && rest.trim_start().starts_with("hook session-")
        })
    }

    /// Whether a hook entry is ours: a command line ([`Ours::command`]) or the exec form
    /// (`command` = our program, `args` starting with `hook session-...`).
    pub fn hook(&self, hook: &Value) -> bool {
        let command = hook["command"].as_str().unwrap_or_default();
        match hook["args"].as_array() {
            Some(args) => {
                self.program(command)
                    && args.first().and_then(Value::as_str) == Some("hook")
                    && args
                        .get(1)
                        .and_then(Value::as_str)
                        .is_some_and(|event| event.starts_with("session-"))
            }
            None => self.command(command),
        }
    }

    /// Whether a hook group (an element of `hooks.<Event>`) consists of our hooks only.
    pub fn hook_group(&self, group: &Value) -> bool {
        group["hooks"]
            .as_array()
            .is_some_and(|hooks| !hooks.is_empty() && hooks.iter().all(|h| self.hook(h)))
    }

    /// Whether a hook group holds one of our hooks beside hooks of others.
    pub fn mixed_group(&self, group: &Value) -> bool {
        group["hooks"].as_array().is_some_and(|hooks| {
            hooks.iter().any(|h| self.hook(h)) && !hooks.iter().all(|h| self.hook(h))
        })
    }
}

/// `text` in single quotes for a PowerShell command line (a `'` inside is doubled).
fn quote_powershell(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Whether an MCP entry is one of ours by its program's file name (`graphersal-memory`); with a
/// [`Setup`], [`Setup::ours`] also knows a renamed binary.
#[cfg(test)]
pub fn is_our_entry(entry: &Value) -> bool {
    Ours::default().entry(entry)
}

/// Whether a hook command is one of ours by its program's file name ([`Ours::command`]).
#[cfg(test)]
pub(crate) fn is_our_command(command: &str) -> bool {
    Ours::default().command(command)
}

/// Whether a hook entry is ours by its program's file name ([`Ours::hook`]).
#[cfg(test)]
pub fn is_our_hook(hook: &Value) -> bool {
    Ours::default().hook(hook)
}

/// Whether Claude Code's settings file (`paths.settings`) holds our SessionStart hook (then
/// the hook delivers the start context, and the MCP instructions leave it out).
pub fn claude_start_hook_installed(paths: &ClaudePaths) -> bool {
    read_json(&paths.settings, &mut Vec::new())["hooks"]["SessionStart"]
        .as_array()
        .is_some_and(|groups| groups.iter().any(is_our_hook_group))
}

/// Whether a hook group (an element of `hooks.<Event>`) consists of our commands only (by the
/// program's file name).
pub fn is_our_hook_group(group: &Value) -> bool {
    Ours::default().hook_group(group)
}

/// The managed block for `RULE_BLOCK` (English), marker lines included, ending with a newline.
#[cfg(test)]
pub fn block_text() -> String {
    block_text_for(Language::English)
}

/// The managed block of a memory language, marker lines included, ending with a newline.
#[cfg(test)]
pub fn block_text_for(language: Language) -> String {
    wrap_block(rule_block(language))
}

/// A rule block text between the marker lines (the begin line carrying its fingerprint).
pub fn wrap_block(text: &str) -> String {
    format!(
        "{BLOCK_BEGIN} fingerprint={} (managed by `graphersal-memory install`; \
         `graphersal-memory uninstall` removes it; an edit inside is reported as a conflict) \
         -->\n{text}{BLOCK_END}\n",
        fsutil::fingerprint(text.as_bytes())
    )
}

/// A managed block found in a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundBlock {
    /// Its first byte (the begin line).
    pub start: usize,
    /// One past its last byte (after the end line's newline).
    pub end: usize,
    /// Whether its text still matches the fingerprint on its begin line.
    pub intact: bool,
    /// Whether it is the block this version writes.
    pub current: bool,
    /// Whether it is untouched and its text one this or an earlier version wrote.
    pub known: bool,
}

/// The managed block of `text` (current = the English block): `Ok(None)` without one, `Err` for
/// a begin line without an end line.
pub fn find_block(text: &str) -> Result<Option<FoundBlock>, String> {
    find_block_for(text, Language::English)
}

/// [`find_block`] with the block of `language` as the current one.
pub fn find_block_for(text: &str, language: Language) -> Result<Option<FoundBlock>, String> {
    find_block_with(text, language, &[])
}

/// [`find_block_for`] with the read-only line naming `read_only` in the current block
/// (`crate::coexist::rule_with`).
pub fn find_block_with(
    text: &str,
    language: Language,
    read_only: &[String],
) -> Result<Option<FoundBlock>, String> {
    let ours = [rule_block(Language::English), rule_block(Language::Any)];
    find_block_in(
        text,
        &crate::coexist::rule_with(rule_block(language), read_only),
        &ours,
        PREVIOUS_BLOCK_FINGERPRINTS,
    )
}

/// The managed block of `language` naming `read_only`, marker lines included.
pub fn block_text_with(language: Language, read_only: &[String]) -> String {
    wrap_block(&crate::coexist::rule_with(rule_block(language), read_only))
}

/// The managed block of `text` with `current` as the block this version writes, `ours` the
/// texts of every memory language and `previous` the fingerprints earlier versions wrote.
pub fn find_block_in(
    text: &str,
    current: &str,
    ours: &[&str],
    previous: &[&str],
) -> Result<Option<FoundBlock>, String> {
    let Some(start) = text
        .match_indices(BLOCK_BEGIN)
        .map(|(at, _)| at)
        .find(|at| *at == 0 || text[..*at].ends_with('\n'))
    else {
        return Ok(None);
    };
    let line_end = text[start..]
        .find('\n')
        .map(|n| start + n + 1)
        .ok_or("the managed block has no end marker")?;
    let begin_line = &text[start..line_end];
    let recorded = begin_line
        .split_whitespace()
        .find_map(|word| word.strip_prefix("fingerprint="))
        .unwrap_or_default();
    let end_at = text[line_end..]
        .match_indices(BLOCK_END)
        .map(|(at, _)| line_end + at)
        .find(|at| text[..*at].ends_with('\n'))
        .ok_or("the managed block has no end marker")?;
    // A file with CRLF line ends holds the block with CRLF too: compared with LF.
    let content = &text[line_end..end_at].replace("\r\n", "\n");
    let mut end = end_at + BLOCK_END.len();
    if text[end..].starts_with("\r\n") {
        end += 2;
    } else if text[end..].starts_with('\n') {
        end += 1;
    }
    let intact = fsutil::fingerprint(content.as_bytes()) == recorded;
    let is_current = intact && text[start..end].replace("\r\n", "\n") == wrap_block(current);
    // A block of ours whatever other memory tools its last line names read-only (M15).
    let base = crate::coexist::strip_read_only(content);
    let is_ours = ours.iter().any(|block| {
        fsutil::fingerprint(block.as_bytes()) == recorded || (base != content && *block == base)
    });
    // An earlier version's block that names other memory tools read-only: its recorded
    // fingerprint covers that line, the list holds the fingerprints of the texts without it.
    let fingerprint = fsutil::fingerprint(base.as_bytes());
    let was_ours = previous.contains(&recorded)
        || (base != content && previous.contains(&fingerprint.as_str()));
    Ok(Some(FoundBlock {
        start,
        end,
        intact,
        current: is_current,
        known: is_current || (intact && (is_ours || was_ours)),
    }))
}

/// The separator put before a block appended to `text` (CRLF when `text` uses CRLF).
pub(crate) fn block_separator(text: &str) -> &'static str {
    let crlf = text.contains("\r\n");
    if text.is_empty() || text.ends_with("\n\n") || text.ends_with("\r\n\r\n") {
        ""
    } else if text.ends_with('\n') {
        if crlf { "\r\n" } else { "\n" }
    } else if crlf {
        "\r\n\r\n"
    } else {
        "\n\n"
    }
}

/// `block` (LF line ends) with the line ends of `text`: CRLF when `text` uses them.
pub(crate) fn line_ends_like(text: &str, block: &str) -> String {
    if text.contains("\r\n") {
        block.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        block.to_string()
    }
}

/// Options of `install`: every wizard question has a flag (`None`/`false`: the question's
/// default, or the current answer on an installed system).
#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    /// The harnesses to install into (`--harness`; `None`: the wizard asks among the detected
    /// ones, without questions every detected one, Claude Code when none is found).
    pub harnesses: Option<Vec<Kind>>,
    /// Show the plan only.
    pub dry_run: bool,
    /// Do not ask.
    pub yes: bool,
    /// Register the develop server too (`--enable-develop`).
    pub enable_develop: bool,
    /// Switch session summaries on without asking (`--summaries`).
    pub summaries: bool,
    /// The memory language (`--language`).
    pub language: Option<Language>,
    /// The embedding model (`--model`): a repository or `hash-ngram/1` (`builtin`).
    pub model: Option<String>,
    /// Do not import the existing file memory now (`--no-import`).
    pub no_import: bool,
    /// Keep the harness's own memory on (`--keep-auto-memory`).
    pub keep_auto_memory: bool,
    /// Other memory tools (MCP server names) the rule block names read-only
    /// (`--other-read-only`).
    pub other_read_only: Vec<String>,
    /// Other memory tools kept as they are, not named (`--keep-other`).
    pub keep_other: Vec<String>,
    /// Other memory tools whose MCP entries are switched off (`--disable-other`; the only way
    /// besides a yes in the wizard: `--yes` alone never switches anything of another tool off).
    pub disable_other: Vec<String>,
    /// Other memory tools install switched off that are switched on again (`--enable-other`).
    pub enable_other: Vec<String>,
    /// The project whose own instruction files and MCP files are read for the "Other memory"
    /// findings (the current directory's; `None`: user scope only, as in tests).
    pub project_dir: Option<PathBuf>,
    /// Ask the wizard's questions (stdin and stdout are terminals, no `--yes`).
    pub interactive: bool,
    /// Colour in the wizard.
    pub color: bool,
}

/// Options of `uninstall`.
#[derive(Debug, Clone, Default)]
pub struct UninstallOptions {
    /// The harnesses to uninstall from (`--harness`; `None`: every harness it is installed
    /// in, Claude Code when none).
    pub harnesses: Option<Vec<Kind>>,
    /// Show the plan only.
    pub dry_run: bool,
    /// Do not ask.
    pub yes: bool,
    /// Delete the data directory too.
    pub purge: bool,
    /// Only unregister the develop server.
    pub disable_develop_only: bool,
    /// Do not export the memories back into the harness's file memory (`--no-export`).
    pub no_export: bool,
    /// Ask whether to export (stdin and stdout are terminals, no `--yes`).
    pub interactive: bool,
    /// Colour in the output (a terminal that supports it).
    pub color: bool,
}

/// One planned file change.
#[derive(Debug, Clone)]
pub(crate) struct Change {
    pub(crate) path: PathBuf,
    /// The new text; `None` deletes the file.
    pub(crate) after: Option<String>,
    /// Containers this change creates (dotted paths), for a later structural removal.
    pub(crate) created: Vec<String>,
    /// The separator written before the managed block.
    pub(crate) separator: Option<String>,
}

/// The state file of one harness: what install changed (`<data dir>/install/<harness>.json`,
/// `claude-code.json` or `opencode.json`): `{"version": 1, "installed_at", "updated_at",
/// "exe", "created_dirs": [...], "files": {"<path>": {"existed", "backup", "backup_valid",
/// "written", "created", "separator"}}}`.
#[derive(Debug, Clone, Default)]
pub(crate) struct State {
    kind: Kind,
    pub(crate) meta: serde_json::Map<String, Value>,
    files: std::collections::BTreeMap<String, serde_json::Map<String, Value>>,
    created_dirs: Vec<String>,
}

impl State {
    fn path(setup: &Setup) -> PathBuf {
        Self::path_of(setup, Kind::ClaudeCode)
    }

    pub(crate) fn path_of(setup: &Setup, kind: Kind) -> PathBuf {
        setup.state_dir().join(format!("{}.json", kind.id()))
    }

    fn load(setup: &Setup) -> Result<Self, String> {
        Self::load_of(setup, Kind::ClaudeCode)
    }

    /// The state of `kind`'s installation (empty when install never ran for it).
    pub(crate) fn load_of(setup: &Setup, kind: Kind) -> Result<Self, String> {
        let path = Self::path_of(setup, kind);
        let Some(bytes) = fsutil::read_optional(&path)? else {
            return Ok(Self {
                kind,
                ..Self::default()
            });
        };
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|err| format!("{} is invalid: {err}", path.display()))?;
        if value.get("version").and_then(Value::as_u64) != Some(FORMAT) {
            return Err(format!("{}: unsupported version", path.display()));
        }
        let mut meta = value.as_object().cloned().unwrap_or_default();
        let files = match meta.remove("files") {
            Some(Value::Object(files)) => files
                .into_iter()
                .filter_map(|(path, entry)| match entry {
                    Value::Object(entry) => Some((path, entry)),
                    _ => None,
                })
                .collect(),
            _ => Default::default(),
        };
        let created_dirs = match meta.remove("created_dirs") {
            Some(Value::Array(dirs)) => dirs
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        };
        Ok(Self {
            kind,
            meta,
            files,
            created_dirs,
        })
    }

    fn exists(setup: &Setup) -> bool {
        Self::path(setup).exists()
    }

    /// Whether install ran for `kind` (its state file exists).
    pub(crate) fn exists_of(setup: &Setup, kind: Kind) -> bool {
        Self::path_of(setup, kind).exists()
    }

    pub(crate) fn save(&mut self, setup: &Setup) -> Result<(), String> {
        let dir = setup.state_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
        let mut value = self.meta.clone();
        value.insert("version".into(), json!(FORMAT));
        value.insert("created_dirs".into(), json!(self.created_dirs));
        let files: serde_json::Map<String, Value> = self
            .files
            .iter()
            .map(|(path, entry)| (path.clone(), Value::Object(entry.clone())))
            .collect();
        value.insert("files".into(), Value::Object(files));
        let text = jsonedit::render(&Value::Object(value), Some("  "), 0);
        fsutil::write_atomic(
            &Self::path_of(setup, self.kind),
            format!("{text}\n").as_bytes(),
        )
    }

    pub(crate) fn file(&self, path: &Path) -> Option<&serde_json::Map<String, Value>> {
        self.files.get(&path.display().to_string())
    }

    /// Every file entry, by path.
    pub(crate) fn files(&self) -> impl Iterator<Item = (&String, &serde_json::Map<String, Value>)> {
        self.files.iter()
    }

    pub(crate) fn file_mut(&mut self, path: &Path) -> &mut serde_json::Map<String, Value> {
        self.files.entry(path.display().to_string()).or_default()
    }

    pub(crate) fn created_dirs(&self) -> Vec<PathBuf> {
        self.created_dirs.iter().map(PathBuf::from).collect()
    }

    fn add_created_dir(&mut self, dir: &Path) {
        let text = dir.display().to_string();
        if !self.created_dirs.contains(&text) {
            self.created_dirs.push(text);
        }
    }
}

/// What one install step will do, for the wizard and the report.
#[derive(Debug, Clone)]
pub(crate) struct Step {
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
}

/// The planned install: its steps in sections (shared, one per agent, then import and
/// indexing), each with its heading.
struct InstallPlan {
    sections: Vec<(String, Vec<Step>)>,
    /// Claude Code's file changes.
    changes: Vec<Change>,
    /// opencode's file changes.
    opencode_changes: Vec<Change>,
    /// Codex's file changes.
    codex_changes: Vec<Change>,
    /// What happens to Codex's memories (`crate::codex::install::Plan::memories`).
    codex_memories: Option<bool>,
    /// Cursor's file changes.
    cursor_changes: Vec<Change>,
    /// CodeWhale's file changes.
    codewhale_changes: Vec<Change>,
    /// The records of CodeWhale's `config.toml` state entry
    /// (`crate::codewhale::install::Plan::records`).
    codewhale_records: Vec<(&'static str, Option<Value>)>,
    conflicts: Vec<String>,
    imports: Vec<DirPlan>,
    write_config: bool,
    auto_memory: AutoMemoryPlan,
    /// Other memory tools' entries switched off now: (harness, file, name, record).
    other_off: Vec<(Kind, PathBuf, String, Value)>,
    /// Other memory tools' entries switched on again now (install had switched them off).
    other_on: Vec<(Kind, PathBuf, String)>,
}

pub(crate) fn read_text(path: &Path) -> Result<Option<String>, String> {
    match fsutil::read_optional(path)? {
        Some(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| format!("{} is not UTF-8 text", path.display())),
        None => Ok(None),
    }
}

/// `err` (about one agent's file) with the way out: fix that file and run the command again, or
/// leave that agent out with `--harness` (naming the other agents of `kinds`).
pub(crate) fn way_out(err: &str, kinds: &[Kind], command: &str, verb: &str) -> String {
    let owner = kinds.iter().find(|kind| {
        let agent_dir = |kind: &Kind| match kind {
            Kind::ClaudeCode => vec![".claude"],
            Kind::OpenCode => vec!["opencode"],
            Kind::Codex => vec![".codex", "codex"],
            Kind::Cursor => vec![".cursor"],
            Kind::CodeWhale => vec![".codewhale", ".deepseek", ".agents"],
        };
        agent_dir(kind).iter().any(|dir| err.contains(dir))
    });
    let others: Vec<&str> = kinds
        .iter()
        .filter(|kind| Some(*kind) != owner)
        .map(|kind| kind.id())
        .collect();
    let mut text = format!(
        "{err}; nothing was changed. Fix that file (or move it away) and run `graphersal-memory \
         {command}` again"
    );
    if let (Some(owner), false) = (owner, others.is_empty()) {
        text.push_str(&format!(
            "; or {verb} the other agents now and leave {} out: `graphersal-memory {command} \
             --harness <agent>` for {}",
            owner.display_name(),
            join_and(&others)
        ));
    }
    text
}

/// `err` naming the file it is about (`path` put before it unless it names it already).
pub(crate) fn in_file(path: &Path, err: String) -> String {
    let name = path.display().to_string();
    if err.contains(&name) {
        err
    } else {
        format!("{name}: {err}")
    }
}

/// An empty JSON object as the start of a file install creates.
const EMPTY_OBJECT: &str = "{\n}\n";

/// The text a JSON file's edit starts from: its text, or `empty` when it is missing or holds
/// only whitespace (an empty file is an empty configuration, not invalid JSON).
pub(crate) fn json_start(before: &Option<String>, empty: &str) -> String {
    match before {
        Some(text) if !text.trim().is_empty() => text.clone(),
        _ => empty.to_string(),
    }
}

/// Plans the change of the user configuration (the MCP servers).
fn plan_user_config(
    setup: &Setup,
    enable_develop: bool,
    conflicts: &mut Vec<String>,
) -> Result<(Step, Option<Change>), String> {
    let agent = setup.agent();
    let key = agent.mcp_servers_key();
    let path = agent.mcp_config_file();
    let before = read_text(path)?;
    let mut text = json_start(&before, EMPTY_OBJECT);
    let parsed: Value = serde_json::from_str(&text)
        .map_err(|err| format!("{} is not valid JSON: {err}", path.display()))?;
    let mut created = Vec::new();
    let mut lines = Vec::new();
    let mut servers = vec![(USE_SERVER, "use")];
    if enable_develop {
        servers.push((DEVELOP_SERVER, "develop"));
    }
    for (name, profile) in servers {
        let existing = &parsed[key][name];
        if !existing.is_null() && !setup.ours().entry(existing) {
            conflicts.push(format!(
                "{}: {key}.{name} exists and is not ours; kept. Remove it ({}) and run install \
                 again.",
                path.display(),
                agent.remove_server_hint(name)
            ));
            lines.push(format!(
                "{name}: CONFLICT (an entry of another program has this name)"
            ));
            continue;
        }
        let entry = setup.mcp_entry(profile);
        match jsonedit::set_member(&text, &[key], name, &entry)
            .map_err(|e| crate::install::in_file(path, e))?
        {
            Some((new, made)) => {
                lines.push(format!(
                    "{name} ({profile} profile): {}",
                    if existing.is_null() { "add" } else { "update" }
                ));
                text = new;
                created.extend(made);
            }
            None => lines.push(format!("{name} ({profile} profile): already registered")),
        }
    }
    if !enable_develop {
        let develop = &parsed[key][DEVELOP_SERVER];
        if develop.is_null() {
            lines.push(format!(
                "{DEVELOP_SERVER}: not registered (off; `graphersal-memory install \
                 --enable-develop` registers it)"
            ));
        } else if setup.ours().entry(develop)
            && let Some(new) = jsonedit::remove_member(&text, &[key], DEVELOP_SERVER)
                .map_err(|e| crate::install::in_file(path, e))?
        {
            text = new;
            lines.push(format!("{DEVELOP_SERVER}: unregister (you answered no)"));
        } else {
            lines.push(format!("{DEVELOP_SERVER}: registered (kept)"));
        }
    }
    let step = Step {
        title: format!(
            "Register the MCP servers at user scope in {} (key {key})",
            path.display()
        ),
        lines,
    };
    let change = (Some(&text) != before.as_ref()).then(|| Change {
        path: path.to_path_buf(),
        after: Some(text),
        created,
        separator: None,
    });
    Ok((step, change))
}

/// What install does with the harness's own memory switch.
#[derive(Debug, Clone, PartialEq)]
enum AutoMemoryPlan {
    /// Nothing.
    Keep,
    /// Switched off now; `previous` is what the setting was (`null`: absent).
    SwitchOff { previous: Value },
    /// Install's own switch-off is undone (or forgotten).
    Restore,
}

/// The record of install's switch-off in the settings file's state entry.
const AUTO_MEMORY_RECORD: &str = "auto_memory";

/// The sentence the auto-memory step and the summary show when it is switched off.
pub const NOTHING_IS_LOST: &str = "Nothing is lost: if you uninstall graphersal-memory later, \
    every memory, including new ones, is written back to Claude Code's memory files and its \
    auto-memory is switched back on.";

/// What the auto-memory question and the recap say about the start context: that the rules
/// and pinned memories still reach every session (with the numbers), or why the default keeps
/// Claude Code's own memory on.
fn start_context_sentence(current: &Current) -> String {
    match &current.start_blocked {
        Some(reason) => format!("The default is to keep it on: {reason}."),
        None if current.start_preview.total == 0 => "Your rules and pinned memories reach every \
            session through graphersal-memory's start context (none in your memory files yet; \
            `graphersal-memory start-context` shows it)."
            .to_string(),
        None => format!(
            "Your rules and pinned memories still reach every session through \
             graphersal-memory's start context: {} memor{}, {} characters{} \
             (`graphersal-memory start-context` shows it).",
            current.start_preview.total,
            if current.start_preview.total == 1 {
                "y"
            } else {
                "ies"
            },
            current.start_preview.chars(),
            if current.start_preview.not_shown() > 0 {
                format!(
                    ", {} over the budget not shown",
                    current.start_preview.not_shown()
                )
            } else {
                String::new()
            }
        ),
    }
}

/// Plans the change of the user settings (the hooks, and the auto-memory switch).
fn plan_settings(
    setup: &Setup,
    auto_memory_off: bool,
    start_note: &str,
    state: &State,
) -> Result<(Step, Option<Change>, AutoMemoryPlan), String> {
    let path = &setup.paths.settings;
    let before = read_text(path)?;
    let mut text = json_start(&before, EMPTY_OBJECT);
    let parsed: Value = serde_json::from_str(&text)
        .map_err(|err| format!("{} is not valid JSON: {err}", path.display()))?;
    let mut created = Vec::new();
    let mut lines = Vec::new();
    let ours = setup.ours();
    for (event, sub) in HOOK_EVENTS {
        let group = setup.hook_group(sub);
        let (new, made, step) = plan_hook_event(&ours, &text, event, &group)
            .map_err(|e| crate::install::in_file(path, e))?;
        text = new;
        created.extend(made);
        lines.push(match step {
            HookStep::Installed => format!("{event}: already installed"),
            HookStep::Added => format!("{event}: add `{}`", setup.claude_hook_text(sub)),
            HookStep::Updated => format!("{event}: update to `{}`", setup.claude_hook_text(sub)),
        });
    }
    let mut plan = AutoMemoryPlan::Keep;
    if let Some(key) = setup.agent().auto_memory_key() {
        let value = parsed.get(key).cloned();
        let recorded = state
            .file(path)
            .and_then(|entry| entry.get(AUTO_MEMORY_RECORD))
            .cloned();
        let off = value == Some(Value::Bool(false));
        if auto_memory_off {
            if off {
                lines.push(format!(
                    "{key}: false already (auto-memory off{})",
                    if recorded.is_some() {
                        ", switched off by install"
                    } else {
                        ", your own setting"
                    }
                ));
            } else if let Some((new, _)) = jsonedit::set_member(&text, &[], key, &json!(false))
                .map_err(|e| crate::install::in_file(path, e))?
            {
                text = new;
                lines.push(format!(
                    "{key}: false (switch Claude Code's auto-memory off). {start_note} \
                     {NOTHING_IS_LOST}"
                ));
                plan = AutoMemoryPlan::SwitchOff {
                    previous: recorded
                        .as_ref()
                        .map(|r| r["previous"].clone())
                        .unwrap_or_else(|| value.clone().unwrap_or(Value::Null)),
                };
            }
        } else if let Some(record) = recorded {
            if off {
                let previous = &record["previous"];
                let restored = if previous.is_null() {
                    jsonedit::remove_member(&text, &[], key)
                        .map_err(|e| crate::install::in_file(path, e))?
                } else {
                    jsonedit::set_member(&text, &[], key, previous)?.map(|(new, _)| new)
                };
                if let Some(new) = restored {
                    text = new;
                }
                lines.push(format!(
                    "{key}: back to how it was before install (auto-memory on)"
                ));
            } else {
                lines.push(format!("{key}: auto-memory on (kept)"));
            }
            plan = AutoMemoryPlan::Restore;
        } else {
            lines.push(format!(
                "{key}: kept as it is (auto-memory {}){}",
                if off { "off" } else { "on" },
                if off {
                    String::new()
                } else {
                    format!(". {start_note}")
                }
            ));
        }
    }
    let step = Step {
        title: format!(
            "Add the SessionStart and SessionEnd hooks in {}",
            path.display()
        ),
        lines,
    };
    let change = (Some(&text) != before.as_ref()).then(|| Change {
        path: path.clone(),
        after: Some(text),
        created,
        separator: None,
    });
    Ok((step, change, plan))
}

/// Plans the managed block in the instruction file.
fn plan_block(
    setup: &Setup,
    language: Language,
    read_only: &[String],
    conflicts: &mut Vec<String>,
) -> Result<(Step, Option<Change>), String> {
    let agent = setup.agent();
    let path = &agent.instruction_file().to_path_buf();
    let before = read_text(path)?;
    let text = before.clone().unwrap_or_default();
    let title = format!("Write the managed rule block into {}", path.display());
    let mut separator = None;
    let after = match find_block_with(&text, language, read_only) {
        Err(err) => {
            conflicts.push(format!(
                "{}: {err}; kept as it is. Remove the begin line or add the end line, then run \
                 install again.",
                path.display()
            ));
            None
        }
        Ok(Some(found)) if !found.intact => {
            conflicts.push(format!(
                "{}: the managed block was edited by hand; kept as it is. Move your text out of \
                 the block (or delete the block) and run install again.",
                path.display()
            ));
            None
        }
        Ok(Some(found)) if found.current => None,
        Ok(Some(found)) if !found.known => {
            conflicts.push(format!(
                "{}: the managed block was written by another (newer?) version of \
                 graphersal-memory; kept as it is. Run that version's install, or delete the \
                 block and run install again.",
                path.display()
            ));
            None
        }
        Ok(Some(found)) => Some(format!(
            "{}{}{}",
            &text[..found.start],
            line_ends_like(&text, &block_text_with(language, read_only)),
            &text[found.end..]
        )),
        Ok(None) => {
            let sep = block_separator(&text);
            separator = Some(sep.to_string());
            Some(format!(
                "{text}{sep}{}",
                line_ends_like(&text, &block_text_with(language, read_only))
            ))
        }
    };
    let line = match (&after, find_block_with(&text, language, read_only)) {
        (_, Err(_)) | (None, Ok(Some(FoundBlock { known: false, .. }))) => {
            "CONFLICT (see the report)".to_string()
        }
        (None, _) => "already written".to_string(),
        (Some(_), Ok(Some(_))) => "update the block to this version".to_string(),
        (Some(_), Ok(None)) => format!(
            "{} the block (your own text stays as it is)",
            if before.is_some() {
                "append"
            } else {
                "create the file with"
            }
        ),
    };
    let change = after.map(|after| Change {
        path: path.clone(),
        after: Some(after),
        created: Vec::new(),
        separator,
    });
    Ok((
        Step {
            title,
            lines: vec![line],
        },
        change,
    ))
}

/// What is installed and configured now: the current answers of the wizard.
struct Current {
    /// Whether install ran before (its state file exists).
    installed: bool,
    /// The settings file.
    config: Config,
    /// The current answer to the auto-memory question on an installed system (whether the
    /// harness's memory is off now), `None` before the first install.
    auto_memory_answer: Option<bool>,
    /// Whether the harness's own memory is off (by anyone).
    auto_memory_off: bool,
    /// Whether the user switched it off themselves (not install).
    auto_memory_off_by_user: bool,
    /// Whether our develop server is registered.
    develop_registered: bool,
    /// The file-memory import still to do: (directories, new + changed files).
    import_pending: (usize, usize),
    /// The harnesses found on this machine (Claude Code when none is found).
    detected: Vec<Kind>,
    /// Whether Codex's own memories are on (`[features] memories = true`).
    codex_memories_on: bool,
    /// Whether CodeWhale's native memory is on.
    codewhale_memory_on: bool,
    /// CodeWhale's native memory to import (its reviewed entries), when it has a store.
    codewhale_native: Option<crate::codewhale::memory::Native>,
    /// The start context the memory files give (the project with the most always-loaded
    /// memories), within the budget of the settings.
    start_preview: crate::start_context::StartContext,
    /// Why the start context cannot carry the always-loaded memories, when it cannot.
    start_blocked: Option<String>,
    /// Other memory on this machine (M15).
    others: crate::coexist::Findings,
    /// The recorded choices about other memory tools (an earlier install's).
    other_choices: Option<crate::coexist::Choices>,
    /// The other tools' importable data.
    other_sources: Vec<crate::coexist::other_import::Source>,
    /// Tools found whose memory is not importable (or whose data was not found).
    not_importable: Vec<String>,
    /// The other tools' memories not imported yet (new or changed items).
    other_pending: usize,
}

/// The start context Claude Code's memory files give once imported: per project its pinned,
/// feedback and user memories, rendered with `budget`; the project with the most of them.
pub fn start_preview(
    imports: &[import::DirPlan],
    budget: usize,
) -> crate::start_context::StartContext {
    imports
        .iter()
        .map(|plan| {
            let entries = plan
                .dir
                .files
                .iter()
                .filter(|f| f.pinned || f.kind == "feedback" || f.kind == "user")
                .map(|f| crate::start_context::Entry {
                    slug: f.slug.clone(),
                    scope: "project".to_string(),
                    kind: f.kind.clone(),
                    pinned: f.pinned,
                    order: None,
                    title: if f.title.is_empty() {
                        f.slug.clone()
                    } else {
                        f.title.clone()
                    },
                    hook: if f.hook.is_empty() {
                        f.summary.clone()
                    } else {
                        f.hook.clone()
                    },
                    updated_at: String::new(),
                })
                .collect();
            crate::start_context::render(entries, budget)
        })
        .max_by_key(|context| (context.total, context.chars()))
        .unwrap_or_else(|| crate::start_context::render(Vec::new(), budget))
}

/// Why the start context cannot carry the always-loaded memories into Claude Code's sessions
/// (then the auto-memory question defaults to keeping Claude Code's own memory on), or `None`.
/// Install adds the SessionStart hook, which delivers it; without the hook the MCP instructions
/// carry what fits Claude Code's limit for them.
pub fn start_context_blocked(config: &Config) -> Option<String> {
    (config.start_context_chars() == 0).then(|| {
        "memory.start_context_chars is 0 in config.json, so graphersal-memory loads no memories \
         at session start"
            .to_string()
    })
}

impl Current {
    fn read(setup: &Setup, project: Option<&Path>) -> Result<Self, String> {
        let state = State::load(setup)?;
        let codex_state = State::load_of(setup, Kind::Codex)?;
        let config = Config::load(&setup.data_dir)?;
        let agent = setup.agent();
        let mut errors = Vec::new();
        let settings = read_json(agent.settings_file(), &mut errors);
        let user_config = read_json(agent.mcp_config_file(), &mut errors);
        let off = agent
            .auto_memory_key()
            .is_some_and(|key| settings[key] == Value::Bool(false));
        let recorded = state
            .file(agent.settings_file())
            .is_some_and(|entry| entry.contains_key(AUTO_MEMORY_RECORD));
        let codex_memories_on = read_text(&setup.codex.config_file)
            .ok()
            .flatten()
            .and_then(|text| crate::tomledit::parse(&text).ok())
            .is_some_and(|doc| crate::codex::install::memories_on(&doc));
        let codewhale_state = State::load_of(setup, Kind::CodeWhale)?;
        let codewhale_memory_on = crate::codewhale::memory_on(
            &read_text(&setup.codewhale.config_file)
                .ok()
                .flatten()
                .and_then(|text| crate::tomledit::parse(&text).ok())
                .unwrap_or(Value::Null),
            setup.codewhale.memory_env_on,
        );
        let codewhale_native = (setup.program(Kind::CodeWhale).is_some()
            || State::exists_of(setup, Kind::CodeWhale))
        .then(|| {
            crate::codewhale::memory::Native::load(
                &setup.codewhale.memory_store,
                &setup.data_dir,
                project,
            )
        })
        .flatten();
        // On an installed system the current answer is what is true now: a user who switched
        // the harness's memory back on after install keeps it on (unless they answer otherwise).
        let auto_memory_answer = if state.meta.contains_key("auto_memory_off") {
            Some(off)
        } else if codex_state.meta.contains_key("auto_memory_off") {
            Some(!codex_memories_on)
        } else if codewhale_state.meta.contains_key("auto_memory_off") {
            Some(!codewhale_memory_on)
        } else {
            None
        };
        let imports = import::plan(
            import::scan(agent.projects_dir())?,
            &ImportState::load(&setup.data_dir)?,
        );
        let (with_project, _, new, changed, _) = import::totals(&imports);
        let start_preview = start_preview(&imports, config.start_context_chars());
        let start_blocked = start_context_blocked(&config);
        let others = crate::coexist::detect(setup, project);
        let (other_sources, not_importable) = crate::coexist::other_import::sources(setup, &others);
        let other_state = crate::coexist::other_import::State::load(&setup.data_dir);
        let other_pending = other_sources
            .iter()
            .map(|source| other_state.pending_count(source))
            .sum::<usize>()
            + codewhale_native
                .as_ref()
                .map_or(0, |native| native.pending(&setup.data_dir));
        Ok(Self {
            other_pending,
            start_preview,
            start_blocked,
            other_choices: crate::coexist::Choices::load(setup),
            others,
            other_sources,
            not_importable,
            installed: State::exists(setup),
            auto_memory_answer,
            auto_memory_off: off,
            auto_memory_off_by_user: off && !recorded,
            develop_registered: setup
                .ours()
                .entry(&user_config["mcpServers"][DEVELOP_SERVER]),
            import_pending: (with_project, new + changed),
            detected: {
                let found = setup.detected();
                if found.is_empty() {
                    vec![Kind::ClaudeCode]
                } else {
                    found
                }
            },
            config,
            codex_memories_on,
            codewhale_memory_on,
            codewhale_native,
        })
    }

    /// Whether the auto-memory question applies: Claude Code may be chosen and its auto-memory
    /// was not switched off by the user, or Codex (CodeWhale) is among `possible` and its own
    /// memory is on.
    fn ask_auto_memory(&self, claude_possible: bool, possible: &[Kind]) -> bool {
        (claude_possible && !self.auto_memory_off_by_user) || self.own_memory_on(possible)
    }

    /// Whether Codex or CodeWhale is among `kinds` with its own memory on.
    fn own_memory_on(&self, kinds: &[Kind]) -> bool {
        (kinds.contains(&Kind::Codex) && self.codex_memories_on)
            || (kinds.contains(&Kind::CodeWhale) && self.codewhale_memory_on)
    }

    /// Whether the wizard's "Other memory" step applies for the harnesses `kinds`: another
    /// memory tool's user-scope entry there, or memories of another tool to import.
    fn other_step(&self, kinds: &[Kind]) -> bool {
        !self.others.names(kinds).is_empty() || self.other_pending > 0
    }
}

/// The wizard's answers: what install makes true.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answers {
    /// The harnesses to install into.
    pub harnesses: Vec<Kind>,
    /// The memory language.
    pub language: Language,
    /// The embedding model: a repository or `hash-ngram/1`.
    pub model: String,
    /// Import the file memory now.
    pub import: bool,
    /// Switch the harness's own memory off.
    pub auto_memory_off: bool,
    /// Session summaries on.
    pub summaries: bool,
    /// The develop server registered.
    pub develop: bool,
    /// Other memory tools the rule block names read-only (graphersal-memory is the main memory).
    pub read_only: Vec<String>,
    /// Other memory tools kept as they are (not named).
    pub kept: Vec<String>,
    /// Other memory tools whose MCP entries are (or stay) switched off.
    pub disable_other: Vec<String>,
    /// Import the other tools' memories whose format is known.
    pub import_other: bool,
}

/// The model the answers default to: the flag, else the current model (unless the language
/// changed), else the language's model.
fn default_model(options: &InstallOptions, current: &Current, language: Language) -> String {
    if let Some(model) = &options.model {
        return model.clone();
    }
    match &current.config.model {
        Some(model) if current.config.language() == language => model.clone(),
        _ => models::default_for(language).to_string(),
    }
}

/// The answers without questions: every flag, else the current answer, else the default
/// (summaries and the develop server: off unless their flag or the current state says on).
fn default_answers(options: &InstallOptions, current: &Current) -> Answers {
    let language = options
        .language
        .or(current.config.language)
        .unwrap_or_default();
    let harnesses = options
        .harnesses
        .clone()
        .unwrap_or_else(|| current.detected.clone());
    let others = other_defaults(options, current, &harnesses);
    Answers {
        read_only: others.read_only,
        kept: others.kept,
        disable_other: others.disable,
        import_other: !options.no_import,
        harnesses,
        model: default_model(options, current, language),
        language,
        import: !options.no_import,
        // Off only when the start context carries the always-loaded memories instead.
        auto_memory_off: !options.keep_auto_memory
            && current.start_blocked.is_none()
            && current
                .auto_memory_answer
                .unwrap_or(!current.auto_memory_off_by_user),
        summaries: options.summaries || current.config.summaries.enabled,
        develop: options.enable_develop || current.develop_registered,
    }
}

/// What happens to each other memory tool without questions.
struct OtherDefaults {
    read_only: Vec<String>,
    kept: Vec<String>,
    disable: Vec<String>,
}

/// The defaults for the other memory tools of `harnesses`: switched off only by its flag (or
/// when an earlier install switched it off and `--enable-other` does not name it; `--yes`
/// alone never switches anything of another tool off); else named read-only by its flag, kept
/// by its flag, else the earlier choice, else read-only for a known memory tool and kept for one
/// recognised only by its name or tools.
fn other_defaults(
    options: &InstallOptions,
    current: &Current,
    harnesses: &[Kind],
) -> OtherDefaults {
    use crate::coexist::ServerState;
    let mut out = OtherDefaults {
        read_only: Vec::new(),
        kept: Vec::new(),
        disable: Vec::new(),
    };
    for name in current.others.names(harnesses) {
        let servers: Vec<&crate::coexist::Server> = current
            .others
            .user_servers(harnesses)
            .into_iter()
            .filter(|s| s.name == name)
            .collect();
        let off_by_us = servers.iter().any(|s| s.state == ServerState::OffByUs);
        let disable = options.disable_other.contains(&name)
            || (off_by_us && !options.enable_other.contains(&name));
        if disable {
            out.disable.push(name);
            continue;
        }
        if servers.iter().all(|s| s.state == ServerState::Off) {
            continue;
        }
        let choices = current.other_choices.as_ref();
        let read_only = if options.other_read_only.contains(&name) {
            true
        } else if options.keep_other.contains(&name) {
            false
        } else if let Some(choices) =
            choices.filter(|c| c.read_only.contains(&name) || c.kept.contains(&name))
        {
            choices.read_only.contains(&name)
        } else {
            servers.iter().any(|s| s.main_by_default())
        };
        if read_only {
            out.read_only.push(name);
        } else {
            out.kept.push(name);
        }
    }
    out
}

/// The interactive wizard: one question per step that applies, with the current answers (or
/// the flags) as defaults. `Err` when the input ended before a question without a default was
/// answered.
fn ask(
    setup: &Setup,
    options: &InstallOptions,
    current: &Current,
    console: &mut Console<'_>,
) -> Result<Answers, crate::prompt::NoAnswer> {
    let agent = setup.agent();
    let name = agent.display_name();
    let defaults = default_answers(options, current);
    let ask_harnesses = options.harnesses.is_none() && current.detected.len() > 1;
    // The Claude Code questions are asked when Claude Code may be chosen.
    let claude_possible = defaults.harnesses.contains(&Kind::ClaudeCode) || ask_harnesses;
    let codex_possible: Vec<Kind> = if ask_harnesses {
        current.detected.clone()
    } else {
        defaults.harnesses.clone()
    };
    let ask_import = claude_possible && current.import_pending.1 > 0;
    let ask_auto_memory = current.ask_auto_memory(claude_possible, &codex_possible);
    let total = wizard_steps(options, current, claude_possible, &codex_possible);
    let mut n = 0;
    let mut next = || {
        n += 1;
        n
    };

    let harnesses = if ask_harnesses {
        console.step(
            next(),
            total,
            "Which agents",
            &[
                "graphersal-memory found these agent harnesses on this machine. Every one you",
                "choose gets the same memory: one store and one background daemon, so a project's",
                "memory is the same from each agent.",
            ],
        );
        let mut choices = vec![Choice::new("All of them", names_of(&current.detected))];
        for kind in &current.detected {
            choices.push(Choice::new(format!("{} only", kind.display_name()), ""));
        }
        match console.choose(&choices, Some(0))? {
            0 => current.detected.clone(),
            i => vec![current.detected[i - 1]],
        }
    } else {
        defaults.harnesses.clone()
    };
    let claude = harnesses.contains(&Kind::ClaudeCode);
    // Steps counted while another harness could be chosen but not asked for this choice.
    let total = total
        - skipped_steps(
            options,
            current,
            claude_possible,
            &codex_possible,
            &harnesses,
        );

    console.step(
        next(),
        total,
        "Memory language",
        &[
            "Memories are found by meaning. Keeping them in one language lets a search find a",
            "memory whatever language you wrote in; or keep each memory in your own words.",
        ],
    );
    let language = if console.choose(
        &[
            Choice::new(
                "English only",
                "the agent stores and searches in English\n(it translates a query before searching)",
            ),
            Choice::new(
                "Any language",
                "the agent stores in the language of the conversation\n(and searches in it and in English)",
            ),
        ],
        Some(usize::from(defaults.language == Language::Any)),
    )? == 0
    {
        Language::English
    } else {
        Language::Any
    };

    console.step(
        next(),
        total,
        "Embedding model",
        &[
            "The model turns every memory into a vector, so a search finds related wording,",
            "not only the same words. Downloaded once from Hugging Face; it then runs locally.",
        ],
    );
    let default_model = if options.model.is_some() || current.config.model.is_some() {
        default_model(options, current, language)
    } else {
        models::default_for(language).to_string()
    };
    let mut choices = Vec::new();
    let mut repos: Vec<String> = Vec::new();
    for offered in models::OFFERED {
        let cached = models::cached(&setup.data_dir, offered.repo).is_some();
        choices.push(Choice::new(
            models::short_name(offered.repo),
            format!(
                "{}, {}, {}{}",
                offered.languages,
                offered.size,
                offered.license,
                if cached { " (downloaded)" } else { "" }
            ),
        ));
        repos.push(offered.repo.to_string());
    }
    if default_model != models::BUILTIN && models::offered(&default_model).is_none() {
        choices.push(Choice::new(
            models::short_name(&default_model),
            format!("{default_model} (your choice)"),
        ));
        repos.push(default_model.clone());
    }
    choices.push(Choice::new(
        "Built-in (no download)",
        "works offline, but finds only shared words and spellings,\nnot meaning; weaker search",
    ));
    repos.push(models::BUILTIN.to_string());
    let default_index = repos.iter().position(|repo| *repo == default_model);
    let model = repos[console.choose(&choices, default_index)?].clone();

    let import = if ask_import && claude {
        let (dirs, files) = current.import_pending;
        console.step(
            next(),
            total,
            "Import existing file memory",
            &[
                &format!(
                    "{files} memory file{} of {name}'s own memory (MEMORY.md and the files beside",
                    if files == 1 { "" } else { "s" }
                ),
                &format!(
                    "it) in {dirs} project{} are not in the graph yet. They stay per project, as",
                    if dirs == 1 { "" } else { "s" }
                ),
                &format!("{name} keeps them. The files are read, never changed."),
            ],
        );
        console.yes_no(
            "import them now",
            "not now (the session start hook still imports the files of the\nproject you work in)",
            Some(defaults.import),
        )?
    } else {
        defaults.import
    };

    let claude_memory = claude && !current.auto_memory_off_by_user;
    let codex_memory = harnesses.contains(&Kind::Codex) && current.codex_memories_on;
    let codewhale_memory = harnesses.contains(&Kind::CodeWhale) && current.codewhale_memory_on;
    let codewhale_config = setup.show(&setup.codewhale.config_file.display().to_string());
    let auto_memory_off = if ask_auto_memory && claude_memory {
        let mut lines = vec![
            format!("{name} also writes its own memory files and loads them into every session;"),
            "with graphersal-memory both would keep the same facts twice.".to_string(),
            start_context_sentence(current),
            NOTHING_IS_LOST.to_string(),
        ];
        if codex_memory {
            lines.push(
                "The same answer switches Codex's own memories off ([features] memories in"
                    .to_string(),
            );
            lines.push(format!(
                "{}); uninstall switches them on again.",
                setup.show(&setup.codex.config_file.display().to_string())
            ));
        }
        if codewhale_memory {
            lines.push(format!(
                "The same answer switches CodeWhale's own memory off ([memory] enabled in {}),",
                codewhale_config
            ));
            lines.push(
                "after importing its reviewed entries; uninstall switches it on again.".to_string(),
            );
        }
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        console.step(next(), total, &format!("{name}'s auto-memory"), &lines);
        console.yes_no(
            &format!(
                "switch it off ({} false in {})",
                agent.auto_memory_key().unwrap_or_default(),
                setup.show(&agent.settings_file().display().to_string())
            ),
            "keep it on (the files it writes are imported at each session start)",
            Some(defaults.auto_memory_off),
        )?
    } else if ask_auto_memory && codex_memory {
        let mut lines = vec![
            "Codex's own memory feature is on: it generates memories from your chats and",
            "loads them into sessions; with graphersal-memory both would keep the same facts",
            "twice.",
            crate::codex::install::NOTHING_IS_LOST,
        ];
        let codewhale_line = format!(
            "The same answer switches CodeWhale's own memory off ([memory] enabled in {codewhale_config})."
        );
        if codewhale_memory {
            lines.push(&codewhale_line);
        }
        console.step(next(), total, "Codex's memories", &lines);
        console.yes_no(
            &format!(
                "switch it off ([features] memories = false in {})",
                setup.show(&setup.codex.config_file.display().to_string())
            ),
            "keep it on",
            Some(defaults.auto_memory_off),
        )?
    } else if ask_auto_memory && codewhale_memory {
        console.step(
            next(),
            total,
            "CodeWhale's memory",
            &[
                "CodeWhale's own memory is on: it keeps notes in its own store and loads them",
                "into sessions; with graphersal-memory both would keep the same facts twice.",
                crate::codewhale::install::NOTHING_IS_LOST,
            ],
        );
        console.yes_no(
            &format!("switch it off ([memory] enabled = false in {codewhale_config})"),
            "keep it on",
            Some(defaults.auto_memory_off),
        )?
    } else {
        defaults.auto_memory_off
    };

    let others = ask_other_memory(
        setup,
        options,
        current,
        &harnesses,
        defaults.import_other,
        console,
        total,
        &mut next,
    )?;

    let mut summary_lines = vec![
        "Past sessions are indexed locally (their raw turns, no model call). Optionally each"
            .to_string(),
        format!(
            "finished session is summarized through your own `{}`",
            summaries_command(&current.config, &harnesses).join(" ")
        ),
        if crate::harness::costs_nothing(&summaries_command(&current.config, &harnesses)) {
            "(decisions, corrections, outcomes, open threads). With this free model:".to_string()
        } else {
            "(decisions, corrections, outcomes, open threads). That costs model usage:".to_string()
        },
        summaries_estimate(setup, &current.config, &harnesses),
    ];
    if harnesses == [Kind::Cursor] {
        summary_lines.push(
            "Cursor's own CLI is not used for summaries (it reports no cost); say yes only if"
                .to_string(),
        );
        summary_lines.push("the command above is installed and logged in.".to_string());
    }
    let summary_lines: Vec<&str> = summary_lines.iter().map(String::as_str).collect();
    console.step(next(), total, "Session summaries", &summary_lines);
    let summaries = console.yes_no(
        &format!(
            "summarize, at most ${:.2} per month",
            current.config.summaries.monthly_budget_usd
        ),
        "raw turns only (`graphersal-memory summaries enable` later)",
        (current.installed || options.summaries).then_some(defaults.summaries),
    )?;

    console.step(
        next(),
        total,
        "The develop server",
        &[
            "A second MCP server with tools to inspect and reshape the memory's structure",
            "(schema, saved queries, indexes). Most people do not need it; its tool",
            "descriptions are paid on every agent turn.",
        ],
    );
    let develop = console.yes_no(
        &format!("register {DEVELOP_SERVER}"),
        "not now (`graphersal-memory install --enable-develop` later)",
        (current.installed || options.enable_develop).then_some(defaults.develop),
    )?;
    Ok(Answers {
        harnesses,
        language,
        model,
        import,
        auto_memory_off,
        summaries,
        develop,
        read_only: others.0.read_only,
        kept: others.0.kept,
        disable_other: others.0.disable,
        import_other: others.1,
    })
}

/// The wizard's "Other memory" step (when it applies for `harnesses`): what was found, then per
/// other memory tool the choice (graphersal-memory is the main memory and the tool read-only;
/// switch it off; keep both), and whether to import the memories of a tool whose format is
/// known. Without the step: the defaults.
#[allow(clippy::too_many_arguments)]
fn ask_other_memory(
    setup: &Setup,
    options: &InstallOptions,
    current: &Current,
    harnesses: &[Kind],
    import_default: bool,
    console: &mut Console<'_>,
    total: usize,
    next: &mut dyn FnMut() -> usize,
) -> Result<(OtherDefaults, bool), crate::prompt::NoAnswer> {
    use crate::coexist::ServerState;
    let defaults = other_defaults(options, current, harnesses);
    if !current.other_step(harnesses) {
        return Ok((defaults, import_default));
    }
    let mut lines = vec![
        "graphersal-memory found other memory on this machine. Two memories split the facts:"
            .to_string(),
        "an agent saves into one and later searches the other.".to_string(),
    ];
    for server in current.others.user_servers(harnesses) {
        lines.push(format!(
            "- {} ({}): {}{}",
            server.name,
            server.place(setup),
            server.what(),
            match server.state {
                ServerState::Off => ", switched off by you",
                ServerState::OffByUs => ", switched off by graphersal-memory install",
                ServerState::On => "",
            }
        ));
    }
    for mention in current.others.mentions.iter().take(5) {
        lines.push(format!(
            "- {}:{} says \"{}\" (never edited; `graphersal-memory doctor` suggests an edit)",
            setup.show(&mention.file.display().to_string()),
            mention.line,
            mention.text
        ));
    }
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    console.step(next(), total, "Other memory", &lines);
    let mut out = OtherDefaults {
        read_only: Vec::new(),
        kept: Vec::new(),
        disable: Vec::new(),
    };
    for name in current.others.names(harnesses) {
        let servers: Vec<&crate::coexist::Server> = current
            .others
            .user_servers(harnesses)
            .into_iter()
            .filter(|s| s.name == name)
            .collect();
        let main = servers.iter().any(|s| s.main_by_default());
        let mut files: Vec<String> = Vec::new();
        for server in &servers {
            let file = setup.show(&server.file.display().to_string());
            if !files.contains(&file) {
                files.push(file);
            }
        }
        if servers.iter().any(|s| s.state == ServerState::OffByUs) {
            let _ = writeln!(console.out(), "  {name}:");
            if console.yes_no(
                &format!("keep {name} switched off (uninstall switches it on again)"),
                &format!("switch {name} on again now"),
                Some(defaults.disable.contains(&name)),
            )? {
                out.disable.push(name);
            } else if main {
                out.read_only.push(name);
            } else {
                out.kept.push(name);
            }
            continue;
        }
        if servers.iter().all(|s| s.state == ServerState::Off) {
            continue;
        }
        let default = if defaults.disable.contains(&name) {
            1
        } else if defaults.read_only.contains(&name) {
            0
        } else {
            2
        };
        let _ = writeln!(console.out(), "  {name}:");
        let choice = console.choose(
            &[
                Choice::new(
                    "graphersal-memory is the main memory",
                    format!(
                        "{name} stays, named read-only in the rule block\n(search it when \
                         needed, never write there)"
                    ),
                ),
                Choice::new(
                    format!("Switch {name} off"),
                    format!(
                        "in {}; backed up, uninstall switches it on again",
                        files.join(" and ")
                    ),
                ),
                Choice::new(
                    "Keep both as they are",
                    "the rule block does not name it".to_string(),
                ),
            ],
            Some(default),
        )?;
        match choice {
            0 => out.read_only.push(name),
            1 => out.disable.push(name),
            _ => out.kept.push(name),
        }
    }
    let mut import = import_default;
    if current.other_pending > 0 {
        let mut names: Vec<String> = current
            .other_sources
            .iter()
            .map(|s| s.server.clone())
            .collect();
        if current
            .codewhale_native
            .as_ref()
            .is_some_and(|native| native.pending(&setup.data_dir) > 0)
        {
            names.push("CodeWhale's memory".to_string());
        }
        let _ = writeln!(console.out(), "  Import:");
        import = console.yes_no(
            &format!(
                "import {} memor{} of {} now (the files are only read)",
                current.other_pending,
                if current.other_pending == 1 {
                    "y"
                } else {
                    "ies"
                },
                names.join(", ")
            ),
            "not now (`graphersal-memory install` offers it again)",
            Some(import_default),
        )?;
    }
    Ok((out, import))
}

/// The number of wizard steps (the summary included): the harness question when more than
/// one harness is found and `--harness` is not given, the import and auto-memory questions when
/// Claude Code may be chosen.
fn wizard_steps(
    options: &InstallOptions,
    current: &Current,
    claude_possible: bool,
    codex_possible: &[Kind],
) -> usize {
    let ask_harnesses = options.harnesses.is_none() && current.detected.len() > 1;
    let import = claude_possible && current.import_pending.1 > 0;
    let auto = current.ask_auto_memory(claude_possible, codex_possible);
    let others = current.other_step(&possible_harnesses(options, current));
    4 + usize::from(ask_harnesses)
        + usize::from(import)
        + usize::from(auto)
        + usize::from(others)
        + 1
}

/// The harnesses the wizard may install into: the flag's, else every one found.
fn possible_harnesses(options: &InstallOptions, current: &Current) -> Vec<Kind> {
    options
        .harnesses
        .clone()
        .unwrap_or_else(|| current.detected.clone())
}

/// The steps [`wizard_steps`] counted that the choice of `chosen` harnesses leaves out: the
/// import and the auto-memory question when Claude Code (or, for the memories, Codex) is not
/// chosen.
fn skipped_steps(
    options: &InstallOptions,
    current: &Current,
    claude_possible: bool,
    codex_possible: &[Kind],
    chosen: &[Kind],
) -> usize {
    let others_counted = current.other_step(&possible_harnesses(options, current));
    let others_skipped = usize::from(others_counted && !current.other_step(chosen));
    let claude = chosen.contains(&Kind::ClaudeCode);
    let import_counted = claude_possible && current.import_pending.1 > 0;
    let auto_counted = current.ask_auto_memory(claude_possible, codex_possible);
    let auto_asked = (claude && !current.auto_memory_off_by_user) || current.own_memory_on(chosen);
    usize::from(import_counted && !claude)
        + usize::from(auto_counted && !auto_asked)
        + others_skipped
}

/// The command the summaries run through for `harnesses`: the settings file's, except that
/// without Claude Code the default `claude -p` becomes opencode's headless command (with a
/// free model) when opencode is chosen, else Codex's (`codex exec`), else CodeWhale's
/// (`codewhale exec`).
pub fn summaries_command(config: &Config, harnesses: &[Kind]) -> Vec<String> {
    let command = &config.summaries.harness_command;
    let claude_default = crate::config::split_command(crate::config::DEFAULT_HARNESS_COMMAND)
        .is_ok_and(|default| default == *command);
    let other = if !claude_default || harnesses.contains(&Kind::ClaudeCode) {
        None
    } else if harnesses.contains(&Kind::OpenCode) {
        Some(crate::opencode::DEFAULT_HEADLESS_COMMAND)
    } else if harnesses.contains(&Kind::Codex) {
        Some(crate::codex::DEFAULT_HEADLESS_COMMAND)
    } else if harnesses.contains(&Kind::CodeWhale) {
        Some(crate::codewhale::DEFAULT_HEADLESS_COMMAND)
    } else {
        None
    };
    match other {
        Some(other) => crate::config::split_command(other).unwrap_or_default(),
        None => command.clone(),
    }
}

/// The summaries' cost estimate line for the sessions of `harnesses`, or why there is none.
fn summaries_estimate(setup: &Setup, config: &Config, harnesses: &[Kind]) -> String {
    let mut total = crate::sessions::Estimate::default();
    for kind in harnesses {
        let dir = setup.agent_of(*kind).projects_dir().to_path_buf();
        match crate::sessions::estimate_for(
            *kind,
            &dir,
            &setup.data_dir,
            crate::sessions::IDLE,
            config.summaries.since.as_deref(),
        ) {
            Ok(e) => {
                total.sessions += e.sessions;
                total.pending += e.pending;
                total.too_short += e.too_short;
                total.usd += e.usd;
            }
            Err(err) => return format!("(no estimate: {err})"),
        }
    }
    let command = summaries_command(config, harnesses);
    crate::commands::estimate_line_for(&total, &command)
}

/// The plan's lines about the embedding model.
fn model_lines(setup: &Setup, current: &Current, model: &str) -> Vec<String> {
    let mut lines = Vec::new();
    if model == models::BUILTIN {
        lines.push(
            "the built-in embedder hash-ngram/1 (no download; finds shared words and spellings, \
             not meaning)"
                .to_string(),
        );
        lines.push(format!(
            "the check of memories against your instruction files needs an embedding model: off \
             with the built-in embedder (turn it on: `graphersal-memory install --model {}`)",
            models::ENGLISH_MODEL
        ));
        if current
            .config
            .model
            .as_deref()
            .is_some_and(|m| m != models::BUILTIN)
        {
            lines.push(
                "the memory goes back to it (re-embedded by the daemon in the background)"
                    .to_string(),
            );
        }
        return lines;
    }
    let size = models::offered(model)
        .map(|o| format!("{}, {}", o.size, o.license))
        .unwrap_or_else(|| "size and licence from Hugging Face".to_string());
    match models::cached(&setup.data_dir, model) {
        Some(cached) => lines.push(format!(
            "{model} ({}, {}): downloaded already",
            download::human_bytes(cached.bytes),
            models::licence_name(&cached.license)
        )),
        None => lines.push(format!(
            "download {model} ({size}) from {} into {} (the one network access of install; if \
             it fails, the built-in embedder stays and install goes on)",
            setup.source.endpoint,
            models::cache_dir(&setup.data_dir).display()
        )),
    }
    if current.config.model.as_deref() != Some(model) {
        lines.push(
            "the daemon imports it into the store and re-embeds every memory, session, turn and \
             fact with it in the background (calls wait while it runs)"
                .to_string(),
        );
    }
    lines
}

/// Plans the whole install for `answers`.
fn plan_install(
    setup: &Setup,
    options: &InstallOptions,
    answers: &Answers,
    current: &Current,
) -> Result<InstallPlan, String> {
    let mut conflicts = Vec::new();
    let mut changes = Vec::new();
    let state = State::load(setup)?;
    let claude = answers.harnesses.contains(&Kind::ClaudeCode);
    let opencode = answers.harnesses.contains(&Kind::OpenCode);
    let mut shared = vec![Step {
        title: "Agents".to_string(),
        lines: vec![format!(
            "{} (one store and one daemon serve every agent chosen)",
            names_of(&answers.harnesses)
        )],
    }];
    shared.push(Step {
        title: "Memory language".to_string(),
        lines: vec![format!(
            "{}; written into the MCP instructions, the rule block and memory.language in \
             config.json",
            answers.language.describe()
        )],
    });
    shared.push(Step {
        title: "Embedding model".to_string(),
        lines: model_lines(setup, current, &answers.model),
    });
    let config_path = crate::config::path(&setup.data_dir);
    let write_config = !config_path.exists();
    shared.push(Step {
        title: format!("Settings file {}", config_path.display()),
        lines: vec![if write_config {
            "write the defaults (daemon idle time 2h) and the answers".to_string()
        } else {
            "exists: the answers are set in it, everything else kept".to_string()
        }],
    });
    let mut sections = vec![("Shared by every agent".to_string(), shared)];
    let mut auto_memory = AutoMemoryPlan::Keep;
    let mut imports = Vec::new();
    let mut after = Vec::new();
    if claude {
        let mut steps = Vec::new();
        let (step, change) = plan_user_config(setup, answers.develop, &mut conflicts)?;
        steps.push(step);
        changes.extend(change);
        let (step, change, plan) = plan_settings(
            setup,
            answers.auto_memory_off,
            &start_context_sentence(current),
            &state,
        )?;
        auto_memory = plan;
        steps.push(step);
        changes.extend(change);
        let (step, change) =
            plan_block(setup, answers.language, &answers.read_only, &mut conflicts)?;
        steps.push(step);
        changes.extend(change);
        sections.push((Kind::ClaudeCode.display_name().to_string(), steps));
        let (step, planned) = plan_import(setup, answers)?;
        after.push(step);
        imports = planned;
    }
    let mut opencode_changes = Vec::new();
    if opencode {
        let plan = crate::opencode::install::plan(
            setup,
            answers.language,
            &answers.read_only,
            answers.develop,
        )?;
        sections.push((Kind::OpenCode.display_name().to_string(), plan.steps));
        opencode_changes = plan.changes;
        conflicts.extend(plan.conflicts);
    }
    let mut codex_changes = Vec::new();
    let mut codex_memories = None;
    if answers.harnesses.contains(&Kind::Codex) {
        let plan = crate::codex::install::plan(
            setup,
            answers.language,
            &answers.read_only,
            answers.develop,
            answers.auto_memory_off,
        )?;
        sections.push((Kind::Codex.display_name().to_string(), plan.steps));
        codex_changes = plan.changes;
        codex_memories = plan.memories;
        conflicts.extend(plan.conflicts);
    }
    let mut cursor_changes = Vec::new();
    if answers.harnesses.contains(&Kind::Cursor) {
        let plan = crate::cursor::install::plan(
            setup,
            answers.language,
            &answers.read_only,
            answers.develop,
        )?;
        sections.push((Kind::Cursor.display_name().to_string(), plan.steps));
        cursor_changes = plan.changes;
        conflicts.extend(plan.conflicts);
    }
    let mut codewhale_changes = Vec::new();
    let mut codewhale_records = Vec::new();
    if answers.harnesses.contains(&Kind::CodeWhale) {
        let plan = crate::codewhale::install::plan(
            setup,
            answers.language,
            &answers.read_only,
            answers.develop,
            answers.auto_memory_off,
        )?;
        sections.push((Kind::CodeWhale.display_name().to_string(), plan.steps));
        codewhale_changes = plan.changes;
        codewhale_records = plan.records;
        conflicts.extend(plan.conflicts);
    }
    let mut other_off = Vec::new();
    let mut other_on = Vec::new();
    let other_steps = plan_other_memory(
        setup,
        answers,
        current,
        [
            (Kind::ClaudeCode, &mut changes),
            (Kind::OpenCode, &mut opencode_changes),
            (Kind::Codex, &mut codex_changes),
            (Kind::Cursor, &mut cursor_changes),
            (Kind::CodeWhale, &mut codewhale_changes),
        ],
        &mut other_off,
        &mut other_on,
    )?;
    if !other_steps.is_empty() {
        sections.push(("Other memory".to_string(), other_steps));
    }
    after.push(plan_sessions(setup, options, answers, current)?);
    let look_alikes = crate::projects::look_alike_lines(&setup.data_dir);
    if !look_alikes.is_empty() {
        after.push(Step {
            title: "Projects that may have moved (nothing is merged automatically)".to_string(),
            lines: look_alikes
                .into_iter()
                .map(|(what, how)| format!("{what}; {how}"))
                .collect(),
        });
    }
    sections.push(("Then, after the files are written".to_string(), after));
    Ok(InstallPlan {
        sections,
        changes,
        opencode_changes,
        codex_changes,
        codex_memories,
        cursor_changes,
        codewhale_changes,
        codewhale_records,
        conflicts,
        imports,
        write_config,
        auto_memory,
        other_off,
        other_on,
    })
}

/// The change of `changes` for `path`, made from the file's current text when there is none.
fn change_for<'a>(changes: &'a mut Vec<Change>, path: &Path) -> Result<&'a mut Change, String> {
    if let Some(at) = changes.iter().position(|c| c.path == path) {
        return Ok(&mut changes[at]);
    }
    let text = read_text(path)?.unwrap_or_default();
    let at = changes.len();
    changes.push(Change {
        path: path.to_path_buf(),
        after: Some(text),
        created: Vec::new(),
        separator: None,
    });
    Ok(&mut changes[at])
}

/// The "Other memory" part of the plan: each other memory tool of the chosen harnesses with
/// what happens to it (named read-only, switched off in its file, kept, switched on again),
/// the import of the tools whose format is known, and what is only shown (instruction files,
/// memory files another program writes). Switching an entry off or on is folded into the
/// harness's change of that file (one backup, one write, one rollback for the file).
fn plan_other_memory(
    setup: &Setup,
    answers: &Answers,
    current: &Current,
    mut changes: [(Kind, &mut Vec<Change>); 5],
    other_off: &mut Vec<(Kind, PathBuf, String, Value)>,
    other_on: &mut Vec<(Kind, PathBuf, String)>,
) -> Result<Vec<Step>, String> {
    use crate::coexist::ServerState;
    let mut steps = Vec::new();
    let mut lines = Vec::new();
    for server in &current.others.servers {
        if !answers.harnesses.contains(&server.harness) {
            continue;
        }
        let place = server.place(setup);
        if server.project.is_some() {
            lines.push(format!(
                "{} ({place}): a project's own registration, left as it is (`graphersal-memory \
                 doctor` says how to switch it off)",
                server.name
            ));
            continue;
        }
        let disable = answers.disable_other.contains(&server.name);
        let Some((_, list)) = changes.iter_mut().find(|(k, _)| *k == server.harness) else {
            continue;
        };
        match (&server.state, disable) {
            (ServerState::On, true) => {
                let change = change_for(list, &server.file)?;
                let text = change.after.clone().unwrap_or_default();
                match crate::coexist::disable_edit(server, &text)
                    .map_err(|e| in_file(&server.file, e))?
                {
                    Some((new, record)) => {
                        change.after = Some(new);
                        other_off.push((
                            server.harness,
                            server.file.clone(),
                            server.name.clone(),
                            record,
                        ));
                        lines.push(format!(
                            "{} ({place}): switch it off (the file is backed up first; \
                             uninstall switches it on again)",
                            server.name
                        ));
                    }
                    None => lines.push(format!(
                        "{} ({place}): cannot be switched off in place; kept as it is (by hand: \
                         {})",
                        server.name,
                        crate::coexist::off_by_hand(server)
                    )),
                }
            }
            (ServerState::OffByUs, false) => {
                let state = State::load_of(setup, server.harness)?;
                let record = state
                    .file(&server.file)
                    .and_then(|e| e.get(crate::coexist::OFF_RECORD))
                    .and_then(|r| r.get(&server.name))
                    .cloned()
                    .unwrap_or(Value::Null);
                let change = change_for(list, &server.file)?;
                let text = change.after.clone().unwrap_or_default();
                if let Some(new) =
                    crate::coexist::restore_edit(server.harness, &text, &server.name, &record)
                        .map_err(|e| in_file(&server.file, e))?
                {
                    change.after = Some(new);
                }
                other_on.push((server.harness, server.file.clone(), server.name.clone()));
                lines.push(format!(
                    "{} ({place}): switch it on again (install had switched it off)",
                    server.name
                ));
            }
            (ServerState::OffByUs, true) => lines.push(format!(
                "{} ({place}): stays switched off (uninstall switches it on again)",
                server.name
            )),
            (ServerState::Off, _) => lines.push(format!(
                "{} ({place}): switched off by you; left as it is",
                server.name
            )),
            (ServerState::On, false) if answers.read_only.contains(&server.name) => {
                lines.push(format!(
                    "{} ({place}): {}; stays, named read-only in the rule block (graphersal-memory \
                     is the main memory)",
                    server.name,
                    server.what()
                ))
            }
            (ServerState::On, false) => lines.push(format!(
                "{} ({place}): {}; kept as it is (the rule block does not name it)",
                server.name,
                server.what()
            )),
        }
    }
    // A change made only for an entry that ends up as it was is no change.
    for (_, list) in changes.iter_mut() {
        list.retain(|change| {
            change.after.is_none()
                || read_text(&change.path).ok().flatten().as_ref() != change.after.as_ref()
        });
    }
    for plugin in &current.others.plugins {
        if answers.harnesses.contains(&plugin.harness) {
            lines.push(format!(
                "{} plugin or hook {}: another memory beside graphersal-memory; left as it is \
                 (graphersal-memory does not change plugins)",
                plugin.harness.display_name(),
                plugin.name
            ));
        }
    }
    if !lines.is_empty() {
        steps.push(Step {
            title: "Other memory tools".to_string(),
            lines,
        });
    }
    let mut import_lines = Vec::new();
    for source in &current.other_sources {
        let state = crate::coexist::other_import::State::load(&setup.data_dir);
        let pending = state.pending_count(source);
        import_lines.push(format!(
            "{} ({}): {} memor{} in {}, {pending} new or changed{}",
            source.server,
            source.tool.name,
            source.items.len(),
            if source.items.len() == 1 { "y" } else { "ies" },
            source.path.display(),
            if pending == 0 {
                String::new()
            } else if answers.import_other {
                ": imported as global memories (the file is only read)".to_string()
            } else {
                ": not now (--no-import)".to_string()
            }
        ));
    }
    if let Some(native) = &current.codewhale_native {
        let pending = native.pending(&setup.data_dir);
        import_lines.push(format!(
            "CodeWhale's memory: {}{}",
            native.describe(&setup.data_dir),
            if pending == 0 || native.error.is_some() {
                ""
            } else if answers.import_other {
                ": imported (its store is only read)"
            } else {
                ": not now (--no-import)"
            }
        ));
    }
    import_lines.extend(current.not_importable.iter().cloned());
    if !import_lines.is_empty() {
        steps.push(Step {
            title: "Import the other memory tools' memories".to_string(),
            lines: import_lines,
        });
    }
    let mentions: Vec<String> = current
        .others
        .mentions
        .iter()
        .map(|m| {
            format!(
                "{}:{}: \"{}\" (mentions other memory: check it; never changed, \
                 `graphersal-memory doctor` suggests an edit)",
                m.file.display(),
                m.line,
                m.text
            )
        })
        .collect();
    if !mentions.is_empty() {
        steps.push(Step {
            title: "Instruction files that mention other memory (never changed by \
                    graphersal-memory)"
                .to_string(),
            lines: mentions,
        });
    }
    if answers.harnesses.contains(&Kind::ClaudeCode) && !current.others.writers.is_empty() {
        let mut lines: Vec<String> = current
            .others
            .writers
            .iter()
            .map(|w| {
                format!(
                    "{}{}{}",
                    w.dir.display(),
                    w.header
                        .as_ref()
                        .map(|h| format!(": MEMORY.md says \"{h}\""))
                        .unwrap_or_default(),
                    if w.outside > 0 {
                        format!(
                            "; {} file{} changed outside any Claude Code session",
                            w.outside,
                            if w.outside == 1 { "" } else { "s" }
                        )
                    } else {
                        String::new()
                    }
                )
            })
            .collect();
        lines.push(
            "they are imported like any memory file; what that program writes comes back into \
             the graph (`graphersal-memory import --ignore <dir>` stops it)"
                .to_string(),
        );
        steps.push(Step {
            title: "Memory files another program writes".to_string(),
            lines,
        });
    }
    Ok(steps)
}

/// The import step of Claude Code's file memory.
fn plan_import(setup: &Setup, answers: &Answers) -> Result<(Step, Vec<DirPlan>), String> {
    let import_state = ImportState::load(&setup.data_dir)?;
    let imports = import::plan(import::scan(&setup.paths.projects)?, &import_state);
    let (with_project, skipped, new, changed, unchanged) = import::totals(&imports);
    let mut lines = vec![format!(
        "{} memory director{} under {}: {new} new and {changed} changed memory files to import, \
         {unchanged} unchanged",
        with_project + skipped,
        if with_project + skipped == 1 {
            "y"
        } else {
            "ies"
        },
        setup.paths.projects.display()
    )];
    if skipped > 0 {
        lines.push(format!(
            "{skipped} director{} skipped: the project's directory no longer exists",
            if skipped == 1 { "y" } else { "ies" }
        ));
    }
    let ignored: usize = imports.iter().map(|plan| plan.ignored).sum();
    if ignored > 0 {
        lines.push(format!(
            "{ignored} memory file{} not imported: you ignored {} (`graphersal-memory import \
             --unignore <path>` imports again)",
            if ignored == 1 { "" } else { "s" },
            if ignored == 1 { "it" } else { "them" }
        ));
    }
    if answers.import {
        lines.push(
            "The files are read, not changed; every memory stays in its project. CLAUDE.md \
             rules are not imported in this version: CLAUDE.md keeps working as before (Claude \
             Code still loads it)."
                .to_string(),
        );
    } else {
        lines.push(
            "not now (--no-import); the session start hook imports a project's files when you \
             work in it"
                .to_string(),
        );
    }
    Ok((
        Step {
            title: "Import Claude Code's existing file memory (MEMORY.md and the memory files)"
                .to_string(),
            lines,
        },
        imports,
    ))
}

/// The session indexing step, per harness, with the summaries line.
fn plan_sessions(
    setup: &Setup,
    options: &InstallOptions,
    answers: &Answers,
    current: &Current,
) -> Result<Step, String> {
    let mut lines = Vec::new();
    for kind in &answers.harnesses {
        let dir = setup.agent_of(*kind).projects_dir().to_path_buf();
        let request = crate::sessions::Request::for_harness(*kind, &dir);
        let (found, active, todo, _) = crate::sessions::plan(&request, &setup.data_dir)?;
        lines.push(format!(
            "{}: {found} session{} in {}: {todo} to index, {active} still being written \
             (indexed later)",
            kind.display_name(),
            if found == 1 { "" } else { "s" },
            dir.display()
        ));
    }
    let settings = &current.config;
    let estimate = summaries_estimate(setup, settings, &answers.harnesses);
    let command = summaries_command(settings, &answers.harnesses);
    let summaries_line = match (settings.summaries.enabled, answers.summaries) {
        (true, true) => "summaries: on (already switched on in the settings file)".to_string(),
        (true, false) => "summaries: switched off (you answered no)".to_string(),
        (false, true) => format!(
            "summaries: switched on, through `{}`, at most ${:.2} per month; {estimate}",
            command.join(" "),
            settings.summaries.monthly_budget_usd,
        ),
        (false, false) if options.interactive => {
            "summaries: off (raw turns only; `graphersal-memory summaries enable` later)"
                .to_string()
        }
        (false, false) => format!(
            "summaries: off unless you agree below (`--summaries` switches them on; `--yes` \
             alone leaves them off); {estimate}"
        ),
    };
    lines.push(summaries_line);
    Ok(Step {
        title: "Index past sessions (raw turns, locally, in the background)".to_string(),
        lines,
    })
}

/// The plan: each section under its heading after a blank line, the steps numbered across
/// the sections, the home directory as `~`.
fn print_sections(
    out: &mut dyn Write,
    setup: &Setup,
    sections: &[(String, Vec<Step>)],
    color: bool,
) {
    let mut n = 0;
    for (heading, steps) in sections {
        if steps.is_empty() {
            continue;
        }
        let _ = writeln!(out, "\n{}", crate::prompt::paint(color, "1", heading));
        for step in steps {
            n += 1;
            let _ = writeln!(out, "  {n}. {}", setup.show(&step.title));
            for line in &step.lines {
                let line = setup.show(line);
                let line = if line.starts_with("ACTION NEEDED") {
                    crate::prompt::action_needed(color, &line)
                } else {
                    line
                };
                let _ = writeln!(out, "     - {line}");
            }
        }
    }
}

/// Asks `question`; true for `y`/`yes`.
pub(crate) fn confirm(question: &str, input: &mut dyn BufRead, out: &mut dyn Write) -> bool {
    let _ = write!(out, "{question} [y/N] ");
    let _ = out.flush();
    let mut answer = String::new();
    if input.read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Applies planned file changes: backs a file up before its first change, writes it, and
/// records the change in the state (`after_write` adds to the state of a file just written).
/// A file whose text is no longer the one the plan was computed from (`bases`), or that
/// changes while it is written, is not written: the error starts with
/// [`fsutil::CHANGED_MEANWHILE`]. Every file written goes into `journal` with its text before
/// (the first time only).
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_changes(
    setup: &Setup,
    changes: &[Change],
    state: &mut State,
    report: &mut Vec<String>,
    backups: &mut std::collections::BTreeSet<PathBuf>,
    bases: &std::collections::BTreeMap<PathBuf, Option<String>>,
    journal: &mut Vec<(PathBuf, Option<Vec<u8>>)>,
    after_write: &mut dyn FnMut(&Path, &mut State),
) -> Result<(), String> {
    // One backup directory per install: the stamp of a directory already written is reused
    // (the agents' changes are applied one after the other, maybe across a second).
    let stamp = backups
        .iter()
        .next()
        .and_then(|dir| dir.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| crate::daemon::now_utc().replace(':', ""));
    for change in changes {
        let path = &change.path;
        let current = fsutil::read_optional(path)?;
        let current_fp = current.as_deref().map(fsutil::fingerprint);
        if let Some(base) = bases.get(path)
            && *base != current_fp
        {
            return Err(format!("{}: {}", fsutil::CHANGED_MEANWHILE, path.display()));
        }
        let known = state.file(path).cloned();
        let entry_existed = known.is_some();
        // Missing directories on the way are created and recorded.
        if let Some(parent) = path.parent() {
            let mut missing = Vec::new();
            let mut dir = parent.to_path_buf();
            while !dir.exists() {
                missing.push(dir.clone());
                match dir.parent() {
                    Some(up) => dir = up.to_path_buf(),
                    None => break,
                }
            }
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("cannot create {}: {err}", parent.display()))?;
            for dir in missing.iter().rev() {
                state.add_created_dir(dir);
                // A rollback removes it again (when empty).
                journal.push((dir.clone(), None));
            }
        }
        let mut backup_note = String::new();
        if !entry_existed {
            let entry = state.file_mut(path);
            entry.insert("existed".into(), json!(current.is_some()));
            // A file that holds our parts already (left by an install whose record is gone, or
            // one running at the same time) is no original: uninstall removes our parts from
            // it instead of putting this copy back.
            let original = !current.as_deref().is_some_and(holds_ours);
            entry.insert("backup_valid".into(), json!(original));
            if let Some(bytes) = &current {
                let backup = backup_file(setup, &stamp, path, bytes)?;
                backup_note = backed_up(path, &backup, backups);
                entry.insert("backup".into(), json!(backup.display().to_string()));
            }
        } else {
            let written = known
                .as_ref()
                .and_then(|k| k.get("written"))
                .and_then(Value::as_str)
                .map(str::to_string);
            if written != current_fp {
                // Changed since install wrote it: the first backup no longer restores it.
                let entry = state.file_mut(path);
                entry.insert("backup_valid".into(), json!(false));
                if let Some(bytes) = &current {
                    let backup = backup_file(setup, &stamp, path, bytes)?;
                    backup_note = backed_up(path, &backup, backups);
                }
            }
        }
        let mut unchanged = |target: &Path| {
            fsutil::read_optional(target)
                .ok()
                .map(|bytes| bytes.as_deref().map(fsutil::fingerprint))
                == Some(current_fp.clone())
        };
        match &change.after {
            Some(text) => {
                fsutil::write_atomic_checked(path, text.as_bytes(), &mut unchanged)?;
                if !journal.iter().any(|(p, _)| p == path) {
                    journal.push((path.clone(), current.clone()));
                }
                let entry = state.file_mut(path);
                entry.insert(
                    "written".into(),
                    json!(fsutil::fingerprint(text.as_bytes())),
                );
                let mut created: Vec<Value> = entry
                    .get("created")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for item in &change.created {
                    let item = Value::from(item.as_str());
                    if !created.contains(&item) {
                        created.push(item);
                    }
                }
                entry.insert("created".into(), Value::Array(created));
                if let Some(sep) = &change.separator {
                    entry.insert("separator".into(), json!(sep));
                }
                report.push(format!(
                    "{} {}{backup_note}",
                    if current.is_some() {
                        "changed"
                    } else {
                        "created"
                    },
                    path.display()
                ));
            }
            None => {
                remove_file(path)?;
                if !journal.iter().any(|(p, _)| p == path) {
                    journal.push((path.clone(), current.clone()));
                }
                report.push(format!("removed {}{backup_note}", path.display()));
            }
        }
        after_write(path, state);
        state.save(setup)?;
    }
    Ok(())
}

/// Whether a file's bytes hold one of graphersal-memory's parts (any mention of its name: an
/// MCP entry, a hook, a rule block, the plugin). A false alarm only means uninstall removes our
/// parts from the file instead of putting its backup back.
pub(crate) fn holds_ours(bytes: &[u8]) -> bool {
    bytes.windows(17).any(|w| w == b"graphersal-memory")
}

/// Whether the backup a state entry names holds our parts ([`holds_ours`]): then uninstall
/// never puts it back.
pub(crate) fn backup_holds_ours(entry: Option<&serde_json::Map<String, Value>>) -> bool {
    entry
        .and_then(|e| e.get("backup"))
        .and_then(Value::as_str)
        .and_then(|backup| std::fs::read(backup).ok())
        .is_some_and(|bytes| holds_ours(&bytes))
}

/// The report's note about a backup (its directory is reported once, in `backups`).
fn backed_up(
    path: &Path,
    backup: &Path,
    backups: &mut std::collections::BTreeSet<PathBuf>,
) -> String {
    if let Some(dir) = backup.parent() {
        backups.insert(dir.to_path_buf());
    }
    match (path.file_name(), backup.file_name()) {
        (Some(a), Some(b)) if a == b => " (backed up)".to_string(),
        (_, Some(b)) => format!(" (backed up as {})", b.to_string_lossy()),
        _ => " (backed up)".to_string(),
    }
}

fn backup_file(setup: &Setup, stamp: &str, path: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let dir = setup.state_dir().join("backups").join(stamp);
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let mut target = dir.join(&name);
    let mut n = 1;
    while target.exists() {
        n += 1;
        target = dir.join(format!("{name}.{n}"));
    }
    std::fs::write(&target, bytes)
        .map_err(|err| format!("cannot write the backup {}: {err}", target.display()))?;
    Ok(target)
}

pub(crate) fn remove_file(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("cannot remove {}: {err}", path.display())),
    }
}

/// `install`: the wizard (interactive), or the plan with the flags' answers; asks (unless
/// `--yes`), applies it and prints the report. Returns the exit code (1 when a conflict or an
/// import failure left something undone; a failed model download is reported, not a failure).
pub fn install(
    setup: &Setup,
    options: &InstallOptions,
    connect: &mut Connect<'_>,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<u8, String> {
    if options.harnesses.is_none() && setup.detected().is_empty() {
        return Ok(refuse_without_programs(setup, out, options.color));
    }
    // Before anything changes: a data directory that is someone else's folder, a socket path
    // the daemon cannot use, another install running.
    if let Some(reason) = crate::store::foreign_data_dir(&setup.data_dir) {
        return Err(reason);
    }
    crate::ipc::socket_path(&setup.run_dir).map_err(|err| format!("{err}; nothing was changed"))?;
    let current = Current::read(setup, options.project_dir.as_deref())?;
    let interactive = options.interactive && !options.yes && !options.dry_run;
    let color = options.color;
    let mut notes = detection_notes(setup, options);
    // A name in an "other memory" flag that matches no user-scope entry.
    let all = current.others.names(&Kind::ALL);
    for (flag, names) in [
        ("--disable-other", &options.disable_other),
        ("--enable-other", &options.enable_other),
        ("--other-read-only", &options.other_read_only),
        ("--keep-other", &options.keep_other),
    ] {
        for name in names {
            if !all.contains(name) {
                notes.push(format!(
                    "warning: {flag} {name}: no other memory tool's MCP entry of that name is \
                     found at user scope; nothing changes for it"
                ));
            }
        }
    }
    let answers = if interactive {
        let mut console = Console::new(input, out, options.color);
        let offered = options
            .harnesses
            .clone()
            .unwrap_or_else(|| current.detected.clone());
        let title = console.bold(&format!(
            "graphersal-memory install ({})",
            names_of(&offered)
        ));
        let _ = writeln!(console.out(), "{title}");
        for note in &notes {
            let _ = writeln!(console.out(), "  {note}");
        }
        let hint = console
            .dim("A few questions; Enter takes the default. Nothing changes before the last step.");
        let _ = writeln!(console.out(), "{hint}");
        match ask(setup, options, &current, &mut console) {
            Ok(answers) => answers,
            Err(_) => {
                let _ = writeln!(console.out(), "\nAborted: no answer, nothing was changed.");
                return Ok(1);
            }
        }
    } else {
        default_answers(options, &current)
    };
    let plan = plan_install(setup, options, &answers, &current)
        .map_err(|err| way_out(&err, &answers.harnesses, "install", "install into"))?;
    // What must never be skipped: conflicts, Codex's hook approval, "nothing is lost".
    let mut must_read = Vec::new();
    for conflict in &plan.conflicts {
        must_read.push(paint(
            color,
            "33",
            &format!("conflict: {}", setup.show(conflict)),
        ));
    }
    let codex_approval = answers.harnesses.contains(&Kind::Codex)
        && !crate::codex::install::inspect(setup).hooks_trusted;
    if answers.harnesses.contains(&Kind::ClaudeCode)
        && answers.auto_memory_off
        && !current.auto_memory_off
    {
        must_read.push(NOTHING_IS_LOST.to_string());
    } else if plan.codex_memories == Some(true) {
        must_read.push(crate::codex::install::NOTHING_IS_LOST.to_string());
    }
    if interactive {
        let mut console = Console::new(input, out, options.color);
        let offered = options
            .harnesses
            .clone()
            .unwrap_or_else(|| current.detected.clone());
        let ask_harnesses = options.harnesses.is_none() && current.detected.len() > 1;
        let claude_possible = ask_harnesses || offered.contains(&Kind::ClaudeCode);
        let total = wizard_steps(options, &current, claude_possible, &offered)
            - skipped_steps(
                options,
                &current,
                claude_possible,
                &offered,
                &answers.harnesses,
            );
        console.step(total, total, "Ready to install", &[]);
        for (key, value) in recap(setup, &answers, &current, &plan) {
            let _ = writeln!(console.out(), "  {key}: {value}");
        }
        if !must_read.is_empty() || codex_approval {
            let _ = writeln!(console.out());
        }
        for line in &must_read {
            let _ = writeln!(console.out(), "  {line}");
        }
        if codex_approval {
            let line =
                crate::prompt::action_needed(color, &crate::codex::install::hook_approval_line());
            let _ = writeln!(console.out(), "  {line}");
        }
        let _ = writeln!(console.out());
        let sections = &plan.sections;
        let installed = console.confirm_install(&mut |out: &mut dyn Write| {
            let _ = writeln!(out, "Every change install makes:");
            print_sections(out, setup, sections, color);
        });
        if !installed {
            let _ = writeln!(console.out(), "Cancelled: nothing was changed.");
            return Ok(1);
        }
    } else {
        let _ = writeln!(
            out,
            "{}",
            paint(
                color,
                "1",
                &format!(
                    "graphersal-memory install ({})",
                    names_of(&answers.harnesses)
                )
            )
        );
        for note in &notes {
            let _ = writeln!(out, "  {note}");
        }
        let _ = writeln!(
            out,
            "\n{}",
            if options.dry_run {
                "Dry run: this is what install would do (nothing is changed)."
            } else {
                "This is what install will do:"
            }
        );
        print_sections(out, setup, &plan.sections, color);
        // The plan itself says "nothing is lost" in its step; the conflicts follow it.
        if !plan.conflicts.is_empty() {
            let _ = writeln!(out);
        }
        for conflict in &plan.conflicts {
            let line = paint(color, "33", &format!("conflict: {}", setup.show(conflict)));
            let _ = writeln!(out, "  {line}");
        }
        if options.dry_run {
            return Ok(0);
        }
        if !options.yes {
            let _ = writeln!(out);
            match ask_yes_no("Install?", true, input, out) {
                Reply::Yes => {}
                Reply::No => {
                    let _ = writeln!(
                        out,
                        "Cancelled: nothing was changed (pass --yes to run without asking)."
                    );
                    return Ok(1);
                }
                Reply::NoInput => {
                    let _ = writeln!(out, "No answer, nothing was changed: {NO_TERMINAL}.");
                    return Ok(1);
                }
            }
        }
    }
    // The data directory first, private to the user (Unix: 0700); the daemon tightens it at
    // its start too, and says so in its log when it cannot.
    crate::store::create_private_dir(&setup.data_dir)?;
    // Nothing is written while another install or uninstall runs.
    let _lock = InstallLock::take(setup, true)?;
    let mut report = Vec::new();
    let claude = answers.harnesses.contains(&Kind::ClaudeCode);
    let (plan, agent_report, backups) =
        apply_with_retries(setup, options, &answers, &current, plan, &mut report)?;
    // The settings file: the defaults when there is none, then the answers.
    if plan.write_config {
        let path = crate::config::path(&setup.data_dir);
        crate::store::create_private_dir(&setup.data_dir)?;
        fsutil::write_atomic(&path, Config::default_text().as_bytes())?;
        report.push(format!("created {}", path.display()));
    }
    let mut values = vec![("memory", "language", json!(answers.language.name()))];
    let command = summaries_command(&current.config, &answers.harnesses);
    if command != current.config.summaries.harness_command {
        values.push(("summaries", "harness_command", json!(command.join(" "))));
        report.push(format!(
            "summaries run through `{}` when you switch them on ({}; summaries.harness_command in \
             config.json)",
            command.join(" "),
            if crate::opencode::is_opencode(&command) {
                "opencode only; a free model of opencode Zen"
            } else {
                "Codex only; the model of your Codex configuration"
            }
        ));
    }

    // The embedding model: downloaded into the cache (the one network access), then named in
    // the settings, which the daemon follows.
    let model_ready = if answers.model == models::BUILTIN
        || models::cached(&setup.data_dir, &answers.model).is_some()
    {
        true
    } else {
        let _ = writeln!(out, "\nEmbedding model:");
        let live = options.interactive;
        match download::download(
            &answers.model,
            &models::cache_dir(&setup.data_dir),
            &setup.source,
            out,
            live,
        ) {
            Ok(done) => {
                report.push(format!(
                    "embedding model: downloaded {} ({}, licence {}, {}) into {}",
                    answers.model,
                    download::human_bytes(done.bytes),
                    models::licence_name(&done.license),
                    done.identity,
                    done.dir.display()
                ));
                for warning in &done.warnings {
                    report.push(format!("embedding model warning: {warning}"));
                }
                true
            }
            Err(err) => {
                report.push(format!(
                    "embedding model: the download of {} failed ({err}); the {} stays. Run \
                     `graphersal-memory install --model {}` to try again.",
                    answers.model,
                    match &current.config.model {
                        Some(model) if model != models::BUILTIN => format!("model {model}"),
                        _ => "built-in embedder hash-ngram/1".to_string(),
                    },
                    answers.model
                ));
                false
            }
        }
    };
    if model_ready {
        values.push(("embedding", "model", json!(answers.model)));
    }
    let config_path = crate::config::path(&setup.data_dir);
    let before = crate::fsutil::read_optional(&config_path)?
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    let text = crate::config::with_values(before.as_deref(), &values)?;
    if before.as_deref() != Some(text.as_str()) {
        crate::store::create_private_dir(&setup.data_dir)?;
        fsutil::write_atomic(&config_path, text.as_bytes())?;
    }
    if model_ready && current.config.model.as_deref() != Some(answers.model.as_str()) {
        report.push(format!(
            "embedding model: {} (the daemon re-embeds the memory with it in the background)",
            if answers.model == models::BUILTIN {
                "the built-in hash-ngram/1".to_string()
            } else {
                answers.model.clone()
            }
        ));
    }

    // The import goes through the memory functions (the daemon serves the store).
    let mut failures = 0;
    let (_, _, new, changed, _) = import::totals(&plan.imports);
    if !claude {
        // opencode has no file memory to import.
    } else if !answers.import {
        report.push("import: not now (--no-import)".to_string());
    } else if new + changed > 0 {
        let done = import::with_lock(&setup.data_dir, Duration::from_secs(60), || {
            let mut import_state = ImportState::load(&setup.data_dir)?;
            Ok::<_, String>(import::apply(
                &plan.imports,
                connect,
                &mut import_state,
                &setup.data_dir,
                None,
            ))
        })?;
        match done {
            Some(result) => {
                let result = result?;
                report.push(import_summary(&result));
                report.extend(import_conflict_lines(&result));
                for problem in &result.problems {
                    report.push(format!("import failed: {problem}"));
                }
                failures += result.problems.len();
                for warning in &result.warnings {
                    report.push(format!("import warning: {warning}"));
                }
            }
            None => {
                failures += 1;
                report.push(
                    "import skipped: another import is running; run `graphersal-memory import` \
                     later"
                        .to_string(),
                );
            }
        }
    } else {
        report.push("imported nothing: no new or changed memory files".to_string());
    }

    // The other memory tools' memories whose format is known (M15), as global memories.
    if answers.import_other && current.other_pending > 0 {
        let done =
            crate::coexist::other_import::apply(&current.other_sources, connect, &setup.data_dir);
        report.push(done.line(&current.other_sources));
        report.extend(done.kept_edited.iter().map(|line| format!("  {line}")));
        for problem in &done.problems {
            report.push(format!("import failed: {problem}"));
        }
        failures += done.problems.len();
    }
    if answers.import_other
        && let Some(native) = &current.codewhale_native
        && native.pending(&setup.data_dir) > 0
    {
        let done = crate::codewhale::memory::apply(native, connect, &setup.data_dir);
        report.push(done.line(native));
        report.extend(done.kept_edited.iter().map(|line| format!("  {line}")));
        for problem in &done.problems {
            report.push(format!("import failed: {problem}"));
        }
        failures += done.problems.len();
    }
    // The choices about other memory tools: what the rule block names read-only.
    let choices = crate::coexist::Choices {
        read_only: answers.read_only.clone(),
        kept: answers.kept.clone(),
    };
    if current.other_choices.as_ref() != Some(&choices) {
        choices.save(setup)?;
    }

    // Summaries: only with consent (--summaries, a yes in the wizard, or yes to the question;
    // --yes alone: off).
    let settings = Config::load(&setup.data_dir)?;
    if !settings.summaries.enabled {
        let agreed = answers.summaries
            || (!interactive
                && !options.yes
                && confirm(
                    "\nSummarize finished sessions through your own harness (decisions, \
                     corrections, outcomes, open threads; costs model usage, see the estimate \
                     above)?",
                    input,
                    out,
                ));
        if agreed {
            crate::commands::write_summaries(setup, true, None)?;
            report.push(format!(
                "summaries: on, through `{}`, at most ${:.2} per month (`graphersal-memory \
                 summaries disable` switches them off)",
                settings.summaries.harness_command.join(" "),
                settings.summaries.monthly_budget_usd
            ));
        } else {
            report.push(
                "summaries: off (raw turns are indexed; `graphersal-memory summaries enable` \
                 switches them on)"
                    .to_string(),
            );
        }
    } else if !answers.summaries {
        crate::commands::write_summaries(setup, false, None)?;
        report.push("summaries: off (raw turns are still indexed)".to_string());
    }

    // A running daemon follows the new settings now (the embedding model); else at its start.
    let _ = crate::shim::request_running(
        &setup.run_dir,
        &json!({"protocol": crate::protocol::PROTOCOL, "control": "apply_settings"}),
    );
    // Past sessions: indexed by the daemon in the background, when it runs (the import
    // started it); else at the next session start.
    let mut queued = true;
    for kind in &answers.harnesses {
        let dir = setup.agent_of(*kind).projects_dir().to_path_buf();
        let request = crate::sessions::Request::for_harness(*kind, &dir);
        queued &= matches!(
            crate::shim::request_running(&setup.run_dir, &request.to_control()),
            Ok(Some(answer)) if answer["ok"] == true
        );
    }
    match queued {
        true => report.push(
            "past sessions: indexing in the background (`graphersal-memory import --sessions` \
             shows the progress)"
                .to_string(),
        ),
        false => report.push(
            "past sessions: indexed at the next session start (or now: `graphersal-memory \
             import --sessions`)"
                .to_string(),
        ),
    }
    print_install_report(
        out,
        setup,
        &InstallReport {
            agents: &agent_report,
            general: &report,
            backups: &backups,
            conflicts: &plan.conflicts,
            up_to_date: plan.changes.is_empty()
                && plan.opencode_changes.is_empty()
                && plan.codex_changes.is_empty()
                && plan.cursor_changes.is_empty()
                && plan.codewhale_changes.is_empty()
                && plan.conflicts.is_empty(),
            harnesses: &answers.harnesses,
            codex_approval: answers.harnesses.contains(&Kind::Codex)
                && !crate::codex::install::inspect(setup).hooks_trusted,
        },
        color,
    );
    Ok(if plan.conflicts.is_empty() && failures == 0 {
        0
    } else {
        1
    })
}

/// Applies the agents' file changes of `plan` (each harness's state recorded as it goes) and
/// install's auto-memory record. Every file written is recorded in `journal` with its text
/// before (for a rollback); `bases` are the fingerprints the plan's texts were computed from.
#[allow(clippy::too_many_arguments)]
fn apply_agents(
    setup: &Setup,
    answers: &Answers,
    plan: &InstallPlan,
    bases: &std::collections::BTreeMap<PathBuf, Option<String>>,
    journal: &mut Vec<(PathBuf, Option<Vec<u8>>)>,
    agent_report: &mut Vec<(Kind, Vec<String>)>,
    report: &mut Vec<String>,
    backups: &mut std::collections::BTreeSet<PathBuf>,
) -> Result<State, String> {
    let mut state = State::load(setup)?;
    for kind in &answers.harnesses {
        let mut lines = Vec::new();
        let mut kind_state = State::load_of(setup, *kind)?;
        if !kind_state.meta.contains_key("installed_at") {
            kind_state
                .meta
                .insert("installed_at".into(), json!(crate::daemon::now_utc()));
        }
        kind_state
            .meta
            .insert("updated_at".into(), json!(crate::daemon::now_utc()));
        kind_state
            .meta
            .insert("exe".into(), json!(setup.exe.display().to_string()));
        match kind {
            Kind::ClaudeCode => {
                kind_state
                    .meta
                    .insert("auto_memory_off".into(), json!(answers.auto_memory_off));
                kind_state.save(setup)?;
                let settings = setup.paths.settings.clone();
                apply_changes(
                    setup,
                    &plan.changes,
                    &mut kind_state,
                    &mut lines,
                    backups,
                    bases,
                    journal,
                    // install's auto-memory record goes with the write of the settings file.
                    &mut |path, state| {
                        record_other(plan, Kind::ClaudeCode, path, state);
                        if path != settings {
                            return;
                        }
                        match &plan.auto_memory {
                            AutoMemoryPlan::Keep => {}
                            AutoMemoryPlan::SwitchOff { previous } => {
                                state.file_mut(path).insert(
                                    AUTO_MEMORY_RECORD.into(),
                                    json!({"previous": previous}),
                                );
                            }
                            AutoMemoryPlan::Restore => {
                                state.file_mut(path).remove(AUTO_MEMORY_RECORD);
                            }
                        }
                    },
                )?;
                state = kind_state;
            }
            Kind::OpenCode => {
                kind_state.save(setup)?;
                apply_changes(
                    setup,
                    &plan.opencode_changes,
                    &mut kind_state,
                    &mut lines,
                    backups,
                    bases,
                    journal,
                    &mut |path, state| record_other(plan, Kind::OpenCode, path, state),
                )?;
            }
            Kind::Codex => {
                kind_state
                    .meta
                    .insert("auto_memory_off".into(), json!(answers.auto_memory_off));
                kind_state.save(setup)?;
                let config = setup.codex.config_file.clone();
                apply_changes(
                    setup,
                    &plan.codex_changes,
                    &mut kind_state,
                    &mut lines,
                    backups,
                    bases,
                    journal,
                    // The memories record goes with the write of config.toml.
                    &mut |path, state| {
                        record_other(plan, Kind::Codex, path, state);
                        if path != config {
                            return;
                        }
                        match plan.codex_memories {
                            Some(true) => {
                                state.file_mut(path).insert(
                                    crate::codex::install::MEMORIES_RECORD.into(),
                                    json!({"previous": true}),
                                );
                            }
                            Some(false) => {
                                state
                                    .file_mut(path)
                                    .remove(crate::codex::install::MEMORIES_RECORD);
                            }
                            None => {}
                        }
                    },
                )?;
                if plan.codex_memories == Some(true) {
                    lines.push(
                        "Codex's memories: off (uninstall exports every memory as Markdown \
                         and switches them on again)"
                            .to_string(),
                    );
                }
            }
            Kind::Cursor => {
                kind_state.save(setup)?;
                apply_changes(
                    setup,
                    &plan.cursor_changes,
                    &mut kind_state,
                    &mut lines,
                    backups,
                    bases,
                    journal,
                    &mut |path, state| record_other(plan, Kind::Cursor, path, state),
                )?;
            }
            Kind::CodeWhale => {
                kind_state
                    .meta
                    .insert("auto_memory_off".into(), json!(answers.auto_memory_off));
                kind_state.save(setup)?;
                let config = setup.codewhale.config_file.clone();
                apply_changes(
                    setup,
                    &plan.codewhale_changes,
                    &mut kind_state,
                    &mut lines,
                    backups,
                    bases,
                    journal,
                    // The always_load and memory records go with the write of config.toml.
                    &mut |path, state| {
                        record_other(plan, Kind::CodeWhale, path, state);
                        if path != config {
                            return;
                        }
                        for (key, value) in &plan.codewhale_records {
                            match value {
                                Some(value) => {
                                    state.file_mut(path).insert((*key).into(), value.clone());
                                }
                                None => {
                                    state.file_mut(path).remove(*key);
                                }
                            }
                        }
                    },
                )?;
                if plan.codewhale_records.iter().any(|(key, value)| {
                    *key == crate::codewhale::install::MEMORY_RECORD && value.is_some()
                }) {
                    lines.push(
                        "CodeWhale's memory: off (uninstall exports every memory as Markdown \
                         and switches it on again)"
                            .to_string(),
                    );
                }
            }
        }
        for (k, path, name, _) in &plan.other_off {
            if k == kind {
                lines.push(format!(
                    "{name}: switched off in {} (uninstall switches it on again)",
                    path.display()
                ));
            }
        }
        for (k, path, name) in &plan.other_on {
            if k == kind {
                lines.push(format!("{name}: switched on again in {}", path.display()));
            }
        }
        match agent_report.iter_mut().find(|(k, _)| k == kind) {
            Some((_, known)) => known.extend(lines),
            None => agent_report.push((*kind, lines)),
        }
    }
    if matches!(plan.auto_memory, AutoMemoryPlan::SwitchOff { .. }) {
        let line = format!(
            "{}'s auto-memory: off (uninstall writes every memory back to its memory files \
             and switches it on again)",
            setup.agent().display_name()
        );
        match agent_report
            .iter_mut()
            .find(|(k, _)| *k == Kind::ClaudeCode)
        {
            Some((_, lines)) => lines.push(line),
            None => report.push(line),
        }
    }
    Ok(state)
}

/// Records in `state` (the harness `kind`'s) what the plan switched off in `path` (the entry
/// and how to undo it) and forgets what it switched on again, with the write of that file.
fn record_other(plan: &InstallPlan, kind: Kind, path: &Path, state: &mut State) {
    let key = crate::coexist::OFF_RECORD;
    for (k, p, name, record) in &plan.other_off {
        if *k == kind && p == path {
            let entry = state.file_mut(path);
            let records = entry.entry(key).or_insert_with(|| json!({}));
            if let Some(records) = records.as_object_mut() {
                records.insert(name.clone(), record.clone());
            }
        }
    }
    for (k, p, name) in &plan.other_on {
        if *k == kind && p == path {
            let entry = state.file_mut(path);
            if let Some(records) = entry.get_mut(key).and_then(Value::as_object_mut) {
                records.remove(name);
                if records.is_empty() {
                    entry.remove(key);
                }
            }
        }
    }
}

/// Every file change of a plan, of every agent.
fn all_changes(plan: &InstallPlan) -> impl Iterator<Item = &Change> {
    plan.changes
        .iter()
        .chain(&plan.opencode_changes)
        .chain(&plan.codex_changes)
        .chain(&plan.cursor_changes)
        .chain(&plan.codewhale_changes)
}

/// The agents' part of an install: plans again right before writing (a harness may have
/// rewritten its file since the plan was shown, as Claude Code does with `~/.claude.json`), refuses
/// read-only files before anything is written, applies, plans and applies again when a file
/// changed meanwhile (a stale merge is never written), and on any other failure puts back every
/// file and state record it had written. Returns the plan applied, the per-agent report and the
/// backup directories.
#[allow(clippy::type_complexity)]
fn apply_with_retries(
    setup: &Setup,
    options: &InstallOptions,
    answers: &Answers,
    current: &Current,
    shown: InstallPlan,
    report: &mut Vec<String>,
) -> Result<
    (
        InstallPlan,
        Vec<(Kind, Vec<String>)>,
        std::collections::BTreeSet<PathBuf>,
    ),
    String,
> {
    const ATTEMPTS: usize = 5;
    let states: Vec<(PathBuf, Option<Vec<u8>>)> = Kind::ALL
        .iter()
        .map(|kind| {
            let path = State::path_of(setup, *kind);
            fsutil::read_optional(&path).map(|bytes| (path, bytes))
        })
        .collect::<Result<_, _>>()?;
    let mut journal = Vec::new();
    let mut agent_report = Vec::new();
    let mut backups = std::collections::BTreeSet::new();
    let mut previous = shown;
    for attempt in 1..=ATTEMPTS {
        let mut bases = std::collections::BTreeMap::new();
        for change in all_changes(&previous) {
            let bytes = fsutil::read_optional(&change.path)?;
            bases.insert(
                change.path.clone(),
                bytes.as_deref().map(fsutil::fingerprint),
            );
        }
        let plan = plan_install(setup, options, answers, current)?;
        let blocked: Vec<String> = all_changes(&plan)
            .filter_map(|change| fsutil::not_writable(&change.path))
            .collect();
        if !blocked.is_empty() && journal.is_empty() {
            return Err(format!("{}; nothing was changed", blocked.join("; ")));
        }
        let mut lines = Vec::new();
        match apply_agents(
            setup,
            answers,
            &plan,
            &bases,
            &mut journal,
            &mut agent_report,
            &mut lines,
            &mut backups,
        ) {
            Ok(_) => {
                report.extend(lines);
                return Ok((plan, agent_report, backups));
            }
            Err(err) if err.contains(fsutil::CHANGED_MEANWHILE) && attempt < ATTEMPTS => {
                // What was written stays (it is installed and recorded); the next plan sees it.
                report.extend(lines);
                previous = plan;
            }
            Err(err) => return Err(roll_back(setup, &err, &journal, &states)),
        }
    }
    Err("the agents' files kept changing while install wrote them; run install again".into())
}

/// Puts back every file `journal` records (its text before install, or deleted when install
/// created it) and the state files (`states`), after `err` stopped an install. The message
/// says what failed and what was put back, or what could not be.
fn roll_back(
    setup: &Setup,
    err: &str,
    journal: &[(PathBuf, Option<Vec<u8>>)],
    states: &[(PathBuf, Option<Vec<u8>>)],
) -> String {
    let mut failed = Vec::new();
    let mut restored = Vec::new();
    for (path, before) in journal.iter().rev().chain(states.iter()) {
        if path.is_dir() {
            // A directory install created: removed when nothing else is in it.
            let _ = std::fs::remove_dir(path);
            continue;
        }
        let done = match before {
            Some(bytes) => fsutil::write_atomic(path, bytes),
            None => remove_file(path),
        };
        match done {
            Ok(()) => {
                if journal.iter().any(|(p, _)| p == path) && !restored.contains(path) {
                    restored.push(path.clone());
                }
            }
            Err(e) => failed.push(e),
        }
    }
    let names: Vec<String> = restored
        .iter()
        .map(|p| setup.show(&p.display().to_string()))
        .collect();
    if failed.is_empty() {
        format!(
            "{err}. Install stopped and put back what it had written ({}); nothing is installed. \
             Fix that, then run install again (or leave this agent out with --harness)",
            if names.is_empty() {
                "nothing yet".to_string()
            } else {
                names.join(", ")
            }
        )
    } else {
        format!(
            "{err}. Install stopped; it put back {} but could not put back: {} (the copies from \
             before install are in {}). `graphersal-memory uninstall` removes what is left",
            if names.is_empty() {
                "nothing".to_string()
            } else {
                names.join(", ")
            },
            failed.join("; "),
            setup.show(&setup.state_dir().join("backups").display().to_string())
        )
    }
}

/// The lines about detection before the plan: harnesses whose configuration is here but whose
/// program is not (not offered), and `--harness` naming a harness whose program is missing
/// (installed anyway, with a warning).
fn detection_notes(setup: &Setup, options: &InstallOptions) -> Vec<String> {
    let mut notes = Vec::new();
    let named = options.harnesses.clone().unwrap_or_default();
    for kind in &named {
        if setup.program(*kind).is_none() {
            notes.push(format!(
                "warning: {}'s program `{}` is not found (not on PATH, not in its usual \
                 places); installing anyway because --harness names it",
                kind.display_name(),
                Setup::program_name(*kind)
            ));
        }
    }
    for (kind, path) in setup.leftovers() {
        // Named with --harness: warned above.
        if named.contains(&kind) {
            continue;
        }
        notes.push(setup.show(&format!(
            "note: {}: configuration found ({}), but the program `{}` is not installed: not \
             offered (`--harness {}` installs anyway)",
            kind.display_name(),
            path.display(),
            Setup::program_name(kind),
            kind.id()
        )));
    }
    notes
}

/// `install` without `--harness` when no agent's program is found: nothing is changed; the
/// message names what was looked for and where, the leftover configurations, and the way
/// out. Returns the exit code (1).
fn refuse_without_programs(setup: &Setup, out: &mut dyn Write, color: bool) -> u8 {
    let _ = writeln!(
        out,
        "{}",
        paint(
            color,
            "1",
            "graphersal-memory install: no agent program found"
        )
    );
    let _ = writeln!(
        out,
        "  looked for: claude, opencode, codex and cursor on PATH, and in ~/.local/bin/claude, \
         ~/.claude/local/claude, ~/.opencode/bin/opencode and Cursor's app"
    );
    for (kind, path) in setup.leftovers() {
        let _ = writeln!(
            out,
            "{}",
            setup.show(&format!(
                "  configuration found without its program: {} ({})",
                kind.display_name(),
                path.display()
            ))
        );
    }
    let _ = writeln!(
        out,
        "\nNothing was changed. Install an agent first, or name one to install anyway:\n  \
         graphersal-memory install --harness claude-code|opencode|codex|cursor"
    );
    1
}

/// The wizard's recap: one `(topic, answer)` line per topic.
fn recap(
    setup: &Setup,
    answers: &Answers,
    current: &Current,
    plan: &InstallPlan,
) -> Vec<(&'static str, String)> {
    let mut lines = vec![
        ("Agents", names_of(&answers.harnesses)),
        ("Memory language", answers.language.describe().to_string()),
    ];
    let model = if answers.model == models::BUILTIN {
        format!("the built-in {} (no download)", models::BUILTIN)
    } else if models::cached(&setup.data_dir, &answers.model).is_some() {
        format!("{} (downloaded already)", answers.model)
    } else {
        match models::offered(&answers.model) {
            Some(offered) => format!(
                "{} ({} download, {})",
                answers.model, offered.size, offered.license
            ),
            None => format!("{} (downloaded from Hugging Face)", answers.model),
        }
    };
    lines.push(("Embedding model", model));
    if answers.harnesses.contains(&Kind::ClaudeCode) {
        let (_, _, new, changed, _) = import::totals(&plan.imports);
        lines.push((
            "File memory",
            if !answers.import {
                "not imported now (the session start imports a project's files)".to_string()
            } else if new + changed == 0 {
                "nothing new to import".to_string()
            } else {
                format!(
                    "{} memory file{} imported now",
                    new + changed,
                    if new + changed == 1 { "" } else { "s" }
                )
            },
        ));
    }
    if answers.harnesses.contains(&Kind::ClaudeCode) && !current.auto_memory_off_by_user {
        lines.push((
            "Auto-memory",
            if answers.auto_memory_off {
                format!("switched off. {}", start_context_sentence(current))
            } else {
                match &current.start_blocked {
                    Some(reason) => format!("kept on: {reason}"),
                    None => "kept on".to_string(),
                }
            },
        ));
    }
    let todo: usize = answers
        .harnesses
        .iter()
        .filter_map(|kind| {
            let dir = setup.agent_of(*kind).projects_dir().to_path_buf();
            let request = crate::sessions::Request::for_harness(*kind, &dir);
            crate::sessions::plan(&request, &setup.data_dir)
                .ok()
                .map(|(_, _, todo, _)| todo)
        })
        .sum();
    lines.push((
        "Past sessions",
        format!(
            "{todo} to index locally in the background (raw turns{})",
            if answers.summaries {
                ", then summaries"
            } else {
                ""
            }
        ),
    ));
    lines.push((
        "Summaries",
        if answers.summaries {
            format!(
                "on, through `{}`, at most ${:.2} per month",
                summaries_command(&current.config, &answers.harnesses).join(" "),
                current.config.summaries.monthly_budget_usd
            )
        } else {
            "off".to_string()
        },
    ));
    if answers.develop {
        lines.push(("Develop server", format!("{DEVELOP_SERVER} registered")));
    }
    let mut others = Vec::new();
    if !answers.read_only.is_empty() {
        others.push(format!(
            "graphersal-memory is the main memory; {} read-only",
            answers.read_only.join(", ")
        ));
    }
    if !answers.disable_other.is_empty() {
        others.push(format!(
            "{} switched off (uninstall switches {} on again)",
            answers.disable_other.join(", "),
            if answers.disable_other.len() == 1 {
                "it"
            } else {
                "them"
            }
        ));
    }
    if !answers.kept.is_empty() {
        others.push(format!("{} kept as {}", answers.kept.join(", "), {
            if answers.kept.len() == 1 {
                "it is"
            } else {
                "they are"
            }
        }));
    }
    if answers.import_other && current.other_pending > 0 {
        others.push(format!(
            "{} memor{} imported",
            current.other_pending,
            if current.other_pending == 1 {
                "y"
            } else {
                "ies"
            }
        ));
    }
    if !current.others.mentions.is_empty() {
        others.push(format!(
            "{} instruction file line{} about memory elsewhere, never edited (`doctor` shows {})",
            current.others.mentions.len(),
            if current.others.mentions.len() == 1 {
                ""
            } else {
                "s"
            },
            if current.others.mentions.len() == 1 {
                "it"
            } else {
                "them"
            }
        ));
    }
    if !others.is_empty() {
        lines.push(("Other memory", others.join("; ")));
    }
    lines
}

/// What install reports after applying.
struct InstallReport<'a> {
    /// What changed per agent.
    agents: &'a [(Kind, Vec<String>)],
    /// What changed in the memory (settings, model, import, summaries, sessions).
    general: &'a [String],
    /// The backup directories written.
    backups: &'a std::collections::BTreeSet<PathBuf>,
    conflicts: &'a [String],
    /// Nothing in the agents' files needed a change.
    up_to_date: bool,
    harnesses: &'a [Kind],
    /// Codex's hooks still need the user's approval.
    codex_approval: bool,
}

/// The report: the backups once, a section per agent and one for the memory, then the "Next
/// steps" block after a separator, numbered, with ACTION NEEDED highlighted.
fn print_install_report(
    out: &mut dyn Write,
    setup: &Setup,
    report: &InstallReport<'_>,
    color: bool,
) {
    let _ = writeln!(out, "\n{}", paint(color, "1", "Report:"));
    for dir in report.backups {
        let _ = writeln!(
            out,
            "  Backups of the files changed: {}",
            setup.show(&format!("{}{}", dir.display(), std::path::MAIN_SEPARATOR))
        );
    }
    for (kind, lines) in report.agents {
        let _ = writeln!(out, "\n{}:", kind.display_name());
        if lines.is_empty() {
            let _ = writeln!(out, "  already up to date");
        }
        for line in lines {
            let _ = writeln!(out, "  {}", setup.show(line));
        }
    }
    let _ = writeln!(out, "\nMemory:");
    for line in report.general {
        let _ = writeln!(out, "  {}", setup.show(line));
    }
    for conflict in report.conflicts {
        let line = paint(color, "33", &format!("conflict: {}", setup.show(conflict)));
        let _ = writeln!(out, "  {line}");
    }
    if report.up_to_date {
        let _ = writeln!(
            out,
            "  {} configuration: already up to date",
            names_of(report.harnesses)
        );
    }
    let opencode = report.harnesses.contains(&Kind::OpenCode);
    let hooks = report.harnesses.iter().any(|k| *k != Kind::OpenCode);
    let parts = match (hooks, opencode) {
        (true, true) => "the MCP server, hooks and plugin",
        (false, true) => "the MCP server and plugin",
        _ => "the MCP server and hooks",
    };
    let mut steps = vec![format!(
        "Restart running {} sessions to load {parts}.",
        names_of(report.harnesses)
    )];
    if report.codex_approval {
        steps.push(crate::prompt::action_needed(
            color,
            &crate::codex::install::hook_approval_line(),
        ));
    }
    let _ = writeln!(out, "\n{}", crate::prompt::SEPARATOR);
    let _ = writeln!(out, "{}", paint(color, "1", "Next steps"));
    for (n, step) in steps.iter().enumerate() {
        let _ = writeln!(out, "  {}. {step}", n + 1);
    }
    let _ = writeln!(
        out,
        "  `graphersal-memory status` shows what is installed; `graphersal-memory uninstall` \
         removes it."
    );
}

/// One line about an import.
pub fn import_summary(report: &import::ImportReport) -> String {
    let mut line = format!(
        "imported {} memor{} ({} new, {} updated) into {} project{}; {} unchanged",
        report.created + report.updated,
        if report.created + report.updated == 1 {
            "y"
        } else {
            "ies"
        },
        report.created,
        report.updated,
        report.projects.len(),
        if report.projects.len() == 1 { "" } else { "s" },
        report.unchanged
    );
    if !report.skipped_dirs.is_empty() {
        line.push_str(&format!(
            "; skipped {} director{} whose project no longer exists ({})",
            report.skipped_dirs.len(),
            if report.skipped_dirs.len() == 1 {
                "y"
            } else {
                "ies"
            },
            report.skipped_dirs.join(", ")
        ));
    }
    if report.kind_defaulted > 0 {
        line.push_str(&format!(
            "; {} without a known type became kind project",
            report.kind_defaulted
        ));
    }
    if report.kept_forgotten > 0 {
        line.push_str(&format!(
            "; {} forgotten in the memory and unchanged in its file, kept forgotten",
            report.kept_forgotten
        ));
    }
    if report.unchecked > 0 {
        line.push_str(&format!(
            "; {} {} similar memories to check for conflicts (your agent: conflicts())",
            report.unchecked,
            if report.unchecked == 1 { "has" } else { "have" }
        ));
    }
    if report.timed_out {
        line.push_str("; stopped early (time limit), the rest follows next time");
    }
    line
}

/// The lines after an import's summary: its projects' open memory conflicts, the files that
/// still hold a superseded memory, each named with its file, and the changed files whose
/// memory was edited in graphersal-memory (empty when there are none).
pub fn import_conflict_lines(report: &import::ImportReport) -> Vec<String> {
    let mut lines = Vec::new();
    if !report.conflicts.is_empty() {
        lines.push(format!(
            "open memory conflicts: {} (ask your agent to settle them: conflicts(); the file of \
             the memory that loses must change too)",
            report.conflicts.len()
        ));
        lines.extend(report.conflicts.iter().map(|line| format!("  {line}")));
    }
    if !report.stale_files.is_empty() {
        lines.push(format!(
            "files that still hold a superseded memory: {} (edit or delete them)",
            report.stale_files.len()
        ));
        lines.extend(report.stale_files.iter().map(|line| format!("  {line}")));
    }
    if !report.returning.is_empty() {
        lines.push(format!(
            "files that brought back a memory changed or forgotten in the graph: {}",
            report.returning.len()
        ));
        lines.extend(report.returning.iter().map(|line| format!("  {line}")));
    }
    if !report.kept_edited.is_empty() {
        lines.push(format!(
            "changed files whose memory was edited in graphersal-memory (the edit was kept): {}",
            report.kept_edited.len()
        ));
        lines.extend(report.kept_edited.iter().map(|line| format!("  {line}")));
    }
    lines
}

/// The structural removal of our parts from one file's current text: the new text, or `None`
/// when nothing of ours is in it.
fn remove_ours(
    setup: &Setup,
    path: &Path,
    text: &str,
    entry: Option<&serde_json::Map<String, Value>>,
    develop_only: bool,
    conflicts: &mut Vec<String>,
) -> Result<Option<String>, String> {
    let created: Vec<String> = entry
        .and_then(|e| e.get("created"))
        .and_then(Value::as_array)
        .map(|c| {
            c.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut current = text.to_string();
    let mut changed = false;
    if path == setup.paths.user_config {
        let parsed: Value = serde_json::from_str(text)
            .map_err(|err| format!("{} is not valid JSON: {err}", path.display()))?;
        let names: &[&str] = if develop_only {
            &[DEVELOP_SERVER]
        } else {
            &[USE_SERVER, DEVELOP_SERVER]
        };
        for name in names {
            if setup.ours().entry(&parsed["mcpServers"][*name])
                && let Some(new) = jsonedit::remove_member(&current, &["mcpServers"], name)
                    .map_err(|e| crate::install::in_file(path, e))?
            {
                current = new;
                changed = true;
            }
        }
        if changed
            && created.iter().any(|c| c == "mcpServers")
            && jsonedit::is_empty_container(&current, &["mcpServers"])
            && let Some(new) = jsonedit::remove_member(&current, &[], "mcpServers")
                .map_err(|e| crate::install::in_file(path, e))?
        {
            current = new;
        }
    } else if path == setup.paths.settings {
        let created: Vec<&str> = created.iter().map(String::as_str).collect();
        if let Some(new) = remove_hook_groups(&setup.ours(), &current, &created)
            .map_err(|e| crate::install::in_file(path, e))?
        {
            current = new;
            changed = true;
        }
        // The auto-memory switch install turned off goes back to what it was.
        if let Some(record) = entry.and_then(|e| e.get(AUTO_MEMORY_RECORD))
            && let Some(key) = setup.agent().auto_memory_key()
        {
            let parsed: Value = serde_json::from_str(&current)
                .map_err(|err| format!("{} is not valid JSON: {err}", path.display()))?;
            if parsed[key] == Value::Bool(false) {
                let previous = &record["previous"];
                let restored = if previous.is_null() {
                    jsonedit::remove_member(&current, &[], key)
                        .map_err(|e| crate::install::in_file(path, e))?
                } else {
                    jsonedit::set_member(&current, &[], key, previous)?.map(|(new, _)| new)
                };
                if let Some(new) = restored {
                    current = new;
                    changed = true;
                }
            }
        }
    } else if path == setup.paths.instructions {
        match find_block(text) {
            Ok(Some(found)) if found.intact => {
                let separator = entry
                    .and_then(|e| e.get("separator"))
                    .and_then(Value::as_str)
                    .unwrap_or("\n");
                let mut head = &text[..found.start];
                if found.end == text.len() && !separator.is_empty() && head.ends_with(separator) {
                    head = &head[..head.len() - separator.len()];
                }
                current = format!("{head}{}", &text[found.end..]);
                changed = true;
            }
            Ok(Some(_)) => conflicts.push(format!(
                "{}: the managed block was edited by hand, so it was not removed; delete it \
                 yourself (from the begin to the end marker)",
                path.display()
            )),
            Err(err) => conflicts.push(format!("{}: {err}; not changed", path.display())),
            Ok(None) => {}
        }
    }
    Ok(changed.then_some(current))
}

/// What [`plan_hook_event`] does with one event.
pub(crate) enum HookStep {
    /// Our hook is there as it should be (in a group of its own, or inside a group with the
    /// user's hooks, where the user moved it).
    Installed,
    /// Added.
    Added,
    /// Replaced (an older one, or ours in a shared group).
    Updated,
}

/// Plans our hook group of `event` in `text` (`hooks.<event>` groups): kept when it is there
/// already, else every hook of ours goes (whole groups, and ours inside groups shared with the
/// user's hooks) and `group` is appended. Returns the new text and the containers it created.
pub(crate) fn plan_hook_event(
    ours: &Ours,
    text: &str,
    event: &str,
    group: &Value,
) -> Result<(String, Vec<String>, HookStep), String> {
    let parsed = jsonedit::value_of(text).map_err(|err| err.to_string())?;
    let groups = parsed["hooks"][event]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let whole: Vec<&Value> = groups.iter().filter(|g| ours.hook_group(g)).collect();
    let inside: Vec<&Value> = groups
        .iter()
        .filter(|g| ours.mixed_group(g))
        .flat_map(|g| g["hooks"].as_array().into_iter().flatten())
        .filter(|h| ours.hook(h))
        .collect();
    let wanted = &group["hooks"][0];
    if (whole.len() == 1 && whole[0] == group && inside.is_empty())
        || (whole.is_empty() && inside.len() == 1 && inside[0] == wanted)
    {
        return Ok((text.to_string(), Vec::new(), HookStep::Installed));
    }
    let mut text = text.to_string();
    if let Some(new) =
        jsonedit::remove_elements(&text, &["hooks", event], &|g: &Value| ours.hook_group(g))?
    {
        text = new;
    }
    if let Some(new) =
        jsonedit::remove_inner_elements(&text, &["hooks", event], "hooks", &|h: &Value| {
            ours.hook(h)
        })?
    {
        text = new;
    }
    let (new, made) = jsonedit::append_element(&text, &["hooks", event], group)?;
    let step = if whole.is_empty() && inside.is_empty() {
        HookStep::Added
    } else {
        HookStep::Updated
    };
    Ok((new, made, step))
}

/// `text` (a settings or `hooks.json` file with `hooks.<Event>` groups) without our hook
/// groups and without our hooks inside groups shared with the user's hooks (someone moved ours
/// there), then without the containers install created (`created`, dotted) that are empty now.
/// `None` when none of ours is in it.
pub(crate) fn remove_hook_groups(
    ours: &Ours,
    text: &str,
    created: &[&str],
) -> Result<Option<String>, String> {
    let mut current = text.to_string();
    let mut changed = false;
    for (event, _) in HOOK_EVENTS {
        if let Some(new) =
            jsonedit::remove_elements(&current, &["hooks", event], &|g: &Value| ours.hook_group(g))?
        {
            current = new;
            changed = true;
        }
        if let Some(new) =
            jsonedit::remove_inner_elements(&current, &["hooks", event], "hooks", &|h: &Value| {
                ours.hook(h)
            })?
        {
            current = new;
            changed = true;
        }
        let dotted = format!("hooks.{event}");
        if changed
            && created.contains(&dotted.as_str())
            && jsonedit::is_empty_container(&current, &["hooks", event])
            && let Some(new) = jsonedit::remove_member(&current, &["hooks"], event)?
        {
            current = new;
        }
    }
    if changed
        && created.contains(&"hooks")
        && jsonedit::is_empty_container(&current, &["hooks"])
        && let Some(new) = jsonedit::remove_member(&current, &[], "hooks")?
    {
        current = new;
    }
    Ok(changed.then_some(current))
}

/// Whether a file install created holds nothing but what it would start from.
pub(crate) fn is_empty_remainder(path: &Path, text: &str) -> bool {
    if path
        .extension()
        .is_some_and(|e| e == "json" || e == "jsonc")
    {
        jsonedit::value_of(text)
            .ok()
            .is_some_and(|v| v.as_object().is_some_and(|o| o.is_empty()))
    } else {
        text.trim().is_empty()
    }
}

#[cfg(test)]
/// `uninstall` without a memory source: valid only with `no_export` (or
/// `disable_develop_only`), else it fails before changing anything.
pub fn uninstall(
    setup: &Setup,
    options: &UninstallOptions,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<u8, String> {
    uninstall_with(setup, options, None, input, out)
}

/// `uninstall`: first exports every memory back into the harness's file memory (from `source`;
/// unless `no_export`), then removes exactly what install added (or only the develop server),
/// which also switches the harness's own memory back to how it was. Returns the exit code.
pub fn uninstall_with(
    setup: &Setup,
    options: &UninstallOptions,
    source: Option<&mut dyn export::Source>,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<u8, String> {
    let kinds = options.harnesses.clone().unwrap_or_else(|| {
        let installed = setup.installed();
        if installed.is_empty() {
            vec![Kind::ClaudeCode]
        } else {
            installed
        }
    });
    // Nothing installed in the harnesses asked for: nothing to export or remove (no daemon is
    // started, nothing is created); `--purge` still deletes the data.
    let installed = setup.installed();
    let nothing_installed = !kinds.iter().any(|kind| installed.contains(kind));
    let exporting = !options.disable_develop_only && !options.no_export && !nothing_installed;
    // No store: no memory to export (and no daemon started to find that out).
    let no_store = !crate::store::default_store_dir(&setup.data_dir).exists();
    // The memories first: if they cannot be read, nothing is removed.
    let rows = if exporting && !options.dry_run && no_store {
        Some(Vec::new())
    } else if exporting && !options.dry_run {
        let source = source.ok_or(
            "the memories cannot be exported here; nothing was removed (`--no-export` \
             uninstalls without exporting)",
        )?;
        Some(source.memories("").map_err(|err| {
            format!(
                "cannot read the memories to export them ({err}); nothing was removed. \
                 `--no-export` uninstalls without exporting."
            )
        })?)
    } else {
        None
    };
    let state = State::load(setup)?;
    let opencode_state = State::load_of(setup, Kind::OpenCode)?;
    let codex_state = State::load_of(setup, Kind::Codex)?;
    let cursor_state = State::load_of(setup, Kind::Cursor)?;
    let codewhale_state = State::load_of(setup, Kind::CodeWhale)?;
    let mut conflicts = Vec::new();
    let mut changes: Vec<(Kind, Change, &'static str)> = Vec::new();
    if kinds.contains(&Kind::ClaudeCode) {
        for (change, what) in claude_removal(setup, options, &state, &mut conflicts)
            .map_err(|err| way_out(&err, &kinds, "uninstall", "uninstall from"))?
        {
            changes.push((Kind::ClaudeCode, change, what));
        }
    }
    if kinds.contains(&Kind::OpenCode) {
        for (change, what) in crate::opencode::install::removal(
            setup,
            &opencode_state,
            options.disable_develop_only,
            &mut conflicts,
        )
        .map_err(|err| way_out(&err, &kinds, "uninstall", "uninstall from"))?
        {
            changes.push((Kind::OpenCode, change, what));
        }
    }
    if kinds.contains(&Kind::Codex) {
        for (change, what) in crate::codex::install::removal(
            setup,
            &codex_state,
            options.disable_develop_only,
            &mut conflicts,
        )
        .map_err(|err| way_out(&err, &kinds, "uninstall", "uninstall from"))?
        {
            changes.push((Kind::Codex, change, what));
        }
    }
    if kinds.contains(&Kind::Cursor) {
        for (change, what) in crate::cursor::install::removal(
            setup,
            &cursor_state,
            options.disable_develop_only,
            &mut conflicts,
        )
        .map_err(|err| way_out(&err, &kinds, "uninstall", "uninstall from"))?
        {
            changes.push((Kind::Cursor, change, what));
        }
    }
    if kinds.contains(&Kind::CodeWhale) {
        for (change, what) in crate::codewhale::install::removal(
            setup,
            &codewhale_state,
            options.disable_develop_only,
            &mut conflicts,
        )
        .map_err(|err| way_out(&err, &kinds, "uninstall", "uninstall from"))?
        {
            changes.push((Kind::CodeWhale, change, what));
        }
    }
    // Other memory tools' entries install switched off go back on (M15).
    let mut restored: Vec<(Kind, String)> = Vec::new();
    if !options.disable_develop_only {
        for (kind, kind_state) in [
            (Kind::ClaudeCode, &state),
            (Kind::OpenCode, &opencode_state),
            (Kind::Codex, &codex_state),
            (Kind::Cursor, &cursor_state),
            (Kind::CodeWhale, &codewhale_state),
        ] {
            if !kinds.contains(&kind) {
                continue;
            }
            restore_others(kind, kind_state, &mut changes, &mut restored)
                .map_err(|err| way_out(&err, &kinds, "uninstall", "uninstall from"))?;
        }
    }
    // The harnesses it actually changes (or is recorded in); the title names only these.
    let active: Vec<Kind> = kinds
        .iter()
        .copied()
        .filter(|kind| changes.iter().any(|(k, _, _)| k == kind) || State::exists_of(setup, *kind))
        .collect();
    let shown = if active.is_empty() {
        kinds.clone()
    } else {
        active.clone()
    };
    let names = names_of(&shown);
    let title = if options.disable_develop_only {
        format!("graphersal-memory uninstall --disable-develop ({names})")
    } else {
        format!("graphersal-memory uninstall ({names})")
    };
    let _ = writeln!(out, "{}", paint(options.color, "1", &title));
    let export_line = |agent: &dyn Agent, rows: Option<&Vec<export::Row>>| -> String {
        let target = crate::commands::export_target(agent);
        match rows {
            Some(rows) => {
                let projects: std::collections::BTreeSet<&str> = rows
                    .iter()
                    .filter(|r| !r.project.is_empty())
                    .map(|r| r.project.as_str())
                    .collect();
                let global = rows.iter().filter(|r| r.project.is_empty()).count();
                if agent.has_file_memory() {
                    format!(
                        "export {} memor{} of {} project{} ({global} global) back to {target} \
                         (MEMORY.md and one file per memory; the global ones into {}); a file \
                         that differs is kept",
                        rows.len(),
                        if rows.len() == 1 { "y" } else { "ies" },
                        projects.len(),
                        if projects.len() == 1 { "" } else { "s" },
                        agent.global_memory_file().display()
                    )
                } else {
                    format!(
                        "export {} memor{} of {} project{} ({global} global) into {target} \
                         ({} has no file memory; nothing is deleted)",
                        rows.len(),
                        if rows.len() == 1 { "y" } else { "ies" },
                        projects.len(),
                        if projects.len() == 1 { "" } else { "s" },
                        agent.display_name()
                    )
                }
            }
            None if agent.has_file_memory() => {
                format!("export every memory back to {target} first (`--no-export` skips it)")
            }
            None => format!("export every memory into {target} first (`--no-export` skips it)"),
        }
    };
    // The memories go to the harnesses it is removed from.
    let export_to: Vec<Kind> = if active.is_empty() {
        kinds.clone()
    } else {
        active.clone()
    };
    if exporting {
        let _ = writeln!(out, "\nFirst, the memories:");
        for kind in &export_to {
            let agent = setup.agent_of(*kind);
            let line = setup.show(&export_line(agent.as_ref(), rows.as_ref()));
            let _ = writeln!(out, "  - {line}");
        }
    }
    if changes.is_empty() && conflicts.is_empty() && !exporting {
        let _ = writeln!(
            out,
            "\nNothing of graphersal-memory is installed in {names}."
        );
    }
    for kind in &shown {
        let mine: Vec<_> = changes.iter().filter(|(k, _, _)| k == kind).collect();
        if mine.is_empty() {
            continue;
        }
        let _ = writeln!(out, "\n{}:", kind.display_name());
        for (_, change, what) in mine {
            let _ = writeln!(
                out,
                "  - {}: {what}",
                setup.show(&change.path.display().to_string())
            );
        }
        for (_, line) in restored.iter().filter(|(k, _)| k == kind) {
            let _ = writeln!(out, "  - {}", setup.show(line));
        }
    }
    if !conflicts.is_empty() {
        let _ = writeln!(out);
    }
    for conflict in &conflicts {
        let line = paint(
            options.color,
            "33",
            &format!("conflict: {}", setup.show(conflict)),
        );
        let _ = writeln!(out, "  {line}");
    }
    if options.purge {
        let _ = writeln!(
            out,
            "\nThen: delete the data directory {} with every memory in it (the daemon is \
             stopped first)",
            setup.show(&setup.data_dir.display().to_string())
        );
    }
    if options.dry_run {
        let _ = writeln!(out, "\nDry run: nothing was changed.");
        return Ok(0);
    }
    let mut export_rows = rows;
    if options.interactive
        && !options.yes
        && let Some(rows) = &export_rows
        && !rows.is_empty()
    {
        let projects: std::collections::BTreeSet<&str> = rows
            .iter()
            .map(|r| r.project.as_str())
            .filter(|p| !p.is_empty())
            .collect();
        let targets = export_to
            .iter()
            .map(|k| {
                let agent = setup.agent_of(*k);
                let target = crate::commands::export_target(agent.as_ref());
                if agent.has_file_memory() {
                    format!("back to {target}")
                } else {
                    format!("into {target}")
                }
            })
            .collect::<Vec<_>>()
            .join(" and ");
        let question = setup.show(&format!(
            "\nExport {} memor{} of {} project{} {targets}?",
            rows.len(),
            if rows.len() == 1 { "y" } else { "ies" },
            projects.len(),
            if projects.len() == 1 { "" } else { "s" },
        ));
        if ask_yes_no(&question, true, input, out) == Reply::No {
            export_rows = None;
        }
    }
    let needs_ok = !changes.is_empty() || options.purge;
    if needs_ok && !options.yes {
        let reply = if options.purge {
            ask_yes_no(
                "\nThis deletes your memories for good. Uninstall and delete them?",
                false,
                input,
                out,
            )
        } else {
            ask_yes_no("\nUninstall?", true, input, out)
        };
        match reply {
            Reply::Yes => {}
            Reply::No => {
                let _ = writeln!(out, "Cancelled: nothing was changed.");
                return Ok(1);
            }
            Reply::NoInput => {
                let _ = writeln!(out, "No answer, nothing was changed: {NO_TERMINAL}.");
                return Ok(1);
            }
        }
    }
    // Nothing is written while another install or uninstall runs.
    let _lock = InstallLock::take(setup, false)?;
    let mut report = Vec::new();
    let mut agent_report: Vec<(Kind, Vec<String>)> =
        shown.iter().map(|kind| (*kind, Vec::new())).collect();
    let mut push_for = |kind: Kind, line: String, report: &mut Vec<String>| match agent_report
        .iter_mut()
        .find(|(k, _)| *k == kind)
    {
        Some((_, lines)) => lines.push(line),
        None => report.push(line),
    };
    if let Some(rows) = &export_rows {
        if rows.is_empty() {
            report.push("exported nothing: the graph holds no memories".to_string());
        } else {
            for kind in &export_to {
                let agent = setup.agent_of(*kind);
                report.extend(export::write(agent.as_ref(), &setup.data_dir, rows)?.lines());
                if !agent.has_file_memory() {
                    report.push(crate::commands::export_note(agent.as_ref()));
                }
            }
        }
    } else if exporting {
        report.push("not exported (you answered no)".to_string());
    }
    if !options.disable_develop_only
        && kinds.contains(&Kind::ClaudeCode)
        && state
            .file(&setup.paths.settings)
            .is_some_and(|entry| entry.contains_key(AUTO_MEMORY_RECORD))
        && changes
            .iter()
            .any(|(_, change, _)| change.path == setup.paths.settings)
    {
        // Back only when it is still off; a value the user set since install stays.
        let off_now = setup.agent().auto_memory_key().is_some_and(|key| {
            read_json(&setup.paths.settings, &mut Vec::new())[key] == Value::Bool(false)
        });
        push_for(
            Kind::ClaudeCode,
            if off_now {
                format!(
                    "{}'s auto-memory: back to how it was before install",
                    setup.agent().display_name()
                )
            } else {
                format!(
                    "{}'s auto-memory: kept as you set it after install",
                    setup.agent().display_name()
                )
            },
            &mut report,
        );
    }
    if !options.disable_develop_only
        && kinds.contains(&Kind::Codex)
        && codex_state
            .file(&setup.codex.config_file)
            .is_some_and(|entry| entry.contains_key(crate::codex::install::MEMORIES_RECORD))
        && changes
            .iter()
            .any(|(_, change, _)| change.path == setup.codex.config_file)
    {
        push_for(
            Kind::Codex,
            "Codex's memories: switched on again, as before install".to_string(),
            &mut report,
        );
    }
    if !options.disable_develop_only
        && kinds.contains(&Kind::CodeWhale)
        && codewhale_state
            .file(&setup.codewhale.config_file)
            .is_some_and(|entry| entry.contains_key(crate::codewhale::install::MEMORY_RECORD))
        && changes
            .iter()
            .any(|(_, change, _)| change.path == setup.codewhale.config_file)
    {
        push_for(
            Kind::CodeWhale,
            "CodeWhale's memory: switched on again, as before install".to_string(),
            &mut report,
        );
    }
    for (kind, line) in &restored {
        push_for(*kind, line.clone(), &mut report);
    }
    let mut states = [
        (Kind::ClaudeCode, state),
        (Kind::OpenCode, opencode_state),
        (Kind::Codex, codex_state),
        (Kind::Cursor, cursor_state),
        (Kind::CodeWhale, codewhale_state),
    ];
    for (kind, change, _) in &changes {
        match &change.after {
            Some(text) => {
                fsutil::write_atomic(&change.path, text.as_bytes())?;
                push_for(
                    *kind,
                    format!("changed {}", change.path.display()),
                    &mut report,
                );
                if options.disable_develop_only && State::exists_of(setup, *kind) {
                    for (k, state) in states.iter_mut() {
                        if k == kind {
                            state.file_mut(&change.path).insert(
                                "written".into(),
                                json!(fsutil::fingerprint(text.as_bytes())),
                            );
                        }
                    }
                }
            }
            None => {
                remove_file(&change.path)?;
                push_for(
                    *kind,
                    format!("removed {}", change.path.display()),
                    &mut report,
                );
            }
        }
    }
    for (kind, state) in states.iter_mut() {
        if !kinds.contains(kind) {
            continue;
        }
        if options.disable_develop_only {
            if State::exists_of(setup, *kind) {
                state.save(setup)?;
            }
        } else {
            let mut dirs = state.created_dirs();
            dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
            for dir in dirs {
                // Only when empty: a directory someone else filled meanwhile stays.
                if std::fs::remove_dir(&dir).is_ok() {
                    push_for(
                        *kind,
                        format!("removed the directory {}", dir.display()),
                        &mut report,
                    );
                }
            }
            if conflicts.is_empty() {
                remove_file(&State::path_of(setup, *kind))?;
            }
        }
    }
    // The choices about other memory tools go with the last harness.
    if !options.disable_develop_only && conflicts.is_empty() && setup.installed().is_empty() {
        crate::coexist::Choices::remove(setup)?;
    }
    if options.purge {
        report.extend(purge(setup)?);
    }
    let _ = writeln!(out, "\n{}", paint(options.color, "1", "Report:"));
    let nothing = report.is_empty() && agent_report.iter().all(|(_, lines)| lines.is_empty());
    if nothing {
        let _ = writeln!(out, "  nothing to change");
    }
    for line in &report {
        let _ = writeln!(out, "  {}", setup.show(line));
    }
    for (kind, lines) in &agent_report {
        if lines.is_empty() {
            continue;
        }
        let _ = writeln!(out, "\n{}:", kind.display_name());
        for line in lines {
            let _ = writeln!(out, "  {}", setup.show(line));
        }
    }
    if !conflicts.is_empty() {
        let _ = writeln!(out);
    }
    for conflict in &conflicts {
        let line = paint(
            options.color,
            "33",
            &format!("conflict: {}", setup.show(conflict)),
        );
        let _ = writeln!(out, "  {line}");
    }
    if !options.disable_develop_only && !options.purge {
        let _ = writeln!(
            out,
            "\nYour memories stay in {} (`uninstall --purge` deletes them).",
            setup.show(&setup.data_dir.display().to_string())
        );
    }
    Ok(if conflicts.is_empty() { 0 } else { 1 })
}

/// The entries of other memory tools that install switched off in `kind`'s files (recorded in
/// `state`), switched on again: folded into the removal's change of that file (a file put back
/// from its backup has them already), or a change of their own. One line per entry in
/// `restored`.
fn restore_others(
    kind: Kind,
    state: &State,
    changes: &mut Vec<(Kind, Change, &'static str)>,
    restored: &mut Vec<(Kind, String)>,
) -> Result<(), String> {
    for (file, entry) in state.files() {
        let Some(records) = entry
            .get(crate::coexist::OFF_RECORD)
            .and_then(Value::as_object)
        else {
            continue;
        };
        let path = PathBuf::from(file);
        for (name, record) in records {
            let line = format!(
                "{name}: switched on again in {} (install had switched it off)",
                path.display()
            );
            match changes
                .iter_mut()
                .find(|(k, change, _)| *k == kind && change.path == path)
            {
                Some((_, change, what)) => match &change.after {
                    Some(text) => {
                        match crate::coexist::restore_edit(kind, text, name, record)
                            .map_err(|e| in_file(&path, e))?
                        {
                            Some(new) => change.after = Some(new),
                            None if what.starts_with("restore the file") => {}
                            None => continue,
                        }
                        restored.push((kind, line));
                    }
                    // A file install created whose rest is empty: kept for the entry.
                    None if record["how"] == "removed" => {
                        let keys: Vec<&str> = record["path"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .collect();
                        if let Some((new, _)) =
                            jsonedit::set_member(EMPTY_OBJECT, &keys, name, &record["entry"])?
                        {
                            change.after = Some(new);
                            *what = "remove our parts (the entry of another tool stays)";
                            restored.push((kind, line));
                        }
                    }
                    None => {}
                },
                None => {
                    let Some(text) = read_text(&path)? else {
                        continue;
                    };
                    if let Some(new) = crate::coexist::restore_edit(kind, &text, name, record)
                        .map_err(|e| in_file(&path, e))?
                    {
                        changes.push((
                            kind,
                            Change {
                                path: path.clone(),
                                after: Some(new),
                                created: Vec::new(),
                                separator: None,
                            },
                            "switch on again what install switched off",
                        ));
                        restored.push((kind, line));
                    }
                }
            }
        }
    }
    Ok(())
}

/// The removal of our parts from Claude Code's files: one `(change, what)` per file.
fn claude_removal(
    setup: &Setup,
    options: &UninstallOptions,
    state: &State,
    conflicts: &mut Vec<String>,
) -> Result<Vec<(Change, &'static str)>, String> {
    let mut changes = Vec::new();
    let targets: Vec<&Path> = if options.disable_develop_only {
        vec![&setup.paths.user_config]
    } else {
        vec![
            &setup.paths.user_config,
            &setup.paths.settings,
            &setup.paths.instructions,
        ]
    };
    for path in targets {
        let Some(bytes) = fsutil::read_optional(path)? else {
            continue;
        };
        // An empty file holds nothing of ours (and is no JSON to parse).
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let entry = state.file(path);
        let fp = fsutil::fingerprint(&bytes);
        let untouched = entry.is_some_and(|e| {
            e.get("written").and_then(Value::as_str) == Some(fp.as_str())
                && e.get("backup_valid") == Some(&Value::Bool(true))
        });
        // A backup that holds our parts (an install that ran twice at once) is never put back.
        let untouched = untouched && !crate::install::backup_holds_ours(entry);
        if untouched && !options.disable_develop_only {
            let existed = entry.is_some_and(|e| e.get("existed") == Some(&Value::Bool(true)));
            let after = if existed {
                let backup = entry
                    .and_then(|e| e.get("backup"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("the backup of {} is not recorded", path.display()))?;
                let original = std::fs::read(backup)
                    .map_err(|err| format!("cannot read the backup {backup}: {err}"))?;
                Some(
                    String::from_utf8(original)
                        .map_err(|_| format!("the backup {backup} is not UTF-8 text"))?,
                )
            } else {
                None
            };
            changes.push((
                Change {
                    path: path.to_path_buf(),
                    after,
                    created: Vec::new(),
                    separator: None,
                },
                if existed {
                    "restore the file as it was before install"
                } else {
                    "delete the file (install created it)"
                },
            ));
            continue;
        }
        let text = String::from_utf8(bytes.clone())
            .map_err(|_| format!("{} is not UTF-8 text", path.display()))?;
        if let Some(new) = remove_ours(
            setup,
            path,
            &text,
            entry,
            options.disable_develop_only,
            conflicts,
        )
        .map_err(|e| in_file(path, e))?
        {
            let created_by_us =
                entry.is_some_and(|e| e.get("existed") == Some(&Value::Bool(false)));
            let delete = created_by_us && is_empty_remainder(path, &new);
            changes.push((
                Change {
                    path: path.to_path_buf(),
                    after: (!delete).then_some(new),
                    created: Vec::new(),
                    separator: None,
                },
                if options.disable_develop_only {
                    "remove the develop server"
                } else if delete {
                    "remove our parts and delete the file (install created it, nothing else is in it)"
                } else {
                    "remove our parts (the rest of the file stays as it is)"
                },
            ));
        }
    }
    Ok(changes)
}

/// Deletes what graphersal-memory created in the data directory (after stopping its daemon):
/// only the entries of [`crate::store::OUR_ENTRIES`], then the directory itself when nothing
/// else is left in it; anything else in it stays and is named in the report. Then the daemon's
/// files in a run directory outside it.
fn purge(setup: &Setup) -> Result<Vec<String>, String> {
    let mut report = Vec::new();
    let data = &setup.data_dir;
    if crate::shim::control(&setup.run_dir, "stop")?.is_some() {
        report.push("stopped the daemon".to_string());
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while crate::ipc::daemon_alive(&setup.run_dir).unwrap_or(false)
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    if data.is_dir() {
        let foreign = crate::store::foreign_entries(data);
        for name in crate::store::OUR_ENTRIES {
            let path = data.join(name);
            let removed = match std::fs::symlink_metadata(&path) {
                Err(_) => continue,
                Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&path),
                Ok(_) => std::fs::remove_file(&path),
            };
            removed.map_err(|err| format!("cannot delete {}: {err}", path.display()))?;
        }
        if foreign.is_empty() {
            // Only what the system puts into any folder is left (or nothing).
            let _ = std::fs::remove_dir_all(data);
            report.push(format!("deleted the data directory {}", data.display()));
        } else {
            report.push(format!(
                "deleted graphersal-memory's data in {}; kept what else is in it ({})",
                data.display(),
                foreign.join(", ")
            ));
        }
    }
    if setup.run_dir.exists() && !setup.run_dir.starts_with(data) {
        for name in [
            crate::ipc::SOCKET_FILE,
            crate::ipc::PID_FILE,
            crate::ipc::START_LOCK_FILE,
            crate::ipc::LOG_FILE,
            crate::ipc::STOPPED_FILE,
            "daemon.log.1",
        ] {
            let _ = std::fs::remove_file(setup.run_dir.join(name));
        }
        if std::fs::remove_dir(&setup.run_dir).is_ok() {
            report.push(format!(
                "deleted the run directory {}",
                setup.run_dir.display()
            ));
        }
    }
    Ok(report)
}

/// The lock that lets one install or uninstall run at a time (`<data dir>/install/install.lock`).
pub(crate) struct InstallLock {
    _lock: Box<fd_lock::RwLock<std::fs::File>>,
}

/// What a second install or uninstall says while the first runs.
pub const ANOTHER_INSTALL: &str = "another graphersal-memory install or uninstall is running \
    (it holds the install lock); nothing was changed. Wait until it ends, then run this again";

impl InstallLock {
    /// Takes the lock, creating the data directory when `create`; `Ok(None)` when the data
    /// directory does not exist and `create` is false (then no install ran and none runs:
    /// an install creates the directory before anything else). `Err` when another process
    /// holds it.
    pub(crate) fn take(setup: &Setup, create: bool) -> Result<Option<Self>, String> {
        let dir = setup.state_dir();
        if !create && !setup.data_dir.is_dir() {
            return Ok(None);
        }
        std::fs::create_dir_all(&dir)
            .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
        let path = dir.join("install.lock");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|err| format!("cannot open {}: {err}", path.display()))?;
        let mut lock = Box::new(fd_lock::RwLock::new(file));
        match lock.try_write() {
            // The guard only borrows the lock; forgetting it keeps the lock held until the
            // file closes, when `InstallLock` is dropped (the OS releases it then).
            Ok(guard) => std::mem::forget(guard),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(ANOTHER_INSTALL.to_string());
            }
            Err(err) => return Err(format!("cannot lock {}: {err}", path.display())),
        }
        Ok(Some(Self { _lock: lock }))
    }
}

/// The state of one installable part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartState {
    /// Not there.
    Missing,
    /// Ours and what this binary would write.
    Installed,
    /// Ours but different (another binary path, an older version).
    Outdated(String),
    /// Something else under our name, or a block edited by hand.
    Conflict(String),
}

impl PartState {
    pub(crate) fn describe(&self) -> String {
        match self {
            PartState::Missing => "not installed".to_string(),
            PartState::Installed => "installed".to_string(),
            PartState::Outdated(why) => format!("installed, outdated ({why})"),
            PartState::Conflict(why) => format!("CONFLICT ({why})"),
        }
    }
}

/// What is installed where.
#[derive(Debug, Clone)]
pub struct Inspection {
    /// The `use` server.
    pub use_server: PartState,
    /// The `develop` server.
    pub develop_server: PartState,
    /// The hooks, per event.
    pub hooks: Vec<(String, PartState)>,
    /// The managed block.
    pub block: PartState,
    /// Files that could not be read or parsed.
    pub errors: Vec<String>,
    /// Registrations of a server named graphersal-memory at another scope (project or local).
    pub other_scopes: Vec<String>,
    /// Commands of our parts whose binary does not exist.
    pub missing_binaries: Vec<String>,
}

impl Inspection {
    /// Whether nothing of ours is installed.
    pub fn nothing_installed(&self) -> bool {
        self.use_server == PartState::Missing
            && self.develop_server == PartState::Missing
            && self.hooks.iter().all(|(_, s)| *s == PartState::Missing)
            && self.block == PartState::Missing
    }
}

pub(crate) fn read_json(path: &Path, errors: &mut Vec<String>) -> Value {
    match read_text(path) {
        Ok(Some(text)) if text.trim().is_empty() => Value::Null,
        Ok(Some(text)) => match jsonedit::value_of(&text) {
            Ok(value) => value,
            Err(err) => {
                errors.push(format!("{} is not valid JSON: {err}", path.display()));
                Value::Null
            }
        },
        Ok(None) => Value::Null,
        Err(err) => {
            errors.push(err);
            Value::Null
        }
    }
}

/// Inspects Claude Code's files.
pub fn inspect(setup: &Setup) -> Inspection {
    let mut errors = Vec::new();
    let mut missing_binaries = Vec::new();
    let config = read_json(&setup.paths.user_config, &mut errors);
    let server = |name: &str, profile: &str, missing: &mut Vec<String>| {
        let entry = &config["mcpServers"][name];
        if entry.is_null() {
            PartState::Missing
        } else if !setup.ours().entry(entry) {
            PartState::Conflict("another program's entry has this name".to_string())
        } else {
            let command = entry["command"].as_str().unwrap_or_default();
            if !Path::new(command).exists() {
                missing.push(command.to_string());
            }
            if *entry == setup.mcp_entry(profile) {
                PartState::Installed
            } else {
                PartState::Outdated(format!("starts {command}"))
            }
        }
    };
    let use_server = server(USE_SERVER, "use", &mut missing_binaries);
    let develop_server = server(DEVELOP_SERVER, "develop", &mut missing_binaries);
    let mut other_scopes = Vec::new();
    if let Some(projects) = config["projects"].as_object() {
        for (project, value) in projects {
            for name in [USE_SERVER, DEVELOP_SERVER] {
                if !value["mcpServers"][name].is_null() {
                    other_scopes.push(format!("{name} (local scope of {project})"));
                }
            }
        }
    }
    let settings = read_json(&setup.paths.settings, &mut errors);
    let hooks = HOOK_EVENTS
        .iter()
        .map(|(event, sub)| {
            let groups = settings["hooks"][event].as_array();
            let ours: Vec<&Value> = groups
                .map(|g| g.iter().filter(|g| setup.ours().hook_group(g)).collect())
                .unwrap_or_default();
            let state = match ours.as_slice() {
                [] => PartState::Missing,
                [one] if **one == setup.hook_group(sub) => PartState::Installed,
                [one] => PartState::Outdated(format!(
                    "runs {}",
                    one["hooks"][0]["command"].as_str().unwrap_or("?")
                )),
                _ => PartState::Outdated("installed more than once".to_string()),
            };
            (event.to_string(), state)
        })
        .collect();
    let language = Config::load(&setup.data_dir)
        .map(|config| config.language())
        .unwrap_or_default();
    let block = match read_text(&setup.paths.instructions) {
        Ok(Some(text)) => {
            match find_block_with(&text, language, &crate::coexist::read_only_names(setup)) {
                Ok(None) => PartState::Missing,
                Ok(Some(found)) if found.current => PartState::Installed,
                Ok(Some(found)) if found.known => {
                    PartState::Outdated("written by an earlier version".to_string())
                }
                Ok(Some(found)) if found.intact => {
                    PartState::Outdated("written by another version".to_string())
                }
                Ok(Some(_)) => PartState::Conflict("edited by hand".to_string()),
                Err(err) => PartState::Conflict(err),
            }
        }
        Ok(None) => PartState::Missing,
        Err(err) => {
            errors.push(err);
            PartState::Missing
        }
    };
    Inspection {
        use_server,
        develop_server,
        hooks,
        block,
        errors,
        other_scopes,
        missing_binaries,
    }
}

/// The installation part of `status`.
pub fn status_lines(setup: &Setup) -> Vec<String> {
    let inspection = inspect(setup);
    let mut lines = vec![
        "Claude Code:".to_string(),
        format!(
            "  user configuration: {}",
            setup.paths.user_config.display()
        ),
    ];
    lines.push(format!(
        "  MCP server {USE_SERVER} (use): {}",
        inspection.use_server.describe()
    ));
    lines.push(format!(
        "  MCP server {DEVELOP_SERVER}: {}",
        match &inspection.develop_server {
            PartState::Missing =>
                "not registered (off; `graphersal-memory install --enable-develop` turns it on)"
                    .to_string(),
            other => other.describe(),
        }
    ));
    for (event, state) in &inspection.hooks {
        lines.push(format!(
            "  hook {event} ({}): {}",
            setup.paths.settings.display(),
            state.describe()
        ));
    }
    lines.push(format!(
        "  rule block ({}): {}",
        setup.paths.instructions.display(),
        inspection.block.describe()
    ));
    let imported = ImportState::load(&setup.data_dir)
        .map(|s| s.len())
        .unwrap_or_default();
    lines.push(format!(
        "  file memory: {imported} memory files imported so far"
    ));
    lines.push(format!(
        "  auto-memory ({}): {}",
        setup.paths.settings.display(),
        auto_memory_state(setup)
    ));
    if setup.hooks.windows {
        let settings = read_json(&setup.paths.settings, &mut Vec::new());
        lines.push(git_bash_status(
            &crate::winshell::find_git_bash_here(&settings),
            &settings,
        ));
    }
    let errors = inspection.errors.clone();
    let nothing = inspection.nothing_installed();
    for error in &errors {
        lines.push(format!("  problem: {error}"));
    }
    if nothing {
        lines.push("  Not installed: run `graphersal-memory install`.".to_string());
    }
    lines
}

/// The status lines every harness shares: the memory language and the embedding model.
pub fn shared_status_lines(setup: &Setup) -> Vec<String> {
    let mut lines = vec!["Memory:".to_string()];
    match Config::load(&setup.data_dir) {
        Ok(config) => {
            lines.push(format!(
                "  memory language: {}",
                config.language().describe()
            ));
            lines.push(format!(
                "  embedding model: {}",
                match &config.model {
                    Some(model) => models::describe(&setup.data_dir, model),
                    None => format!(
                        "{} (the store's own; `graphersal-memory install` offers a better one)",
                        models::BUILTIN
                    ),
                }
            ));
        }
        Err(err) => lines.push(format!("  settings: {err}")),
    }
    lines
}

/// The state of the harness's own memory switch, for `status`.
fn auto_memory_state(setup: &Setup) -> String {
    let agent = setup.agent();
    let Some(key) = agent.auto_memory_key() else {
        return "the harness has none".to_string();
    };
    let mut errors = Vec::new();
    let settings = read_json(agent.settings_file(), &mut errors);
    let ours = State::load(setup).ok().is_some_and(|state| {
        state
            .file(agent.settings_file())
            .is_some_and(|entry| entry.contains_key(AUTO_MEMORY_RECORD))
    });
    match (&settings[key], ours) {
        (Value::Bool(false), true) => format!(
            "off ({key}: false, set by install; uninstall exports every memory back and switches \
             it on again)"
        ),
        (Value::Bool(false), false) => format!("off ({key}: false, your own setting)"),
        _ => "on (Claude Code also writes memory files; they are imported at session start)"
            .to_string(),
    }
}

/// One finding of `doctor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Whether it is a problem (else it is fine or a note).
    pub problem: bool,
    /// What was checked and found.
    pub text: String,
    /// How to fix it.
    pub fix: Option<String>,
}

pub(crate) fn ok(text: impl Into<String>) -> Finding {
    Finding {
        problem: false,
        text: text.into(),
        fix: None,
    }
}

pub(crate) fn note(text: impl Into<String>, hint: impl Into<String>) -> Finding {
    Finding {
        problem: false,
        text: text.into(),
        fix: Some(hint.into()),
    }
}

pub(crate) fn problem(text: impl Into<String>, fix: impl Into<String>) -> Finding {
    Finding {
        problem: true,
        text: text.into(),
        fix: Some(fix.into()),
    }
}

/// Whether `program` exists: a path, or a name found on the `PATH`.
pub(crate) fn program_found(program: &str) -> bool {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path.is_file();
    }
    std::env::var_os("PATH").is_some_and(|dirs| {
        std::env::split_paths(&dirs)
            .any(|dir| dir.join(program).is_file() || dir.join(format!("{program}.exe")).is_file())
    })
}

/// `doctor`'s checks of the Claude Code installation and (`shared_doctor`) the data directory
/// (the tests' view; the command prints them as two sections).
#[cfg(test)]
pub fn doctor(setup: &Setup) -> Vec<Finding> {
    let mut findings = claude_doctor(setup);
    findings.extend(shared_doctor(setup));
    findings
}

/// `doctor`'s checks of the Claude Code installation only.
pub fn claude_doctor(setup: &Setup) -> Vec<Finding> {
    let inspection = inspect(setup);
    let mut findings = Vec::new();
    for error in &inspection.errors {
        findings.push(problem(
            error.clone(),
            "fix the file by hand (Claude Code cannot read it either), then run install again",
        ));
    }
    let reinstall = "run `graphersal-memory install`";
    match &inspection.use_server {
        PartState::Installed => findings.push(ok(format!("MCP server {USE_SERVER}: registered"))),
        PartState::Missing => findings.push(problem(
            format!("MCP server {USE_SERVER} is not registered in Claude Code"),
            reinstall,
        )),
        PartState::Outdated(why) => findings.push(problem(
            format!("MCP server {USE_SERVER} is registered for another binary or version ({why})"),
            format!("{reinstall} with this binary"),
        )),
        PartState::Conflict(why) => findings.push(problem(
            format!("MCP server {USE_SERVER}: {why}"),
            format!(
                "remove that entry (`claude mcp remove -s user {USE_SERVER}`), then {reinstall}"
            ),
        )),
    }
    match &inspection.develop_server {
        PartState::Missing => findings.push(ok(format!(
            "MCP server {DEVELOP_SERVER}: off (not registered)"
        ))),
        PartState::Installed => findings.push(ok(format!("MCP server {DEVELOP_SERVER}: on"))),
        other => findings.push(problem(
            format!("MCP server {DEVELOP_SERVER}: {}", other.describe()),
            "run `graphersal-memory install --enable-develop`, or `graphersal-memory uninstall \
             --disable-develop`",
        )),
    }
    for (event, state) in &inspection.hooks {
        match state {
            PartState::Installed => findings.push(ok(format!("hook {event}: installed"))),
            other => findings.push(problem(
                format!("hook {event}: {}", other.describe()),
                reinstall,
            )),
        }
    }
    match &inspection.block {
        PartState::Installed => findings.push(ok("rule block: written")),
        PartState::Missing => findings.push(problem(
            format!(
                "rule block missing in {}",
                setup.paths.instructions.display()
            ),
            reinstall,
        )),
        PartState::Outdated(why) => findings.push(problem(format!("rule block {why}"), reinstall)),
        PartState::Conflict(why) => findings.push(problem(
            format!("rule block {why} in {}", setup.paths.instructions.display()),
            "move your text out of the block or delete the block, then run install again",
        )),
    }
    for binary in &inspection.missing_binaries {
        findings.push(problem(
            format!("the registered binary {binary} does not exist"),
            format!("{reinstall} from the binary you use now"),
        ));
    }
    for other in &inspection.other_scopes {
        findings.push(problem(
            format!("another registration: {other}; Claude Code then starts the memory twice"),
            "remove it with `claude mcp remove -s local graphersal-memory` in that project",
        ));
    }
    match ImportState::load(&setup.data_dir).and_then(|state| {
        Ok(import::totals(&import::plan(
            import::scan(&setup.paths.projects)?,
            &state,
        )))
    }) {
        Ok((_, _, new, changed, _)) if new + changed > 0 => findings.push(problem(
            format!(
                "{} memory files are not imported yet ({new} new, {changed} changed)",
                new + changed
            ),
            "run `graphersal-memory import` (the session start hook imports the current \
             project's files too)",
        )),
        Ok(_) => findings.push(ok("file memory: everything imported")),
        Err(err) => findings.push(problem(err, "check the file permissions")),
    }
    if setup.hooks.windows {
        let settings = read_json(&setup.paths.settings, &mut Vec::new());
        findings.extend(git_bash_findings(
            &crate::winshell::find_git_bash_here(&settings),
            &settings,
        ));
    }
    findings
}

/// Whether one of our Claude Code hooks in `settings` is a shell command line with a quoted
/// program (what install wrote before the exec form): only Git Bash runs it.
fn hooks_need_git_bash(settings: &Value) -> bool {
    HOOK_EVENTS.iter().any(|(event, _)| {
        settings["hooks"][event].as_array().is_some_and(|groups| {
            groups.iter().filter(|g| is_our_hook_group(g)).any(|group| {
                group["hooks"]
                    .as_array()
                    .is_some_and(|hooks| hooks.iter().any(crate::winshell::needs_git_bash))
            })
        })
    })
}

/// What works without Git Bash, for the doctor's finding and the docs' wording.
const WITHOUT_GIT_BASH: &str = "What works: the memory tools, and the MCP server itself \
    imports this project's memory files and queues the session indexing at every session start \
    and end. What does not: the hook's session start line, and indexing a session right when it \
    ends (it follows once it has been unchanged for 30 minutes)";

/// `doctor`'s Git Bash finding on Windows: `git_bash` as Claude Code finds it, against our
/// hooks in Claude Code's `settings`.
pub fn git_bash_findings(git_bash: &crate::winshell::GitBash, settings: &Value) -> Vec<Finding> {
    let mut findings = Vec::new();
    if let Some(ignored) = &git_bash.ignored {
        findings.push(note(
            format!("Git Bash: {ignored}"),
            "Claude Code ignores such a value too and looks in the default places",
        ));
    }
    match &git_bash.path {
        Some(path) => findings.push(ok(format!(
            "Git Bash: {path} (Claude Code runs shell hooks there)"
        ))),
        None if hooks_need_git_bash(settings) => findings.push(problem(
            format!(
                "Git Bash not found, and the graphersal-memory hooks in Claude Code's settings \
                 are written for it: without Git Bash Claude Code runs hooks in PowerShell, \
                 which does not start a quoted program. {WITHOUT_GIT_BASH}"
            ),
            "run `graphersal-memory install` again (it writes hooks that Claude Code starts \
             without a shell), or install Git for Windows (https://git-scm.com/downloads/win), \
             or set CLAUDE_CODE_GIT_BASH_PATH to its bash.exe in the `env` of Claude Code's \
             settings; then start a new session",
        )),
        None => findings.push(ok(
            "Git Bash: not found; Claude Code runs shell hooks in PowerShell, and the \
             graphersal-memory hooks need no shell",
        )),
    }
    findings
}

/// `status`'s Git Bash line on Windows.
pub fn git_bash_status(git_bash: &crate::winshell::GitBash, settings: &Value) -> String {
    match &git_bash.path {
        Some(path) => format!("  Git Bash: {path}"),
        None if hooks_need_git_bash(settings) => "  Git Bash: not found, and the installed \
            hooks need it (run `graphersal-memory install` again; `graphersal-memory doctor` \
            says more)"
            .to_string(),
        None => "  Git Bash: not found (Claude Code runs hooks in PowerShell; the \
            graphersal-memory hooks run without a shell)"
            .to_string(),
    }
}

/// `doctor`'s checks every harness shares: the settings file, the summaries' harness, the
/// embedding model, moved projects, the registry, the daemon.
pub fn shared_doctor(setup: &Setup) -> Vec<Finding> {
    let mut findings = Vec::new();
    match Config::load(&setup.data_dir) {
        Ok(config) => {
            if config.unknown.is_empty() {
                findings.push(ok(format!(
                    "settings: {}",
                    if crate::config::path(&setup.data_dir).exists() {
                        "config.json is valid"
                    } else {
                        "defaults (no config.json)"
                    }
                )));
            } else {
                findings.push(problem(
                    format!(
                        "config.json has keys this version does not know: {}",
                        config.unknown.join(", ")
                    ),
                    "check their spelling (known: daemon.idle, memory.language, \
                     memory.start_context_chars, embedding.model, \
                     summaries.enabled, summaries.harness_command, summaries.monthly_budget_usd, \
                     summaries.since, store.checkpoint_after, store.keep_history)",
                ));
            }
        }
        Err(err) => findings.push(problem(err, "fix or delete config.json")),
    }
    if let Ok(config) = Config::load(&setup.data_dir) {
        let program = config
            .summaries
            .harness_command
            .first()
            .cloned()
            .unwrap_or_default();
        if !config.summaries.enabled {
            findings.push(ok("summaries: off (raw turns are indexed)"));
        } else if program_found(&program) {
            findings.push(ok(format!("summaries: on, through {program}")));
        } else {
            findings.push(problem(
                format!("summaries are on, but the harness {program} is not found"),
                "install it, set summaries.harness_command in config.json, or run \
                 `graphersal-memory summaries disable`",
            ));
        }
    }
    if let Ok(config) = Config::load(&setup.data_dir) {
        match config.model.as_deref() {
            None | Some(models::BUILTIN) => findings.push(ok(format!(
                "embedding model: the built-in {} (no download)",
                models::BUILTIN
            ))),
            Some(model) => match models::check(&setup.data_dir, model) {
                Ok(identity) => findings.push(ok(format!(
                    "embedding model: {model} is present and loads ({identity})"
                ))),
                Err(err) => findings.push(problem(
                    format!("embedding model: {err}"),
                    format!(
                        "run `graphersal-memory install --model {model}` (or `--model builtin`)"
                    ),
                )),
            },
        }
    }
    findings.extend(crate::commands::store_findings(&setup.run_dir));
    for (what, how) in crate::projects::look_alike_lines(&setup.data_dir) {
        findings.push(note(what, how));
    }
    if let Err(err) = crate::registry::Registry::parse(
        &std::fs::read(setup.data_dir.join(crate::registry::REGISTRY_FILE)).unwrap_or_else(|_| {
            br#"{"version": 1, "default": "default", "projects": {}}"#.to_vec()
        }),
    ) {
        findings.push(problem(
            format!("registry.json is invalid: {err}"),
            "fix it or delete it (it is recreated with the defaults)",
        ));
    }
    match crate::ipc::socket_path(&setup.run_dir) {
        Ok(_) => {}
        Err(err) => findings.push(problem(err, "choose a shorter run directory")),
    }
    match crate::shim::control(&setup.run_dir, "status") {
        Ok(Some(_)) => findings.push(ok("daemon: running")),
        Ok(None) => {
            let socket = setup.run_dir.join(crate::ipc::SOCKET_FILE);
            if socket.exists() && !crate::ipc::daemon_alive(&setup.run_dir).unwrap_or(true) {
                findings.push(ok(
                    "daemon: not running (a stale socket file is left; the next session removes \
                     it)",
                ));
            } else {
                findings.push(ok("daemon: not running (the next agent session starts it)"));
            }
        }
        Err(err) => findings.push(problem(
            format!("the daemon does not answer properly: {err}"),
            format!(
                "stop it (`kill` the process in {}) and start a new session; its log is {}",
                crate::ipc::PID_FILE,
                setup.run_dir.join(crate::ipc::LOG_FILE).display()
            ),
        )),
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_paths_follow_claude_config_dir_then_home() {
        let paths = ClaudePaths::resolve(&|name| match name {
            "HOME" => Some("/home/u".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(paths.user_config, PathBuf::from("/home/u/.claude.json"));
        assert_eq!(
            paths.settings,
            PathBuf::from("/home/u/.claude/settings.json")
        );
        assert_eq!(
            paths.instructions,
            PathBuf::from("/home/u/.claude/CLAUDE.md")
        );
        let custom = ClaudePaths::resolve(&|name| match name {
            "CLAUDE_CONFIG_DIR" => Some("/c".into()),
            "HOME" => Some("/home/u".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(custom.user_config, PathBuf::from("/c/.claude.json"));
        assert_eq!(custom.projects, PathBuf::from("/c/projects"));
        assert!(ClaudePaths::resolve(&|_| None).is_err());
    }

    #[test]
    fn the_block_is_found_and_a_hand_edit_is_detected() {
        let block = block_text();
        let text = format!("# Mine\n\n{block}after\n");
        let found = find_block(&text).unwrap().unwrap();
        assert!(found.intact && found.current);
        assert_eq!(&text[found.start..found.end], block);
        let edited = text.replace("Save with", "Store with");
        let found = find_block(&edited).unwrap().unwrap();
        assert!(!found.intact && !found.current);
        assert!(find_block("no block").unwrap().is_none());
        assert!(find_block(&format!("{BLOCK_BEGIN} x -->\nno end\n")).is_err());
        // A marker inside a line is not a block.
        assert!(
            find_block(&format!("see {BLOCK_BEGIN} here\n"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn entries_and_hooks_are_recognised_as_ours() {
        assert!(is_our_entry(
            &json!({"command": "/x/graphersal-memory", "args": ["mcp"]})
        ));
        assert!(is_our_entry(
            &json!({"command": "C:\\x\\graphersal-memory.exe", "args": ["mcp", "--profile", "use"]})
        ));
        assert!(!is_our_entry(
            &json!({"command": "/x/other", "args": ["mcp"]})
        ));
        assert!(!is_our_entry(
            &json!({"command": "/x/graphersal-memory", "args": ["daemon"]})
        ));
        let group = json!({"hooks": [{"type": "command", "command": "\"/x/graphersal-memory\" hook session-start"}]});
        assert!(is_our_hook_group(&group));
        let mixed = json!({"hooks": [{"command": "\"/x/graphersal-memory\" hook session-start"}, {"command": "echo hi"}]});
        assert!(!is_our_hook_group(&mixed));
        assert!(!is_our_hook_group(&json!({"hooks": []})));
    }

    #[test]
    fn separators_before_the_block() {
        assert_eq!(block_separator(""), "");
        assert_eq!(block_separator("a\n\n"), "");
        assert_eq!(block_separator("a\n"), "\n");
        assert_eq!(block_separator("a"), "\n\n");
    }
}
