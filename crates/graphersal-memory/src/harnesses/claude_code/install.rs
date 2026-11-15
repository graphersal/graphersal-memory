//! Claude Code's install parts (`crate::harnesses::claude_code`): the MCP entries in `~/.claude.json`, the
//! hooks and the auto-memory switch in `~/.claude/settings.json`, the managed rule block in
//! `~/.claude/CLAUDE.md` (its texts and the fingerprints earlier versions wrote), the plan and
//! the structural removal, `inspect`, `status_lines` and `claude_doctor` (with the Git Bash
//! checks of its hooks on Windows). The shared wizard and the loops are `crate::install`, which
//! re-exports these names.

#![allow(clippy::wildcard_imports)]

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::config::Config;
use crate::harnesses::agent::{Harness, Kind};
use crate::install::language::Language;
use crate::install::*;
use crate::memory::import::ImportState;
use crate::{fsutil, install::jsonedit};

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
    /// when set, else `~/.claude` (home: [`crate::runtime::store::home_dir`]).
    pub fn resolve(env: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let set = |name: &str| env(name).filter(|value| !value.is_empty());
        if let Some(dir) = set("CLAUDE_CONFIG_DIR") {
            let dir = PathBuf::from(dir);
            return Ok(Self::with(dir.join(".claude.json"), dir));
        }
        let home = crate::runtime::store::home_dir(env)?;
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
/// (`crate::install::coexist::rule_with`).
pub fn find_block_with(
    text: &str,
    language: Language,
    read_only: &[String],
) -> Result<Option<FoundBlock>, String> {
    let ours = [rule_block(Language::English), rule_block(Language::Any)];
    find_block_in(
        text,
        &crate::install::coexist::rule_with(rule_block(language), read_only),
        &ours,
        PREVIOUS_BLOCK_FINGERPRINTS,
    )
}

/// The managed block of `language` naming `read_only`, marker lines included.
pub fn block_text_with(language: Language, read_only: &[String]) -> String {
    wrap_block(&crate::install::coexist::rule_with(
        rule_block(language),
        read_only,
    ))
}

/// Claude Code's auto-memory as it is now: off when `autoMemoryEnabled` is false; switched off
/// by the user when install did not record it; asked about unless the user switched it off.
pub(crate) fn claude_own_memory_now(
    setup: &Setup,
) -> Result<crate::harnesses::agent::OwnMemoryNow, String> {
    let state = State::load(setup)?;
    let agent = setup.agent();
    let settings = read_json(agent.settings_file(), &mut Vec::new());
    let off = agent
        .auto_memory_key()
        .is_some_and(|key| settings[key] == Value::Bool(false));
    let recorded = state
        .file(agent.settings_file())
        .is_some_and(|entry| entry.contains_key(AUTO_MEMORY_RECORD));
    Ok(crate::harnesses::agent::OwnMemoryNow {
        on: !off,
        askable: !(off && !recorded),
        off_by_user: off && !recorded,
        answered: state.meta.contains_key("auto_memory_off"),
    })
}

/// Whether Claude Code's develop server is registered (an entry of ours).
pub(crate) fn claude_develop_on(setup: &Setup) -> bool {
    let user_config = read_json(&setup.paths().user_config, &mut Vec::new());
    setup
        .ours()
        .entry(&user_config["mcpServers"][DEVELOP_SERVER])
}

/// When install switched Claude Code's auto-memory off (its state records it): the settings
/// file uninstall switches it back in, and the report line (back to how it was while it is
/// still off; a value the user set since install stays).
pub(crate) fn claude_own_memory_restored(
    setup: &Setup,
    state: &State,
) -> Option<(PathBuf, String)> {
    let settings = setup.paths().settings.clone();
    if !state
        .file(&settings)
        .is_some_and(|entry| entry.contains_key(AUTO_MEMORY_RECORD))
    {
        return None;
    }
    let off_now = setup
        .agent()
        .auto_memory_key()
        .is_some_and(|key| read_json(&settings, &mut Vec::new())[key] == Value::Bool(false));
    let name = Kind::ClaudeCode.display_name();
    let line = if off_now {
        format!("{name}'s auto-memory: back to how it was before install")
    } else {
        format!("{name}'s auto-memory: kept as you set it after install")
    };
    Some((settings, line))
}

/// Claude Code's part of an install: the MCP entries, the hooks and the auto-memory switch in
/// the settings, the rule block.
pub(crate) fn claude_plan(setup: &Setup, input: &PlanInput<'_>) -> Result<HarnessPlan, String> {
    let state = State::load(setup)?;
    let mut conflicts = Vec::new();
    let mut steps = Vec::new();
    let mut changes = Vec::new();
    let (step, change) = plan_user_config(setup, input.develop, &mut conflicts)?;
    steps.push(step);
    changes.extend(change);
    let (step, change, auto_memory) =
        plan_settings(setup, input.own_memory_off, input.start_sentence, &state)?;
    steps.push(step);
    changes.extend(change);
    let (step, change) = plan_block(setup, input.language, input.read_only, &mut conflicts)?;
    steps.push(step);
    changes.extend(change);
    let mut plan = HarnessPlan::of(steps, changes, conflicts);
    plan.own_memory_off = Some(input.own_memory_off);
    let settings = setup.paths().settings.clone();
    match auto_memory {
        AutoMemoryPlan::Keep => {}
        AutoMemoryPlan::SwitchOff { previous } => {
            plan.records.push((
                settings,
                AUTO_MEMORY_RECORD,
                Some(json!({"previous": previous})),
            ));
            plan.switched_off = Some(format!(
                "{}'s auto-memory: off (uninstall writes every memory back to its memory files \
                 and switches it on again)",
                Kind::ClaudeCode.display_name()
            ));
            plan.nothing_is_lost = Some(NOTHING_IS_LOST);
        }
        AutoMemoryPlan::Restore => plan.records.push((settings, AUTO_MEMORY_RECORD, None)),
    }
    Ok(plan)
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

/// Plans the change of the user settings (the hooks, and the auto-memory switch).
fn plan_settings(
    setup: &Setup,
    auto_memory_off: bool,
    start_note: &str,
    state: &State,
) -> Result<(Step, Option<Change>, AutoMemoryPlan), String> {
    let path = &setup.paths().settings;
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
    if path == setup.paths().user_config {
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
    } else if path == setup.paths().settings {
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
    } else if path == setup.paths().instructions {
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

/// The removal of our parts from Claude Code's files: one `(change, what)` per file.
pub(crate) fn claude_removal(
    setup: &Setup,
    develop_only: bool,
    state: &State,
    conflicts: &mut Vec<String>,
) -> Result<Vec<(Change, &'static str)>, String> {
    let mut changes = Vec::new();
    let targets: Vec<&Path> = if develop_only {
        vec![&setup.paths().user_config]
    } else {
        vec![
            &setup.paths().user_config,
            &setup.paths().settings,
            &setup.paths().instructions,
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
        if untouched && !develop_only {
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
        if let Some(new) = remove_ours(setup, path, &text, entry, develop_only, conflicts)
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
                if develop_only {
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

/// Inspects Claude Code's files.
pub fn inspect(setup: &Setup) -> Inspection {
    let mut errors = Vec::new();
    let mut missing_binaries = Vec::new();
    let config = read_json(&setup.paths().user_config, &mut errors);
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
    let settings = read_json(&setup.paths().settings, &mut errors);
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
    let block = match read_text(&setup.paths().instructions) {
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
            setup.paths().user_config.display()
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
            setup.paths().settings.display(),
            state.describe()
        ));
    }
    lines.push(format!(
        "  rule block ({}): {}",
        setup.paths().instructions.display(),
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
        setup.paths().settings.display(),
        auto_memory_state(setup)
    ));
    if setup.hooks.windows {
        let settings = read_json(&setup.paths().settings, &mut Vec::new());
        lines.push(git_bash_status(
            &crate::install::winshell::find_git_bash_here(&settings),
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
                setup.paths().instructions.display()
            ),
            reinstall,
        )),
        PartState::Outdated(why) => findings.push(problem(format!("rule block {why}"), reinstall)),
        PartState::Conflict(why) => findings.push(problem(
            format!(
                "rule block {why} in {}",
                setup.paths().instructions.display()
            ),
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
        Ok(crate::memory::import::totals(&crate::memory::import::plan(
            crate::harnesses::claude_code::import::scan(&setup.paths().projects)?,
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
        let settings = read_json(&setup.paths().settings, &mut Vec::new());
        findings.extend(git_bash_findings(
            &crate::install::winshell::find_git_bash_here(&settings),
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
                    .is_some_and(|hooks| hooks.iter().any(crate::install::winshell::needs_git_bash))
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
pub fn git_bash_findings(
    git_bash: &crate::install::winshell::GitBash,
    settings: &Value,
) -> Vec<Finding> {
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
pub fn git_bash_status(git_bash: &crate::install::winshell::GitBash, settings: &Value) -> String {
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

/// Whether Claude Code's settings file (`paths.settings`) holds our SessionStart hook (then
/// the hook delivers the start context, and the MCP instructions leave it out).
pub fn claude_start_hook_installed(paths: &ClaudePaths) -> bool {
    read_json(&paths.settings, &mut Vec::new())["hooks"]["SessionStart"]
        .as_array()
        .is_some_and(|groups| groups.iter().any(is_our_hook_group))
}
