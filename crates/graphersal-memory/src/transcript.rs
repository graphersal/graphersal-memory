//! The Claude Code transcript parser: one `<claude config>/projects/<project>/<session>.jsonl`
//! file (one JSON record per line) becomes a [`Transcript`]: the session's id, working
//! directory, title, times, branches, files edited, the harness's own compaction summaries and
//! its turns.
//!
//! A turn is one prompt of the user (with the messages sent while the assistant worked and the
//! answers to its questions) and the assistant's text replies until the next prompt, with the
//! names of the tools it used. Tool inputs and tool results are not kept (only the paths of the
//! files the assistant edited). Records of sub-agents (`isSidechain`, and the transcripts in a
//! session's `subagents/` directory, which are never read) are left out, and so are records the
//! harness generates itself (task notifications, command output, reminders, hand-backs of
//! sub-agents).
//!
//! Robustness: a line that is not a JSON object, a record type this version does not know and a
//! last line without its line end (a session still being written) are skipped; the fingerprint
//! covers exactly the complete lines, so a session that grows gets a new one.

use serde_json::Value;

use crate::fsutil;

/// The longest text kept per stored field (characters).
pub const CAP: usize = 60_000;

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

/// One turn of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    /// Its position in the session, from 0.
    pub n: usize,
    /// When the prompt was sent (the record's timestamp, may be empty).
    pub at: String,
    /// The prompt, verbatim.
    pub user: String,
    /// The assistant's text replies, verbatim, separated by blank lines.
    pub assistant: String,
    /// The names of the tools the assistant used, in first-use order.
    pub tools: Vec<String>,
}

/// One parsed transcript.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transcript {
    /// The session id (the records' `sessionId`, else the file name).
    pub session_id: String,
    /// The working directory of the session (its first record that names one).
    pub cwd: Option<String>,
    /// The title the harness generated (any language), or empty.
    pub title: String,
    /// The first timestamp.
    pub started_at: String,
    /// The last timestamp.
    pub ended_at: String,
    /// The git branches seen, in order.
    pub branches: Vec<String>,
    /// The absolute paths of the files the assistant edited, in order.
    pub files: Vec<String>,
    /// The harness's compaction summaries, verbatim (cut at 20 000 characters each).
    pub compact: Vec<String>,
    /// The turns.
    pub turns: Vec<Turn>,
    /// The records read.
    pub records: usize,
    /// Lines skipped: not JSON objects, or a last line without its line end.
    pub skipped_lines: usize,
    /// Whether the last line had no line end (the session is being written).
    pub partial_tail: bool,
    /// The bytes of the complete lines.
    pub bytes: usize,
    /// The fingerprint of the complete lines.
    pub fingerprint: String,
}

/// Cuts `text` to at most `max` characters, keeping the start and the end around ` [...] `.
pub fn clip(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let head = max * 2 / 5;
    let tail = max.saturating_sub(head);
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(count - tail).collect();
    format!("{start} [...] {end}")
}

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
        "assistant" => read_assistant(record, out),
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

fn read_assistant(record: &Value, out: &mut Transcript) {
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

/// `path` relative to the project root `root`, or to the root of a worktree
/// (`.../worktrees/<name>/<rest>`); `None` for a file elsewhere.
pub fn relative_file(path: &str, root: &str) -> Option<String> {
    let marker = "/worktrees/";
    if let Some(at) = path.find(marker)
        && let Some((_, rest)) = path[at + marker.len()..].split_once('/')
        && !rest.is_empty()
    {
        return Some(rest.to_string());
    }
    let root = root.trim_end_matches(['/', '\\']);
    let rest = path.strip_prefix(root)?.strip_prefix(['/', '\\'])?;
    (!rest.is_empty()).then(|| rest.to_string())
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
}
