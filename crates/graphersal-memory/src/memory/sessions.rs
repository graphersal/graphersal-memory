//! Indexing of past sessions: every finished Claude Code transcript
//! (`<claude config>/projects/<project>/<session>.jsonl`, parsed by `crate::memory::transcript`) and every
//! finished opencode session (its database, read by `crate::harnesses::opencode::sessions`) and every
//! finished Codex rollout (`$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl`, read by
//! `crate::harnesses::codex::sessions`) and every finished Cursor transcript its hooks reported (the
//! registry `<data dir>/sessions/cursor-transcripts.json`, read by `crate::harnesses::cursor::sessions`)
//! becomes a
//! `session` vertex with its `turn`s, and, when the user opted in, an English summary split into
//! `fact`s (`crate::memory::summaries`). A [`Request`] names one harness; each harness has its own state
//! file. An opencode session is addressed like a file, as `<opencode.db>#<session id>`
//! ([`crate::harnesses::opencode::sessions::unit_path`]), so the hooks' `active`/`ended` lists and the state
//! work the same for both.
//!
//! - **Finished**: a transcript unchanged for [`IDLE`] (30 minutes), or one a `SessionEnd` hook
//!   named; a transcript the current session writes (`active`) is left alone. A session that
//!   grows later (a resumed session) gets a new fingerprint and is indexed again.
//! - **Idempotent**: the session vertex carries the fingerprint of the transcript lines its turns
//!   came from (set by the last batch, so a run that stopped half-way is redone), and
//!   `<data dir>/sessions/claude-code.json` remembers each file's size, time and fingerprint, so
//!   an unchanged file is not even read again.
//! - **Small units**: a session is written as one call for its properties and calls of at most
//!   [`BATCH_TURNS`] turns ([`BATCH_CHARS`] characters), each a short job on the store's session
//!   thread, so an agent's memory call never waits long behind the indexing. The daemon runs
//!   this on a thread of its own (`crate::runtime::daemon`); writes go through [`Writer`].
//! - **Memories written**: the calls of our memory tools a transcript records
//!   (`crate::memory::transcript::MemoryWrite`, every harness's reader) link the session to each memory
//!   it wrote (`wrote` edges, internal function `link_memories`, replaced as a whole per
//!   session). A session indexed before this existed is linked once, at the next run, from its
//!   transcript if it still exists (the state entry's `memory_links` is [`LINKS_VERSION`]
//!   afterwards; a repeated link changes nothing).
//! - **Project**: the transcript's working directory, mapped to the project key as a session
//!   started there would be (its git root); else the project directory's name, decoded like the
//!   file-memory import does. Every vertex carries the project, so sessions are scoped like
//!   memories.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::fsutil;
use crate::harnesses::agent::Kind;
use crate::memory::summaries::{self, Refused, Summarizer};
use crate::memory::transcript::{self, CAP, Transcript, clip};

/// A transcript unchanged for this long counts as finished.
pub const IDLE: Duration = Duration::from_secs(30 * 60);

/// The most turns written by one call.
pub const BATCH_TURNS: usize = 40;

/// The most characters of turn text written by one call.
pub const BATCH_CHARS: usize = 200_000;

/// How often a summary of one transcript version is tried before it waits for the session to
/// change.
const SUMMARY_ATTEMPTS: u64 = 2;

/// How often one version of a session file is tried before it is skipped: once, then once
/// more in a later run. A new version of the file, or a new version of this binary (which may
/// fix the cause), tries again.
const INDEX_ATTEMPTS: u64 = 2;

/// The state file's format version.
const FORMAT: u64 = 1;

/// The version of the session -> memory links an entry of the state file records
/// (`memory_links`): an indexed session whose entry has another one is linked again once.
pub const LINKS_VERSION: u64 = 1;

/// The writes and reads of the indexing, by project (the daemon routes each to the project's
/// store; tests call a session directly).
pub trait Writer {
    /// Runs the catalog function `name` for `project`'s store.
    fn call(&mut self, project: &str, name: &str, args: Value) -> Result<Value, String>;
}

/// What one indexing run is asked to do.
#[derive(Debug, Clone, Default)]
pub struct Request {
    /// The harness whose sessions these are.
    pub harness: Kind,
    /// Where the harness keeps its sessions: Claude Code's projects directory
    /// (`~/.claude/projects`), opencode's data directory (`~/.local/share/opencode`, holding
    /// `opencode.db`), Codex's `sessions/`; for Cursor the registry file of the transcripts its
    /// hooks reported.
    pub projects_dir: PathBuf,
    /// Transcripts known to be finished (a `SessionEnd` named them).
    pub ended: BTreeSet<PathBuf>,
    /// Transcripts being written by a running session.
    pub active: BTreeSet<PathBuf>,
    /// A transcript unchanged for this long counts as finished.
    pub idle: Duration,
}

/// The control request that asks the daemon to index (`crate::runtime::protocol`).
pub const CONTROL_INDEX: &str = "index_sessions";

/// The control request that reports the indexing's progress.
pub const CONTROL_STATUS: &str = "sessions_status";

impl Request {
    /// A request for the Claude Code transcripts under `projects_dir`, with the default idle
    /// time.
    pub fn new(projects_dir: &Path) -> Self {
        Self::for_harness(Kind::default(), projects_dir)
    }

    /// A request for the sessions of `harness` kept in `dir`, with the default idle time.
    pub fn for_harness(harness: Kind, dir: &Path) -> Self {
        Self {
            harness,
            projects_dir: dir.to_path_buf(),
            idle: IDLE,
            ..Self::default()
        }
    }

    /// The request as the daemon's control line.
    pub fn to_control(&self) -> Value {
        let paths = |set: &BTreeSet<PathBuf>| -> Vec<String> {
            set.iter().map(|p| p.display().to_string()).collect()
        };
        json!({
            "protocol": crate::runtime::protocol::PROTOCOL, "control": CONTROL_INDEX,
            "harness": self.harness.id(),
            "projects_dir": self.projects_dir.display().to_string(),
            "ended": paths(&self.ended), "active": paths(&self.active),
        })
    }

    /// The request of a control line (the idle time is always [`IDLE`]; without `harness`:
    /// Claude Code).
    pub fn from_control(line: &Value) -> Result<Self, String> {
        let harness = match line["harness"].as_str() {
            None => Kind::default(),
            Some(id) => Kind::from_id(id).ok_or_else(|| format!("unknown harness {id}"))?,
        };
        let dir = line["projects_dir"]
            .as_str()
            .filter(|dir| Path::new(dir).is_absolute())
            .ok_or("index_sessions needs an absolute projects_dir")?;
        let paths = |key: &str| -> BTreeSet<PathBuf> {
            line[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect()
        };
        Ok(Self {
            harness,
            projects_dir: PathBuf::from(dir),
            ended: paths("ended"),
            active: paths("active"),
            idle: IDLE,
        })
    }

    /// Adds a later request of the same harness to this one (a run not started yet does both).
    pub fn merge(&mut self, later: Request) {
        self.projects_dir = later.projects_dir;
        self.ended.extend(later.ended);
        self.active.extend(later.active);
        for ended in &self.ended {
            self.active.remove(ended);
        }
    }
}

/// What the indexing would do now, from the state alone (nothing is parsed or written):
/// transcripts found, still being written, new or changed, unchanged.
pub fn plan(request: &Request, data_dir: &Path) -> Result<(usize, usize, usize, usize), String> {
    let state = State::load_for(data_dir, request.harness)?;
    let now = SystemTime::now();
    let (mut found, mut active, mut todo, mut unchanged) = (0, 0, 0, 0);
    for Unit {
        path: file,
        size,
        modified,
    } in units(request)?
    {
        found += 1;
        if !finished(request, &file, modified, now) {
            active += 1;
            continue;
        }
        let entry = state.entry(&file).cloned().unwrap_or(Value::Null);
        if entry["size"].as_u64() == Some(size) && entry["mtime"].as_u64() == Some(modified) {
            unchanged += 1;
        } else {
            todo += 1;
        }
    }
    Ok((found, active, todo, unchanged))
}

/// What a run did so far.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// `scan`, `index`, `summarize` or `done`.
    pub phase: String,
    /// Transcripts found.
    pub found: usize,
    /// Transcripts still being written (left for later).
    pub active: usize,
    /// Transcripts of the current phase handled.
    pub done: usize,
    /// Transcripts the current phase handles.
    pub total: usize,
    /// Sessions written (new or grown).
    pub indexed: usize,
    /// Sessions unchanged since their last indexing.
    pub unchanged: usize,
    /// Transcripts without a single turn.
    pub empty: usize,
    /// Turns written.
    pub turns: usize,
    /// Summaries made.
    pub summarized: usize,
    /// Facts written.
    pub facts: usize,
    /// Sessions still waiting for a summary (budget, failures, a stopped run).
    pub summaries_pending: usize,
    /// Whether the monthly budget stopped the summaries.
    pub budget_reached: bool,
    /// Session files skipped because this version of them failed [`INDEX_ATTEMPTS`] times.
    pub failed: usize,
    /// Sessions linked to the memories they wrote (`wrote` edges; also an unchanged session
    /// linked once by the backfill).
    pub linked: usize,
    /// What failed, one line each.
    pub problems: Vec<String>,
}

impl Report {
    /// The report as JSON (the daemon's `sessions_status`).
    pub fn to_json(&self) -> Value {
        json!({
            "phase": self.phase, "found": self.found, "active": self.active,
            "done": self.done, "total": self.total, "indexed": self.indexed,
            "unchanged": self.unchanged, "empty": self.empty, "turns": self.turns,
            "summarized": self.summarized, "facts": self.facts,
            "summaries_pending": self.summaries_pending, "budget_reached": self.budget_reached,
            "failed": self.failed, "linked": self.linked, "problems": self.problems,
        })
    }
}

/// One session to index: a transcript file, or an opencode session (`<db>#<id>`), with its size
/// and modification time (seconds), which say whether it changed since its last indexing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// The transcript file, or `<opencode.db>#<session id>`.
    pub path: PathBuf,
    /// Its size (opencode: the bytes of its stored messages and parts).
    pub size: u64,
    /// Its modification time, seconds since the epoch.
    pub modified: u64,
}

/// The sessions of `request`'s harness. Claude Code: the transcripts ([`scan`]); a file that
/// cannot be read is skipped. opencode: the top-level sessions of its database (none when it
/// has no database yet; `Err` when the database cannot be read).
/// The sessions of one harness as the indexing reads them (`crate::harnesses::sessions`):
/// without a setup, from the request's directory and the data directory only (the daemon
/// indexes).
pub trait SessionSource: Sync {
    /// Its session units under `dir` (files, or `<db>#<id>` for opencode).
    fn units(&self, dir: &Path) -> Result<Vec<Unit>, String>;
    /// A unit's own id (the graph's `session:<id>`): its file name without extension unless
    /// the harness says otherwise.
    fn unit_id(&self, file: &Path) -> String {
        file.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    }
    /// The transcript of a unit and its source label (`transcript:<file name>`, ...).
    fn read(&self, file: &Path, data_dir: &Path) -> Result<(Transcript, String), String>;
    /// The units under `sessions` of the work directory `dir` written since `since`
    /// (seconds), which the end of its MCP connection marks ended (a harness without a
    /// session end hook: CodeWhale).
    fn written_since(&self, _sessions: &Path, _dir: &Path, _since: u64) -> Vec<PathBuf> {
        Vec::new()
    }
    /// Where its sessions are, read from the environment, when the end of its `use`
    /// connection ends them (CodeWhale); `None` otherwise.
    fn ended_by_connection(&self, _env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
        None
    }
}

