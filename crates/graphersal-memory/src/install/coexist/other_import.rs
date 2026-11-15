//! Import from another memory tool whose format is known (M15), into global memories (those
//! tools keep one memory for every project):
//!
//! - **the reference memory server** (`@modelcontextprotocol/server-memory`): its knowledge
//!   graph file, `MEMORY_FILE_PATH` of the entry's environment when it is absolute, else
//!   `memory.jsonl` (older versions: `memory.json`) beside the package in npx's cache
//!   (`~/.npm/_npx/*/node_modules/@modelcontextprotocol/server-memory/dist/`). One JSON object
//!   per line: `{"type": "entity", "name", "entityType", "observations": [...]}` and `{"type":
//!   "relation", "from", "to", "relationType"}`. Each entity becomes the memory
//!   `kg-<name>`: its observations as the body (the first one the summary), its relations as
//!   lines and links; kind `user` for an entity of type user/me/self/preference, else
//!   `reference`.
//! - **Basic Memory**: the project folders of `~/.basic-memory/config.json` (`projects`), else
//!   `~/basic-memory`; each Markdown note becomes the memory `basic-<permalink>` (front matter
//!   `title`, `permalink`), kind `reference`, its text the body.
//!
//! The prefixes keep an imported item from replacing a memory of the user's with the same slug.
//! Every memory passes `from_file` (the data file as its `source`), so conflicts and returning
//! facts name the file. `<data dir>/import/other-memory.json` records each item's fingerprint:
//! a later import sends only new and changed items; an item whose memory was edited in
//! graphersal-memory after its import keeps the edit (`remember` answers `kept: "edited"`, the
//! report says so). The files are only read. Other tools'
//! formats are listed as "not importable yet".

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{Findings, Format, Tool};
use crate::install::{Setup, read_text};
use crate::memory::import::{Caller, Connect, slugify};
use crate::{fsutil, install::jsonedit};

/// The state file in `<data dir>/import/`.
const STATE_FILE: &str = "other-memory.json";

/// The format version of the state file.
const FORMAT: u64 = 1;

/// Entity types that are about the user.
const USER_TYPES: &[&str] = &["user", "me", "self", "preference", "preferences", "owner"];

/// One memory to import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Its key in the data (an entity's name, a note's path).
    pub key: String,
    /// The memory's slug.
    pub slug: String,
    /// The memory's kind.
    pub kind: String,
    /// Its title.
    pub title: String,
    /// One line.
    pub summary: String,
    /// The body.
    pub body: String,
    /// Slugs it links to.
    pub links: Vec<String>,
    /// What is imported, fingerprinted.
    pub fingerprint: String,
}

/// A data file (or folder) of another tool, with its items.
#[derive(Debug, Clone)]
pub struct Source {
    /// The tool.
    pub tool: &'static Tool,
    /// The MCP server's name it was found through.
    pub server: String,
    /// The data file or folder.
    pub path: PathBuf,
    /// The items.
    pub items: Vec<Item>,
}

/// The item fingerprints of earlier imports (`<data dir>/import/other-memory.json`).
#[derive(Debug, Clone, Default)]
pub struct State {
    items: BTreeMap<String, Value>,
}

impl State {
    fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("import").join(STATE_FILE)
    }

    /// The state of `data_dir` (empty when there is none or it does not parse).
    pub fn load(data_dir: &Path) -> Self {
        let items = std::fs::read(Self::path(data_dir))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| value["items"].as_object().cloned())
            .map(|items| items.into_iter().collect())
            .unwrap_or_default();
        Self { items }
    }

    fn save(&self, data_dir: &Path) -> Result<(), String> {
        let path = Self::path(data_dir);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
        }
        let items: serde_json::Map<String, Value> = self.items.clone().into_iter().collect();
        let value = json!({"version": FORMAT, "items": items});
        fsutil::write_atomic(
            &path,
            format!("{}\n", jsonedit::render(&value, Some("  "), 0)).as_bytes(),
        )
    }

    fn key(source: &Source, item: &Item) -> String {
        format!("{}#{}", source.path.display(), item.key)
    }

    /// The slugs of every imported item.
    pub fn slugs(&self) -> std::collections::BTreeSet<String> {
        self.items
            .values()
            .filter_map(|entry| entry["slug"].as_str())
            .map(str::to_string)
            .collect()
    }

    /// Whether `item` of `source` is new or changed since its import.
    pub fn pending(&self, source: &Source, item: &Item) -> bool {
        self.items
            .get(&Self::key(source, item))
            .and_then(|entry| entry["fingerprint"].as_str())
            != Some(item.fingerprint.as_str())
    }

    /// How many items of `source` are new or changed.
    pub fn pending_count(&self, source: &Source) -> usize {
        source
            .items
            .iter()
            .filter(|item| self.pending(source, item))
            .count()
    }
}

