//! `install`, `uninstall`, `status` and `doctor` for DeepSeek Harness: what is written into its
//! files, and nothing else (`crate::harnesses::dsh` explains why each part goes where it goes):
//!
//! - in the home layer `$DSH_HOME/cordis.patch.yml`: one managed block
//!   (`crate::install::yamledit`) holding one `- insert:` item with the rows
//!   `graphersal-memory` (the `use` profile) and `graphersal-memory-develop` (`disabled: true`
//!   until `--enable-develop`); a missing file is created with the block, dsh's empty template
//!   (`[]`) is replaced by it;
//! - the managed rule block in `$DSH_HOME/AGENTS.md`.
//!
//! No hooks, no profile layer, never `cordis.yml`. The state is `<data dir>/install/dsh.json`,
//! backups as for the other harnesses; `uninstall` restores an untouched file byte for byte from
//! its backup (or deletes a file install created), else removes only our block from its current
//! text, putting `[]` back when nothing else is left (an empty or comments-only patch file stops
//! every dsh start), and deletes a patch file install created that holds nothing else.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::fsutil;
use crate::harnesses::agent::{Harness, Kind};
use crate::install::language::Language;
use crate::install::{
    Change, DEVELOP_SERVER, Finding, PartState, Setup, State, Step, USE_SERVER, block_separator,
    find_block_in, note, ok, problem, read_text, wrap_block, yamledit,
};

/// The rule block for dsh with the English memory language rule.
pub const RULE_BLOCK: &str = "\
## Memory: graphersal-memory

Your long-term memory of this user lives in a local graph, served by the MCP server
`graphersal-memory` (its tools `mcp__graphersal-memory__memories`, `__recall`, `__memory`,
`__remember`, `__forget`, `__sessions`, `__session`).

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

/// The rule block for dsh with the \"any language\" memory language rule.
pub const RULE_BLOCK_ANY: &str = "\
## Memory: graphersal-memory

Your long-term memory of this user lives in a local graph, served by the MCP server
`graphersal-memory` (its tools `mcp__graphersal-memory__memories`, `__recall`, `__memory`,
`__remember`, `__forget`, `__sessions`, `__session`).

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

/// The fingerprints of dsh rule blocks earlier versions wrote (add the current one whenever a
/// text above changes).
pub const PREVIOUS_BLOCK_FINGERPRINTS: &[&str] = &[];

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

/// The arguments of a profile's server.
fn args(setup: &Setup, profile: &str) -> Vec<String> {
    let mut args = vec![
        "mcp".to_string(),
        "--profile".to_string(),
        profile.to_string(),
    ];
    args.extend(setup.dir_args.iter().cloned());
    args
}

/// The servers: (row id and server name, profile).
const SERVERS: [(&str, &str); 2] = [(USE_SERVER, "use"), (DEVELOP_SERVER, "develop")];

/// The row of a server as dsh reads it (`disabled: true` when off).
pub fn row(setup: &Setup, name: &str, profile: &str, enabled: bool) -> Value {
    let mut config = setup
        .dsh_agent()
        .mcp_entry(&setup.exe, &args(setup, profile));
    config["serverName"] = json!(name);
    let mut row = json!({"id": name, "name": crate::harnesses::dsh::MCP_CLIENT, "config": config});
    if !enabled {
        row["disabled"] = json!(true);
    }
    row
}

