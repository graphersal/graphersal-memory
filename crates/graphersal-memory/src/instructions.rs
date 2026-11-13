//! Memories against the instruction files the harnesses load (CLAUDE.md, AGENTS.md, rules
//! files): the conflict check of `docs/how-it-works.md#memories-against-instruction-files`.
//!
//! The rules of those files are never imported as memories (the harness loads the files
//! itself, so a rule would reach the agent twice) and the files are never edited. Each rule (a
//! list item, a paragraph or a line, [`rules_in`]) becomes an `instruction` vertex, known by a
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
//! syncs the vertices and scans within [`SCAN_BUDGET`]; the rest of the scan follows at the
//! next sync. Only local files are read, and only to compare them with the memories.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::agents::Kind;
use crate::install::Setup;

/// The control request that syncs a project's instruction files (`crate::daemon`).
pub const CONTROL: &str = "sync_instructions";

/// How long the daemon scans for candidates in one sync; the rest follows at the next sync.
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

/// The rules of an instruction file's text as (first line, text): list items, paragraphs and
/// block quotes, a wrapped line joined to its item, the list marker taken off. Left out:
/// headings, tables, code blocks, HTML comments, a front matter (`---` ... `---`, as in
/// Cursor's `.mdc` files), graphersal-memory's managed block, and units shorter than
/// [`MIN_RULE_CHARS`] characters or [`MIN_RULE_WORDS`] words. A text longer than
/// [`MAX_RULE_CHARS`] is cut.
pub fn rules_in(text: &str) -> Vec<(usize, String)> {
    let mut rules = Vec::new();
    let mut current: Vec<(usize, &str)> = Vec::new();
    let flush = |current: &mut Vec<(usize, &str)>, rules: &mut Vec<(usize, String)>| {
        if let Some(rule) = unit_text(current) {
            rules.push(rule);
        }
        current.clear();
    };
    let (mut in_block, mut in_code, mut in_comment) = (false, false, false);
    let mut in_front = text.lines().next().is_some_and(|l| l.trim() == "---");
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if in_front {
            if i > 0 && trimmed == "---" {
                in_front = false;
            }
            continue;
        }
        if trimmed.starts_with(crate::install::BLOCK_BEGIN) {
            flush(&mut current, &mut rules);
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
            flush(&mut current, &mut rules);
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        if trimmed.starts_with("<!--") {
            flush(&mut current, &mut rules);
            in_comment = !trimmed.contains("-->");
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('|') {
            flush(&mut current, &mut rules);
            continue;
        }
        if crate::coexist::starts_list_item(trimmed) || trimmed.starts_with('>') {
            flush(&mut current, &mut rules);
        }
        current.push((i + 1, line));
    }
    flush(&mut current, &mut rules);
    rules
}

/// The rule of one unit's lines (`None` when it is too short to be one).
fn unit_text(lines: &[(usize, &str)]) -> Option<(usize, String)> {
    let first = lines.first()?.0;
    let joined = lines
        .iter()
        .map(|(_, line)| line.trim())
        .collect::<Vec<_>>()
        .join(" ");
    let text = strip_marker(&joined).split_whitespace().collect::<Vec<_>>().join(" ");
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
        for (line, text) in rules_in(&text).into_iter().take(MAX_RULES_PER_FILE) {
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
            rules.push(json!({"id": id, "text": text, "file": path.display().to_string(),
                              "line": line, "loaded_by": loaded_by, "global": global}));
        }
    }
    (rules, notes)
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
    /// New candidates of the conflict check.
    pub found: u64,
    /// Rules still to scan (at the next sync).
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
            found: n("found"),
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
            "instruction files: {} rule{} ({} new, {} removed); {} new candidate{} for the \
             conflict check",
            self.rules,
            if self.rules == 1 { "" } else { "s" },
            self.created,
            self.removed,
            self.found,
            if self.found == 1 { "" } else { "s" }
        );
        if self.pending > 0 {
            line.push_str(&format!(
                "; {} rule{} still to compare (next time)",
                self.pending,
                if self.pending == 1 { "" } else { "s" }
            ));
        }
        line
    }
}

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
        let text = serde_json::to_string_pretty(&json!({"version": 1, "projects": projects}))
            .map_err(|err| err.to_string())?;
        crate::fsutil::write_atomic(&path, format!("{text}\n").as_bytes())
    }
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
                (8, "Never push to the main branch without asking first.".to_string()),
                (9, "Write commit messages in the imperative mood.".to_string()),
                (10, "Keep the changelog current for every release.".to_string()),
                (11, "Prefer small pull requests over big ones.".to_string()),
            ]
        );
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
        let long = "word ".repeat(200);
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
                .join(crate::agents::GLOBAL_RULES_FILE),
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
                setup.paths.config_dir.join("rules").join("style.md").as_path(),
                project.join("CLAUDE.md").as_path(),
                project.join("CLAUDE.local.md").as_path(),
                project.join(".claude").join("CLAUDE.md").as_path(),
                project.join(".claude").join("rules").join("testing.md").as_path(),
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
        std::fs::write(home.path().join("notes.txt"), "Use pnpm for every package.\n").unwrap();
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
        let first = sync(data.path(), "/w", &files, false, &mut send).unwrap().unwrap();
        assert_eq!(first.pending, 1);
        // The scan did not finish: sent again.
        assert!(sync(data.path(), "/w", &files, false, &mut send).unwrap().is_some());
        pending = 0;
        let mut send = |line: &Value| {
            sent.push(line.clone());
            Ok(json!({"ok": true, "rules": 1, "pending": pending}))
        };
        assert!(sync(data.path(), "/w", &files, false, &mut send).unwrap().is_some());
        // Finished and unchanged: nothing sent; forced, or changed: sent.
        assert!(sync(data.path(), "/w", &files, false, &mut send).unwrap().is_none());
        assert!(sync(data.path(), "/w", &files, true, &mut send).unwrap().is_some());
        std::fs::write(&file, "Always use pnpm for every package.\n").unwrap();
        assert!(sync(data.path(), "/w", &files, false, &mut send).unwrap().is_some());
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
