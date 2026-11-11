//! Import of Claude Code's file memory: every `<claude config>/projects/<project>/memory/`
//! directory (a `MEMORY.md` index and one markdown file per memory, with a front matter of
//! `name`, `description` and `metadata.type`) becomes memories of that project.
//!
//! - The memory's slug is the file name (lower case, other characters turned into `-`); its kind
//!   is `metadata.type` (or a top-level `type`), `project` when missing or unknown; its summary
//!   is `description`; its body is the text below the front matter (a trailing `**Why:**` /
//!   `**How to apply:**` section is passed as `why` / `how_to_apply`, which `remember` appends
//!   back); its title and hook come from its `MEMORY.md` line (`- [Title](file.md) — hook`),
//!   which also gives the order; `[[slug]]` in the body become links. A memory listed under a
//!   `## Pinned` heading of `MEMORY.md` is pinned (loaded into the start of every session,
//!   `crate::start_context`); moved out of it, it is unpinned at its next import (only when this
//!   import pinned it: a pin set in the graph stays). The line's position becomes the
//!   memory's `order`, so the start context and `memories()` keep the `MEMORY.md` order; a
//!   position the import set before is not sent again, so an order set in the graph stays
//!   until the line moves.
//! - The project is the directory the harness ran in: the `cwd` recorded in the project's
//!   transcripts, else the directory name decoded against the file system (Claude Code names it
//!   after the path with every other character turned into `-`). The memories get the project
//!   key of that directory (its git root), as a session started there would. A directory whose
//!   project no longer exists is skipped and reported.
//! - Idempotent: every file is written through the memory function `remember` (replace by slug,
//!   never a duplicate), and `<data dir>/import/claude-code.json` remembers each imported file's
//!   fingerprint, so a later import sends only new and changed files. A memory forgotten in the
//!   graph is not brought back while its file's text is unchanged (a moved `MEMORY.md` line
//!   alone does not: the state records the text's own fingerprint, `content`, and `forgotten`);
//!   a superseded one stays marked (an import never removes the `supersedes` edge).
//! - Conflicts (M12): `remember` gets the file as `from_file` (the memory's `source`, its
//!   `imported_at`, and the file's modification time as its `updated_at`, the decision time);
//!   a memory with similar existing memories is marked `unchecked` until an agent settles it.
//!   After each project the report reads `conflicts()`: the open conflicts and the files that
//!   still hold a superseded memory, named so the user can edit them.
//! - The files are only read, never changed or deleted. Rules in `CLAUDE.md` files are not
//!   imported in this version.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::fsutil;

/// The import's state directory in the data directory.
const STATE_DIR: &str = "import";

/// The state file of the Claude Code import.
const STATE_FILE: &str = "claude-code.json";

/// The lock file that keeps two imports (two hooks at once) apart.
const LOCK_FILE: &str = "import.lock";

/// The format version of the state file.
const FORMAT: u64 = 1;

/// The memory kinds of the memory schema.
pub const KINDS: [&str; 4] = ["user", "feedback", "project", "reference"];

/// How many directories the file-system decoding of a project name visits at most.
const DECODE_BUDGET: usize = 4000;

/// One memory file, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryFile {
    /// The file.
    pub path: PathBuf,
    /// The memory's slug.
    pub slug: String,
    /// The memory's kind.
    pub kind: String,
    /// Whether the kind was missing or unknown (and `project` was taken).
    pub kind_defaulted: bool,
    /// One line.
    pub summary: String,
    /// The markdown body (without the why / how-to-apply tail when it was split off).
    pub body: String,
    /// The reason, from a trailing `**Why:**`.
    pub why: String,
    /// When and how to apply it, from a trailing `**How to apply:**`.
    pub how_to_apply: String,
    /// Its index line's title.
    pub title: String,
    /// Its index line's hook.
    pub hook: String,
    /// Its position in `MEMORY.md` (files not listed there come after, by name).
    pub order: usize,
    /// Whether `MEMORY.md` lists it (then `order` becomes the memory's `order`).
    pub listed: bool,
    /// The slugs it links to.
    pub links: Vec<String>,
    /// Whether `MEMORY.md` lists it under `## Pinned`.
    pub pinned: bool,
    /// The fingerprint of what is imported (the file and its index line).
    pub fingerprint: String,
    /// The fingerprint of the file's text alone (a change of only its index line keeps it).
    pub content: String,
    /// When the file last changed (`YYYY-MM-DDTHH:MM:SSZ`; empty when unknown): the memory's
    /// decision time.
    pub modified_at: String,
}

impl MemoryFile {
    /// The arguments of `remember` for this memory; `pinned` is `true`, `false` or `null`
    /// (keep the graph's), `order` a number or `null` (keep the graph's).
    pub fn remember_args(&self, pinned: Value, order: Value) -> Value {
        json!({
            "pinned": pinned,
            "order": order,
            "slug": self.slug,
            "kind": self.kind,
            "summary": self.summary,
            "body": self.body,
            "why": self.why,
            "how_to_apply": self.how_to_apply,
            "title": self.title,
            "hook": self.hook,
            "links": self.links,
            "from_file": {"path": self.path.display().to_string(), "modified_at": self.modified_at,
                          "dir": self.path.parent().map(|dir| format!("{}{}", dir.display(),
                                                                       std::path::MAIN_SEPARATOR))
                                                    .unwrap_or_default()},
        })
    }
}

/// One memory directory of the harness.
#[derive(Debug, Clone)]
pub struct MemoryDir {
    /// The `memory` directory.
    pub dir: PathBuf,
    /// The project directory's name (the harness's encoding of the path).
    pub name: String,
    /// The project key, or `None` when the project's directory was not found.
    pub project: Option<String>,
    /// The memory files.
    pub files: Vec<MemoryFile>,
    /// Files that could not be read or parsed, with the reason.
    pub problems: Vec<String>,
}

