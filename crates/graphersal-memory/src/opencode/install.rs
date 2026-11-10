//! `install`, `uninstall`, `status` and `doctor` for opencode: what is written into opencode's
//! global configuration directory, and nothing else (`crate::opencode` explains why each part
//! goes where it goes):
//!
//! - in the configuration file: the members `mcp.graphersal-memory` (the `use` profile,
//!   enabled) and `mcp.graphersal-memory-develop` (`"enabled": false` until `--enable-develop`),
//!   and the rules file's path as one element of `instructions`;
//! - the rules file [`crate::opencode::RULES_FILE`] holding only the managed rule block (the
//!   fingerprinted begin line as in Claude Code's `CLAUDE.md`, so a hand edit is a conflict);
//! - the plugin file `plugins/graphersal-memory.js`.
//!
//! The state is `<data dir>/install/opencode.json`, backups as for Claude Code; `uninstall`
//! restores an untouched file byte for byte from its backup (or deletes a file install
//! created), else removes only our parts from its current text.

use std::path::Path;

use serde_json::{Value, json};

use crate::agents::{Agent, Kind, is_our_entry};
use crate::install::{
    Change, DEVELOP_SERVER, Finding, PartState, Setup, State, Step, USE_SERVER, block_separator,
    find_block_in, ok, problem, read_json, read_text, wrap_block,
};
use crate::language::Language;
use crate::{fsutil, jsonedit};

/// The rule block for opencode with the English memory language rule.
pub const RULE_BLOCK: &str = "\
## Memory: graphersal-memory

Your long-term memory of this user lives in a local graph, served by the MCP server
`graphersal-memory` (tools named `graphersal-memory_<function>`).

- Store and search memories in English. Translate a user's words into English before saving
  them or searching; quote exact wording only when the exact words matter.
- Search it before you answer from memory or ask the user something they may have told you
  before: `memories(text)` for rules, decisions and preferences, `recall(text)` for everything,
  `memory(slug)` to read one in full.
- Save with `remember`: search with `memories(text)` first, so you replace a memory instead of
  duplicating it; scope \"global\" for rules and preferences that hold in every project. Remove
  with `forget`.
- Keep what you learn about this user in the graph, not in AGENTS.md or other files.
- Past sessions of this project: `sessions(text)` finds what was decided, corrected or left
  open in them (from their summaries, when summaries are on), `session(id)` reads one, and
  `recall(text, k, \"turn\")` finds what was literally said.
";

/// The rule block for opencode with the \"any language\" memory language rule.
pub const RULE_BLOCK_ANY: &str = "\
## Memory: graphersal-memory

Your long-term memory of this user lives in a local graph, served by the MCP server
`graphersal-memory` (tools named `graphersal-memory_<function>`).

- Store memories in the language the user uses; search in that language and in English.
- Search it before you answer from memory or ask the user something they may have told you
  before: `memories(text)` for rules, decisions and preferences, `recall(text)` for everything,
  `memory(slug)` to read one in full.
- Save with `remember`: search with `memories(text)` first, so you replace a memory instead of
  duplicating it; scope \"global\" for rules and preferences that hold in every project. Remove
  with `forget`.
- Keep what you learn about this user in the graph, not in AGENTS.md or other files.
- Past sessions of this project: `sessions(text)` finds what was decided, corrected or left
  open in them (from their summaries, when summaries are on), `session(id)` reads one, and
  `recall(text, k, \"turn\")` finds what was literally said.
";

/// The fingerprints of opencode rule blocks earlier versions wrote (none yet; add the current
/// one whenever a text above changes).
pub const PREVIOUS_BLOCK_FINGERPRINTS: &[&str] = &[];

/// The block text of a memory language.
pub fn rule_block(language: Language) -> &'static str {
    match language {
        Language::English => RULE_BLOCK,
        Language::Any => RULE_BLOCK_ANY,
    }
}

/// The managed block of `text` with `language`'s as the current one.
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

/// The text a configuration file install creates starts from.
fn new_config() -> String {
    format!(
        "{{\n  \"$schema\": {}\n}}\n",
        Value::from(crate::opencode::SCHEMA)
    )
}

/// The MCP entry of a profile.
pub fn mcp_entry(setup: &Setup, profile: &str, enabled: bool) -> Value {
    let mut args = vec![
        "mcp".to_string(),
        "--profile".to_string(),
        profile.to_string(),
    ];
    args.extend(setup.dir_args.iter().cloned());
    let mut entry = setup.opencode_agent().mcp_entry(&setup.exe, &args);
    entry["enabled"] = json!(enabled);
    entry
}

