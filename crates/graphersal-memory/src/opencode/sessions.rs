//! opencode's stored sessions -> [`Transcript`]s, read from its SQLite database (opencode 1.x:
//! `opencode.db`, tables `session`, `message` and `part`, `packages/core/src/session/sql.ts`;
//! the columns read here exist since its first migration).
//!
//! - **Sessions**: the rows of `session` without `parent_id` (sub-agent sessions are left out,
//!   as Claude Code's sidechains are). Working directory `directory`, title `title`, times
//!   `time_created`/`time_updated` (milliseconds).
//! - **Turns**: a user message's `text` parts (not `synthetic` or `ignored`) start a turn; the
//!   assistant messages up to the next prompt add their `text` parts, the names of their `tool`
//!   parts, and the files their `edit`/`write` tools (`state.input.filePath`) and `patch` parts
//!   (`files`) changed. Reasoning, tool input and tool output are not kept. A `compaction`
//!   part's summary (the next assistant text of a `summary` message) is kept as a compaction
//!   summary.
//! - **Change detection**: a session's unit size is the bytes of its message and part data, its
//!   modification time the newest of the session's and its parts' `time_updated`; the
//!   fingerprint covers exactly the data the transcript is built from, in order.
//! - The database is opened read-only (opencode keeps it in WAL mode and may write while it is
//!   read); a database that does not exist yet means no sessions.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;

use crate::sessions::Unit;
use crate::transcript::{Transcript, Turn, clip};

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

/// The top-level sessions of `db` as indexing units (none when the database does not exist).
pub fn units(db: &Path) -> Result<Vec<Unit>, String> {
    if !db.is_file() {
        return Ok(Vec::new());
    }
    let connection = open(db)?;
    let mut statement = connection
        .prepare(
            "SELECT s.id, s.time_updated,
                    COALESCE((SELECT SUM(LENGTH(m.data)) FROM message m WHERE m.session_id = s.id), 0)
                  + COALESCE((SELECT SUM(LENGTH(p.data)) FROM part p WHERE p.session_id = s.id), 0),
                    COALESCE((SELECT MAX(p.time_updated) FROM part p WHERE p.session_id = s.id), 0)
             FROM session s WHERE s.parent_id IS NULL ORDER BY s.id",
        )
        .map_err(sql_error(db))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(sql_error(db))?;
    let mut out = Vec::new();
    for row in rows {
        let (id, updated, size, part_updated) = row.map_err(sql_error(db))?;
        out.push(Unit {
            path: unit_path(db, &id),
            size: u64::try_from(size).unwrap_or(0),
            modified: u64::try_from(updated.max(part_updated) / 1000).unwrap_or(0),
        });
    }
    Ok(out)
}

/// The session ids of `db` whose working directory is `directory` (or below it), newest first:
/// the sessions of one project (the bench's question source).
pub fn of_project(db: &Path, directory: &Path) -> Result<Vec<(String, u64)>, String> {
    if !db.is_file() {
        return Ok(Vec::new());
    }
    let connection = open(db)?;
    let mut statement = connection
        .prepare(
            "SELECT id, directory, time_updated FROM session WHERE parent_id IS NULL \
             ORDER BY time_updated DESC, id",
        )
        .map_err(sql_error(db))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(sql_error(db))?;
    let mut out = Vec::new();
    for row in rows {
        let (id, dir, updated) = row.map_err(sql_error(db))?;
        if Path::new(&dir).starts_with(directory) {
            out.push((id, u64::try_from(updated / 1000).unwrap_or(0)));
        }
    }
    Ok(out)
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

/// Reads session `id` of `db` into a transcript.
pub fn read(db: &Path, id: &str) -> Result<Transcript, String> {
    let connection = open(db)?;
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
    let mut by_message: std::collections::BTreeMap<String, Vec<String>> = Default::default();
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

/// The transcript of one session's rows.
fn build(
    id: &str,
    directory: &str,
    title: &str,
    created: i64,
    updated: i64,
    messages: &[(String, i64, String)],
    parts: &std::collections::BTreeMap<String, Vec<String>>,
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
}
