//! CodeWhale's native memory, read only: detection for `status`/`doctor`/install and the import
//! of its reviewed entries into graphersal-memory (`crate::install`'s import step and `import`).
//!
//! The store is SQLite, `<memory root>/store.sqlite3` (`crate::harnesses::codewhale`), schema
//! `crates/memory/src/sql/schema.sql` of CodeWhale v0.10.2: table `memories` (`id`, `scope` =
//! a JSON object `{"tenant", "user", "workspace"?, ...}`, `kind` = preference | fact |
//! decision | constraint | procedure | lesson | commitment | episode | handoff, `status` =
//! candidate | active | stale | superseded | rejected, `title`, `body`, `updated_at` in
//! seconds), and `memory_preferences` (`pinned`, `suppressed`) since its second migration. A
//! memory the model writes stays a `candidate` until the user reviews it; only `active` and not
//! suppressed entries are imported. Text is immutable there: a correction is a new entry and the
//! old one becomes `superseded`, so the import follows `active` only.
//!
//! A workspace's id is the SHA-256 (lower-case hex) of its `git config --get
//! remote.origin.url`, the raw text trimmed (`native_memory.rs`, `workspace_id`). The import maps
//! it back to a project graphersal-memory knows (the paths of `projects.json` and the current
//! project) by computing the same for each; entries of a workspace not found are left out and
//! counted. Each entry becomes the memory `codewhale-<title>-<id prefix>` (the prefix keeps two
//! entries with one title apart and an entry from replacing a memory of the user's), with
//! `from_file` = the store, and its fingerprint is recorded in
//! `<data dir>/import/codewhale-memory.json` so a later import sends only new and changed
//! entries. The store is opened read-only and never changed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

use crate::memory::import::{Caller, Connect, slugify};
use crate::{fsutil, install::jsonedit};

/// The state file in `<data dir>/import/`.
const STATE_FILE: &str = "codewhale-memory.json";

/// One reviewed entry of the native memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Its id in the store.
    pub id: String,
    /// Its workspace id (`None`: global).
    pub workspace: Option<String>,
    /// CodeWhale's kind.
    pub kind: String,
    /// Its title.
    pub title: String,
    /// Its text.
    pub body: String,
    /// Whether the user pinned it.
    pub pinned: bool,
    /// When it was last changed (seconds).
    pub updated_at: i64,
}

impl Entry {
    /// The memory's slug.
    pub fn slug(&self) -> String {
        let mut title = slugify(&self.title);
        if title.len() > 50 {
            title.truncate(50);
            title = title.trim_end_matches('-').to_string();
        }
        let prefix: String = self
            .id
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(6)
            .collect::<String>()
            .to_ascii_lowercase();
        match (title.is_empty(), prefix.is_empty()) {
            (true, _) => format!("codewhale-{prefix}"),
            (false, true) => format!("codewhale-{title}"),
            (false, false) => format!("codewhale-{title}-{prefix}"),
        }
    }

    /// The graphersal-memory kind of CodeWhale's: how to work (preference, constraint,
    /// procedure, lesson) = feedback; a global fact = user; the rest = project (reference when
    /// global).
    pub fn our_kind(&self) -> &'static str {
        match (self.kind.as_str(), self.workspace.is_some()) {
            ("preference" | "constraint" | "procedure" | "lesson", _) => "feedback",
            ("fact", false) => "user",
            (_, true) => "project",
            (_, false) => "reference",
        }
    }

    /// What is imported, fingerprinted.
    pub fn fingerprint(&self) -> String {
        fsutil::fingerprint(
            format!(
                "{}\n{}\n{}\n{}",
                self.our_kind(),
                self.title,
                self.body,
                self.pinned
            )
            .as_bytes(),
        )
    }
}