/// Claude Code's name for the project directory of `path`: every character that is not an
/// ASCII letter or digit becomes `-`.
pub fn claude_slug(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// A memory slug from a file name or a link: lower case, characters other than `a-z`, `0-9`
/// and `-` become `-`, repeated and outer dashes removed.
pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        let c = c.to_ascii_lowercase();
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() {
            c
        } else {
            '-'
        };
        if c == '-' && (out.is_empty() || out.ends_with('-')) {
            continue;
        }
        out.push(c);
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// The memory directories under the harness's `projects` directory, sorted by name.
pub fn scan(projects_dir: &Path) -> Result<Vec<MemoryDir>, String> {
    let entries = match std::fs::read_dir(projects_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("cannot read {}: {err}", projects_dir.display())),
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("memory").is_dir())
        .collect();
    dirs.sort();
    Ok(dirs
        .iter()
        .map(|project_dir| {
            let project =
                resolve_project_dir(project_dir).map(|dir| crate::mcp::project_key(None, &dir));
            read_dir(&project_dir.join("memory"), project)
        })
        .collect())
}

/// Reads one memory directory (its project already resolved).
pub fn read_dir(dir: &Path, project: Option<String>) -> MemoryDir {
    let name = dir
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut out = MemoryDir {
        dir: dir.to_path_buf(),
        name,
        project,
        files: Vec::new(),
        problems: Vec::new(),
    };
    let index_text = std::fs::read_to_string(dir.join("MEMORY.md")).unwrap_or_default();
    let index = index_lines(&index_text);
    let pinned = pinned_stems(&index_text);
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path.extension().is_some_and(|ext| ext == "md")
                    && path.file_name().is_some_and(|name| name != "MEMORY.md")
                    // The export's own copies beside a file that differed (`<slug>.graphersal.md`)
                    // are the graph's text, never a memory of their own.
                    && !path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().ends_with(".graphersal.md"))
            })
            .collect(),
        Err(err) => {
            out.problems
                .push(format!("cannot read {}: {err}", dir.display()));
            return out;
        }
    };
    paths.sort();
    let mut seen = BTreeSet::new();
    for path in paths {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                out.problems
                    .push(format!("cannot read {}: {err}", path.display()));
                continue;
            }
        };
        let entry = index.get(&stem);
        match parse_file(&path, &stem, &text, entry, index.len()) {
            Ok(mut file) => {
                if entry.is_some() {
                    // The position is the memory's `order`: a moved line is a change.
                    file.listed = true;
                    file.fingerprint = fsutil::fingerprint(
                        format!("{}\n\u{0}order={}", file.fingerprint, file.order).as_bytes(),
                    );
                }
                if pinned.contains(&stem) {
                    // Only a pinned file's fingerprint changes: the others keep theirs, so an
                    // upgrade does not send every file again.
                    file.pinned = true;
                    file.fingerprint = fsutil::fingerprint(
                        format!("{}\n\u{0}pinned", file.fingerprint).as_bytes(),
                    );
                }
                if !seen.insert(file.slug.clone()) {
                    out.problems.push(format!(
                        "{}: another file has the slug {} already; skipped",
                        path.display(),
                        file.slug
                    ));
                    continue;
                }
                out.files.push(file);
            }
            Err(err) => out.problems.push(format!("{}: {err}", path.display())),
        }
    }
    out.files
        .sort_by(|a, b| (a.order, &a.path).cmp(&(b.order, &b.path)));
    out
}

/// One `MEMORY.md` line: title, hook, position.
#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexEntry {
    title: String,
    hook: String,
    position: usize,
}

/// The lines `- [Title](file.md) — hook` of a `MEMORY.md`, by file stem.
fn index_lines(text: &str) -> BTreeMap<String, IndexEntry> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let Some(rest) = line.trim_start().strip_prefix("- [") else {
            continue;
        };
        let Some((title, rest)) = rest.split_once("](") else {
            continue;
        };
        let Some((target, rest)) = rest.split_once(')') else {
            continue;
        };
        let Some(stem) = target.strip_suffix(".md") else {
            continue;
        };
        let stem = stem.rsplit('/').next().unwrap_or(stem).to_string();
        let rest = rest.trim();
        let hook = ["—", "–", "-", ":"]
            .iter()
            .find_map(|dash| rest.strip_prefix(dash))
            .unwrap_or(rest)
            .trim()
            .to_string();
        let position = out.len();
        out.entry(stem).or_insert(IndexEntry {
            title: title.to_string(),
            hook,
            position,
        });
    }
    out
}

/// The file stems listed under a `## Pinned` heading of a `MEMORY.md` (up to the next
/// heading of any level).
fn pinned_stems(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let heading = trimmed.trim_start_matches('#').trim();
            inside = trimmed.starts_with("## ") && heading.eq_ignore_ascii_case("pinned");
            continue;
        }
        if inside {
            out.extend(index_lines(line).into_keys());
        }
    }
    out
}

