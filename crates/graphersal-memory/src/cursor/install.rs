//! `install`, `uninstall`, `status` and `doctor` for Cursor: what is written into `~/.cursor`,
//! and nothing else (`crate::cursor` explains why each part goes where it goes):
//!
//! - in `mcp.json`: the member `mcpServers.graphersal-memory` (the `use` profile), and
//!   `mcpServers.graphersal-memory-develop` only with `--enable-develop` (the file has no
//!   switched-off state);
//! - in `hooks.json`: one entry in `hooks.sessionStart` and one in `hooks.sessionEnd`, each the
//!   command `"<this binary>" hook session-start|session-end ... --harness cursor` (a new file
//!   starts with `"version": 1`);
//! - our own user rule file `~/.cursor/rules/graphersal-memory.mdc`: front matter
//!   (`alwaysApply: true`) and the managed rule block.
//!
//! The state is `<data dir>/install/cursor.json`, backups as for Claude Code; `uninstall`
//! restores an untouched file byte for byte from its backup (or deletes a file install
//! created), else removes only our parts from its current text.

use serde_json::{Value, json};

use crate::agents::{Agent, is_our_entry};
use crate::install::{
    Change, DEVELOP_SERVER, Finding, PartState, Setup, State, Step, USE_SERVER, find_block_in,
    is_our_command, ok, problem, read_json, read_text, wrap_block,
};
use crate::language::Language;
use crate::{fsutil, jsonedit};

/// The rule block for Cursor with the English memory language rule.
pub const RULE_BLOCK: &str = "\
## Memory: graphersal-memory

Your long-term memory of this user lives in a local graph, served by the MCP server
`graphersal-memory` (its tools `memories`, `recall`, `memory`, `remember`, `forget`, `sessions`,
`session`).

- Store and search memories in English. Translate a user's words into English before saving
  them or searching; quote exact wording only when the exact words matter.
- Search it before you answer from memory or ask the user something they may have told you
  before: `memories(text)` for rules, decisions and preferences, `recall(text)` for everything,
  `memory(slug)` to read one in full.
- Save with `remember`: search with `memories(text)` first, so you replace a memory instead of
  duplicating it; scope \"global\" for rules and preferences that hold in every project. Remove
  with `forget`.
- Keep what you learn about this user in the graph, not in rules files, AGENTS.md or other
  files.
- Past sessions of this project: `sessions(text)` finds what was decided, corrected or left
  open in them (from their summaries, when summaries are on), `session(id)` reads one, and
  `recall(text, k, \"turn\")` finds what was literally said.
";

/// The rule block for Cursor with the \"any language\" memory language rule.
pub const RULE_BLOCK_ANY: &str = "\
## Memory: graphersal-memory

Your long-term memory of this user lives in a local graph, served by the MCP server
`graphersal-memory` (its tools `memories`, `recall`, `memory`, `remember`, `forget`, `sessions`,
`session`).

- Store memories in the language the user uses; search in that language and in English.
- Search it before you answer from memory or ask the user something they may have told you
  before: `memories(text)` for rules, decisions and preferences, `recall(text)` for everything,
  `memory(slug)` to read one in full.
- Save with `remember`: search with `memories(text)` first, so you replace a memory instead of
  duplicating it; scope \"global\" for rules and preferences that hold in every project. Remove
  with `forget`.
- Keep what you learn about this user in the graph, not in rules files, AGENTS.md or other
  files.
- Past sessions of this project: `sessions(text)` finds what was decided, corrected or left
  open in them (from their summaries, when summaries are on), `session(id)` reads one, and
  `recall(text, k, \"turn\")` finds what was literally said.
";

/// The fingerprints of Cursor rule blocks earlier versions wrote (none yet; add the current one
/// whenever a text above changes).
pub const PREVIOUS_BLOCK_FINGERPRINTS: &[&str] = &[];

/// The front matter of our rule file: always applied (cursor.com/docs/rules, "Rule anatomy").
pub const FRONT_MATTER: &str = "---\n\
description: How to use the long-term memory served by the MCP server graphersal-memory\n\
alwaysApply: true\n\
---\n\n";

