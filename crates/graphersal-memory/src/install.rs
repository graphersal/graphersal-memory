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
//!   `"<this binary>" hook session-start|session-end`. Our entries are recognised by that
//!   command.
//! - **The managed rule block** in the user instruction file (`~/.claude/CLAUDE.md`), between
//!   `<!-- graphersal-memory:begin ... -->` and `<!-- graphersal-memory:end -->`; the begin line
//!   carries the fingerprint of the block's text, so a hand edit inside is detected and reported
//!   as a conflict, never overwritten.
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

use crate::config::Config;
use crate::import::{self, Connect, DirPlan, ImportState};
use crate::{fsutil, jsonedit};

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

/// The state file of the Claude Code installation.
const STATE_FILE: &str = "claude-code.json";

/// The format version of the state file.
const FORMAT: u64 = 1;

/// The text of the managed rule block (between its marker lines).
pub const RULE_BLOCK: &str = "\
## Memory: graphersal-memory

Your long-term memory of this user lives in a local graph, served by the MCP server
`graphersal-memory`, not in memory files.

- Search it before you answer from memory or ask the user something they may have told you
  before: `memories(text)` for rules, decisions and preferences, `recall(text)` for everything,
  `memory(slug)` to read one in full.
- Save with `remember`: search with `memories(text)` first, so you replace a memory instead of
  duplicating it; scope \"global\" for rules and preferences that hold in every project. Remove
  with `forget`.
- Do not write memory files (`MEMORY.md` and the files beside it) any more. Files written anyway
  are imported into the graph at the next session start.
- Past sessions (`sessions(text)`, `session(id)`) are not indexed yet in this version.
";

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
    /// when set, else `~/.claude` (home: `HOME`, else `USERPROFILE`).
    pub fn resolve(env: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let set = |name: &str| env(name).filter(|value| !value.is_empty());
        if let Some(dir) = set("CLAUDE_CONFIG_DIR") {
            let dir = PathBuf::from(dir);
            return Ok(Self::with(dir.join(".claude.json"), dir));
        }
        let home = set("HOME")
            .or_else(|| set("USERPROFILE"))
            .ok_or("cannot find the home directory (HOME is not set)")?;
        let home = PathBuf::from(home);
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
    /// The data directory.
    pub data_dir: PathBuf,
    /// The run directory.
    pub run_dir: PathBuf,
    /// This binary (what the MCP entries and hooks start).
    pub exe: PathBuf,
    /// Arguments that point a started process at an explicitly chosen data or run directory
    /// (`--data-dir X`, `--run-dir Y`); empty for the defaults.
    pub dir_args: Vec<String>,
}

impl Setup {
    /// The MCP entry of a profile (`use` or `develop`).
    pub fn mcp_entry(&self, profile: &str) -> Value {
        let mut args = vec![
            "mcp".to_string(),
            "--profile".to_string(),
            profile.to_string(),
        ];
        args.extend(self.dir_args.iter().cloned());
        json!({"type": "stdio", "command": self.exe.display().to_string(), "args": args,
               "env": {}})
    }

    /// The hook command of an event (`session-start`, `session-end`).
    pub fn hook_command(&self, event: &str) -> String {
        let mut command = format!("{} hook {event}", quote(&self.exe.display().to_string()));
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

    /// The hook group of an event, as it goes into the settings.
    pub fn hook_group(&self, event: &str) -> Value {
        json!({"hooks": [{"type": "command", "command": self.hook_command(event),
                          "timeout": HOOK_TIMEOUT_SECS}]})
    }

    fn state_dir(&self) -> PathBuf {
        self.data_dir.join(STATE_DIR)
    }
}

/// `text` in double quotes for a shell command line.
fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\\\""))
}