/// Parses one memory file.
fn parse_file(
    path: &Path,
    stem: &str,
    text: &str,
    entry: Option<&IndexEntry>,
    unlisted: usize,
) -> Result<MemoryFile, String> {
    let normalized = text.replace("\r\n", "\n");
    let (front, body) = split_front_matter(&normalized);
    let slug = slugify(stem);
    if slug.is_empty() {
        return Err("the file name gives no slug".to_string());
    }
    let declared = field(front, "type").map(|kind| kind.to_ascii_lowercase());
    let (kind, kind_defaulted) = match declared {
        Some(kind) if KINDS.contains(&kind.as_str()) => (kind, false),
        _ => ("project".to_string(), true),
    };
    let body = strip_export_note(body.trim()).to_string();
    let mut summary = field(front, "description").unwrap_or_default();
    if summary.is_empty() {
        summary = body
            .lines()
            .map(|line| line.trim().trim_start_matches('#').trim())
            .find(|line| !line.is_empty())
            .unwrap_or(stem)
            .chars()
            .take(200)
            .collect();
    }
    let (main, why, how_to_apply) = split_why(&body);
    let main = if main.is_empty() {
        summary.clone()
    } else {
        main
    };
    let links: Vec<String> = wiki_links(&body)
        .into_iter()
        .filter(|link| *link != slug)
        .collect();
    let (title, hook, order) = match entry {
        Some(entry) => (entry.title.clone(), entry.hook.clone(), entry.position),
        None => (String::new(), String::new(), unlisted),
    };
    let fingerprint = fsutil::fingerprint(format!("{text}\n\u{0}{title}\n\u{0}{hook}").as_bytes());
    Ok(MemoryFile {
        path: path.to_path_buf(),
        slug,
        kind,
        kind_defaulted,
        summary,
        body: main,
        why,
        how_to_apply,
        title,
        hook,
        order,
        links,
        listed: false,
        pinned: false,
        fingerprint,
        content: fsutil::fingerprint(text.as_bytes()),
        modified_at: std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|since| crate::daemon::utc_timestamp(since.as_secs()))
            .unwrap_or_default(),
    })
}

/// The front matter (between the first two `---` lines) and the rest.
fn split_front_matter(text: &str) -> (&str, &str) {
    if let Some(rest) = text.strip_prefix("---\n")
        && let Some(end) = rest.find("\n---")
    {
        let front = &rest[..end];
        let after = &rest[end + 4..];
        let after = after.split_once('\n').map_or("", |(_, body)| body);
        return (front, after);
    }
    ("", text)
}

/// The first paragraph the export puts above the body of a superseded memory or one in an open
/// conflict ([`crate::export::SUPERSEDED_NOTE`], [`crate::export::CONFLICT_NOTE`]): taken off
/// again, so an export imported back gives the same memory.
fn strip_export_note(body: &str) -> &str {
    let first = body.split("\n\n").next().unwrap_or_default();
    let ours = (first.starts_with("SUPERSEDED by ") || first.starts_with("CONFLICT with "))
        && first.ends_with(crate::export::NOTE_END)
        && !first.contains('\n');
    if ours {
        body[first.len()..].trim_start()
    } else {
        body
    }
}

/// A front-matter field (top-level or nested one level, as `metadata.type`), unquoted; a
/// folded or literal block (`>`, `>-`, `|`, `|-`) joined from its indented lines.
fn field(front: &str, key: &str) -> Option<String> {
    let lines: Vec<&str> = front.lines().collect();
    lines.iter().enumerate().find_map(|(at, line)| {
        let (name, value) = line.trim().split_once(':')?;
        if name.trim() != key {
            return None;
        }
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        if matches!(value, ">" | ">-" | ">+" | "|" | "|-" | "|+") {
            let indent = line.len() - line.trim_start().len();
            let block: Vec<&str> = lines[at + 1..]
                .iter()
                .take_while(|next| {
                    next.trim().is_empty() || next.len() - next.trim_start().len() > indent
                })
                .map(|next| next.trim())
                .collect();
            let separator = if value.starts_with('>') { " " } else { "\n" };
            let joined = block.join(separator).trim().to_string();
            return (!joined.is_empty()).then_some(joined);
        }
        Some(unquote(value))
    })
}

/// What a memory file says, for the export's comparison: its kind, summary and body (after
/// the front matter, line ends and blank lines normalized). Two files that say the same give
/// the same value whatever their formatting (CRLF, a blank line more, folded YAML, metadata
/// keys of others).
pub fn meaning(text: &str) -> (String, String, String) {
    let normalized = text.replace("\r\n", "\n");
    let (front, body) = split_front_matter(&normalized);
    // As the import reads it: an unknown kind is `project`, a missing description the body's
    // first line.
    let kind = field(front, "type")
        .map(|kind| kind.to_ascii_lowercase())
        .filter(|kind| KINDS.contains(&kind.as_str()))
        .unwrap_or_else(|| "project".to_string());
    let mut summary = field(front, "description").unwrap_or_default();
    if summary.is_empty() {
        summary = body
            .lines()
            .map(|line| line.trim().trim_start_matches('#').trim())
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .chars()
            .take(200)
            .collect();
    }
    let mut lines: Vec<&str> = Vec::new();
    for line in body.trim().lines().map(str::trim_end) {
        if line.is_empty() && lines.last().is_some_and(|l| l.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    // A blank line between the why and how-to-apply lines (or none) says the same.
    let body = lines
        .join("\n")
        .replace("\n\n**How to apply:**", "\n**How to apply:**")
        .replace("**Why:**\n", "**Why:** ")
        .replace("**How to apply:**\n", "**How to apply:** ");
    (kind, summary, body)
}

fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 && bytes[0] == bytes[bytes.len() - 1] && matches!(bytes[0], b'"' | b'\'') {
        let inner = &value[1..value.len() - 1];
        if bytes[0] == b'"' {
            return inner.replace("\\\"", "\"").replace("\\\\", "\\");
        }
        return inner.replace("''", "'");
    }
    value.to_string()
}

/// Splits a trailing `**Why:** ...` / `**How to apply:** ...` section off the body (only when
/// each marker appears once and the section ends the body).
fn split_why(body: &str) -> (String, String, String) {
    const WHY: &str = "**Why:**";
    const HOW: &str = "**How to apply:**";
    let whys = body.matches(WHY).count();
    let hows = body.matches(HOW).count();
    if whys > 1 || hows > 1 || (whys == 0 && hows == 0) {
        return (body.to_string(), String::new(), String::new());
    }
    let why_at = body.find(WHY);
    let how_at = body.find(HOW);
    let tail_start = match (why_at, how_at) {
        (Some(w), Some(h)) if w < h => w,
        (Some(_), Some(_)) => return (body.to_string(), String::new(), String::new()),
        (Some(w), None) => w,
        (None, Some(h)) => h,
        (None, None) => return (body.to_string(), String::new(), String::new()),
    };
    let main = body[..tail_start].trim().to_string();
    let why = match (why_at, how_at) {
        (Some(w), Some(h)) => body[w + WHY.len()..h].trim().to_string(),
        (Some(w), None) => body[w + WHY.len()..].trim().to_string(),
        _ => String::new(),
    };
    let how = how_at
        .map(|h| body[h + HOW.len()..].trim().to_string())
        .unwrap_or_default();
    (main, why, how)
}

/// The `[[slug]]` links of a body, slugified, sorted, without duplicates.
fn wiki_links(body: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find("]]") else { break };
        let link = slugify(&rest[..end]);
        if !link.is_empty() && end <= 100 {
            out.insert(link);
        }
        rest = &rest[end + 2..];
    }
    out.into_iter().collect()
}

