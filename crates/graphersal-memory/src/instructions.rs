//! Memories against the instruction files the harnesses load (CLAUDE.md, AGENTS.md, rules
//! files): the conflict check of `docs/how-it-works.md#memories-against-instruction-files`.
//!
//! The rules of those files are never imported as memories (the harness loads the files
//! itself, so a rule would reach the agent twice) and the files are never edited. Each rule (a
//! list item, a paragraph or a line that is a directive, [`rules_in_file`]: descriptions of the
//! project are no rules) becomes an `instruction` vertex, known by a
//! fingerprint of its project, its file and its text ([`rule_id`]): a moved line keeps its
//! vertex, a changed or removed one is dropped with its candidates and decisions. The catalog
//! functions `sync_instructions` and `scan_instructions` (and `remember`, for a memory whose
//! text changed) find the memories near each rule; the agent or the user decides with `settle`
//! (`rule`), the decisions are `compared_with` edges.
//!
//! Who does what: the product side (the shim at a session's start, the session-start hooks,
//! `import`) knows the harnesses and lists the files ([`files`]); it sends them in the control
//! request [`CONTROL`] unless they are unchanged since the last complete sync ([`sync`], state
//! in `<data dir>/import/instructions.json`). The daemon reads the files ([`read_rules`]),
//! syncs the vertices and answers; the scan for candidates runs in the daemon's background
//! (rounds of [`SCAN_BUDGET`], batches of [`SCAN_BATCH`] rules), so a session start never waits
//! for it. Only local files are read, and only to compare them with the memories.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::agents::Kind;
use crate::install::Setup;

/// The control request that syncs a project's instruction files (`crate::daemon`).
pub const CONTROL: &str = "sync_instructions";

/// How long one background scan round of a project lasts (then other queued projects get
/// their turn, and the project another round).
pub const SCAN_BUDGET: Duration = Duration::from_secs(3);

/// How many rules one `scan_instructions` call scans (one job on the store's session thread,
/// so the agent's calls are not held up for long).
pub const SCAN_BATCH: u64 = 10;

/// A rule shorter than this many characters is no rule (a label, a link, a stray word).
const MIN_RULE_CHARS: usize = 20;

/// A rule with fewer words than this is no rule either.
const MIN_RULE_WORDS: usize = 4;

/// The longest rule text kept, in characters (a longer paragraph is cut, then "…").
const MAX_RULE_CHARS: usize = 400;

/// The most rules read from one file.
const MAX_RULES_PER_FILE: usize = 300;

/// The most rules of one sync (a project's files and the user-level ones).
const MAX_RULES: usize = 800;

/// The largest instruction file read, in bytes.
const MAX_FILE_BYTES: u64 = 1 << 20;

/// An instruction file a harness loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSpec {
    /// The file.
    pub path: PathBuf,
    /// A user-level file (loaded in every project): its rules are global.
    pub global: bool,
    /// The harnesses that load it.
    pub loaded_by: Vec<Kind>,
}

/// The harnesses whose instruction files count: those graphersal-memory is installed in, else
/// the one named by `harness` (the environment's, `crate::shim::harness_name`), else none.
pub fn kinds(setup: &Setup, harness: &str) -> Vec<Kind> {
    let mut kinds = setup.installed();
    if kinds.is_empty()
        && let Some(kind) = Kind::from_id(harness)
    {
        kinds.push(kind);
    }
    kinds
}

/// The instruction files the harnesses `kinds` load: the user-level ones and those of
/// `project` (its root). graphersal-memory's own files are left out (its rules files and the
/// exported global memories, `crate::coexist::instruction_files`); its managed block in a
/// shared file is left out by [`rules_in`].
pub fn files(setup: &Setup, kinds: &[Kind], project: Option<&Path>) -> Vec<FileSpec> {
    let user: Vec<PathBuf> = crate::coexist::instruction_files(setup, None)
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    let all = match project {
        Some(root) => crate::coexist::instruction_files(setup, Some(root)),
        None => crate::coexist::instruction_files(setup, None),
    };
    all.into_iter()
        .filter_map(|(path, loaded)| {
            let loaded_by: Vec<Kind> = loaded.into_iter().filter(|k| kinds.contains(k)).collect();
            (!loaded_by.is_empty()).then(|| FileSpec {
                global: user.contains(&path),
                path,
                loaded_by,
            })
        })
        .collect()
}

/// The rules of an instruction file's text as (first line, text): [`rules_in_file`] of a file
/// that is not a rules file by its path (the tests' view).
#[cfg(test)]
pub fn rules_in(text: &str) -> Vec<(usize, String)> {
    rules_in_file(text, false)
}

