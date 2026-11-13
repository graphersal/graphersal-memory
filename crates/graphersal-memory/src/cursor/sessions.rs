//! Cursor's sessions: the transcripts its hooks report, read into the [`Transcript`] the
//! indexing writes (never changed).
//!
//! Cursor's chat history lives in an internal database whose format is not documented and
//! changes between versions; it is not read. Every agent hook gets `transcript_path`, the
//! conversation's transcript file (cursor.com/docs/hooks, "Common schema"; null when
//! transcripts are off), and `workspace_roots`. Our `sessionStart`/`sessionEnd` hooks record
//! each transcript with its workspace in the registry `<data dir>/sessions/
//! cursor-transcripts.json` ([`record`]): `{"version": 1, "transcripts": {"<path>":
//! {"project", "session", "first_seen", "last_seen"}}}`. Only registered transcripts are
//! indexed ([`units`]), so a session that ran before install, or with the hooks off, is not.
//!
//! The transcript is JSON Lines; its line format is not documented. What is recognised: a line
//! with a role (`role`, else `type`, else `message.role`) `user` or `assistant` and a
//! `message.content` (or `content`) that is a string or a list of parts; `text` parts are the
//! words, `tool_use` parts name a tool. A user message's text inside `<user_query>` tags is the
//! prompt (Cursor puts its context around it); a user message of tool results only is not a
//! turn. `timestamp` and `cwd`, when present, are taken. Everything else is skipped; a
//! transcript with nothing recognisable has no turns and counts as empty. Sub-agent transcripts
//! (`.../subagents/...`) and files that are not `.jsonl` are not taken.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::sessions::Unit;
use crate::transcript::{CAP, Transcript, Turn, clip, memory_writes};

/// The registry's format version.
const FORMAT: u64 = 1;

/// How long [`record`] waits for another hook's write.
const LOCK_WAIT: Duration = Duration::from_secs(5);

/// One registered transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The workspace directory the hook named (the session's project directory).
    pub project: String,
    /// Cursor's session (conversation) id.
    pub session: String,
}

/// Whether `path` is a transcript the indexing takes: absolute, `.jsonl`, not a sub-agent's.
pub fn is_transcript(path: &Path) -> bool {
    path.is_absolute()
        && path.extension().is_some_and(|ext| ext == "jsonl")
        && !path.components().any(|c| c.as_os_str() == "subagents")
}

/// The registry's transcripts.
pub fn load(registry: &Path) -> Result<BTreeMap<PathBuf, Entry>, String> {
    let Some(bytes) = crate::fsutil::read_optional(registry)? else {
        return Ok(BTreeMap::new());
    };
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|err| format!("{} is invalid: {err}", registry.display()))?;
    if value["version"].as_u64() != Some(FORMAT) {
        return Err(format!("{}: unsupported version", registry.display()));
    }
    Ok(value["transcripts"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(path, entry)| {
            Some((
                PathBuf::from(path),
                Entry {
                    project: entry["project"].as_str()?.to_string(),
                    session: entry["session"].as_str().unwrap_or_default().to_string(),
                },
            ))
        })
        .collect())
}

