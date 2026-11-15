//! CodeWhale's sessions: one JSON document per session, `<home>/sessions/<uuid>.json`,
//! rewritten whole and atomically after each turn (`crates/tui/src/session_manager.rs`), read
//! into the [`Transcript`] the indexing writes (never changed).
//!
//! The document (`SavedSession`, schema 1) has `metadata` (`id`, `title`, `created_at`,
//! `updated_at`, `workspace`, `mode`, ...), `messages` (the active branch: `{role, content}`,
//! content blocks tagged by `type`: `text`, `thinking`, `tool_use {name, input}`,
//! `tool_result`, ...) and `journal` (the canonical history, a tree of entries `{id,
//! parent_id, kind, created_at, ...}`, kind `message {message}`, `user {text}`, `assistant
//! {text}`, `compaction {summary}`, `branch_summary`, `system`; `leaf_id` the active branch's
//! last entry; `crates/runtime/src/session_tree.rs`). Read here: the active branch of the
//! journal (its entries carry the times), else `messages` (times only from the metadata).
//!
//! A user message with text starts a turn (a leading `<turn_meta>` block taken off, as
//! CodeWhale does for its titles); tool results come back as user messages and are not turns;
//! assistant text is the answer, `thinking` is not read; `tool_use` names the tools, the files
//! edited (`write_file`, `edit_file`, `write`, `edit`: `path`; `apply_patch`: its `*** Add/
//! Update File:` lines) and the memories our memory tools wrote. Compaction summaries are the
//! harness's own. The other files of the sessions directory (`checkpoints/`, archives,
//! `session_boot_owners.json`) are not sessions. A document that does not parse is reported,
//! never guessed at.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::sessions::Unit;
use crate::transcript::{CAP, Transcript, Turn, clip, memory_writes};

/// Whether `path` names a CodeWhale session document (`<uuid>.json`: 36 characters of hex
/// digits and dashes, then `.json`).
pub fn is_session(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "json")
        && path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| {
                stem.len() == 36
                    && stem.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
                    && stem.matches('-').count() == 4
            })
}

/// The session documents directly in `sessions`, sorted.
pub fn scan(sessions: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(sessions) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|path| path.is_file() && is_session(path))
        .collect();
    out.sort();
    out
}

/// The sessions of `sessions` as units (size and modification time).
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

/// Reads a session document.
pub fn read(path: &Path) -> Result<Transcript, String> {
    let bytes =
        std::fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    parse(&bytes).map_err(|err| format!("{}: {err}", path.display()))
}

/// The workspace a session document names (`metadata.workspace`), read without parsing its
/// messages into turns.
pub fn workspace_of(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value["metadata"]["workspace"]
        .as_str()
        .filter(|w| !w.is_empty())
        .map(str::to_string)
}

/// The text blocks of a message's content (a list of blocks, or a plain string).
fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The user's prompt in a stored user text: a leading `<turn_meta>...</turn_meta>` taken off
/// (`session_manager.rs`, `extract_user_prompt`).
fn user_prompt(raw: &str) -> &str {
    let trimmed = raw.trim_start();
    let Some(after) = trimmed.strip_prefix("<turn_meta>") else {
        return trimmed;
    };
    match after.find("</turn_meta>") {
        Some(close) => after[close + "</turn_meta>".len()..].trim_start(),
        None => after.trim_start(),
    }
}

/// The files an `apply_patch` input names (`*** Add File: <path>`, `*** Update File: <path>`).
fn patch_files(input: &Value) -> Vec<String> {
    let text = ["patch", "input", "diff"]
        .iter()
        .find_map(|key| input[*key].as_str())
        .or(input.as_str())
        .unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            line.strip_prefix("*** Add File:")
                .or_else(|| line.strip_prefix("*** Update File:"))
        })
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .collect()
}

/// One message of the active branch, with its time (empty when unknown).
struct Entry {
    role: String,
    content: Value,
    at: String,
    compaction: Option<String>,
}

