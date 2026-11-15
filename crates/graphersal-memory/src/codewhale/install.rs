//! `install`, `uninstall`, `status` and `doctor` for CodeWhale: what is written into its files,
//! and nothing else (`crate::codewhale` explains why each part goes where it goes):
//!
//! - in `mcp.json`: the members `graphersal-memory` (the `use` profile) and
//!   `graphersal-memory-develop` (`"disabled": true` until `--enable-develop`) of the servers
//!   object the file uses (`servers`, or `mcpServers` when the file has that one);
//! - in `config.toml`: our `use` tools in `[tools] always_load`, so the model has them without a
//!   `tool_search` first: a managed block `[tools]` when the file has no `tools` table, else our
//!   names added to the user's list in place (its earlier value recorded, restored by
//!   uninstall); and, only when the user agreed, `[memory] enabled = true` set to `false`
//!   (recorded, restored by uninstall);
//! - the managed rule block in the global instruction file CodeWhale loads.
//!
//! No hooks (`crate::codewhale`, "Session start and end"). The state is
//! `<data dir>/install/codewhale.json`, backups as for Claude Code; `uninstall` restores an
//! untouched file byte for byte from its backup (or deletes a file install created), else
//! removes only our parts from its current text. CodeWhale edits `config.toml` itself through
//! `toml_edit`, which keeps comments, so our block's marker lines stay.

use std::path::Path;

use serde_json::{Value, json};

use crate::agents::{Agent, Kind};
use crate::install::{
    Change, DEVELOP_SERVER, Finding, PartState, Setup, State, Step, USE_SERVER, block_separator,
    find_block_in, ok, problem, read_json, read_text, wrap_block,
};
use crate::language::Language;
use crate::{fsutil, jsonedit, tomledit};

/// The rule block for CodeWhale with the English memory language rule.
pub const RULE_BLOCK: &str = "\
## Memory: graphersal-memory

Your long-term memory of this user lives in a local graph, served by the MCP server
`graphersal-memory` (its tools `mcp_graphersal-memory_memories`, `_recall`, `_memory`,
`_remember`, `_forget`, `_sessions`, `_session`; if they are not in your tool list,
`tool_search` for `graphersal-memory` first).

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
- Keep what you learn about this user in the graph, not in AGENTS.md, CodeWhale's own memory
  (`remember`, `#` notes) or other files.
- If another instruction (a CLAUDE.md, AGENTS.md or rules file, in any language) tells you to
  keep memory elsewhere (`MEMORY.md`, another tool or place), memory still lives here: tell the
  user once that the two instructions disagree and which file says it, so they can fix it.
- Past sessions of this project: `sessions(text)` finds what was decided, corrected or left
  open in them (from their summaries, when summaries are on), `session(id)` reads one, and
  `recall(text, k, \"turn\")` finds what was literally said.
";

/// The rule block for CodeWhale with the \"any language\" memory language rule.
pub const RULE_BLOCK_ANY: &str = "\
## Memory: graphersal-memory

Your long-term memory of this user lives in a local graph, served by the MCP server
`graphersal-memory` (its tools `mcp_graphersal-memory_memories`, `_recall`, `_memory`,
`_remember`, `_forget`, `_sessions`, `_session`; if they are not in your tool list,
`tool_search` for `graphersal-memory` first).

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
- Keep what you learn about this user in the graph, not in AGENTS.md, CodeWhale's own memory
  (`remember`, `#` notes) or other files.
- If another instruction (a CLAUDE.md, AGENTS.md or rules file, in any language) tells you to
  keep memory elsewhere (`MEMORY.md`, another tool or place), memory still lives here: tell the
  user once that the two instructions disagree and which file says it, so they can fix it.
- Past sessions of this project: `sessions(text)` finds what was decided, corrected or left
  open in them (from their summaries, when summaries are on), `session(id)` reads one, and
  `recall(text, k, \"turn\")` finds what was literally said.
";

/// The fingerprints of CodeWhale rule blocks earlier versions wrote (add the current one
/// whenever a text above changes).
pub const PREVIOUS_BLOCK_FINGERPRINTS: &[&str] = &[
    // 0.2.0 before the line about what to save (English, any language).
    "0bbf5987ae4fa76f",
    "0ca85f86e53c85a7",
];

