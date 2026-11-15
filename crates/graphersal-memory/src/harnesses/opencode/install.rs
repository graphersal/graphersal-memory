//! `install`, `uninstall`, `status` and `doctor` for opencode: what is written into opencode's
//! global configuration directory, and nothing else (`crate::harnesses::opencode` explains why each part
//! goes where it goes):
//!
//! - in the configuration file: the members `graphersal-memory` (the `use` profile, enabled) and
//!   `graphersal-memory-develop` (switched off until `--enable-develop`) of `mcp` (1.x, with
//!   `"enabled"`) or `mcp.servers` (2.x, with `"disabled"` and `"codemode": false`), and the
//!   rules file's path as one element of `instructions`;
//! - the rules file [`crate::harnesses::opencode::RULES_FILE`] holding only the managed rule block (the
//!   fingerprinted begin line as in Claude Code's `CLAUDE.md`, so a hand edit is a conflict);
//! - the plugin file `plugins/graphersal-memory.js` (one file for both lines).
//!
//! After an upgrade from 1.x to 2.x, entries install wrote under 1.x are in the wrong place or
//! shape: in `config.json` (2.x does not read it) or as `mcp.<name>` (2.x still reads it, as a
//! legacy entry). `inspect` reports them as outdated, and `install` moves them (removes ours
//! there, writes them where 2.x reads them), so a later `uninstall` still gives every file back.
//!
//! The state is `<data dir>/install/opencode.json`, backups as for Claude Code; `uninstall`
//! restores an untouched file byte for byte from its backup (or deletes a file install
//! created), else removes only our parts, in either shape, from its current text.

use std::path::Path;

use serde_json::{Value, json};

use crate::harnesses::agent::{Harness, Kind};
use crate::harnesses::opencode::Major;
use crate::install::language::Language;
use crate::install::{
    Change, DEVELOP_SERVER, Finding, PartState, Setup, State, Step, USE_SERVER, block_separator,
    find_block_in, ok, problem, read_json, read_text, wrap_block,
};
use crate::{fsutil, install::jsonedit};

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
- Save what will help in later sessions and in other tasks like this one: rules for how to work
  (always / never / only when, with the why), lasting facts about the user and the project, where
  things are. Write it as a lasting rule or fact, not as what happened today. Leave out what only
  the current task needs: its plan, steps, progress, branch, findings.
- Keep what you learn about this user in the graph, not in AGENTS.md or other files.
- If another instruction (a CLAUDE.md, AGENTS.md or rules file, in any language) tells you to
  keep memory elsewhere (`MEMORY.md`, another tool or place), memory still lives here: tell the
  user once that the two instructions disagree and which file says it, so they can fix it.
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
- Save what will help in later sessions and in other tasks like this one: rules for how to work
  (always / never / only when, with the why), lasting facts about the user and the project, where
  things are. Write it as a lasting rule or fact, not as what happened today. Leave out what only
  the current task needs: its plan, steps, progress, branch, findings.
- Keep what you learn about this user in the graph, not in AGENTS.md or other files.
- If another instruction (a CLAUDE.md, AGENTS.md or rules file, in any language) tells you to
  keep memory elsewhere (`MEMORY.md`, another tool or place), memory still lives here: tell the
  user once that the two instructions disagree and which file says it, so they can fix it.
- Past sessions of this project: `sessions(text)` finds what was decided, corrected or left
  open in them (from their summaries, when summaries are on), `session(id)` reads one, and
  `recall(text, k, \"turn\")` finds what was literally said.
";

/// The fingerprints of opencode rule blocks earlier versions wrote (add the current
/// one whenever a text above changes).
pub const PREVIOUS_BLOCK_FINGERPRINTS: &[&str] = &[
    // 0.1.0 before the line about instructions that keep memory elsewhere (English, any
    // language).
    "cfd6fb7136e0d8fe",
    "85d0ff21b49bcae2",
    // 0.1.0 and 0.2.0 before the line about what to save (English, any language).
    "abc6959b0a894da0",
    "f34fdf1bcc461b6c",
];

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