/// Whether an element of `instructions` is our rules file.
fn is_our_instruction(setup: &Setup, value: &Value) -> bool {
    value.as_str().is_some_and(|path| {
        Path::new(path) == setup.opencode.rules_file
            || path.ends_with(&format!("/{}", crate::opencode::RULES_FILE))
    })
}

/// The planned install for opencode: its steps, file changes and conflicts.
pub struct Plan {
    /// The steps for the wizard and the report.
    pub steps: Vec<Step>,
    /// The file changes.
    pub changes: Vec<Change>,
    /// What could not be done.
    pub conflicts: Vec<String>,
}

/// Plans opencode's part of the install.
pub fn plan(setup: &Setup, language: Language, develop: bool) -> Result<Plan, String> {
    let state = State::load_of(setup, Kind::OpenCode)?;
    let mut conflicts = Vec::new();
    let mut steps = Vec::new();
    let mut changes = Vec::new();
    let paths = &setup.opencode;

    // The configuration file: the MCP servers and the rules file in `instructions`.
    let path = &paths.config_file;
    let before = read_text(path)?;
    let mut text = before.clone().unwrap_or_else(new_config);
    let parsed = jsonedit::value_of(&text)
        .map_err(|err| format!("{} is not valid JSON(C): {err}", path.display()))?;
    let mut created = Vec::new();
    let mut lines = Vec::new();
    for (name, profile, enabled) in [
        (USE_SERVER, "use", true),
        (DEVELOP_SERVER, "develop", develop),
    ] {
        let existing = &parsed["mcp"][name];
        if !existing.is_null() && !is_our_entry(existing) {
            conflicts.push(format!(
                "{}: mcp.{name} exists and is not ours; kept. Remove it and run install again.",
                path.display()
            ));
            lines.push(format!(
                "{name}: CONFLICT (an entry of another program has this name)"
            ));
            continue;
        }
        let entry = mcp_entry(setup, profile, enabled);
        let state_word = if enabled { "enabled" } else { "disabled" };
        match jsonedit::set_member(&text, &["mcp"], name, &entry)? {
            Some((new, made)) => {
                lines.push(format!(
                    "{name} ({profile} profile): {} ({state_word})",
                    if existing.is_null() { "add" } else { "update" }
                ));
                text = new;
                created.extend(made);
            }
            None => lines.push(format!(
                "{name} ({profile} profile): already registered ({state_word})"
            )),
        }
    }
    let rules = paths.rules_file.display().to_string();
    let listed = parsed["instructions"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| is_our_instruction(setup, item)));
    if parsed["instructions"].is_array() || parsed["instructions"].is_null() {
        if listed {
            lines.push(format!("instructions: {rules} already listed"));
        } else {
            let (new, made) = jsonedit::append_element(&text, &["instructions"], &json!(rules))?;
            text = new;
            created.extend(made);
            lines.push(format!(
                "instructions: add {rules} (loaded into every session)"
            ));
        }
    } else {
        conflicts.push(format!(
            "{}: `instructions` is not a list; kept. Make it a list and run install again.",
            path.display()
        ));
    }
    steps.push(Step {
        title: format!(
            "Register the MCP servers and the rules file in opencode's {}",
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

    // The rules file.
    let path = &paths.rules_file;
    let before = read_text(path)?;
    let current = before.clone().unwrap_or_default();
    let block = wrap_block(rule_block(language));
    let mut separator = None;
    let (after, line) = match find_block(&current, language) {
        Err(err) => {
            conflicts.push(format!("{}: {err}; kept as it is.", path.display()));
            (None, "CONFLICT (see the report)".to_string())
        }
        Ok(Some(found)) if !found.intact => {
            conflicts.push(format!(
                "{}: the managed block was edited by hand; kept as it is. Move your text out of \
                 the block (or delete the block) and run install again.",
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
                "{}{block}{}",
                &current[..found.start],
                &current[found.end..]
            )),
            "update the block to this version".to_string(),
        ),
        Ok(None) => {
            let sep = block_separator(&current);
            separator = Some(sep.to_string());
            (
                Some(format!("{current}{sep}{block}")),
                if before.is_some() {
                    "append the block".to_string()
                } else {
                    "create it with the block".to_string()
                },
            )
        }
    };
    steps.push(Step {
        title: format!("Write the managed rule block into {}", path.display()),
        lines: vec![line],
    });
    if let Some(after) = after {
        changes.push(Change {
            path: path.clone(),
            after: Some(after),
            created: Vec::new(),
            separator,
        });
    }

    // The plugin file.
    let path = &paths.plugin_file;
    let wanted = crate::opencode::plugin_text(&setup.exe, &setup.dir_args);
    let line = match read_text(path)? {
        None => {
            changes.push(Change {
                path: path.clone(),
                after: Some(wanted),
                created: Vec::new(),
                separator: None,
            });
            "create it (session start: indexing + one toast line; session end: indexing)"
                .to_string()
        }
        Some(text) if text == wanted => "already installed".to_string(),
        Some(text) if crate::opencode::is_our_plugin(&text) && untouched(&state, path, &text) => {
            changes.push(Change {
                path: path.clone(),
                after: Some(wanted),
                created: Vec::new(),
                separator: None,
            });
            "update it to this version".to_string()
        }
        Some(_) => {
            conflicts.push(format!(
                "{}: the file exists and is not the one install wrote (or was edited); kept. \
                 Delete it and run install again.",
                path.display()
            ));
            "CONFLICT (see the report)".to_string()
        }
    };
    steps.push(Step {
        title: format!("Add the session plugin {}", path.display()),
        lines: vec![line],
    });
    Ok(Plan {
        steps,
        changes,
        conflicts,
    })
}

/// Whether `text` is what install last wrote to `path`.
fn untouched(state: &State, path: &Path, text: &str) -> bool {
    state
        .file(path)
        .and_then(|entry| entry.get("written"))
        .and_then(Value::as_str)
        == Some(fsutil::fingerprint(text.as_bytes()).as_str())
}

/// The removal of our parts from opencode's files: one `(change, what)` per file. With
/// `develop_only`, only the develop server is switched off (`"enabled": false`).
pub fn removal(
    setup: &Setup,
    state: &State,
    develop_only: bool,
    conflicts: &mut Vec<String>,
) -> Result<Vec<(Change, &'static str)>, String> {
    let paths = &setup.opencode;
    let mut out = Vec::new();
    let mut targets = vec![paths.config_file.clone()];
    if !develop_only {
        targets.push(paths.rules_file.clone());
        targets.push(paths.plugin_file.clone());
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
        let new = if path == paths.config_file {
            remove_from_config(setup, &text, entry, develop_only)?
        } else if path == paths.rules_file {
            match find_block(&text, Language::English) {
                Ok(Some(found)) if found.intact => {
                    let separator = entry
                        .and_then(|e| e.get("separator"))
                        .and_then(Value::as_str)
                        .unwrap_or("\n");
                    let mut head = &text[..found.start];
                    if found.end == text.len() && !separator.is_empty() && head.ends_with(separator)
                    {
                        head = &head[..head.len() - separator.len()];
                    }
                    Some(format!("{head}{}", &text[found.end..]))
                }
                Ok(Some(_)) => {
                    conflicts.push(format!(
                        "{}: the managed block was edited by hand, so it was not removed; delete \
                         it yourself",
                        path.display()
                    ));
                    None
                }
                Err(err) => {
                    conflicts.push(format!("{}: {err}; not changed", path.display()));
                    None
                }
                Ok(None) => None,
            }
        } else if crate::opencode::is_our_plugin(&text) {
            if entry.is_some() && !untouched_text(entry, &text) {
                conflicts.push(format!(
                    "{}: our plugin was edited by hand, so it was not removed; delete it yourself",
                    path.display()
                ));
                None
            } else {
                out.push((
                    Change {
                        path: path.clone(),
                        after: None,
                        created: Vec::new(),
                        separator: None,
                    },
                    "delete our plugin",
                ));
                None
            }
        } else {
            None
        };
        if let Some(new) = new {
            let empty = if path == paths.config_file {
                jsonedit::value_of(&new).ok().is_some_and(|v| {
                    v.as_object()
                        .is_some_and(|o| o.keys().all(|k| k == "$schema"))
                })
            } else {
                new.trim().is_empty()
            };
            let delete = created_by_us && empty;
            out.push((
                Change {
                    path: path.clone(),
                    after: (!delete).then_some(new),
                    created: Vec::new(),
                    separator: None,
                },
                if develop_only {
                    "switch the develop server off"
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

fn untouched_text(entry: Option<&serde_json::Map<String, Value>>, text: &str) -> bool {
    entry.and_then(|e| e.get("written")).and_then(Value::as_str)
        == Some(fsutil::fingerprint(text.as_bytes()).as_str())
}

/// The configuration text without our MCP members and our `instructions` element (and the
/// containers install created, when they are empty then); `develop_only`: the develop server
/// switched off instead.
fn remove_from_config(
    setup: &Setup,
    text: &str,
    entry: Option<&serde_json::Map<String, Value>>,
    develop_only: bool,
) -> Result<Option<String>, String> {
    let parsed = jsonedit::value_of(text).map_err(|err| {
        format!(
            "{} is not valid JSON(C): {err}",
            setup.opencode.config_file.display()
        )
    })?;
    let created: Vec<&str> = entry
        .and_then(|e| e.get("created"))
        .and_then(Value::as_array)
        .map(|c| c.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut current = text.to_string();
    let mut changed = false;
    if develop_only {
        let develop = &parsed["mcp"][DEVELOP_SERVER];
        if is_our_entry(develop)
            && develop["enabled"] != false
            && let Some((new, _)) = jsonedit::set_member(
                &current,
                &["mcp"],
                DEVELOP_SERVER,
                &mcp_entry(setup, "develop", false),
            )?
        {
            return Ok(Some(new));
        }
        return Ok(None);
    }
    for name in [USE_SERVER, DEVELOP_SERVER] {
        if is_our_entry(&parsed["mcp"][name])
            && let Some(new) = jsonedit::remove_member(&current, &["mcp"], name)?
        {
            current = new;
            changed = true;
        }
    }
    if changed
        && created.contains(&"mcp")
        && jsonedit::is_empty_container(&current, &["mcp"])
        && let Some(new) = jsonedit::remove_member(&current, &[], "mcp")?
    {
        current = new;
    }
    if let Some(new) = jsonedit::remove_elements(&current, &["instructions"], &|v| {
        is_our_instruction(setup, v)
    })? {
        current = new;
        changed = true;
        if created.contains(&"instructions")
            && jsonedit::is_empty_container(&current, &["instructions"])
            && let Some(new) = jsonedit::remove_member(&current, &[], "instructions")?
        {
            current = new;
        }
    }
    Ok(changed.then_some(current))
}

/// What is installed in opencode.
#[derive(Debug, Clone)]
pub struct Inspection {
    /// The `use` server.
    pub use_server: PartState,
    /// The `develop` server, and whether it is enabled.
    pub develop_server: (PartState, bool),
    /// The rules file in `instructions`.
    pub instructions: PartState,
    /// The managed block in the rules file.
    pub block: PartState,
    /// The plugin.
    pub plugin: PartState,
    /// Files that could not be read or parsed.
    pub errors: Vec<String>,
}

impl Inspection {
    /// Whether nothing of ours is installed.
    pub fn nothing_installed(&self) -> bool {
        self.use_server == PartState::Missing
            && self.develop_server.0 == PartState::Missing
            && self.instructions == PartState::Missing
            && self.block == PartState::Missing
            && self.plugin == PartState::Missing
    }
}

/// Inspects opencode's files.
pub fn inspect(setup: &Setup) -> Inspection {
    let paths = &setup.opencode;
    let mut errors = Vec::new();
    let config = read_json(&paths.config_file, &mut errors);
    let server = |name: &str, profile: &str| -> (PartState, bool) {
        let entry = &config["mcp"][name];
        if entry.is_null() {
            (PartState::Missing, false)
        } else if !is_our_entry(entry) {
            (
                PartState::Conflict("another program's entry has this name".to_string()),
                false,
            )
        } else {
            let enabled = entry["enabled"] != false;
            if *entry == mcp_entry(setup, profile, enabled) {
                (PartState::Installed, enabled)
            } else {
                (
                    PartState::Outdated(format!("starts {}", crate::agents::entry_program(entry))),
                    enabled,
                )
            }
        }
    };
    let use_server = server(USE_SERVER, "use").0;
    let develop_server = server(DEVELOP_SERVER, "develop");
    let instructions = if config["instructions"]
        .as_array()
        .is_some_and(|items| items.iter().any(|i| is_our_instruction(setup, i)))
    {
        PartState::Installed
    } else {
        PartState::Missing
    };
    let language = crate::config::Config::load(&setup.data_dir)
        .map(|c| c.language())
        .unwrap_or_default();
    let block = match read_text(&paths.rules_file) {
        Ok(Some(text)) => match find_block(&text, language) {
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
        },
        Ok(None) => PartState::Missing,
        Err(err) => {
            errors.push(err);
            PartState::Missing
        }
    };
    let plugin = match read_text(&paths.plugin_file) {
        Ok(None) => PartState::Missing,
        Ok(Some(text)) if text == crate::opencode::plugin_text(&setup.exe, &setup.dir_args) => {
            PartState::Installed
        }
        Ok(Some(text)) if crate::opencode::is_our_plugin(&text) => {
            PartState::Outdated("another binary or version, or edited".to_string())
        }
        Ok(Some(_)) => PartState::Conflict("another program's file has this name".to_string()),
        Err(err) => {
            errors.push(err);
            PartState::Missing
        }
    };
    Inspection {
        use_server,
        develop_server,
        instructions,
        block,
        plugin,
        errors,
    }
}

/// The opencode part of `status`.
pub fn status_lines(setup: &Setup) -> Vec<String> {
    let inspection = inspect(setup);
    let paths = &setup.opencode;
    let mut lines = vec![format!(
        "opencode (configuration {}):",
        paths.config_file.display()
    )];
    lines.push(format!(
        "  MCP server {USE_SERVER} (use): {}",
        inspection.use_server.describe()
    ));
    lines.push(format!(
        "  MCP server {DEVELOP_SERVER}: {}",
        match &inspection.develop_server {
            (PartState::Missing, _) => "not registered".to_string(),
            (PartState::Installed, false) =>
                "registered, disabled (`graphersal-memory install --harness opencode \
                 --enable-develop` enables it)"
                    .to_string(),
            (PartState::Installed, true) => "registered, enabled".to_string(),
            (other, _) => other.describe(),
        }
    ));
    lines.push(format!(
        "  rules file in instructions: {}",
        inspection.instructions.describe()
    ));
    lines.push(format!(
        "  rule block ({}): {}",
        paths.rules_file.display(),
        inspection.block.describe()
    ));
    lines.push(format!(
        "  session plugin ({}): {}",
        paths.plugin_file.display(),
        inspection.plugin.describe()
    ));
    lines.push("  auto-memory: opencode has none (nothing to switch off)".to_string());
    lines.push(format!(
        "  headless command (summaries, bench): `{}` by default",
        setup.opencode_agent().headless_command()
    ));
    lines.push(format!(
        "  sessions: read from {}{}",
        paths.db.display(),
        if paths.db.is_file() {
            ""
        } else {
            " (not there yet)"
        }
    ));
    for error in &inspection.errors {
        lines.push(format!("  problem: {error}"));
    }
    if inspection.nothing_installed() {
        lines.push(
            "  Not installed: run `graphersal-memory install --harness opencode`.".to_string(),
        );
    }
    lines
}

/// `doctor`'s checks of the opencode installation.
pub fn doctor(setup: &Setup) -> Vec<Finding> {
    let inspection = inspect(setup);
    let paths = &setup.opencode;
    let reinstall = "run `graphersal-memory install --harness opencode`";
    let mut findings = Vec::new();
    for error in &inspection.errors {
        findings.push(problem(
            format!("opencode: {error}"),
            "fix the file by hand (opencode cannot read it either), then run install again",
        ));
    }
    let check =
        |findings: &mut Vec<Finding>, what: String, state: &PartState, fix: &str| match state {
            PartState::Installed => findings.push(ok(format!("opencode: {what}: installed"))),
            other => findings.push(problem(
                format!("opencode: {what}: {}", other.describe()),
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
        (PartState::Installed, enabled) => findings.push(ok(format!(
            "opencode: MCP server {DEVELOP_SERVER}: {}",
            if *enabled { "on" } else { "off (disabled)" }
        ))),
        (state, _) => check(
            &mut findings,
            format!("MCP server {DEVELOP_SERVER}"),
            state,
            reinstall,
        ),
    }
    check(
        &mut findings,
        "rules file in instructions".to_string(),
        &inspection.instructions,
        reinstall,
    );
    check(
        &mut findings,
        format!("rule block in {}", paths.rules_file.display()),
        &inspection.block,
        reinstall,
    );
    check(
        &mut findings,
        format!("session plugin {}", paths.plugin_file.display()),
        &inspection.plugin,
        reinstall,
    );
    match crate::opencode::sessions::units(&paths.db) {
        Ok(units) => findings.push(ok(format!(
            "opencode: {} session(s) readable in {}",
            units.len(),
            paths.db.display()
        ))),
        Err(err) => findings.push(problem(
            format!("opencode: {err}"),
            "check the file's permissions; the sessions of opencode are indexed from it",
        )),
    }
    findings
}