/// `<prefix>:<file name>`, a session's source label.
pub fn source_label(prefix: &str, file: &Path) -> String {
    format!(
        "{prefix}:{}",
        file.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    )
}

pub fn units(request: &Request) -> Result<Vec<Unit>, String> {
    crate::harnesses::sessions(request.harness).units(&request.projects_dir)
}

/// The transcripts under `projects_dir`: `<dir>/<session>.jsonl` (the sub-agents' transcripts
/// in `<dir>/<session>/subagents/` are not taken), sorted.
pub fn scan(projects_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(dirs) = std::fs::read_dir(projects_dir) else {
        return out;
    };
    for dir in dirs.filter_map(Result::ok).map(|e| e.path()) {
        if !dir.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        out.extend(
            files
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|ext| ext == "jsonl")),
        );
    }
    out.sort();
    out
}

/// The indexing state of every session of one harness (`<data dir>/sessions/<harness>.json`:
/// `claude-code.json`, `opencode.json`).
#[derive(Debug, Clone, Default)]
pub struct State {
    harness: Kind,
    files: BTreeMap<String, Value>,
}

impl State {
    #[cfg(test)]
    fn path(data_dir: &Path) -> PathBuf {
        Self::path_for(data_dir, Kind::ClaudeCode)
    }

    fn path_for(data_dir: &Path, harness: Kind) -> PathBuf {
        data_dir
            .join("sessions")
            .join(format!("{}.json", harness.id()))
    }

    /// Reads the state of `harness` in `data_dir` (empty when there is none).
    pub fn load_for(data_dir: &Path, harness: Kind) -> Result<Self, String> {
        let path = Self::path_for(data_dir, harness);
        let Some(bytes) = fsutil::read_optional(&path)? else {
            return Ok(Self {
                harness,
                files: BTreeMap::new(),
            });
        };
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|err| format!("{} is invalid: {err}", path.display()))?;
        if value["version"].as_u64() != Some(FORMAT) {
            return Err(format!("{}: unsupported version", path.display()));
        }
        Ok(Self {
            harness,
            files: value["files"]
                .as_object()
                .map(|files| files.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default(),
        })
    }

    /// Writes the state.
    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        let path = Self::path_for(data_dir, self.harness);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
        }
        let files: serde_json::Map<String, Value> = self.files.clone().into_iter().collect();
        let value = json!({"version": FORMAT, "files": files});
        fsutil::write_atomic(
            &path,
            format!(
                "{}\n",
                crate::install::jsonedit::render(&value, Some("  "), 0)
            )
            .as_bytes(),
        )
    }

    /// The entry of `file`.
    pub fn entry(&self, file: &Path) -> Option<&Value> {
        self.files.get(&file.display().to_string())
    }

    fn entry_mut(&mut self, file: &Path) -> &mut Value {
        self.files
            .entry(file.display().to_string())
            .or_insert_with(|| json!({}))
    }

    /// How many transcripts the state lists as indexed (with turns).
    pub fn indexed(&self) -> usize {
        self.files
            .values()
            .filter(|e| e["fingerprint"].is_string() && e["empty"] != true)
            .count()
    }

    /// How many session files are skipped because their current version failed to index
    /// (as recorded; a changed file is tried again).
    pub fn failed(&self) -> usize {
        self.files
            .values()
            .filter(|e| e["failed"]["attempts"].as_u64().unwrap_or(0) >= INDEX_ATTEMPTS)
            .count()
    }

    /// How many indexed sessions have a summary of their current version.
    pub fn summarized(&self) -> usize {
        self.files
            .values()
            .filter(|e| e["fingerprint"].is_string() && e["summarized"] == e["fingerprint"])
            .count()
    }
}

/// A file's size and modification time (seconds since the epoch).
pub(crate) fn stamp(file: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(file).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Some((meta.len(), modified))
}

/// Whether `file` (modified at `modified`) is finished for `request`.
fn finished(request: &Request, file: &Path, modified: u64, now: SystemTime) -> bool {
    if request.active.contains(file) {
        return false;
    }
    if request.ended.contains(file) {
        return true;
    }
    let now = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    now.saturating_sub(modified) >= request.idle.as_secs()
}

/// The project key of a session: its working directory's (git root), else the harness project
/// directory's decoded name; `None` when neither names a directory.
pub fn project_of(transcript: &Transcript, file: &Path) -> Option<String> {
    if let Some(cwd) = &transcript.cwd {
        let dir = Path::new(cwd);
        return Some(if dir.is_dir() {
            crate::runtime::mcp::project_key(None, dir)
        } else {
            // The directory is gone: keep its path, the key it had.
            cwd.clone()
        });
    }
    let dir = crate::memory::import::resolve_project_dir(file.parent()?)?;
    Some(crate::runtime::mcp::project_key(None, &dir))
}

/// The session's id in the graph: `session:<transcript file name>` (Codex: `session:<thread
/// id>`; opencode:
/// `session:<session id>`).
pub fn session_id(harness: Kind, file: &Path) -> String {
    format!(
        "session:{}",
        crate::harnesses::sessions(harness).unit_id(file)
    )
}

/// One parsed transcript with what the indexing derives from it.
struct Parsed {
    harness: Kind,
    transcript: Transcript,
    id: String,
    uuid: String,
    /// The project's directory (its path; edited files are relative to it).
    project: String,
    /// The project's stable id (`crate::memory::projects`): what the graph records and routes by.
    key: String,
    files: Vec<String>,
    title: String,
    started_at: String,
    source: String,
}

/// Parses a transcript (a Cursor transcript is one `data_dir`'s registry lists); with `resolve`
/// its project's directory is resolved to the project's stable id (`crate::memory::projects::resolve`),
/// else the id is the directory.
fn parse_file(
    harness: Kind,
    file: &Path,
    modified: u64,
    data_dir: &Path,
    resolve: bool,
) -> Result<Option<Parsed>, String> {
    let reader = crate::harnesses::sessions(harness);
    let uuid = reader.unit_id(file);
    let (transcript, source) = reader.read(file, data_dir)?;
    if transcript.turns.is_empty() {
        return Ok(None);
    }
    let project = project_of(&transcript, file).ok_or_else(|| {
        format!(
            "{}: the session's project directory is not known (no working directory recorded)",
            file.display()
        )
    })?;
    let files = transcript
        .files
        .iter()
        .filter_map(|path| transcript::relative_file(path, &project))
        .collect();
    let key = if resolve {
        crate::memory::projects::resolve(data_dir, &project)?.id
    } else {
        project.clone()
    };
    let title = if transcript.title.is_empty() {
        format!("Session {}", uuid.chars().take(8).collect::<String>())
    } else {
        transcript.title.clone()
    };
    let started_at = if transcript.started_at.is_empty() {
        crate::runtime::daemon::utc_timestamp(modified)
    } else {
        transcript.started_at.clone()
    };
    Ok(Some(Parsed {
        harness,
        id: session_id(harness, file),
        source,
        transcript,
        uuid,
        project,
        key,
        files,
        title,
        started_at,
    }))
}

impl Parsed {
    fn session_properties(&self) -> Value {
        let t = &self.transcript;
        let summary = format!("Session '{}' ({} turns).", self.title, t.turns.len());
        let mut p = json!({
            "name": self.title, "title": self.title, "summary": summary,
            "embed_text": format!("{}: {summary}", self.title),
            "turns": t.turns.len(), "started_at": self.started_at,
            "ended_at": if t.ended_at.is_empty() { &self.started_at } else { &t.ended_at },
            "branches": t.branches, "files_edited": self.files, "source": self.source,
            "source_bytes": t.bytes, "visibility": "private", "origin": "sessions",
            "harness": self.harness.id(), "project": self.key,
        });
        if !t.compact.is_empty() {
            p["compact_summaries"] = json!(clip(&t.compact.join("\n\n---\n\n"), CAP));
        }
        p
    }

    fn turn_rows(&self) -> Vec<(usize, Value)> {
        self.transcript
            .turns
            .iter()
            .map(|turn| {
                let name = format!("{} #{}", self.title, turn.n);
                let user = clip(&turn.user, CAP);
                let assistant = clip(&turn.assistant, CAP);
                let size = user.len() + assistant.len();
                let row = json!({
                    "id": format!("turn:{}:{}", self.uuid, turn.n),
                    "p": {
                        "name": name, "n": turn.n,
                        "at": if turn.at.is_empty() { &self.started_at } else { &turn.at },
                        "user": user, "assistant": assistant, "tools": turn.tools,
                        "summary": clip(&turn.user, 300),
                        "embed_text": format!("{name}: {}", clip(&turn.user, 500)),
                        "source": self.source, "visibility": "private", "origin": "sessions",
                        "project": self.key,
                    },
                });
                (size, row)
            })
            .collect()
    }

    fn fact_rows(&self, output: &Value) -> Vec<Value> {
        summaries::facts(output)
            .into_iter()
            .map(|(kind, i, text, why)| {
                let mut embed = format!("{kind}: {}", clip(&text, 600));
                if !why.is_empty() {
                    embed.push_str(&format!(" Why: {}", clip(&why, 400)));
                }
                let mut p = json!({
                    "name": clip(&text, 80), "summary": clip(&text, 1500), "fact_kind": kind,
                    "n": i, "at": self.started_at, "source": self.source,
                    "visibility": "private", "origin": "sessions", "project": self.key,
                    "embed_text": embed,
                });
                if !why.is_empty() {
                    p["why"] = json!(clip(&why, 1500));
                }
                json!({"id": format!("fact:{}:{kind}:{i}", self.uuid), "p": p})
            })
            .collect()
    }
}

/// Writes one session: its properties, then its turns in batches (the last one sets the
/// fingerprint). Returns the turns written.
fn write_session(writer: &mut dyn Writer, parsed: &Parsed) -> Result<usize, String> {
    writer.call(
        &parsed.key,
        "index_session",
        json!({"id": parsed.id, "p": parsed.session_properties()}),
    )?;
    let rows = parsed.turn_rows();
    let total = rows.len();
    let mut batch = Vec::new();
    let mut size = 0;
    for (i, (row_size, row)) in rows.into_iter().enumerate() {
        batch.push(row);
        size += row_size;
        let last = i + 1 == total;
        if last || batch.len() >= BATCH_TURNS || size >= BATCH_CHARS {
            let fingerprint = if last {
                parsed.transcript.fingerprint.clone()
            } else {
                String::new()
            };
            writer.call(
                &parsed.key,
                "index_turns",
                json!({"id": parsed.id, "rows": std::mem::take(&mut batch),
                       "fingerprint": fingerprint}),
            )?;
            size = 0;
        }
    }
    Ok(total)
}

/// The memories a session wrote, one row per slug in first-write order: `{slug, actions (the
/// tool calls in first-use order), at, last_at, count}` (the `links` of `link_memories`).
pub fn memory_links(writes: &[transcript::MemoryWrite]) -> Vec<Value> {
    let mut order: Vec<&str> = Vec::new();
    let mut rows: BTreeMap<&str, (Vec<&str>, &str, &str, u64)> = BTreeMap::new();
    for write in writes {
        let row = rows.entry(write.slug.as_str()).or_insert_with(|| {
            order.push(write.slug.as_str());
            (Vec::new(), write.at.as_str(), write.at.as_str(), 0)
        });
        if !row.0.contains(&write.action.as_str()) {
            row.0.push(write.action.as_str());
        }
        if row.1.is_empty() {
            row.1 = write.at.as_str();
        }
        if !write.at.is_empty() {
            row.2 = write.at.as_str();
        }
        row.3 += 1;
    }
    order
        .into_iter()
        .filter_map(|slug| {
            rows.get(slug).map(|(actions, at, last, count)| {
                json!({"slug": slug, "actions": actions, "at": at, "last_at": last,
                       "count": count})
            })
        })
        .collect()
}

