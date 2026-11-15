//! Claude Code's transcripts (`~/.claude/projects/<dir>/<session>.jsonl`): the parser
//! (`parse`) of their records into a `crate::transcript::Transcript` (turns, title, times,
//! branches, edited files, compaction summaries, memory writes; sidechains and generated
//! records left out; unknown records, bad lines and a partial last line skipped).

#![allow(clippy::wildcard_imports)]

use serde_json::Value;

use crate::fsutil;
use crate::transcript::*;

/// The longest compaction summary kept (characters).
const COMPACT_CAP: usize = 20_000;

/// The longest answer to the assistant's question kept (characters).
const ANSWER_CAP: usize = 3_000;

/// Prefixes of user records the harness writes itself.
const SKIP_PREFIXES: [&str; 9] = [
    "<task-notification",
    "<bash-stdout",
    "<bash-stderr",
    "<local-command-stdout",
    "<local-command-caveat",
    "<system-reminder",
    "[Request interrupted",
    "<command-message",
    "Caveat:",
];

/// Slash commands that carry no content.
const SKIP_COMMANDS: [&str; 8] = [
    "clear", "exit", "resume", "config", "model", "fast", "cost", "status",
];

/// The text of a message's content: a string, or the `text` blocks of a list.
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

/// The text between `<tag>` and `</tag>`.
fn between<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(&text[start..end])
}

/// A user record's text -> the prompt to keep, or `None` for records the harness generated.
pub fn prompt_of(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || SKIP_PREFIXES.iter().any(|prefix| t.starts_with(prefix)) {
        return None;
    }
    if let Some(name) = between(t, "command-name") {
        let name = name.trim();
        let bare = name.trim_start_matches('/');
        if t.starts_with("<command-name>") && SKIP_COMMANDS.contains(&bare) {
            return None;
        }
        let args = between(t, "command-args").map(str::trim).unwrap_or("");
        return Some(format!("/{bare} {args}").trim().to_string());
    }
    if let Some(input) = between(t, "bash-input") {
        return Some(format!("! {}", input.trim()));
    }
    Some(t.to_string())
}

/// Parses a transcript's bytes; `fallback_id` is the session id when no record names one.
pub fn parse(bytes: &[u8], fallback_id: &str) -> Transcript {
    let complete = match bytes.iter().rposition(|byte| *byte == b'\n') {
        Some(end) => &bytes[..=end],
        None => &bytes[..0],
    };
    let mut out = Transcript {
        session_id: String::new(),
        bytes: complete.len(),
        fingerprint: fsutil::fingerprint(complete),
        partial_tail: complete.len() < bytes.len(),
        ..Transcript::default()
    };
    if out.partial_tail {
        out.skipped_lines += 1;
    }
    let mut summary_title = String::new();
    for line in complete.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(Value::Object(record)) = serde_json::from_slice::<Value>(line) else {
            out.skipped_lines += 1;
            continue;
        };
        out.records += 1;
        let record = Value::Object(record);
        if record["isSidechain"] == Value::Bool(true) {
            continue;
        }
        read_record(&record, &mut out, &mut summary_title);
    }
    if out.title.is_empty() {
        out.title = summary_title;
    }
    if out.session_id.is_empty() {
        out.session_id = fallback_id.to_string();
    }
    out
}