/// The directory a harness project directory stands for: the `cwd` of its transcripts whose
/// encoding is the directory's name, else the name decoded against the file system.
pub fn resolve_project_dir(project_dir: &Path) -> Option<PathBuf> {
    let name = project_dir.file_name()?.to_str()?.to_string();
    if let Some(dir) = cwd_from_transcripts(project_dir, &name) {
        return Some(dir);
    }
    decode(&name)
}

/// The first transcript `cwd` (of up to 20 transcripts, the first 64 KiB of each) whose
/// encoding equals `name` and that exists.
fn cwd_from_transcripts(project_dir: &Path, name: &str) -> Option<PathBuf> {
    use std::io::Read;
    let mut files: Vec<PathBuf> = std::fs::read_dir(project_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    files.sort();
    for file in files.iter().take(20) {
        let Ok(handle) = std::fs::File::open(file) else {
            continue;
        };
        let mut head = String::new();
        let _ = handle.take(64 * 1024).read_to_string(&mut head);
        for line in head.lines() {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if let Some(cwd) = value.get("cwd").and_then(Value::as_str)
                && claude_slug(cwd) == name
                && Path::new(cwd).is_dir()
            {
                return Some(PathBuf::from(cwd));
            }
        }
    }
    None
}

/// Decodes a project directory name against the file system: walks down from the root,
/// matching each directory entry's encoding against the name (every `-` may be a separator or
/// part of a name). `None` when no existing directory matches within the budget.
pub fn decode(name: &str) -> Option<PathBuf> {
    let (root, rest) = if let Some(rest) = name.strip_prefix('-') {
        (PathBuf::from("/"), rest)
    } else {
        // A Windows drive: `C:\x` is encoded `C--x`.
        let mut chars = name.chars();
        let drive = chars.next().filter(char::is_ascii_alphabetic)?;
        let rest = name.get(1..)?.strip_prefix("--")?;
        (PathBuf::from(format!("{drive}:\\")), rest)
    };
    let mut budget = DECODE_BUDGET;
    walk(&root, rest, &mut budget)
}

fn walk(dir: &Path, rest: &str, budget: &mut usize) -> Option<PathBuf> {
    if rest.is_empty() {
        return Some(dir.to_path_buf());
    }
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    let mut entries: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_type()
                .is_ok_and(|t| t.is_dir() || t.is_symlink())
        })
        .map(|entry| {
            (
                claude_slug(&entry.file_name().to_string_lossy()),
                entry.path(),
            )
        })
        .filter(|(encoded, _)| !encoded.is_empty() && rest.starts_with(encoded.as_str()))
        .collect();
    // Longer matches first: `my-app` before `my`.
    entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.1.cmp(&b.1)));
    for (encoded, path) in entries {
        let after = &rest[encoded.len()..];
        if after.is_empty() {
            if path.is_dir() {
                return Some(path);
            }
            continue;
        }
        if let Some(next) = after.strip_prefix('-')
            && let Some(found) = walk(&path, next, budget)
        {
            return Some(found);
        }
    }
    None
}

/// What the import knows about the files it imported (`<data dir>/import/claude-code.json`).
#[derive(Debug, Clone, Default)]
pub struct ImportState {
    files: BTreeMap<String, Value>,
}