/// The active branch of a journal: from `leaf_id` up through `parent_id`, in order; `None`
/// when the journal has no entries or its chain does not resolve.
fn active_branch(journal: &Value, leaf: Option<&str>) -> Option<Vec<Entry>> {
    let entries = journal["entries"].as_array().filter(|e| !e.is_empty())?;
    let by_id: HashMap<&str, &Value> = entries
        .iter()
        .filter_map(|e| Some((e["id"].as_str()?, e)))
        .collect();
    let leaf = journal["leaf_id"]
        .as_str()
        .or(leaf)
        .or_else(|| entries.last()?["id"].as_str())?;
    let mut chain = Vec::new();
    let mut at = Some(leaf);
    while let Some(id) = at {
        let entry = by_id.get(id)?;
        if chain.len() > entries.len() {
            return None; // a cycle
        }
        chain.push(*entry);
        at = entry["parent_id"].as_str();
    }
    chain.reverse();
    Some(
        chain
            .into_iter()
            .filter_map(|entry| {
                let at = entry["created_at"].as_str().unwrap_or_default().to_string();
                let (role, content, compaction) = match entry["kind"].as_str()? {
                    "message" => (
                        entry["message"]["role"].as_str()?.to_string(),
                        entry["message"]["content"].clone(),
                        None,
                    ),
                    "user" => ("user".to_string(), entry["text"].clone(), None),
                    "assistant" => ("assistant".to_string(), entry["text"].clone(), None),
                    "compaction" => (
                        "system".to_string(),
                        Value::Null,
                        entry["summary"].as_str().map(str::to_string),
                    ),
                    _ => return None,
                };
                Some(Entry {
                    role,
                    content,
                    at,
                    compaction,
                })
            })
            .collect(),
    )
}

