//! `install`, `uninstall`, `status` and `doctor` for Codex: what is written into the Codex home,
//! and nothing else (`crate::codex` explains why each part goes where it goes):
//!
//! - in `config.toml`: one managed block (`crate::tomledit`) with the tables
//!   `[mcp_servers.graphersal-memory]` (the `use` profile) and
//!   `[mcp_servers.graphersal-memory-develop]` (`enabled = false` until `--enable-develop`);
//!   and, only when the user agreed, `memories = true` of `[features]` set to `false` in place
//!   (its earlier value recorded, restored by uninstall);
//! - in `hooks.json`: one group in `hooks.SessionStart` and one in `hooks.SessionEnd`, each
//!   with the single command `"<this binary>" hook session-start|session-end ... --harness
//!   codex`;
//! - the managed rule block in the global instruction file (`AGENTS.md`, or
//!   `AGENTS.override.md` when that is the one Codex loads).
//!
//! The state is `<data dir>/install/codex.json`, backups as for Claude Code; `uninstall`
//! restores an untouched file byte for byte from its backup (or deletes a file install
//! created), else removes only our parts from its current text. Codex writes into
//! `config.toml` itself (project trust, the trust of our hooks in `hooks.state`), so after a
//! session the file is usually no longer untouched: then our block (and the memories value) is
//! taken out and Codex's own additions stay.

use std::path::Path;

use serde_json::{Value, json};

use crate::agents::{Harness, Kind};
use crate::install::{
    Change, DEVELOP_SERVER, Finding, PartState, Setup, State, Step, USE_SERVER, block_separator,
    find_block_in, ok, problem, read_json, read_text, wrap_block,
};
use crate::language::Language;
use crate::{fsutil, jsonedit, tomledit};

/// The rule block for Codex with the English memory language rule.
pub const RULE_BLOCK: &str = "\
## Memory: graphersal-memory

Your long-term memory of this user lives in a local graph, served by the MCP server
`graphersal-memory` (tools named `mcp__graphersal_memory__<function>`).

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

/// The rule block for Codex with the \"any language\" memory language rule.
pub const RULE_BLOCK_ANY: &str = "\
## Memory: graphersal-memory

Your long-term memory of this user lives in a local graph, served by the MCP server
`graphersal-memory` (tools named `mcp__graphersal_memory__<function>`).

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

/// The fingerprints of Codex rule blocks earlier versions wrote (add the current one
/// whenever a text above changes).
pub const PREVIOUS_BLOCK_FINGERPRINTS: &[&str] = &[
    // 0.1.0 before the line about instructions that keep memory elsewhere (English, any
    // language).
    "1bc9f6cc3e931923",
    "b6febc586b5062d9",
    // 0.1.0 and 0.2.0 before the line about what to save (English, any language).
    "f29c2e9b33d2575b",
    "43abd78a40c97e71",
];

/// The record of install's switch-off of Codex's memories in the state entry of `config.toml`.
pub const MEMORIES_RECORD: &str = "codex_memories";

/// The sentence the memories step shows when Codex's memories are switched off.
pub const NOTHING_IS_LOST: &str = "Nothing is lost: if you uninstall graphersal-memory later, \
    every memory, including new ones, is exported as readable Markdown files and Codex's \
    memories are switched back on.";

/// How to approve our hooks in Codex (user decision 2026-11-10: the trust stays with the user,
/// and every place that mentions the hooks says so explicitly).
pub const HOOK_APPROVAL_HOW: &str = "open Codex, type `/hooks` and trust both \
    graphersal-memory hooks (SessionStart and SessionEnd)";

/// What happens without the approval.
pub const HOOK_APPROVAL_WITHOUT: &str = "without it the memory still works, but past sessions \
    are indexed only after 30 minutes idle instead of at session end";

/// The visible line about the approval, for the wizard, the report, `status` and `doctor`.
pub fn hook_approval_line() -> String {
    format!(
        "ACTION NEEDED: Codex needs your approval for the two graphersal-memory hooks: \
         {HOOK_APPROVAL_HOW}; {HOOK_APPROVAL_WITHOUT}."
    )
}

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

