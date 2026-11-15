//! opencode's stored sessions -> [`Transcript`]s, read from its SQLite database `opencode.db`.
//! Both lines keep it in the same file: 1.x in the tables `session`, `message` and `part`
//! (`packages/core/src/session/sql.ts` at v1.19.5; the columns read here exist since its first
//! migration), 2.x in `session_v2`, `session_message` and `session_part`
//! (`packages/core/src/session/sql.ts` at v2.2.4). 2.x upgrades a 1.x file in place: it copies
//! every 1.x session into its tables with the same id and leaves the 1.x rows as they were
//! (`packages/core/src/database/v1-migration.bun.ts`), so after an upgrade a session can be in
//! both. Each session is read once ([`Source`]): from 2.x's tables when 2.x changed it after
//! the copy (its `time_updated` is newer than the 1.x row's), else from 1.x's (a copy that only
//! migrated keeps the 1.x row's time, so its fingerprint, and the graph's session, stay as they
//! were).
//!
//! - **Sessions**: the rows without `parent_id` (sub-agent sessions are left out, as Claude
//!   Code's sidechains are). Working directory `directory`, title `title` (2.x: may be empty),
//!   times `time_created`/`time_updated` (milliseconds).
//! - **1.x turns**: a user message's `text` parts (not `synthetic` or `ignored`) start a turn;
//!   the assistant messages up to the next prompt add their `text` parts, the names of their
//!   `tool` parts, and the files their `edit`/`write` tools (`state.input.filePath`) and
//!   `patch` parts (`files`) changed. A `compaction` part's summary (the next assistant text of
//!   a `summary` message) is kept as a compaction summary.
//! - **2.x turns**: the `session_message` rows in `seq` order, typed by their `type` column. A
//!   `user` row's `text` starts a turn; an `assistant` row's content (its `session_part` rows
//!   when it has any, else the `content` array in its data, as 2.x itself reads it,
//!   `session/history.ts`, `withParts`) adds its `text` parts, its tool names, and the files of
//!   the `edit`/`write` tools (`state.input.path`, relative to the session's directory) and of
//!   `patch` (`state.metadata.files[].file`); a completed `compaction` row's `summary` is kept
//!   as a compaction summary. Other rows (switches, shell, synthetic, system, idle) are left
//!   out.
//! - Reasoning, tool input and tool output are never kept (but the slugs of the memories a call
//!   of our memory tools names, `crate::transcript::memory_writes`).
//! - **Change detection**: a session's unit size is the bytes of its message and part data, its
//!   modification time the newest `time_updated` of the session and its parts (2.x: and its
//!   messages); the fingerprint covers exactly the data the transcript is built from, in order.
//! - The database is opened read-only (opencode keeps it in WAL mode and may write while it is
//!   read); a database that does not exist yet means no sessions.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;

use crate::sessions::Unit;
use crate::transcript::{Transcript, Turn, clip, memory_writes};

/// The separator between the database path and a session id in a unit path.
const SEPARATOR: char = '#';

/// The unit path of session `id` in database `db`: `<db>#<id>`.
pub fn unit_path(db: &Path, id: &str) -> PathBuf {
    PathBuf::from(format!("{}{SEPARATOR}{id}", db.display()))
}

/// The database and session id of a unit path made by [`unit_path`] (`None` for a transcript
/// file).
pub fn split_unit(path: &Path) -> Option<(PathBuf, String)> {
    let text = path.to_str()?;
    let (db, id) = text.rsplit_once(SEPARATOR)?;
    (id.starts_with("ses_") && db.ends_with(".db")).then(|| (PathBuf::from(db), id.to_string()))
}

fn open(db: &Path) -> Result<Connection, String> {
    let connection = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|err| format!("cannot open opencode's database {}: {err}", db.display()))?;
    let _ = connection.busy_timeout(std::time::Duration::from_secs(5));
    Ok(connection)
}

fn sql_error(db: &Path) -> impl Fn(rusqlite::Error) -> String + '_ {
    move |err| format!("cannot read opencode's database {}: {err}", db.display())
}

/// The table names of `db`.
pub fn tables(db: &Path) -> Result<Vec<String>, String> {
    let connection = open(db)?;
    table_names(&connection, db)
}

fn table_names(connection: &Connection, db: &Path) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .map_err(sql_error(db))?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(sql_error(db))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error(db))?;
    Ok(names)
}

/// Which tables of a database hold sessions.
#[derive(Debug, Clone, Copy, Default)]
struct Layout {
    /// 1.x: `session`, `message`, `part`.
    v1: bool,
    /// 2.x: `session_v2`, `session_message`.
    v2: bool,
    /// 2.x: `session_part` (added in 2.2).
    v2_parts: bool,
}

fn layout(connection: &Connection, db: &Path) -> Result<Layout, String> {
    let names = table_names(connection, db)?;
    let has = |name: &str| names.iter().any(|n| n == name);
    Ok(Layout {
        v1: has("session") && has("message") && has("part"),
        v2: has("session_v2") && has("session_message"),
        v2_parts: has("session_part"),
    })
}

/// Which tables a session is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    V1,
    V2,
}

/// One top-level session row: id, directory, time_updated, size, newest part/message time.
struct Row {
    id: String,
    directory: String,
    updated: i64,
    size: i64,
    newest: i64,
}

fn v1_rows(connection: &Connection, db: &Path) -> Result<Vec<Row>, String> {
    let mut statement = connection
        .prepare(
            "SELECT s.id, s.directory, s.time_updated,
                    COALESCE((SELECT SUM(LENGTH(m.data)) FROM message m WHERE m.session_id = s.id), 0)
                  + COALESCE((SELECT SUM(LENGTH(p.data)) FROM part p WHERE p.session_id = s.id), 0),
                    COALESCE((SELECT MAX(p.time_updated) FROM part p WHERE p.session_id = s.id), 0)
             FROM session s WHERE s.parent_id IS NULL ORDER BY s.id",
        )
        .map_err(sql_error(db))?;
    let rows = statement
        .query_map([], |row| {
            Ok(Row {
                id: row.get(0)?,
                directory: row.get(1)?,
                updated: row.get(2)?,
                size: row.get(3)?,
                newest: row.get(4)?,
            })
        })
        .map_err(sql_error(db))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error(db))?;
    Ok(rows)
}