/// Whether an MCP entry is one of ours: it starts a `graphersal-memory` binary with `mcp`.
pub fn is_our_entry(entry: &Value) -> bool {
    let command = entry["command"].as_str().unwrap_or_default();
    // The file name on any platform (an entry may come from another system's path style).
    let name = command.rsplit(['/', '\\']).next().unwrap_or_default();
    let name = name.strip_suffix(".exe").unwrap_or(name);
    name == "graphersal-memory" && entry["args"][0] == "mcp"
}

/// Whether a hook command is one of ours.
fn is_our_command(command: &str) -> bool {
    command.contains("graphersal-memory") && command.contains(" hook session-")
}

/// Whether a hook group (an element of `hooks.<Event>`) consists of our commands only.
pub fn is_our_hook_group(group: &Value) -> bool {
    group["hooks"].as_array().is_some_and(|hooks| {
        !hooks.is_empty()
            && hooks
                .iter()
                .all(|hook| is_our_command(hook["command"].as_str().unwrap_or_default()))
    })
}

/// The managed block for `RULE_BLOCK`, marker lines included, ending with a newline.
pub fn block_text() -> String {
    format!(
        "{BLOCK_BEGIN} fingerprint={} (managed by `graphersal-memory install`; \
         `graphersal-memory uninstall` removes it; an edit inside is reported as a conflict) \
         -->\n{RULE_BLOCK}{BLOCK_END}\n",
        fsutil::fingerprint(RULE_BLOCK.as_bytes())
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
}

/// The managed block of `text`: `Ok(None)` without one, `Err` for a begin line without an end
/// line.
pub fn find_block(text: &str) -> Result<Option<FoundBlock>, String> {
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
    let content = &text[line_end..end_at];
    let mut end = end_at + BLOCK_END.len();
    if text[end..].starts_with("\r\n") {
        end += 2;
    } else if text[end..].starts_with('\n') {
        end += 1;
    }
    let intact = fsutil::fingerprint(content.as_bytes()) == recorded;
    Ok(Some(FoundBlock {
        start,
        end,
        intact,
        current: intact && text[start..end] == block_text(),
    }))
}

/// The separator put before a block appended to `text`.
fn block_separator(text: &str) -> &'static str {
    if text.is_empty() || text.ends_with("\n\n") {
        ""
    } else if text.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    }
}

/// Options of `install`.
#[derive(Debug, Clone, Copy, Default)]
pub struct InstallOptions {
    /// Show the plan only.
    pub dry_run: bool,
    /// Do not ask.
    pub yes: bool,
    /// Ask for the model download step (planned).
    pub with_model: bool,
    /// Register the develop server too.
    pub enable_develop: bool,
    /// Switch session summaries on without asking.
    pub summaries: bool,
}

/// Options of `uninstall`.
#[derive(Debug, Clone, Copy, Default)]
pub struct UninstallOptions {
    /// Show the plan only.
    pub dry_run: bool,
    /// Do not ask.
    pub yes: bool,
    /// Delete the data directory too.
    pub purge: bool,
    /// Only unregister the develop server.
    pub disable_develop_only: bool,
}

/// One planned file change.
#[derive(Debug, Clone)]
struct Change {
    path: PathBuf,
    /// The new text; `None` deletes the file.
    after: Option<String>,
    /// Containers this change creates (dotted paths), for a later structural removal.
    created: Vec<String>,
    /// The separator written before the managed block.
    separator: Option<String>,
}

/// The state file: what install changed (`<data dir>/install/claude-code.json`):
/// `{"version": 1, "installed_at", "updated_at", "exe", "created_dirs": [...], "files":
/// {"<path>": {"existed", "backup", "backup_valid", "written", "created", "separator"}}}`.
#[derive(Debug, Clone, Default)]
struct State {
    meta: serde_json::Map<String, Value>,
    files: std::collections::BTreeMap<String, serde_json::Map<String, Value>>,
    created_dirs: Vec<String>,
}

impl State {
    fn path(setup: &Setup) -> PathBuf {
        setup.state_dir().join(STATE_FILE)
    }