/// `text` cut to `max` characters.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}...", text.chars().take(max).collect::<String>())
    }
}

/// The items of the reference memory server's file `text`.
pub fn parse_memory_server(text: &str) -> Vec<Item> {
    let mut entities: Vec<Value> = Vec::new();
    let mut relations: Vec<Value> = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        match value["type"].as_str() {
            Some("entity") => entities.push(value),
            Some("relation") => relations.push(value),
            _ => {}
        }
    }
    // An older whole-file form: {"entities": [...], "relations": [...]}.
    if entities.is_empty()
        && let Ok(value) = serde_json::from_str::<Value>(text)
    {
        entities = value["entities"].as_array().cloned().unwrap_or_default();
        relations = value["relations"].as_array().cloned().unwrap_or_default();
    }
    let slug_of = |name: &str| format!("kg-{}", slugify(name));
    entities
        .iter()
        .filter_map(|entity| {
            let name = entity["name"].as_str()?.trim();
            let slug = slug_of(name);
            if name.is_empty() || slug == "kg-" {
                return None;
            }
            let kind_word = entity["entityType"].as_str().unwrap_or_default().trim();
            let observations: Vec<&str> = entity["observations"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|o| !o.is_empty())
                .collect();
            let mut body = format!(
                "**{name}**{}\n",
                if kind_word.is_empty() {
                    String::new()
                } else {
                    format!(" ({kind_word})")
                }
            );
            if !observations.is_empty() {
                body.push('\n');
                for observation in &observations {
                    body.push_str(&format!("- {observation}\n"));
                }
            }
            let mut links = Vec::new();
            let mut lines = Vec::new();
            for relation in &relations {
                let (from, to, how) = (
                    relation["from"].as_str().unwrap_or_default(),
                    relation["to"].as_str().unwrap_or_default(),
                    relation["relationType"].as_str().unwrap_or("relates to"),
                );
                if from == name || to == name {
                    lines.push(format!("- {from} {how} {to}"));
                    let other = if from == name { to } else { from };
                    let other = slug_of(other);
                    if other != slug && !links.contains(&other) {
                        links.push(other);
                    }
                }
            }
            if !lines.is_empty() {
                body.push_str("\nRelations:\n");
                for line in &lines {
                    body.push_str(line);
                    body.push('\n');
                }
            }
            let summary = observations
                .first()
                .map(|o| clip(o, 200))
                .unwrap_or_else(|| {
                    if kind_word.is_empty() {
                        name.to_string()
                    } else {
                        format!("{name} ({kind_word})")
                    }
                });
            let kind = if USER_TYPES.contains(&kind_word.to_lowercase().as_str()) {
                "user"
            } else {
                "reference"
            };
            let fingerprint = fsutil::fingerprint(format!("{kind}\n{summary}\n{body}").as_bytes());
            Some(Item {
                key: format!("entity:{name}"),
                slug,
                kind: kind.to_string(),
                title: name.to_string(),
                summary,
                body,
                links,
                fingerprint,
            })
        })
        .collect()
}