/// The MCP entry of a profile as Codex reads it back (`enabled` only when false).
pub fn mcp_entry(setup: &Setup, profile: &str, enabled: bool) -> Value {
    let mut args = vec![
        "mcp".to_string(),
        "--profile".to_string(),
        profile.to_string(),
    ];
    args.extend(setup.dir_args.iter().cloned());
    let mut entry = setup.codex_agent().mcp_entry(&setup.exe, &args);
    if !enabled {
        entry["enabled"] = json!(false);
    }
    entry
}

/// The managed block's tables for the two servers.
pub fn block_body(setup: &Setup, develop: bool) -> String {
    let table = |name: &str, profile: &str, enabled: bool| {
        let entry = mcp_entry(setup, profile, enabled);
        let args: Vec<String> = entry["args"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        let mut text = format!(
            "[mcp_servers.{name}]\ncommand = {}\nargs = {}\nstartup_timeout_sec = {}\n",
            tomledit::string(entry["command"].as_str().unwrap_or_default()),
            tomledit::strings(&args),
            crate::codex::STARTUP_TIMEOUT_SECS
        );
        if !enabled {
            text.push_str("enabled = false\n");
        }
        text
    };
    format!(
        "{}\n{}",
        table(USE_SERVER, "use", true),
        table(DEVELOP_SERVER, "develop", develop)
    )
}

/// The hook command of an event (`session-start`, `session-end`).
pub fn hook_command(setup: &Setup, event: &str) -> String {
    format!("{} --harness codex", setup.hook_command(event))
}

/// The hook group of an event, as it goes into `hooks.json`.
pub fn hook_group(setup: &Setup, event: &str) -> Value {
    let timeout = if event == "session-end" {
        crate::codex::END_HOOK_TIMEOUT_SECS
    } else {
        crate::codex::START_HOOK_TIMEOUT_SECS
    };
    json!({"hooks": [{"type": "command", "command": hook_command(setup, event),
                      "timeout": timeout}]})
}

/// The keys of our server entries that belong to the user: tool approvals Codex writes when the
/// user answers "always allow" (`tools.<tool>.approval_mode`, `codex-rs/core/src/
/// mcp_tool_call.rs`, `persist_custom_mcp_tool_approval_with`) or sets by hand.
const USER_KEYS: [&str; 2] = ["tools", "default_tools_approval_mode"];

/// An entry without the user's keys ([`USER_KEYS`]).
pub fn without_user_keys(entry: &Value) -> Value {
    let mut entry = entry.clone();
    if let Some(map) = entry.as_object_mut() {
        for key in USER_KEYS {
            map.remove(key);
        }
    }
    entry
}

/// Whether a managed block's text, the user's keys aside, holds exactly our two servers (with
/// the develop server in either state): a block Codex only added tool approvals to.
fn block_is_ours(setup: &Setup, body: &str) -> bool {
    let Ok(doc) = tomledit::parse(body) else {
        return false;
    };
    let servers = doc["mcp_servers"].as_object().cloned().unwrap_or_default();
    let develop = without_user_keys(&doc["mcp_servers"][DEVELOP_SERVER]);
    servers.len() == 2
        && without_user_keys(&doc["mcp_servers"][USE_SERVER]) == mcp_entry(setup, "use", true)
        && (develop == mcp_entry(setup, "develop", true)
            || develop == mcp_entry(setup, "develop", false))
        && doc.as_object().is_some_and(|o| o.len() == 1)
}

/// `text` without the tables of our servers' user keys written outside the block
/// (`[mcp_servers.graphersal-memory.tools.remember]`): left behind they would name a server
/// without a command.
fn remove_user_tables(text: &str) -> String {
    let mut current = text.to_string();
    for header in tomledit::headers(text) {
        let ours = [USE_SERVER, DEVELOP_SERVER]
            .iter()
            .any(|name| header.starts_with(&format!("mcp_servers.{name}.")));
        if ours && let Some(new) = tomledit::remove_table(&current, &header) {
            current = new;
        }
    }
    current
}

/// Whether an entry is switched on.
fn enabled(entry: &Value) -> bool {
    entry["enabled"] != false
}

/// The planned install for Codex: its steps, file changes and conflicts.
pub struct Plan {
    /// The steps for the wizard and the report.
    pub steps: Vec<Step>,
    /// The file changes.
    pub changes: Vec<Change>,
    /// What could not be done.
    pub conflicts: Vec<String>,
    /// What happens to Codex's memories: `Some(true)` switched off now, `Some(false)` install's
    /// switch-off undone, `None` nothing.
    pub memories: Option<bool>,
}

/// Whether Codex's own memories are on in `doc` (`[features] memories = true`).
pub fn memories_on(doc: &Value) -> bool {
    doc["features"]["memories"] == true
}

/// Plans Codex's part of the install.
pub fn plan(
    setup: &Setup,
    language: Language,
    read_only: &[String],
    develop: bool,
    memories_off: bool,
) -> Result<Plan, String> {
    let state = State::load_of(setup, Kind::Codex)?;
    let mut conflicts = Vec::new();
    let mut steps = Vec::new();
    let mut changes = Vec::new();
    let paths = &setup.codex;

    // config.toml: the managed block of the two servers, and the memories switch.
    let path = &paths.config_file;
    let before = read_text(path)?;
    let mut text = before.clone().unwrap_or_default();
    let doc = tomledit::parse(&text)
        .map_err(|err| format!("{} is not valid TOML: {err}", path.display()))?;
    let mut lines = Vec::new();
    let block = tomledit::find(&text).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut blocked = false;
    match &block {
        Some(found)
            if !found.intact
                && block_is_ours(setup, &found.body)
                && without_user_keys(&doc["mcp_servers"][DEVELOP_SERVER])
                    == mcp_entry(setup, "develop", develop) =>
        {
            // Codex added the user's tool approvals inside the block: kept as it is.
            lines.push(
                "graphersal-memory, graphersal-memory-develop: already registered (the block \
                 holds your tool approvals; kept)"
                    .to_string(),
            );
            blocked = true;
        }
        Some(found) if !found.intact => {
            conflicts.push(format!(
                "{}: the graphersal-memory block was edited by hand; kept as it is. Move your \
                 changes out of the block (or delete the block) and run install again.",
                path.display()
            ));
            lines.push("CONFLICT (see the report)".to_string());
            blocked = true;
        }
        _ => {}
    }
    let in_block = |name: &str| {
        block.as_ref().is_some_and(|found| {
            tomledit::parse(&found.body).is_ok_and(|b| !b["mcp_servers"][name].is_null())
        })
    };
    for name in [USE_SERVER, DEVELOP_SERVER] {
        let existing = &doc["mcp_servers"][name];
        if existing.is_null() || in_block(name) {
            continue;
        }
        if setup.ours().entry(existing) {
            // Ours, outside the block (its marker comments were lost): taken out, written anew.
            if let Some(new) = tomledit::remove_table(&text, &format!("mcp_servers.{name}")) {
                text = new;
            }
        } else {
            conflicts.push(format!(
                "{}: [mcp_servers.{name}] exists and is not ours; kept. Remove it and run \
                 install again.",
                path.display()
            ));
            lines.push(format!(
                "{name}: CONFLICT (an entry of another program has this name)"
            ));
            blocked = true;
        }
    }
    if !blocked {
        let body = block_body(setup, develop);
        let new = tomledit::put_block(&text, &body)
            .map_err(|err| format!("{}: {err}", path.display()))?;
        let unchanged = block.as_ref().is_some_and(|found| found.body == body) && new == text;
        for (name, profile, on) in [
            (USE_SERVER, "use", true),
            (DEVELOP_SERVER, "develop", develop),
        ] {
            let word = if on { "enabled" } else { "disabled" };
            let existing = &doc["mcp_servers"][name];
            lines.push(format!(
                "{name} ({profile} profile): {} ({word})",
                if existing.is_null() {
                    "add"
                } else if without_user_keys(existing) == mcp_entry(setup, profile, on) {
                    "already registered"
                } else {
                    "update"
                }
            ));
        }
        if !unchanged {
            text = new;
        }
    }
    // The memories switch.
    let recorded = state
        .file(path)
        .and_then(|entry| entry.get(MEMORIES_RECORD))
        .is_some();
    let mut memories = None;
    if memories_on(&doc) {
        if memories_off {
            match tomledit::set_bool(&text, "features", "memories", false)
                .map_err(|e| crate::install::in_file(path, e))?
            {
                Some(new) => {
                    text = new;
                    memories = Some(true);
                    lines.push(format!(
                        "[features] memories: false (switch Codex's memories off). {NOTHING_IS_LOST}"
                    ));
                }
                None => lines.push(
                    "[features] memories: on, and not written as a plain `memories = true` \
                     line; switch it off yourself if you want"
                        .to_string(),
                ),
            }
        } else {
            lines.push("[features] memories: on (kept)".to_string());
        }
    } else if recorded {
        if memories_off {
            lines.push("[features] memories: off (switched off by install)".to_string());
        } else {
            if doc["features"]["memories"] == false
                && let Some(new) = tomledit::set_bool(&text, "features", "memories", true)
                    .map_err(|e| crate::install::in_file(path, e))?
            {
                text = new;
            }
            lines.push("[features] memories: back to on, as before install".to_string());
            memories = Some(false);
        }
    }
    steps.push(Step {
        title: format!(
            "Register the MCP servers in Codex's {} (tables mcp_servers.*)",
            path.display()
        ),
        lines,
    });
    if Some(&text) != before.as_ref() && !(before.is_none() && text.is_empty()) {
        changes.push(Change {
            path: path.clone(),
            after: Some(text),
            created: Vec::new(),
            separator: None,
        });
    }

    // hooks.json: the session start and end hooks.
    let path = &paths.hooks_file;
    let before = read_text(path)?;
    let mut text = crate::install::json_start(&before, "{\n}\n");
    jsonedit::value_of(&text)
        .map_err(|err| format!("{} is not valid JSON: {err}", path.display()))?;
    let mut created = Vec::new();
    let mut lines = Vec::new();
    let ours = setup.ours();
    for (event, sub) in crate::install::HOOK_EVENTS {
        let group = hook_group(setup, sub);
        let (new, made, step) = crate::install::plan_hook_event(&ours, &text, event, &group)
            .map_err(|e| crate::install::in_file(path, e))?;
        text = new;
        created.extend(made);
        lines.push(match step {
            crate::install::HookStep::Installed => format!("{event}: already installed"),
            crate::install::HookStep::Added => {
                format!("{event}: add `{}`", hook_command(setup, sub))
            }
            crate::install::HookStep::Updated => {
                format!("{event}: update to `{}`", hook_command(setup, sub))
            }
        });
    }
    lines.push(hook_approval_line());
    steps.push(Step {
        title: format!(
            "Add the SessionStart and SessionEnd hooks in {}",
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
    Ok(Plan {
        steps,
        changes,
        conflicts,
        memories,
    })
}

/// The removal of our parts from Codex's files: one `(change, what)` per file. With
/// `develop_only`, only the develop server is switched off (`enabled = false`).
pub fn removal(
    setup: &Setup,
    state: &State,
    develop_only: bool,
    conflicts: &mut Vec<String>,
) -> Result<Vec<(Change, &'static str)>, String> {
    let paths = &setup.codex;
    let mut out = Vec::new();
    let mut targets = vec![paths.config_file.clone()];
    if !develop_only {
        targets.push(paths.hooks_file.clone());
        targets.push(paths.instruction_file.clone());
        // A block written while the other global file was the one Codex loads.
        for name in ["AGENTS.md", "AGENTS.override.md"] {
            let other = paths.codex_home.join(name);
            if !targets.contains(&other) && state.file(&other).is_some() {
                targets.push(other);
            }
        }
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
        let (new, empty) = if path == paths.config_file {
            let new = remove_from_config(setup, state, &path, &text, develop_only, conflicts)
                .map_err(|e| crate::install::in_file(&path, e))?;
            let empty = new.as_deref().is_some_and(|t| t.trim().is_empty());
            (new, empty)
        } else if path == paths.hooks_file {
            let new =
                remove_hooks(setup, &text, entry).map_err(|e| crate::install::in_file(&path, e))?;
            let empty = new.as_deref().is_some_and(|t| {
                jsonedit::value_of(t)
                    .ok()
                    .is_some_and(|v| v.as_object().is_some_and(|o| o.is_empty()))
            });
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

/// `config.toml` without our block (or our tables, when the block's markers were lost), with
/// Codex's memories back on when install switched them off; `develop_only`: the block with the
/// develop server switched off instead.
fn remove_from_config(
    setup: &Setup,
    state: &State,
    path: &Path,
    text: &str,
    develop_only: bool,
    conflicts: &mut Vec<String>,
) -> Result<Option<String>, String> {
    let doc = tomledit::parse(text)
        .map_err(|err| format!("{} is not valid TOML: {err}", path.display()))?;
    let block = tomledit::find(text).map_err(|err| format!("{}: {err}", path.display()))?;
    if develop_only {
        let develop = &doc["mcp_servers"][DEVELOP_SERVER];
        if !(setup.ours().entry(develop) && enabled(develop)) {
            return Ok(None);
        }
        return match block {
            Some(found) if found.intact => {
                Ok(Some(tomledit::put_block(text, &block_body(setup, false))?))
            }
            _ => {
                conflicts.push(format!(
                    "{}: the graphersal-memory block was edited by hand; set `enabled = false` \
                     in [mcp_servers.{DEVELOP_SERVER}] yourself",
                    path.display()
                ));
                Ok(None)
            }
        };
    }
    let mut current = text.to_string();
    let mut changed = false;
    match block {
        Some(found) if found.intact || block_is_ours(setup, &found.body) => {
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
        None => {
            for name in [USE_SERVER, DEVELOP_SERVER] {
                if setup.ours().entry(&doc["mcp_servers"][name])
                    && let Some(new) =
                        tomledit::remove_table(&current, &format!("mcp_servers.{name}"))
                {
                    current = new;
                    changed = true;
                }
            }
        }
    }
    let without = remove_user_tables(&current);
    if without != current && tomledit::parse(&without).is_ok() {
        current = without;
        changed = true;
    }
    let recorded = state
        .file(path)
        .and_then(|entry| entry.get(MEMORIES_RECORD))
        .is_some();
    if recorded
        && doc["features"]["memories"] == false
        && let Some(new) = tomledit::set_bool(&current, "features", "memories", true)
            .map_err(|e| crate::install::in_file(path, e))?
    {
        current = new;
        changed = true;
    }
    Ok(changed.then_some(current))
}

/// `hooks.json` without our groups (and the containers install created, when empty then).
fn remove_hooks(
    setup: &Setup,
    text: &str,
    entry: Option<&serde_json::Map<String, Value>>,
) -> Result<Option<String>, String> {
    let created: Vec<&str> = entry
        .and_then(|e| e.get("created"))
        .and_then(Value::as_array)
        .map(|c| c.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    crate::install::remove_hook_groups(&setup.ours(), text, &created)
}

/// What is installed in Codex.
#[derive(Debug, Clone)]
pub struct Inspection {
    /// The `use` server.
    pub use_server: PartState,
    /// The `develop` server, and whether it is enabled.
    pub develop_server: (PartState, bool),
    /// The hooks, per event.
    pub hooks: Vec<(String, PartState)>,
    /// Whether Codex recorded a trust for each of our hooks (`/hooks`).
    pub hooks_trusted: bool,
    /// The managed rule block.
    pub block: PartState,
    /// Whether Codex's memories are on, and whether install switched them off.
    pub memories: (bool, bool),
    /// Files that could not be read or parsed.
    pub errors: Vec<String>,
}

impl Inspection {
    /// Whether nothing of ours is installed.
    pub fn nothing_installed(&self) -> bool {
        self.use_server == PartState::Missing
            && self.develop_server.0 == PartState::Missing
            && self.hooks.iter().all(|(_, s)| *s == PartState::Missing)
            && self.block == PartState::Missing
    }
}

/// Inspects Codex's files.
pub fn inspect(setup: &Setup) -> Inspection {
    let paths = &setup.codex;
    let mut errors = Vec::new();
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
    let server = |name: &str, profile: &str| -> (PartState, bool) {
        let entry = &config["mcp_servers"][name];
        if entry.is_null() {
            (PartState::Missing, false)
        } else if !setup.ours().entry(entry) {
            (
                PartState::Conflict("another program's entry has this name".to_string()),
                false,
            )
        } else {
            let on = enabled(entry);
            if without_user_keys(entry) == mcp_entry(setup, profile, on) {
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
    let hooks_json = read_json(&paths.hooks_file, &mut errors);
    let mut hooks = Vec::new();
    let mut trusted = true;
    for (event, sub) in crate::install::HOOK_EVENTS {
        let groups = hooks_json["hooks"][event].as_array();
        let ours: Vec<(usize, &Value)> = groups
            .map(|g| {
                g.iter()
                    .enumerate()
                    .filter(|(_, g)| setup.ours().hook_group(g))
                    .collect()
            })
            .unwrap_or_default();
        let state = match ours.as_slice() {
            [] => PartState::Missing,
            [(_, group)] if **group == hook_group(setup, sub) => PartState::Installed,
            _ => PartState::Outdated("another binary or version".to_string()),
        };
        // Codex records a trust as `hooks.state."<hooks.json>:<event>:<group>:<handler>"`.
        let label = if sub == "session-start" {
            "session_start"
        } else {
            "session_end"
        };
        trusted &= !ours.is_empty()
            && ours.iter().all(|(i, _)| {
                let key = format!("{}:{label}:{i}:0", paths.hooks_file.display());
                config["hooks"]["state"][&key]["trusted_hash"].is_string()
            });
        hooks.push((event.to_string(), state));
    }
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
    let recorded = State::load_of(setup, Kind::Codex)
        .ok()
        .and_then(|state| {
            state
                .file(&paths.config_file)
                .map(|e| e.contains_key(MEMORIES_RECORD))
        })
        .unwrap_or(false);
    Inspection {
        use_server: server(USE_SERVER, "use").0,
        develop_server: server(DEVELOP_SERVER, "develop"),
        hooks,
        hooks_trusted: trusted,
        block,
        memories: (memories_on(&config), recorded),
        errors,
    }
}

/// The Codex part of `status`.
pub fn status_lines(setup: &Setup) -> Vec<String> {
    let inspection = inspect(setup);
    let paths = &setup.codex;
    let mut lines = vec![
        "Codex:".to_string(),
        format!("  configuration: {}", paths.config_file.display()),
    ];
    if let Some(program) = crate::codex::find_program(setup.search_path.as_deref()) {
        lines.push(format!(
            "  program: {}{}",
            program.display(),
            crate::codex::version_of(&program)
                .map(|v| format!(" ({v})"))
                .unwrap_or_default()
        ));
    }
    lines.push(format!(
        "  MCP server {USE_SERVER} (use): {}",
        inspection.use_server.describe()
    ));
    lines.push(format!(
        "  MCP server {DEVELOP_SERVER}: {}",
        match &inspection.develop_server {
            (PartState::Missing, _) => "not registered".to_string(),
            (PartState::Installed, false) =>
                "registered, disabled (`graphersal-memory install --harness codex \
                 --enable-develop` enables it)"
                    .to_string(),
            (PartState::Installed, true) => "registered, enabled".to_string(),
            (other, _) => other.describe(),
        }
    ));
    for (event, state) in &inspection.hooks {
        lines.push(format!(
            "  hook {event} ({}): {}",
            paths.hooks_file.display(),
            state.describe()
        ));
    }
    if inspection
        .hooks
        .iter()
        .any(|(_, s)| *s == PartState::Installed)
    {
        lines.push(format!(
            "  hook trust: {}",
            if inspection.hooks_trusted {
                "recorded by Codex"
            } else {
                "NOT APPROVED YET"
            }
        ));
        if !inspection.hooks_trusted {
            lines.push(format!("  {}", hook_approval_line()));
        }
    }
    lines.push(format!(
        "  rule block ({}): {}",
        paths.instruction_file.display(),
        inspection.block.describe()
    ));
    lines.push(format!(
        "  Codex memories: {}",
        match inspection.memories {
            (true, _) => "on ([features] memories = true)",
            (false, true) => "off (switched off by install; uninstall switches them on again)",
            (false, false) => "off",
        }
    ));
    lines.push(format!(
        "  headless command (summaries, bench): `{}` by default",
        setup.codex_agent().headless_command().unwrap_or_default()
    ));
    lines.push(format!(
        "  sessions: read from {}{}",
        paths.sessions.display(),
        if paths.sessions.is_dir() {
            ""
        } else {
            " (not there yet)"
        }
    ));
    for error in &inspection.errors {
        lines.push(format!("  problem: {error}"));
    }
    if inspection.nothing_installed() {
        lines.push("  Not installed: run `graphersal-memory install --harness codex`.".to_string());
    }
    lines
}

/// `doctor`'s checks of the Codex installation.
pub fn doctor(setup: &Setup) -> Vec<Finding> {
    let inspection = inspect(setup);
    let paths = &setup.codex;
    let reinstall = "run `graphersal-memory install --harness codex`";
    let mut findings = vec![ok(format!(
        "Codex: home {}, configuration {}",
        paths.codex_home.display(),
        paths.config_file.display()
    ))];
    for error in &inspection.errors {
        findings.push(problem(
            format!("Codex: {error}"),
            "fix the file by hand (Codex cannot read it either), then run install again",
        ));
    }
    let check =
        |findings: &mut Vec<Finding>, what: String, state: &PartState, fix: &str| match state {
            PartState::Installed => findings.push(ok(format!("Codex: {what}: installed"))),
            other => findings.push(problem(
                format!("Codex: {what}: {}", other.describe()),
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
            "Codex: MCP server {DEVELOP_SERVER}: {}",
            if *on { "on" } else { "off (enabled = false)" }
        ))),
        (state, _) => check(
            &mut findings,
            format!("MCP server {DEVELOP_SERVER}"),
            state,
            reinstall,
        ),
    }
    for (event, state) in &inspection.hooks {
        check(&mut findings, format!("hook {event}"), state, reinstall);
    }
    if inspection
        .hooks
        .iter()
        .all(|(_, s)| *s == PartState::Installed)
    {
        if inspection.hooks_trusted {
            findings.push(ok("Codex: the hooks are trusted (/hooks)"));
        } else {
            // Not a fault: the user's own step, shown as in `status` (highlighted, and repeated
            // as the last line of the output).
            findings.push(ok(hook_approval_line()));
        }
    }
    check(
        &mut findings,
        format!("rule block in {}", paths.instruction_file.display()),
        &inspection.block,
        reinstall,
    );
    findings.push(ok(format!(
        "Codex: {} session rollout(s) under {}",
        crate::codex::sessions::scan(&paths.sessions).len(),
        paths.sessions.display()
    )));
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_body_is_valid_toml_with_both_servers() {
        let tmp = crate::mcp_tests::TempDir::new("codex-body");
        let setup = crate::install_tests::setup_in(&tmp.path().join("h"), &tmp.path().join("d"));
        let body = block_body(&setup, false);
        let doc = tomledit::parse(&body).unwrap();
        assert_eq!(
            doc["mcp_servers"][USE_SERVER],
            mcp_entry(&setup, "use", true)
        );
        assert_eq!(
            doc["mcp_servers"][DEVELOP_SERVER],
            mcp_entry(&setup, "develop", false)
        );
        assert_eq!(doc["mcp_servers"][DEVELOP_SERVER]["enabled"], false);
        assert!(crate::agents::is_our_entry(&doc["mcp_servers"][USE_SERVER]));
        let on = tomledit::parse(&block_body(&setup, true)).unwrap();
        assert!(on["mcp_servers"][DEVELOP_SERVER]["enabled"].is_null());
    }
}