/// The reviewed (active, not suppressed) entries of the store at `store`.
pub fn read(store: &Path) -> Result<Vec<Entry>, String> {
    let fail = |err: rusqlite::Error| {
        format!(
            "cannot read CodeWhale's memory store {}: {err}",
            store.display()
        )
    };
    let connection = Connection::open_with_flags(
        store,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(fail)?;
    let _ = connection.busy_timeout(std::time::Duration::from_secs(5));
    let has_preferences: bool = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'memory_preferences'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(fail)?
        > 0;
    let sql = if has_preferences {
        "SELECT m.id, m.scope, m.kind, m.title, m.body, m.updated_at, COALESCE(p.pinned, 0) \
         FROM memories m LEFT JOIN memory_preferences p ON p.memory_id = m.id \
         WHERE m.status = 'active' AND COALESCE(p.suppressed, 0) = 0 ORDER BY m.rowid"
    } else {
        "SELECT id, scope, kind, title, body, updated_at, 0 FROM memories \
         WHERE status = 'active' ORDER BY rowid"
    };
    let mut statement = connection.prepare(sql).map_err(fail)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })
        .map_err(fail)?;
    let mut out = Vec::new();
    for row in rows {
        let (id, scope, kind, title, body, updated_at, pinned) = row.map_err(fail)?;
        let scope: Value = serde_json::from_str(&scope).unwrap_or(Value::Null);
        // Entries of a session or an agent scope are not the user's memory.
        if !scope["session"].is_null() || !scope["agent"].is_null() {
            continue;
        }
        out.push(Entry {
            id,
            workspace: scope["workspace"].as_str().map(str::to_string),
            kind,
            title: title.trim().to_string(),
            body: body.trim().to_string(),
            pinned: pinned != 0,
            updated_at,
        });
    }
    Ok(out)
}