/// The items of a Basic Memory folder (its Markdown notes, dot directories left out).
pub fn parse_basic_memory(root: &Path) -> Vec<Item> {
    let mut files = Vec::new();
    collect_md(root, 8, &mut files);
    files.sort();
    files
        .iter()
        .filter_map(|file| {
            let text = read_text(file).ok().flatten()?;
            let (front, body) = crate::memory::import::split_front_matter(&text);
            let relative = file
                .strip_prefix(root)
                .unwrap_or(file)
                .with_extension("")
                .display()
                .to_string();
            let permalink =
                crate::memory::import::field(front, "permalink").unwrap_or(relative.clone());
            let slug = format!("basic-{}", slugify(&permalink));
            if slug == "basic-" {
                return None;
            }
            let title = crate::memory::import::field(front, "title").unwrap_or_else(|| {
                file.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            let first_line = body
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty() && !line.starts_with('#'))
                .unwrap_or_default();
            let summary = if title.is_empty() {
                clip(first_line, 200)
            } else {
                clip(&title, 200)
            };
            let body = body.trim().to_string();
            let fingerprint = fsutil::fingerprint(format!("{summary}\n{body}").as_bytes());
            Some(Item {
                key: relative,
                slug,
                kind: "reference".to_string(),
                title,
                summary,
                body,
                links: Vec::new(),
                fingerprint,
            })
        })
        .collect()
}

fn collect_md(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        if path.is_dir() {
            if !hidden && depth > 1 {
                collect_md(&path, depth - 1, out);
            }
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

/// The reference memory server's data file for the entry `entry`, when it is found.
pub fn memory_server_file(home: &Path, entry: &Value) -> Option<PathBuf> {
    for env in ["env", "environment"] {
        if let Some(path) = entry[env]["MEMORY_FILE_PATH"].as_str() {
            let path = match path.strip_prefix("~/") {
                Some(rest) => home.join(rest),
                None => PathBuf::from(path),
            };
            if path.is_absolute() {
                return path.is_file().then_some(path);
            }
        }
    }
    let cache = home.join(".npm").join("_npx");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&cache)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    dirs.sort();
    for dir in dirs {
        let dist = dir
            .join("node_modules")
            .join("@modelcontextprotocol")
            .join("server-memory")
            .join("dist");
        for name in ["memory.jsonl", "memory.json"] {
            if dist.join(name).is_file() {
                return Some(dist.join(name));
            }
        }
    }
    None
}

/// Basic Memory's project folders.
pub fn basic_memory_folders(home: &Path) -> Vec<PathBuf> {
    let config = home.join(".basic-memory").join("config.json");
    let mut folders: Vec<PathBuf> = std::fs::read(&config)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value["projects"].as_object().cloned())
        .map(|projects| {
            projects
                .values()
                .filter_map(|v| v.as_str().or_else(|| v["path"].as_str()))
                .map(|p| match p.strip_prefix("~/") {
                    Some(rest) => home.join(rest),
                    None => PathBuf::from(p),
                })
                .collect()
        })
        .unwrap_or_default();
    if folders.is_empty() {
        folders.push(home.join("basic-memory"));
    }
    folders.retain(|f| f.is_dir());
    folders.sort();
    folders.dedup();
    folders
}

/// The importable data of the other tools found, and one line per tool found whose format is
/// not importable yet (or whose data was not found).
pub fn sources(setup: &Setup, findings: &Findings) -> (Vec<Source>, Vec<String>) {
    let home = &setup.opencode().home;
    let mut out: Vec<Source> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    for server in &findings.servers {
        let Some(tool) = server.tool else { continue };
        let paths: Vec<PathBuf> = match tool.format {
            Some(Format::MemoryServer) => memory_server_file(home, &server.entry)
                .into_iter()
                .collect(),
            Some(Format::BasicMemory) => basic_memory_folders(home),
            None => {
                let line = format!("{} ({}): not importable yet", server.name, tool.name);
                if !notes.contains(&line) {
                    notes.push(line);
                }
                continue;
            }
        };
        if paths.is_empty() {
            let line = format!(
                "{} ({}): its data was not found, nothing to import",
                server.name, tool.name
            );
            if !notes.contains(&line) {
                notes.push(line);
            }
            continue;
        }
        for path in paths {
            if out.iter().any(|s| s.path == path) {
                continue;
            }
            let items = match tool.format {
                Some(Format::MemoryServer) => read_text(&path)
                    .ok()
                    .flatten()
                    .map(|text| parse_memory_server(&text))
                    .unwrap_or_default(),
                _ => parse_basic_memory(&path),
            };
            out.push(Source {
                tool,
                server: server.name.clone(),
                path,
                items,
            });
        }
    }
    (out, notes)
}

/// What an import from other tools did.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Memories created.
    pub created: usize,
    /// Memories replaced.
    pub updated: usize,
    /// Items unchanged since their import.
    pub unchanged: usize,
    /// Failures.
    pub problems: Vec<String>,
    /// Changed items whose memory was edited in graphersal-memory after its import: the edited
    /// text was kept, one line each.
    pub kept_edited: Vec<String>,
}