/// Applies one main-session record.
fn read_record(record: &Value, out: &mut Transcript, summary_title: &mut String) {
    if out.session_id.is_empty()
        && let Some(id) = record["sessionId"].as_str().filter(|id| !id.is_empty())
    {
        out.session_id = id.to_string();
    }
    if out.cwd.is_none()
        && let Some(cwd) = record["cwd"].as_str().filter(|cwd| !cwd.is_empty())
    {
        out.cwd = Some(cwd.to_string());
    }
    let at = record["timestamp"].as_str().unwrap_or_default().to_string();
    if !at.is_empty() {
        if out.started_at.is_empty() {
            out.started_at = at.clone();
        }
        out.ended_at = at.clone();
    }
    if let Some(branch) = record["gitBranch"].as_str()
        && !branch.is_empty()
        && branch != "HEAD"
        && !out.branches.iter().any(|b| b == branch)
    {
        out.branches.push(branch.to_string());
    }
    match record["type"].as_str().unwrap_or_default() {
        "ai-title" => {
            if let Some(title) = record["aiTitle"]
                .as_str()
                .or_else(|| record["title"].as_str())
                .filter(|title| !title.is_empty())
            {
                out.title = title.to_string();
            }
        }
        // Older versions name a session with a `summary` record.
        "summary" => {
            if let Some(title) = record["summary"].as_str() {
                *summary_title = title.to_string();
            }
        }
        "attachment" => {
            let attachment = &record["attachment"];
            if attachment["type"] == "queued_command"
                && attachment["origin"]["kind"] == "human"
                && let Some(prompt) = attachment["prompt"].as_str()
            {
                match out.turns.last_mut() {
                    Some(turn) => {
                        turn.user.push_str("\n\n[sent while the assistant worked] ");
                        turn.user.push_str(prompt);
                    }
                    None => {
                        let at = attachment["timestamp"].as_str().unwrap_or(&at).to_string();
                        new_turn(out, prompt.to_string(), at);
                    }
                }
            }
        }
        "user" => read_user(record, out, at),
        "assistant" => read_assistant(record, out, &at),
        // Every other record type (snapshots, API records, modes, ...) carries no dialogue.
        _ => {}
    }
}

fn new_turn(out: &mut Transcript, user: String, at: String) {
    let n = out.turns.len();
    out.turns.push(Turn {
        n,
        at,
        user,
        assistant: String::new(),
        tools: Vec::new(),
    });
}

fn read_user(record: &Value, out: &mut Transcript, at: String) {
    let content = &record["message"]["content"];
    if record["isCompactSummary"] == Value::Bool(true) {
        out.compact.push(clip(&text_of(content), COMPACT_CAP));
        return;
    }
    if let (Value::Array(blocks), Some(turn)) = (content, out.turns.last_mut()) {
        for block in blocks.iter().filter(|b| b["type"] == "tool_result") {
            let result = match &block["content"] {
                Value::String(text) => text.clone(),
                other => text_of(other),
            };
            if result.contains("questions have been answered") {
                turn.user
                    .push_str("\n\n[answer to the assistant's question] ");
                turn.user.push_str(&clip(&result, ANSWER_CAP));
            }
        }
    }
    if record["isMeta"] == Value::Bool(true) || !record["toolUseResult"].is_null() {
        return;
    }
    // Messages the harness generates (task notifications, hand-backs of sub-agents) carry an
    // origin other than a human.
    if let Some(kind) = record["origin"]["kind"].as_str()
        && kind != "human"
    {
        return;
    }
    if let Some(prompt) = prompt_of(&text_of(content)) {
        new_turn(out, prompt, at);
    }
}