/// The SHA-256 of `text`, lower-case hex.
pub fn sha256_hex(text: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, text.as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// CodeWhale's workspace id of the repository at `root` (`None` without an origin).
pub fn workspace_id(root: &Path) -> Option<String> {
    let output = crate::harnesses::headless::without_window(&mut std::process::Command::new("git"))
        .arg("-C")
        .arg(root)
        .args(["config", "--get", "remote.origin.url"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !url.is_empty()).then(|| sha256_hex(&url))
}

/// The project roots graphersal-memory knows (`projects.json`'s paths that exist, and `extra`),
/// by CodeWhale's workspace id.
pub fn workspace_roots(data_dir: &Path, extra: Option<&Path>) -> BTreeMap<String, PathBuf> {
    let mut roots: Vec<PathBuf> = crate::memory::projects::Projects::load(data_dir)
        .map(|p| p.list())
        .unwrap_or_default()
        .into_iter()
        .flat_map(|project| project.paths.into_iter().map(PathBuf::from))
        .collect();
    roots.extend(extra.map(Path::to_path_buf));
    let mut out = BTreeMap::new();
    for root in roots.into_iter().filter(|r| r.is_dir()) {
        if let Some(id) = workspace_id(&root) {
            out.entry(id).or_insert(root);
        }
    }
    out
}

/// The native memory's importable entries, with their projects.
#[derive(Debug, Clone, Default)]
pub struct Native {
    /// The store.
    pub store: PathBuf,
    /// The entries to import, each with its project root (`None`: global).
    pub items: Vec<(Entry, Option<PathBuf>)>,
    /// Entries of workspaces no known project matches (left out).
    pub unmapped: usize,
    /// Why the store could not be read.
    pub error: Option<String>,
}

impl Native {
    /// The native memory at `store` (`None` when there is no store), its workspaces mapped
    /// to the projects of `data_dir` and `project`.
    pub fn load(store: &Path, data_dir: &Path, project: Option<&Path>) -> Option<Self> {
        if !store.is_file() {
            return None;
        }
        let mut out = Native {
            store: store.to_path_buf(),
            ..Native::default()
        };
        match read(store) {
            Ok(entries) => {
                let needs_roots = entries.iter().any(|e| e.workspace.is_some());
                let roots = if needs_roots {
                    workspace_roots(data_dir, project)
                } else {
                    BTreeMap::new()
                };
                for entry in entries {
                    match &entry.workspace {
                        None => out.items.push((entry, None)),
                        Some(id) => match roots.get(id) {
                            Some(root) => {
                                let root = root.clone();
                                out.items.push((entry, Some(root)));
                            }
                            None => out.unmapped += 1,
                        },
                    }
                }
            }
            Err(err) => out.error = Some(err),
        }
        Some(out)
    }

    /// How many entries are new or changed since their import.
    pub fn pending(&self, data_dir: &Path) -> usize {
        let state = load_state(data_dir);
        self.items
            .iter()
            .filter(|(entry, _)| is_pending(&state, entry))
            .count()
    }

    /// One line for install's plan and `status`.
    pub fn describe(&self, data_dir: &Path) -> String {
        if let Some(err) = &self.error {
            return err.clone();
        }
        let mut line = format!(
            "{} reviewed entr{} in {}, {} new or changed",
            self.items.len(),
            if self.items.len() == 1 { "y" } else { "ies" },
            self.store.display(),
            self.pending(data_dir)
        );
        if self.unmapped > 0 {
            line.push_str(&format!(
                "; {} of repositories graphersal-memory has not seen yet (imported once a \
                 session ran there)",
                self.unmapped
            ));
        }
        line
    }
}

fn state_path(data_dir: &Path) -> PathBuf {
    data_dir.join("import").join(STATE_FILE)
}

fn load_state(data_dir: &Path) -> serde_json::Map<String, Value> {
    std::fs::read(state_path(data_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value["items"].as_object().cloned())
        .unwrap_or_default()
}

fn is_pending(state: &serde_json::Map<String, Value>, entry: &Entry) -> bool {
    state
        .get(&entry.id)
        .and_then(|item| item["fingerprint"].as_str())
        != Some(entry.fingerprint().as_str())
}

/// The slugs of every imported entry (the export leaves them out of the harness's files, as
/// it does for other tools' memories).
pub fn imported_slugs(data_dir: &Path) -> std::collections::BTreeSet<String> {
    load_state(data_dir)
        .values()
        .filter_map(|item| item["slug"].as_str().map(str::to_string))
        .collect()
}

/// What an import of the native memory did.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Memories created.
    pub created: usize,
    /// Memories replaced.
    pub updated: usize,
    /// Entries unchanged since their import.
    pub unchanged: usize,
    /// Failures.
    pub problems: Vec<String>,
    /// Changed entries whose memory was edited in graphersal-memory: the edit was kept.
    pub kept_edited: Vec<String>,
}

impl Report {
    /// One line for the report.
    pub fn line(&self, native: &Native) -> String {
        let mut line = format!(
            "CodeWhale's memory: imported {} entr{} ({} new, {} updated; {} unchanged) from {}",
            self.created + self.updated,
            if self.created + self.updated == 1 {
                "y"
            } else {
                "ies"
            },
            self.created,
            self.updated,
            self.unchanged,
            native.store.display()
        );
        if native.unmapped > 0 {
            line.push_str(&format!(
                "; {} of unknown repositories left out",
                native.unmapped
            ));
        }
        if !self.kept_edited.is_empty() {
            line.push_str(&format!(
                "; {} changed but edited in graphersal-memory, the edit kept",
                self.kept_edited.len()
            ));
        }
        line
    }
}

impl crate::harnesses::agent::NativeMemory for Native {
    fn name(&self) -> &'static str {
        "CodeWhale's memory"
    }

    fn pending(&self, data_dir: &Path) -> usize {
        Native::pending(self, data_dir)
    }

    fn describe(&self, data_dir: &Path) -> String {
        Native::describe(self, data_dir)
    }

    fn failed(&self) -> bool {
        self.error.is_some()
    }

    fn import(
        &self,
        connect: &mut Connect<'_>,
        data_dir: &Path,
    ) -> crate::harnesses::agent::NativeImport {
        let report = apply(self, connect, data_dir);
        crate::harnesses::agent::NativeImport {
            line: report.line(self),
            kept_edited: report.kept_edited,
            problems: report.problems,
        }
    }
}

/// Imports the new and changed entries of `native` (one connection per project, one without a
/// project for the global ones), recording each in the state of `data_dir`.
pub fn apply(native: &Native, connect: &mut Connect<'_>, data_dir: &Path) -> Report {
    let mut report = Report::default();
    let mut state = load_state(data_dir);
    let modified_at = std::fs::metadata(&native.store)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| crate::runtime::daemon::utc_timestamp(since.as_secs()))
        .unwrap_or_default();
    let dir = native
        .store
        .parent()
        .map(|d| format!("{}{}", d.display(), std::path::MAIN_SEPARATOR))
        .unwrap_or_default();
    let mut callers: BTreeMap<String, Box<dyn Caller>> = BTreeMap::new();
    for (entry, root) in &native.items {
        if !is_pending(&state, entry) {
            report.unchanged += 1;
            continue;
        }
        let key = root
            .as_ref()
            .map(|r| r.display().to_string())
            .unwrap_or_default();
        if !callers.contains_key(&key) {
            match connect(&key) {
                Ok(caller) => {
                    callers.insert(key.clone(), caller);
                }
                Err(err) => {
                    report
                        .problems
                        .push(format!("cannot reach the memory: {err}"));
                    continue;
                }
            }
        }
        let Some(caller) = callers.get_mut(&key) else {
            continue;
        };
        let slug = entry.slug();
        let summary = if entry.title.is_empty() {
            entry.body.lines().next().unwrap_or_default().to_string()
        } else {
            entry.title.clone()
        };
        let args = json!({
            "slug": slug, "kind": entry.our_kind(), "summary": summary,
            "body": entry.body, "why": "", "how_to_apply": "", "title": entry.title,
            "hook": "", "pinned": entry.pinned.then_some(true), "order": null, "links": [],
            "scope": if root.is_some() { "project" } else { "global" },
            "from_file": {"path": native.store.display().to_string(),
                          "modified_at": modified_at, "dir": dir},
        });
        let record = json!({"fingerprint": entry.fingerprint(), "slug": slug,
                            "imported_at": crate::runtime::daemon::now_utc()});
        match caller.call("remember", args) {
            Ok(answer) if answer["kept"] == "edited" => {
                report
                    .kept_edited
                    .push(crate::memory::import::kept_edited_line(
                        &native.store.display().to_string(),
                        &slug,
                    ));
                state.insert(entry.id.clone(), record);
            }
            Ok(answer) => {
                if answer["created"] == Value::Bool(true) {
                    report.created += 1;
                } else {
                    report.updated += 1;
                }
                state.insert(entry.id.clone(), record);
            }
            Err(err) => report.problems.push(format!("{slug}: {err}")),
        }
    }
    let path = state_path(data_dir);
    let saved = path
        .parent()
        .map_or(Ok(()), |d| {
            std::fs::create_dir_all(d)
                .map_err(|err| format!("cannot create {}: {err}", d.display()))
        })
        .and_then(|()| {
            let value = json!({"version": 1, "items": state});
            fsutil::write_atomic(
                &path,
                format!("{}\n", jsonedit::render(&value, Some("  "), 0)).as_bytes(),
            )
        });
    if let Err(err) = saved {
        report.problems.push(err);
    }
    report
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A store in the schema of CodeWhale v0.10.2 (the tables and columns read here) at `path`,
    /// with `rows` (id, scope workspace, kind, status, title, body).
    /// One row of a test store: id, workspace, kind, status, title, body.
    pub(crate) type Row<'a> = (&'a str, Option<&'a str>, &'a str, &'a str, &'a str, &'a str);

    pub(crate) fn write_store(path: &Path, rows: &[Row<'_>]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE memories (rowid INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, \
                 scope TEXT NOT NULL, kind TEXT NOT NULL, semantic_key TEXT, status TEXT NOT NULL, \
                 revision INTEGER NOT NULL, title TEXT NOT NULL, body TEXT NOT NULL, tags TEXT NOT \
                 NULL, payload TEXT NOT NULL, content_hash TEXT NOT NULL, repo_revision TEXT, \
                 importance REAL NOT NULL, confidence REAL NOT NULL, created_at INTEGER NOT NULL, \
                 updated_at INTEGER NOT NULL, expires_at INTEGER);
                 CREATE TABLE memory_preferences (memory_id TEXT PRIMARY KEY, revision INTEGER NOT \
                 NULL DEFAULT 1, pinned INTEGER NOT NULL DEFAULT 0, suppressed INTEGER NOT NULL \
                 DEFAULT 0);
                 PRAGMA application_id=1129794866; PRAGMA user_version=2;",
            )
            .unwrap();
        for (id, workspace, kind, status, title, body) in rows {
            let mut scope = json!({"tenant": "local", "user": "owner"});
            if let Some(workspace) = workspace {
                scope["workspace"] = json!(workspace);
            }
            connection
                .execute(
                    "INSERT INTO memories (id, scope, kind, status, revision, title, body, tags, \
                     payload, content_hash, importance, confidence, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, '[]', '{}', 'h', 0.5, 0.9, 1, 2)",
                    rusqlite::params![id, scope.to_string(), kind, status, title, body],
                )
                .unwrap();
        }
    }

    #[test]
    fn only_reviewed_entries_are_read() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("cw-native");
        let store = tmp.path().join("memory").join("store.sqlite3");
        write_store(
            &store,
            &[
                (
                    "m-1",
                    None,
                    "preference",
                    "active",
                    "Tabs",
                    "Use tabs, never spaces.",
                ),
                (
                    "m-2",
                    None,
                    "fact",
                    "candidate",
                    "Pending",
                    "Not reviewed yet.",
                ),
                (
                    "m-3",
                    Some("abc"),
                    "decision",
                    "active",
                    "DB",
                    "SQLite for the cache.",
                ),
                ("m-4", None, "fact", "superseded", "Old", "Replaced."),
                (
                    "m-5",
                    None,
                    "fact",
                    "active",
                    "Hidden",
                    "Suppressed by the user.",
                ),
            ],
        );
        let connection = Connection::open(&store).unwrap();
        connection
            .execute(
                "INSERT INTO memory_preferences (memory_id, suppressed) VALUES ('m-5', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO memory_preferences (memory_id, pinned) VALUES ('m-1', 1)",
                [],
            )
            .unwrap();
        drop(connection);
        let before = std::fs::read(&store).unwrap();
        let entries = read(&store).unwrap();
        assert_eq!(
            entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            vec!["m-1", "m-3"]
        );
        assert!(entries[0].pinned);
        assert_eq!(entries[0].our_kind(), "feedback");
        assert_eq!(entries[0].slug(), "codewhale-tabs-m1");
        assert_eq!(entries[1].workspace.as_deref(), Some("abc"));
        assert_eq!(entries[1].our_kind(), "project");
        assert_eq!(std::fs::read(&store).unwrap(), before, "read only");
        assert!(read(&tmp.path().join("missing.sqlite3")).is_err());
    }

    #[test]
    fn workspace_ids_are_codewhales() {
        // native_memory.rs: sha256 of the raw origin URL.
        assert_eq!(
            sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let tmp = crate::runtime::mcp_tests::TempDir::new("cw-ws");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !git(&["init", "-q"]) {
            return; // no git on this machine
        }
        assert_eq!(workspace_id(&repo), None, "no origin");
        assert!(git(&["remote", "add", "origin", "git@example.com:a/b.git"]));
        assert_eq!(
            workspace_id(&repo),
            Some(sha256_hex("git@example.com:a/b.git"))
        );
    }
}