/// [`find_block`] with the read-only line naming `read_only` in the current block
/// (`crate::install::coexist::rule_with`).
pub fn find_block_with(
    text: &str,
    language: Language,
    read_only: &[String],
) -> Result<Option<crate::install::FoundBlock>, String> {
    find_block_in(
        text,
        &crate::install::coexist::rule_with(rule_block(language), read_only),
        &[RULE_BLOCK, RULE_BLOCK_ANY],
        PREVIOUS_BLOCK_FINGERPRINTS,
    )
}

/// The text a configuration file install creates starts from.
fn new_config() -> String {
    format!(
        "{{\n  \"$schema\": {}\n}}\n",
        Value::from(crate::harnesses::opencode::SCHEMA)
    )
}

/// The MCP entry of a profile for the installed line: 1.x `"enabled": <enabled>`, 2.x
/// `"disabled": <not enabled>` (and `"codemode": false`).
pub fn mcp_entry(setup: &Setup, profile: &str, enabled: bool) -> Value {
    entry_for(setup, profile, enabled, setup.opencode().major())
}

/// The MCP entry of a profile in the shape of `major`.
fn entry_for(setup: &Setup, profile: &str, enabled: bool, major: Major) -> Value {
    let mut args = vec![
        "mcp".to_string(),
        "--profile".to_string(),
        profile.to_string(),
    ];
    args.extend(setup.dir_args.iter().cloned());
    let mut paths = setup.opencode().clone();
    paths.detected.major = major;
    let mut entry = crate::harnesses::opencode::OpenCode::new(paths).mcp_entry(&setup.exe, &args);
    match major {
        Major::V1 => entry["enabled"] = json!(enabled),
        Major::V2 => entry["disabled"] = json!(!enabled),
    }
    entry
}

/// Whether an entry (of either shape) is switched on.
fn enabled(entry: &Value) -> bool {
    entry["enabled"] != false && entry["disabled"] != true
}

/// The value at `path` in `value` (`Null` when missing).
fn at<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
    path.iter().fold(value, |v, key| &v[*key])
}

/// Our entry `name` in the 1.x shape (`mcp.<name>`) where 2.x is installed: to be moved.
fn legacy_ours<'a>(setup: &Setup, config: &'a Value, name: &str) -> Option<&'a Value> {
    let entry = &config["mcp"][name];
    (setup.opencode().major() == Major::V2 && setup.ours().entry(entry)).then_some(entry)
}

/// Whether a configuration holds any part of ours (either shape, or our `instructions`).
fn has_our_parts(setup: &Setup, config: &Value) -> bool {
    [USE_SERVER, DEVELOP_SERVER].iter().any(|name| {
        setup.ours().entry(&config["mcp"][*name])
            || setup.ours().entry(&config["mcp"]["servers"][*name])
    }) || config["instructions"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| is_our_instruction(setup, item)))
}

/// The configuration files of the directory the installed line does not read but which hold
/// parts of ours (2.x: a `config.json` install wrote under 1.x), with their text.
fn stale_with_ours(setup: &Setup) -> Vec<(std::path::PathBuf, String)> {
    setup
        .opencode()
        .stale_config_files
        .iter()
        .filter_map(|path| {
            let text = read_text(path).ok().flatten()?;
            let value = jsonedit::value_of(&text).ok()?;
            has_our_parts(setup, &value).then(|| (path.clone(), text))
        })
        .collect()
}