/// The start of a `hooks.json` install creates (cursor.com/docs/hooks: `"version": 1`).
const NEW_HOOKS_FILE: &str = "{\n  \"version\": 1\n}\n";

/// Cursor's hook events and our subcommands.
pub const HOOK_EVENTS: [(&str, &str); 2] = [
    ("sessionStart", "session-start"),
    ("sessionEnd", "session-end"),
];

/// How Cursor asks before a memory tool call (its default; install never changes it).
pub const APPROVAL_NOTE: &str = "Cursor asks before each MCP tool call by default; to stop \
    being asked for the memory tools, allow them in the approval prompt or in your run mode \
    settings (your choice; install does not change it)";

/// The block text of a memory language.
pub fn rule_block(language: Language) -> &'static str {
    match language {
        Language::English => RULE_BLOCK,
        Language::Any => RULE_BLOCK_ANY,
    }
}

/// The managed rule block of `text` with `language`'s as the current one.
pub fn find_block(
    text: &str,
    language: Language,
) -> Result<Option<crate::install::FoundBlock>, String> {
    find_block_in(
        text,
        rule_block(language),
        &[RULE_BLOCK, RULE_BLOCK_ANY],
        PREVIOUS_BLOCK_FINGERPRINTS,
    )
}

/// The whole text of our rule file.
pub fn rules_text(language: Language) -> String {
    format!("{FRONT_MATTER}{}", wrap_block(rule_block(language)))
}

/// The MCP entry of a profile.
pub fn mcp_entry(setup: &Setup, profile: &str) -> Value {
    let mut args = vec![
        "mcp".to_string(),
        "--profile".to_string(),
        profile.to_string(),
    ];
    args.extend(setup.dir_args.iter().cloned());
    setup.cursor_agent().mcp_entry(&setup.exe, &args)
}

/// The hook command of an event (`session-start`, `session-end`). On Windows Cursor runs hook
/// commands through PowerShell (cursor.com/docs/hooks): the words every shell runs unquoted
/// when there are such (`crate::winshell`), else PowerShell's `& '...'` form.
pub fn hook_command(setup: &Setup, event: &str) -> String {
    hook_command_for(setup, event, cfg!(windows))
}

fn hook_command_for(setup: &Setup, event: &str, powershell: bool) -> String {
    let command = if powershell && setup.hooks.plain.is_none() {
        setup.powershell_hook_command(event)
    } else {
        setup.hook_command(event)
    };
    format!("{command} --harness cursor")
}

/// The hook entry of an event, as it goes into `hooks.json`.
pub fn hook_entry(setup: &Setup, event: &str) -> Value {
    json!({"command": hook_command(setup, event), "timeout": crate::cursor::HOOK_TIMEOUT_SECS})
}

/// Whether a hook entry (an element of `hooks.<event>`) is ours.
pub fn is_our_hook(entry: &Value) -> bool {
    is_our_command(entry["command"].as_str().unwrap_or_default())
}

/// The planned install for Cursor: its steps, file changes and conflicts.
pub struct Plan {
    /// The steps for the wizard and the report.
    pub steps: Vec<Step>,
    /// The file changes.
    pub changes: Vec<Change>,
    /// What could not be done.
    pub conflicts: Vec<String>,
}