fn v2_rows(connection: &Connection, db: &Path, layout: Layout) -> Result<Vec<Row>, String> {
    let (part_size, part_newest) = if layout.v2_parts {
        (
            "COALESCE((SELECT SUM(LENGTH(p.data)) FROM session_part p WHERE p.session_id = s.id), 0)",
            "COALESCE((SELECT MAX(p.time_updated) FROM session_part p WHERE p.session_id = s.id), 0)",
        )
    } else {
        ("0", "0")
    };
    let sql = format!(
        "SELECT s.id, s.directory, s.time_updated,
                COALESCE((SELECT SUM(LENGTH(m.data)) FROM session_message m WHERE m.session_id = s.id), 0)
              + {part_size},
                MAX(COALESCE((SELECT MAX(m.time_updated) FROM session_message m WHERE m.session_id = s.id), 0),
                    {part_newest})
         FROM session_v2 s WHERE s.parent_id IS NULL ORDER BY s.id"
    );
    let mut statement = connection.prepare(&sql).map_err(sql_error(db))?;
    let rows = statement
        .query_map([], |row| {
            Ok(Row {
                id: row.get(0)?,
                directory: row.get(1)?,
                updated: row.get(2)?,
                size: row.get(3)?,
                newest: row.get(4)?,
            })
        })
        .map_err(sql_error(db))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error(db))?;
    Ok(rows)
}

/// Every top-level session of `db` once, with the tables it is read from, ordered by id.
fn sessions(connection: &Connection, db: &Path) -> Result<Vec<(Row, Source)>, String> {
    let layout = layout(connection, db)?;
    let mut out: BTreeMap<String, (Row, Source)> = BTreeMap::new();
    if layout.v1 {
        for row in v1_rows(connection, db)? {
            out.insert(row.id.clone(), (row, Source::V1));
        }
    }
    if layout.v2 {
        for row in v2_rows(connection, db, layout)? {
            // A copy 2.x has not changed since the upgrade is read from 1.x's rows.
            let keep_v1 = out
                .get(&row.id)
                .is_some_and(|(v1, _)| row.updated <= v1.updated);
            if !keep_v1 {
                out.insert(row.id.clone(), (row, Source::V2));
            }
        }
    }
    Ok(out.into_values().collect())
}

/// The top-level sessions of `db` as indexing units (none when the database does not exist).
pub fn units(db: &Path) -> Result<Vec<Unit>, String> {
    if !db.is_file() {
        return Ok(Vec::new());
    }
    let connection = open(db)?;
    Ok(sessions(&connection, db)?
        .into_iter()
        .map(|(row, _)| Unit {
            path: unit_path(db, &row.id),
            size: u64::try_from(row.size).unwrap_or(0),
            modified: u64::try_from(row.updated.max(row.newest) / 1000).unwrap_or(0),
        })
        .collect())
}

/// The session ids of `db` whose working directory is `directory` (or below it), newest first:
/// the sessions of one project (the bench's question source).
pub fn of_project(db: &Path, directory: &Path) -> Result<Vec<(String, u64)>, String> {
    if !db.is_file() {
        return Ok(Vec::new());
    }
    let connection = open(db)?;
    let mut out: Vec<(String, i64)> = sessions(&connection, db)?
        .into_iter()
        .filter(|(row, _)| Path::new(&row.directory).starts_with(directory))
        .map(|(row, _)| (row.id, row.updated))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(out
        .into_iter()
        .map(|(id, updated)| (id, u64::try_from(updated / 1000).unwrap_or(0)))
        .collect())
}

/// Milliseconds since the epoch as an RFC 3339 UTC timestamp (empty for 0).
fn timestamp(ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    let secs = u64::try_from(ms / 1000).unwrap_or(0);
    let millis = ms % 1000;
    let base = crate::daemon::utc_timestamp(secs);
    // `utc_timestamp` ends with `Z`; add the milliseconds as Claude Code's records carry them.
    match base.strip_suffix('Z') {
        Some(head) => format!("{head}.{millis:03}Z"),
        None => base,
    }
}