/// Links the session to the memories it wrote (replacing its earlier links); `false` without
/// a memory write (nothing to link: a transcript only grows, so a session without writes never
/// had links).
fn link_session(writer: &mut dyn Writer, parsed: &Parsed) -> Result<bool, String> {
    if parsed.transcript.memory_writes.is_empty() {
        return Ok(false);
    }
    let links = memory_links(&parsed.transcript.memory_writes);
    let answer = writer.call(
        &parsed.key,
        "link_memories",
        json!({"id": parsed.id, "links": links}),
    )?;
    match answer["error"].as_str() {
        Some(err) => Err(format!("{}: {err}", parsed.id)),
        None => Ok(true),
    }
}

/// The one-time link of a session indexed before the links existed (its state entry has no
/// current `memory_links`): read its transcript again, link, record the version.
fn backfill_links(
    harness: Kind,
    file: &Path,
    modified: u64,
    data_dir: &Path,
    writer: &mut dyn Writer,
    state: &mut State,
) -> Result<bool, String> {
    let linked = match parse_file(harness, file, modified, data_dir, true)? {
        Some(parsed) => link_session(writer, &parsed)?,
        None => false,
    };
    let slot = state.entry_mut(file);
    if slot.is_object() {
        slot["memory_links"] = json!(LINKS_VERSION);
    }
    Ok(linked)
}

/// What the graph knows of each session of a project: id -> (fingerprint, summarized).
type Known = BTreeMap<String, (String, String)>;

fn known_of(
    writer: &mut dyn Writer,
    cache: &mut BTreeMap<String, Known>,
    project: &str,
) -> Result<Known, String> {
    if let Some(known) = cache.get(project) {
        return Ok(known.clone());
    }
    let rows = writer.call(project, "indexed_sessions", json!({}))?;
    let mut known = Known::new();
    for row in rows.as_array().into_iter().flatten() {
        if let Some(id) = row["id"].as_str() {
            known.insert(
                id.to_string(),
                (
                    row["fingerprint"].as_str().unwrap_or_default().to_string(),
                    row["summarized"].as_str().unwrap_or_default().to_string(),
                ),
            );
        }
    }
    cache.insert(project.to_string(), known.clone());
    Ok(known)
}

/// Runs one indexing pass: every finished transcript that is new or changed is written, then,
/// with a summarizer (the user opted in), the sessions without a summary of their current
/// version are summarized within the budget. `progress` sees the report after every unit;
/// `stop` ends the run between units.
pub fn run(
    request: &Request,
    data_dir: &Path,
    writer: &mut dyn Writer,
    summarizer: Option<&mut Summarizer>,
    progress: &mut dyn FnMut(&Report),
    stop: &dyn Fn() -> bool,
) -> Report {
    let mut report = Report {
        phase: "scan".into(),
        ..Report::default()
    };
    let loaded = State::load_for(data_dir, request.harness)
        .and_then(|state| units(request).map(|units| (state, units)));
    let (mut state, units) = match loaded {
        Ok(both) => both,
        Err(err) => {
            report.problems.push(err);
            report.phase = "done".into();
            return report;
        }
    };
    let now = SystemTime::now();
    let mut todo = Vec::new();
    for Unit {
        path: file,
        size,
        modified,
    } in units
    {
        report.found += 1;
        if !finished(request, &file, modified, now) {
            report.active += 1;
            continue;
        }
        todo.push((file, size, modified));
    }
    report.phase = "index".into();
    report.total = todo.len();
    progress(&report);
    let mut cache: BTreeMap<String, Known> = BTreeMap::new();
    let mut to_summarize = Vec::new();
    for (file, size, modified) in todo {
        if stop() {
            break;
        }
        report.done += 1;
        let entry = state.entry(&file).cloned().unwrap_or(Value::Null);
        let same_file =
            entry["size"].as_u64() == Some(size) && entry["mtime"].as_u64() == Some(modified);
        if same_file && entry["empty"] == true {
            report.empty += 1;
            continue;
        }
        if same_file && entry["fingerprint"].is_string() {
            report.unchanged += 1;
            if entry["memory_links"].as_u64() != Some(LINKS_VERSION) {
                let linked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _doing =
                        crate::runtime::daemon::doing(format!("linking {}", file.display()));
                    backfill_links(
                        request.harness,
                        &file,
                        modified,
                        data_dir,
                        writer,
                        &mut state,
                    )
                }))
                .unwrap_or_else(|_| Err("linking it panicked (a bug, logged)".to_string()));
                match linked {
                    Ok(true) => report.linked += 1,
                    Ok(false) => {}
                    Err(err) => report.problems.push(format!(
                        "{}: the memories it wrote are not linked: {err}",
                        file.display()
                    )),
                }
                if let Err(err) = state.save(data_dir) {
                    report.problems.push(err);
                }
            }
            if entry["summarized"] != entry["fingerprint"] {
                to_summarize.push(file);
            }
            continue;
        }
        let failures = failures_of(&entry, size, modified);
        if failures >= INDEX_ATTEMPTS {
            report.failed += 1;
            continue;
        }
        // A panic while indexing one file (a bug) is caught here: the file is recorded as
        // failed and the run goes on with the next one.
        let indexed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _doing = crate::runtime::daemon::doing(format!("indexing {}", file.display()));
            crate::runtime::daemon::injected_panic("index", &file.display().to_string());
            index_one(
                request.harness,
                &file,
                size,
                modified,
                data_dir,
                writer,
                &mut cache,
                &mut state,
            )
        }))
        .unwrap_or_else(|_| Err("indexing it panicked (a bug, logged)".to_string()));
        match indexed {
            Ok(Indexed::Empty) => report.empty += 1,
            Ok(Indexed::Unchanged { summarized, linked }) => {
                report.unchanged += 1;
                report.linked += usize::from(linked);
                if !summarized {
                    to_summarize.push(file.clone());
                }
            }
            Ok(Indexed::Written { turns, linked }) => {
                report.indexed += 1;
                report.linked += usize::from(linked);
                report.turns += turns;
                to_summarize.push(file.clone());
            }
            Err(err) => {
                record_failure(&mut state, &file, size, modified, failures + 1);
                let shown = file.display().to_string();
                let err = if err.starts_with(&shown) {
                    err
                } else {
                    format!("{shown}: {err}")
                };
                report.problems.push(format!(
                    "{err} (attempt {} of {INDEX_ATTEMPTS})",
                    failures + 1
                ));
            }
        }
        if let Err(err) = state.save(data_dir) {
            report.problems.push(err);
        }
        progress(&report);
    }
    report.summaries_pending = to_summarize.len();
    if let Some(summarizer) = summarizer {
        report.phase = "summarize".into();
        report.done = 0;
        report.total = to_summarize.len();
        progress(&report);
        for file in to_summarize {
            if stop() {
                break;
            }
            report.done += 1;
            let summarized = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _doing =
                    crate::runtime::daemon::doing(format!("summarizing {}", file.display()));
                summarize_one(
                    request.harness,
                    &file,
                    data_dir,
                    writer,
                    summarizer,
                    &mut state,
                )
            }))
            .unwrap_or_else(|_| {
                Err(Refused::Failed(format!(
                    "{}: summarizing it panicked (a bug, logged)",
                    file.display()
                )))
            });
            match summarized {
                Ok(Summarized::Done { facts }) => {
                    report.summarized += 1;
                    report.facts += facts;
                    report.summaries_pending -= 1;
                }
                Ok(Summarized::Skipped) => report.summaries_pending -= 1,
                Ok(Summarized::Waiting) => {}
                Err(Refused::Budget { spent, cap }) => {
                    report.budget_reached = true;
                    report.problems.push(format!(
                        "the summaries' monthly budget is reached (${spent:.2} of ${cap:.2} \
                         spent); raw turns are still indexed, summaries continue next month"
                    ));
                    break;
                }
                Err(Refused::Failed(err)) => report.problems.push(err),
            }
            if let Err(err) = state.save(data_dir) {
                report.problems.push(err);
            }
            progress(&report);
        }
    }
    report.phase = "done".into();
    progress(&report);
    report
}

/// How often the current version (`size`, `modified`) of a session file failed under this
/// binary's version.
fn failures_of(entry: &Value, size: u64, modified: u64) -> u64 {
    let failed = &entry["failed"];
    if failed["size"].as_u64() == Some(size)
        && failed["mtime"].as_u64() == Some(modified)
        && failed["version"].as_str() == Some(crate::runtime::protocol::VERSION)
    {
        failed["attempts"].as_u64().unwrap_or(0)
    } else {
        0
    }
}

/// Records a failed attempt at a session file's current version (the entry's other fields,
/// from an earlier version that was indexed, stay; a later success replaces the entry).
fn record_failure(state: &mut State, file: &Path, size: u64, modified: u64, attempts: u64) {
    let slot = state.entry_mut(file);
    if !slot.is_object() {
        *slot = json!({});
    }
    slot["failed"] = json!({
        "size": size, "mtime": modified, "version": crate::runtime::protocol::VERSION,
        "attempts": attempts, "at": crate::runtime::daemon::now_utc(),
    });
}

enum Indexed {
    Empty,
    Unchanged { summarized: bool, linked: bool },
    Written { turns: usize, linked: bool },
}

#[allow(clippy::too_many_arguments)]
fn index_one(
    harness: Kind,
    file: &Path,
    size: u64,
    modified: u64,
    data_dir: &Path,
    writer: &mut dyn Writer,
    cache: &mut BTreeMap<String, Known>,
    state: &mut State,
) -> Result<Indexed, String> {
    let Some(parsed) = parse_file(harness, file, modified, data_dir, true)? else {
        *state.entry_mut(file) = json!({"size": size, "mtime": modified, "empty": true});
        return Ok(Indexed::Empty);
    };
    let known = known_of(writer, cache, &parsed.key)?;
    let fingerprint = parsed.transcript.fingerprint.clone();
    let (stored, summarized) = known.get(&parsed.id).cloned().unwrap_or_default();
    let written = if stored == fingerprint {
        None
    } else {
        Some(write_session(writer, &parsed)?)
    };
    let linked = link_session(writer, &parsed)?;
    let outcome = match written {
        None => Indexed::Unchanged {
            summarized: summarized == fingerprint,
            linked,
        },
        Some(turns) => Indexed::Written { turns, linked },
    };
    let previous = state.entry(file).cloned().unwrap_or(Value::Null);
    let summary_of = if summarized == fingerprint {
        json!(fingerprint)
    } else {
        previous["summarized"].clone()
    };
    *state.entry_mut(file) = json!({
        "size": size, "mtime": modified, "fingerprint": fingerprint,
        "session": parsed.id, "project": parsed.key,
        "turns": parsed.transcript.turns.len(), "started_at": parsed.started_at,
        "prompt_chars": summaries::prompt(&parsed.transcript, &parsed.files, &parsed.project)
            .chars().count(),
        "summarized": summary_of,
        "indexed_at": crate::runtime::daemon::now_utc(),
        "memory_links": LINKS_VERSION,
    });
    Ok(outcome)
}