/// The rules of an instruction file's text as (first line, text): list items, paragraphs and
/// block quotes, a wrapped line joined to its item, the list marker taken off, that are
/// DIRECTIVES ([`is_directive`]): a unit with an English directive marker ("never", "always",
/// "must", "do not", "prefer", "instead", ...), or any unit under a heading that names rules
/// ("Rules", "Conventions", "Guidelines", "Do", "Don't", "Main rule", [`is_rules_heading`]),
/// or any unit of a rules file (`rules_file`: a file in a rules directory, Cursor's `.mdc`,
/// `.cursorrules`; or a text with a front matter, as rules files have). Descriptions ("`io`:
/// GraphSON 3.0 ..."), table rows, headings and link lists are no rules: comparing them with
/// the memories found only lines on the same topic, never a contradiction.
///
/// Limitation: the markers are English words. A line in another language (or English without a
/// marker) counts only under a rules-like heading or in a rules file; elsewhere it is skipped.
///
/// Left out besides: headings, tables, code blocks, HTML comments, a front matter (`---` ...
/// `---`, as in Cursor's `.mdc` files), graphersal-memory's managed block, and units shorter
/// than [`MIN_RULE_CHARS`] characters or [`MIN_RULE_WORDS`] words. A text longer than
/// [`MAX_RULE_CHARS`] is cut.
pub fn rules_in_file(text: &str, rules_file: bool) -> Vec<(usize, String)> {
    let mut rules = Vec::new();
    let mut current: Vec<(usize, &str)> = Vec::new();
    // The open headings (level, names rules): a unit is under a rules heading when any of them
    // names rules.
    let mut headings: Vec<(usize, bool)> = Vec::new();
    let mut in_front = text.lines().next().is_some_and(|l| l.trim() == "---");
    let all_rules = rules_file || in_front;
    let flush = |current: &mut Vec<(usize, &str)>,
                 rules: &mut Vec<(usize, String)>,
                 headings: &[(usize, bool)]| {
        let ruled = all_rules || headings.iter().any(|(_, rules)| *rules);
        if let Some(rule) = unit_text(current)
            && is_directive(&rule.1, ruled)
        {
            rules.push(rule);
        }
        current.clear();
    };
    let (mut in_block, mut in_code, mut in_comment) = (false, false, false);
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if in_front {
            if i > 0 && trimmed == "---" {
                in_front = false;
            }
            continue;
        }
        if trimmed.starts_with(crate::install::BLOCK_BEGIN) {
            flush(&mut current, &mut rules, &headings);
            in_block = true;
            continue;
        }
        if in_block {
            if trimmed.starts_with(crate::install::BLOCK_END) {
                in_block = false;
            }
            continue;
        }
        if in_comment {
            if trimmed.contains("-->") {
                in_comment = false;
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush(&mut current, &mut rules, &headings);
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        if trimmed.starts_with("<!--") {
            flush(&mut current, &mut rules, &headings);
            in_comment = !trimmed.contains("-->");
            continue;
        }
        if trimmed.starts_with('#') {
            flush(&mut current, &mut rules, &headings);
            let level = trimmed.chars().take_while(|c| *c == '#').count();
            while headings.last().is_some_and(|(l, _)| *l >= level) {
                headings.pop();
            }
            headings.push((level, is_rules_heading(&trimmed[level..])));
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('|') {
            flush(&mut current, &mut rules, &headings);
            continue;
        }
        if crate::coexist::starts_list_item(trimmed) || trimmed.starts_with('>') {
            flush(&mut current, &mut rules, &headings);
        }
        current.push((i + 1, line));
    }
    flush(&mut current, &mut rules, &headings);
    rules
}

/// The words of a heading that names rules (any of them, case-insensitive, as a whole word):
/// "Rules", "Conventions", "Guidelines", "Do", "Don't", "Main rule", "Policies", ...
const RULES_HEADING_WORDS: [&str; 17] = [
    "rule",
    "rules",
    "convention",
    "conventions",
    "guideline",
    "guidelines",
    "do",
    "dos",
    "don't",
    "dont",
    "don'ts",
    "donts",
    "do's",
    "policy",
    "policies",
    "requirements",
    "constraints",
];

/// Whether a heading's text (without its `#`s) names rules ([`RULES_HEADING_WORDS`]).
pub fn is_rules_heading(heading: &str) -> bool {
    words_of(heading)
        .iter()
        .any(|w| RULES_HEADING_WORDS.contains(&w.as_str()))
}

/// The English directive markers, each a whole word (or words) of a unit, case-insensitive.
const DIRECTIVE_WORDS: [&str; 21] = [
    "never",
    "always",
    "must",
    "mustn't",
    "should",
    "shouldn't",
    "don't",
    "dont",
    "do not",
    "prefer",
    "avoid",
    "instead",
    "required",
    "forbidden",
    "prohibited",
    "make sure",
    "only when",
    "only if",
    "only after",
    "only through",
    "only via",
];

/// Whether a unit's text is a directive: under a rules heading (or in a rules file, `ruled`)
/// every unit that is not a mere link list, elsewhere a unit in English (a majority of ASCII
/// letters) with a directive marker ([`DIRECTIVE_WORDS`], or "no" opening a sentence: "No
/// panics in step paths").
pub fn is_directive(text: &str, ruled: bool) -> bool {
    if is_link_list(text) {
        return false;
    }
    if ruled {
        return true;
    }
    let letters = text.chars().filter(|c| c.is_alphabetic()).count();
    let ascii = text.chars().filter(char::is_ascii_alphabetic).count();
    if ascii * 2 <= letters {
        return false;
    }
    let words = words_of(text);
    let has = |marker: &str| {
        let parts: Vec<&str> = marker.split(' ').collect();
        words
            .windows(parts.len())
            .any(|w| w.iter().zip(&parts).all(|(a, b)| a == b))
    };
    if DIRECTIVE_WORDS.iter().any(|m| has(m)) {
        return true;
    }
    // "No <noun> ...": "no" opening the text or a sentence.
    let lower = text.to_lowercase().replace('\u{2019}', "'");
    lower.starts_with("no ")
        || lower
            .split(['.', '!', '?', ':', ';'])
            .skip(1)
            .any(|s| s.trim_start().starts_with("no "))
}

/// The lowercase words of `text` (letters, digits and apostrophes; a curly apostrophe made
/// straight), for whole-word matching.
fn words_of(text: &str) -> Vec<String> {
    text.to_lowercase()
        .replace('\u{2019}', "'")
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .map(|w| w.trim_matches('\''))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether a unit is only links (`[text](url)` and bare URLs, with less than
/// [`MIN_RULE_WORDS`] words besides): a link list, no rule.
fn is_link_list(text: &str) -> bool {
    let mut rest = String::new();
    let mut skip_to = 0;
    for (i, c) in text.char_indices() {
        if i < skip_to {
            continue;
        }
        if c == '['
            && let Some(close) = text[i..].find("](")
            && let Some(end) = text[i + close..].find(')')
        {
            skip_to = i + close + end + 1;
            continue;
        }
        rest.push(c);
    }
    let words = rest
        .split_whitespace()
        .filter(|w| !w.starts_with("http://") && !w.starts_with("https://"))
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count();
    words < MIN_RULE_WORDS
}

/// The rule of one unit's lines (`None` when it is too short to be one).
fn unit_text(lines: &[(usize, &str)]) -> Option<(usize, String)> {
    let first = lines.first()?.0;
    let joined = lines
        .iter()
        .map(|(_, line)| line.trim())
        .collect::<Vec<_>>()
        .join(" ");
    let text = strip_marker(&joined)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if text.chars().count() < MIN_RULE_CHARS || text.split_whitespace().count() < MIN_RULE_WORDS {
        return None;
    }
    if text.chars().count() > MAX_RULE_CHARS {
        let cut: String = text.chars().take(MAX_RULE_CHARS - 1).collect();
        return Some((first, format!("{}…", cut.trim_end())));
    }
    Some((first, text))
}

/// `text` without its list or quote marker (`- `, `* `, `+ `, `1. `, `1) `, `> `) and a task
/// box (`[ ] `, `[x] `).
fn strip_marker(text: &str) -> &str {
    let mut rest = text.trim_start();
    while let Some(after) = rest.strip_prefix('>') {
        rest = after.trim_start();
    }
    for marker in ["- ", "* ", "+ "] {
        if let Some(after) = rest.strip_prefix(marker) {
            rest = after.trim_start();
            break;
        }
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && (rest[digits..].starts_with(". ") || rest[digits..].starts_with(") ")) {
        rest = rest[digits + 2..].trim_start();
    }
    for task in ["[ ] ", "[x] ", "[X] "] {
        if let Some(after) = rest.strip_prefix(task) {
            rest = after;
            break;
        }
    }
    rest
}

/// The vertex id of a rule: `instruction:` + a fingerprint of the project (`""` for a
/// user-level file), the file's key (a project file's path relative to the project root, so
/// two checkouts of one project share their rules; a user-level file's path) and the text
/// with its white space collapsed.
pub fn rule_id(project: &str, key: &str, text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let key = key.replace('\\', "/");
    format!(
        "instruction:{}",
        crate::fsutil::fingerprint(format!("{project}\n{key}\n{text}").as_bytes())
    )
}

/// The rules of the files of a sync request (`files`: `{path, global, loaded_by}`; `root`:
/// the project's root, `project`: its stable id), as `sync_instructions` takes them, and notes
/// on files that could not be read. Only Markdown files (`.md`, `.mdc`, `.cursorrules`) up to
/// [`MAX_FILE_BYTES`] are read.
pub fn read_rules(files: &[Value], root: &str, project: &str) -> (Vec<Value>, Vec<String>) {
    let mut rules = Vec::new();
    let mut notes = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for file in files {
        let Some(path) = file["path"].as_str().map(PathBuf::from) else {
            continue;
        };
        let markdown = path.extension().is_some_and(|e| e == "md" || e == "mdc")
            || path.file_name().is_some_and(|n| n == ".cursorrules");
        if !markdown {
            notes.push(format!("{} is not an instruction file", path.display()));
            continue;
        }
        // Without a project every file counts as a user-level one.
        let global = file["global"] == true || project.is_empty();
        match std::fs::metadata(&path) {
            Ok(meta) if meta.len() > MAX_FILE_BYTES => {
                notes.push(format!("{} is larger than 1 MiB: left out", path.display()));
                continue;
            }
            Ok(_) => {}
            Err(_) => continue,
        }
        let text = match crate::install::read_text(&path) {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            Err(err) => {
                notes.push(err);
                continue;
            }
        };
        let key = if global {
            path.display().to_string()
        } else {
            path.strip_prefix(root)
                .map(|rel| rel.display().to_string())
                .unwrap_or_else(|_| path.display().to_string())
        };
        let loaded_by = file["loaded_by"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(",");
        let scope = if global { "" } else { project };
        for (line, text) in rules_in_file(&text, is_rules_file(&path))
            .into_iter()
            .take(MAX_RULES_PER_FILE)
        {
            let id = rule_id(scope, &key, &text);
            if !seen.insert(id.clone()) {
                continue;
            }
            if rules.len() >= MAX_RULES {
                notes.push(format!(
                    "more than {MAX_RULES} rules in the instruction files: the rest is left out"
                ));
                return (rules, notes);
            }
            rules.push(
                json!({"id": id, "text": text, "file": path.display().to_string(),
                              "line": line, "loaded_by": loaded_by, "global": global}),
            );
        }
    }
    (rules, notes)
}

/// Whether `path` is a rules file, every unit of which is a rule: a file in a `rules` directory
/// (`.claude/rules/`, `.cursor/rules/`, opencode's), a Cursor `.mdc` file, `.cursorrules`.
fn is_rules_file(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "mdc")
        || path.file_name().is_some_and(|n| n == ".cursorrules")
        || path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|n| n == "rules")
}

/// The control request that syncs the instruction files `files` of the project at `root`
/// (`""`: no project, the user-level files only).
pub fn control_line(root: &str, files: &[FileSpec]) -> Value {
    let files: Vec<Value> = files
        .iter()
        .map(|f| {
            json!({"path": f.path.display().to_string(), "global": f.global,
                   "loaded_by": f.loaded_by.iter().map(|k| k.id()).collect::<Vec<_>>()})
        })
        .collect();
    json!({"protocol": crate::protocol::PROTOCOL, "control": CONTROL, "project": root,
           "files": files})
}

/// A fingerprint of the files' paths, scopes, harnesses and texts: unchanged files need no
/// sync.
pub fn fingerprint(files: &[FileSpec]) -> String {
    let mut all = Vec::new();
    for file in files {
        all.extend_from_slice(file.path.display().to_string().as_bytes());
        all.push(u8::from(file.global));
        for kind in &file.loaded_by {
            all.extend_from_slice(kind.id().as_bytes());
        }
        all.push(0);
        if let Ok(Some(bytes)) = crate::fsutil::read_optional(&file.path) {
            all.extend_from_slice(crate::fsutil::fingerprint(&bytes).as_bytes());
        }
        all.push(0);
    }
    crate::fsutil::fingerprint(&all)
}

/// What a sync did (the daemon's answer).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Synced {
    /// The rules read.
    pub rules: u64,
    /// New rules.
    pub created: u64,
    /// Removed (changed or deleted) rules.
    pub removed: u64,
    /// Rules not yet compared with the memories when the daemon answered (it goes on in the
    /// background).
    pub pending: u64,
    /// Notes on files that could not be read.
    pub notes: Vec<String>,
}

impl Synced {
    /// The answer read back.
    pub fn from_answer(answer: &Value) -> Result<Self, String> {
        if answer["ok"] != Value::Bool(true) {
            return Err(answer["error"]["message"]
                .as_str()
                .unwrap_or("the daemon refused the instruction file sync")
                .to_string());
        }
        let n = |key: &str| answer[key].as_u64().unwrap_or(0);
        Ok(Self {
            rules: n("rules"),
            created: n("created"),
            removed: n("removed"),
            pending: n("pending"),
            notes: answer["notes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
        })
    }

    /// The line `import` prints.
    pub fn line(&self) -> String {
        let mut line = format!(
            "instruction files: {} rule{} ({} new, {} removed)",
            self.rules,
            if self.rules == 1 { "" } else { "s" },
            self.created,
            self.removed,
        );
        if self.pending > 0 {
            line.push_str(&format!(
                "; {} rule{} being compared with the memories in the background (the conflict \
                 check: conflicts(), or the UI's Conflicts tab)",
                self.pending,
                if self.pending == 1 { "" } else { "s" }
            ));
        }
        line
    }
}

/// The version of the record of the last syncs ([`State`]): 2 since only directives are rules.
const STATE_VERSION: u64 = 2;

/// The record of the last syncs, `<data dir>/import/instructions.json`: per project root
/// (`""`: none) the fingerprint of the files and whether that sync finished its scan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct State(BTreeMap<String, (String, bool)>);

impl State {
    fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("import").join("instructions.json")
    }

    fn load(data_dir: &Path) -> Self {
        let Ok(Some(bytes)) = crate::fsutil::read_optional(&Self::path(data_dir)) else {
            return Self::default();
        };
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        // A record of another version (version 1: before only directives were rules) is no
        // record: every project's files are synced again, so the vertices of lines that are no
        // rules any more go with their candidates.
        if value["version"] != STATE_VERSION {
            return Self::default();
        }
        Self(
            value["projects"]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(root, entry)| {
                    Some((
                        root.clone(),
                        (
                            entry["fingerprint"].as_str()?.to_string(),
                            entry["complete"] == true,
                        ),
                    ))
                })
                .collect(),
        )
    }

    fn save(&self, data_dir: &Path) -> Result<(), String> {
        let projects: serde_json::Map<String, Value> = self
            .0
            .iter()
            .map(|(root, (fingerprint, complete))| {
                (
                    root.clone(),
                    json!({"fingerprint": fingerprint, "complete": complete}),
                )
            })
            .collect();
        let path = Self::path(data_dir);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
        }
        let text =
            serde_json::to_string_pretty(&json!({"version": STATE_VERSION, "projects": projects}))
                .map_err(|err| err.to_string())?;
        crate::fsutil::write_atomic(&path, format!("{text}\n").as_bytes())
    }
}

/// The marker of the one-time rescan of a store's instruction lines ([`rescan_once`]), in
/// `<data dir>/import/`, followed by the store's name.
const RESCANNED_PREFIX: &str = "instructions-rescanned-";

/// Once per store (graphersal-memory 2026-11-14, when only directive lines became rules and the
/// candidate bar went up): the candidates still waiting for their check (`compared_with` edges
/// with decision "unchecked") are dropped and every instruction line is scanned again under the
/// new bar; settled decisions (compatible, conflict) stay. The lines that are no rules any more
/// go at the next sync of their project (the sync record's version changed, [`STATE_VERSION`]).
/// A marker file keeps it from running again. Returns how many candidates it dropped.
pub fn rescan_once(
    data_dir: &Path,
    store: &str,
    call: &dyn Fn(&str, Value) -> Result<Value, String>,
) -> Result<u64, String> {
    let dir = data_dir.join("import");
    let marker = dir.join(format!("{RESCANNED_PREFIX}{store}"));
    if marker.exists() {
        return Ok(0);
    }
    let dropped = call("rescan_instructions", json!({}))?["dropped"]
        .as_u64()
        .unwrap_or(0);
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    crate::fsutil::write_atomic(&marker, format!("{dropped}\n").as_bytes())?;
    Ok(dropped)
}

/// Sends a control request to the daemon and returns its answer.
pub type Send<'a> = dyn FnMut(&Value) -> Result<Value, String> + 'a;

/// Syncs the instruction files `files` of the project at `root` through `send`: nothing
/// (`None`) when they are unchanged since the last sync that finished its scan, unless `force`.
pub fn sync(
    data_dir: &Path,
    root: &str,
    files: &[FileSpec],
    force: bool,
    send: &mut Send<'_>,
) -> Result<Option<Synced>, String> {
    let fingerprint = fingerprint(files);
    let mut state = State::load(data_dir);
    if !force && state.0.get(root) == Some(&(fingerprint.clone(), true)) {
        return Ok(None);
    }
    let synced = Synced::from_answer(&send(&control_line(root, files))?)?;
    state
        .0
        .insert(root.to_string(), (fingerprint, synced.pending == 0));
    state.save(data_dir)?;
    Ok(Some(synced))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install_tests::setup_in;
    use crate::mcp_tests::TempDir;

    #[test]
    fn rules_are_list_items_paragraphs_and_quotes() {
        let text = "# Project rules\n\
                    \n\
                    Always run the test suite before you commit anything.\n\
                    It takes a minute.\n\
                    \n\
                    - Use npm for every package installation here.\n\
                    \x20 A wrapped second line of the same item.\n\
                    * Never push to the main branch without asking first.\n\
                    1. Write commit messages in the imperative mood.\n\
                    - [ ] Keep the changelog current for every release.\n\
                    > Prefer small pull requests over big ones.\n\
                    - Short item\n\
                    | Need | Command used for it |\n\
                    |---|---|\n";
        let rules = rules_in(text);
        assert_eq!(
            rules,
            vec![
                (
                    3,
                    "Always run the test suite before you commit anything. It takes a minute."
                        .to_string()
                ),
                (
                    6,
                    "Use npm for every package installation here. A wrapped second line of the \
                     same item."
                        .to_string()
                ),
                (
                    8,
                    "Never push to the main branch without asking first.".to_string()
                ),
                (
                    9,
                    "Write commit messages in the imperative mood.".to_string()
                ),
                (
                    10,
                    "Keep the changelog current for every release.".to_string()
                ),
                (11, "Prefer small pull requests over big ones.".to_string()),
            ]
        );
    }

    /// A copy-shaped excerpt of a real project CLAUDE.md: descriptions of the workspace and the
    /// feature flags, a commands block, a table, and a numbered "## Rules" section.
    const REAL_CLAUDE_MD: &str = r#"# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

**Graphersal** is an in-memory graph traversal engine in Rust modelled on Apache TinkerPop Gremlin. Keep this file short: detail of a subsystem goes into its nested file, not here.

| Nested guidance | Covers |
|---|---|
| `crates/graphersal/src/graph/CLAUDE.md` | storage, handles, custom storages |

## Workspace

- `crates/graphersal`: the core library. Almost all work happens here.
- `crates/graphersal-cli`: the REPL, a one-shot scriptable query runner, `graphersal store ...`, and `--server`.
- `crates/graphersal-session`: the playground session logic (JSON in, JSON out), shared by `graphersal-wasm` and the dev server; serves any storage. Publishable but internal.
- `book/`: mdBook docs.

`graphersal` feature flags. All are off by default and must stay strictly additive:
- `io`: GraphSON 3.0 (primary exchange format, dependency-free) and GraphML (`quick-xml`); neither carries the schema.
- `script`: the Rhai DSL.
- `display`: tables via `tabled`. Never pull `tabled` into `script` or the engine.
- `serde`: `ChangeSet` serialization and the `json` visualizer. It is not a graph file format.

The workspace is on Rust edition 2024 (MSRV 1.88).

## Commands

```bash
cargo fmt   --all --check                       # rustfmt.toml: max_width = 100, stable options only
cargo test  --package graphersal --features script,io,display,serde,persist,gql
```

Use `web_time::Instant`, never `std::time::Instant` (it panics on wasm32-unknown-unknown).

## Rules

1. **No panics in step or script paths.** Don't use `unwrap`/`expect`/`panic!`. Map every failure to a `TraverserError` variant.
2. **Keep DSL parity.** Any change to a Rust step also updates its Rhai binding.
3. **Evaluate lazily; don't allocate per item.** Steps must not allocate `HashMap` keys for each traverser.
9. **Library code logs through `tracing` macros.** Never `println!`/`eprintln!`/`dbg!`.
12. **English** in every on-disk artifact: code, comments, and docs. Chat may be in any language.
17. **One integration-test binary.** Every integration test lives in `tests/all/<name>.rs`.
"#;

    #[test]
    fn a_real_claude_md_keeps_its_rules_and_skips_its_descriptions() {
        let rules: Vec<(usize, String)> = rules_in(REAL_CLAUDE_MD);
        let texts: Vec<&str> = rules.iter().map(|(_, t)| t.as_str()).collect();
        let starts = |prefix: &str| texts.iter().any(|t| t.starts_with(prefix));
        // Every numbered item of "## Rules" is kept, with or without a marker word.
        for rule in [
            "**No panics in step or script paths.**",
            "**Keep DSL parity.**",
            "**Evaluate lazily; don't allocate per item.**",
            "**Library code logs through `tracing` macros.**",
            "**English** in every on-disk artifact",
            "**One integration-test binary.**",
        ] {
            assert!(starts(rule), "{rule} missing: {texts:#?}");
        }
        // Directive lines elsewhere are kept.
        assert!(
            starts("`display`: tables via `tabled`. Never pull"),
            "{texts:#?}"
        );
        assert!(starts("Use `web_time::Instant`, never"), "{texts:#?}");
        assert!(
            starts("`graphersal` feature flags. All are off by default and must"),
            "{texts:#?}"
        );
        // Descriptions are skipped.
        for description in [
            "This file provides guidance",
            "`crates/graphersal`: the core library.",
            "`crates/graphersal-cli`:",
            "`crates/graphersal-session`:",
            "`io`: GraphSON 3.0",
            "`script`: the Rhai DSL.",
            "`serde`:",
            "The workspace is on Rust edition 2024",
            "`book/`",
        ] {
            assert!(!starts(description), "{description} kept: {texts:#?}");
        }
        assert_eq!(texts.len(), 9, "{texts:#?}");
    }

    #[test]
    fn directive_markers_are_whole_words_and_other_languages_need_a_rules_heading() {
        for text in [
            "Never push to the main branch without asking.",
            "You MUST run the tests before every commit.",
            "Don\u{2019}t commit generated files to the repository.",
            "Do not edit the vendored files by hand.",
            "Prefer small pull requests over big ones.",
            "Avoid global state in the test binaries.",
            "Use pnpm instead of npm for the web packages.",
            "No public glob re-exports in the crate root.",
            "Grep the files only when the graph does not hold it.",
            "Signed commits are required on the release branch.",
        ] {
            assert!(is_directive(text, false), "{text}");
        }
        for text in [
            "Nevertheless the cache is rebuilt every night.",
            "The hallways of the build system are long and winding.",
            "The playground is a static page, native only.",
            "The server keeps no state between two requests.",
            "Использовать только pnpm для всех пакетов, никогда npm.",
            "[The book](book/README.md) and [the API](docs/api.md)",
        ] {
            assert!(!is_directive(text, false), "{text}");
        }
        // Under a rules heading every unit is a rule, in any language; a link list never.
        assert!(is_directive(
            "Использовать только pnpm для всех пакетов.",
            true
        ));
        assert!(is_directive(
            "Write commit messages in the imperative mood.",
            true
        ));
        assert!(!is_directive(
            "[The book](book/README.md) - [API](docs/api.md)",
            true
        ));
        let text = "# Notes\n\nКоммиты пишутся на английском языке всегда.\n\n## Правила и Rules\n\n\
                    - Коммиты пишутся на английском языке всегда.\n\n### Details\n\n\
                    - Write commit messages in the imperative mood.\n\n# Overview\n\n\
                    - Write commit messages in the imperative mood here.\n";
        let rules: Vec<usize> = rules_in(text).into_iter().map(|(line, _)| line).collect();
        assert_eq!(
            rules,
            vec![7, 11],
            "a subheading stays under its rules heading"
        );
        for heading in [
            " Rules",
            "## MAIN RULE: graph first",
            " Do and Don't",
            " Conventions",
        ] {
            assert!(is_rules_heading(heading), "{heading}");
        }
        for heading in [
            " Workspace",
            " Commands",
            " Local instructions",
            " Ruler sizes",
        ] {
            assert!(!is_rules_heading(heading), "{heading}");
        }
    }

    #[test]
    fn every_unit_of_a_rules_file_is_a_rule() {
        let text = "Write commit messages in the imperative mood.\n";
        assert!(rules_in(text).is_empty());
        assert_eq!(rules_in_file(text, true).len(), 1);
        assert!(is_rules_file(Path::new("/w/.claude/rules/style.md")));
        assert!(is_rules_file(Path::new("/w/.cursor/rules/go.mdc")));
        assert!(is_rules_file(Path::new("/w/.cursorrules")));
        assert!(!is_rules_file(Path::new("/w/CLAUDE.md")));
        let home = TempDir::new("ic-rules-file");
        let dir = home.path().join("rules");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("style.md"), text).unwrap();
        std::fs::write(home.path().join("CLAUDE.md"), text).unwrap();
        let spec = |path: PathBuf| json!({"path": path.display().to_string(), "global": true, "loaded_by": ["claude-code"]});
        let (rules, _) = read_rules(
            &[
                spec(dir.join("style.md")),
                spec(home.path().join("CLAUDE.md")),
            ],
            "",
            "",
        );
        assert_eq!(rules.len(), 1, "{rules:?}");
        assert!(rules[0]["file"].as_str().unwrap().ends_with("style.md"));
    }

    #[test]
    fn a_sync_record_of_the_old_version_is_synced_again() {
        let home = TempDir::new("ic-v1-home");
        let data = TempDir::new("ic-v1-data");
        let file = home.path().join("CLAUDE.md");
        std::fs::write(&file, "Always use npm for every package.\n").unwrap();
        let files = vec![FileSpec {
            path: file,
            global: false,
            loaded_by: vec![Kind::ClaudeCode],
        }];
        let mut send = |_: &Value| Ok(json!({"ok": true, "rules": 1, "pending": 0}));
        assert!(
            sync(data.path(), "/w", &files, false, &mut send)
                .unwrap()
                .is_some()
        );
        assert!(
            sync(data.path(), "/w", &files, false, &mut send)
                .unwrap()
                .is_none()
        );
        // The same record written by a binary before only directives were rules (version 1).
        let path = State::path(data.path());
        let old = std::fs::read_to_string(&path)
            .unwrap()
            .replace("\"version\": 2", "\"version\": 1");
        std::fs::write(&path, old).unwrap();
        assert!(
            sync(data.path(), "/w", &files, false, &mut send)
                .unwrap()
                .is_some()
        );
        assert!(
            sync(data.path(), "/w", &files, false, &mut send)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn the_rescan_runs_once_per_store() {
        let data = TempDir::new("ic-rescan");
        let calls = std::cell::RefCell::new(Vec::new());
        let call = |name: &str, _: Value| {
            calls.borrow_mut().push(name.to_string());
            Ok(json!({"dropped": 4, "lines": 9}))
        };
        assert_eq!(rescan_once(data.path(), "default", &call).unwrap(), 4);
        assert_eq!(rescan_once(data.path(), "default", &call).unwrap(), 0);
        assert_eq!(rescan_once(data.path(), "work", &call).unwrap(), 4);
        assert_eq!(
            *calls.borrow(),
            ["rescan_instructions", "rescan_instructions"]
        );
        // A failure leaves no marker: it runs again next time.
        let fail = |_: &str, _: Value| Err("the store failed".to_string());
        assert!(rescan_once(data.path(), "other", &fail).is_err());
        assert_eq!(rescan_once(data.path(), "other", &call).unwrap(), 4);
    }

    #[test]
    fn code_blocks_comments_front_matter_and_the_managed_block_are_left_out() {
        let text = format!(
            "---\n\
             description: always applied rule of this repository\n\
             alwaysApply: true\n\
             ---\n\
             Use tabs for indentation in every Go source file.\n\
             \n\
             ```sh\n\
             npm install --save-dev everything you can find\n\
             ```\n\
             <!-- a comment that says to use yarn for all the packages -->\n\
             <!--\n\
             a longer comment that says to use yarn for all the packages\n\
             -->\n\
             {} fingerprint=abc -->\n\
             Write memories into graphersal-memory, not into files.\n\
             {}\n\
             Answer in English even when the user writes in German.\n",
            crate::install::BLOCK_BEGIN,
            crate::install::BLOCK_END
        );
        let rules: Vec<String> = rules_in(&text).into_iter().map(|(_, t)| t).collect();
        assert_eq!(
            rules,
            vec![
                "Use tabs for indentation in every Go source file.".to_string(),
                "Answer in English even when the user writes in German.".to_string(),
            ]
        );
    }

    #[test]
    fn a_long_paragraph_is_cut() {
        let long = format!("Always {}", "word ".repeat(200));
        let rules = rules_in(&long);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].1.chars().count(), MAX_RULE_CHARS);
        assert!(rules[0].1.ends_with('…'));
    }

    #[test]
    fn rule_ids_follow_project_file_and_text_but_not_white_space() {
        let a = rule_id("p1", "CLAUDE.md", "Use npm  for packages");
        assert_eq!(a, rule_id("p1", "CLAUDE.md", "Use npm for packages"));
        assert!(a.starts_with("instruction:") && a.len() == "instruction:".len() + 16);
        assert_ne!(a, rule_id("p2", "CLAUDE.md", "Use npm for packages"));
        assert_ne!(a, rule_id("p1", "AGENTS.md", "Use npm for packages"));
        assert_ne!(a, rule_id("p1", "CLAUDE.md", "Use pnpm for packages"));
        assert_eq!(a, rule_id("p1", "CLAUDE.md", "Use npm for packages"));
    }

    #[test]
    fn the_files_are_those_of_the_installed_harnesses_without_ours() {
        let home = TempDir::new("ic-files-home");
        let data = TempDir::new("ic-files-data");
        let setup = setup_in(home.path(), data.path());
        let project = home.path().join("work");
        std::fs::create_dir_all(project.join(".claude").join("rules")).unwrap();
        std::fs::create_dir_all(project.join(".cursor").join("rules")).unwrap();
        std::fs::create_dir_all(setup.paths.config_dir.join("rules")).unwrap();
        std::fs::create_dir_all(&setup.codex.codex_home).unwrap();
        let write = |path: &Path| std::fs::write(path, "Use npm for every package.\n").unwrap();
        write(&setup.paths.instructions);
        write(&setup.paths.config_dir.join("rules").join("style.md"));
        write(
            &setup
                .paths
                .config_dir
                .join("rules")
                .join(crate::claude_code::GLOBAL_RULES_FILE),
        );
        write(&setup.codex.codex_home.join("AGENTS.md"));
        write(&project.join("CLAUDE.md"));
        write(&project.join("CLAUDE.local.md"));
        write(&project.join(".claude").join("CLAUDE.md"));
        write(&project.join(".claude").join("rules").join("testing.md"));
        write(&project.join("AGENTS.md"));
        write(&project.join(".cursor").join("rules").join("go.mdc"));

        let claude = files(&setup, &[Kind::ClaudeCode], Some(&project));
        let paths: Vec<&Path> = claude.iter().map(|f| f.path.as_path()).collect();
        assert_eq!(
            paths,
            vec![
                setup.paths.instructions.as_path(),
                setup
                    .paths
                    .config_dir
                    .join("rules")
                    .join("style.md")
                    .as_path(),
                project.join("CLAUDE.md").as_path(),
                project.join("CLAUDE.local.md").as_path(),
                project.join(".claude").join("CLAUDE.md").as_path(),
                project
                    .join(".claude")
                    .join("rules")
                    .join("testing.md")
                    .as_path(),
            ]
        );
        assert!(claude[0].global && claude[1].global && !claude[2].global);
        assert!(claude.iter().all(|f| f.loaded_by == vec![Kind::ClaudeCode]));

        let codex = files(&setup, &[Kind::Codex], Some(&project));
        let paths: Vec<&Path> = codex.iter().map(|f| f.path.as_path()).collect();
        assert_eq!(
            paths,
            vec![
                setup.codex.codex_home.join("AGENTS.md").as_path(),
                project.join("AGENTS.md").as_path()
            ]
        );
        let cursor = files(&setup, &[Kind::Cursor], Some(&project));
        assert!(
            cursor
                .iter()
                .any(|f| f.path == project.join(".cursor").join("rules").join("go.mdc")),
            "{cursor:?}"
        );
        assert!(files(&setup, &[], Some(&project)).is_empty());
        // Without a project: the user-level files only.
        let global = files(&setup, &[Kind::ClaudeCode], None);
        assert_eq!(global.len(), 2);
        assert!(global.iter().all(|f| f.global));
    }

    #[test]
    fn read_rules_keys_project_files_by_their_relative_path() {
        let home = TempDir::new("ic-read-home");
        let a = home.path().join("a");
        let b = home.path().join("b");
        for root in [&a, &b] {
            std::fs::create_dir_all(root).unwrap();
            std::fs::write(
                root.join("CLAUDE.md"),
                "# Rules\n\n- Always use npm for packages.\n- Run the tests before committing.\n",
            )
            .unwrap();
        }
        std::fs::write(
            home.path().join("notes.txt"),
            "Use pnpm for every package.\n",
        )
        .unwrap();
        let spec = |root: &Path| {
            vec![
                json!({"path": root.join("CLAUDE.md").display().to_string(), "global": false,
                       "loaded_by": ["claude-code"]}),
                json!({"path": home.path().join("notes.txt").display().to_string(),
                       "global": true, "loaded_by": ["claude-code"]}),
            ]
        };
        let (from_a, notes) = read_rules(&spec(&a), &a.display().to_string(), "proj-1");
        assert_eq!(from_a.len(), 2, "{from_a:?}");
        assert!(notes[0].contains("not an instruction file"), "{notes:?}");
        assert_eq!(from_a[0]["line"], 3);
        assert_eq!(from_a[0]["text"], "Always use npm for packages.");
        assert_eq!(from_a[0]["global"], false);
        assert_eq!(from_a[0]["loaded_by"], "claude-code");
        // Another checkout of the same project: the same rules.
        let (from_b, _) = read_rules(&spec(&b), &b.display().to_string(), "proj-1");
        assert_eq!(from_a[0]["id"], from_b[0]["id"]);
        assert_ne!(from_a[0]["file"], from_b[0]["file"]);
        // Without a project a project file is a user-level one.
        let (alone, _) = read_rules(&spec(&a)[..1], "", "");
        assert_eq!(alone[0]["global"], true);
    }

    #[test]
    fn an_unchanged_set_of_files_is_not_sent_again_until_its_scan_finished() {
        let home = TempDir::new("ic-sync-home");
        let data = TempDir::new("ic-sync-data");
        let file = home.path().join("CLAUDE.md");
        std::fs::write(&file, "Always use npm for every package.\n").unwrap();
        let files = vec![FileSpec {
            path: file.clone(),
            global: false,
            loaded_by: vec![Kind::ClaudeCode],
        }];
        let mut sent = Vec::new();
        let mut pending = 1;
        let mut send = |line: &Value| {
            sent.push(line.clone());
            Ok(json!({"ok": true, "rules": 1, "created": 1, "pending": pending}))
        };
        let first = sync(data.path(), "/w", &files, false, &mut send)
            .unwrap()
            .unwrap();
        assert_eq!(first.pending, 1);
        // The scan did not finish: sent again.
        assert!(
            sync(data.path(), "/w", &files, false, &mut send)
                .unwrap()
                .is_some()
        );
        pending = 0;
        let mut send = |line: &Value| {
            sent.push(line.clone());
            Ok(json!({"ok": true, "rules": 1, "pending": pending}))
        };
        assert!(
            sync(data.path(), "/w", &files, false, &mut send)
                .unwrap()
                .is_some()
        );
        // Finished and unchanged: nothing sent; forced, or changed: sent.
        assert!(
            sync(data.path(), "/w", &files, false, &mut send)
                .unwrap()
                .is_none()
        );
        assert!(
            sync(data.path(), "/w", &files, true, &mut send)
                .unwrap()
                .is_some()
        );
        std::fs::write(&file, "Always use pnpm for every package.\n").unwrap();
        assert!(
            sync(data.path(), "/w", &files, false, &mut send)
                .unwrap()
                .is_some()
        );
        assert_eq!(sent.len(), 5);
        assert_eq!(sent[0]["control"], CONTROL);
        assert_eq!(sent[0]["project"], "/w");
        assert_eq!(sent[0]["files"][0]["loaded_by"], json!(["claude-code"]));
        // A refusal is an error and records nothing.
        let mut refuse =
            |_: &Value| Ok(json!({"ok": false, "error": {"message": "the store failed"}}));
        let err = sync(data.path(), "/x", &files, false, &mut refuse).unwrap_err();
        assert!(err.contains("the store failed"), "{err}");
    }
}