/// The record of install's switch-off of the native memory in the state entry of
/// `config.toml`.
pub const MEMORY_RECORD: &str = "codewhale_memory";

/// The record of install's edit of the user's `[tools] always_load` in the state entry of
/// `config.toml`: `{"previous": [...] | null, "added": [...]}`.
pub const ALWAYS_LOAD_RECORD: &str = "codewhale_always_load";

/// The sentence the memory step shows when CodeWhale's memory is switched off.
pub const NOTHING_IS_LOST: &str = "Nothing is lost: its reviewed entries are imported into \
    graphersal-memory (its store is only read), and if you uninstall graphersal-memory later, \
    every memory is exported as readable Markdown files and CodeWhale's memory is switched back \
    on.";

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

/// [`find_block`] with the read-only line naming `read_only` in the current block
/// (`crate::coexist::rule_with`).
pub fn find_block_with(
    text: &str,
    language: Language,
    read_only: &[String],
) -> Result<Option<crate::install::FoundBlock>, String> {
    find_block_in(
        text,
        &crate::coexist::rule_with(rule_block(language), read_only),
        &[RULE_BLOCK, RULE_BLOCK_ANY],
        PREVIOUS_BLOCK_FINGERPRINTS,
    )
}

/// The MCP entry of a profile (`"disabled": true` when off).
pub fn mcp_entry(setup: &Setup, profile: &str, enabled: bool) -> Value {
    let mut args = vec![
        "mcp".to_string(),
        "--profile".to_string(),
        profile.to_string(),
    ];
    args.extend(setup.dir_args.iter().cloned());
    let mut entry = setup.codewhale_agent().mcp_entry(&setup.exe, &args);
    if !enabled {
        entry["disabled"] = json!(true);
    }
    entry
}

/// Whether an entry is switched on (`enabled` and `disabled` both honoured).
fn enabled(entry: &Value) -> bool {
    entry["enabled"] != Value::Bool(false) && entry["disabled"] != Value::Bool(true)
}

/// An entry without the switch values that change nothing (`"disabled": false`, `"enabled":
/// true`): CodeWhale's `/mcp enable` writes `"disabled": false` into an entry it switches on.
fn without_default_switches(entry: &Value) -> Value {
    let mut entry = entry.clone();
    if let Some(object) = entry.as_object_mut() {
        if object.get("disabled") == Some(&Value::Bool(false)) {
            object.remove("disabled");
        }
        if object.get("enabled") == Some(&Value::Bool(true)) {
            object.remove("enabled");
        }
    }
    entry
}

/// The body of the managed `config.toml` block (when the file has no `tools` table).
pub fn block_body() -> String {
    format!(
        "[tools]\nalways_load = {}\n",
        tomledit::strings(&crate::codewhale::tool_names())
    )
}