enum Summarized {
    Done {
        facts: usize,
    },
    /// Not worth a summary (too short, before `since`).
    Skipped,
    /// Tried too often for this version; waits for the session to change.
    Waiting,
}

fn summarize_one(
    harness: Kind,
    file: &Path,
    data_dir: &Path,
    writer: &mut dyn Writer,
    summarizer: &mut Summarizer,
    state: &mut State,
) -> Result<Summarized, Refused> {
    let entry = state.entry(file).cloned().unwrap_or(Value::Null);
    let Some(fingerprint) = entry["fingerprint"].as_str().map(str::to_string) else {
        return Ok(Summarized::Waiting);
    };
    if entry["summarized"] == json!(fingerprint) {
        return Ok(Summarized::Skipped);
    }
    if let Some(since) = summarizer.since()
        && entry["started_at"].as_str().unwrap_or_default() < since
    {
        return Ok(Summarized::Skipped);
    }
    let attempts = if entry["attempted"] == json!(fingerprint) {
        entry["attempts"].as_u64().unwrap_or(0)
    } else {
        0
    };
    if attempts >= SUMMARY_ATTEMPTS {
        return Ok(Summarized::Waiting);
    }
    let modified = match crate::harnesses::opencode::sessions::split_unit(file) {
        Some(_) => entry["mtime"].as_u64().unwrap_or(0),
        None => match stamp(file) {
            Some((_, modified)) => modified,
            None => return Ok(Summarized::Waiting),
        },
    };
    let parsed = match parse_file(harness, file, modified, data_dir, true) {
        Ok(Some(parsed)) if parsed.transcript.fingerprint == fingerprint => parsed,
        // Changed since its indexing: the next run indexes it again first.
        Ok(_) => return Ok(Summarized::Waiting),
        Err(err) => return Err(Refused::Failed(err)),
    };
    let condensed = summaries::condensed(&parsed.transcript, &parsed.files, usize::MAX);
    if condensed.chars().count() < summaries::MIN_CHARS {
        let slot = state.entry_mut(file);
        slot["summarized"] = json!(fingerprint);
        slot["summary"] = json!("too short");
        return Ok(Summarized::Skipped);
    }
    let prompt = summaries::prompt(&parsed.transcript, &parsed.files, &parsed.project);
    let summary = match summarizer.summarize(&prompt) {
        Ok(summary) => summary,
        Err(Refused::Failed(err)) => {
            let slot = state.entry_mut(file);
            slot["attempted"] = json!(fingerprint);
            slot["attempts"] = json!(attempts + 1);
            return Err(Refused::Failed(format!(
                "{}: the summary failed: {err}",
                file.display()
            )));
        }
        Err(other) => return Err(other),
    };
    let output = &summary.output;
    let headline = clip(output["headline"].as_str().unwrap_or_default().trim(), 200);
    let text = clip(output["summary"].as_str().unwrap_or_default().trim(), 600);
    let facts = parsed.fact_rows(output);
    let count = facts.len();
    let p = json!({
        "name": headline, "summary": text, "body": summaries::render(output),
        "embed_text": format!("{headline}: {text}"), "summarized": fingerprint,
        "summarized_bytes": parsed.transcript.bytes,
    });
    writer
        .call(
            &parsed.key,
            "set_summary",
            json!({"id": parsed.id, "p": p, "facts": facts}),
        )
        .map_err(Refused::Failed)?;
    let slot = state.entry_mut(file);
    slot["summarized"] = json!(fingerprint);
    slot["summary"] = json!("done");
    if let Some(cost) = summary.cost_usd {
        slot["summary_cost_usd"] = json!(cost);
    }
    Ok(Summarized::Done { facts: count })
}

/// What a summaries run would cost: sessions finished and indexed or waiting, how many lack a
/// summary, and the estimate of summarizing them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Estimate {
    /// Finished sessions with turns.
    pub sessions: usize,
    /// Of them, without a summary of their current version (and not before `since`).
    pub pending: usize,
    /// Of them, too short for a summary.
    pub too_short: usize,
    /// The estimated list-price cost of the pending ones, USD.
    pub usd: f64,
}

/// Estimates the summaries of every finished transcript under `projects_dir` (read only: the
/// state of `data_dir` and the transcripts; nothing is written, no model is called).
#[cfg(all(test, unix))]
pub fn estimate(
    projects_dir: &Path,
    data_dir: &Path,
    idle: Duration,
    since: Option<&str>,
) -> Result<Estimate, String> {
    estimate_for(Kind::default(), projects_dir, data_dir, idle, since)
}