/// The YAML text of a server's row, as an element of the `insert` list.
fn row_text(setup: &Setup, name: &str, profile: &str, enabled: bool) -> String {
    let list = |items: &[String]| {
        items
            .iter()
            .map(|item| yamledit::string(item))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut text = format!(
        "    - id: {name}\n      name: {}\n",
        yamledit::string(crate::harnesses::dsh::MCP_CLIENT)
    );
    if !enabled {
        text.push_str("      disabled: true\n");
    }
    // dsh drops `DSH_*` from the child's environment: a moved home goes back in through `env`
    // (`DshPaths::row_env`).
    let paths = setup.dsh();
    let home = if paths.explicit_home {
        format!(
            "          {}: {}\n",
            crate::harnesses::dsh::HOME_ENV,
            yamledit::string(&paths.dsh_home.display().to_string())
        )
    } else {
        String::new()
    };
    text.push_str(&format!(
        "      config:\n        serverName: {name}\n        transport: stdio\n        command: {}\n        \
         args: [{}]\n        env:\n          {}: {}\n{home}        startupTimeoutMs: {}\n",
        yamledit::string(&setup.exe.display().to_string()),
        list(&args(setup, profile)),
        crate::runtime::shim::HARNESS_ENV,
        Kind::Dsh.id(),
        crate::harnesses::dsh::STARTUP_TIMEOUT_MS,
    ));
    text
}

/// The body of the managed block: one `insert` item with both rows (`develop`: the develop row
/// on).
pub fn block_body(setup: &Setup, develop: bool) -> String {
    format!(
        "- insert:\n{}{}",
        row_text(setup, USE_SERVER, "use", true),
        row_text(setup, DEVELOP_SERVER, "develop", develop)
    )
}

/// The rows of the `insert` items among `items`.
fn inserted(items: &[Value]) -> Vec<&Value> {
    items
        .iter()
        .filter_map(|item| item["insert"].as_array())
        .flatten()
        .collect()
}

/// Whether a row is one of the names of our servers, by its id or its server name.
fn names_ours(row: &Value) -> bool {
    SERVERS.iter().any(|(name, _)| {
        row["id"].as_str() == Some(name) || row["config"]["serverName"].as_str() == Some(name)
    })
}

/// Whether a block body (edited by hand) still holds only our rows: one `insert` item whose
/// rows are our servers starting one of our programs.
fn body_is_ours(setup: &Setup, body: &str) -> bool {
    yamledit::items(body).is_ok_and(|items| {
        items.len() == 1
            && items[0].as_object().is_some_and(|o| o.len() == 1)
            && inserted(&items)
                .iter()
                .all(|row| names_ours(row) && setup.ours().entry(&row["config"]))
    })
}

/// The rows of our servers that are not in our block: `(file text without the block)`'s
/// inserted rows with our ids or server names (dsh would refuse a second server of a name).
fn foreign_rows(text: &str) -> Vec<String> {
    let rest = match yamledit::remove_block(text) {
        Ok(Some(rest)) => rest,
        _ => text.to_string(),
    };
    yamledit::items(&rest)
        .map(|items| {
            inserted(&items)
                .into_iter()
                .filter(|row| names_ours(row))
                .map(|row| row["id"].as_str().unwrap_or("(no id)").to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The error of a patch file install cannot edit.
fn refused(path: &Path, err: &str) -> String {
    format!(
        "{}: {err}; dsh cannot start with it either. Fix the file, then run `graphersal-memory \
         install --harness dsh` again",
        path.display()
    )
}

/// The planned install for dsh: its steps, file changes and conflicts.
pub struct Plan {
    /// The steps for the wizard and the report.
    pub steps: Vec<Step>,
    /// The file changes.
    pub changes: Vec<Change>,
    /// What could not be done.
    pub conflicts: Vec<String>,
}

/// Plans dsh's part of the install.
pub fn plan(
    setup: &Setup,
    language: Language,
    read_only: &[String],
    develop: bool,
) -> Result<Plan, String> {
    let paths = setup.dsh();
    let mut conflicts = Vec::new();
    let mut steps = Vec::new();
    let mut changes = Vec::new();

    // cordis.patch.yml: the rows.
    let path = &paths.patch_file;
    let before = read_text(path)?;
    let text = before.clone().unwrap_or_default();
    let found = yamledit::find(&text).map_err(|err| refused(path, &err))?;
    yamledit::top_level(&text).map_err(|err| refused(path, &err))?;
    let word = if develop { "enabled" } else { "disabled" };
    let mut lines = Vec::new();
    let foreign = foreign_rows(&text);
    let mut after = None;
    match &found {
        Some(found) if !found.intact => {
            conflicts.push(format!(
                "{}: the graphersal-memory block was edited by hand; kept as it is. Move your \
                 changes out of the block (or delete the block) and run install again.",
                path.display()
            ));
            lines.push("CONFLICT (see the report)".to_string());
        }
        _ if !foreign.is_empty() => {
            conflicts.push(format!(
                "{}: a row of another program uses the name {} (dsh refuses a second server of \
                 one name); kept. Remove it and run install again.",
                path.display(),
                foreign.join(", ")
            ));
            lines.push("CONFLICT (a row of another program has our name)".to_string());
        }
        Some(found) if found.body == block_body(setup, develop) => {
            lines.push(format!("{USE_SERVER} (use profile): already registered"));
            lines.push(format!(
                "{DEVELOP_SERVER} (develop profile): already registered ({word})"
            ));
        }
        _ => {
            let new = yamledit::put_block(&text, &block_body(setup, develop))
                .map_err(|err| refused(path, &err))?;
            let verb = if found.is_some() { "update" } else { "add" };
            lines.push(format!("{USE_SERVER} (use profile): {verb}"));
            lines.push(format!(
                "{DEVELOP_SERVER} (develop profile): {verb} ({word})"
            ));
            if before.is_none() {
                lines.push("create the file with the block (uninstall deletes it)".to_string());
            }
            after = Some(new);
        }
    }
    if !develop {
        lines.push(format!(
            "{DEVELOP_SERVER} stays off; `graphersal-memory install --harness dsh \
             --enable-develop` switches it on"
        ));
    }
    lines.push(
        "in the home layer, which every dsh profile applies after its own; profiles and \
         cordis.yml are not touched"
            .to_string(),
    );
    steps.push(Step {
        title: format!(
            "Register the MCP servers in dsh's home layer {}",
            path.display()
        ),
        lines,
    });
    if let Some(after) = after.filter(|after| Some(after) != before.as_ref()) {
        changes.push(Change {
            path: path.clone(),
            after: Some(after),
            created: Vec::new(),
            separator: None,
        });
    }

    // AGENTS.md of the dsh home: the managed rule block.
    let path = &paths.instruction_file;
    let before = read_text(path)?;
    let current = before.clone().unwrap_or_default();
    let block = crate::install::line_ends_like(
        &current,
        &wrap_block(&crate::install::coexist::rule_with(
            rule_block(language),
            read_only,
        )),
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
            "dsh loads it first in every profile but sdk-minimal (which loads no instruction \
             files); never into ~/.agents/AGENTS.md"
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
    })
}

/// The removal of our parts from dsh's files: one `(change, what)` per file. With
/// `develop_only`, only the develop row is switched off (`disabled: true`).
pub fn removal(
    setup: &Setup,
    state: &State,
    develop_only: bool,
    conflicts: &mut Vec<String>,
) -> Result<Vec<(Change, &'static str)>, String> {
    let paths = setup.dsh();
    let mut out = Vec::new();
    let mut targets = vec![paths.patch_file.clone()];
    if !develop_only {
        targets.push(paths.instruction_file.clone());
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
        let (new, empty) = if path == paths.patch_file {
            let new = remove_rows(setup, &path, &text, develop_only, conflicts);
            // Nothing but our block was in it: no entry left (`[]` or nothing).
            let empty = new
                .as_deref()
                .is_some_and(|t| yamledit::items(t).is_ok_and(|items| items.is_empty()));
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

/// The patch file without our block (`[]` put back when nothing else is left), or with the
/// develop row switched off (`develop_only`); `None` when nothing changes. What cannot be
/// removed safely is a conflict.
fn remove_rows(
    setup: &Setup,
    path: &Path,
    text: &str,
    develop_only: bool,
    conflicts: &mut Vec<String>,
) -> Option<String> {
    let found = match yamledit::find(text) {
        Ok(Some(found)) => found,
        Ok(None) => return None,
        Err(err) => {
            conflicts.push(format!("{}: {err}; not changed", path.display()));
            return None;
        }
    };
    if develop_only {
        let on = found.intact
            && yamledit::items(&found.body).is_ok_and(|items| {
                inserted(&items)
                    .iter()
                    .any(|row| row["id"] == DEVELOP_SERVER && row["disabled"] != Value::Bool(true))
            });
        if !on {
            return None;
        }
        return match yamledit::put_block(text, &block_body(setup, false)) {
            Ok(new) => Some(new),
            Err(err) => {
                conflicts.push(format!(
                    "{}: the develop server could not be switched off ({err}); set `disabled: \
                     true` on its row yourself",
                    path.display()
                ));
                None
            }
        };
    }
    if !found.intact && !body_is_ours(setup, &found.body) {
        conflicts.push(format!(
            "{}: the graphersal-memory block was edited by hand, so it was not removed; delete \
             it yourself (put `[]` there when nothing else is left: dsh does not start with an \
             empty patch file)",
            path.display()
        ));
        return None;
    }
    match yamledit::remove_block(text) {
        Ok(new) => new,
        Err(err) => {
            conflicts.push(format!(
                "{}: the graphersal-memory block could not be removed ({err}); delete it \
                 yourself (put `[]` there when nothing else is left)",
                path.display()
            ));
            None
        }
    }
}

/// What is installed in dsh.
#[derive(Debug, Clone)]
pub struct Inspection {
    /// The `use` row.
    pub use_row: PartState,
    /// The `develop` row, and whether it is enabled.
    pub develop_row: (PartState, bool),
    /// The managed rule block.
    pub block: PartState,
    /// Files that could not be read or parsed.
    pub errors: Vec<String>,
}

impl Inspection {
    /// Whether nothing of ours is installed.
    pub fn nothing_installed(&self) -> bool {
        self.use_row == PartState::Missing
            && self.develop_row.0 == PartState::Missing
            && self.block == PartState::Missing
    }
}

/// The rows of our block in `text`, each server's state.
fn rows_state(setup: &Setup, text: &str) -> Result<[(PartState, bool); 2], String> {
    let conflict = |why: &str| {
        [
            (PartState::Conflict(why.to_string()), false),
            (PartState::Conflict(why.to_string()), false),
        ]
    };
    let Some(found) = yamledit::find(text)? else {
        let foreign = foreign_rows(text);
        if foreign.is_empty() {
            return Ok([(PartState::Missing, false), (PartState::Missing, false)]);
        }
        return Ok(SERVERS.map(|(name, _)| {
            if foreign.iter().any(|id| id == name) {
                (
                    PartState::Conflict("a row of another program has this name".to_string()),
                    false,
                )
            } else {
                (PartState::Missing, false)
            }
        }));
    };
    if !found.intact {
        return Ok(conflict("the block was edited by hand"));
    }
    let items = yamledit::items(&found.body)?;
    let rows = inserted(&items);
    Ok(SERVERS.map(|(name, profile)| {
        match rows.iter().find(|row| row["id"].as_str() == Some(name)) {
            None => (PartState::Missing, false),
            Some(found) => {
                let on = found["disabled"] != Value::Bool(true);
                if **found == row(setup, name, profile, on) {
                    (PartState::Installed, on)
                } else {
                    (
                        PartState::Outdated(format!(
                            "starts {}",
                            found["config"]["command"].as_str().unwrap_or_default()
                        )),
                        on,
                    )
                }
            }
        }
    }))
}

/// Inspects dsh's files.
pub fn inspect(setup: &Setup) -> Inspection {
    let paths = setup.dsh();
    let mut errors = Vec::new();
    let [use_row, develop_row] = match read_text(&paths.patch_file) {
        Ok(Some(text)) => rows_state(setup, &text).unwrap_or_else(|err| {
            errors.push(format!("{}: {err}", paths.patch_file.display()));
            [(PartState::Missing, false), (PartState::Missing, false)]
        }),
        Ok(None) => [(PartState::Missing, false), (PartState::Missing, false)],
        Err(err) => {
            errors.push(err);
            [(PartState::Missing, false), (PartState::Missing, false)]
        }
    };
    let language = crate::config::Config::load(&setup.data_dir)
        .map(|c| c.language())
        .unwrap_or_default();
    let block = match read_text(&paths.instruction_file) {
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
        use_row: use_row.0,
        develop_row,
        block,
        errors,
    }
}

/// The patch files of every layer under the dsh home that exist, each with its items (the home
/// layer first; a file that cannot be read or parsed is left out).
fn layers(setup: &Setup) -> Vec<(String, PathBuf, Vec<Value>)> {
    let paths = setup.dsh();
    let mut files = vec![("home".to_string(), paths.patch_file.clone())];
    files.extend(
        paths
            .profile_patches()
            .into_iter()
            .map(|(name, file)| (format!("profile {name}"), file)),
    );
    files
        .into_iter()
        .filter_map(|(layer, file)| {
            let text = read_text(&file).ok().flatten()?;
            let items = yamledit::items(&text).ok()?;
            Some((layer, file, items))
        })
        .collect()
}

/// Whether patch items switch dsh's session log upload off (`config: {enabled: false}` or
/// `disabled: true` on its row).
pub fn switches_session_log_off(items: &[Value]) -> bool {
    items.iter().any(|item| {
        item["id"] == crate::harnesses::dsh::SESSION_LOG_ROW
            && (item["disabled"] == Value::Bool(true)
                || item["config"]["enabled"] == Value::Bool(false))
    })
}

/// The layers that switch the session log upload off (empty: it is on in every profile).
pub fn session_log_off(setup: &Setup) -> Vec<String> {
    layers(setup)
        .into_iter()
        .filter(|(_, _, items)| switches_session_log_off(items))
        .map(|(layer, _, _)| layer)
        .collect()
}

/// The `mcp-manager` servers whose name is one of ours (or starts like ours), with their layer:
/// dsh would refuse our row of that name there.
pub fn manager_clashes(setup: &Setup) -> Vec<(String, String)> {
    layers(setup)
        .into_iter()
        .flat_map(|(layer, _, items)| {
            items
                .iter()
                .filter(|item| item["id"] == crate::harnesses::dsh::MCP_MANAGER_ROW)
                .filter_map(|item| item["config"]["servers"].as_object())
                .flat_map(|servers| servers.keys())
                .filter(|name| name.starts_with(USE_SERVER))
                .map(|name| (layer.clone(), name.clone()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The privacy note's text.
const SESSION_LOG_NOTE: &str = "dsh's session log upload (`session-log-deepseek`) is on: with \
    the official DeepSeek API it sends the session log, the memory tools' results included, \
    with every request";

/// How to switch the session log upload off.
const SESSION_LOG_FIX: &str = "to keep memories out of it, add `- id: session-log-deepseek` \
    with `config: {enabled: false}` to ~/.dsh/cordis.patch.yml (or switch \"Upload Session Log\" \
    off in dsh's Settings); graphersal-memory never changes it";

/// The dsh part of `status`.
pub fn status_lines(setup: &Setup) -> Vec<String> {
    let inspection = inspect(setup);
    let paths = setup.dsh();
    let mut lines = vec![
        "DeepSeek Harness (preview):".to_string(),
        format!("  home: {}", paths.dsh_home.display()),
        match setup.program(Kind::Dsh) {
            Some(program) => format!("  program: {}", program.display()),
            None => format!(
                "  program: not found (`npm install -g {}` installs it; npx leaves none)",
                crate::harnesses::dsh::NPM_PACKAGE
            ),
        },
        format!(
            "  MCP server {USE_SERVER} (use, {}): {}",
            paths.patch_file.display(),
            inspection.use_row.describe()
        ),
        format!(
            "  MCP server {DEVELOP_SERVER}: {}",
            match &inspection.develop_row {
                (PartState::Missing, _) => "not registered".to_string(),
                (PartState::Installed, false) =>
                    "registered, disabled (`graphersal-memory install --harness dsh \
                     --enable-develop` enables it)"
                        .to_string(),
                (PartState::Installed, true) => "registered, enabled".to_string(),
                (other, _) => other.describe(),
            }
        ),
        format!(
            "  rule block ({}): {}",
            paths.instruction_file.display(),
            inspection.block.describe()
        ),
        "  summaries and bench: not through dsh (it cannot run without tools)".to_string(),
        format!(
            "  sessions: {} in {}",
            crate::harnesses::dsh::sessions::scan(&paths.sessions).len(),
            paths.sessions.display()
        ),
        "  session start and end: through the MCP connection (no hooks); a session is indexed \
         when dsh exits, or 30 minutes after its last change"
            .to_string(),
    ];
    let off = session_log_off(setup);
    lines.push(if off.is_empty() {
        format!("  note: {SESSION_LOG_NOTE}")
    } else {
        format!(
            "  session log upload (session-log-deepseek): off in the {} layer(s)",
            off.join(", ")
        )
    });
    for error in &inspection.errors {
        lines.push(format!("  problem: {error}"));
    }
    if inspection.nothing_installed() {
        lines.push("  Not installed: run `graphersal-memory install --harness dsh`.".to_string());
    }
    lines
}

/// `doctor`'s checks of the dsh installation.
pub fn doctor(setup: &Setup) -> Vec<Finding> {
    let inspection = inspect(setup);
    let paths = setup.dsh();
    let reinstall = "run `graphersal-memory install --harness dsh`";
    let mut findings = vec![ok(format!(
        "DeepSeek Harness: home {}, home layer {}",
        paths.dsh_home.display(),
        paths.patch_file.display()
    ))];
    match setup.program(Kind::Dsh) {
        Some(program) => findings.push(ok(format!(
            "DeepSeek Harness: program {}",
            program.display()
        ))),
        None => findings.push(note(
            "DeepSeek Harness: the `dsh` program is not found (`npx` leaves none)",
            format!(
                "install it globally: `npm install -g {}`",
                crate::harnesses::dsh::NPM_PACKAGE
            ),
        )),
    }
    for error in &inspection.errors {
        findings.push(problem(
            format!("DeepSeek Harness: {error}"),
            "fix the file by hand (dsh cannot start with it either), then run install again",
        ));
    }
    let check =
        |findings: &mut Vec<Finding>, what: String, state: &PartState, fix: &str| match state {
            PartState::Installed => {
                findings.push(ok(format!("DeepSeek Harness: {what}: installed")))
            }
            other => findings.push(problem(
                format!("DeepSeek Harness: {what}: {}", other.describe()),
                fix.to_string(),
            )),
        };
    check(
        &mut findings,
        format!("MCP server {USE_SERVER}"),
        &inspection.use_row,
        reinstall,
    );
    match &inspection.develop_row {
        (PartState::Installed, on) => findings.push(ok(format!(
            "DeepSeek Harness: MCP server {DEVELOP_SERVER}: {}",
            if *on { "on" } else { "off (disabled)" }
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
        format!("rule block in {}", paths.instruction_file.display()),
        &inspection.block,
        reinstall,
    );
    for (layer, name) in manager_clashes(setup) {
        findings.push(note(
            format!(
                "DeepSeek Harness: the {layer} layer's MCP manager has a server named `{name}`: \
                 in the web and Desktop profiles dsh refuses a second server of one name, so \
                 ours may not load there"
            ),
            "rename or remove that server in dsh's Settings (MCP)",
        ));
    }
    if paths
        .profiles
        .join(crate::harnesses::dsh::MINIMAL_PROFILE)
        .is_dir()
    {
        findings.push(note(
            "DeepSeek Harness: the sdk-minimal profile loads no instruction files, so the rule \
             block is not read there",
            "nothing to do: the memory tools and the MCP server's instructions still reach it",
        ));
    }
    if session_log_off(setup).is_empty() {
        findings.push(note(
            format!("DeepSeek Harness: {SESSION_LOG_NOTE}"),
            SESSION_LOG_FIX,
        ));
    }
    findings.push(ok(format!(
        "DeepSeek Harness: {} saved session(s) in {}",
        crate::harnesses::dsh::sessions::scan(&paths.sessions).len(),
        paths.sessions.display()
    )));
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harnesses::dsh::{DshPaths, fixture};
    use crate::install::install_tests::{
        Memory, bare_setup_in, run_install, run_uninstall, snapshot, with_programs,
    };
    use crate::install::{InstallOptions, UninstallOptions};
    use crate::runtime::mcp_tests::TempDir;

    /// A machine with only dsh's program.
    fn machine(home: &Path, data: &Path) -> Setup {
        with_programs(bare_setup_in(home, data), &["dsh"])
    }

    fn into_dsh() -> InstallOptions {
        InstallOptions {
            harnesses: Some(vec![Kind::Dsh]),
            yes: true,
            ..InstallOptions::default()
        }
    }

    fn out_of_dsh() -> UninstallOptions {
        UninstallOptions {
            harnesses: Some(vec![Kind::Dsh]),
            yes: true,
            no_export: true,
            ..UninstallOptions::default()
        }
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    /// Nothing of ours anywhere but in the dsh home: never `~/.agents` or CodeWhale's files.
    fn nothing_elsewhere(home: &Path) {
        for dir in [".agents", ".codewhale", ".deepseek", ".claude"] {
            assert!(!home.join(dir).exists(), "{dir} was written");
        }
    }

    #[test]
    fn the_block_holds_one_insert_item_with_both_rows() {
        let tmp = TempDir::new("dsh-block");
        let mut setup = bare_setup_in(tmp.path(), tmp.path());
        // A Windows path with spaces and backslashes, and a quote: quoted for YAML.
        setup.exe = PathBuf::from(r#"C:\Program Files\graphersal "memory"\graphersal-memory.exe"#);
        for develop in [false, true] {
            let body = block_body(&setup, develop);
            let items = yamledit::items(&body).unwrap();
            assert_eq!(
                items,
                vec![json!({"insert": [
                    row(&setup, USE_SERVER, "use", true),
                    row(&setup, DEVELOP_SERVER, "develop", develop),
                ]})],
                "{body}"
            );
            let rows = inserted(&items);
            assert_eq!(rows[0]["name"], "@deepseek-ai/dsh-mcp-client");
            assert_eq!(rows[0]["config"]["serverName"], USE_SERVER);
            assert_eq!(rows[0]["config"]["transport"], "stdio");
            assert_eq!(
                rows[0]["config"]["command"],
                r#"C:\Program Files\graphersal "memory"\graphersal-memory.exe"#
            );
            assert_eq!(rows[0]["config"]["args"][2], "use");
            assert_eq!(rows[0]["config"]["env"]["GRAPHERSAL_MEMORY_HARNESS"], "dsh");
            assert_eq!(rows[0]["config"]["startupTimeoutMs"], 60_000);
            assert!(rows[0].get("disabled").is_none());
            assert_eq!(rows[1].get("disabled").is_some(), !develop);
            assert!(setup.ours().entry(&rows[0]["config"]));
            assert!(body_is_ours(&setup, &body));
        }
        assert!(!body_is_ours(
            &setup,
            "- insert:\n    - id: other\n      config: {command: x, args: [mcp]}\n"
        ));
    }

    #[test]
    fn a_missing_patch_file_is_created_and_deleted_again() {
        let home = TempDir::new("dsh-missing-home");
        let data = TempDir::new("dsh-missing-data");
        let setup = machine(home.path(), data.path());
        let before = snapshot(home.path());
        let memory = Memory::open(data.path());
        let (code, out) = run_install(&setup, &memory, into_dsh());
        assert_eq!(code, 0, "{out}");
        let paths = setup.dsh();
        assert_eq!(
            read(&paths.patch_file),
            yamledit::wrap(&block_body(&setup, false))
        );
        assert!(read(&paths.instruction_file).contains("mcp__graphersal-memory__memories"));
        nothing_elsewhere(home.path());
        assert!(status_lines(&setup).join("\n").contains("installed"));
        let (code, out) = run_uninstall(&setup, out_of_dsh());
        assert_eq!(code, 0, "{out}");
        assert_eq!(snapshot(home.path()), before, "{out}");
        assert!(!paths.dsh_home.exists());
        memory.close();
    }

    #[test]
    fn the_empty_template_gives_way_to_the_block_and_comes_back() {
        let home = TempDir::new("dsh-template-home");
        let data = TempDir::new("dsh-template-data");
        fixture::user_files(home.path());
        let setup = machine(home.path(), data.path());
        let paths = setup.dsh().clone();
        let memory = Memory::open(data.path());
        run_install(&setup, &memory, into_dsh());
        let installed = read(&paths.patch_file);
        assert!(installed.starts_with("# DeepSeek Harness home patch.\n"));
        assert!(!installed.contains("[]"), "{installed}");
        // The profile layer is never written.
        assert_eq!(
            read(&paths.profiles.join("web").join("cordis.patch.yml")),
            fixture::WEB_PATCH
        );
        // Changed by the user afterwards: only our block goes, `[]` comes back.
        std::fs::write(&paths.patch_file, format!("# mine\n{installed}")).unwrap();
        let (code, out) = run_uninstall(&setup, out_of_dsh());
        assert_eq!(code, 0, "{out}");
        assert_eq!(
            read(&paths.patch_file),
            format!("# mine\n{}", fixture::HOME_PATCH)
        );
        assert_eq!(
            yamledit::top_level(&read(&paths.patch_file)),
            Ok(yamledit::Top::EmptyFlow)
        );
        memory.close();
    }

    #[test]
    fn user_overrides_with_crlf_stay_byte_for_byte() {
        let home = TempDir::new("dsh-crlf-home");
        let data = TempDir::new("dsh-crlf-data");
        let setup = machine(home.path(), data.path());
        let paths = setup.dsh().clone();
        std::fs::create_dir_all(&paths.dsh_home).unwrap();
        let original = fixture::USER_PATCH.replace('\n', "\r\n");
        std::fs::write(&paths.patch_file, &original).unwrap();
        let before = snapshot(home.path());
        let memory = Memory::open(data.path());
        let (code, out) = run_install(&setup, &memory, into_dsh());
        assert_eq!(code, 0, "{out}");
        let installed = read(&paths.patch_file);
        assert!(installed.starts_with(&original));
        assert!(
            !installed.replace("\r\n", "").contains('\n'),
            "{installed:?}"
        );
        let items = yamledit::items(&installed).unwrap();
        assert_eq!(items.len(), 4);
        assert_eq!(items[..3], yamledit::items(&original).unwrap()[..]);
        // Untouched: the backup comes back.
        let (code, out) = run_uninstall(&setup, out_of_dsh());
        assert_eq!(code, 0, "{out}");
        assert_eq!(snapshot(home.path()), before);
        // Changed after install (a line appended): the structural removal keeps it.
        run_install(&setup, &memory, into_dsh());
        let later = format!(
            "{}- id: hmr\r\n  disabled: true\r\n",
            read(&paths.patch_file)
        );
        std::fs::write(&paths.patch_file, &later).unwrap();
        let (code, out) = run_uninstall(&setup, out_of_dsh());
        assert_eq!(code, 0, "{out}");
        assert_eq!(
            read(&paths.patch_file),
            format!("{original}- id: hmr\r\n  disabled: true\r\n")
        );
        memory.close();
    }

    #[test]
    fn a_patch_file_dsh_cannot_read_is_refused_with_nothing_written() {
        for text in [
            "id: hmr\ndisabled: true\n",
            "[{id: hmr}]\n",
            "- id: a\n  id: b\n",
            "- !custom x\n",
        ] {
            let home = TempDir::new("dsh-refused-home");
            let data = TempDir::new("dsh-refused-data");
            let setup = machine(home.path(), data.path());
            let paths = setup.dsh().clone();
            std::fs::create_dir_all(&paths.dsh_home).unwrap();
            std::fs::write(&paths.patch_file, text).unwrap();
            let before = snapshot(home.path());
            let memory = Memory::open(data.path());
            let mut connect = memory.connector();
            let mut out = Vec::new();
            let err =
                crate::install::install(&setup, &into_dsh(), &mut connect, &mut &b""[..], &mut out)
                    .unwrap_err();
            assert!(
                err.contains(&paths.patch_file.display().to_string()),
                "{err}"
            );
            assert!(err.contains("--harness dsh"), "{err}");
            assert!(err.contains("nothing was changed"), "{err}");
            assert_eq!(snapshot(home.path()), before, "{text:?}");
            memory.close();
        }
    }

    #[test]
    fn a_hand_edit_inside_the_block_is_a_conflict_never_overwritten() {
        let home = TempDir::new("dsh-edit-home");
        let data = TempDir::new("dsh-edit-data");
        fixture::user_files(home.path());
        let setup = machine(home.path(), data.path());
        let paths = setup.dsh().clone();
        let memory = Memory::open(data.path());
        run_install(&setup, &memory, into_dsh());
        let edited = read(&paths.patch_file).replace(
            "    - id: graphersal-memory-develop\n",
            "    - id: my-own\n      name: x\n    - id: graphersal-memory-develop\n",
        );
        std::fs::write(&paths.patch_file, &edited).unwrap();
        let (code, out) = run_install(&setup, &memory, into_dsh());
        assert_eq!(code, 1, "{out}");
        assert!(out.contains("edited by hand"), "{out}");
        assert_eq!(read(&paths.patch_file), edited);
        assert!(matches!(inspect(&setup).use_row, PartState::Conflict(_)));
        // Not ours any more: uninstall leaves it and says so.
        let (_, out) = run_uninstall(&setup, out_of_dsh());
        assert!(out.contains("edited by hand"), "{out}");
        assert_eq!(read(&paths.patch_file), edited);
        // An edit that keeps only our rows (a longer timeout) is still removed.
        std::fs::write(&paths.patch_file, fixture::HOME_PATCH).unwrap();
        run_install(&setup, &memory, into_dsh());
        let longer = read(&paths.patch_file).replacen("60000", "90000", 1);
        std::fs::write(&paths.patch_file, &longer).unwrap();
        run_uninstall(&setup, out_of_dsh());
        assert_eq!(read(&paths.patch_file), fixture::HOME_PATCH);
        memory.close();
    }

    #[test]
    fn a_row_of_another_program_with_our_name_is_a_conflict() {
        let home = TempDir::new("dsh-foreign-home");
        let data = TempDir::new("dsh-foreign-data");
        let setup = machine(home.path(), data.path());
        let paths = setup.dsh().clone();
        std::fs::create_dir_all(&paths.dsh_home).unwrap();
        let text = "- insert:\n    - id: memory\n      name: '@deepseek-ai/dsh-mcp-client'\n      config:\n        serverName: graphersal-memory\n        transport: stdio\n        command: other\n";
        std::fs::write(&paths.patch_file, text).unwrap();
        let memory = Memory::open(data.path());
        let (code, out) = run_install(&setup, &memory, into_dsh());
        assert_eq!(code, 1, "{out}");
        assert!(out.contains("uses the name"), "{out}");
        assert_eq!(read(&paths.patch_file), text);
        memory.close();
    }

    #[test]
    fn dsh_home_moves_every_file_and_nothing_goes_elsewhere() {
        let home = TempDir::new("dsh-own-home");
        let data = TempDir::new("dsh-own-data");
        let own = home.path().join("tools").join("dsh home");
        let text = home.path().display().to_string();
        let own_text = own.display().to_string();
        let env = |name: &str| match name {
            "HOME" | "USERPROFILE" => Some(text.clone()),
            "DSH_HOME" => Some(own_text.clone()),
            _ => None,
        };
        let mut setup = machine(home.path(), data.path());
        setup.set_dsh(DshPaths::resolve(&env).unwrap());
        // A file of the user's own in the moved home: given back byte for byte.
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("cordis.patch.yml"), fixture::USER_PATCH).unwrap();
        let before = snapshot(home.path());
        let memory = Memory::open(data.path());
        let (code, out) = run_install(&setup, &memory, into_dsh());
        assert_eq!(code, 0, "{out}");
        let installed = read(&own.join("cordis.patch.yml"));
        assert!(installed.contains("serverName: graphersal-memory"));
        assert!(read(&own.join("AGENTS.md")).contains("## Memory: graphersal-memory"));
        assert!(!home.path().join(".dsh").exists());
        nothing_elsewhere(home.path());
        // Both rows give dsh's child the moved home back (dsh drops DSH_* from its env).
        let found = yamledit::find(&installed).unwrap().unwrap();
        let items = yamledit::items(&found.body).unwrap();
        let rows = inserted(&items);
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(
                row["config"]["env"]["DSH_HOME"],
                own_text.as_str(),
                "{installed}"
            );
            assert_eq!(row["config"]["env"]["GRAPHERSAL_MEMORY_HARNESS"], "dsh");
        }
        assert_eq!(inspect(&setup).use_row, PartState::Installed);
        // A second install and a refresh change nothing.
        let (code, out) = run_install(&setup, &memory, into_dsh());
        assert_eq!(code, 0, "{out}");
        let (code, out) = run_install(&setup, &memory, crate::install::install_tests::refresh());
        assert_eq!(code, 0, "{out}");
        assert_eq!(read(&own.join("cordis.patch.yml")), installed);
        let (code, out) = run_uninstall(&setup, out_of_dsh());
        assert_eq!(code, 0, "{out}");
        assert_eq!(snapshot(home.path()), before, "{out}");
        memory.close();
    }

    #[test]
    fn the_default_home_adds_no_dsh_home_to_the_rows() {
        let tmp = TempDir::new("dsh-default-env");
        let setup = bare_setup_in(tmp.path(), tmp.path());
        assert!(!setup.dsh().explicit_home);
        let body = block_body(&setup, false);
        assert!(!body.contains("DSH_HOME"), "{body}");
        assert!(body.contains("        env:\n          GRAPHERSAL_MEMORY_HARNESS: dsh\n        startupTimeoutMs: 60000\n"), "{body}");
    }

    #[test]
    fn status_and_doctor_note_the_session_log_upload_and_name_clashes() {
        let home = TempDir::new("dsh-notes-home");
        let data = TempDir::new("dsh-notes-data");
        fixture::user_files(home.path());
        let setup = machine(home.path(), data.path());
        let paths = setup.dsh().clone();
        let memory = Memory::open(data.path());
        run_install(&setup, &memory, into_dsh());
        let status = status_lines(&setup).join("\n");
        assert!(status.contains("session-log-deepseek"), "{status}");
        assert!(status.contains("note:"), "{status}");
        let findings = doctor(&setup);
        let privacy = findings
            .iter()
            .find(|f| f.text.contains("session-log-deepseek"))
            .unwrap();
        assert!(!privacy.problem, "a note, not a problem");
        assert!(findings.iter().all(|f| !f.problem), "{findings:?}");
        assert!(!findings.iter().any(|f| f.text.contains("MCP manager")));

        // Switched off in a profile, a server of the manager named like ours, sdk-minimal.
        let web = paths.profiles.join("web").join("cordis.patch.yml");
        std::fs::write(
            &web,
            format!(
                "{}- id: session-log-deepseek\n  config:\n    enabled: false\n",
                fixture::WEB_PATCH.replace("      github:", "      graphersal-memory:")
            ),
        )
        .unwrap();
        std::fs::create_dir_all(paths.profiles.join("sdk-minimal")).unwrap();
        let status = status_lines(&setup).join("\n");
        assert!(status.contains("off in the profile web layer"), "{status}");
        let findings = doctor(&setup);
        assert!(
            !findings.iter().any(|f| f.text.contains("is on: with")),
            "{findings:?}"
        );
        assert!(
            findings
                .iter()
                .any(|f| f.text.contains("profile web layer's MCP manager") && !f.problem),
            "{findings:?}"
        );
        assert!(findings.iter().any(|f| f.text.contains("sdk-minimal")));
        // The home layer's switch counts too, in either form.
        assert!(switches_session_log_off(&[
            json!({"id": "session-log-deepseek", "disabled": true})
        ]));
        assert!(!switches_session_log_off(&[
            json!({"id": "session-log-deepseek", "config": {"enabled": true}})
        ]));
        // Without the program: a note on how to install it.
        let bare = bare_setup_in(home.path(), data.path());
        let findings = doctor(&bare);
        assert!(
            findings.iter().any(|f| f
                .fix
                .as_deref()
                .is_some_and(|fix| fix.contains("npm install -g @deepseek-ai/dsh"))),
            "{findings:?}"
        );
        run_uninstall(&setup, out_of_dsh());
        memory.close();
    }

    #[test]
    fn the_rule_blocks_name_dshs_tool_names() {
        for block in [RULE_BLOCK, RULE_BLOCK_ANY] {
            assert!(block.contains("mcp__graphersal-memory__memories"));
            assert!(!block.contains("tool_search"));
        }
        assert!(
            crate::harnesses::dsh::tool_names()
                .contains(&"mcp__graphersal-memory__memories".to_string())
        );
        let text = wrap_block(RULE_BLOCK);
        let found = find_block(&text, Language::English).unwrap().unwrap();
        assert!(found.intact && found.current);
    }
}
