//! Codex's sessions: one JSON Lines rollout file per session,
//! `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<timestamp>-<thread id>.jsonl`, read into the
//! [`Transcript`] the indexing writes (never changed).
//!
//! A rollout line is `{"timestamp", "type", "payload"}` (`codex-rs/history/src/
//! rollout_payload.rs`). Read here:
//!
//! - `session_meta`: the thread `id`, `cwd`, `source` (a session of a sub-agent, whose `source`
//!   is an object `{"subagent": ...}`, is left out like Claude Code's sidechains) and `git`
//!   (`branch`);
//! - `event_msg` `user_message` (`message`) starts a turn, `agent_message` (`message`) is the
//!   assistant's text: what Codex persists in its default history mode (`legacy`,
//!   `codex-rs/rollout/src/policy.rs`); in the `paginated` mode the same comes as
//!   `event_msg` `item_completed` with an `item` of type `UserMessage`/`AgentMessage`;
//! - `response_item` `function_call`/`custom_tool_call` (`name`): the tools of a turn, and the
//!   slugs of the memories a call of our memory tools names (`arguments`/`input`, also from
//!   `event_msg` `mcp_tool_call_end`'s `invocation`; `crate::transcript::memory_writes`);
//! - `event_msg` `patch_apply_end` (`changes`: path -> change): files edited;
//! - `compacted` (`message`) and `event_msg` `context_compacted`: Codex's compaction summaries;
//! - `turn_context` (`cwd`): the working directory when no `session_meta` names one.
//!
//! The injected instructions and environment context (`response_item` messages of role
//! `user`/`developer`) are not the user's words and are not read. Unknown records, bad lines and
//! a partial last line are skipped; the fingerprint covers the complete lines.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::sessions::Unit;
use crate::transcript::{CAP, Transcript, Turn, clip, memory_writes};

/// Whether `path` is a Codex rollout file (`rollout-*.jsonl`).
pub fn is_rollout(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
}

/// The thread id in a rollout's file name (`rollout-2026-11-10T12-00-00-<id>.jsonl`; a reverted
/// thread appends `_<rollout id>`), else the file stem.
pub fn thread_id(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let core = stem.strip_prefix("rollout-").unwrap_or(&stem);
    // `YYYY-MM-DDTHH-MM-SS-` is 20 characters.
    match core.get(20..).filter(|_| core.get(19..20) == Some("-")) {
        Some(ids) => ids.split('_').next().unwrap_or(ids).to_string(),
        None => stem,
    }
}