/// [`estimate`] for the sessions of `harness` kept in `dir`.
pub fn estimate_for(
    harness: Kind,
    dir: &Path,
    data_dir: &Path,
    idle: Duration,
    since: Option<&str>,
) -> Result<Estimate, String> {
    let state = State::load_for(data_dir, harness)?;
    let request = Request {
        harness,
        projects_dir: dir.to_path_buf(),
        idle,
        ..Request::default()
    };
    let now = SystemTime::now();
    let mut out = Estimate::default();
    for Unit {
        path: file,
        size,
        modified,
    } in units(&request)?
    {
        if !finished(&request, &file, modified, now) {
            continue;
        }
        let entry = state.entry(&file).cloned().unwrap_or(Value::Null);
        let same =
            entry["size"].as_u64() == Some(size) && entry["mtime"].as_u64() == Some(modified);
        let (chars, started, summarized) = if same && entry["fingerprint"].is_string() {
            (
                entry["prompt_chars"].as_u64().unwrap_or(0) as usize,
                entry["started_at"].as_str().unwrap_or_default().to_string(),
                entry["summarized"] == entry["fingerprint"],
            )
        } else if same && entry["empty"] == true {
            continue;
        } else {
            match parse_file(harness, &file, modified, data_dir, false) {
                Ok(Some(parsed)) => (
                    summaries::prompt(&parsed.transcript, &parsed.files, &parsed.project)
                        .chars()
                        .count(),
                    parsed.started_at.clone(),
                    false,
                ),
                Ok(None) | Err(_) => continue,
            }
        };
        out.sessions += 1;
        if summarized || since.is_some_and(|since| started.as_str() < since) {
            continue;
        }
        // The prompt is the instructions (~1.6k characters) plus the condensed transcript.
        if chars < summaries::MIN_CHARS + 1_600 {
            out.too_short += 1;
            continue;
        }
        out.pending += 1;
        out.usd += summaries::estimate_usd(chars);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphersal_session::Session;

    /// The store in this process: a session with the catalog, the project ignored.
    pub(crate) struct Direct {
        pub session: Session,
        pub calls: Vec<String>,
    }

    impl Direct {
        pub fn new() -> Self {
            let mut session = Session::new();
            crate::runtime::catalog::apply(&mut session).unwrap();
            Self {
                session,
                calls: Vec::new(),
            }
        }

        pub fn query(&mut self, script: &str) -> Value {
            let answer: Value = serde_json::from_str(&self.session.execute(
                script,
                &graphersal_session::RunOptions {
                    no_limit: true,
                    ..Default::default()
                },
            ))
            .unwrap();
            assert_eq!(answer["ok"], true, "{script}: {answer}");
            answer["execution"]["results"].clone()
        }
    }

    impl Writer for Direct {
        fn call(&mut self, _project: &str, name: &str, args: Value) -> Result<Value, String> {
            self.calls.push(name.to_string());
            crate::runtime::catalog::call(&mut self.session, name, &args)
        }
    }

    /// A projects directory with one project (a git root) and the fixture transcript.
    fn setup(tmp: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let work = crate::runtime::mcp::canonical(tmp).join("work");
        std::fs::create_dir_all(work.join(".git")).unwrap();
        let projects = tmp.join("projects");
        let dir = projects.join(crate::memory::import::claude_slug(
            &work.display().to_string(),
        ));
        std::fs::create_dir_all(dir.join("abc").join("subagents")).unwrap();
        // The fixture is JSON: the directory goes in as a JSON string's content (a Windows
        // path's backslashes escaped).
        let escaped = serde_json::to_string(&work.display().to_string()).unwrap();
        let text = crate::harnesses::claude_code::transcript::tests::fixture()
            .replace("/w/app", &escaped[1..escaped.len() - 1]);
        let file = dir.join("abc.jsonl");
        std::fs::write(&file, text).unwrap();
        // A sub-agent transcript and a stray file are never indexed.
        std::fs::write(
            dir.join("abc").join("subagents").join("agent-1.jsonl"),
            "{\"type\":\"user\",\"message\":{\"content\":\"x\"}}\n",
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        (projects, work, file)
    }

    fn request(projects: &Path) -> Request {
        Request {
            projects_dir: projects.to_path_buf(),
            idle: Duration::ZERO,
            ..Request::default()
        }
    }

    fn index(request: &Request, data: &Path, writer: &mut Direct) -> Report {
        run(request, data, writer, None, &mut |_| {}, &|| false)
    }

    #[test]
    fn sessions_are_indexed_once_and_again_when_they_grow() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("sess");
        let data = crate::runtime::mcp_tests::TempDir::new("sess-data");
        let (projects, work, file) = setup(tmp.path());
        let mut store = Direct::new();
        let first = index(&request(&projects), data.path(), &mut store);
        assert_eq!(first.problems, Vec::<String>::new());
        assert_eq!((first.found, first.indexed, first.turns), (1, 1, 3));
        let project = work.display().to_string();
        let session = store.query(
            "g.v(\"session:abc\").project(\"name\", \"project\", \"turns\", \"fingerprint\", \"files\", \"harness\")\
             .by(\"name\").by(\"project\").by(\"turns\").by(\"fingerprint\").by(\"files_edited\").by(\"harness\").next()",
        );
        assert_eq!(session[0]["name"], "Parser cache");
        assert_eq!(session[0]["project"], project);
        assert_eq!(session[0]["turns"], 3);
        assert_eq!(session[0]["files"], json!(["src/parse.rs"]));
        assert_eq!(session[0]["harness"], "claude-code");
        let turns = store.query("g.v(\"session:abc\").out(\"contains\").has_label(\"turn\").has(\"project\").count().next()");
        assert_eq!(turns, json!([3]));
        let turn = store.query("g.v(\"turn:abc:0\").values(\"tools\").next()");
        assert_eq!(turn, json!(["Edit", "Bash"]));

        // Unchanged: nothing is read or written again.
        store.calls.clear();
        let second = index(&request(&projects), data.path(), &mut store);
        assert_eq!((second.indexed, second.unchanged), (0, 1));
        assert!(store.calls.is_empty(), "{:?}", store.calls);

        // The state is lost: the graph's fingerprint still prevents a rewrite.
        std::fs::remove_file(State::path(data.path())).unwrap();
        store.calls.clear();
        let third = index(&request(&projects), data.path(), &mut store);
        assert_eq!((third.indexed, third.unchanged), (0, 1));
        assert_eq!(store.calls, vec!["indexed_sessions"]);

        // A resumed session grows: indexed again, with the new turn.
        let mut text = std::fs::read_to_string(&file).unwrap();
        text.push_str(&format!(
            "{}\n",
            json!({"type": "user", "timestamp": "2026-01-03T09:00:00.000Z",
                   "message": {"content": "One more thing."}})
        ));
        std::fs::write(&file, text).unwrap();
        let grown = index(&request(&projects), data.path(), &mut store);
        assert_eq!((grown.indexed, grown.turns), (1, 4), "{grown:?}");
        assert_eq!(
            store.query("g.v(\"session:abc\").out(\"contains\").count().next()"),
            json!([4])
        );
        assert_eq!(
            store.query("g.v(\"session:abc\").values(\"turns\").next()"),
            json!([4])
        );
    }

    /// Takes `memory_links` out of every entry of the state file (a state written before the
    /// links existed).
    fn forget_links_in_state(data: &Path) {
        fn strip(value: &mut Value) {
            match value {
                Value::Object(map) => {
                    map.remove("memory_links");
                    map.values_mut().for_each(strip);
                }
                Value::Array(items) => items.iter_mut().for_each(strip),
                _ => {}
            }
        }
        let path = State::path(data);
        let mut state: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        strip(&mut state);
        std::fs::write(&path, state.to_string()).unwrap();
    }

    #[test]
    fn a_session_is_linked_to_the_memories_it_wrote_and_old_sessions_are_backfilled_once() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("sess-links");
        let data = crate::runtime::mcp_tests::TempDir::new("sess-links-data");
        let (projects, work, file) = setup(tmp.path());
        let project = work.display().to_string();
        let mut text = std::fs::read_to_string(&file).unwrap();
        text.push_str(&format!(
            "{}\n",
            json!({"type": "assistant", "sessionId": "s-1", "timestamp": "2026-01-02T11:00:09.000Z",
                   "message": {"content": [
                       {"type": "tool_use", "name": "mcp__graphersal-memory__remember",
                        "input": {"slug": "parser-cache", "summary": "LRU"}},
                       {"type": "tool_use", "name": "mcp__graphersal-memory__pin",
                        "input": {"slug": "parser-cache"}},
                       {"type": "tool_use", "name": "mcp__graphersal-memory__remember",
                        "input": {"slug": "g-rule", "scope": "global"}},
                       {"type": "tool_use", "name": "mcp__graphersal-memory__remember",
                        "input": {"slug": "never-written"}}]}})
        ));
        std::fs::write(&file, text).unwrap();
        let mut store = Direct::new();
        for (slug, scope) in [("parser-cache", "project"), ("g-rule", "global")] {
            crate::runtime::catalog::call(
                &mut store.session,
                "remember",
                &json!({"slug": slug, "kind": "project", "summary": slug, "body": slug,
                        "scope": scope, "project": project}),
            )
            .unwrap();
        }
        let wrote = |store: &mut Direct| {
            store.query(
                "g.v(\"session:abc\").out_e(\"wrote\").order().by(__.in_v().id())\
                 .project(\"to\", \"actions\", \"count\", \"at\").by(__.in_v().id())\
                 .by(\"actions\").by(\"count\").by(\"at\").to_list()",
            )
        };
        let first = index(&request(&projects), data.path(), &mut store);
        assert_eq!(first.problems, Vec::<String>::new());
        assert_eq!((first.indexed, first.linked), (1, 1));
        let edges = wrote(&mut store).as_array().unwrap().clone();
        assert_eq!(edges.len(), 2, "the missing memory has no edge: {edges:?}");
        assert_eq!(edges[0]["to"], "memory:g-rule", "{edges:?}");
        assert_eq!(edges[1]["to"], format!("memory:parser-cache@{project}"));
        assert_eq!(edges[1]["actions"], json!(["remember", "pin"]));
        assert_eq!(edges[1]["count"], 2);
        assert_eq!(edges[1]["at"], "2026-01-02T11:00:09.000Z");

        // The UI's Graph tab: expanding the memory finds the session through its wrote edge.
        let around = crate::runtime::catalog::call(
            &mut store.session,
            "memory_neighbours",
            &json!({"slug": "parser-cache", "project": project}),
        )
        .unwrap();
        assert!(
            around["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n["id"] == "session:abc" && n["label"] == "session"),
            "{around}"
        );
        assert!(
            around["edges"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["label"] == "wrote"
                    && e["source"] == "session:abc"
                    && e["actions"] == json!(["remember", "pin"])),
            "{around}"
        );
        assert_eq!(around["imported"], "");

        // Unchanged and linked: nothing is read or written again.
        store.calls.clear();
        let second = index(&request(&projects), data.path(), &mut store);
        assert_eq!((second.unchanged, second.linked), (1, 0));
        assert!(store.calls.is_empty(), "{:?}", store.calls);

        // A state written before the links existed: one backfill pass, the same edges.
        forget_links_in_state(data.path());
        let backfill = index(&request(&projects), data.path(), &mut store);
        assert_eq!(
            (backfill.unchanged, backfill.linked, backfill.problems.len()),
            (1, 1, 0)
        );
        assert_eq!(store.calls, vec!["link_memories"]);
        assert_eq!(wrote(&mut store).as_array().unwrap().len(), 2);
        store.calls.clear();
        let after = index(&request(&projects), data.path(), &mut store);
        assert_eq!(after.linked, 0);
        assert!(
            store.calls.is_empty(),
            "the backfill runs once: {:?}",
            store.calls
        );

        // Sessions indexed before this version have no edges: the backfill adds them, and a
        // repeated one changes nothing.
        store.query("g.e().has_label(\"wrote\").drop().iterate()");
        for _ in 0..2 {
            forget_links_in_state(data.path());
            index(&request(&projects), data.path(), &mut store);
            assert_eq!(wrote(&mut store).as_array().unwrap(), &edges);
        }
    }

    #[test]
    fn running_sessions_wait_and_ended_ones_do_not() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("sess-act");
        let data = crate::runtime::mcp_tests::TempDir::new("sess-act-data");
        let (projects, _, file) = setup(tmp.path());
        let mut store = Direct::new();
        let mut waiting = request(&projects);
        waiting.idle = Duration::from_secs(3600);
        let report = index(&waiting, data.path(), &mut store);
        assert_eq!((report.active, report.indexed), (1, 0));
        let mut ended = waiting.clone();
        ended.ended.insert(file.clone());
        assert_eq!(index(&ended, data.path(), &mut store).indexed, 1);
        let mut active = request(&projects);
        active.active.insert(file.clone());
        std::fs::write(&file, "").unwrap();
        assert_eq!(index(&active, data.path(), &mut store).active, 1);
    }

    #[test]
    fn many_turns_are_written_in_small_batches() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("sess-batch");
        let data = crate::runtime::mcp_tests::TempDir::new("sess-batch-data");
        let (projects, _, file) = setup(tmp.path());
        let mut text = std::fs::read_to_string(&file).unwrap();
        for i in 0..90 {
            text.push_str(&format!(
                "{}\n",
                json!({"type": "user", "timestamp": "2026-01-03T09:00:00.000Z",
                       "message": {"content": format!("prompt {i}")}})
            ));
        }
        std::fs::write(&file, text).unwrap();
        let mut store = Direct::new();
        let report = index(&request(&projects), data.path(), &mut store);
        assert_eq!(report.turns, 93);
        let batches = store.calls.iter().filter(|c| *c == "index_turns").count();
        assert_eq!(batches, 3, "{:?}", store.calls);
    }

    #[test]
    fn empty_and_unreadable_transcripts_are_reported_not_fatal() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("sess-empty");
        let data = crate::runtime::mcp_tests::TempDir::new("sess-empty-data");
        let (projects, _, file) = setup(tmp.path());
        std::fs::write(&file, "{\"type\":\"mode\"}\nnot json\n{\"partial\": ").unwrap();
        let mut store = Direct::new();
        let report = index(&request(&projects), data.path(), &mut store);
        assert_eq!((report.empty, report.indexed), (1, 0));
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        // A transcript without a working directory in an undecodable project directory.
        let odd = projects.join("-nowhere-at-all");
        std::fs::create_dir_all(&odd).unwrap();
        std::fs::write(
            odd.join("x.jsonl"),
            "{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n",
        )
        .unwrap();
        let report = index(&request(&projects), data.path(), &mut store);
        assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
        assert!(report.problems[0].contains("project directory is not known"));
    }

    /// Text that broke the engine (graphersal-session 0.6.0 cut a script inside a multi-byte
    /// character before its last `(`): Cyrillic, an emoji and a no-break space each right
    /// before an unbalanced parenthesis inside a string.
    const AWKWARD: &str = "Готово: релиз проверен 🎉, see (draft — е(1 and \u{a0}(x 🎉(";

    /// [`AWKWARD`] as the content of a JSON string (no quotes).
    fn awkward_json() -> String {
        let quoted = serde_json::to_string(AWKWARD).unwrap();
        quoted[1..quoted.len() - 1].to_string()
    }

    /// Claude Code's fixture with non-ASCII turns and title is indexed whole. Passes only with
    /// the engine fix of graphersal main 2b2a148c (graphersal 0.6.1).
    #[test]
    fn non_ascii_claude_code_turns_are_indexed_whole() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("sess-utf8");
        let data = crate::runtime::mcp_tests::TempDir::new("sess-utf8-data");
        let (projects, _, file) = setup(tmp.path());
        let text = std::fs::read_to_string(&file)
            .unwrap()
            .replace("Add a cache to the parser.", &awkward_json());
        std::fs::write(&file, text).unwrap();
        let mut store = Direct::new();
        let report = index(&request(&projects), data.path(), &mut store);
        assert_eq!(report.problems, Vec::<String>::new());
        assert_eq!(report.indexed, 1);
        let user = store.query("g.v(\"turn:abc:0\").values(\"user\").next()");
        assert!(
            user[0].as_str().unwrap_or_default().starts_with(AWKWARD),
            "{user}"
        );
    }

    /// opencode's fixture database with non-ASCII parts and title is indexed whole. Passes only
    /// with the engine fix of graphersal main 2b2a148c (graphersal 0.6.1).
    #[test]
    fn non_ascii_opencode_turns_are_indexed_whole() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("oc-utf8");
        let data = crate::runtime::mcp_tests::TempDir::new("oc-utf8-data");
        let (db, _) = opencode_setup(tmp.path());
        let c = rusqlite::Connection::open(&db).unwrap();
        c.execute(
            "UPDATE part SET data = replace(data, 'Add a cache to the parser.', ?1)",
            [awkward_json()],
        )
        .unwrap();
        c.execute(
            "UPDATE session SET title = ?1 WHERE id = 'ses_abc'",
            [format!("Кэш е({AWKWARD}")],
        )
        .unwrap();
        drop(c);
        let mut store = Direct::new();
        let report = index(&opencode_request(&db), data.path(), &mut store);
        assert_eq!(report.problems, Vec::<String>::new());
        assert_eq!(report.indexed, 1);
        assert_eq!(
            store.query("g.v(\"session:ses_abc\").values(\"name\").next()"),
            json!([format!("Кэш е({AWKWARD}")])
        );
        let user = store.query("g.v(\"turn:ses_abc:0\").values(\"user\").next()");
        assert!(
            user[0].as_str().unwrap_or_default().contains(AWKWARD),
            "{user}"
        );
    }

    /// A writer that panics on the session `poisoned` (a bug in a store call).
    struct Panicking {
        inner: Direct,
        poisoned: String,
    }

    impl Writer for Panicking {
        fn call(&mut self, project: &str, name: &str, args: Value) -> Result<Value, String> {
            if args.to_string().contains(&self.poisoned) {
                panic!("a bug while writing");
            }
            self.inner.call(project, name, args)
        }
    }

    /// A panic while indexing one file is recorded; the run goes on with the next file; the
    /// file is tried once more in a later run and then skipped until it changes.
    #[test]
    fn a_failing_session_file_is_skipped_after_one_retry_and_the_rest_is_indexed() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("sess-fail");
        let data = crate::runtime::mcp_tests::TempDir::new("sess-fail-data");
        let (projects, _, file) = setup(tmp.path());
        let bad = file.with_file_name("bad.jsonl");
        std::fs::copy(&file, &bad).unwrap();
        let mut writer = Panicking {
            inner: Direct::new(),
            poisoned: "session:bad".to_string(),
        };
        let run_once = |writer: &mut Panicking| {
            run(
                &request(&projects),
                data.path(),
                writer,
                None,
                &mut |_| {},
                &|| false,
            )
        };
        let first = run_once(&mut writer);
        assert_eq!(first.indexed, 1, "{:?}", first.problems);
        assert_eq!(first.problems.len(), 1, "{:?}", first.problems);
        assert!(
            first.problems[0].contains("bad.jsonl"),
            "{:?}",
            first.problems
        );
        assert!(
            first.problems[0].contains("panicked"),
            "{:?}",
            first.problems
        );
        assert!(
            first.problems[0].contains("attempt 1 of 2"),
            "{:?}",
            first.problems
        );
        let second = run_once(&mut writer);
        assert!(
            second.problems[0].contains("attempt 2 of 2"),
            "{:?}",
            second.problems
        );
        let third = run_once(&mut writer);
        assert_eq!(
            (third.problems.len(), third.failed),
            (0, 1),
            "{:?}",
            third.problems
        );
        let state = State::load_for(data.path(), Kind::ClaudeCode).unwrap();
        assert_eq!((state.indexed(), state.failed()), (1, 1));
        // A new version of the file is tried again (and indexed once the bug is gone).
        writer.poisoned = "no-such-session".to_string();
        std::fs::write(
            &bad,
            format!("{}\n", std::fs::read_to_string(&file).unwrap()),
        )
        .unwrap();
        let fourth = run_once(&mut writer);
        assert_eq!(fourth.problems, Vec::<String>::new());
        assert_eq!(fourth.indexed, 1);
        let state = State::load_for(data.path(), Kind::ClaudeCode).unwrap();
        assert_eq!((state.indexed(), state.failed()), (2, 0));
    }

    /// A fake `claude`: records its arguments and prompt, answers a canned summary with a cost.
    #[cfg(unix)]
    pub(crate) fn fake_harness(dir: &Path, cost: f64) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let log = dir.join("calls.log");
        let answer = json!({"type": "result", "is_error": false, "total_cost_usd": cost,
            "structured_output": {"headline": "Added a parser cache",
                "summary": "The user asked for a cache in the parser; it was added and tested.",
                "decisions": [{"decision": "Cache parsed files by path in an LRU map",
                               "why": "repeated parses dominated the profile"}],
                "corrections": ["Keep the cache small"], "outcomes": ["Tests pass"],
                "open_threads": ["Measure the hit rate"]}});
        let script = bin.join("claude");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf 'CALL %s\\n' \"$*\" >> '{log}'\nprintf 'HOOK %s\\n' \"$GRAPHERSAL_MEMORY_HOOK\" >> '{log}'\ncat >> '{log}'\nprintf '\\nEND\\n' >> '{log}'\ncat <<'JSON'\n{answer}\nJSON\n",
                log = log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (script, log)
    }

    /// The settings file with summaries switched on (or off) for the fake harness.
    #[cfg(unix)]
    fn summaries_config(data: &Path, enabled: bool, harness: &Path, budget: f64) {
        std::fs::write(
            crate::config::path(data),
            json!({"version": 1, "summaries": {"enabled": enabled,
                "harness_command": format!("'{}' -p --model sonnet", harness.display()),
                "monthly_budget_usd": budget}})
            .to_string(),
        )
        .unwrap();
    }

    /// Makes the fixture long enough for a summary.
    #[cfg(unix)]
    fn lengthen(file: &Path) {
        let mut text = std::fs::read_to_string(file).unwrap();
        text.push_str(&format!(
            "{}\n",
            json!({"type": "user", "timestamp": "2026-01-03T09:00:00.000Z",
                   "message": {"content": "Explain the cache design in detail. ".repeat(60)}})
        ));
        std::fs::write(file, text).unwrap();
    }

    #[cfg(unix)]
    fn with_summaries(request: &Request, data: &Path, store: &mut Direct) -> Report {
        let (summarizer, err) = Summarizer::new(data);
        assert_eq!(err, None);
        let mut summarizer = summarizer;
        run(
            request,
            data,
            store,
            summarizer.as_mut(),
            &mut |_| {},
            &|| false,
        )
    }

    #[cfg(unix)]
    fn calls(log: &Path) -> usize {
        std::fs::read_to_string(log)
            .map(|text| text.lines().filter(|l| l.starts_with("CALL ")).count())
            .unwrap_or(0)
    }

    #[cfg(unix)]
    #[test]
    fn without_consent_the_harness_is_never_called() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("sum-off");
        let data = crate::runtime::mcp_tests::TempDir::new("sum-off-data");
        let (projects, _, file) = setup(tmp.path());
        lengthen(&file);
        let (harness, log) = fake_harness(tmp.path(), 0.1);
        let mut store = Direct::new();
        // No settings file, then a settings file naming the harness but not enabled.
        assert!(Summarizer::new(data.path()).0.is_none());
        let report = with_summaries(&request(&projects), data.path(), &mut store);
        assert_eq!(report.indexed, 1);
        summaries_config(data.path(), false, &harness, 100.0);
        assert!(Summarizer::new(data.path()).0.is_none());
        std::fs::remove_file(State::path(data.path())).unwrap();
        with_summaries(&request(&projects), data.path(), &mut store);
        assert_eq!(calls(&log), 0);
        assert!(!log.exists());
        assert_eq!(
            store.query("g.v().has_label(\"fact\").count().next()"),
            json!([0])
        );
        // A broken settings file sends nothing either.
        std::fs::write(crate::config::path(data.path()), "{").unwrap();
        let (none, err) = Summarizer::new(data.path());
        assert!(none.is_none() && err.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn with_consent_summaries_become_facts_that_sessions_finds() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("sum-on");
        let data = crate::runtime::mcp_tests::TempDir::new("sum-on-data");
        let (projects, work, file) = setup(tmp.path());
        lengthen(&file);
        let (harness, log) = fake_harness(tmp.path(), 0.12);
        summaries_config(data.path(), true, &harness, 5.0);
        let mut store = Direct::new();
        let report = with_summaries(&request(&projects), data.path(), &mut store);
        assert_eq!(report.problems, Vec::<String>::new());
        assert_eq!((report.indexed, report.summarized, report.facts), (1, 1, 4));
        assert_eq!(report.summaries_pending, 0);
        let recorded = std::fs::read_to_string(&log).unwrap();
        assert_eq!(calls(&log), 1);
        assert!(
            recorded.contains("CALL -p --model sonnet --output-format json --json-schema"),
            "{recorded}"
        );
        assert!(
            recorded.contains("--strict-mcp-config --no-session-persistence"),
            "{recorded}"
        );
        assert!(recorded.contains("HOOK off"), "{recorded}");
        assert!(
            recorded.contains("USER: Add a cache to the parser."),
            "{recorded}"
        );
        assert!(
            !recorded.contains("test output"),
            "tool results are never sent"
        );

        let session = store.query(
            "g.v(\"session:abc\").project(\"name\", \"body\").by(\"name\").by(\"body\").next()",
        );
        assert_eq!(session[0]["name"], "Added a parser cache");
        assert!(
            session[0]["body"]
                .as_str()
                .unwrap()
                .contains("**Decisions:**")
        );
        let project = work.display().to_string();
        let found = crate::runtime::catalog::call(
            &mut store.session,
            "sessions",
            &json!({"text": "why an LRU cache for parsed files", "k": 2, "project": project}),
        )
        .unwrap();
        assert_eq!(found[0]["kind"], "decision", "{found}");
        assert_eq!(found[0]["fact"], "Cache parsed files by path in an LRU map");
        assert_eq!(found[0]["why"], "repeated parses dominated the profile");
        assert_eq!(found[0]["session"], "session:abc");
        assert_eq!(found[0]["headline"], "Added a parser cache");
        // Another project does not see them.
        let other = crate::runtime::catalog::call(
            &mut store.session,
            "sessions",
            &json!({"text": "LRU cache", "k": 2, "project": "/elsewhere"}),
        )
        .unwrap();
        assert_eq!(other, json!([]));
        let spend = summaries::Spend::load(data.path()).unwrap();
        assert!((spend.usd(&summaries::this_month()) - 0.12).abs() < 1e-9);

        // Done once: a second run calls nothing; a re-index of a grown session keeps the
        // summary until the new one replaces it.
        let again = with_summaries(&request(&projects), data.path(), &mut store);
        assert_eq!(again.summarized, 0);
        assert_eq!(calls(&log), 1);
        lengthen(&file);
        let mut store_calls = Direct {
            session: store.session,
            calls: Vec::new(),
        };
        let grown = run(
            &request(&projects),
            data.path(),
            &mut store_calls,
            None,
            &mut |_| {},
            &|| false,
        );
        assert_eq!(grown.indexed, 1);
        let name = store_calls.query("g.v(\"session:abc\").values(\"name\").next()");
        assert_eq!(name, json!(["Added a parser cache"]));
        assert_eq!(grown.summaries_pending, 1);
    }

    #[cfg(unix)]
    #[test]
    fn the_monthly_budget_stops_the_calls() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("sum-cap");
        let data = crate::runtime::mcp_tests::TempDir::new("sum-cap-data");
        let (projects, _, file) = setup(tmp.path());
        lengthen(&file);
        let second = file.with_file_name("def.jsonl");
        std::fs::copy(&file, &second).unwrap();
        let (harness, log) = fake_harness(tmp.path(), 0.25);
        summaries_config(data.path(), true, &harness, 0.3);
        let mut store = Direct::new();
        let report = with_summaries(&request(&projects), data.path(), &mut store);
        assert_eq!(
            report.indexed, 2,
            "raw turns are indexed whatever the budget"
        );
        assert_eq!(report.summarized, 1);
        assert!(report.budget_reached);
        assert_eq!(report.summaries_pending, 1);
        assert_eq!(calls(&log), 1);
        // The next run in the same month calls nothing.
        let again = with_summaries(&request(&projects), data.path(), &mut store);
        assert!(again.budget_reached);
        assert_eq!(calls(&log), 1);
        // A budget of 0 never calls.
        summaries_config(data.path(), true, &harness, 0.0);
        std::fs::remove_file(data.path().join("sessions").join("spend.json")).unwrap();
        let zero = with_summaries(&request(&projects), data.path(), &mut store);
        assert!(zero.budget_reached);
        assert_eq!(calls(&log), 1);
    }

    #[cfg(unix)]
    #[test]
    fn the_estimate_counts_pending_sessions_without_calling_anything() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("sum-est");
        let data = crate::runtime::mcp_tests::TempDir::new("sum-est-data");
        let (projects, _, file) = setup(tmp.path());
        lengthen(&file);
        let estimate = estimate(&projects, data.path(), Duration::ZERO, None).unwrap();
        assert_eq!((estimate.sessions, estimate.pending), (1, 1));
        assert!(estimate.usd > 0.0 && estimate.usd < 1.0, "{estimate:?}");
        let later = estimate_since(&projects, data.path(), "2027-01-01");
        assert_eq!(later.pending, 0);
    }

    #[cfg(unix)]
    fn estimate_since(projects: &Path, data: &Path, since: &str) -> Estimate {
        estimate(projects, data, Duration::ZERO, Some(since)).unwrap()
    }

    fn opencode_setup(tmp: &Path) -> (PathBuf, PathBuf) {
        let work = crate::runtime::mcp::canonical(tmp).join("work");
        std::fs::create_dir_all(work.join(".git")).unwrap();
        let db = tmp.join("share/opencode/opencode.db");
        crate::harnesses::opencode::sessions::tests::fixture(&db, &work.display().to_string());
        (db, work)
    }

    fn opencode_request(db: &Path) -> Request {
        Request {
            idle: Duration::ZERO,
            ..Request::for_harness(Kind::OpenCode, db)
        }
    }

    #[test]
    fn opencode_sessions_are_indexed_once_and_again_when_they_grow() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("oc-idx");
        let data = crate::runtime::mcp_tests::TempDir::new("oc-idx-data");
        let (db, work) = opencode_setup(tmp.path());
        let mut store = Direct::new();
        let first = index(&opencode_request(&db), data.path(), &mut store);
        assert_eq!(first.problems, Vec::<String>::new());
        assert_eq!((first.found, first.indexed, first.turns), (1, 1, 3));
        let session = store.query(
            "g.v(\"session:ses_abc\").project(\"name\", \"project\", \"harness\", \"files\", \"source\")\
             .by(\"name\").by(\"project\").by(\"harness\").by(\"files_edited\").by(\"source\").next()",
        );
        assert_eq!(session[0]["name"], "Parser cache");
        assert_eq!(session[0]["project"], work.display().to_string());
        assert_eq!(session[0]["harness"], "opencode");
        assert_eq!(session[0]["files"], json!(["src/parse.rs"]));
        assert_eq!(session[0]["source"], "opencode:ses_abc");
        assert_eq!(
            store.query("g.v(\"turn:ses_abc:0\").values(\"tools\").next()"),
            json!(["edit"])
        );
        assert!(data.path().join("sessions/opencode.json").exists());
        assert!(!data.path().join("sessions/claude-code.json").exists());
        // Unchanged: nothing is read or written again.
        store.calls.clear();
        let second = index(&opencode_request(&db), data.path(), &mut store);
        assert_eq!((second.indexed, second.unchanged), (0, 1));
        assert!(store.calls.is_empty(), "{:?}", store.calls);
        // Grown: indexed again.
        crate::harnesses::opencode::sessions::tests::grow(&db);
        let grown = index(&opencode_request(&db), data.path(), &mut store);
        assert_eq!((grown.indexed, grown.turns), (1, 4), "{grown:?}");
        // A running session waits; an ended one does not.
        let unit = crate::harnesses::opencode::sessions::unit_path(&db, "ses_abc");
        let mut active = opencode_request(&db);
        active.idle = Duration::from_secs(3600 * 24 * 365 * 100);
        active.active.insert(unit.clone());
        assert_eq!(index(&active, data.path(), &mut store).active, 1);
        // The control line carries the harness.
        let line = opencode_request(&db).to_control();
        assert_eq!(line["harness"], "opencode");
        assert_eq!(
            Request::from_control(&line).unwrap().harness,
            Kind::OpenCode
        );
        let old = json!({"projects_dir": crate::runtime::mcp_tests::abs("/x"), "ended": [], "active": []});
        assert_eq!(
            Request::from_control(&old).unwrap().harness,
            Kind::ClaudeCode
        );
    }

    #[test]
    fn after_an_upgrade_to_2x_only_new_sessions_are_indexed() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("oc12-idx");
        let data = crate::runtime::mcp_tests::TempDir::new("oc12-idx-data");
        let (db, work) = opencode_setup(tmp.path());
        let mut store = Direct::new();
        let first = index(&opencode_request(&db), data.path(), &mut store);
        assert_eq!((first.found, first.indexed), (1, 1), "{first:?}");
        // 2.x upgrades the database: the 1.x session is copied (same id and time), a new
        // session is made in 2.x.
        crate::harnesses::opencode::sessions::tests::fixture_v2(&db, &work.display().to_string());
        let c = rusqlite::Connection::open(&db).unwrap();
        c.execute(
            "INSERT INTO session_v2 (id, project_id, slug, directory, title, version, \
             time_created, time_updated) SELECT id, project_id, slug, directory, title, version, \
             time_created, time_updated FROM session WHERE id = 'ses_abc'",
            [],
        )
        .unwrap();
        let second = index(&opencode_request(&db), data.path(), &mut store);
        assert_eq!(second.problems, Vec::<String>::new());
        assert_eq!(
            (second.found, second.indexed, second.unchanged),
            (2, 1, 1),
            "the copied session is not indexed again: {second:?}"
        );
        let new = store.query(
            "g.v(\"session:ses_v2a\").project(\"harness\", \"files\").by(\"harness\")\
             .by(\"files_edited\").next()",
        );
        assert_eq!(new[0]["harness"], "opencode");
        // Files outside the project are left out, as for every harness.
        assert_eq!(new[0]["files"], json!(["src/flags.rs"]));
        assert_eq!(
            store.query("g.v(\"turn:ses_v2a:1\").values(\"tools\").next()"),
            json!(["patch"])
        );
    }

    /// A fake opencode 2.x: answers `--version` with `opencode v2.2.4`, everything else like
    /// [`fake_opencode`].
    #[cfg(unix)]
    fn fake_opencode_v2(dir: &Path) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let (inner, log) = fake_opencode(&dir.join("inner"));
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = bin.join("opencode");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'opencode v2.2.4'; exit 0; fi\nexec '{}' \"$@\"\n",
                inner.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (script, log)
    }

    #[cfg(unix)]
    #[test]
    fn summaries_through_opencode_2x_run_standalone_in_the_sandbox() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("oc2-sum");
        let data = crate::runtime::mcp_tests::TempDir::new("oc2-sum-data");
        let work = crate::runtime::mcp::canonical(tmp.path()).join("work");
        std::fs::create_dir_all(work.join(".git")).unwrap();
        let db = tmp.path().join("share/opencode/opencode.db");
        crate::harnesses::opencode::sessions::tests::fixture_v2(&db, &work.display().to_string());
        {
            // Long enough for a summary.
            let c = rusqlite::Connection::open(&db).unwrap();
            c.execute(
                "INSERT INTO session_message VALUES ('msg_99', 'ses_v2a', 'user', 99, 1, 1, ?1)",
                [json!({"time": {"created": 1}, "files": [],
                    "text": "Explain the flag design in detail. ".repeat(60)})
                .to_string()],
            )
            .unwrap();
        }
        let (harness, log) = fake_opencode_v2(tmp.path());
        std::fs::write(
            crate::config::path(data.path()),
            json!({"version": 1, "summaries": {"enabled": true,
                "harness_command": format!("'{}' run --model opencode/big-pickle", harness.display()),
                "monthly_budget_usd": 0.5}})
            .to_string(),
        )
        .unwrap();
        let mut store = Direct::new();
        let report = with_summaries(&opencode_request(&db), data.path(), &mut store);
        assert_eq!(report.problems, Vec::<String>::new());
        assert_eq!((report.indexed, report.summarized), (1, 1));
        let recorded = std::fs::read_to_string(&log).unwrap();
        assert!(
            recorded.contains("CALL run --standalone --model opencode/big-pickle --format json"),
            "{recorded}"
        );
        let sandbox = data.path().join("sessions/harness/opencode");
        assert!(
            recorded.contains(&format!(
                "ENV {} {} off",
                sandbox.join("config").display(),
                sandbox.join("data").display()
            )),
            "{recorded}"
        );
        assert!(
            recorded.contains("USER: Rename the config flag."),
            "{recorded}"
        );
    }

    /// A fake `opencode`: records its arguments, environment and prompt, answers `--format
    /// json` events with a summary and a cost of 0 (a free model).
    #[cfg(unix)]
    fn fake_opencode(dir: &Path) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let log = dir.join("calls.log");
        let summary = json!({"headline": "Added a parser cache",
            "summary": "The user asked for a cache in the parser; it was added.",
            "decisions": [{"decision": "Use an LRU map", "why": "repeated parses"}],
            "corrections": [], "outcomes": ["Tests pass"], "open_threads": []});
        let events = [
            json!({"type": "step_start", "part": {"type": "step-start"}}),
            json!({"type": "text", "part": {"type": "text", "text": summary.to_string()}}),
            json!({"type": "step_finish", "part": {"type": "step-finish", "cost": 0,
                "tokens": {"input": 900, "output": 100, "reasoning": 0, "cache": {"read": 0, "write": 0}}}}),
        ]
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let script = bin.join("opencode");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf 'CALL %s\\n' \"$*\" >> '{log}'\nprintf 'ENV %s %s %s\\n' \"$XDG_CONFIG_HOME\" \"$XDG_DATA_HOME\" \"$GRAPHERSAL_MEMORY_HOOK\" >> '{log}'\ncat >> '{log}'\nprintf '\\nEND\\n' >> '{log}'\ncat <<'JSON'\n{events}\nJSON\n",
                log = log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (script, log)
    }

    #[cfg(unix)]
    #[test]
    fn summaries_through_opencode_run_in_a_sandbox_and_cost_nothing_with_a_free_model() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("oc-sum");
        let data = crate::runtime::mcp_tests::TempDir::new("oc-sum-data");
        let (db, _) = opencode_setup(tmp.path());
        // Long enough for a summary.
        {
            let c = rusqlite::Connection::open(&db).unwrap();
            c.execute(
                "INSERT INTO message VALUES ('msg_09', 'ses_abc', 1767440000000, 1767440000000, ?1)",
                [json!({"role": "user", "time": {"created": 9}}).to_string()],
            )
            .unwrap();
            c.execute(
                "INSERT INTO part VALUES ('prt_09', 'msg_09', 'ses_abc', 1767440000000, 1767440000000, ?1)",
                [json!({"type": "text", "text": "Explain the cache design in detail. ".repeat(60)}).to_string()],
            )
            .unwrap();
        }
        let (harness, log) = fake_opencode(tmp.path());
        std::fs::write(
            crate::config::path(data.path()),
            json!({"version": 1, "summaries": {"enabled": true,
                "harness_command": format!("'{}' run --model opencode/big-pickle", harness.display()),
                "monthly_budget_usd": 0.5}})
            .to_string(),
        )
        .unwrap();
        let mut store = Direct::new();
        let report = with_summaries(&opencode_request(&db), data.path(), &mut store);
        assert_eq!(report.problems, Vec::<String>::new());
        assert_eq!((report.indexed, report.summarized), (1, 1));
        let recorded = std::fs::read_to_string(&log).unwrap();
        assert!(
            recorded.contains("CALL run --model opencode/big-pickle --format json"),
            "{recorded}"
        );
        let sandbox = data.path().join("sessions/harness/opencode");
        assert!(
            recorded.contains(&format!(
                "ENV {} {} off",
                sandbox.join("config").display(),
                sandbox.join("data").display()
            )),
            "{recorded}"
        );
        assert!(
            recorded.contains("USER: Add a cache to the parser."),
            "{recorded}"
        );
        let config: Value = serde_json::from_str(
            &std::fs::read_to_string(sandbox.join("config/opencode/opencode.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(config["permission"]["*"], "deny");
        assert_eq!(config["mcp"], json!({}));
        assert_eq!(
            store.query("g.v(\"session:ses_abc\").values(\"name\").next()"),
            json!(["Added a parser cache"])
        );
        let spend = summaries::Spend::load(data.path()).unwrap();
        assert_eq!(spend.usd(&summaries::this_month()), 0.0);
        let estimate =
            estimate_for(Kind::OpenCode, &db, data.path(), Duration::ZERO, None).unwrap();
        assert_eq!(estimate.sessions, 1);
        let line = crate::commands::estimate_line_for(
            &estimate,
            &crate::config::split_command("opencode run --model opencode/big-pickle").unwrap(),
        );
        assert!(line.contains("$0.00"), "{line}");
    }

    fn codex_setup(tmp: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let work = crate::runtime::mcp::canonical(tmp).join("work");
        std::fs::create_dir_all(work.join(".git")).unwrap();
        let sessions = tmp.join("codex/sessions");
        let file = crate::harnesses::codex::sessions::tests::write_fixture(
            &sessions,
            &work.display().to_string(),
        );
        (sessions, work, file)
    }

    fn codex_request(sessions: &Path) -> Request {
        Request {
            idle: Duration::ZERO,
            ..Request::for_harness(Kind::Codex, sessions)
        }
    }

    #[test]
    fn codex_rollouts_are_indexed_once_and_again_when_they_grow() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("cx-idx");
        let data = crate::runtime::mcp_tests::TempDir::new("cx-idx-data");
        let (sessions, work, file) = codex_setup(tmp.path());
        let id = "019a5c1e-0000-7000-8000-000000000001";
        let mut store = Direct::new();
        let first = index(&codex_request(&sessions), data.path(), &mut store);
        assert_eq!(first.problems, Vec::<String>::new());
        assert_eq!((first.found, first.indexed, first.turns), (1, 1, 2));
        let session = store.query(&format!(
            "g.v(\"session:{id}\").project(\"name\", \"project\", \"harness\", \"files\", \"source\")\
             .by(\"name\").by(\"project\").by(\"harness\").by(\"files_edited\").by(\"source\").next()"
        ));
        assert_eq!(session[0]["harness"], "codex");
        assert_eq!(session[0]["project"], work.display().to_string());
        assert_eq!(session[0]["files"], json!(["src/parser.rs"]));
        assert_eq!(
            session[0]["name"],
            "Use tabs in this repository, never spaces."
        );
        assert!(
            session[0]["source"]
                .as_str()
                .unwrap()
                .starts_with("rollout:rollout-2026-11-09"),
            "{session}"
        );
        assert!(data.path().join("sessions/codex.json").exists());
        // Unchanged: not read again.
        let second = index(&codex_request(&sessions), data.path(), &mut store);
        assert_eq!((second.found, second.indexed), (1, 0));
        // A resumed session appends to its rollout: indexed again.
        let mut text = std::fs::read_to_string(&file).unwrap();
        text.push_str(&format!(
            "{}\n{}\n",
            json!({"timestamp": "2026-11-09T11:00:00.000Z", "type": "event_msg",
                   "payload": {"type": "user_message", "message": "And the lexer?"}}),
            json!({"timestamp": "2026-11-09T11:00:05.000Z", "type": "event_msg",
                   "payload": {"type": "agent_message", "message": "The lexer is fine."}})
        ));
        std::fs::write(&file, text).unwrap();
        let grown = index(&codex_request(&sessions), data.path(), &mut store);
        assert_eq!((grown.indexed, grown.turns), (1, 3), "{:?}", grown.problems);
        // The session being written is left alone.
        let mut active = codex_request(&sessions);
        active.active.insert(file.clone());
        let line = active.to_control();
        assert_eq!(line["harness"], "codex");
        assert_eq!(Request::from_control(&line).unwrap().harness, Kind::Codex);
    }

    #[test]
    fn cursor_transcripts_reported_by_the_hooks_are_indexed_with_their_workspace() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("cu-idx");
        let data = crate::runtime::mcp_tests::TempDir::new("cu-idx-data");
        let work = crate::runtime::mcp::canonical(tmp.path()).join("work");
        std::fs::create_dir_all(work.join(".git")).unwrap();
        let registry = data
            .path()
            .join("sessions")
            .join(crate::harnesses::cursor::REGISTRY_FILE);
        let file = crate::harnesses::cursor::sessions::tests::write_fixture(
            &tmp.path().join("cursor"),
            "c1",
        );
        // A transcript nobody reported is not read.
        let unreported = crate::harnesses::cursor::sessions::tests::write_fixture(
            &tmp.path().join("cursor"),
            "c0",
        );
        let request = Request {
            idle: Duration::ZERO,
            ..Request::for_harness(Kind::Cursor, &registry)
        };
        let mut store = Direct::new();
        let none = index(&request, data.path(), &mut store);
        assert_eq!((none.found, none.indexed), (0, 0), "{:?}", none.problems);
        crate::harnesses::cursor::sessions::record(
            &registry,
            &file,
            &work.display().to_string(),
            "c1",
        )
        .unwrap();
        let first = index(&request, data.path(), &mut store);
        assert_eq!(first.problems, Vec::<String>::new());
        assert_eq!((first.found, first.indexed, first.turns), (1, 1, 2));
        let session = store.query(
            "g.v(\"session:c1\").project(\"name\", \"project\", \"harness\", \"source\")\
             .by(\"name\").by(\"project\").by(\"harness\").by(\"source\").next()",
        );
        assert_eq!(session[0]["harness"], "cursor");
        assert_eq!(session[0]["project"], work.display().to_string());
        assert_eq!(session[0]["source"], "cursor:c1.jsonl");
        assert_eq!(
            session[0]["name"],
            "Use tabs in this repository, never spaces."
        );
        assert!(data.path().join("sessions/cursor.json").exists());
        let again = index(&request, data.path(), &mut store);
        assert_eq!((again.found, again.indexed, again.unchanged), (1, 0, 1));
        // Active: left alone; the control line names the harness.
        let mut active = request.clone();
        active.active.insert(file.clone());
        assert_eq!(index(&active, data.path(), &mut store).active, 1);
        let line = active.to_control();
        assert_eq!(line["harness"], "cursor");
        assert_eq!(Request::from_control(&line).unwrap().harness, Kind::Cursor);
        assert!(crate::harnesses::cursor::sessions::project_of(&registry, &unreported).is_none());
        let estimate =
            estimate_for(Kind::Cursor, &registry, data.path(), Duration::ZERO, None).unwrap();
        assert_eq!(estimate.sessions, 1);
    }

    /// A fake `codex`: records its arguments, `CODEX_HOME` and prompt, answers `exec --json`
    /// with the events of one summary.
    #[cfg(unix)]
    fn fake_codex(dir: &Path) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let log = dir.join("codex-calls.log");
        let summary = json!({"headline": "Tabs and a parser fix",
            "summary": "The user set a tabs rule; the parser's off-by-one was fixed.",
            "decisions": [{"decision": "Tabs, never spaces", "why": "repository style"}],
            "corrections": [], "outcomes": ["Parser fixed"], "open_threads": []});
        let events = [
            json!({"type": "thread.started", "thread_id": "t"}),
            json!({"type": "turn.started"}),
            json!({"type": "item.completed", "item": {"id": "i", "type": "agent_message",
                   "text": summary.to_string()}}),
            json!({"type": "turn.completed", "usage": {"input_tokens": 900,
                   "cached_input_tokens": 0, "output_tokens": 100, "reasoning_output_tokens": 0}}),
        ]
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let script = bin.join("codex");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf 'CALL %s\\n' \"$*\" >> '{log}'\nprintf 'ENV %s %s\\n' \"$CODEX_HOME\" \"$GRAPHERSAL_MEMORY_HOOK\" >> '{log}'\ncat >> '{log}'\nprintf '\\nEND\\n' >> '{log}'\ncat <<'JSON'\n{events}\nJSON\n",
                log = log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (script, log)
    }

    #[cfg(unix)]
    #[test]
    fn summaries_through_codex_exec_run_in_a_sandbox_home_and_cost_nothing_locally() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("cx-sum");
        let data = crate::runtime::mcp_tests::TempDir::new("cx-sum-data");
        let (sessions, _, file) = codex_setup(tmp.path());
        // Long enough for a summary.
        let mut text = std::fs::read_to_string(&file).unwrap();
        text.push_str(&format!(
            "{}\n",
            json!({"timestamp": "2026-11-09T10:02:00.000Z", "type": "event_msg", "payload": {
                "type": "user_message", "message": "Explain the parser design in detail. ".repeat(60)}})
        ));
        std::fs::write(&file, text).unwrap();
        // The user's Codex home: a model setting, a login, an MCP server of theirs.
        let user_home = tmp.path().join("user-codex");
        std::fs::create_dir_all(&user_home).unwrap();
        std::fs::write(
            user_home.join("config.toml"),
            "model = \"openai/gpt-oss-20b\"\n\n[mcp_servers.theirs]\ncommand = \"x\"\n",
        )
        .unwrap();
        std::fs::write(user_home.join("auth.json"), "{}").unwrap();
        let (harness, log) = fake_codex(tmp.path());
        std::fs::write(
            crate::config::path(data.path()),
            json!({"version": 1, "summaries": {"enabled": true,
                "harness_command": format!("'{}' exec --oss --local-provider lmstudio", harness.display()),
                "monthly_budget_usd": 0.5}})
            .to_string(),
        )
        .unwrap();
        let (summarizer, err) = Summarizer::new(data.path());
        assert_eq!(err, None);
        // The user's Codex home, as the daemon's environment would name it.
        let home = tmp.path().display().to_string();
        let codex_home = user_home.display().to_string();
        let mut summarizer = summarizer.map(|s| {
            s.with_user_env(std::sync::Arc::new(move |name: &str| match name {
                "HOME" => Some(home.clone()),
                "CODEX_HOME" => Some(codex_home.clone()),
                _ => None,
            }))
        });
        let mut store = Direct::new();
        let report = run(
            &codex_request(&sessions),
            data.path(),
            &mut store,
            summarizer.as_mut(),
            &mut |_| {},
            &|| false,
        );
        assert_eq!(report.problems, Vec::<String>::new());
        assert_eq!((report.indexed, report.summarized), (1, 1));
        let recorded = std::fs::read_to_string(&log).unwrap();
        assert!(
            recorded.contains(
                "CALL exec --oss --local-provider lmstudio --json --ephemeral --skip-git-repo-check --sandbox read-only -c model_provider=\"lmstudio\""
            ),
            "{recorded}"
        );
        let sandbox = data.path().join("sessions/harness/codex/home");
        assert!(
            recorded.contains(&format!("ENV {} off", sandbox.display())),
            "{recorded}"
        );
        assert!(
            recorded.contains("Use tabs in this repository"),
            "{recorded}"
        );
        let config = crate::install::tomledit::parse(
            &std::fs::read_to_string(sandbox.join("config.toml")).unwrap(),
        )
        .unwrap();
        assert_eq!(config["model"], "openai/gpt-oss-20b");
        assert!(
            config["mcp_servers"].is_null(),
            "the user's servers stay out"
        );
        assert_eq!(config["features"]["shell_tool"], false);
        assert!(sandbox.join("auth.json").is_file());
        let id = "019a5c1e-0000-7000-8000-000000000001";
        assert_eq!(
            store.query(&format!("g.v(\"session:{id}\").values(\"name\").next()")),
            json!(["Tabs and a parser fix"])
        );
        let spend = summaries::Spend::load(data.path()).unwrap();
        assert_eq!(spend.usd(&summaries::this_month()), 0.0);
        let estimate =
            estimate_for(Kind::Codex, &sessions, data.path(), Duration::ZERO, None).unwrap();
        assert_eq!(estimate.sessions, 1);
        let line = crate::commands::estimate_line_for(
            &estimate,
            &crate::config::split_command("codex exec --oss").unwrap(),
        );
        assert!(
            line.contains("$0.00") && line.contains("local model"),
            "{line}"
        );
    }
}