impl Report {
    /// One line for the install report.
    pub fn line(&self, sources: &[Source]) -> String {
        let names: Vec<String> = sources
            .iter()
            .map(|s| format!("{} ({})", s.server, s.path.display()))
            .collect();
        let mut line = format!(
            "other memory: imported {} memor{} ({} new, {} updated; {} unchanged) as global \
             memories from {}",
            self.created + self.updated,
            if self.created + self.updated == 1 {
                "y"
            } else {
                "ies"
            },
            self.created,
            self.updated,
            self.unchanged,
            names.join(", ")
        );
        if !self.kept_edited.is_empty() {
            line.push_str(&format!(
                "; {} changed but edited in graphersal-memory, the edit kept",
                self.kept_edited.len()
            ));
        }
        line
    }
}

/// The modification time of `path` as `YYYY-MM-DDTHH:MM:SSZ`.
fn modified_at(path: &Path) -> String {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| crate::runtime::daemon::utc_timestamp(since.as_secs()))
        .unwrap_or_default()
}

/// Imports the new and changed items of `sources` as global memories (through one connection
/// without a project), recording each in the state of `data_dir`.
pub fn apply(sources: &[Source], connect: &mut Connect<'_>, data_dir: &Path) -> Report {
    let mut report = Report::default();
    let mut state = State::load(data_dir);
    let todo: usize = sources.iter().map(|s| state.pending_count(s)).sum();
    for source in sources {
        report.unchanged += source.items.len() - state.pending_count(source);
    }
    if todo == 0 {
        return report;
    }
    let mut caller: Box<dyn Caller> = match connect("") {
        Ok(caller) => caller,
        Err(err) => {
            report
                .problems
                .push(format!("cannot reach the memory: {err}"));
            return report;
        }
    };
    for source in sources {
        // The memory of a data file is the file itself; a folder's is each note.
        for item in &source.items {
            if !state.pending(source, item) {
                continue;
            }
            let file = if source.path.is_dir() {
                source.path.join(format!("{}.md", item.key))
            } else {
                source.path.clone()
            };
            let dir = file
                .parent()
                .map(|d| format!("{}{}", d.display(), std::path::MAIN_SEPARATOR))
                .unwrap_or_default();
            let args = json!({
                "slug": item.slug, "kind": item.kind, "summary": item.summary,
                "body": item.body, "why": "", "how_to_apply": "", "title": item.title,
                "hook": "", "pinned": null, "order": null, "links": item.links,
                "scope": "global",
                "from_file": {"path": file.display().to_string(), "modified_at": modified_at(&file),
                              "dir": dir},
            });
            match caller.call("remember", args) {
                // Edited in graphersal-memory since its import: the edit stays; the item's new
                // fingerprint is recorded below so it is not sent again.
                Ok(answer) if answer["kept"] == "edited" => {
                    report
                        .kept_edited
                        .push(crate::memory::import::kept_edited_line(
                            &file.display().to_string(),
                            &item.slug,
                        ));
                    state.items.insert(
                        State::key(source, item),
                        json!({"fingerprint": item.fingerprint, "slug": item.slug,
                               "imported_at": crate::runtime::daemon::now_utc()}),
                    );
                }
                Ok(answer) => {
                    if answer["created"] == Value::Bool(true) {
                        report.created += 1;
                    } else {
                        report.updated += 1;
                    }
                    state.items.insert(
                        State::key(source, item),
                        json!({"fingerprint": item.fingerprint, "slug": item.slug,
                               "imported_at": crate::runtime::daemon::now_utc()}),
                    );
                }
                Err(err) => report.problems.push(format!(
                    "{} ({}): {err}",
                    item.slug,
                    source.path.display()
                )),
            }
        }
    }
    if let Err(err) = state.save(data_dir) {
        report.problems.push(err);
    }
    report
}