/// The rollout files under `sessions` (three directory levels, year/month/day), sorted.
pub fn scan(sessions: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![(sessions.to_path_buf(), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for path in entries.filter_map(Result::ok).map(|e| e.path()) {
            if path.is_dir() && depth < 3 {
                dirs.push((path, depth + 1));
            } else if path.is_file() && is_rollout(&path) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// The rollouts of `sessions` as units (size and modification time).
pub fn units(sessions: &Path) -> Vec<Unit> {
    scan(sessions)
        .into_iter()
        .filter_map(|path| {
            let (size, modified) = crate::sessions::stamp(&path)?;
            Some(Unit {
                path,
                size,
                modified,
            })
        })
        .collect()
}

/// Reads a rollout file.
pub fn read(path: &Path) -> Result<Transcript, String> {
    let bytes =
        std::fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    Ok(parse(&bytes, &thread_id(path)))
}

/// The rollouts of the project rooted at `directory` (by their recorded working directory),
/// with their modification times.
pub fn of_project(sessions: &Path, directory: &Path) -> Vec<(PathBuf, u64)> {
    units(sessions)
        .into_iter()
        .filter(|unit| {
            read(&unit.path).is_ok_and(|t| {
                t.cwd
                    .as_deref()
                    .is_some_and(|cwd| Path::new(cwd).starts_with(directory))
            })
        })
        .map(|unit| (unit.path, unit.modified))
        .collect()
}

/// The text items of a `UserMessage`/`AgentMessage` item's `content`.
fn content_text(content: &Value) -> String {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| part["type"] == "text" || part["type"] == "Text")
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn new_turn(out: &mut Transcript, user: &str, at: &str) {
    let user = user.trim();
    if user.is_empty() {
        return;
    }
    if out.title.is_empty() {
        out.title = clip(user.lines().next().unwrap_or_default(), 120);
    }
    let n = out.turns.len();
    out.turns.push(Turn {
        n,
        at: at.to_string(),
        user: clip(user, CAP),
        assistant: String::new(),
        tools: Vec::new(),
    });
}

fn add_answer(out: &mut Transcript, text: &str) {
    let text = text.trim();
    if let Some(turn) = out.turns.last_mut()
        && !text.is_empty()
    {
        if !turn.assistant.is_empty() {
            turn.assistant.push_str("\n\n");
        }
        turn.assistant.push_str(text);
        turn.assistant = clip(&turn.assistant, CAP);
    }
}

/// Parses a rollout's bytes; `fallback_id` is the session id when no `session_meta` names one.
pub fn parse(bytes: &[u8], fallback_id: &str) -> Transcript {
    let complete = match bytes.iter().rposition(|byte| *byte == b'\n') {
        Some(end) => &bytes[..=end],
        None => &bytes[..0],
    };
    let mut out = Transcript {
        bytes: complete.len(),
        fingerprint: crate::fsutil::fingerprint(complete),
        partial_tail: complete.len() < bytes.len(),
        ..Transcript::default()
    };
    if out.partial_tail {
        out.skipped_lines += 1;
    }
    let mut meta_seen = false;
    let mut sub_agent = false;
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
        let payload = &record["payload"];
        match record["type"].as_str().unwrap_or_default() {
            "session_meta" if !meta_seen => {
                meta_seen = true;
                if let Some(id) = payload["id"].as_str().filter(|id| !id.is_empty()) {
                    out.session_id = id.to_string();
                }
                if let Some(cwd) = payload["cwd"].as_str().filter(|cwd| !cwd.is_empty()) {
                    out.cwd = Some(cwd.to_string());
                }
                sub_agent = payload["source"].is_object();
                if let Some(branch) = payload["git"]["branch"].as_str()
                    && !branch.is_empty()
                    && branch != "HEAD"
                {
                    out.branches.push(branch.to_string());
                }
            }
            "turn_context" => {
                if out.cwd.is_none()
                    && let Some(cwd) = payload["cwd"].as_str().filter(|cwd| !cwd.is_empty())
                {
                    out.cwd = Some(cwd.to_string());
                }
            }
            "compacted" => {
                if let Some(message) = payload["message"].as_str().filter(|m| !m.is_empty()) {
                    out.compact.push(clip(message, CAP));
                }
            }
            "response_item" => {
                if matches!(
                    payload["type"].as_str(),
                    Some("function_call" | "custom_tool_call")
                ) && let Some(name) = payload["name"].as_str().filter(|n| !n.is_empty())
                {
                    let input = if payload["arguments"].is_null() {
                        &payload["input"]
                    } else {
                        &payload["arguments"]
                    };
                    out.memory_writes.extend(memory_writes(name, input, at));
                    if let Some(turn) = out.turns.last_mut()
                        && !turn.tools.iter().any(|t| t == name)
                    {
                        turn.tools.push(name.to_string());
                    }
                }
            }
            "event_msg" => match payload["type"].as_str().unwrap_or_default() {
                "user_message" => new_turn(
                    &mut out,
                    payload["message"].as_str().unwrap_or_default(),
                    at,
                ),
                "agent_message" => {
                    add_answer(&mut out, payload["message"].as_str().unwrap_or_default())
                }
                "item_completed" => {
                    let item = &payload["item"];
                    match item["type"].as_str().unwrap_or_default() {
                        "UserMessage" => new_turn(&mut out, &content_text(&item["content"]), at),
                        "AgentMessage" => add_answer(&mut out, &content_text(&item["content"])),
                        _ => {}
                    }
                }
                // An MCP call as an event (`invocation`: server, tool, arguments); a call seen
                // both here and as a `function_call` links the same memory twice, which the
                // link counts once per memory.
                "mcp_tool_call_end" => {
                    let invocation = &payload["invocation"];
                    if let (Some(server), Some(tool)) =
                        (invocation["server"].as_str(), invocation["tool"].as_str())
                    {
                        out.memory_writes.extend(memory_writes(
                            &format!("{server}__{tool}"),
                            &invocation["arguments"],
                            at,
                        ));
                    }
                }
                "patch_apply_end" => {
                    if let Some(changes) = payload["changes"].as_object() {
                        for path in changes.keys() {
                            if !out.files.iter().any(|f| f == path) {
                                out.files.push(path.clone());
                            }
                        }
                    }
                }
                "context_compacted" => {
                    if let Some(message) = payload["message"].as_str().filter(|m| !m.is_empty()) {
                        out.compact.push(clip(message, CAP));
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    if sub_agent {
        out.turns.clear();
    }
    if out.session_id.is_empty() {
        out.session_id = fallback_id.to_string();
    }
    out
}

/// Codex's sessions (`crate::sessions::SessionSource`): its rollout files.
pub struct Sessions;

impl crate::sessions::SessionSource for Sessions {
    fn units(&self, dir: &Path) -> Result<Vec<Unit>, String> {
        Ok(units(dir))
    }

    /// The thread id of a rollout.
    fn unit_id(&self, file: &Path) -> String {
        if is_rollout(file) {
            thread_id(file)
        } else {
            file.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        }
    }

    fn read(&self, file: &Path, _data_dir: &Path) -> Result<(Transcript, String), String> {
        Ok((read(file)?, crate::sessions::source_label("rollout", file)))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// A rollout in Codex's format (legacy history mode) of a session in `cwd`.
    pub(crate) fn fixture(cwd: &str) -> String {
        [
            json!({"timestamp": "2026-11-09T10:00:00.000Z", "type": "session_meta", "payload": {
                "id": "019a5c1e-0000-7000-8000-000000000001", "timestamp": "2026-11-09T10:00:00.000Z",
                "cwd": cwd, "originator": "codex_cli_rs", "cli_version": "0.167.0", "source": "cli",
                "git": {"commit_hash": "abc", "branch": "main"}}}),
            json!({"timestamp": "2026-11-09T10:00:00.100Z", "type": "response_item", "payload": {
                "type": "message", "role": "user", "content": [{"type": "input_text",
                "text": "<environment_context>injected</environment_context>"}]}}),
            json!({"timestamp": "2026-11-09T10:00:01.000Z", "type": "turn_context", "payload": {
                "cwd": cwd, "approval_policy": "on-request", "model": "gpt-5.5"}}),
            json!({"timestamp": "2026-11-09T10:00:01.000Z", "type": "event_msg", "payload": {
                "type": "user_message", "message": "Use tabs in this repository, never spaces."}}),
            json!({"timestamp": "2026-11-09T10:00:02.000Z", "type": "response_item", "payload": {
                "type": "function_call", "name": "mcp__graphersal_memory__remember",
                "arguments": "{}", "call_id": "c1"}}),
            json!({"timestamp": "2026-11-09T10:00:03.000Z", "type": "event_msg", "payload": {
                "type": "agent_message", "message": "Noted: tabs, never spaces."}}),
            json!({"timestamp": "2026-11-09T10:01:00.000Z", "type": "event_msg", "payload": {
                "type": "user_message", "message": "Fix the parser."}}),
            json!({"timestamp": "2026-11-09T10:01:05.000Z", "type": "event_msg", "payload": {
                "type": "patch_apply_end", "call_id": "c2", "success": true,
                "changes": {format!("{cwd}/src/parser.rs"): {"type": "update"}}}}),
            json!({"timestamp": "2026-11-09T10:01:09.000Z", "type": "event_msg", "payload": {
                "type": "agent_message", "message": "Fixed the off-by-one in the parser."}}),
            json!({"timestamp": "2026-11-09T10:01:10.000Z", "type": "event_msg", "payload": {
                "type": "token_count", "info": null}}),
        ]
        .iter()
        .map(|v| format!("{v}\n"))
        .collect()
    }

    /// Writes a fixture rollout under `sessions` and returns its path.
    pub(crate) fn write_fixture(sessions: &Path, cwd: &str) -> PathBuf {
        let dir = sessions.join("2026/11/09");
        std::fs::create_dir_all(&dir).unwrap();
        let path =
            dir.join("rollout-2026-11-09T10-00-00-019a5c1e-0000-7000-8000-000000000001.jsonl");
        std::fs::write(&path, fixture(cwd)).unwrap();
        path
    }

    #[test]
    fn a_rollout_becomes_turns_with_tools_files_and_branch() {
        let t = parse(fixture("/w/app").as_bytes(), "x");
        assert_eq!(t.session_id, "019a5c1e-0000-7000-8000-000000000001");
        assert_eq!(t.cwd.as_deref(), Some("/w/app"));
        assert_eq!(t.branches, vec!["main"]);
        assert_eq!(t.turns.len(), 2);
        assert_eq!(
            t.turns[0].user,
            "Use tabs in this repository, never spaces."
        );
        assert_eq!(t.turns[0].assistant, "Noted: tabs, never spaces.");
        assert_eq!(t.turns[0].tools, vec!["mcp__graphersal_memory__remember"]);
        assert_eq!(t.turns[1].at, "2026-11-09T10:01:00.000Z");
        assert_eq!(t.files, vec!["/w/app/src/parser.rs"]);
        assert_eq!(t.title, "Use tabs in this repository, never spaces.");
        assert_eq!(t.started_at, "2026-11-09T10:00:00.000Z");
        assert_eq!(t.ended_at, "2026-11-09T10:01:10.000Z");
        assert!(!t.partial_tail);
        assert!(
            !t.turns
                .iter()
                .any(|turn| turn.user.contains("environment_context"))
        );
    }

    #[test]
    fn paginated_items_partial_lines_and_sub_agents() {
        let text = [
            json!({"timestamp": "t1", "type": "session_meta", "payload": {"id": "abc", "cwd": "/w"}}),
            json!({"timestamp": "t2", "type": "event_msg", "payload": {"type": "item_completed",
                "thread_id": "abc", "turn_id": "u", "item": {"type": "UserMessage", "id": "1",
                "content": [{"type": "text", "text": "Hello"}]}}}),
            json!({"timestamp": "t3", "type": "event_msg", "payload": {"type": "item_completed",
                "thread_id": "abc", "turn_id": "u", "item": {"type": "AgentMessage", "id": "2",
                "content": [{"type": "Text", "text": "Hi"}]}}}),
            json!({"timestamp": "t4", "type": "compacted", "payload": {"message": "Earlier: setup."}}),
        ]
        .iter()
        .map(|v| format!("{v}\n"))
        .collect::<String>();
        let with_tail = format!("{text}{{\"timestamp\": \"t5\", \"type\": \"event");
        let t = parse(with_tail.as_bytes(), "x");
        assert_eq!(t.turns.len(), 1);
        assert_eq!(
            (t.turns[0].user.as_str(), t.turns[0].assistant.as_str()),
            ("Hello", "Hi")
        );
        assert_eq!(t.compact, vec!["Earlier: setup."]);
        assert!(t.partial_tail);
        assert_eq!(t.fingerprint, crate::fsutil::fingerprint(text.as_bytes()));
        let sub = text.replace(
            "\"cwd\":\"/w\"",
            "\"cwd\":\"/w\",\"source\":{\"subagent\":\"review\"}",
        );
        assert!(parse(sub.as_bytes(), "x").turns.is_empty(), "{sub}");
    }

    #[test]
    fn rollouts_are_found_by_name_under_the_date_directories() {
        let tmp = crate::mcp_tests::TempDir::new("codex-scan");
        let sessions = tmp.path().join("sessions");
        let path = write_fixture(&sessions, "/w/app");
        std::fs::write(path.with_file_name("rollout-old.jsonl.zst"), "x").unwrap();
        std::fs::write(sessions.join("2026/11/09/notes.txt"), "x").unwrap();
        assert_eq!(scan(&sessions), vec![path.clone()]);
        assert_eq!(thread_id(&path), "019a5c1e-0000-7000-8000-000000000001");
        assert_eq!(
            thread_id(Path::new(
                "rollout-2026-11-09T10-00-00-019a5c1e-0000-7000-8000-000000000001_019a5c1e-0000-7000-8000-000000000002.jsonl"
            )),
            "019a5c1e-0000-7000-8000-000000000001"
        );
        assert_eq!(units(&sessions).len(), 1);
        assert_eq!(of_project(&sessions, Path::new("/w/app")).len(), 1);
        assert!(of_project(&sessions, Path::new("/w/other")).is_empty());
        assert!(scan(&tmp.path().join("missing")).is_empty());
    }

    #[test]
    fn a_rollout_records_the_memories_its_tool_calls_wrote() {
        let mut text = fixture("/w/app");
        for line in [
            json!({"timestamp": "2026-11-09T10:02:00.000Z", "type": "response_item", "payload": {
                "type": "function_call", "name": "mcp__graphersal_memory__remember",
                "arguments": "{\"slug\":\"tabs\",\"summary\":\"Tabs\"}", "call_id": "c3"}}),
            json!({"timestamp": "2026-11-09T10:02:05.000Z", "type": "event_msg", "payload": {
                "type": "mcp_tool_call_end", "call_id": "c4", "invocation": {
                    "server": "graphersal-memory", "tool": "settle",
                    "arguments": {"slug": "tabs", "other": "spaces", "decision": "supersedes"}}}}),
        ] {
            text.push_str(&format!("{line}\n"));
        }
        let t = parse(text.as_bytes(), "x");
        let got: Vec<(&str, &str)> = t
            .memory_writes
            .iter()
            .map(|w| (w.action.as_str(), w.slug.as_str()))
            .collect();
        // The fixture's own call has no slug (`{}`): no write.
        assert_eq!(
            got,
            vec![
                ("remember", "tabs"),
                ("settle", "tabs"),
                ("settle", "spaces")
            ]
        );
        assert_eq!(t.memory_writes[0].at, "2026-11-09T10:02:00.000Z");
    }
}