fn read_assistant(record: &Value, out: &mut Transcript, at: &str) {
    let Some(turn) = out.turns.last_mut() else {
        return;
    };
    let Some(blocks) = record["message"]["content"].as_array() else {
        return;
    };
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => {
                let text = block["text"].as_str().unwrap_or_default().trim();
                if !text.is_empty() {
                    if !turn.assistant.is_empty() {
                        turn.assistant.push_str("\n\n");
                    }
                    turn.assistant.push_str(text);
                }
            }
            Some("tool_use") => {
                let name = block["name"].as_str().unwrap_or_default();
                if !name.is_empty() && !turn.tools.iter().any(|t| t == name) {
                    turn.tools.push(name.to_string());
                }
                out.memory_writes
                    .extend(memory_writes(name, &block["input"], at));
                if matches!(name, "Edit" | "Write" | "MultiEdit" | "NotebookEdit")
                    && let Some(path) = block["input"]["file_path"]
                        .as_str()
                        .or_else(|| block["input"]["notebook_path"].as_str())
                    && !out.files.iter().any(|f| f == path)
                {
                    out.files.push(path.to_string());
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// An anonymized transcript in the shape Claude Code writes (no real content).
    pub(crate) fn fixture() -> String {
        let lines = [
            json!({"type": "permission-mode", "permissionMode": "default", "sessionId": "s-1"}),
            json!({"type": "user", "sessionId": "s-1", "cwd": "/w/app", "gitBranch": "main",
                   "timestamp": "2026-01-02T10:00:00.000Z", "origin": {"kind": "human"},
                   "message": {"role": "user", "content": "Add a cache to the parser."}}),
            json!({"type": "assistant", "sessionId": "s-1", "timestamp": "2026-01-02T10:00:05.000Z",
                   "message": {"role": "assistant", "content": [
                       {"type": "thinking", "thinking": "hidden"},
                       {"type": "text", "text": "I will add it."},
                       {"type": "tool_use", "name": "Edit", "input": {"file_path": "/w/app/src/parse.rs"}},
                       {"type": "tool_use", "name": "Bash", "input": {"command": "cargo test"}}]}}),
            json!({"type": "user", "sessionId": "s-1", "timestamp": "2026-01-02T10:00:09.000Z",
                   "toolUseResult": {"stdout": "ok"},
                   "message": {"content": [{"type": "tool_result", "content": "test output"}]}}),
            json!({"type": "attachment", "sessionId": "s-1", "timestamp": "2026-01-02T10:00:10.000Z",
                   "attachment": {"type": "queued_command", "prompt": "Also keep it small.",
                                  "origin": {"kind": "human"}}}),
            json!({"type": "user", "sessionId": "s-1", "isSidechain": true, "agentId": "a1",
                   "timestamp": "2026-01-02T10:00:11.000Z",
                   "message": {"content": "A sub-agent's task, never a turn."}}),
            json!({"type": "user", "sessionId": "s-1", "timestamp": "2026-01-02T10:00:12.000Z",
                   "origin": {"kind": "task-notification"},
                   "message": {"content": "<task-notification>done</task-notification>"}}),
            json!({"type": "user", "sessionId": "s-1", "timestamp": "2026-01-02T10:00:13.000Z",
                   "origin": {"kind": "peer", "from": "a1"},
                   "message": {"content": "A sub-agent's report."}}),
            json!({"type": "assistant", "sessionId": "s-1", "timestamp": "2026-01-02T10:00:20.000Z",
                   "message": {"content": [{"type": "text", "text": "Done; tests pass."},
                       {"type": "tool_use", "name": "Bash", "input": {}}]}}),
            json!({"type": "frobnicate-v9", "sessionId": "s-1", "payload": [1, 2, 3]}),
            json!({"type": "ai-title", "sessionId": "s-1", "aiTitle": "Parser cache"}),
            json!({"type": "user", "sessionId": "s-1", "isMeta": true, "timestamp": "2026-01-02T11:00:00.000Z",
                   "message": {"content": "meta"}}),
            json!({"type": "user", "sessionId": "s-1", "timestamp": "2026-01-02T11:00:01.000Z",
                   "message": {"content": "<command-name>/clear</command-name>"}}),
            json!({"type": "user", "sessionId": "s-1", "isCompactSummary": true,
                   "timestamp": "2026-01-02T11:00:02.000Z",
                   "message": {"content": "Earlier: the cache was planned."}}),
            json!({"type": "user", "sessionId": "s-1", "gitBranch": "feat/cache",
                   "timestamp": "2026-01-02T11:00:03.000Z",
                   "message": {"content": "<command-name>/review</command-name><command-args>src</command-args>"}}),
            json!({"type": "user", "sessionId": "s-1", "timestamp": "2026-01-02T11:00:04.000Z",
                   "message": {"content": "<bash-input>git status</bash-input>"}}),
        ];
        let mut text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        text.push_str("not json at all\n[1, 2]\n\n");
        text
    }

    #[test]
    fn a_transcript_becomes_turns_without_sub_agents_and_generated_records() {
        let parsed = parse(fixture().as_bytes(), "file-id");
        assert_eq!(parsed.session_id, "s-1");
        assert_eq!(parsed.cwd.as_deref(), Some("/w/app"));
        assert_eq!(parsed.title, "Parser cache");
        assert_eq!(parsed.started_at, "2026-01-02T10:00:00.000Z");
        assert_eq!(parsed.ended_at, "2026-01-02T11:00:04.000Z");
        assert_eq!(parsed.branches, vec!["main", "feat/cache"]);
        assert_eq!(parsed.files, vec!["/w/app/src/parse.rs"]);
        assert_eq!(parsed.compact, vec!["Earlier: the cache was planned."]);
        assert_eq!(parsed.skipped_lines, 2, "not json, an array");
        assert!(!parsed.partial_tail);
        let users: Vec<&str> = parsed.turns.iter().map(|t| t.user.as_str()).collect();
        assert_eq!(
            users,
            vec![
                "Add a cache to the parser.\n\n[sent while the assistant worked] Also keep it small.",
                "/review src",
                "! git status",
            ]
        );
        let first = &parsed.turns[0];
        assert_eq!(first.n, 0);
        assert_eq!(first.at, "2026-01-02T10:00:00.000Z");
        assert_eq!(first.assistant, "I will add it.\n\nDone; tests pass.");
        assert_eq!(first.tools, vec!["Edit", "Bash"]);
        assert!(!format!("{parsed:?}").contains("sub-agent"));
    }

    #[test]
    fn a_partial_last_line_is_skipped_and_not_fingerprinted() {
        let whole = fixture();
        let partial = format!("{whole}{{\"type\": \"user\", \"message\": {{\"content\": \"half");
        let a = parse(whole.as_bytes(), "x");
        let b = parse(partial.as_bytes(), "x");
        assert!(b.partial_tail);
        assert_eq!(b.skipped_lines, a.skipped_lines + 1);
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(a.turns, b.turns);
        assert_eq!(b.bytes, whole.len());
        // The line completes: a new fingerprint and one more turn.
        let grown = format!(
            "{whole}{}\n",
            json!({"type": "user", "message": {"content": "next"}, "timestamp": "2026-01-03T00:00:00Z"})
        );
        let c = parse(grown.as_bytes(), "x");
        assert_ne!(c.fingerprint, a.fingerprint);
        assert_eq!(c.turns.len(), a.turns.len() + 1);
        assert_eq!(c.ended_at, "2026-01-03T00:00:00Z");
    }

    #[test]
    fn empty_and_unknown_input_gives_an_empty_transcript() {
        let empty = parse(b"", "id-1");
        assert_eq!(empty.session_id, "id-1");
        assert!(empty.turns.is_empty());
        let odd = parse(
            b"{\"type\":\"x\"}\n{\"type\":\"summary\",\"summary\":\"Old title\"}\n",
            "id-2",
        );
        assert_eq!(odd.records, 2);
        assert_eq!(odd.title, "Old title");
        assert!(odd.turns.is_empty());
    }

    #[test]
    fn prompts_and_clips() {
        assert_eq!(prompt_of("  "), None);
        assert_eq!(prompt_of("<system-reminder>x</system-reminder>"), None);
        assert_eq!(prompt_of("<command-name>/exit</command-name>"), None);
        assert_eq!(
            prompt_of("<command-name>plan</command-name>").as_deref(),
            Some("/plan")
        );
        assert_eq!(prompt_of(" hi ").as_deref(), Some("hi"));
        assert_eq!(clip("abcdef", 10), "abcdef");
        let clipped = clip(&"x".repeat(100), 10);
        assert!(clipped.starts_with("xxxx [...] "), "{clipped}");
        assert_eq!(clip("äöüäöüäöüä-", 5), "äö [...] üä-");
    }

    #[test]
    fn edited_files_become_relative() {
        assert_eq!(
            relative_file("/w/app/src/a.rs", "/w/app").as_deref(),
            Some("src/a.rs")
        );
        assert_eq!(
            relative_file("/w/app/.claude/worktrees/agent-1/src/b.rs", "/w/app").as_deref(),
            Some("src/b.rs")
        );
        assert_eq!(relative_file("/tmp/x.rs", "/w/app"), None);
        assert_eq!(relative_file("/w/apple/x.rs", "/w/app"), None);
    }

    #[test]
    fn windows_paths_become_relative_with_slashes() {
        let rel = |path, root| relative_file(path, root);
        assert_eq!(
            rel(r"C:\w\app\src\a.rs", r"C:\w\app").as_deref(),
            Some("src/a.rs")
        );
        assert_eq!(
            rel(r"c:\W\App\src\a.rs", r"C:\w\app\").as_deref(),
            Some("src/a.rs"),
            "no case on Windows"
        );
        assert_eq!(
            rel(r"C:\w\app/src/a.rs", "C:/w/app").as_deref(),
            Some("src/a.rs"),
            "either separator"
        );
        assert_eq!(
            rel(r"C:\w\app\.claude\worktrees\agent-1\src\b.rs", r"C:\w\app").as_deref(),
            Some("src/b.rs")
        );
        assert_eq!(
            rel(r"\\srv\share\app\x.rs", r"\\srv\share\app").as_deref(),
            Some("x.rs")
        );
        assert_eq!(rel(r"C:\w\apple\x.rs", r"C:\w\app"), None);
        assert_eq!(rel(r"D:\w\app\x.rs", r"C:\w\app"), None);
        assert_eq!(rel(r"C:\w\app", r"C:\w\app"), None);
    }

    #[test]
    fn recorded_paths_join_as_text() {
        assert_eq!(join_text("/w/app", "src/a.rs"), "/w/app/src/a.rs");
        assert_eq!(join_text("/w/app/", "src/a.rs"), "/w/app/src/a.rs");
        assert_eq!(join_text(r"C:\w\app", "src/a.rs"), r"C:\w\app\src\a.rs");
        assert_eq!(join_text("C:/w/app", "src/a.rs"), "C:/w/app/src/a.rs");
        assert_eq!(join_text("/w/app", "/x.rs"), "/x.rs");
        assert_eq!(join_text("/w/app", r"D:\x.rs"), r"D:\x.rs");
        assert_eq!(join_text("", "a.rs"), "a.rs");
        assert_eq!(join_text("/w", ""), "");
        assert!(is_absolute_text("/x") && is_absolute_text(r"C:\x") && is_absolute_text(r"\\s\x"));
        assert!(!is_absolute_text("x") && !is_absolute_text("C:x"));
    }

    #[test]
    fn memory_tools_are_recognised_in_every_harness_spelling() {
        for name in [
            "mcp__graphersal-memory__remember",
            "mcp__graphersal_memory__remember",
            "graphersal-memory_remember",
            "mcp__graphersal-memory-develop__remember",
            "remember",
        ] {
            assert_eq!(memory_tool(name), Some("remember"), "{name}");
        }
        assert_eq!(memory_tool("graphersal-memory_settle"), Some("settle"));
        for name in [
            "mcp__other__remember",
            "Edit",
            "remembered",
            "my_pin",
            "spin",
            "",
        ] {
            assert_eq!(memory_tool(name), None, "{name}");
        }
        // Inputs: an object or its JSON text; settle names two memories; no slug, no write.
        let w = memory_writes("remember", &json!({"slug": "tabs"}), "t1");
        assert_eq!(
            w,
            vec![MemoryWrite {
                action: "remember".into(),
                slug: "tabs".into(),
                at: "t1".into()
            }]
        );
        let w = memory_writes(
            "x_graphersal_memory__pin",
            &json!("{\"slug\":\"a\",\"pinned\":false}"),
            "",
        );
        assert_eq!(w[0].action, "unpin");
        let w = memory_writes(
            "graphersal-memory_settle",
            &json!({"slug": "a", "other": "b", "decision": "both"}),
            "",
        );
        assert_eq!(
            w.iter().map(|w| w.slug.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!(memory_writes("remember", &json!({"summary": "no slug"}), "").is_empty());
        assert!(memory_writes("Edit", &json!({"slug": "x"}), "").is_empty());
    }

    #[test]
    fn a_claude_code_transcript_records_the_memories_it_wrote() {
        let mut text = fixture();
        text.push_str(&format!(
            "{}\n",
            json!({"type": "assistant", "sessionId": "s-1", "timestamp": "2026-01-02T11:00:09.000Z",
                   "message": {"content": [
                       {"type": "tool_use", "name": "mcp__graphersal-memory__remember",
                        "input": {"slug": "parser-cache", "kind": "project", "summary": "LRU"}},
                       {"type": "tool_use", "name": "mcp__graphersal-memory__pin",
                        "input": {"slug": "parser-cache"}},
                       {"type": "tool_use", "name": "mcp__other-server__remember",
                        "input": {"slug": "not-ours"}}]}})
        ));
        let t = parse(text.as_bytes(), "x");
        let got: Vec<(&str, &str, &str)> = t
            .memory_writes
            .iter()
            .map(|w| (w.action.as_str(), w.slug.as_str(), w.at.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("remember", "parser-cache", "2026-01-02T11:00:09.000Z"),
                ("pin", "parser-cache", "2026-01-02T11:00:09.000Z"),
            ]
        );
        // The fixture alone wrote no memory.
        assert!(parse(fixture().as_bytes(), "x").memory_writes.is_empty());
    }
}