/// Records `transcript` of the session `session` in the workspace `project` (a hook's input),
/// under a lock so hooks running at once do not lose each other's entries. A transcript that
/// is not one the indexing takes ([`is_transcript`]) is not recorded (`Ok(false)`).
pub fn record(
    registry: &Path,
    transcript: &Path,
    project: &str,
    session: &str,
) -> Result<bool, String> {
    if !is_transcript(transcript) || project.is_empty() {
        return Ok(false);
    }
    let dir = registry
        .parent()
        .ok_or_else(|| format!("{} has no directory", registry.display()))?;
    std::fs::create_dir_all(dir)
        .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    let name = format!(
        "{}.lock",
        registry
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    );
    let mut lock = crate::ipc::LockFile::open(dir, &name)?;
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        if let Some(_guard) = lock.try_lock()? {
            return write_entry(registry, transcript, project, session).map(|()| true);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{} is locked by another hook; the transcript is recorded at the next hook",
                registry.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn write_entry(
    registry: &Path,
    transcript: &Path,
    project: &str,
    session: &str,
) -> Result<(), String> {
    let mut value = match crate::fsutil::read_optional(registry)? {
        Some(bytes) => serde_json::from_slice::<Value>(&bytes)
            .ok()
            .filter(|v| v["version"].as_u64() == Some(FORMAT))
            .unwrap_or_else(|| json!({"version": FORMAT, "transcripts": {}})),
        None => json!({"version": FORMAT, "transcripts": {}}),
    };
    if !value["transcripts"].is_object() {
        value["transcripts"] = json!({});
    }
    let now = crate::daemon::now_utc();
    let key = transcript.display().to_string();
    let slot = &mut value["transcripts"][&key];
    let first_seen = slot["first_seen"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| now.clone());
    *slot = json!({"project": project, "session": session, "first_seen": first_seen,
                   "last_seen": now});
    crate::fsutil::write_atomic(
        registry,
        format!("{}\n", crate::jsonedit::render(&value, Some("  "), 0)).as_bytes(),
    )
}

/// The registered transcripts that exist, as units (size and modification time).
pub fn units(registry: &Path) -> Result<Vec<Unit>, String> {
    Ok(load(registry)?
        .into_keys()
        .filter(|path| is_transcript(path))
        .filter_map(|path| {
            let (size, modified) = crate::sessions::stamp(&path)?;
            Some(Unit {
                path,
                size,
                modified,
            })
        })
        .collect())
}

/// The workspace the hooks recorded for `transcript`, when it is registered.
pub fn project_of(registry: &Path, transcript: &Path) -> Option<String> {
    load(registry)
        .ok()?
        .remove(transcript)
        .map(|entry| entry.project)
}

/// The session id of a transcript: its file stem (`<id>.jsonl`).
pub fn session_id(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Reads a transcript of the workspace `project`.
pub fn read(path: &Path, project: &str) -> Result<Transcript, String> {
    let bytes =
        std::fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    Ok(parse(&bytes, &session_id(path), project))
}

/// The text of a message's content (a string, or the `text` of its text parts), and the names
/// of the tools it uses, and whether it holds anything but tool results.
fn content_parts(content: &Value) -> (String, Vec<String>, bool) {
    match content {
        Value::String(text) => (text.clone(), Vec::new(), true),
        Value::Array(parts) => {
            let mut texts = Vec::new();
            let mut tools = Vec::new();
            let mut other = false;
            for part in parts {
                match part["type"].as_str().unwrap_or_default() {
                    "text" => {
                        other = true;
                        if let Some(text) = part["text"].as_str() {
                            texts.push(text);
                        }
                    }
                    "tool_use" => {
                        other = true;
                        if let Some(name) = part["name"].as_str().filter(|n| !n.is_empty()) {
                            tools.push(name.to_string());
                        }
                    }
                    "tool_result" => {}
                    _ => other = true,
                }
            }
            (texts.join("\n"), tools, other)
        }
        _ => (String::new(), Vec::new(), false),
    }
}

/// The prompt in a user message's text: what is inside `<user_query>` tags when there are
/// any, else the whole text.
pub fn user_query(text: &str) -> String {
    const OPEN: &str = "<user_query>";
    const CLOSE: &str = "</user_query>";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        let after = &rest[start + OPEN.len()..];
        let end = after.find(CLOSE).unwrap_or(after.len());
        out.push(after[..end].trim());
        rest = after.get(end + CLOSE.len()..).unwrap_or_default();
    }
    if out.is_empty() {
        text.trim().to_string()
    } else {
        out.join("\n\n")
    }
}

/// Parses a transcript's bytes; `fallback_id` is the session id, `project` the workspace the
/// hooks recorded (the working directory when no line names one).
pub fn parse(bytes: &[u8], fallback_id: &str, project: &str) -> Transcript {
    let complete = match bytes.iter().rposition(|byte| *byte == b'\n') {
        Some(end) => &bytes[..=end],
        None => &bytes[..0],
    };
    let mut out = Transcript {
        session_id: fallback_id.to_string(),
        bytes: complete.len(),
        fingerprint: crate::fsutil::fingerprint(complete),
        partial_tail: complete.len() < bytes.len(),
        ..Transcript::default()
    };
    if out.partial_tail {
        out.skipped_lines += 1;
    }
    for line in complete.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(record @ Value::Object(_)) = serde_json::from_slice::<Value>(line) else {
            out.skipped_lines += 1;
            continue;
        };
        out.records += 1;
        let at = record["timestamp"].as_str().unwrap_or_default();
        if !at.is_empty() {
            if out.started_at.is_empty() {
                out.started_at = at.to_string();
            }
            out.ended_at = at.to_string();
        }
        if out.cwd.is_none()
            && let Some(cwd) = record["cwd"].as_str().filter(|c| !c.is_empty())
        {
            out.cwd = Some(cwd.to_string());
        }
        let role = record["role"]
            .as_str()
            .or_else(|| record["type"].as_str())
            .or_else(|| record["message"]["role"].as_str())
            .unwrap_or_default();
        let content = if record["message"]["content"].is_null() {
            &record["content"]
        } else {
            &record["message"]["content"]
        };
        let (text, tools, meaningful) = content_parts(content);
        match role {
            "user" if meaningful => {
                let prompt = user_query(&text);
                if prompt.is_empty() {
                    continue;
                }
                if out.title.is_empty() {
                    out.title = clip(prompt.lines().next().unwrap_or_default(), 120);
                }
                let n = out.turns.len();
                out.turns.push(Turn {
                    n,
                    at: at.to_string(),
                    user: clip(&prompt, CAP),
                    assistant: String::new(),
                    tools: Vec::new(),
                });
            }
            "assistant" => {
                // The memories its calls of our memory tools name (bare tool names here).
                for part in content.as_array().into_iter().flatten() {
                    if part["type"] == "tool_use"
                        && let Some(name) = part["name"].as_str()
                    {
                        out.memory_writes
                            .extend(memory_writes(name, &part["input"], at));
                    }
                }
                let Some(turn) = out.turns.last_mut() else {
                    continue;
                };
                let text = text.trim();
                if !text.is_empty() {
                    if !turn.assistant.is_empty() {
                        turn.assistant.push_str("\n\n");
                    }
                    turn.assistant.push_str(text);
                    turn.assistant = clip(&turn.assistant, CAP);
                }
                for tool in tools {
                    if !turn.tools.contains(&tool) {
                        turn.tools.push(tool);
                    }
                }
            }
            _ => {}
        }
    }
    if out.cwd.is_none() && !project.is_empty() {
        out.cwd = Some(project.to_string());
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A transcript in the shape Cursor's hooks point at (role + message.content parts, the
    /// prompt inside `<user_query>`; hand-built: the format is not documented).
    pub(crate) fn fixture() -> String {
        [
            json!({"role": "user", "message": {"content": [{"type": "text", "text":
                "<user_info>OS: darwin</user_info>\n<user_query>\nUse tabs in this repository, never spaces.\n</user_query>"}]}}),
            json!({"role": "assistant", "message": {"content": [
                {"type": "text", "text": "I'll remember that."},
                {"type": "tool_use", "id": "t1", "name": "remember", "input": {"slug": "tabs"}}]}}),
            json!({"role": "user", "message": {"content": [{"type": "tool_result",
                "tool_use_id": "t1", "content": "saved"}]}}),
            json!({"role": "assistant", "message": {"content": [{"type": "text",
                "text": "Noted: tabs, never spaces."}]}}),
            json!({"role": "user", "message": {"content": [{"type": "text",
                "text": "<user_query>Fix the parser.</user_query>"}]}}),
            json!({"role": "assistant", "message": {"content": [
                {"type": "tool_use", "id": "t2", "name": "Write", "input": {}},
                {"type": "text", "text": "Fixed the off-by-one."}]}}),
            json!({"type": "turn_ended", "status": "completed"}),
        ]
        .iter()
        .map(|v| format!("{v}\n"))
        .collect()
    }

    /// Writes the fixture as `<root>/agent-transcripts/<id>/<id>.jsonl` and returns its path.
    pub(crate) fn write_fixture(root: &Path, id: &str) -> PathBuf {
        let dir = root.join("agent-transcripts").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{id}.jsonl"));
        std::fs::write(&path, fixture()).unwrap();
        path
    }

    #[test]
    fn a_transcript_becomes_turns_with_tools() {
        let t = parse(fixture().as_bytes(), "c1", "/w/app");
        assert_eq!(t.session_id, "c1");
        assert_eq!(t.cwd.as_deref(), Some("/w/app"));
        assert_eq!(t.turns.len(), 2, "{t:?}");
        assert_eq!(
            t.turns[0].user,
            "Use tabs in this repository, never spaces."
        );
        assert_eq!(
            t.turns[0].assistant,
            "I'll remember that.\n\nNoted: tabs, never spaces."
        );
        assert_eq!(t.turns[0].tools, vec!["remember"]);
        assert_eq!(t.turns[1].user, "Fix the parser.");
        assert_eq!(t.turns[1].tools, vec!["Write"]);
        assert_eq!(t.title, "Use tabs in this repository, never spaces.");
        assert!(!t.partial_tail);
        assert_eq!(t.records, 7);
    }

    #[test]
    fn claude_code_shaped_lines_strings_bad_lines_and_a_partial_tail() {
        let text = [
            json!({"type": "user", "timestamp": "t1", "cwd": "/w/real",
                   "message": {"role": "user", "content": "Hello"}}),
            json!({"type": "assistant", "timestamp": "t2",
                   "message": {"role": "assistant", "content": [{"type": "text", "text": "Hi"}]}}),
        ]
        .iter()
        .map(|v| format!("{v}\n"))
        .collect::<String>();
        let with_junk = format!("{text}not json\n{{\"role\": \"assi");
        let t = parse(with_junk.as_bytes(), "x", "/w/hook");
        assert_eq!(t.turns.len(), 1);
        assert_eq!(
            (t.turns[0].user.as_str(), t.turns[0].assistant.as_str()),
            ("Hello", "Hi")
        );
        assert_eq!(t.cwd.as_deref(), Some("/w/real"), "the line's cwd wins");
        assert_eq!((t.started_at.as_str(), t.ended_at.as_str()), ("t1", "t2"));
        assert!(t.partial_tail);
        assert_eq!(t.skipped_lines, 2);
        let complete = format!("{text}not json\n");
        assert_eq!(
            t.fingerprint,
            crate::fsutil::fingerprint(complete.as_bytes())
        );
        // Nothing recognisable: no turns (counted as empty by the indexing).
        assert!(parse(b"{\"event\": \"x\"}\n", "x", "/w").turns.is_empty());
    }

    #[test]
    fn the_prompt_is_inside_the_user_query_tags() {
        assert_eq!(user_query("plain"), "plain");
        assert_eq!(
            user_query("<ctx>a</ctx><user_query> one </user_query>x<user_query>two"),
            "one\n\ntwo"
        );
    }

    #[test]
    fn the_registry_records_transcripts_from_hooks() {
        let tmp = crate::mcp_tests::TempDir::new("cursor-registry");
        let registry = tmp
            .path()
            .join("data/sessions")
            .join(crate::cursor::REGISTRY_FILE);
        let transcript = write_fixture(&tmp.path().join("cursor-project"), "c1");
        assert!(load(&registry).unwrap().is_empty());
        assert!(units(&registry).unwrap().is_empty());
        assert!(record(&registry, &transcript, "/w/app", "c1").unwrap());
        // Again (the end hook): one entry, first_seen kept.
        let first = std::fs::read_to_string(&registry).unwrap();
        assert!(record(&registry, &transcript, "/w/app", "c1").unwrap());
        let value: Value =
            serde_json::from_str(&std::fs::read_to_string(&registry).unwrap()).unwrap();
        let before: Value = serde_json::from_str(&first).unwrap();
        let key = transcript.display().to_string();
        assert_eq!(
            value["transcripts"][&key]["first_seen"],
            before["transcripts"][&key]["first_seen"]
        );
        assert_eq!(value["transcripts"].as_object().unwrap().len(), 1);
        // Not taken: a sub-agent's transcript, a text file, a relative path, no workspace.
        let sub = transcript.parent().unwrap().join("subagents/s1.jsonl");
        assert!(!record(&registry, &sub, "/w/app", "s1").unwrap());
        assert!(!record(&registry, &transcript.with_extension("txt"), "/w/app", "c").unwrap());
        assert!(!record(&registry, Path::new("c1.jsonl"), "/w/app", "c").unwrap());
        assert!(!record(&registry, &transcript, "", "c1").unwrap());
        // A registered file that is gone is no unit.
        assert!(record(&registry, &tmp.path().join("gone/g.jsonl"), "/w/app", "g").unwrap());
        let units = units(&registry).unwrap();
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].path, transcript);
        assert_eq!(
            project_of(&registry, &transcript).as_deref(),
            Some("/w/app")
        );
        assert_eq!(session_id(&transcript), "c1");
        assert_eq!(read(&transcript, "/w/app").unwrap().turns.len(), 2);
    }

    #[test]
    fn a_cursor_transcript_records_the_memories_it_wrote() {
        let t = parse(fixture().as_bytes(), "x", "/w/app");
        let got: Vec<(&str, &str)> = t
            .memory_writes
            .iter()
            .map(|w| (w.action.as_str(), w.slug.as_str()))
            .collect();
        // Cursor records bare tool names: `remember` with a slug is ours; `Write` is not.
        assert_eq!(got, vec![("remember", "tabs")]);
    }
}