/// The strings of an array value.
fn strings_of(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// Whether a managed block body is ours (a `[tools]` table with only `always_load`).
fn block_is_ours(body: &str) -> bool {
    tomledit::parse(body).is_ok_and(|doc| {
        doc.as_object().is_some_and(|o| o.len() == 1)
            && doc["tools"]
                .as_object()
                .is_some_and(|t| t.len() == 1 && t.contains_key("always_load"))
    })
}

/// The planned install for CodeWhale: its steps, file changes and conflicts.
pub struct Plan {
    /// The steps for the wizard and the report.
    pub steps: Vec<Step>,
    /// The file changes.
    pub changes: Vec<Change>,
    /// What could not be done.
    pub conflicts: Vec<String>,
    /// The records of the `config.toml` state entry to set (`Some`) or remove (`None`) with
    /// its write.
    pub records: Vec<(&'static str, Option<Value>)>,
}

/// Plans CodeWhale's part of the install.
pub fn plan(
    setup: &Setup,
    language: Language,
    read_only: &[String],
    develop: bool,
    memory_off: bool,
) -> Result<Plan, String> {
    let state = State::load_of(setup, Kind::CodeWhale)?;
    let mut conflicts = Vec::new();
    let mut steps = Vec::new();
    let mut changes = Vec::new();
    let mut records = Vec::new();
    let paths = &setup.codewhale;

    // mcp.json: the servers.
    let path = &paths.mcp_file;
    let before = read_text(path)?;
    let mut text = crate::install::json_start(&before, "{\n}\n");
    let parsed = jsonedit::value_of(&text)
        .map_err(|err| format!("{} is not valid JSON: {err}", path.display()))?;
    let mut lines = Vec::new();
    let mut created = Vec::new();
    let key = crate::codewhale::servers_key_of(path);
    if parsed.get("servers").is_some() && parsed.get("mcpServers").is_some() {
        conflicts.push(format!(
            "{}: has both `servers` and `mcpServers`, which CodeWhale cannot read; kept. Keep \
             one of the two and run install again.",
            path.display()
        ));
        lines.push("CONFLICT (see the report)".to_string());
    } else {
        for (name, profile, on) in [
            (USE_SERVER, "use", true),
            (DEVELOP_SERVER, "develop", develop),
        ] {
            let existing = &parsed[key][name];
            if !existing.is_null() && !setup.ours().entry(existing) {
                conflicts.push(format!(
                    "{}: {key}.{name} exists and is not ours; kept. Remove it and run install \
                     again.",
                    path.display()
                ));
                lines.push(format!(
                    "{name}: CONFLICT (an entry of another program has this name)"
                ));
                continue;
            }
            let word = if on { "enabled" } else { "disabled" };
            let wanted = mcp_entry(setup, profile, on);
            // `/mcp enable` leaves `"disabled": false` in an entry: the same entry, kept.
            if !existing.is_null() && without_default_switches(existing) == wanted {
                lines.push(format!(
                    "{name} ({profile} profile): already registered ({word})"
                ));
                continue;
            }
            match jsonedit::set_member(&text, &[key], name, &wanted)
                .map_err(|e| crate::install::in_file(path, e))?
            {
                Some((new, made)) => {
                    lines.push(format!(
                        "{name} ({profile} profile): {} ({word})",
                        if existing.is_null() { "add" } else { "update" }
                    ));
                    text = new;
                    created.extend(made);
                }
                None => lines.push(format!(
                    "{name} ({profile} profile): already registered ({word})"
                )),
            }
        }
        if !develop {
            lines.push(format!(
                "{DEVELOP_SERVER} stays off; `graphersal-memory install --harness codewhale \
                 --enable-develop` switches it on"
            ));
        }
    }
    steps.push(Step {
        title: format!(
            "Register the MCP servers in CodeWhale's {} (key {key})",
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

    // config.toml: [tools] always_load, and the memory switch.
    let path = &paths.config_file;
    let before = read_text(path)?;
    let mut text = before.clone().unwrap_or_default();
    let doc = tomledit::parse(&text)
        .map_err(|err| format!("{} is not valid TOML: {err}", path.display()))?;
    let mut lines = Vec::new();
    let names = crate::codewhale::tool_names();
    let block = tomledit::find(&text).map_err(|err| format!("{}: {err}", path.display()))?;
    let recorded = state.file(path).and_then(|e| e.get(ALWAYS_LOAD_RECORD));
    match &block {
        Some(found) if !found.intact => {
            conflicts.push(format!(
                "{}: the graphersal-memory block was edited by hand; kept as it is. Move your \
                 changes out of the block (or delete the block) and run install again.",
                path.display()
            ));
            lines.push("[tools] always_load: CONFLICT (see the report)".to_string());
        }
        Some(found) => {
            let body = block_body();
            if found.body == body {
                lines.push("[tools] always_load: already lists the memory tools".to_string());
            } else {
                text = tomledit::put_block(&text, &body)
                    .map_err(|err| format!("{}: {err}", path.display()))?;
                lines.push("[tools] always_load: update the memory tools".to_string());
            }
        }
        None if doc["tools"].is_null() => {
            text = tomledit::put_block(&text, &block_body())
                .map_err(|err| format!("{}: {err}", path.display()))?;
            lines.push(format!(
                "[tools] always_load: add the {} memory tools (in a managed block), so the \
                 model has them without a tool_search first",
                names.len()
            ));
        }
        None => {
            let current = strings_of(&doc["tools"]["always_load"]);
            let missing: Vec<String> = names
                .iter()
                .filter(|n| !current.contains(n))
                .cloned()
                .collect();
            if missing.is_empty() {
                lines.push("[tools] always_load: already lists the memory tools".to_string());
            } else {
                let mut list = current.clone();
                list.extend(missing.iter().cloned());
                match tomledit::set_string_array(&text, "tools", "always_load", &list)
                    .map_err(|e| crate::install::in_file(path, e))?
                {
                    Some(new) => {
                        text = new;
                        // The first install's record stays (what was there before us).
                        let previous = match recorded {
                            Some(record) => record["previous"].clone(),
                            None if doc["tools"]["always_load"].is_null() => Value::Null,
                            None => json!(current),
                        };
                        let mut added: Vec<String> = recorded
                            .map(|r| strings_of(&r["added"]))
                            .unwrap_or_default();
                        for name in &missing {
                            if !added.contains(name) {
                                added.push(name.clone());
                            }
                        }
                        records.push((
                            ALWAYS_LOAD_RECORD,
                            Some(json!({"previous": previous, "added": added})),
                        ));
                        lines.push(format!(
                            "[tools] always_load: add the {} memory tools to your list \
                             (uninstall takes them out again)",
                            missing.len()
                        ));
                    }
                    None => lines.push(format!(
                        "[tools] always_load: your `tools` table is not written as a [tools] \
                         section, so it is not edited; add these names yourself to load the \
                         memory tools up front: {}",
                        missing.join(", ")
                    )),
                }
            }
        }
    }
    // The memory switch.
    let memory_recorded = state
        .file(path)
        .and_then(|entry| entry.get(MEMORY_RECORD))
        .is_some();
    let on = crate::codewhale::memory_on(&doc, paths.memory_env_on);
    if on {
        let plain =
            doc["memory"]["enabled"] == Value::Bool(true) && doc["memory"]["backend"].is_null();
        if !memory_off {
            lines.push("CodeWhale's memory: on (kept)".to_string());
        } else if plain {
            match tomledit::set_bool(&text, "memory", "enabled", false)
                .map_err(|e| crate::install::in_file(path, e))?
            {
                Some(new) => {
                    text = new;
                    records.push((MEMORY_RECORD, Some(json!({"previous": true}))));
                    lines.push(format!(
                        "[memory] enabled: false (switch CodeWhale's memory off). {NOTHING_IS_LOST}"
                    ));
                }
                None => lines.push(
                    "CodeWhale's memory: on, and not written as a plain `enabled = true` line; \
                     switch it off yourself if you want"
                        .to_string(),
                ),
            }
        } else {
            lines.push(
                "CodeWhale's memory: on through `[memory] backend` or CODEWHALE_MEMORY / \
                 DEEPSEEK_MEMORY; switch it off there yourself if you want (backend = \"off\")"
                    .to_string(),
            );
        }
    } else if memory_recorded {
        if memory_off {
            lines.push("CodeWhale's memory: off (switched off by install)".to_string());
        } else {
            if doc["memory"]["enabled"] == Value::Bool(false)
                && let Some(new) = tomledit::set_bool(&text, "memory", "enabled", true)
                    .map_err(|e| crate::install::in_file(path, e))?
            {
                text = new;
            }
            records.push((MEMORY_RECORD, None));
            lines.push("CodeWhale's memory: back to on, as before install".to_string());
        }
    }
    steps.push(Step {
        title: format!("Load the memory tools up front in {}", path.display()),
        lines,
    });
    if Some(&text) != before.as_ref() && !(before.is_none() && text.is_empty()) {
        changes.push(Change {
            path: path.clone(),
            after: Some(text),
            created: Vec::new(),
            separator: None,
        });
    } else {
        records.clear();
    }

    // The global instruction file: the managed rule block.
    let path = &paths.instruction_file;
    let before = read_text(path)?;
    let current = before.clone().unwrap_or_default();
    let block = crate::install::line_ends_like(
        &current,
        &wrap_block(&crate::coexist::rule_with(rule_block(language), read_only)),
    );
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
                    "append the block (your own text stays as it is)".to_string()
                } else {
                    "create it with the block".to_string()
                },
            )
        }
    };
    steps.push(Step {
        title: format!("Write the managed rule block into {}", path.display()),
        lines: vec![
            line,
            "CodeWhale loads it before the project's AGENTS.md files; all of them share 48 KiB, \
             and the global file is dropped first when a project's files fill it (`doctor` \
             warns)"
                .to_string(),
        ],
    });
    if let Some(after) = after {
        changes.push(Change {
            path: path.clone(),
            after: Some(after),
            created: Vec::new(),
            separator,
        });
    }
    Ok(Plan {
        steps,
        changes,
        conflicts,
        records,
    })
}

/// The removal of our parts from CodeWhale's files: one `(change, what)` per file. With
/// `develop_only`, only the develop server is switched off (`"disabled": true`).
pub fn removal(
    setup: &Setup,
    state: &State,
    develop_only: bool,
    conflicts: &mut Vec<String>,
) -> Result<Vec<(Change, &'static str)>, String> {
    let paths = &setup.codewhale;
    let mut out = Vec::new();
    let mut targets = vec![paths.mcp_file.clone()];
    if !develop_only {
        targets.push(paths.config_file.clone());
        targets.push(paths.instruction_file.clone());
        // A block written while another global file was the one CodeWhale loads.
        for other in crate::codewhale::instruction_candidates(&paths.home) {
            if !targets.contains(&other) && state.file(&other).is_some() {
                targets.push(other);
            }
        }
    }
    for path in targets {
        let Some(bytes) = fsutil::read_optional(&path)? else {
            continue;
        };
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let entry = state.file(&path);
        let fp = fsutil::fingerprint(&bytes);
        let untouched = entry.is_some_and(|e| {
            e.get("written").and_then(Value::as_str) == Some(fp.as_str())
                && e.get("backup_valid") == Some(&Value::Bool(true))
        });
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
        let (new, empty) = if path == paths.mcp_file {
            let new = remove_servers(setup, &text, entry, develop_only)
                .map_err(|e| crate::install::in_file(&path, e))?;
            let empty = new.as_deref().is_some_and(|t| {
                jsonedit::value_of(t)
                    .ok()
                    .is_some_and(|v| v.as_object().is_some_and(|o| o.is_empty()))
            });
            (new, empty)
        } else if path == paths.config_file {
            let new = remove_from_config(entry, &path, &text, conflicts)
                .map_err(|e| crate::install::in_file(&path, e))?;
            let empty = new.as_deref().is_some_and(|t| t.trim().is_empty());
            (new, empty)
        } else {
            let new = match find_block(&text, Language::English) {
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
            };
            let empty = new.as_deref().is_some_and(|t| t.trim().is_empty());
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

/// `mcp.json` without our servers (and the servers object install created, when empty then);
/// `develop_only`: with the develop server switched off instead.
fn remove_servers(
    setup: &Setup,
    text: &str,
    entry: Option<&serde_json::Map<String, Value>>,
    develop_only: bool,
) -> Result<Option<String>, String> {
    let parsed = jsonedit::value_of(text).map_err(|err| format!("not valid JSON: {err}"))?;
    let mut current = text.to_string();
    let mut changed = false;
    for key in ["servers", "mcpServers"] {
        if develop_only {
            let develop = &parsed[key][DEVELOP_SERVER];
            if setup.ours().entry(develop)
                && enabled(develop)
                && let Some((new, _)) = jsonedit::set_member(
                    &current,
                    &[key],
                    DEVELOP_SERVER,
                    &mcp_entry(setup, "develop", false),
                )?
            {
                current = new;
                changed = true;
            }
            continue;
        }
        for name in [USE_SERVER, DEVELOP_SERVER] {
            if setup.ours().entry(&parsed[key][name])
                && let Some(new) = jsonedit::remove_member(&current, &[key], name)?
            {
                current = new;
                changed = true;
            }
        }
        let created = entry
            .and_then(|e| e.get("created"))
            .and_then(Value::as_array)
            .is_some_and(|c| c.iter().any(|item| item == key));
        if changed
            && created
            && jsonedit::is_empty_container(&current, &[key])
            && let Some(new) = jsonedit::remove_member(&current, &[], key)?
        {
            current = new;
        }
    }
    Ok(changed.then_some(current))
}

/// `config.toml` without our block, without our names in the user's `always_load` (the key
/// removed when install added it and nothing else is left), and with the native memory back on
/// when install switched it off.
fn remove_from_config(
    entry: Option<&serde_json::Map<String, Value>>,
    path: &Path,
    text: &str,
    conflicts: &mut Vec<String>,
) -> Result<Option<String>, String> {
    let doc = tomledit::parse(text)
        .map_err(|err| format!("{} is not valid TOML: {err}", path.display()))?;
    let mut current = text.to_string();
    let mut changed = false;
    match tomledit::find(text).map_err(|err| format!("{}: {err}", path.display()))? {
        Some(found) if found.intact || block_is_ours(&found.body) => {
            if let Some(new) = tomledit::remove_block(&current)? {
                current = new;
                changed = true;
            }
        }
        Some(_) => conflicts.push(format!(
            "{}: the graphersal-memory block was edited by hand, so it was not removed; delete \
             it yourself",
            path.display()
        )),
        None => {}
    }
    if let Some(record) = entry.and_then(|e| e.get(ALWAYS_LOAD_RECORD)) {
        let added = strings_of(&record["added"]);
        let list = strings_of(&doc["tools"]["always_load"]);
        let rest: Vec<String> = list
            .iter()
            .filter(|n| !added.contains(n))
            .cloned()
            .collect();
        if rest.len() != list.len() {
            let new = if rest.is_empty() && record["previous"].is_null() {
                tomledit::remove_array_key(&current, "tools", "always_load")?
            } else {
                tomledit::set_string_array(&current, "tools", "always_load", &rest)?
            };
            match new {
                Some(new) => {
                    current = new;
                    changed = true;
                }
                None => conflicts.push(format!(
                    "{}: could not take the memory tools out of [tools] always_load; remove \
                     them yourself: {}",
                    path.display(),
                    added.join(", ")
                )),
            }
        }
    }
    if entry.and_then(|e| e.get(MEMORY_RECORD)).is_some()
        && doc["memory"]["enabled"] == Value::Bool(false)
        && let Some(new) = tomledit::set_bool(&current, "memory", "enabled", true)
            .map_err(|e| crate::install::in_file(path, e))?
    {
        current = new;
        changed = true;
    }
    Ok(changed.then_some(current))
}

/// What is installed in CodeWhale.
#[derive(Debug, Clone)]
pub struct Inspection {
    /// The `use` server.
    pub use_server: PartState,
    /// The `develop` server, and whether it is enabled.
    pub develop_server: (PartState, bool),
    /// Our tools in `[tools] always_load`.
    pub always_load: PartState,
    /// The managed rule block.
    pub block: PartState,
    /// Whether CodeWhale's memory is on, and whether install switched it off.
    pub memory: (bool, bool),
    /// Files that could not be read or parsed.
    pub errors: Vec<String>,
}

impl Inspection {
    /// Whether nothing of ours is installed.
    pub fn nothing_installed(&self) -> bool {
        self.use_server == PartState::Missing
            && self.develop_server.0 == PartState::Missing
            && self.always_load == PartState::Missing
            && self.block == PartState::Missing
    }
}

/// Inspects CodeWhale's files.
pub fn inspect(setup: &Setup) -> Inspection {
    let paths = &setup.codewhale;
    let mut errors = Vec::new();
    let mcp = read_json(&paths.mcp_file, &mut errors);
    let key = crate::codewhale::servers_key_of(&paths.mcp_file);
    let server = |name: &str, profile: &str| -> (PartState, bool) {
        let entry = &mcp[key][name];
        if entry.is_null() {
            (PartState::Missing, false)
        } else if !setup.ours().entry(entry) {
            (
                PartState::Conflict("another program's entry has this name".to_string()),
                false,
            )
        } else {
            let on = enabled(entry);
            if without_default_switches(entry) == mcp_entry(setup, profile, on) {
                (PartState::Installed, on)
            } else {
                (
                    PartState::Outdated(format!(
                        "starts {}",
                        entry["command"].as_str().unwrap_or_default()
                    )),
                    on,
                )
            }
        }
    };
    let config = match read_text(&paths.config_file) {
        Ok(Some(text)) => tomledit::parse(&text).unwrap_or_else(|err| {
            errors.push(format!(
                "{} is not valid TOML: {err}",
                paths.config_file.display()
            ));
            Value::Null
        }),
        Ok(None) => Value::Null,
        Err(err) => {
            errors.push(err);
            Value::Null
        }
    };
    let listed = strings_of(&config["tools"]["always_load"]);
    let names = crate::codewhale::tool_names();
    let always_load = match names.iter().filter(|n| listed.contains(n)).count() {
        0 => PartState::Missing,
        n if n == names.len() => PartState::Installed,
        _ => PartState::Outdated("some memory tools are missing from the list".to_string()),
    };
    let language = crate::config::Config::load(&setup.data_dir)
        .map(|c| c.language())
        .unwrap_or_default();
    let block = match read_text(&paths.instruction_file) {
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
    let recorded = State::load_of(setup, Kind::CodeWhale)
        .ok()
        .and_then(|state| {
            state
                .file(&paths.config_file)
                .map(|e| e.contains_key(MEMORY_RECORD))
        })
        .unwrap_or(false);
    Inspection {
        use_server: server(USE_SERVER, "use").0,
        develop_server: server(DEVELOP_SERVER, "develop"),
        always_load,
        block,
        memory: (
            crate::codewhale::memory_on(&config, paths.memory_env_on),
            recorded,
        ),
        errors,
    }
}

/// The bytes of the instruction files CodeWhale loads for the project rooted at `root` (its
/// `AGENTS.md` files from the repository root down to `root`; the global file not counted).
pub fn project_instruction_bytes(root: &Path) -> u64 {
    let mut total = 0;
    let mut dir = Some(root);
    while let Some(at) = dir {
        for name in ["AGENTS.md", "WHALE.md"] {
            if let Ok(meta) = std::fs::metadata(at.join(name)) {
                total += meta.len();
            }
        }
        if at.join(".git").exists() {
            break;
        }
        dir = at.parent();
    }
    total
}

/// The CodeWhale part of `status`.
pub fn status_lines(setup: &Setup) -> Vec<String> {
    let inspection = inspect(setup);
    let paths = &setup.codewhale;
    let mut lines = vec![
        "CodeWhale:".to_string(),
        format!("  home: {}", paths.codewhale_home.display()),
    ];
    if let Some(program) = setup.program(Kind::CodeWhale) {
        lines.push(format!(
            "  program: {}{}",
            program.display(),
            crate::codewhale::version_of(&program)
                .map(|v| format!(" ({v})"))
                .unwrap_or_default()
        ));
    }
    lines.push(format!(
        "  MCP server {USE_SERVER} (use, {}): {}",
        paths.mcp_file.display(),
        inspection.use_server.describe()
    ));
    lines.push(format!(
        "  MCP server {DEVELOP_SERVER}: {}",
        match &inspection.develop_server {
            (PartState::Missing, _) => "not registered".to_string(),
            (PartState::Installed, false) =>
                "registered, disabled (`graphersal-memory install --harness codewhale \
                 --enable-develop` enables it)"
                    .to_string(),
            (PartState::Installed, true) => "registered, enabled".to_string(),
            (other, _) => other.describe(),
        }
    ));
    lines.push(format!(
        "  memory tools loaded up front ([tools] always_load, {}): {}",
        paths.config_file.display(),
        inspection.always_load.describe()
    ));
    lines.push(format!(
        "  rule block ({}): {}",
        paths.instruction_file.display(),
        inspection.block.describe()
    ));
    lines.push(format!(
        "  CodeWhale's memory: {}",
        match inspection.memory {
            (true, _) => "on",
            (false, true) => "off (switched off by install; uninstall switches it on again)",
            (false, false) => "off",
        }
    ));
    lines.push(format!(
        "  headless command (summaries): `{}` by default; the bench through CodeWhale is planned",
        crate::codewhale::DEFAULT_HEADLESS_COMMAND
    ));
    lines.push(format!(
        "  sessions: {} in {}",
        crate::codewhale::sessions::scan(&paths.sessions).len(),
        paths.sessions.display()
    ));
    lines.push(
        "  session start and end: through the MCP connection (no hooks); a session is indexed \
         when CodeWhale exits; CodeWhale asks once per memory tool per run (no permanent \
         approval for MCP tools)"
            .to_string(),
    );
    for error in &inspection.errors {
        lines.push(format!("  problem: {error}"));
    }
    if inspection.nothing_installed() {
        lines.push(
            "  Not installed: run `graphersal-memory install --harness codewhale`.".to_string(),
        );
    }
    lines
}

/// `doctor`'s checks of the CodeWhale installation; `project` the current project's root (its
/// instruction files are measured against CodeWhale's budget).
pub fn doctor(setup: &Setup, project: Option<&Path>) -> Vec<Finding> {
    let inspection = inspect(setup);
    let paths = &setup.codewhale;
    let reinstall = "run `graphersal-memory install --harness codewhale`";
    let mut findings = vec![ok(format!(
        "CodeWhale: home {}, MCP servers {}",
        paths.codewhale_home.display(),
        paths.mcp_file.display()
    ))];
    for error in &inspection.errors {
        findings.push(problem(
            format!("CodeWhale: {error}"),
            "fix the file by hand (CodeWhale cannot read it either), then run install again",
        ));
    }
    if std::fs::symlink_metadata(&paths.mcp_file).is_ok_and(|m| m.file_type().is_symlink()) {
        findings.push(problem(
            format!(
                "CodeWhale: {} is a symbolic link, which CodeWhale refuses to read",
                paths.mcp_file.display()
            ),
            "replace the link by the file it points to",
        ));
    }
    let check =
        |findings: &mut Vec<Finding>, what: String, state: &PartState, fix: &str| match state {
            PartState::Installed => findings.push(ok(format!("CodeWhale: {what}: installed"))),
            other => findings.push(problem(
                format!("CodeWhale: {what}: {}", other.describe()),
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
        (PartState::Installed, on) => findings.push(ok(format!(
            "CodeWhale: MCP server {DEVELOP_SERVER}: {}",
            if *on { "on" } else { "off (disabled)" }
        ))),
        (state, _) => check(
            &mut findings,
            format!("MCP server {DEVELOP_SERVER}"),
            state,
            reinstall,
        ),
    }
    match &inspection.always_load {
        PartState::Installed => findings.push(ok(
            "CodeWhale: the memory tools are loaded up front ([tools] always_load)",
        )),
        other => findings.push(Finding {
            problem: false,
            text: format!(
                "CodeWhale: the memory tools are not all in [tools] always_load ({}): the model \
                 has to tool_search them first",
                other.describe()
            ),
            fix: Some(reinstall.to_string()),
        }),
    }
    check(
        &mut findings,
        format!("rule block in {}", paths.instruction_file.display()),
        &inspection.block,
        reinstall,
    );
    if let Some(root) = project {
        let bytes = project_instruction_bytes(root);
        let global = std::fs::metadata(&paths.instruction_file).map_or(0, |m| m.len());
        if bytes + global > crate::codewhale::INSTRUCTION_BUDGET {
            findings.push(Finding {
                problem: false,
                text: format!(
                    "CodeWhale: the AGENTS.md files of {} ({bytes} bytes) and the global file \
                     ({global} bytes) are over CodeWhale's 48 KiB budget: it drops the global \
                     file first, and the rule block with it",
                    root.display()
                ),
                fix: Some(
                    "shorten the project's AGENTS.md files; the memory tools stay loaded \
                     through [tools] always_load either way"
                        .to_string(),
                ),
            });
        }
    }
    findings.push(ok(format!(
        "CodeWhale: {} saved session(s) in {}",
        crate::codewhale::sessions::scan(&paths.sessions).len(),
        paths.sessions.display()
    )));
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codewhales_own_switch_values_are_no_change() {
        let entry = json!({"command": "x", "disabled": false, "enabled": true});
        assert_eq!(without_default_switches(&entry), json!({"command": "x"}));
        let off = json!({"command": "x", "disabled": true});
        assert_eq!(without_default_switches(&off), off);
        assert!(!enabled(&off));
    }

    #[test]
    fn the_block_body_lists_every_use_tool() {
        let doc = tomledit::parse(&block_body()).unwrap();
        assert_eq!(
            strings_of(&doc["tools"]["always_load"]),
            crate::codewhale::tool_names()
        );
        assert!(block_is_ours(&block_body()));
        assert!(!block_is_ours("[tools]\nalways_load = []\nx = 1\n"));
    }

    #[test]
    fn the_rule_blocks_name_codewhales_tool_names() {
        for block in [RULE_BLOCK, RULE_BLOCK_ANY] {
            assert!(block.contains("mcp_graphersal-memory_memories"));
            assert!(block.contains("tool_search"));
        }
        let text = wrap_block(RULE_BLOCK);
        let found = find_block(&text, Language::English).unwrap().unwrap();
        assert!(found.intact && found.current);
    }

    #[test]
    fn project_instructions_are_measured_up_to_the_repository_root() {
        let tmp = crate::mcp_tests::TempDir::new("cw-budget");
        let repo = tmp.path().join("repo");
        let sub = repo.join("crates").join("a");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join("AGENTS.md"), "x".repeat(100)).unwrap();
        std::fs::write(sub.join("AGENTS.md"), "y".repeat(10)).unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "outside the repository").unwrap();
        assert_eq!(project_instruction_bytes(&sub), 110);
    }
}