impl ImportState {
    fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(STATE_DIR).join(STATE_FILE)
    }

    /// Reads the state of `data_dir` (empty when there is none).
    pub fn load(data_dir: &Path) -> Result<Self, String> {
        let path = Self::path(data_dir);
        let Some(bytes) = fsutil::read_optional(&path)? else {
            return Ok(Self::default());
        };
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|err| format!("{} is invalid: {err}", path.display()))?;
        if value.get("version").and_then(Value::as_u64) != Some(FORMAT) {
            return Err(format!("{}: unsupported version", path.display()));
        }
        let files = value
            .get("files")
            .and_then(Value::as_object)
            .map(|files| files.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        Ok(Self { files })
    }

    /// The fingerprint of `file`'s text when this import last read it (`None`: never imported).
    pub fn content_of(&self, file: &Path) -> Option<&str> {
        self.files
            .get(&file.display().to_string())
            .and_then(|entry| entry["content"].as_str())
    }

    /// Every recorded file with its slug, project and import time (for the one-time migration
    /// of memories imported before the import marked them, `migrate_sources`).
    pub fn records(&self) -> Vec<Value> {
        self.files
            .iter()
            .filter(|(_, entry)| entry["forgotten"] != Value::Bool(true))
            .map(|(path, entry)| {
                json!({"path": path, "slug": entry["slug"], "project": entry["project"],
                       "imported_at": entry["imported_at"]})
            })
            .collect()
    }

    /// Writes the state.
    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        let path = Self::path(data_dir);
        let dir = data_dir.join(STATE_DIR);
        std::fs::create_dir_all(&dir)
            .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
        let files: serde_json::Map<String, Value> = self.files.clone().into_iter().collect();
        let value = json!({"version": FORMAT, "files": files});
        fsutil::write_atomic(
            &path,
            format!("{}\n", crate::jsonedit::render(&value, Some("  "), 0)).as_bytes(),
        )
    }

    fn fingerprint_of(&self, file: &Path) -> Option<&str> {
        self.files
            .get(&file.display().to_string())
            .and_then(|entry| entry["fingerprint"].as_str())
    }

    /// Whether `file`'s memory was forgotten in the graph after this import wrote it and its
    /// text has not changed since: only its index line moved, which must not bring it back.
    fn forgotten_unless_changed(&self, file: &MemoryFile) -> Option<bool> {
        let entry = self.files.get(&file.path.display().to_string())?;
        if entry["content"].as_str() != Some(file.content.as_str()) {
            return None;
        }
        Some(entry["forgotten"] == Value::Bool(true))
    }

    /// Records that `file`'s memory is gone from the graph although its text is unchanged:
    /// the import does not bring it back until the file's text changes.
    fn record_forgotten(&mut self, file: &MemoryFile, project: &str) {
        self.record(file, project);
        if let Some(entry) = self.files.get_mut(&file.path.display().to_string()) {
            entry["forgotten"] = Value::Bool(true);
        }
    }

    /// [`ImportState::record`] for tests in other modules.
    #[cfg(test)]
    pub fn record_for_test(&mut self, file: &MemoryFile, project: &str) {
        self.record(file, project);
    }

    fn record(&mut self, file: &MemoryFile, project: &str) {
        let mut entry = json!({"fingerprint": file.fingerprint, "content": file.content,
                               "slug": file.slug, "project": project,
                               "imported_at": crate::daemon::now_utc()});
        if file.pinned {
            entry["pinned"] = Value::Bool(true);
        }
        if file.listed {
            entry["order"] = Value::from(file.order as u64);
        }
        self.files.insert(file.path.display().to_string(), entry);
    }

    /// The `order` argument of `remember` for `file`: its `MEMORY.md` position when it is
    /// listed and the line moved since this import last set it (or it never did), else `null`
    /// (an order set in the graph, by `pin`, stays while the line stays where it was).
    fn order_arg(&self, file: &MemoryFile) -> Value {
        if !file.listed {
            return Value::Null;
        }
        let recorded = self
            .files
            .get(&file.path.display().to_string())
            .and_then(|entry| entry["order"].as_u64());
        if recorded == Some(file.order as u64) {
            Value::Null
        } else {
            Value::from(file.order as u64)
        }
    }

    /// The `pinned` argument of `remember` for `file`: `true` under `## Pinned`, `false` when
    /// this import pinned it before and it moved out, else `null` (the graph's pin stays).
    fn pinned_arg(&self, file: &MemoryFile) -> Value {
        let was = self
            .files
            .get(&file.path.display().to_string())
            .is_some_and(|entry| entry["pinned"] == Value::Bool(true));
        match (file.pinned, was) {
            (true, _) => Value::Bool(true),
            (false, true) => Value::Bool(false),
            (false, false) => Value::Null,
        }
    }

    /// How many files the state lists.
    pub fn len(&self) -> usize {
        self.files.len()
    }
}

/// What an import will do with one directory.
#[derive(Debug, Clone)]
pub struct DirPlan {
    /// The directory and its files.
    pub dir: MemoryDir,
    /// Files never imported.
    pub new: Vec<MemoryFile>,
    /// Files changed since their import.
    pub changed: Vec<MemoryFile>,
    /// Files unchanged since their import.
    pub unchanged: usize,
}

/// Plans the import of `dirs` against `state`.
pub fn plan(dirs: Vec<MemoryDir>, state: &ImportState) -> Vec<DirPlan> {
    dirs.into_iter()
        .map(|dir| {
            let mut new = Vec::new();
            let mut changed = Vec::new();
            let mut unchanged = 0;
            for file in &dir.files {
                match state.fingerprint_of(&file.path) {
                    None => new.push(file.clone()),
                    Some(known) if known != file.fingerprint => changed.push(file.clone()),
                    Some(_) => unchanged += 1,
                }
            }
            DirPlan {
                dir,
                new,
                changed,
                unchanged,
            }
        })
        .collect()
}

/// Totals of a plan: (directories with a project, skipped directories, new, changed,
/// unchanged).
pub fn totals(plans: &[DirPlan]) -> (usize, usize, usize, usize, usize) {
    let mut out = (0, 0, 0, 0, 0);
    for plan in plans {
        if plan.dir.project.is_some() {
            out.0 += 1;
            out.2 += plan.new.len();
            out.3 += plan.changed.len();
            out.4 += plan.unchanged;
        } else {
            out.1 += 1;
        }
    }
    out
}

/// A connection to the memory functions, bound to one project.
pub trait Caller {
    /// Calls the memory function `name`.
    fn call(&mut self, name: &str, arguments: Value) -> Result<Value, String>;
}