/// Parses a session document's bytes.
pub fn parse(bytes: &[u8]) -> Result<Transcript, String> {
    let doc: Value =
        serde_json::from_slice(bytes).map_err(|err| format!("not a session document: {err}"))?;
    let meta = &doc["metadata"];
    if !doc.is_object() || !meta.is_object() {
        return Err("not a session document (no metadata)".to_string());
    }
    let mut out = Transcript {
        bytes: bytes.len(),
        fingerprint: crate::fsutil::fingerprint(bytes),
        session_id: meta["id"].as_str().unwrap_or_default().to_string(),
        cwd: meta["workspace"]
            .as_str()
            .filter(|w| !w.is_empty())
            .map(str::to_string),
        started_at: meta["created_at"].as_str().unwrap_or_default().to_string(),
        ended_at: meta["updated_at"].as_str().unwrap_or_default().to_string(),
        records: 1,
        ..Transcript::default()
    };
    let title = meta["title"].as_str().map(user_prompt).unwrap_or_default();
    if !title.is_empty() && title != "New Session" && title != "Session" {
        out.title = clip(title.lines().next().unwrap_or_default(), 120);
    }
    let entries = active_branch(&doc["journal"], doc["leaf_id"].as_str()).unwrap_or_else(|| {
        doc["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|message| Entry {
                role: message["role"].as_str().unwrap_or_default().to_string(),
                content: message["content"].clone(),
                at: String::new(),
                compaction: None,
            })
            .collect()
    });
    let workspace = out.cwd.clone();
    for entry in entries {
        if let Some(summary) = entry.compaction.filter(|s| !s.trim().is_empty()) {
            out.compact.push(clip(&summary, CAP));
            continue;
        }
        match entry.role.as_str() {
            "user" => {
                let text = text_of(&entry.content);
                let prompt = user_prompt(&text).trim();
                // Tool results only, or a runtime event CodeWhale adds to the history as a
                // user message (MCP server instructions, mode notices, sub-agent results;
                // `crates/tui/src/runtime_handoff.rs`): not the user's words.
                if prompt.is_empty() || prompt.starts_with("<codewhale:runtime_event") {
                    continue;
                }
                if out.title.is_empty() {
                    out.title = clip(prompt.lines().next().unwrap_or_default(), 120);
                }
                let at = if entry.at.is_empty() && out.turns.is_empty() {
                    out.started_at.clone()
                } else {
                    entry.at.clone()
                };
                let n = out.turns.len();
                out.turns.push(Turn {
                    n,
                    at,
                    user: clip(prompt, CAP),
                    assistant: String::new(),
                    tools: Vec::new(),
                });
            }
            "assistant" | "assistant_interrupted" => {
                let text = text_of(&entry.content);
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
                for block in entry.content.as_array().into_iter().flatten() {
                    if block["type"] != "tool_use" {
                        continue;
                    }
                    let Some(name) = block["name"].as_str().filter(|n| !n.is_empty()) else {
                        continue;
                    };
                    let input = &block["input"];
                    out.memory_writes
                        .extend(memory_writes(name, input, &entry.at));
                    let edited = match name {
                        "write_file" | "edit_file" | "write" | "edit" => ["path", "file_path"]
                            .iter()
                            .find_map(|key| input[*key].as_str())
                            .map(|p| vec![p.to_string()])
                            .unwrap_or_default(),
                        "apply_patch" => patch_files(input),
                        _ => Vec::new(),
                    };
                    for file in edited {
                        let file = match &workspace {
                            Some(root) => crate::transcript::join_text(root, &file),
                            None => file,
                        };
                        if !out.files.contains(&file) {
                            out.files.push(file);
                        }
                    }
                    if let Some(turn) = out.turns.last_mut()
                        && !turn.tools.iter().any(|t| t == name)
                    {
                        turn.tools.push(name.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    if out.session_id.is_empty() {
        out.session_id = "unknown".to_string();
    }
    Ok(out)
}

/// The sessions of `sessions` whose workspace is `directory` or inside it, written at or
/// after `since` (seconds since the epoch): what a TUI that ran there since then wrote. They are
/// marked ended when its MCP connection closes (the daemon, `crate::daemon`; the shim too, when
/// it lives long enough, `crate::session_sync`).
pub fn written_since(sessions: &Path, directory: &Path, since: u64) -> Vec<PathBuf> {
    units(sessions)
        .into_iter()
        .filter(|unit| unit.modified >= since)
        .filter(|unit| {
            workspace_of(&unit.path).is_some_and(|w| Path::new(&w).starts_with(directory))
        })
        .map(|unit| unit.path)
        .collect()
}

/// CodeWhale's sessions (`crate::sessions::SessionSource`): its saved sessions, ended
/// by the end of its MCP connection (it has no session end hook we use).
pub struct Sessions;

impl crate::sessions::SessionSource for Sessions {
    fn units(&self, dir: &Path) -> Result<Vec<Unit>, String> {
        Ok(units(dir))
    }

    fn read(&self, file: &Path, _data_dir: &Path) -> Result<(Transcript, String), String> {
        Ok((
            read(file)?,
            crate::sessions::source_label("codewhale", file),
        ))
    }

    fn written_since(&self, sessions: &Path, dir: &Path, since: u64) -> Vec<PathBuf> {
        written_since(sessions, dir, since)
    }

    fn ended_by_connection(&self, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
        crate::codewhale::CodeWhalePaths::resolve(env)
            .ok()
            .map(|paths| paths.sessions)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) const ID: &str = "0a1b2c3d-0000-4000-8000-000000000001";

    /// A saved session in CodeWhale's format (schema 1, with its journal) in `workspace`.
    pub(crate) fn fixture(workspace: &str) -> String {
        let message = |id: &str, parent: Option<&str>, at: &str, msg: Value| {
            let mut entry = json!({"id": id, "kind": "message", "message": msg,
                "created_at": at, "spawn_depth": 0});
            if let Some(parent) = parent {
                entry["parent_id"] = json!(parent);
            }
            entry
        };
        let user1 = json!({"role": "user", "content": [{"type": "text",
            "text": "<turn_meta>cache</turn_meta>\nUse tabs in this repository, never spaces."}]});
        let assistant1 = json!({"role": "assistant", "content": [
            {"type": "thinking", "thinking": "the user wants tabs"},
            {"type": "tool_use", "id": "call-1", "name": "mcp_graphersal-memory_remember",
             "input": {"slug": "tabs", "summary": "Tabs, never spaces"}},
        ]});
        let result1 = json!({"role": "user", "content": [{"type": "tool_result",
            "tool_use_id": "call-1", "content": "ok"}]});
        let answer1 = json!({"role": "assistant", "content": [{"type": "text",
            "text": "Noted: tabs, never spaces."}]});
        let user2 =
            json!({"role": "user", "content": [{"type": "text", "text": "Fix the parser."}]});
        let assistant2 = json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": "call-2", "name": "edit_file",
             "input": {"path": "src/parser.rs", "old": "a", "new": "b"}},
            {"type": "tool_use", "id": "call-3", "name": "apply_patch",
             "input": {"patch": "*** Begin Patch\n*** Update File: /abs/lib.rs\n@@\n*** End Patch"}},
            {"type": "text", "text": "Fixed the off-by-one in the parser."},
        ]});
        let abandoned = json!({"role": "user", "content": [{"type": "text",
            "text": "A branch that was edited away."}]});
        let messages = vec![
            user1.clone(),
            assistant1.clone(),
            result1.clone(),
            answer1.clone(),
            user2.clone(),
            assistant2.clone(),
        ];
        json!({
            "schema_version": 1,
            "metadata": {"id": ID, "title": "Use tabs in this repository, never spaces.",
                "created_at": "2026-11-14T10:00:00Z", "updated_at": "2026-11-14T10:05:00Z",
                "message_count": 6, "total_tokens": 1234, "model": "gpt-oss-20b",
                "model_provider": "ollama", "workspace": workspace, "mode": "agent"},
            "messages": messages,
            "journal": {"entries": [
                message("e1", None, "2026-11-14T10:00:00Z", user1),
                message("e2", Some("e1"), "2026-11-14T10:00:05Z", assistant1),
                message("e3", Some("e2"), "2026-11-14T10:00:06Z", result1),
                message("e4", Some("e3"), "2026-11-14T10:00:09Z", answer1),
                message("x1", Some("e4"), "2026-11-14T10:01:00Z", abandoned),
                {"id": "c1", "parent_id": "e4", "kind": "compaction",
                 "summary": "Earlier: the user prefers tabs.", "created_at": "2026-11-14T10:02:00Z"},
                message("e5", Some("c1"), "2026-11-14T10:03:00Z", user2),
                message("e6", Some("e5"), "2026-11-14T10:04:00Z", assistant2),
            ], "leaf_id": "e6", "schema_version": 1},
            "system_prompt": null
        })
        .to_string()
    }

    /// Writes a fixture session under `sessions` and returns its path.
    pub(crate) fn write_fixture(sessions: &Path, workspace: &str) -> PathBuf {
        std::fs::create_dir_all(sessions).unwrap();
        let path = sessions.join(format!("{ID}.json"));
        std::fs::write(&path, fixture(workspace)).unwrap();
        path
    }

    #[test]
    fn a_session_becomes_turns_with_tools_files_and_memories() {
        let t = parse(fixture("/w/app").as_bytes()).unwrap();
        assert_eq!(t.session_id, ID);
        assert_eq!(t.cwd.as_deref(), Some("/w/app"));
        assert_eq!(t.title, "Use tabs in this repository, never spaces.");
        assert_eq!(t.turns.len(), 2, "{:?}", t.turns);
        assert_eq!(
            t.turns[0].user,
            "Use tabs in this repository, never spaces."
        );
        assert_eq!(t.turns[0].assistant, "Noted: tabs, never spaces.");
        assert_eq!(t.turns[0].tools, vec!["mcp_graphersal-memory_remember"]);
        assert_eq!(t.turns[0].at, "2026-11-14T10:00:00Z");
        assert_eq!(t.turns[1].user, "Fix the parser.");
        assert_eq!(t.turns[1].at, "2026-11-14T10:03:00Z");
        assert_eq!(t.turns[1].assistant, "Fixed the off-by-one in the parser.");
        assert_eq!(t.turns[1].tools, vec!["edit_file", "apply_patch"]);
        assert_eq!(
            t.files,
            vec![
                crate::transcript::join_text("/w/app", "src/parser.rs"),
                "/abs/lib.rs".to_string()
            ]
        );
        assert_eq!(t.compact, vec!["Earlier: the user prefers tabs."]);
        assert_eq!(t.memory_writes.len(), 1);
        assert_eq!(t.memory_writes[0].slug, "tabs");
        assert_eq!(t.memory_writes[0].at, "2026-11-14T10:00:05Z");
        assert_eq!(t.started_at, "2026-11-14T10:00:00Z");
        assert_eq!(t.ended_at, "2026-11-14T10:05:00Z");
        assert!(
            !t.turns.iter().any(|turn| turn.user.contains("edited away")),
            "only the active branch"
        );
        assert!(!t.turns[0].assistant.contains("the user wants tabs"));
    }

    #[test]
    fn runtime_events_are_not_the_users_words() {
        let mut doc: Value = serde_json::from_str(&fixture("/w")).unwrap();
        doc.as_object_mut().unwrap().remove("journal");
        let event = json!({"role": "user", "content": [
            {"type": "text", "text": "<codewhale:runtime_event kind=\"mcp_server_instructions\" visibility=\"internal\">\nguidance\n</codewhale:runtime_event>"},
            {"type": "text", "text": "<turn_meta>runtime</turn_meta>"}]});
        doc["messages"].as_array_mut().unwrap().insert(0, event);
        let t = parse(doc.to_string().as_bytes()).unwrap();
        assert_eq!(t.turns.len(), 2);
        assert!(!t.turns[0].user.contains("runtime_event"));
    }

    #[test]
    fn without_a_journal_the_messages_are_read() {
        let mut doc: Value = serde_json::from_str(&fixture("/w")).unwrap();
        doc.as_object_mut().unwrap().remove("journal");
        let t = parse(doc.to_string().as_bytes()).unwrap();
        assert_eq!(t.turns.len(), 2);
        assert_eq!(t.turns[0].at, "2026-11-14T10:00:00Z", "the session's start");
        assert_eq!(t.turns[1].at, "");
        assert!(t.compact.is_empty());
        // A journal whose chain does not resolve falls back to the messages too.
        doc["journal"] = json!({"entries": [{"id": "a", "parent_id": "missing",
            "kind": "user", "text": "x", "created_at": "t"}], "leaf_id": "a"});
        assert_eq!(parse(doc.to_string().as_bytes()).unwrap().turns.len(), 2);
    }

    #[test]
    fn a_new_session_stub_has_no_turns_and_bad_documents_are_errors() {
        let stub = json!({"schema_version": 1, "metadata": {"id": ID, "title": "New Session",
            "created_at": "t", "updated_at": "t", "workspace": "/w"}, "messages": []});
        let t = parse(stub.to_string().as_bytes()).unwrap();
        assert!(t.turns.is_empty());
        assert_eq!(t.title, "");
        assert!(parse(b"{\"a\": 1}").is_err());
        assert!(parse(b"not json").is_err());
    }

    #[test]
    fn sessions_are_found_by_name_and_workspace() {
        let tmp = crate::mcp_tests::TempDir::new("cw-scan");
        let sessions = tmp.path().join("sessions");
        let path = write_fixture(&sessions, "/w/app");
        std::fs::create_dir_all(sessions.join("checkpoints")).unwrap();
        std::fs::write(
            sessions.join("checkpoints").join(format!("{ID}.json")),
            "{}",
        )
        .unwrap();
        std::fs::write(sessions.join("session_boot_owners.json"), "{}").unwrap();
        assert_eq!(scan(&sessions), vec![path.clone()]);
        assert_eq!(units(&sessions).len(), 1);
        assert_eq!(workspace_of(&path).as_deref(), Some("/w/app"));
        assert_eq!(
            written_since(&sessions, Path::new("/w/app"), 0),
            vec![path.clone()]
        );
        assert!(written_since(&sessions, Path::new("/w/app"), u64::MAX).is_empty());
        assert_eq!(
            written_since(&sessions, Path::new("/w"), 0),
            vec![path.clone()],
            "a session in a directory of the project"
        );
        assert!(written_since(&sessions, Path::new("/w/other"), 0).is_empty());
        assert!(scan(&tmp.path().join("missing")).is_empty());
        assert!(!is_session(Path::new("notes.json")));
    }
}