/// Whether an element of `instructions` is our rules file.
fn is_our_instruction(setup: &Setup, value: &Value) -> bool {
    value.as_str().is_some_and(|path| {
        Path::new(path) == setup.opencode().rules_file
            || path.ends_with(&format!("/{}", crate::harnesses::opencode::RULES_FILE))
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
pub fn plan(
    setup: &Setup,
    language: Language,
    read_only: &[String],
    develop: bool,
) -> Result<Plan, String> {
    let state = State::load_of(setup, Kind::OpenCode)?;
    let mut conflicts = Vec::new();
    let mut steps = Vec::new();
    let mut changes = Vec::new();
    let paths = &setup.opencode();

    // Parts an install under 1.x left in a file 2.x does not read: removed there (and written
    // into the file 2.x reads below).
    for (stale, text) in stale_with_ours(setup) {
        let entry = state.file(&stale);
        if let Some(new) = remove_from_config(setup, &stale, &text, entry, false)
            .map_err(|e| crate::install::in_file(&stale, e))?
        {
            let empty = only_schema(&new);
            let created_by_us =
                entry.is_some_and(|e| e.get("existed") == Some(&Value::Bool(false)));
            steps.push(Step {
                title: format!(
                    "Move our entries out of {} (opencode {} does not read it)",
                    stale.display(),
                    paths.major().label()
                ),
                lines: vec![format!("remove our parts from {}", stale.display())],
            });
            changes.push(Change {
                path: stale,
                after: (!(created_by_us && empty)).then_some(new),
                created: Vec::new(),
                separator: None,
            });
        }
    }

    // The configuration file: the MCP servers and the rules file in `instructions`.
    let path = &paths.config_file;
    let servers = paths.servers_path();
    let before = read_text(path)?;
    let mut text = match &before {
        Some(text) if !text.trim().is_empty() => text.clone(),
        _ => new_config(),
    };
    let parsed = jsonedit::value_of(&text)
        .map_err(|err| format!("{} is not valid JSON(C): {err}", path.display()))?;
    let mut created = Vec::new();
    let mut lines = Vec::new();
    let place = servers.join(".");
    for (name, profile, enabled) in [
        (USE_SERVER, "use", true),
        (DEVELOP_SERVER, "develop", develop),
    ] {
        let existing = &at(&parsed, servers)[name];
        if !existing.is_null() && !setup.ours().entry(existing) {
            conflicts.push(format!(
                "{}: {place}.{name} exists and is not ours; kept. Remove it and run install again.",
                path.display()
            ));
            lines.push(format!(
                "{name}: CONFLICT (an entry of another program has this name)"
            ));
            continue;
        }
        let mut moved = false;
        if legacy_ours(setup, &parsed, name).is_some()
            && let Some(new) = jsonedit::remove_member(&text, &["mcp"], name)
                .map_err(|e| crate::install::in_file(path, e))?
        {
            text = new;
            moved = true;
        }
        let entry = mcp_entry(setup, profile, enabled);
        let state_word = if enabled { "enabled" } else { "disabled" };
        match jsonedit::set_member(&text, servers, name, &entry)
            .map_err(|e| crate::install::in_file(path, e))?
        {
            Some((new, made)) => {
                lines.push(format!(
                    "{name} ({profile} profile): {} ({state_word})",
                    if moved {
                        format!("move from mcp.{name} (the opencode 1.x shape) to {place}.{name}")
                    } else if existing.is_null() {
                        "add".to_string()
                    } else {
                        "update".to_string()
                    }
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
            let (new, made) = jsonedit::append_element(&text, &["instructions"], &json!(rules))
                .map_err(|e| crate::install::in_file(path, e))?;
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
            "Register the MCP servers and the rules file in opencode's {} (opencode {})",
            path.display(),
            paths.major().label()
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
    let block = wrap_block(&crate::install::coexist::rule_with(
        rule_block(language),
        read_only,
    ));
    let mut separator = None;
    let (after, line) = match find_block_with(&current, language, read_only) {
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
    let wanted = crate::harnesses::opencode::plugin_text(&setup.exe, &setup.dir_args);
    let line = match read_text(path)? {
        None => {
            changes.push(Change {
                path: path.clone(),
                after: Some(wanted),
                created: Vec::new(),
                separator: None,
            });
            match paths.major() {
                Major::V1 => {
                    "create it (session start: indexing + one toast line; session end: indexing)"
                }
                Major::V2 => "create it (session start and end: indexing)",
            }
            .to_string()
        }
        Some(text) if text == wanted => "already installed".to_string(),
        Some(text)
            if crate::harnesses::opencode::is_our_plugin(&text)
                && untouched(&state, path, &text) =>
        {
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
    let paths = &setup.opencode();
    let mut out = Vec::new();
    let mut targets = vec![paths.config_file.clone()];
    if !develop_only {
        targets.extend(stale_with_ours(setup).into_iter().map(|(path, _)| path));
        targets.push(paths.rules_file.clone());
        targets.push(paths.plugin_file.clone());
    }
    for path in targets {
        let Some(bytes) = fsutil::read_optional(&path)? else {
            continue;
        };
        // An empty file holds nothing of ours (and is no JSON to parse).
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let entry = state.file(&path);
        let fp = fsutil::fingerprint(&bytes);
        let untouched = entry.is_some_and(|e| {
            e.get("written").and_then(Value::as_str) == Some(fp.as_str())
                && e.get("backup_valid") == Some(&Value::Bool(true))
        });
        // A backup that holds our parts (an install that ran twice at once) is never put back.
        let untouched = untouched && !crate::install::backup_holds_ours(entry);
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
        let config_file = path == paths.config_file || paths.stale_config_files.contains(&path);
        let new = if config_file {
            remove_from_config(setup, &path, &text, entry, develop_only)
                .map_err(|e| crate::install::in_file(&path, e))?
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
        } else if crate::harnesses::opencode::is_our_plugin(&text) {
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
            let empty = if config_file {
                only_schema(&new)
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

/// Whether a configuration text holds nothing but its `$schema`.
fn only_schema(text: &str) -> bool {
    jsonedit::value_of(text).ok().is_some_and(|v| {
        v.as_object()
            .is_some_and(|o| o.keys().all(|k| k == "$schema"))
    })
}

/// The configuration text without our MCP members (in either shape: `mcp.<name>` and
/// `mcp.servers.<name>`) and our `instructions` element (and the containers install created,
/// when they are empty then); `develop_only`: the develop server switched off instead.
fn remove_from_config(
    setup: &Setup,
    path: &Path,
    text: &str,
    entry: Option<&serde_json::Map<String, Value>>,
    develop_only: bool,
) -> Result<Option<String>, String> {
    let parsed = jsonedit::value_of(text)
        .map_err(|err| format!("{} is not valid JSON(C): {err}", path.display()))?;
    let created: Vec<&str> = entry
        .and_then(|e| e.get("created"))
        .and_then(Value::as_array)
        .map(|c| c.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut current = text.to_string();
    let mut changed = false;
    if develop_only {
        for (place, major) in [
            (&["mcp", "servers"][..], Major::V2),
            (&["mcp"][..], Major::V1),
        ] {
            let develop = &at(&parsed, place)[DEVELOP_SERVER];
            if setup.ours().entry(develop)
                && enabled(develop)
                && let Some((new, _)) = jsonedit::set_member(
                    &current,
                    place,
                    DEVELOP_SERVER,
                    &entry_for(setup, "develop", false, major),
                )?
            {
                current = new;
                changed = true;
            }
        }
        return Ok(changed.then_some(current));
    }
    for name in [USE_SERVER, DEVELOP_SERVER] {
        for place in [&["mcp", "servers"][..], &["mcp"][..]] {
            if setup.ours().entry(&at(&parsed, place)[name])
                && let Some(new) = jsonedit::remove_member(&current, place, name)
                    .map_err(|e| crate::install::in_file(path, e))?
            {
                current = new;
                changed = true;
            }
        }
    }
    if changed {
        for (parent, key) in [(&["mcp"][..], "servers"), (&[][..], "mcp")] {
            let dotted = [parent, &[key]].concat().join(".");
            if created.contains(&dotted.as_str())
                && jsonedit::is_empty_container(&current, &[parent, &[key]].concat())
                && let Some(new) = jsonedit::remove_member(&current, parent, key)
                    .map_err(|e| crate::install::in_file(path, e))?
            {
                current = new;
            }
        }
    }
    if let Some(new) = jsonedit::remove_elements(&current, &["instructions"], &|v| {
        is_our_instruction(setup, v)
    })? {
        current = new;
        changed = true;
        if created.contains(&"instructions")
            && jsonedit::is_empty_container(&current, &["instructions"])
            && let Some(new) = jsonedit::remove_member(&current, &[], "instructions")
                .map_err(|e| crate::install::in_file(path, e))?
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
    let paths = &setup.opencode();
    let mut errors = Vec::new();
    let config = read_json(&paths.config_file, &mut errors);
    let stale = stale_with_ours(setup);
    let stale_names = stale
        .iter()
        .map(|(path, _)| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let stale_values: Vec<Value> = stale
        .iter()
        .filter_map(|(_, text)| jsonedit::value_of(text).ok())
        .collect();
    let in_stale = |check: &dyn Fn(&Value) -> bool| stale_values.iter().any(check);
    let server = |name: &str, profile: &str| -> (PartState, bool) {
        let entry = &at(&config, paths.servers_path())[name];
        if entry.is_null() {
            if let Some(old) = legacy_ours(setup, &config, name) {
                return (
                    PartState::Outdated(format!(
                        "in the opencode 1.x shape (mcp.{name}); install moves it to \
                         mcp.servers.{name}"
                    )),
                    enabled(old),
                );
            }
            if in_stale(&|v| setup.ours().entry(&v["mcp"][name])) {
                return (
                    PartState::Outdated(format!(
                        "in {stale_names}, which opencode {} does not read; install moves it",
                        paths.major().label()
                    )),
                    false,
                );
            }
            (PartState::Missing, false)
        } else if !setup.ours().entry(entry) {
            (
                PartState::Conflict("another program's entry has this name".to_string()),
                false,
            )
        } else {
            let on = enabled(entry);
            if *entry == mcp_entry(setup, profile, on) {
                (PartState::Installed, on)
            } else {
                (
                    PartState::Outdated(format!(
                        "starts {}",
                        crate::harnesses::agent::entry_program(entry)
                    )),
                    on,
                )
            }
        }
    };
    let use_server = server(USE_SERVER, "use").0;
    let develop_server = server(DEVELOP_SERVER, "develop");
    let ours = |v: &Value| {
        v["instructions"]
            .as_array()
            .is_some_and(|items| items.iter().any(|i| is_our_instruction(setup, i)))
    };
    let instructions = if ours(&config) {
        PartState::Installed
    } else if in_stale(&ours) {
        PartState::Outdated(format!(
            "in {stale_names}, which opencode {} does not read; install moves it",
            paths.major().label()
        ))
    } else {
        PartState::Missing
    };
    let language = crate::config::Config::load(&setup.data_dir)
        .map(|c| c.language())
        .unwrap_or_default();
    let block = match read_text(&paths.rules_file) {
        Ok(Some(text)) => {
            match find_block_with(
                &text,
                language,
                &crate::install::coexist::read_only_names(setup),
            ) {
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
    let plugin = match read_text(&paths.plugin_file) {
        Ok(None) => PartState::Missing,
        Ok(Some(text))
            if text == crate::harnesses::opencode::plugin_text(&setup.exe, &setup.dir_args) =>
        {
            PartState::Installed
        }
        Ok(Some(text)) if crate::harnesses::opencode::is_our_plugin(&text) => {
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
    let paths = &setup.opencode();
    let mut lines = vec![
        "opencode:".to_string(),
        format!("  configuration: {}", paths.config_file.display()),
    ];
    lines.push(format!("  version: {}", paths.detected.describe()));
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
        super::DEFAULT_HEADLESS_COMMAND
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
    let paths = &setup.opencode();
    let reinstall = "run `graphersal-memory install --harness opencode`";
    let mut findings = vec![ok(format!(
        "opencode: {}; its configuration file is {}",
        paths.detected.describe(),
        paths.config_file.display()
    ))];
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
    match crate::harnesses::opencode::sessions::units(&paths.db) {
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