/// Reads session `id` of `db` into a transcript (from the tables [`units`] chose for it).
pub fn read(db: &Path, id: &str) -> Result<Transcript, String> {
    let connection = open(db)?;
    let layout = layout(&connection, db)?;
    let updated = |table: &str| -> Result<Option<i64>, String> {
        connection
            .query_row(
                &format!("SELECT time_updated FROM {table} WHERE id = ?1"),
                [id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(sql_error(db))
    };
    let v1 = if layout.v1 { updated("session")? } else { None };
    let v2 = if layout.v2 {
        updated("session_v2")?
    } else {
        None
    };
    match (v1, v2) {
        (_, Some(new)) if v1.is_none_or(|old| new > old) => read_v2(&connection, db, id, layout),
        (Some(_), _) => read_v1(&connection, db, id),
        _ => Err(format!(
            "opencode session {id} is no longer in {}",
            db.display()
        )),
    }
}

fn read_v1(connection: &Connection, db: &Path, id: &str) -> Result<Transcript, String> {
    let session = connection
        .query_row(
            "SELECT directory, title, time_created, time_updated FROM session WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(sql_error(db))?
        .ok_or_else(|| format!("opencode session {id} is no longer in {}", db.display()))?;
    let mut messages = connection
        .prepare(
            "SELECT id, time_created, data FROM message WHERE session_id = ?1 \
             ORDER BY time_created, id",
        )
        .map_err(sql_error(db))?;
    let messages: Vec<(String, i64, String)> = messages
        .query_map([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(sql_error(db))?
        .collect::<Result<_, _>>()
        .map_err(sql_error(db))?;
    let mut parts = connection
        .prepare("SELECT message_id, data FROM part WHERE session_id = ?1 ORDER BY message_id, id")
        .map_err(sql_error(db))?;
    let mut by_message: BTreeMap<String, Vec<String>> = Default::default();
    for row in parts
        .query_map([id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(sql_error(db))?
    {
        let (message, data) = row.map_err(sql_error(db))?;
        by_message.entry(message).or_default().push(data);
    }
    let (directory, title, created, updated) = session;
    Ok(build(
        id,
        &directory,
        &title,
        created,
        updated,
        &messages,
        &by_message,
    ))
}

/// One 2.x message row: id, type, time_created, data.
type V2Message = (String, String, i64, String);

fn read_v2(
    connection: &Connection,
    db: &Path,
    id: &str,
    layout: Layout,
) -> Result<Transcript, String> {
    let session = connection
        .query_row(
            "SELECT directory, COALESCE(title, ''), time_created, time_updated FROM session_v2 \
             WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(sql_error(db))?
        .ok_or_else(|| format!("opencode session {id} is no longer in {}", db.display()))?;
    let mut messages = connection
        .prepare(
            "SELECT id, type, time_created, data FROM session_message WHERE session_id = ?1 \
             ORDER BY seq",
        )
        .map_err(sql_error(db))?;
    let messages: Vec<V2Message> = messages
        .query_map([id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(sql_error(db))?
        .collect::<Result<_, _>>()
        .map_err(sql_error(db))?;
    let mut by_message: HashMap<String, Vec<String>> = HashMap::new();
    if layout.v2_parts {
        let mut parts = connection
            .prepare(
                "SELECT message_id, data FROM session_part WHERE session_id = ?1 \
                 ORDER BY message_id, ordinal",
            )
            .map_err(sql_error(db))?;
        for row in parts
            .query_map([id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sql_error(db))?
        {
            let (message, data) = row.map_err(sql_error(db))?;
            by_message.entry(message).or_default().push(data);
        }
    }
    let (directory, title, created, updated) = session;
    Ok(build_v2(
        id,
        &directory,
        &title,
        created,
        updated,
        &messages,
        &by_message,
    ))
}

/// The transcript of one 2.x session's rows (see the module docs).
fn build_v2(
    id: &str,
    directory: &str,
    title: &str,
    created: i64,
    updated: i64,
    messages: &[V2Message],
    parts: &HashMap<String, Vec<String>>,
) -> Transcript {
    let mut out = Transcript {
        session_id: id.to_string(),
        cwd: (!directory.is_empty()).then(|| directory.to_string()),
        title: title.to_string(),
        started_at: timestamp(created),
        ended_at: timestamp(updated),
        ..Transcript::default()
    };
    let mut hashed: Vec<u8> = Vec::new();
    // Recorded text, maybe of another platform: joined as text, not as a local path.
    let absolute = |file: &str| -> String { crate::transcript::join_text(directory, file) };
    for (message_id, kind, at, data) in messages {
        hashed.extend_from_slice(data.as_bytes());
        hashed.push(b'\n');
        out.records += 1;
        let Ok(message) = serde_json::from_str::<Value>(data) else {
            out.skipped_lines += 1;
            continue;
        };
        match kind.as_str() {
            "user" => {
                let prompt = message["text"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if prompt.is_empty() {
                    continue;
                }
                let n = out.turns.len();
                out.turns.push(Turn {
                    n,
                    at: timestamp(*at),
                    user: prompt,
                    assistant: String::new(),
                    tools: Vec::new(),
                });
            }
            "assistant" => {
                let stored: Vec<Value> = parts
                    .get(message_id)
                    .into_iter()
                    .flatten()
                    .filter_map(|data| {
                        hashed.extend_from_slice(data.as_bytes());
                        hashed.push(b'\n');
                        serde_json::from_str::<Value>(data).ok()
                    })
                    .collect();
                let content: Vec<Value> = if stored.is_empty() {
                    message["content"].as_array().cloned().unwrap_or_default()
                } else {
                    stored
                };
                let text = content
                    .iter()
                    .filter(|p| p["type"] == "text")
                    .filter_map(|p| p["text"].as_str())
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                for part in content.iter().filter(|p| p["type"] == "tool") {
                    let Some(tool) = part["name"].as_str() else {
                        continue;
                    };
                    if let Some(turn) = out.turns.last_mut()
                        && !turn.tools.iter().any(|t| t == tool)
                    {
                        turn.tools.push(tool.to_string());
                    }
                    let state = &part["state"];
                    out.memory_writes
                        .extend(memory_writes(tool, &state["input"], &timestamp(*at)));
                    let mut files: Vec<String> = Vec::new();
                    if matches!(tool, "edit" | "write")
                        && let Some(file) = state["input"]["path"].as_str()
                    {
                        files.push(absolute(file));
                    }
                    if tool == "patch" {
                        for file in state["metadata"]["files"].as_array().into_iter().flatten() {
                            if let Some(file) = file["file"].as_str() {
                                files.push(absolute(file));
                            }
                        }
                    }
                    for file in files {
                        if !file.is_empty() && !out.files.contains(&file) {
                            out.files.push(file);
                        }
                    }
                }
                if let Some(turn) = out.turns.last_mut()
                    && !text.is_empty()
                {
                    if !turn.assistant.is_empty() {
                        turn.assistant.push_str("\n\n");
                    }
                    turn.assistant.push_str(&text);
                }
            }
            "compaction" => {
                if message["status"] == "completed"
                    && let Some(summary) = message["summary"].as_str()
                    && !summary.trim().is_empty()
                {
                    out.compact.push(clip(summary.trim(), 20_000));
                }
            }
            _ => {}
        }
    }
    out.bytes = hashed.len();
    out.fingerprint = crate::fsutil::fingerprint(&hashed);
    out
}

/// The transcript of one session's rows.
fn build(
    id: &str,
    directory: &str,
    title: &str,
    created: i64,
    updated: i64,
    messages: &[(String, i64, String)],
    parts: &BTreeMap<String, Vec<String>>,
) -> Transcript {
    let mut out = Transcript {
        session_id: id.to_string(),
        cwd: (!directory.is_empty()).then(|| directory.to_string()),
        title: title.to_string(),
        started_at: timestamp(created),
        ended_at: timestamp(updated),
        ..Transcript::default()
    };
    let mut hashed: Vec<u8> = Vec::new();
    let mut in_summary = false;
    for (message_id, at, data) in messages {
        hashed.extend_from_slice(data.as_bytes());
        hashed.push(b'\n');
        out.records += 1;
        let Ok(message) = serde_json::from_str::<Value>(data) else {
            out.skipped_lines += 1;
            continue;
        };
        let message_parts: Vec<Value> = parts
            .get(message_id)
            .into_iter()
            .flatten()
            .filter_map(|data| {
                hashed.extend_from_slice(data.as_bytes());
                hashed.push(b'\n');
                serde_json::from_str::<Value>(data).ok()
            })
            .collect();
        let texts = |keep: &dyn Fn(&Value) -> bool| -> String {
            message_parts
                .iter()
                .filter(|p| p["type"] == "text" && p["synthetic"] != true && p["ignored"] != true)
                .filter(|p| keep(p))
                .filter_map(|p| p["text"].as_str())
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        };
        match message["role"].as_str() {
            Some("user") => {
                if message_parts.iter().any(|p| p["type"] == "compaction") {
                    in_summary = true;
                    continue;
                }
                let prompt = texts(&|_| true);
                if prompt.is_empty() {
                    continue;
                }
                in_summary = false;
                let n = out.turns.len();
                out.turns.push(Turn {
                    n,
                    at: timestamp(*at),
                    user: prompt,
                    assistant: String::new(),
                    tools: Vec::new(),
                });
            }
            Some("assistant") => {
                let text = texts(&|_| true);
                if in_summary || message["summary"] == true {
                    if !text.is_empty() {
                        out.compact.push(clip(&text, 20_000));
                    }
                    continue;
                }
                for part in &message_parts {
                    if part["type"] == "tool"
                        && let Some(tool) = part["tool"].as_str()
                    {
                        out.memory_writes.extend(memory_writes(
                            tool,
                            &part["state"]["input"],
                            &timestamp(*at),
                        ));
                        if let Some(turn) = out.turns.last_mut()
                            && !turn.tools.iter().any(|t| t == tool)
                        {
                            turn.tools.push(tool.to_string());
                        }
                        if matches!(tool, "edit" | "write" | "multiedit")
                            && let Some(file) = part["state"]["input"]["filePath"].as_str()
                            && !out.files.iter().any(|f| f == file)
                        {
                            out.files.push(file.to_string());
                        }
                    }
                    if part["type"] == "patch" {
                        for file in part["files"].as_array().into_iter().flatten() {
                            if let Some(file) = file.as_str()
                                && !out.files.iter().any(|f| f == file)
                            {
                                out.files.push(file.to_string());
                            }
                        }
                    }
                }
                if let Some(turn) = out.turns.last_mut()
                    && !text.is_empty()
                {
                    if !turn.assistant.is_empty() {
                        turn.assistant.push_str("\n\n");
                    }
                    turn.assistant.push_str(&text);
                }
            }
            _ => {}
        }
    }
    out.bytes = hashed.len();
    out.fingerprint = crate::fsutil::fingerprint(&hashed);
    out
}

/// opencode's sessions (`crate::sessions::SessionSource`): the units of its database.
pub struct Sessions;

impl crate::sessions::SessionSource for Sessions {
    fn units(&self, dir: &Path) -> Result<Vec<Unit>, String> {
        units(dir)
    }

    /// The opencode session id of `<db>#<id>`.
    fn unit_id(&self, file: &Path) -> String {
        match split_unit(file) {
            Some((_, id)) => id,
            None => file
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
        }
    }

    fn read(&self, file: &Path, _data_dir: &Path) -> Result<(Transcript, String), String> {
        let (db, id) = split_unit(file)
            .ok_or_else(|| format!("{} is not an opencode session", file.display()))?;
        Ok((read(&db, &id)?, format!("opencode:{id}")))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// The tables opencode's first migration creates (the columns read here), with a project.
    pub(crate) const SCHEMA: &str = "
        CREATE TABLE `project` (`id` text PRIMARY KEY, `worktree` text NOT NULL, `vcs` text,
          `name` text, `time_created` integer NOT NULL, `time_updated` integer NOT NULL,
          `sandboxes` text NOT NULL DEFAULT '[]');
        CREATE TABLE `session` (`id` text PRIMARY KEY, `project_id` text NOT NULL,
          `parent_id` text, `slug` text NOT NULL, `directory` text NOT NULL, `title` text NOT NULL,
          `version` text NOT NULL, `share_url` text, `summary_additions` integer,
          `summary_deletions` integer, `summary_files` integer, `summary_diffs` text,
          `revert` text, `permission` text, `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL, `time_compacting` integer, `time_archived` integer,
          CONSTRAINT `fk_session_project_id_project_id_fk` FOREIGN KEY (`project_id`)
            REFERENCES `project`(`id`) ON DELETE CASCADE);
        CREATE TABLE `message` (`id` text PRIMARY KEY, `session_id` text NOT NULL,
          `time_created` integer NOT NULL, `time_updated` integer NOT NULL, `data` text NOT NULL,
          CONSTRAINT `fk_message_session_id_session_id_fk` FOREIGN KEY (`session_id`)
            REFERENCES `session`(`id`) ON DELETE CASCADE);
        CREATE TABLE `part` (`id` text PRIMARY KEY, `message_id` text NOT NULL,
          `session_id` text NOT NULL, `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL, `data` text NOT NULL,
          CONSTRAINT `fk_part_message_id_message_id_fk` FOREIGN KEY (`message_id`)
            REFERENCES `message`(`id`) ON DELETE CASCADE);
        PRAGMA journal_mode = WAL;";

    /// A database at `db` with one session in `directory`: three turns (one with an edit, a
    /// synthetic part, reasoning and a tool output that must not be kept), a sub-agent session.
    pub(crate) fn fixture(db: &Path, directory: &str) {
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let c = Connection::open(db).unwrap();
        c.execute_batch(SCHEMA).unwrap();
        c.execute(
            "INSERT INTO project VALUES ('p1', ?1, 'git', NULL, 1, 1, '[]')",
            [directory],
        )
        .unwrap();
        for (id, parent, title) in [
            ("ses_abc", None, "Parser cache"),
            ("ses_sub", Some("ses_abc"), "Sub-agent"),
        ] {
            c.execute(
                "INSERT INTO session (id, project_id, parent_id, slug, directory, title, version, \
                 time_created, time_updated) VALUES (?1, 'p1', ?2, 'brave-otter', ?3, ?4, \
                 '1.19.5', 1767430800000, 1767434400000)",
                rusqlite::params![id, parent, directory, title],
            )
            .unwrap();
        }
        let user = |_: &str| {
            json!({"role": "user", "time": {"created": 1}, "agent": "build",
            "model": {"providerID": "opencode", "modelID": "big-pickle"}})
        };
        let assistant = json!({"role": "assistant", "time": {"created": 2, "completed": 3},
            "parentID": "x", "modelID": "big-pickle", "providerID": "opencode", "mode": "build",
            "agent": "build", "path": {"cwd": directory, "root": directory}, "cost": 0,
            "tokens": {"input": 10, "output": 5, "reasoning": 0, "cache": {"read": 0, "write": 0}}});
        let file = format!("{directory}/src/parse.rs");
        let rows: Vec<(&str, i64, Value, Vec<Value>)> = vec![
            (
                "msg_01",
                1767430800000,
                user("a"),
                vec![
                    json!({"type": "text", "text": "Add a cache to the parser."}),
                    json!({"type": "text", "text": "<file attached>", "synthetic": true}),
                ],
            ),
            (
                "msg_02",
                1767430810000,
                assistant.clone(),
                vec![
                    json!({"type": "step-start"}),
                    json!({"type": "reasoning", "text": "secret thinking"}),
                    json!({"type": "tool", "callID": "c1", "tool": "edit", "state": {"status": "completed",
                    "input": {"filePath": file, "oldString": "a", "newString": "b"},
                    "output": "test output", "title": "edit", "metadata": {}, "time": {"start": 1, "end": 2}}}),
                    json!({"type": "text", "text": "Added an LRU cache."}),
                    json!({"type": "step-finish", "reason": "stop", "cost": 0,
                    "tokens": {"input": 10, "output": 5, "reasoning": 0, "cache": {"read": 0, "write": 0}}}),
                ],
            ),
            (
                "msg_03",
                1767430900000,
                user("b"),
                vec![json!({"type": "text", "text": "Run the tests."})],
            ),
            (
                "msg_04",
                1767430910000,
                assistant.clone(),
                vec![
                    json!({"type": "tool", "callID": "c2", "tool": "bash", "state": {"status": "completed",
                    "input": {"command": "cargo test"}, "output": "test output", "title": "t",
                    "metadata": {}, "time": {"start": 1, "end": 2}}}),
                    json!({"type": "text", "text": "All tests pass."}),
                ],
            ),
            (
                "msg_05",
                1767431000000,
                user("c"),
                vec![json!({"type": "text", "text": "Thanks, keep the cache small."})],
            ),
            (
                "msg_06",
                1767431010000,
                assistant,
                vec![json!({"type": "text", "text": "Noted."})],
            ),
        ];
        for (id, at, data, parts) in rows {
            c.execute(
                "INSERT INTO message VALUES (?1, 'ses_abc', ?2, ?2, ?3)",
                rusqlite::params![id, at, data.to_string()],
            )
            .unwrap();
            for (i, part) in parts.iter().enumerate() {
                c.execute(
                    "INSERT INTO part VALUES (?1, ?2, 'ses_abc', ?3, ?3, ?4)",
                    rusqlite::params![format!("prt_{id}_{i}"), id, at, part.to_string()],
                )
                .unwrap();
            }
        }
        c.execute(
            "INSERT INTO message VALUES ('msg_s1', 'ses_sub', 1, 1, ?1)",
            [user("s").to_string()],
        )
        .unwrap();
    }

    /// Adds one more turn to the fixture session.
    pub(crate) fn grow(db: &Path) {
        let c = Connection::open(db).unwrap();
        c.execute(
            "INSERT INTO message VALUES ('msg_07', 'ses_abc', 1767440000000, 1767440000000, ?1)",
            [json!({"role": "user", "time": {"created": 7}}).to_string()],
        )
        .unwrap();
        c.execute(
            "INSERT INTO part VALUES ('prt_07', 'msg_07', 'ses_abc', 1767440000000, 1767440000000, ?1)",
            [json!({"type": "text", "text": "One more thing."}).to_string()],
        )
        .unwrap();
    }

    /// The session tables of opencode 2.x (v2.2.4 `packages/core/src/database/schema.gen.ts`,
    /// the columns as there; `project` comes from [`SCHEMA`] or [`PROJECT_V2`]).
    pub(crate) const SCHEMA_V2: &str = "
        CREATE TABLE `session_v2` (`id` text PRIMARY KEY, `project_id` text NOT NULL,
          `workspace_id` text, `parent_id` text, `fork_session_id` text, `fork_boundary` text,
          `slug` text NOT NULL, `directory` text NOT NULL, `path` text, `title` text,
          `version` text NOT NULL, `share_url` text, `summary_additions` integer,
          `summary_deletions` integer, `summary_files` integer, `summary_diffs` text,
          `metadata` text, `cost` real DEFAULT 0 NOT NULL, `tokens_input` integer DEFAULT 0 NOT NULL,
          `tokens_output` integer DEFAULT 0 NOT NULL, `tokens_reasoning` integer DEFAULT 0 NOT NULL,
          `tokens_cache_read` integer DEFAULT 0 NOT NULL,
          `tokens_cache_write` integer DEFAULT 0 NOT NULL, `revert` text, `permission` text,
          `agent` text, `model` text, `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL, `time_busy` integer, `time_idle` integer,
          `time_viewed` integer, `idle_outcome` text, `time_compacting` integer,
          `time_archived` integer, `time_suspended` integer,
          `resume_attempts` integer DEFAULT 0 NOT NULL,
          CONSTRAINT `fk_session_v2_project_id_project_id_fk` FOREIGN KEY (`project_id`)
            REFERENCES `project`(`id`) ON DELETE CASCADE);
        CREATE TABLE `session_message` (`id` text PRIMARY KEY, `session_id` text NOT NULL,
          `type` text NOT NULL, `seq` integer NOT NULL, `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL, `data` text NOT NULL,
          CONSTRAINT `fk_session_message_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`)
            REFERENCES `session_v2`(`id`) ON DELETE CASCADE);
        CREATE TABLE `session_part` (`message_id` text NOT NULL, `session_id` text NOT NULL,
          `ordinal` integer NOT NULL, `type` text NOT NULL, `call_id` text,
          `time_created` integer NOT NULL, `time_updated` integer NOT NULL, `data` text NOT NULL,
          CONSTRAINT `session_part_pk` PRIMARY KEY(`message_id`, `ordinal`));";

    /// 2.x's `project` table (the columns a row needs).
    const PROJECT_V2: &str = "
        CREATE TABLE `project` (`id` text PRIMARY KEY, `worktree` text NOT NULL, `vcs` text,
          `name` text, `time_created` integer NOT NULL, `time_updated` integer NOT NULL,
          `sandboxes` text NOT NULL DEFAULT '[]');
        PRAGMA journal_mode = WAL;";

    /// Inserts a 2.x session row.
    fn insert_v2_session(c: &Connection, id: &str, parent: Option<&str>, directory: &str) {
        c.execute(
            "INSERT INTO session_v2 (id, project_id, parent_id, slug, directory, title, version, \
             time_created, time_updated) VALUES (?1, 'p1', ?2, 'calm-fox', ?3, NULL, '2.2.4', \
             1767520800000, 1767524400000)",
            rusqlite::params![id, parent, directory],
        )
        .unwrap();
    }

    /// Inserts a 2.x message row (as 2.x stores it: no `type` or `id` in the data).
    fn insert_v2_message(
        c: &Connection,
        session: &str,
        id: &str,
        kind: &str,
        seq: i64,
        data: Value,
    ) {
        let at = 1767520800000 + seq * 1000;
        c.execute(
            "INSERT INTO session_message VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
            rusqlite::params![id, session, kind, seq, at, data.to_string()],
        )
        .unwrap();
    }

    /// Adds 2.x's tables to `db` (a new file, or a 1.x one being upgraded) with one session
    /// `ses_v2a` in `directory`: a turn whose assistant content is inline (reasoning, an edit
    /// with a relative path, text), a turn whose content is in `session_part` (a patch),
    /// a compaction, a model switch, a turn, and a sub-agent session.
    pub(crate) fn fixture_v2(db: &Path, directory: &str) {
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let c = Connection::open(db).unwrap();
        let has_project = c
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'project'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
            > 0;
        if !has_project {
            c.execute_batch(PROJECT_V2).unwrap();
            c.execute(
                "INSERT INTO project VALUES ('p1', ?1, 'git', NULL, 1, 1, '[]')",
                [directory],
            )
            .unwrap();
        }
        c.execute_batch(SCHEMA_V2).unwrap();
        insert_v2_session(&c, "ses_v2a", None, directory);
        insert_v2_session(&c, "ses_v2sub", Some("ses_v2a"), directory);
        let model = json!({"id": "big-pickle", "providerID": "opencode", "variant": "default"});
        let user =
            |text: &str, at: i64| json!({"time": {"created": at}, "text": text, "files": []});
        insert_v2_message(
            &c,
            "ses_v2a",
            "msg_10",
            "user",
            4,
            user("Rename the config flag.", 1),
        );
        insert_v2_message(
            &c,
            "ses_v2a",
            "msg_11",
            "assistant",
            5,
            json!({
            "time": {"created": 2, "completed": 3}, "agent": "build", "model": model,
            "content": [
                {"type": "reasoning", "text": "secret thinking"},
                {"type": "tool", "id": "c1", "name": "edit", "time": {"created": 2},
                 "state": {"status": "completed",
                   "input": {"path": "src/flags.rs", "oldString": "a", "newString": "b"},
                   "content": [{"type": "text", "text": "test output"}]}},
                {"type": "text", "text": "Renamed it."}],
            "finish": "stop", "cost": 0,
            "tokens": {"input": 10, "output": 5, "reasoning": 0, "cache": {"read": 0, "write": 0}}}),
        );
        insert_v2_message(
            &c,
            "ses_v2a",
            "msg_12",
            "idle",
            6,
            json!({"time": {"created": 4}, "outcome": "succeeded"}),
        );
        insert_v2_message(
            &c,
            "ses_v2a",
            "msg_13",
            "user",
            7,
            user("Apply the patch too.", 5),
        );
        insert_v2_message(
            &c,
            "ses_v2a",
            "msg_14",
            "assistant",
            8,
            json!({
            "time": {"created": 6}, "agent": "build", "model": model, "content": []}),
        );
        for (ordinal, part) in [
            json!({"type": "tool", "id": "c2", "name": "patch", "time": {"created": 6},
                   "state": {"status": "completed", "input": {"patchText": "secret patch"},
                     "content": [{"type": "text", "text": "test output"}],
                     "metadata": {"files": [{"file": "/elsewhere/x.rs", "patch": "", "additions": 1,
                       "deletions": 0, "status": "modified"}]}}}),
            json!({"type": "text", "text": "Patched."}),
        ]
        .iter()
        .enumerate()
        {
            c.execute(
                "INSERT INTO session_part VALUES ('msg_14', 'ses_v2a', ?1, ?2, NULL, 1767520808000, \
                 1767520808000, ?3)",
                rusqlite::params![ordinal as i64, part["type"].as_str().unwrap(), part.to_string()],
            )
            .unwrap();
        }
        insert_v2_message(
            &c,
            "ses_v2a",
            "msg_15",
            "compaction",
            9,
            json!({
            "time": {"created": 7}, "status": "completed", "reason": "auto",
            "summary": "Earlier: the flag was renamed and a patch applied."}),
        );
        insert_v2_message(
            &c,
            "ses_v2a",
            "msg_16",
            "model-switched",
            10,
            json!({"time": {"created": 8}, "model": model}),
        );
        insert_v2_message(
            &c,
            "ses_v2a",
            "msg_17",
            "user",
            11,
            user("Thanks, done.", 9),
        );
        insert_v2_message(
            &c,
            "ses_v2sub",
            "msg_s2",
            "user",
            1,
            user("Sub-agent task", 1),
        );
    }

    #[test]
    fn a_2x_session_becomes_turns_from_inline_content_and_parts() {
        let tmp = crate::mcp_tests::TempDir::new("oc2-sess");
        let db = tmp.path().join("opencode.db");
        fixture_v2(&db, "/w/app");
        assert!(!tables(&db).unwrap().contains(&"session".to_string()));
        let units = units(&db).unwrap();
        assert_eq!(units.len(), 1, "the sub-agent session is left out");
        assert_eq!(units[0].path, unit_path(&db, "ses_v2a"));
        let t = read(&db, "ses_v2a").unwrap();
        assert_eq!(t.title, "", "2.x titles may be empty");
        assert_eq!(t.cwd.as_deref(), Some("/w/app"));
        assert_eq!(t.started_at, "2026-01-04T10:00:00.000Z");
        let users: Vec<&str> = t.turns.iter().map(|turn| turn.user.as_str()).collect();
        assert_eq!(
            users,
            vec![
                "Rename the config flag.",
                "Apply the patch too.",
                "Thanks, done."
            ]
        );
        assert_eq!(t.turns[0].assistant, "Renamed it.");
        assert_eq!(t.turns[0].tools, vec!["edit"]);
        assert_eq!(t.turns[1].assistant, "Patched.");
        assert_eq!(t.turns[1].tools, vec!["patch"]);
        assert_eq!(t.files, vec!["/w/app/src/flags.rs", "/elsewhere/x.rs"]);
        assert_eq!(
            t.compact,
            vec!["Earlier: the flag was renamed and a patch applied."]
        );
        let all = format!("{t:?}");
        assert!(
            !all.contains("secret") && !all.contains("test output"),
            "{all}"
        );
        // A part that changes changes the fingerprint and the unit.
        let before = (t.fingerprint.clone(), units[0].size);
        let c = Connection::open(&db).unwrap();
        c.execute(
            "UPDATE session_part SET data = ?1, time_updated = 1767530000000 WHERE message_id = \
             'msg_14' AND ordinal = 1",
            [json!({"type": "text", "text": "Patched, and tested."}).to_string()],
        )
        .unwrap();
        let after = read(&db, "ses_v2a").unwrap();
        assert_eq!(after.turns[1].assistant, "Patched, and tested.");
        assert_ne!(after.fingerprint, before.0);
        let grown = super::units(&db).unwrap();
        assert!(grown[0].size > before.1);
        assert_eq!(grown[0].modified, 1767530000);
        assert_eq!(of_project(&db, Path::new("/w/app")).unwrap().len(), 1);
    }

    #[test]
    fn after_an_upgrade_each_session_is_read_once_from_the_right_tables() {
        let tmp = crate::mcp_tests::TempDir::new("oc12-sess");
        let db = tmp.path().join("opencode.db");
        fixture(&db, "/w/app");
        let v1_only = read(&db, "ses_abc").unwrap();
        // 2.x upgrades the file: its tables appear, the 1.x session is copied with its id and
        // time, the 1.x rows stay; a new session is made in 2.x.
        fixture_v2(&db, "/w/app");
        let c = Connection::open(&db).unwrap();
        c.execute(
            "INSERT INTO session_v2 (id, project_id, slug, directory, title, version, \
             time_created, time_updated) SELECT id, project_id, slug, directory, title, version, \
             time_created, time_updated FROM session WHERE id = 'ses_abc'",
            [],
        )
        .unwrap();
        insert_v2_message(
            &c,
            "ses_abc",
            "msg_m1",
            "user",
            1,
            json!({"time": {"created": 1}, "text": "Add a cache to the parser.", "files": []}),
        );
        let ids: Vec<PathBuf> = units(&db).unwrap().into_iter().map(|u| u.path).collect();
        assert_eq!(
            ids,
            vec![unit_path(&db, "ses_abc"), unit_path(&db, "ses_v2a")],
            "no duplicate for the copied session"
        );
        let migrated = read(&db, "ses_abc").unwrap();
        assert_eq!(
            migrated.fingerprint, v1_only.fingerprint,
            "a copy 2.x did not change is read from 1.x: nothing to index again"
        );
        assert_eq!(migrated.turns.len(), 3);
        assert_eq!(read(&db, "ses_v2a").unwrap().turns.len(), 3);
        assert_eq!(of_project(&db, Path::new("/w/app")).unwrap().len(), 2);
        // The session is continued in 2.x: read from 2.x's tables from now on.
        c.execute(
            "UPDATE session_v2 SET time_updated = 1767600000000 WHERE id = 'ses_abc'",
            [],
        )
        .unwrap();
        insert_v2_message(
            &c,
            "ses_abc",
            "msg_m2",
            "user",
            2,
            json!({"time": {"created": 2}, "text": "Continued in 2.x.", "files": []}),
        );
        let continued = read(&db, "ses_abc").unwrap();
        assert_eq!(
            continued
                .turns
                .iter()
                .map(|t| t.user.as_str())
                .collect::<Vec<_>>(),
            vec!["Add a cache to the parser.", "Continued in 2.x."]
        );
        assert_ne!(continued.fingerprint, v1_only.fingerprint);
        let unit = units(&db).unwrap().into_iter().next().unwrap();
        assert_eq!(unit.modified, 1767600000);
    }

    #[test]
    fn a_session_becomes_turns_without_sub_agents_reasoning_or_tool_output() {
        let tmp = crate::mcp_tests::TempDir::new("oc-sess");
        let db = tmp.path().join("opencode.db");
        fixture(&db, "/w/app");
        let units = units(&db).unwrap();
        assert_eq!(units.len(), 1, "the sub-agent session is left out");
        assert_eq!(units[0].path, unit_path(&db, "ses_abc"));
        assert_eq!(units[0].modified, 1767434400);
        assert_eq!(
            split_unit(&units[0].path),
            Some((db.clone(), "ses_abc".to_string()))
        );
        let t = read(&db, "ses_abc").unwrap();
        assert_eq!(t.title, "Parser cache");
        assert_eq!(t.cwd.as_deref(), Some("/w/app"));
        assert_eq!(t.started_at, "2026-01-03T09:00:00.000Z");
        assert_eq!(t.turns.len(), 3);
        assert_eq!(t.turns[0].user, "Add a cache to the parser.");
        assert_eq!(t.turns[0].assistant, "Added an LRU cache.");
        assert_eq!(t.turns[0].tools, vec!["edit"]);
        assert_eq!(t.turns[1].tools, vec!["bash"]);
        assert_eq!(t.files, vec!["/w/app/src/parse.rs"]);
        let all = format!("{t:?}");
        assert!(!all.contains("secret thinking") && !all.contains("test output"));
        assert!(!all.contains("<file attached>"));
        // Growing changes the size, the fingerprint and the turns.
        let before = t.fingerprint.clone();
        grow(&db);
        let grown = read(&db, "ses_abc").unwrap();
        assert_eq!(grown.turns.len(), 4);
        assert_ne!(grown.fingerprint, before);
        assert!(super::units(&db).unwrap()[0].size > units[0].size);
        // No database: no sessions; a transcript path is not a unit.
        assert!(
            super::units(&tmp.path().join("none.db"))
                .unwrap()
                .is_empty()
        );
        assert!(split_unit(Path::new("/x/abc.jsonl")).is_none());
        assert_eq!(of_project(&db, Path::new("/w/app")).unwrap().len(), 1);
        assert!(of_project(&db, Path::new("/w/other")).unwrap().is_empty());
    }

    #[test]
    fn opencode_sessions_of_both_lines_record_the_memories_they_wrote() {
        let tmp = crate::mcp_tests::TempDir::new("oc-mem");
        // 1.x: a tool part of msg_04 (its tool name is `<server>_<tool>`).
        let db = tmp.path().join("v1").join("opencode.db");
        fixture(&db, "/w/app");
        let part = json!({"type": "tool", "callID": "c9", "tool": "graphersal-memory_remember",
            "state": {"status": "completed", "input": {"slug": "lru-cache", "summary": "LRU"},
              "output": "saved", "title": "remember", "metadata": {}, "time": {"start": 1, "end": 2}}});
        Connection::open(&db)
            .unwrap()
            .execute(
                "INSERT INTO part VALUES ('prt_mem', 'msg_04', 'ses_abc', 1767430911000, \
                 1767430911000, ?1)",
                [part.to_string()],
            )
            .unwrap();
        let t = read(&db, "ses_abc").unwrap();
        let got: Vec<(&str, &str)> = t
            .memory_writes
            .iter()
            .map(|w| (w.action.as_str(), w.slug.as_str()))
            .collect();
        assert_eq!(got, vec![("remember", "lru-cache")]);
        assert!(
            t.memory_writes[0].at.starts_with("2026-01-03"),
            "{:?}",
            t.memory_writes
        );

        // 2.x: a tool part of msg_14 in `session_part` (named by `name`).
        let db = tmp.path().join("v2").join("opencode.db");
        fixture_v2(&db, "/w/app");
        let part = json!({"type": "tool", "id": "c9", "name": "graphersal-memory_pin",
            "time": {"created": 6}, "state": {"status": "completed",
              "input": {"slug": "flags", "pinned": true}, "content": []}});
        Connection::open(&db)
            .unwrap()
            .execute(
                "INSERT INTO session_part VALUES ('msg_14', 'ses_v2a', 2, 'tool', NULL, \
                 1767520808000, 1767520808000, ?1)",
                [part.to_string()],
            )
            .unwrap();
        let t = read(&db, "ses_v2a").unwrap();
        let got: Vec<(&str, &str)> = t
            .memory_writes
            .iter()
            .map(|w| (w.action.as_str(), w.slug.as_str()))
            .collect();
        assert_eq!(got, vec![("pin", "flags")]);
    }
}