impl<R: std::io::BufRead, W: std::io::Write> Caller for crate::client::McpClient<R, W> {
    fn call(&mut self, name: &str, arguments: Value) -> Result<Value, String> {
        crate::client::McpClient::call(self, name, arguments)
    }
}

/// Opens a connection bound to a project.
pub type Connect<'a> = dyn FnMut(&str) -> Result<Box<dyn Caller>, String> + 'a;

/// An owned connector that can move to another thread.
pub type SendConnect = Box<dyn FnMut(&str) -> Result<Box<dyn Caller>, String> + Send>;

/// What an import did.
#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    /// Memories created.
    pub created: usize,
    /// Memories replaced.
    pub updated: usize,
    /// Files unchanged since their import (not sent).
    pub unchanged: usize,
    /// Projects written to.
    pub projects: BTreeSet<String>,
    /// Directories skipped (no project found), by name.
    pub skipped_dirs: Vec<String>,
    /// Files whose kind was missing or unknown.
    pub kind_defaulted: usize,
    /// Failures: a memory that could not be written, a project that could not be reached.
    pub problems: Vec<String>,
    /// Files that could not be read or parsed (skipped).
    pub warnings: Vec<String>,
    /// Whether the deadline stopped the import early.
    pub timed_out: bool,
    /// Memories written that wait for their conflict check (their nearest memories are for the
    /// agent to judge: `conflicts()`).
    pub unchecked: usize,
    /// Files whose memory was forgotten in the graph and whose text did not change: not
    /// imported again (only their index line moved).
    pub kept_forgotten: usize,
    /// The open conflicts of the projects written to, one line each naming both memories and
    /// their files.
    pub conflicts: Vec<String>,
    /// Files that still hold a superseded memory, one line each (edit or delete them).
    pub stale_files: Vec<String>,
}