/// Plans Cursor's part of the install.
pub fn plan(setup: &Setup, language: Language, develop: bool) -> Result<Plan, String> {
    let mut conflicts = Vec::new();
    let mut steps = Vec::new();
    let mut changes = Vec::new();
    let paths = &setup.cursor;

    // mcp.json: the servers.
    let path = &paths.mcp_file;
    let key = "mcpServers";
    let before = read_text(path)?;
    let mut text = before.clone().unwrap_or_else(|| "{\n}\n".to_string());
    let parsed = jsonedit::value_of(&text)
        .map_err(|err| format!("{} is not valid JSON: {err}", path.display()))?;
    let mut created = Vec::new();
    let mut lines = Vec::new();
    let mut servers = vec![(USE_SERVER, "use")];
    if develop {
        servers.push((DEVELOP_SERVER, "develop"));
    }
    for (name, profile) in servers {
        let existing = &parsed[key][name];
        if !existing.is_null() && !is_our_entry(existing) {
            conflicts.push(format!(
                "{}: {key}.{name} exists and is not ours; kept. Remove it and run install again.",
                path.display()
            ));
            lines.push(format!(
                "{name}: CONFLICT (an entry of another program has this name)"
            ));
            continue;
        }
        match jsonedit::set_member(&text, &[key], name, &mcp_entry(setup, profile))? {
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
    if !develop {
        let existing = &parsed[key][DEVELOP_SERVER];
        if existing.is_null() {
            lines.push(format!(
                "{DEVELOP_SERVER}: not registered (off; `graphersal-memory install --harness \
                 cursor --enable-develop` registers it)"
            ));
        } else if is_our_entry(existing)
            && let Some(new) = jsonedit::remove_member(&text, &[key], DEVELOP_SERVER)?
        {
            text = new;
            lines.push(format!("{DEVELOP_SERVER}: unregister (you answered no)"));
        } else {
            lines.push(format!("{DEVELOP_SERVER}: registered (kept)"));
        }
    }
    lines.push(APPROVAL_NOTE.to_string());
    steps.push(Step {
        title: format!(
            "Register the MCP servers in Cursor's {} (key {key})",
            path.display()
        ),
        lines,
    });
    if Some(&text) != before.as_ref() {
        changes.push(Change {
            path: path.clone(),
            after: Some(text),
            created,
            separator: None,
        });
    }

    // hooks.json: the session start and end hooks.
    let path = &paths.hooks_file;
    let before = read_text(path)?;
    let mut text = before.clone().unwrap_or_else(|| NEW_HOOKS_FILE.to_string());
    let parsed = jsonedit::value_of(&text)
        .map_err(|err| format!("{} is not valid JSON: {err}", path.display()))?;
    let mut created = Vec::new();
    let mut lines = Vec::new();
    for (event, sub) in HOOK_EVENTS {
        let entry = hook_entry(setup, sub);
        let ours: Vec<&Value> = parsed["hooks"][event]
            .as_array()
            .map(|entries| entries.iter().filter(|e| is_our_hook(e)).collect())
            .unwrap_or_default();
        if ours.len() == 1 && *ours[0] == entry {
            lines.push(format!("{event}: already installed"));
            continue;
        }
        if !ours.is_empty()
            && let Some(new) = jsonedit::remove_elements(&text, &["hooks", event], &is_our_hook)?
        {
            text = new;
        }
        let (new, made) = jsonedit::append_element(&text, &["hooks", event], &entry)?;
        text = new;
        created.extend(made);
        lines.push(format!(
            "{event}: {} `{}`",
            if ours.is_empty() { "add" } else { "update to" },
            hook_command(setup, sub)
        ));
    }
    lines.push(
        "user hooks need no approval; Cursor reloads the file on save (restart Cursor if they \
         do not load)"
            .to_string(),
    );
    steps.push(Step {
        title: format!(
            "Add the sessionStart and sessionEnd hooks in {}",
            path.display()
        ),
        lines,
    });
    if Some(&text) != before.as_ref() {
        changes.push(Change {
            path: path.clone(),
            after: Some(text),
            created,
            separator: None,
        });
    }

    // Our user rule file.
    let path = &paths.rules_file;
    let before = read_text(path)?;
    let (after, line) = match &before {
        None => (
            Some(rules_text(language)),
            "create it (front matter alwaysApply: true, and the rule block)".to_string(),
        ),
        Some(current) => match find_block(current, language) {
            Err(err) => {
                conflicts.push(format!("{}: {err}; kept as it is.", path.display()));
                (None, "CONFLICT (see the report)".to_string())
            }
            Ok(None) => {
                conflicts.push(format!(
                    "{}: exists and is not graphersal-memory's; kept as it is. Rename or delete \
                     it and run install again.",
                    path.display()
                ));
                (None, "CONFLICT (see the report)".to_string())
            }
            Ok(Some(found)) if !found.intact => {
                conflicts.push(format!(
                    "{}: the managed block was edited by hand; kept as it is. Move your text out \
                     of the block (or delete the file) and run install again.",
                    path.display()
                ));
                (None, "CONFLICT (see the report)".to_string())
            }
            Ok(Some(found)) if found.current => (None, "already written".to_string()),
            Ok(Some(found)) if !found.known => {
                conflicts.push(format!(
                    "{}: the managed block was written by another (newer?) version of \
                     graphersal-memory; kept as it is.",
                    path.display()
                ));
                (None, "CONFLICT (see the report)".to_string())
            }
            Ok(Some(found)) => (
                Some(format!(
                    "{}{}{}",
                    &current[..found.start],
                    wrap_block(rule_block(language)),
                    &current[found.end..]
                )),
                "update the block to this version".to_string(),
            ),
        },
    };
    steps.push(Step {
        title: format!("Write the rule file {}", path.display()),
        lines: vec![line],
    });
    if let Some(after) = after {
        changes.push(Change {
            path: path.clone(),
            after: Some(after),
            created: Vec::new(),
            separator: None,
        });
    }
    Ok(Plan {
        steps,
        changes,
        conflicts,
    })
}

/// The removal of our parts from Cursor's files: one `(change, what)` per file. With
/// `develop_only`, only the develop server is unregistered.
pub fn removal(
    setup: &Setup,
    state: &State,
    develop_only: bool,
    conflicts: &mut Vec<String>,
) -> Result<Vec<(Change, &'static str)>, String> {
    let paths = &setup.cursor;
    let mut out = Vec::new();
    let mut targets = vec![paths.mcp_file.clone()];
    if !develop_only {
        targets.push(paths.hooks_file.clone());
        targets.push(paths.rules_file.clone());
    }
    for path in targets {
        let Some(bytes) = fsutil::read_optional(&path)? else {
            continue;
        };
        let entry = state.file(&path);
        let fp = fsutil::fingerprint(&bytes);
        let untouched = entry.is_some_and(|e| {
            e.get("written").and_then(Value::as_str) == Some(fp.as_str())
                && e.get("backup_valid") == Some(&Value::Bool(true))
        });
        let existed = entry.is_some_and(|e| e.get("existed") == Some(&Value::Bool(true)));
        if untouched && !develop_only {
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
            out.push((
                Change {
                    path: path.clone(),
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
        let text = String::from_utf8(bytes)
            .map_err(|_| format!("{} is not UTF-8 text", path.display()))?;
        let created_by_us = entry.is_some_and(|e| e.get("existed") == Some(&Value::Bool(false)));
        let (new, empty) = if path == paths.mcp_file {
            let new = remove_servers(&text, entry, develop_only)?;
            let empty = new.as_deref().is_some_and(|t| {
                jsonedit::value_of(t)
                    .ok()
                    .is_some_and(|v| v.as_object().is_some_and(|o| o.is_empty()))
            });
            (new, empty)
        } else if path == paths.hooks_file {
            let new = remove_hooks(&text, entry)?;
            // Empty: nothing but the `"version"` install wrote.
            let empty = new.as_deref().is_some_and(|t| {
                jsonedit::value_of(t).ok().is_some_and(|v| {
                    v.as_object()
                        .is_some_and(|o| o.keys().all(|k| k == "version"))
                })
            });
            (new, empty)
        } else {
            let new = match find_block(&text, Language::English) {
                Ok(Some(found)) if found.intact => {
                    Some(format!("{}{}", &text[..found.start], &text[found.end..]))
                }
                Ok(Some(_)) => {
                    conflicts.push(format!(
                        "{}: the managed block was edited by hand, so it was not removed; delete \
                         the file yourself",
                        path.display()
                    ));
                    None
                }
                Err(err) => {
                    conflicts.push(format!("{}: {err}; not changed", path.display()));
                    None
                }
                Ok(None) => None,
            };
            // Empty: nothing but our front matter.
            let empty = new
                .as_deref()
                .is_some_and(|t| t.trim().is_empty() || t.trim() == FRONT_MATTER.trim());
            (new, empty)
        };
        if let Some(new) = new {
            let delete = created_by_us && empty && !develop_only;
            out.push((
                Change {
                    path: path.clone(),
                    after: (!delete).then_some(new),
                    created: Vec::new(),
                    separator: None,
                },
                if develop_only {
                    "unregister the develop server"
                } else if delete {
                    "remove our parts and delete the file (install created it, nothing else is in it)"
                } else {
                    "remove our parts (the rest of the file stays as it is)"
                },
            ));
        }
    }
    Ok(out)
}

/// `mcp.json` without our servers (and the `mcpServers` object install created, when empty
/// then); `develop_only`: without the develop server only.
fn remove_servers(
    text: &str,
    entry: Option<&serde_json::Map<String, Value>>,
    develop_only: bool,
) -> Result<Option<String>, String> {
    let parsed = jsonedit::value_of(text).map_err(|err| format!("not valid JSON: {err}"))?;
    let names: &[&str] = if develop_only {
        &[DEVELOP_SERVER]
    } else {
        &[USE_SERVER, DEVELOP_SERVER]
    };
    let mut current = text.to_string();
    let mut changed = false;
    for name in names {
        if is_our_entry(&parsed["mcpServers"][name])
            && let Some(new) = jsonedit::remove_member(&current, &["mcpServers"], name)?
        {
            current = new;
            changed = true;
        }
    }
    let created = entry
        .and_then(|e| e.get("created"))
        .and_then(Value::as_array)
        .is_some_and(|c| c.iter().any(|item| item == "mcpServers"));
    if changed
        && !develop_only
        && created
        && jsonedit::is_empty_container(&current, &["mcpServers"])
        && let Some(new) = jsonedit::remove_member(&current, &[], "mcpServers")?
    {
        current = new;
    }
    Ok(changed.then_some(current))
}

/// `hooks.json` without our entries (and the containers install created, when empty then).
fn remove_hooks(
    text: &str,
    entry: Option<&serde_json::Map<String, Value>>,
) -> Result<Option<String>, String> {
    let created: Vec<&str> = entry
        .and_then(|e| e.get("created"))
        .and_then(Value::as_array)
        .map(|c| c.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut current = text.to_string();
    let mut changed = false;
    for (event, _) in HOOK_EVENTS {
        if let Some(new) = jsonedit::remove_elements(&current, &["hooks", event], &is_our_hook)? {
            current = new;
            changed = true;
            let dotted = format!("hooks.{event}");
            if created.contains(&dotted.as_str())
                && jsonedit::is_empty_container(&current, &["hooks", event])
                && let Some(new) = jsonedit::remove_member(&current, &["hooks"], event)?
            {
                current = new;
            }
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

/// What is installed in Cursor.
#[derive(Debug, Clone)]
pub struct Inspection {
    /// The `use` server.
    pub use_server: PartState,
    /// The `develop` server.
    pub develop_server: PartState,
    /// The hooks, per event.
    pub hooks: Vec<(String, PartState)>,
    /// Our rule file.
    pub rules: PartState,
    /// Files that could not be read or parsed.
    pub errors: Vec<String>,
}

impl Inspection {
    /// Whether nothing of ours is installed.
    pub fn nothing_installed(&self) -> bool {
        self.use_server == PartState::Missing
            && self.develop_server == PartState::Missing
            && self.hooks.iter().all(|(_, s)| *s == PartState::Missing)
            && self.rules == PartState::Missing
    }
}

/// Inspects Cursor's files.
pub fn inspect(setup: &Setup) -> Inspection {
    let paths = &setup.cursor;
    let mut errors = Vec::new();
    let config = read_json(&paths.mcp_file, &mut errors);
    let server = |name: &str, profile: &str| -> PartState {
        let entry = &config["mcpServers"][name];
        if entry.is_null() {
            PartState::Missing
        } else if !is_our_entry(entry) {
            PartState::Conflict("another program's entry has this name".to_string())
        } else if *entry == mcp_entry(setup, profile) {
            PartState::Installed
        } else {
            PartState::Outdated(format!(
                "starts {}",
                entry["command"].as_str().unwrap_or_default()
            ))
        }
    };
    let hooks_json = read_json(&paths.hooks_file, &mut errors);
    let hooks = HOOK_EVENTS
        .iter()
        .map(|(event, sub)| {
            let ours: Vec<&Value> = hooks_json["hooks"][*event]
                .as_array()
                .map(|entries| entries.iter().filter(|e| is_our_hook(e)).collect())
                .unwrap_or_default();
            let state = match ours.as_slice() {
                [] => PartState::Missing,
                [entry] if **entry == hook_entry(setup, sub) => PartState::Installed,
                _ => PartState::Outdated("another binary or version".to_string()),
            };
            (event.to_string(), state)
        })
        .collect();
    let language = crate::config::Config::load(&setup.data_dir)
        .map(|c| c.language())
        .unwrap_or_default();
    let rules = match read_text(&paths.rules_file) {
        Ok(Some(text)) => match find_block(&text, language) {
            Ok(None) => PartState::Conflict("a file of another program has this name".to_string()),
            Ok(Some(found)) if found.current => PartState::Installed,
            Ok(Some(found)) if found.known => {
                PartState::Outdated("written by an earlier version".to_string())
            }
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
        use_server: server(USE_SERVER, "use"),
        develop_server: server(DEVELOP_SERVER, "develop"),
        hooks,
        rules,
        errors,
    }
}

/// How many transcripts the hooks reported, and how many of them exist.
fn transcripts(setup: &Setup) -> (usize, usize) {
    let registered = crate::cursor::sessions::load(&setup.cursor.registry)
        .map(|r| r.len())
        .unwrap_or(0);
    let present = crate::cursor::sessions::units(&setup.cursor.registry)
        .map(|u| u.len())
        .unwrap_or(0);
    (registered, present)
}

/// The Cursor part of `status`.
pub fn status_lines(setup: &Setup) -> Vec<String> {
    let inspection = inspect(setup);
    let paths = &setup.cursor;
    let mut lines = vec![
        "Cursor:".to_string(),
        format!("  configuration: {}", paths.cursor_dir.display()),
    ];
    if let Some(program) = crate::cursor::find_program(setup.search_path.as_deref()) {
        lines.push(format!("  program: {}", program.display()));
    }
    lines.push(format!(
        "  MCP server {USE_SERVER} (use, {}): {}",
        paths.mcp_file.display(),
        inspection.use_server.describe()
    ));
    lines.push(format!(
        "  MCP server {DEVELOP_SERVER}: {}",
        match &inspection.develop_server {
            PartState::Missing => "not registered (`graphersal-memory install --harness cursor \
                 --enable-develop` registers it)"
                .to_string(),
            other => other.describe(),
        }
    ));
    for (event, state) in &inspection.hooks {
        lines.push(format!(
            "  hook {event} ({}): {}",
            paths.hooks_file.display(),
            state.describe()
        ));
    }
    lines.push(format!(
        "  rule file ({}): {}",
        paths.rules_file.display(),
        inspection.rules.describe()
    ));
    lines.push("  Cursor memories: none in a file (nothing to switch off)".to_string());
    lines.push(
        "  headless command (summaries, bench): none (Cursor's CLI is not used; summaries run \
         through Claude Code, opencode or Codex when one is installed)"
            .to_string(),
    );
    let (registered, present) = transcripts(setup);
    lines.push(format!(
        "  sessions: {registered} transcript(s) reported by the hooks ({present} on disk), \
         registry {}",
        paths.registry.display()
    ));
    for error in &inspection.errors {
        lines.push(format!("  problem: {error}"));
    }
    if inspection.nothing_installed() {
        lines
            .push("  Not installed: run `graphersal-memory install --harness cursor`.".to_string());
    }
    lines
}

/// `doctor`'s checks of the Cursor installation.
pub fn doctor(setup: &Setup) -> Vec<Finding> {
    let inspection = inspect(setup);
    let paths = &setup.cursor;
    let reinstall = "run `graphersal-memory install --harness cursor`";
    let mut findings = vec![ok(format!(
        "Cursor: configuration {}",
        paths.cursor_dir.display()
    ))];
    for error in &inspection.errors {
        findings.push(problem(
            format!("Cursor: {error}"),
            "fix the file by hand (Cursor cannot read it either), then run install again",
        ));
    }
    let check =
        |findings: &mut Vec<Finding>, what: String, state: &PartState, fix: &str| match state {
            PartState::Installed => findings.push(ok(format!("Cursor: {what}: installed"))),
            other => findings.push(problem(
                format!("Cursor: {what}: {}", other.describe()),
                fix.to_string(),
            )),
        };
    check(
        &mut findings,
        format!("MCP server {USE_SERVER}"),
        &inspection.use_server,
        reinstall,
    );
    match &inspection.develop_server {
        PartState::Missing => findings.push(ok(format!(
            "Cursor: MCP server {DEVELOP_SERVER}: off (not registered)"
        ))),
        state => check(
            &mut findings,
            format!("MCP server {DEVELOP_SERVER}"),
            state,
            reinstall,
        ),
    }
    for (event, state) in &inspection.hooks {
        check(&mut findings, format!("hook {event}"), state, reinstall);
    }
    check(
        &mut findings,
        format!("rule file {}", paths.rules_file.display()),
        &inspection.rules,
        reinstall,
    );
    let (registered, present) = transcripts(setup);
    if inspection
        .hooks
        .iter()
        .all(|(_, s)| *s == PartState::Installed)
        && registered == 0
    {
        findings.push(Finding {
            problem: false,
            text: "Cursor: no transcript reported by the hooks yet".to_string(),
            fix: Some(
                "start a chat in Cursor after install; if none appears, check that the hooks \
                 ran (Cursor's Hooks output channel) and that transcripts are on (the hooks \
                 receive no transcript_path otherwise)"
                    .to_string(),
            ),
        });
    } else {
        findings.push(ok(format!(
            "Cursor: {registered} transcript(s) reported by the hooks, {present} on disk"
        )));
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_windows_hook_command_is_powershell() {
        let tmp = crate::mcp_tests::TempDir::new("cu-ps");
        let mut setup = crate::install_tests::setup_in(tmp.path(), tmp.path());
        setup.exe = std::path::PathBuf::from(r"C:\Users\O'Neil\gm\graphersal-memory.exe");
        setup.dir_args = vec!["--data-dir".into(), r"C:\d $x".into()];
        assert_eq!(
            hook_command_for(&setup, "session-start", true),
            r"& 'C:\Users\O''Neil\gm\graphersal-memory.exe' hook session-start --data-dir 'C:\d $x' --harness cursor"
        );
        assert_eq!(
            hook_command_for(&setup, "session-end", false),
            r#""C:\Users\O'Neil\gm\graphersal-memory.exe" hook session-end --data-dir "C:\d $x" --harness cursor"#
        );
        assert!(is_our_command(&hook_command_for(
            &setup,
            "session-start",
            true
        )));
    }

    #[test]
    fn the_rule_file_has_front_matter_and_the_block() {
        let text = rules_text(Language::English);
        assert!(text.starts_with("---\ndescription: "), "{text}");
        assert!(text.contains("\nalwaysApply: true\n---\n\n<!-- graphersal-memory:begin"));
        let found = find_block(&text, Language::English).unwrap().unwrap();
        assert!(found.intact && found.current);
        assert_eq!(&text[..found.start], FRONT_MATTER);
        let any = find_block(&text, Language::Any).unwrap().unwrap();
        assert!(
            any.known && !any.current,
            "the other language's block is known"
        );
        // The YAML front matter has no `: ` inside a value.
        for line in FRONT_MATTER.lines().filter(|l| l.contains(':')) {
            assert_eq!(line.matches(": ").count(), 1, "{line}");
        }
    }
}
