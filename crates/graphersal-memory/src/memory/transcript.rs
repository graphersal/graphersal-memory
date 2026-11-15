//! What every harness's session reader shares: a session as [`Transcript`] and its [`Turn`]s,
//! the memory writes through our memory tools ([`memory_tool`], [`memory_writes`]; every
//! harness's spelling), the text helpers (`clip`) and the paths a harness recorded (maybe of
//! another platform: `is_windows_path`, `join_text`, `relative_file`). Claude Code's JSONL
//! parser is `crate::harnesses::claude_code::transcript`.
//!
//! A turn is one prompt of the user (with the messages sent while the assistant worked and the
//! answers to its questions) and the assistant's text replies until the next prompt, with the
//! names of the tools it used. Tool inputs and tool results are not kept (only the paths of the
//! files the assistant edited, and the slugs of the memories it wrote through our memory tools,
//! [`MemoryWrite`]).

use serde_json::Value;

/// The longest text kept per stored field (characters).
pub const CAP: usize = 60_000;

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

/// One call of a graphersal-memory memory tool that names a memory (`remember`, `forget`,
/// `pin`/unpin, `settle`), as a session's transcript records it: the link between the session
/// and the memory (`crate::memory::sessions`, the `wrote` edge). A `settle` names two memories (`slug`
/// and `other`): one write each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryWrite {
    /// `remember`, `forget`, `pin`, `unpin` or `settle`.
    pub action: String,
    /// The memory's slug.
    pub slug: String,
    /// When (the record's timestamp; may be empty).
    pub at: String,
}

/// The memory tool a harness's tool name calls, when it is one of graphersal-memory's (either
/// server, `use` or `develop`): the name ends in `remember`, `forget`, `pin` or `settle`, and
/// what comes before is nothing (Cursor records bare names) or names the server, in any
/// harness's spelling (`mcp__graphersal-memory__remember` (Claude Code),
/// `mcp__graphersal_memory__remember` (Codex), `graphersal-memory_remember` (opencode)).
pub fn memory_tool(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase().replace('-', "_");
    for tool in ["remember", "forget", "pin", "settle"] {
        let Some(prefix) = lower.strip_suffix(tool) else {
            continue;
        };
        if prefix.is_empty() {
            return Some(tool);
        }
        let Some(server) = prefix.strip_suffix(['_', '.', '/', ':']) else {
            continue;
        };
        if server.contains("graphersal") && server.contains("memory") {
            return Some(tool);
        }
    }
    None
}

/// The memory writes of one tool call: `name` the harness's tool name, `input` its arguments
/// (an object, or its JSON text as Codex records it), `at` the record's time. Nothing for
/// another tool or a call without a `slug`.
pub fn memory_writes(name: &str, input: &Value, at: &str) -> Vec<MemoryWrite> {
    let Some(tool) = memory_tool(name) else {
        return Vec::new();
    };
    let parsed;
    let input = match input {
        Value::String(text) => {
            parsed = serde_json::from_str::<Value>(text).unwrap_or(Value::Null);
            &parsed
        }
        other => other,
    };
    let slug_of = |key: &str| {
        input[key]
            .as_str()
            .map(str::trim)
            .filter(|slug| !slug.is_empty() && slug.len() <= 200)
            .map(str::to_string)
    };
    let action = if tool == "pin" && input["pinned"] == Value::Bool(false) {
        "unpin"
    } else {
        tool
    };
    let mut out = Vec::new();
    for key in ["slug", "other"] {
        if key == "other" && tool != "settle" {
            break;
        }
        if let Some(slug) = slug_of(key) {
            out.push(MemoryWrite {
                action: action.to_string(),
                slug,
                at: at.to_string(),
            });
        }
    }
    out
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
    /// The memories the session wrote through graphersal-memory's memory tools, in order.
    pub memory_writes: Vec<MemoryWrite>,
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

/// `path` relative to the project root `root`, or to the root of a worktree
/// (`.../worktrees/<name>/<rest>`); `None` for a file elsewhere. A Windows path (a drive letter
/// or a UNC share, [`is_windows_path`]) is compared with either separator and without case,
/// and its relative path uses `/`, as on the other platforms.
pub fn relative_file(path: &str, root: &str) -> Option<String> {
    if is_windows_path(root) || is_windows_path(path) {
        let path = path.replace('\\', "/");
        let root = root.replace('\\', "/");
        let root = root.trim_end_matches('/');
        if let Some(rest) = worktree_relative(&path) {
            return Some(rest);
        }
        let head = path.get(..root.len())?;
        if !head.eq_ignore_ascii_case(root) {
            return None;
        }
        let rest = path[root.len()..].strip_prefix('/')?;
        return (!rest.is_empty()).then(|| rest.to_string());
    }
    if let Some(rest) = worktree_relative(path) {
        return Some(rest);
    }
    let root = root.trim_end_matches(['/', '\\']);
    let rest = path.strip_prefix(root)?.strip_prefix(['/', '\\'])?;
    (!rest.is_empty()).then(|| rest.to_string())
}

/// The part of `path` after `.../worktrees/<name>/`, when it has one.
fn worktree_relative(path: &str) -> Option<String> {
    let marker = "/worktrees/";
    if let Some(at) = path.find(marker)
        && let Some((_, rest)) = path[at + marker.len()..].split_once('/')
        && !rest.is_empty()
    {
        return Some(rest.to_string());
    }
    None
}

/// Whether `path` (text another program recorded, not necessarily a path of this machine) is a
/// Windows path: a drive letter (`C:\`, `C:/`) or a UNC share (`\\server`).
pub fn is_windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/'))
        || path.starts_with(r"\\")
}

/// Whether `path` (recorded text) is absolute on the platform it came from: a Unix path from
/// `/`, or a Windows path ([`is_windows_path`]).
pub fn is_absolute_text(path: &str) -> bool {
    path.starts_with('/') || is_windows_path(path)
}

/// The recorded path `file` resolved against the recorded directory `directory` as text, with
/// the directory's separator (`\` for a Windows directory without `/`); an absolute `file`, an
/// empty one or an empty directory stays as it is.
pub fn join_text(directory: &str, file: &str) -> String {
    if file.is_empty() || directory.is_empty() || is_absolute_text(file) {
        return file.to_string();
    }
    let separator = if is_windows_path(directory) && !directory.contains('/') {
        '\\'
    } else {
        '/'
    };
    let file = if separator == '\\' {
        file.replace('/', "\\")
    } else {
        file.to_string()
    };
    format!(
        "{}{separator}{}",
        directory.trim_end_matches(['/', '\\']),
        file
    )
}