/// Runs `plans`: every new or changed file goes through `remember` (in index order, then a
/// second call for a memory whose links point to memories created after it), and its
/// fingerprint goes into `state` (saved to `data_dir` after each directory).
pub fn apply(
    plans: &[DirPlan],
    connect: &mut Connect<'_>,
    state: &mut ImportState,
    data_dir: &Path,
    deadline: Option<Instant>,
) -> ImportReport {
    let mut report = ImportReport::default();
    for plan in plans {
        report.warnings.extend(plan.dir.problems.iter().cloned());
        let Some(project) = &plan.dir.project else {
            report.skipped_dirs.push(plan.dir.name.clone());
            continue;
        };
        report.unchanged += plan.unchanged;
        let mut todo: Vec<&MemoryFile> = plan.new.iter().chain(plan.changed.iter()).collect();
        if todo.is_empty() {
            continue;
        }
        todo.sort_by(|a, b| (a.order, &a.path).cmp(&(b.order, &b.path)));
        let mut caller = match connect(project) {
            Ok(caller) => caller,
            Err(err) => {
                report.problems.push(format!(
                    "{}: cannot reach the memory: {err}",
                    plan.dir.dir.display()
                ));
                continue;
            }
        };
        let written: BTreeSet<&str> = todo.iter().map(|f| f.slug.as_str()).collect();
        let mut again = Vec::new();
        for file in &todo {
            if deadline.is_some_and(|d| Instant::now() >= d) {
                report.timed_out = true;
                break;
            }
            // A memory forgotten in the graph whose file only moved in MEMORY.md stays
            // forgotten; one whose file text is unchanged is checked in the graph first.
            match state.forgotten_unless_changed(file) {
                Some(true) => {
                    state.record_forgotten(file, project);
                    report.kept_forgotten += 1;
                    continue;
                }
                Some(false) => {
                    if let Ok(found) = caller.call("memory", json!({"slug": file.slug}))
                        && found
                            .get("error")
                            .and_then(Value::as_str)
                            .is_some_and(|e| e.starts_with("no memory"))
                    {
                        state.record_forgotten(file, project);
                        report.kept_forgotten += 1;
                        continue;
                    }
                }
                None => {}
            }
            let pinned = state.pinned_arg(file);
            let order = state.order_arg(file);
            match caller.call(
                "remember",
                file.remember_args(pinned.clone(), order.clone()),
            ) {
                Ok(answer) => {
                    if answer["created"] == Value::Bool(true) {
                        report.created += 1;
                    } else {
                        report.updated += 1;
                    }
                    if file.kind_defaulted {
                        report.kind_defaulted += 1;
                    }
                    if answer["unchecked"] == Value::Bool(true) {
                        report.unchecked += 1;
                    }
                    state.record(file, project);
                    let missing: Vec<&str> = answer["missing_targets"]
                        .as_array()
                        .map(|m| m.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    if missing.iter().any(|slug| written.contains(slug)) {
                        again.push((*file, pinned, order));
                    }
                }
                Err(err) => report
                    .problems
                    .push(format!("{}: {err}", file.path.display())),
            }
        }
        // Links to memories created later in this import resolve on the second call.
        for (file, pinned, order) in again {
            if deadline.is_some_and(|d| Instant::now() >= d) {
                report.timed_out = true;
                break;
            }
            if let Err(err) = caller.call("remember", file.remember_args(pinned, order)) {
                report
                    .problems
                    .push(format!("{}: {err}", file.path.display()));
            }
        }
        // What waits for a decision in this project now: open conflicts and files that still
        // hold a superseded memory, named with their files.
        if !report.timed_out
            && let Ok(listed) = caller.call("conflicts", json!({"k": 1}))
        {
            let (conflicts, stale) = conflict_report(&listed);
            for line in conflicts {
                if !report.conflicts.contains(&line) {
                    report.conflicts.push(line);
                }
            }
            for line in stale {
                if !report.stale_files.contains(&line) {
                    report.stale_files.push(line);
                }
            }
        }
        report.projects.insert(project.clone());
        if let Err(err) = state.save(data_dir) {
            report.problems.push(err);
        }
        if report.timed_out {
            break;
        }
    }
    report
}

/// The lines of a `conflicts()` answer: the open conflicts (both memories with their files)
/// and the files that still hold a superseded memory.
pub fn conflict_report(listed: &Value) -> (Vec<String>, Vec<String>) {
    let side = |m: &Value| {
        let slug = m["slug"].as_str().unwrap_or_default();
        match m["source"].as_str() {
            Some(source) if source != "remember" && !source.is_empty() => {
                format!("{slug} (file {source})")
            }
            _ => format!("{slug} (remember)"),
        }
    };
    let conflicts = listed["open"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| format!("{} vs {}", side(&row["a"]), side(&row["b"])))
        .collect();
    let stale = listed["stale_files"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| {
            format!(
                "{} (superseded by {}): {}",
                row["slug"].as_str().unwrap_or_default(),
                row["superseded_by"].as_str().unwrap_or_default(),
                row["file"].as_str().unwrap_or_default()
            )
        })
        .collect();
    (conflicts, stale)
}

/// The marker of the one-time source migration ([`migrate_sources`]).
const MIGRATED_FILE: &str = "sources-migrated";

/// Once per data directory: memories imported before the import marked them (source `user`,
/// no `imported_at`) get the file they came from as `source` and their import time from the
/// import's records, so conflicts name their files. A marker file in `<data dir>/import/`
/// keeps it from running again (an agent's later `remember` of such a memory must not be
/// marked again). Returns how many memories it changed.
pub fn migrate_sources(
    data_dir: &Path,
    call: &dyn Fn(&str, Value) -> Result<Value, String>,
) -> Result<u64, String> {
    let marker = data_dir.join(STATE_DIR).join(MIGRATED_FILE);
    if marker.exists() {
        return Ok(0);
    }
    let records = ImportState::load(data_dir)?.records();
    let migrated = if records.is_empty() {
        0
    } else {
        call("migrate_import_sources", json!({"records": records}))?["migrated"]
            .as_u64()
            .unwrap_or(0)
    };
    let dir = data_dir.join(STATE_DIR);
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    fsutil::write_atomic(&marker, format!("{migrated}\n").as_bytes())?;
    Ok(migrated)
}

/// Runs `work` holding the import lock of `data_dir`, taken within `wait`; `Ok(None)` when
/// another import holds it all that time.
pub fn with_lock<T>(
    data_dir: &Path,
    wait: Duration,
    work: impl FnOnce() -> T,
) -> Result<Option<T>, String> {
    let dir = data_dir.join(STATE_DIR);
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(LOCK_FILE))
        .map_err(|err| format!("cannot open the import lock: {err}"))?;
    let mut lock = fd_lock::RwLock::new(file);
    let deadline = Instant::now() + wait;
    loop {
        match lock.try_write() {
            Ok(_guard) => return Ok(Some(work())),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(err) => return Err(format!("cannot take the import lock: {err}")),
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_claude_names() {
        assert_eq!(slugify("My_Memory File"), "my-memory-file");
        assert_eq!(slugify("--a--b--"), "a-b");
        assert_eq!(slugify("Ünï"), "n");
        assert_eq!(
            claude_slug("/Users/a/Application Support/x.y"),
            "-Users-a-Application-Support-x-y"
        );
    }

    #[test]
    fn index_lines_give_title_hook_and_order() {
        let index = index_lines(
            "<!-- header -->\n## How\n- [Graph first](graph-first.md) — study the graph\n\
             - [No hook](no-hook.md)\nnot a line\n- [Sub](dir/sub.md) - dash hook\n",
        );
        assert_eq!(index["graph-first"].title, "Graph first");
        assert_eq!(index["graph-first"].hook, "study the graph");
        assert_eq!(index["graph-first"].position, 0);
        assert_eq!(index["no-hook"].hook, "");
        assert_eq!(index["sub"].hook, "dash hook");
        assert_eq!(index["sub"].position, 2);
    }

    #[test]
    fn the_pinned_section_lists_its_files_up_to_the_next_heading() {
        let text = "# Index\n## How to work\n- [A](a.md) — x\n## Pinned\n- [B](b.md) — y\n\
                    - [C](dir/c.md)\n### Sub heading\n- [D](d.md)\n## pinned\n- [E](e.md)\n";
        let pinned: Vec<String> = pinned_stems(text).into_iter().collect();
        assert_eq!(pinned, ["b", "c", "e"]);
        assert!(pinned_stems("- [A](a.md)\n").is_empty());
    }

    #[test]
    fn a_pinned_file_gets_its_own_fingerprint_and_the_others_keep_theirs() {
        let dir = crate::mcp_tests::TempDir::new("pinned-import");
        let memory = dir.path().join("memory");
        std::fs::create_dir_all(&memory).unwrap();
        std::fs::write(
            memory.join("a.md"),
            "---\ndescription: A\ntype: feedback\n---\nA.\n",
        )
        .unwrap();
        std::fs::write(
            memory.join("b.md"),
            "---\ndescription: B\ntype: project\n---\nB.\n",
        )
        .unwrap();
        std::fs::write(
            memory.join("MEMORY.md"),
            "- [A](a.md) — a\n- [B](b.md) — b\n",
        )
        .unwrap();
        let before = read_dir(&memory, Some("/p".into()));
        std::fs::write(
            memory.join("MEMORY.md"),
            "- [A](a.md) — a\n## Pinned\n- [B](b.md) — b\n",
        )
        .unwrap();
        let after = read_dir(&memory, Some("/p".into()));
        assert_eq!(before.files[0].fingerprint, after.files[0].fingerprint);
        assert!(!after.files[0].pinned && after.files[1].pinned);
        assert_ne!(before.files[1].fingerprint, after.files[1].fingerprint);
        assert_eq!(
            after.files[1].remember_args(Value::Bool(true), Value::Null)["pinned"],
            true
        );
    }

    #[test]
    fn a_memory_file_is_parsed() {
        let text = "---\nname: ci-manual\ndescription: \"CI: manual only\"\nmetadata:\n  type: feedback\n  graph_hash: 1\n---\n\n\
                    CI runs on dispatch. See [[Release-Plan]] and [[ci-manual]].\n\n**Why:** minutes cost.\n**How to apply:** never add push triggers.\n";
        let entry = IndexEntry {
            title: "CI manual".into(),
            hook: "dispatch only".into(),
            position: 3,
        };
        let file = parse_file(
            Path::new("/m/ci-manual.md"),
            "ci-manual",
            text,
            Some(&entry),
            9,
        )
        .unwrap();
        assert_eq!(file.slug, "ci-manual");
        assert_eq!(file.kind, "feedback");
        assert!(!file.kind_defaulted);
        assert_eq!(file.summary, "CI: manual only");
        assert_eq!(
            file.body,
            "CI runs on dispatch. See [[Release-Plan]] and [[ci-manual]]."
        );
        assert_eq!(file.why, "minutes cost.");
        assert_eq!(file.how_to_apply, "never add push triggers.");
        assert_eq!(file.links, vec!["release-plan".to_string()]);
        assert_eq!((file.title.as_str(), file.order), ("CI manual", 3));

        let plain = parse_file(Path::new("/m/x.md"), "X", "# Heading\n\nText.", None, 9).unwrap();
        assert_eq!(plain.kind, "project");
        assert!(plain.kind_defaulted);
        assert_eq!(plain.summary, "Heading");
        assert_eq!(plain.body, "# Heading\n\nText.");
        assert_eq!(plain.order, 9);
        let old_style = parse_file(
            Path::new("/m/u.md"),
            "u",
            "---\ntype: user\n---\nbody",
            None,
            0,
        )
        .unwrap();
        assert_eq!(old_style.kind, "user");
    }

    /// The text helpers cut only at ASCII markers, so multi-byte text around them stays whole
    /// (the byte-slicing pattern of the engine's B1 panic, checked here for the product).
    #[test]
    fn non_ascii_text_around_the_markers_stays_whole() {
        let body = "Решение е(1 🎉\u{a0}(x\n\n**Why:** потому что е(\n\n**How to apply:** \u{a0}всегда 🎉(";
        let (main, why, how) = split_why(body);
        assert_eq!(main, "Решение е(1 🎉\u{a0}(x");
        assert_eq!(why, "потому что е(");
        assert_eq!(how, "всегда 🎉(");
        // Links are slugified to ASCII: only `x` is left of these three.
        assert_eq!(
            wiki_links("см. [[релиз-е(]] и [[🎉]] и [[\u{a0}x]]"),
            vec!["x".to_string()]
        );
        assert_eq!(unquote("\"е(1 🎉\""), "е(1 🎉");
        assert_eq!(unquote("е\""), "е\"");
        let (front, rest) = split_front_matter("---\nname: е(\n---\nтело 🎉(\n");
        assert_eq!((front, rest), ("name: е(", "тело 🎉(\n"));
    }

    #[test]
    fn why_is_split_only_from_a_clean_tail() {
        assert_eq!(
            split_why("a\n\n**Why:** b"),
            ("a".into(), "b".into(), String::new())
        );
        assert_eq!(
            split_why("a\n**How to apply:** c"),
            ("a".into(), String::new(), "c".into())
        );
        let twice = "**Why:** x **Why:** y";
        assert_eq!(split_why(twice).0, twice);
        let reversed = "**How to apply:** c\n**Why:** b";
        assert_eq!(split_why(reversed).0, reversed);
    }

    #[test]
    fn a_project_name_decodes_against_the_file_system() {
        let tmp = crate::mcp_tests::TempDir::new("decode");
        let deep = tmp.path().join("my-app").join("sub dir");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(tmp.path().join("my")).unwrap();
        let root = crate::mcp::canonical(tmp.path());
        let target = root.join("my-app").join("sub dir");
        let name = claude_slug(&target.display().to_string());
        assert_eq!(decode(&name), Some(target));
        assert_eq!(decode(&format!("{name}-missing")), None);
    }

    #[test]
    fn transcripts_name_the_project_directory() {
        let tmp = crate::mcp_tests::TempDir::new("cwd");
        let work = crate::mcp::canonical(tmp.path()).join("work");
        std::fs::create_dir_all(&work).unwrap();
        let name = claude_slug(&work.display().to_string());
        let project_dir = tmp.path().join("projects").join(&name);
        std::fs::create_dir_all(project_dir.join("memory")).unwrap();
        std::fs::write(
            project_dir.join("s.jsonl"),
            format!(
                "not json\n{}\n",
                json!({"type": "user", "cwd": work.display().to_string()})
            ),
        )
        .unwrap();
        assert_eq!(resolve_project_dir(&project_dir), Some(work));
    }

    #[test]
    fn the_lock_is_exclusive() {
        let tmp = crate::mcp_tests::TempDir::new("ilock");
        let inner = with_lock(tmp.path(), Duration::ZERO, || {
            with_lock(tmp.path(), Duration::from_millis(100), || ()).unwrap()
        })
        .unwrap();
        assert_eq!(inner, Some(None), "a second taker waits and gives up");
        assert_eq!(
            with_lock(tmp.path(), Duration::ZERO, || 7).unwrap(),
            Some(7)
        );
    }
}