    fn load(setup: &Setup) -> Result<Self, String> {
        let path = Self::path(setup);
        let Some(bytes) = fsutil::read_optional(&path)? else {
            return Ok(Self::default());
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
            meta,
            files,
            created_dirs,
        })
    }

    fn exists(setup: &Setup) -> bool {
        Self::path(setup).exists()
    }

    fn save(&mut self, setup: &Setup) -> Result<(), String> {
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
        fsutil::write_atomic(&Self::path(setup), format!("{text}\n").as_bytes())
    }

    fn file(&self, path: &Path) -> Option<&serde_json::Map<String, Value>> {
        self.files.get(&path.display().to_string())
    }

    fn file_mut(&mut self, path: &Path) -> &mut serde_json::Map<String, Value> {
        self.files.entry(path.display().to_string()).or_default()
    }

    fn created_dirs(&self) -> Vec<PathBuf> {
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
struct Step {
    title: String,
    lines: Vec<String>,
}

/// The planned install.
struct InstallPlan {
    steps: Vec<Step>,
    changes: Vec<Change>,
    conflicts: Vec<String>,
    imports: Vec<DirPlan>,
    write_config: bool,
}

fn read_text(path: &Path) -> Result<Option<String>, String> {
    match fsutil::read_optional(path)? {
        Some(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| format!("{} is not UTF-8 text", path.display())),
        None => Ok(None),
    }
}

/// An empty JSON object as the start of a file install creates.
const EMPTY_OBJECT: &str = "{\n}\n";

/// Plans the change of the user configuration (the MCP servers).
fn plan_user_config(
    setup: &Setup,
    enable_develop: bool,
    conflicts: &mut Vec<String>,
) -> Result<(Step, Option<Change>), String> {
    let path = &setup.paths.user_config;
    let before = read_text(path)?;
    let mut text = before.clone().unwrap_or_else(|| EMPTY_OBJECT.to_string());
    let parsed: Value = serde_json::from_str(&text)
        .map_err(|err| format!("{} is not valid JSON: {err}", path.display()))?;
    let mut created = Vec::new();
    let mut lines = Vec::new();
    let mut servers = vec![(USE_SERVER, "use")];
    if enable_develop {
        servers.push((DEVELOP_SERVER, "develop"));
    }
    for (name, profile) in servers {
        let existing = &parsed["mcpServers"][name];
        if !existing.is_null() && !is_our_entry(existing) {
            conflicts.push(format!(
                "{}: mcpServers.{name} exists and is not ours; kept. Remove it (`claude mcp \
                 remove -s user {name}`) and run install again.",
                path.display()
            ));
            lines.push(format!(
                "{name}: CONFLICT (an entry of another program has this name)"
            ));
            continue;
        }
        let entry = setup.mcp_entry(profile);
        match jsonedit::set_member(&text, &["mcpServers"], name, &entry)? {
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
        let develop = &parsed["mcpServers"][DEVELOP_SERVER];
        lines.push(if develop.is_null() {
            format!(
                "{DEVELOP_SERVER}: not registered (off; `graphersal-memory install \
                 --enable-develop` registers it)"
            )
        } else {
            format!("{DEVELOP_SERVER}: registered (kept)")
        });
    }
    let step = Step {
        title: format!(
            "Register the MCP servers at user scope in {} (key mcpServers)",
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
    Ok((step, change))
}

/// Plans the change of the user settings (the hooks).
fn plan_settings(setup: &Setup) -> Result<(Step, Option<Change>), String> {
    let path = &setup.paths.settings;
    let before = read_text(path)?;
    let mut text = before.clone().unwrap_or_else(|| EMPTY_OBJECT.to_string());
    let parsed: Value = serde_json::from_str(&text)
        .map_err(|err| format!("{} is not valid JSON: {err}", path.display()))?;
    let mut created = Vec::new();
    let mut lines = Vec::new();
    for (event, sub) in HOOK_EVENTS {
        let group = setup.hook_group(sub);
        let current = parsed["hooks"][event].as_array();
        let ours: Vec<&Value> = current
            .map(|groups| groups.iter().filter(|g| is_our_hook_group(g)).collect())
            .unwrap_or_default();
        if ours.len() == 1 && *ours[0] == group {
            lines.push(format!("{event}: already installed"));
            continue;
        }
        if !ours.is_empty()
            && let Some(new) =
                jsonedit::remove_elements(&text, &["hooks", event], &is_our_hook_group)?
        {
            text = new;
        }
        let (new, made) = jsonedit::append_element(&text, &["hooks", event], &group)?;
        text = new;
        created.extend(made);
        lines.push(format!(
            "{event}: {} `{}`",
            if ours.is_empty() { "add" } else { "update to" },
            setup.hook_command(sub)
        ));
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
    Ok((step, change))
}

/// Plans the managed block in the instruction file.
fn plan_block(
    setup: &Setup,
    conflicts: &mut Vec<String>,
) -> Result<(Step, Option<Change>), String> {
    let path = &setup.paths.instructions;
    let before = read_text(path)?;
    let text = before.clone().unwrap_or_default();
    let title = format!("Write the managed rule block into {}", path.display());
    let mut separator = None;
    let after = match find_block(&text) {
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
        Ok(Some(found)) => Some(format!(
            "{}{}{}",
            &text[..found.start],
            block_text(),
            &text[found.end..]
        )),
        Ok(None) => {
            let sep = block_separator(&text);
            separator = Some(sep.to_string());
            Some(format!("{text}{sep}{}", block_text()))
        }
    };
    let line = match (&after, find_block(&text)) {
        (_, Err(_)) | (None, Ok(Some(FoundBlock { intact: false, .. }))) => {
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

/// Plans the whole install.
fn plan_install(setup: &Setup, options: &InstallOptions) -> Result<InstallPlan, String> {
    let mut conflicts = Vec::new();
    let mut steps = Vec::new();
    let mut changes = Vec::new();
    let (step, change) = plan_user_config(setup, options.enable_develop, &mut conflicts)?;
    steps.push(step);
    changes.extend(change);
    let (step, change) = plan_settings(setup)?;
    steps.push(step);
    changes.extend(change);

    let state = ImportState::load(&setup.data_dir)?;
    let imports = import::plan(import::scan(&setup.paths.projects)?, &state);
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
    lines.push(
        "The files are read, not changed. CLAUDE.md rules are not imported in this version."
            .to_string(),
    );
    steps.push(Step {
        title: "Import the existing file memory (MEMORY.md and the memory files)".to_string(),
        lines,
    });

    let (step, change) = plan_block(setup, &mut conflicts)?;
    steps.push(step);
    changes.extend(change);

    let request = crate::sessions::Request::new(&setup.paths.projects);
    let (found, active, todo, _) = crate::sessions::plan(&request, &setup.data_dir)?;
    let settings = Config::load(&setup.data_dir)?;
    let estimate = crate::sessions::estimate(
        &setup.paths.projects,
        &setup.data_dir,
        crate::sessions::IDLE,
        settings.summaries.since.as_deref(),
    )?;
    let summaries_line = if settings.summaries.enabled {
        "summaries: on (already switched on in the settings file)".to_string()
    } else if options.summaries {
        format!(
            "summaries: switched on (--summaries), through `{}`, at most ${:.2} per month; {}",
            settings.summaries.harness_command.join(" "),
            settings.summaries.monthly_budget_usd,
            crate::commands::estimate_line(&estimate)
        )
    } else {
        format!(
            "summaries: off unless you agree below (`--summaries` switches them on; `--yes` \
             alone leaves them off); {}",
            crate::commands::estimate_line(&estimate)
        )
    };
    steps.push(Step {
        title: "Index past sessions (raw turns, locally, in the background)".to_string(),
        lines: vec![
            format!(
                "{found} session transcript{} under {}: {todo} to index, {active} still being \
                 written (indexed later)",
                if found == 1 { "" } else { "s" },
                setup.paths.projects.display()
            ),
            summaries_line,
        ],
    });

    steps.push(Step {
        title: "Embedding model".to_string(),
        lines: vec![if options.with_model {
            "--with-model: the download of potion-multilingual-128M is planned and not available \
             yet; the built-in embedder hash-ngram/1 is used (no download)"
                .to_string()
        } else {
            "the built-in embedder hash-ngram/1 (no download); a better model is planned"
                .to_string()
        }],
    });

    let config_path = crate::config::path(&setup.data_dir);
    let write_config = !config_path.exists();
    steps.push(Step {
        title: format!("Settings file {}", config_path.display()),
        lines: vec![if write_config {
            "write the defaults (daemon idle time 2h)".to_string()
        } else {
            "exists (kept)".to_string()
        }],
    });
    Ok(InstallPlan {
        steps,
        changes,
        conflicts,
        imports,
        write_config,
    })
}

fn print_steps(out: &mut dyn Write, steps: &[Step]) {
    for (n, step) in steps.iter().enumerate() {
        let _ = writeln!(out, "  {}. {}", n + 1, step.title);
        for line in &step.lines {
            let _ = writeln!(out, "     - {line}");
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
/// records the change in the state.
fn apply_changes(
    setup: &Setup,
    changes: &[Change],
    state: &mut State,
    report: &mut Vec<String>,
) -> Result<(), String> {
    let stamp = crate::daemon::now_utc().replace(':', "");
    for change in changes {
        let path = &change.path;
        let current = fsutil::read_optional(path)?;
        let current_fp = current.as_deref().map(fsutil::fingerprint);
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
            }
        }
        let mut backup_note = String::new();
        if !entry_existed {
            let entry = state.file_mut(path);
            entry.insert("existed".into(), json!(current.is_some()));
            entry.insert("backup_valid".into(), json!(true));
            if let Some(bytes) = &current {
                let backup = backup_file(setup, &stamp, path, bytes)?;
                backup_note = format!(" (backup: {})", backup.display());
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
                    backup_note = format!(" (backup: {})", backup.display());
                }
            }
        }
        match &change.after {
            Some(text) => {
                fsutil::write_atomic(path, text.as_bytes())?;
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
                report.push(format!("removed {}{backup_note}", path.display()));
            }
        }
        state.save(setup)?;
    }
    Ok(())
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

fn remove_file(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("cannot remove {}: {err}", path.display())),
    }
}

/// `install`: shows the plan, asks (unless `--yes`), applies it and prints the report. Returns
/// the exit code (1 when a conflict or an import failure left something undone).
pub fn install(
    setup: &Setup,
    options: &InstallOptions,
    connect: &mut Connect<'_>,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<u8, String> {
    let plan = plan_install(setup, options)?;
    let _ = writeln!(out, "graphersal-memory install (Claude Code)\n");
    let _ = writeln!(
        out,
        "{}",
        if options.dry_run {
            "Dry run: this is what install would do (nothing is changed):"
        } else {
            "This will:"
        }
    );
    print_steps(out, &plan.steps);
    for conflict in &plan.conflicts {
        let _ = writeln!(out, "  conflict: {conflict}");
    }
    if options.dry_run {
        return Ok(0);
    }
    if !options.yes && !confirm("\nProceed?", input, out) {
        let _ = writeln!(
            out,
            "Aborted: nothing was changed (pass --yes to run without asking)."
        );
        return Ok(1);
    }
    let mut state = State::load(setup)?;
    if !state.meta.contains_key("installed_at") {
        state
            .meta
            .insert("installed_at".into(), json!(crate::daemon::now_utc()));
    }
    state
        .meta
        .insert("updated_at".into(), json!(crate::daemon::now_utc()));
    state
        .meta
        .insert("exe".into(), json!(setup.exe.display().to_string()));
    state.save(setup)?;
    let mut report = Vec::new();
    apply_changes(setup, &plan.changes, &mut state, &mut report)?;

    // The import goes through the memory functions (the daemon serves the store).
    let mut failures = 0;
    let (_, _, new, changed, _) = import::totals(&plan.imports);
    if new + changed > 0 {
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

    if plan.write_config {
        let path = crate::config::path(&setup.data_dir);
        std::fs::create_dir_all(&setup.data_dir)
            .map_err(|err| format!("cannot create {}: {err}", setup.data_dir.display()))?;
        fsutil::write_atomic(&path, Config::default_text().as_bytes())?;
        report.push(format!("created {}", path.display()));
    }

    // Summaries: only with consent (--summaries, or yes to the question; --yes alone: off).
    let settings = Config::load(&setup.data_dir)?;
    if !settings.summaries.enabled {
        let agreed = options.summaries
            || (!options.yes
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
    }

    // Past sessions: indexed by the daemon in the background, when it runs (the import
    // started it); else at the next session start.
    let request = crate::sessions::Request::new(&setup.paths.projects);
    match crate::shim::request_running(&setup.run_dir, &request.to_control()) {
        Ok(Some(answer)) if answer["ok"] == true => report.push(
            "past sessions: indexing in the background (`graphersal-memory import --sessions` \
             shows the progress)"
                .to_string(),
        ),
        _ => report.push(
            "past sessions: indexed at the next session start (or now: `graphersal-memory \
             import --sessions`)"
                .to_string(),
        ),
    }
    if options.with_model {
        report.push(
            "embedding model: the download is planned, not available yet (the built-in \
             hash-ngram/1 embedder is used)"
                .to_string(),
        );
    }
    let _ = writeln!(out, "\nReport:");
    for line in &report {
        let _ = writeln!(out, "  {line}");
    }
    for conflict in &plan.conflicts {
        let _ = writeln!(out, "  conflict: {conflict}");
    }
    if plan.changes.is_empty() && plan.conflicts.is_empty() {
        let _ = writeln!(out, "  Claude Code configuration: already up to date");
    }
    let _ = writeln!(
        out,
        "\nRestart running Claude Code sessions to load the MCP server and the hooks. \
         `graphersal-memory status` shows what is installed; `graphersal-memory uninstall` \
         removes it."
    );
    Ok(if plan.conflicts.is_empty() && failures == 0 {
        0
    } else {
        1
    })
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
    if report.timed_out {
        line.push_str("; stopped early (time limit), the rest follows next time");
    }
    line
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
            if is_our_entry(&parsed["mcpServers"][*name])
                && let Some(new) = jsonedit::remove_member(&current, &["mcpServers"], name)?
            {
                current = new;
                changed = true;
            }
        }
        if changed
            && created.iter().any(|c| c == "mcpServers")
            && jsonedit::is_empty_container(&current, &["mcpServers"])
            && let Some(new) = jsonedit::remove_member(&current, &[], "mcpServers")?
        {
            current = new;
        }
    } else if path == setup.paths.settings {
        for (event, _) in HOOK_EVENTS {
            if let Some(new) =
                jsonedit::remove_elements(&current, &["hooks", event], &is_our_hook_group)?
            {
                current = new;
                changed = true;
                let dotted = format!("hooks.{event}");
                if created.contains(&dotted)
                    && jsonedit::is_empty_container(&current, &["hooks", event])
                    && let Some(new) = jsonedit::remove_member(&current, &["hooks"], event)?
                {
                    current = new;
                }
            }
        }
        if changed
            && created.iter().any(|c| c == "hooks")
            && jsonedit::is_empty_container(&current, &["hooks"])
            && let Some(new) = jsonedit::remove_member(&current, &[], "hooks")?
        {
            current = new;
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

/// Whether a file install created holds nothing but what it would start from.
fn is_empty_remainder(path: &Path, text: &str) -> bool {
    if path.extension().is_some_and(|e| e == "json") {
        serde_json::from_str::<Value>(text)
            .ok()
            .is_some_and(|v| v.as_object().is_some_and(|o| o.is_empty()))
    } else {
        text.trim().is_empty()
    }
}

/// `uninstall`: removes exactly what install added (or only the develop server). Returns the
/// exit code.
pub fn uninstall(
    setup: &Setup,
    options: &UninstallOptions,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<u8, String> {
    let state = State::load(setup)?;
    let mut conflicts = Vec::new();
    let mut changes: Vec<(Change, &'static str)> = Vec::new();
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
        let entry = state.file(path);
        let fp = fsutil::fingerprint(&bytes);
        let untouched = entry.is_some_and(|e| {
            e.get("written").and_then(Value::as_str) == Some(fp.as_str())
                && e.get("backup_valid") == Some(&Value::Bool(true))
        });
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
            &mut conflicts,
        )? {
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
    let title = if options.disable_develop_only {
        "graphersal-memory uninstall --disable-develop (Claude Code)"
    } else {
        "graphersal-memory uninstall (Claude Code)"
    };
    let _ = writeln!(out, "{title}\n");
    if changes.is_empty() && conflicts.is_empty() {
        let _ = writeln!(
            out,
            "Nothing of graphersal-memory is installed in Claude Code."
        );
    }
    for (change, what) in &changes {
        let _ = writeln!(out, "  - {}: {what}", change.path.display());
    }
    for conflict in &conflicts {
        let _ = writeln!(out, "  conflict: {conflict}");
    }
    if options.purge {
        let _ = writeln!(
            out,
            "  - delete the data directory {} with every memory in it (the daemon is stopped \
             first)",
            setup.data_dir.display()
        );
    }
    if options.dry_run {
        let _ = writeln!(out, "\nDry run: nothing was changed.");
        return Ok(0);
    }
    let needs_ok = !changes.is_empty() || options.purge;
    if needs_ok && !options.yes {
        let question = if options.purge {
            "\nThis deletes your memories for good. Proceed?"
        } else {
            "\nProceed?"
        };
        if !confirm(question, input, out) {
            let _ = writeln!(out, "Aborted: nothing was changed.");
            return Ok(1);
        }
    }
    let mut report = Vec::new();
    let mut state = state;
    for (change, _) in &changes {
        match &change.after {
            Some(text) => {
                fsutil::write_atomic(&change.path, text.as_bytes())?;
                report.push(format!("changed {}", change.path.display()));
                if options.disable_develop_only && State::exists(setup) {
                    state.file_mut(&change.path).insert(
                        "written".into(),
                        json!(fsutil::fingerprint(text.as_bytes())),
                    );
                }
            }
            None => {
                remove_file(&change.path)?;
                report.push(format!("removed {}", change.path.display()));
            }
        }
    }
    if options.disable_develop_only {
        if State::exists(setup) {
            state.save(setup)?;
        }
    } else {
        let mut dirs = state.created_dirs();
        dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
        for dir in dirs {
            // Only when empty: a directory someone else filled meanwhile stays.
            if std::fs::remove_dir(&dir).is_ok() {
                report.push(format!("removed the directory {}", dir.display()));
            }
        }
        if conflicts.is_empty() {
            remove_file(&State::path(setup))?;
        }
    }
    if options.purge {
        report.extend(purge(setup)?);
    }
    let _ = writeln!(out, "\nReport:");
    if report.is_empty() {
        let _ = writeln!(out, "  nothing to change");
    }
    for line in &report {
        let _ = writeln!(out, "  {line}");
    }
    for conflict in &conflicts {
        let _ = writeln!(out, "  conflict: {conflict}");
    }
    if !options.disable_develop_only && !options.purge {
        let _ = writeln!(
            out,
            "  Your memories stay in {} (`uninstall --purge` deletes them).",
            setup.data_dir.display()
        );
    }
    Ok(if conflicts.is_empty() { 0 } else { 1 })
}

/// Deletes the data directory (after stopping its daemon) and the daemon's files.
fn purge(setup: &Setup) -> Result<Vec<String>, String> {
    let mut report = Vec::new();
    let data = &setup.data_dir;
    if data.exists() {
        let ours = [
            "stores",
            "registry.json",
            "config.json",
            "install",
            "import",
            "projects.json",
            "run",
        ]
        .iter()
        .any(|name| data.join(name).exists());
        let empty = std::fs::read_dir(data).is_ok_and(|mut entries| entries.next().is_none());
        if !ours && !empty {
            return Err(format!(
                "{} does not look like a graphersal-memory data directory (no stores, no \
                 registry); not deleted",
                data.display()
            ));
        }
    }
    if crate::shim::control(&setup.run_dir, "stop")?.is_some() {
        report.push("stopped the daemon".to_string());
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while crate::ipc::daemon_alive(&setup.run_dir).unwrap_or(false)
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    if data.exists() {
        std::fs::remove_dir_all(data)
            .map_err(|err| format!("cannot delete {}: {err}", data.display()))?;
        report.push(format!("deleted the data directory {}", data.display()));
    }
    if setup.run_dir.exists() && !setup.run_dir.starts_with(data) {
        for name in [
            crate::ipc::SOCKET_FILE,
            crate::ipc::PID_FILE,
            crate::ipc::START_LOCK_FILE,
            crate::ipc::LOG_FILE,
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
    fn describe(&self) -> String {
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

fn read_json(path: &Path, errors: &mut Vec<String>) -> Value {
    match read_text(path) {
        Ok(Some(text)) => match serde_json::from_str(&text) {
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
        } else if !is_our_entry(entry) {
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
                .map(|g| g.iter().filter(|g| is_our_hook_group(g)).collect())
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
    let block = match read_text(&setup.paths.instructions) {
        Ok(Some(text)) => match find_block(&text) {
            Ok(None) => PartState::Missing,
            Ok(Some(found)) if found.current => PartState::Installed,
            Ok(Some(found)) if found.intact => {
                PartState::Outdated("written by another version".to_string())
            }
            Ok(Some(_)) => PartState::Conflict("edited by hand".to_string()),
            Err(err) => PartState::Conflict(err),
        },
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
    let mut lines = vec![format!(
        "Claude Code (user configuration {}):",
        setup.paths.user_config.display()
    )];
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
    for error in &inspection.errors {
        lines.push(format!("  problem: {error}"));
    }
    if inspection.nothing_installed() {
        lines.push("  Not installed: run `graphersal-memory install`.".to_string());
    }
    lines
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

fn ok(text: impl Into<String>) -> Finding {
    Finding {
        problem: false,
        text: text.into(),
        fix: None,
    }
}

fn problem(text: impl Into<String>, fix: impl Into<String>) -> Finding {
    Finding {
        problem: true,
        text: text.into(),
        fix: Some(fix.into()),
    }
}

/// Whether `program` exists: a path, or a name found on the `PATH`.
fn program_found(program: &str) -> bool {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path.is_file();
    }
    std::env::var_os("PATH").is_some_and(|dirs| {
        std::env::split_paths(&dirs)
            .any(|dir| dir.join(program).is_file() || dir.join(format!("{program}.exe")).is_file())
    })
}

/// `doctor`'s checks of the installation and the data directory.
pub fn doctor(setup: &Setup) -> Vec<Finding> {
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
        PartState::Outdated(_) => {
            findings.push(problem("rule block written by another version", reinstall))
        }
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
                    "check their spelling (known: daemon.idle, summaries.enabled, \
                     summaries.harness_command, summaries.monthly_budget_usd, summaries.since)",
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
