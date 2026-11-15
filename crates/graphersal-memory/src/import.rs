//! What every importer and the export share: the connection to the daemon (`Caller`,
//! `Connect`), memory slugs, and the memory-file format (front matter, `meaning`). Claude
//! Code's file-memory import is `crate::claude_code::import`, re-exported here under its old
//! paths.

use serde_json::Value;

pub use crate::claude_code::import::*;

/// The memory kinds of the memory schema.
pub const KINDS: [&str; 4] = ["user", "feedback", "project", "reference"];

/// A memory slug from a file name or a link: lower case, characters other than `a-z`, `0-9`
/// and `-` become `-`, repeated and outer dashes removed.
pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        let c = c.to_ascii_lowercase();
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() {
            c
        } else {
            '-'
        };
        if c == '-' && (out.is_empty() || out.ends_with('-')) {
            continue;
        }
        out.push(c);
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// The front matter (between the first two `---` lines) and the rest.
pub(crate) fn split_front_matter(text: &str) -> (&str, &str) {
    if let Some(rest) = text.strip_prefix("---\n")
        && let Some(end) = rest.find("\n---")
    {
        let front = &rest[..end];
        let after = &rest[end + 4..];
        let after = after.split_once('\n').map_or("", |(_, body)| body);
        return (front, after);
    }
    ("", text)
}

/// A front-matter field (top-level or nested one level, as `metadata.type`), unquoted; a
/// folded or literal block (`>`, `>-`, `|`, `|-`) joined from its indented lines.
pub(crate) fn field(front: &str, key: &str) -> Option<String> {
    let lines: Vec<&str> = front.lines().collect();
    lines.iter().enumerate().find_map(|(at, line)| {
        let (name, value) = line.trim().split_once(':')?;
        if name.trim() != key {
            return None;
        }
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        if matches!(value, ">" | ">-" | ">+" | "|" | "|-" | "|+") {
            let indent = line.len() - line.trim_start().len();
            let block: Vec<&str> = lines[at + 1..]
                .iter()
                .take_while(|next| {
                    next.trim().is_empty() || next.len() - next.trim_start().len() > indent
                })
                .map(|next| next.trim())
                .collect();
            let separator = if value.starts_with('>') { " " } else { "\n" };
            let joined = block.join(separator).trim().to_string();
            return (!joined.is_empty()).then_some(joined);
        }
        Some(unquote(value))
    })
}

/// What a memory file says, for the export's comparison: its kind, summary and body (after
/// the front matter, line ends and blank lines normalized). Two files that say the same give
/// the same value whatever their formatting (CRLF, a blank line more, folded YAML, metadata
/// keys of others).
pub fn meaning(text: &str) -> (String, String, String) {
    let normalized = text.replace("\r\n", "\n");
    let (front, body) = split_front_matter(&normalized);
    // As the import reads it: an unknown kind is `project`, a missing description the body's
    // first line.
    let kind = field(front, "type")
        .map(|kind| kind.to_ascii_lowercase())
        .filter(|kind| KINDS.contains(&kind.as_str()))
        .unwrap_or_else(|| "project".to_string());
    let mut summary = field(front, "description").unwrap_or_default();
    if summary.is_empty() {
        summary = body
            .lines()
            .map(|line| line.trim().trim_start_matches('#').trim())
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .chars()
            .take(200)
            .collect();
    }
    let mut lines: Vec<&str> = Vec::new();
    for line in body.trim().lines().map(str::trim_end) {
        if line.is_empty() && lines.last().is_some_and(|l| l.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    // A blank line between the why and how-to-apply lines (or none) says the same.
    let body = lines
        .join("\n")
        .replace("\n\n**How to apply:**", "\n**How to apply:**")
        .replace("**Why:**\n", "**Why:** ")
        .replace("**How to apply:**\n", "**How to apply:** ");
    (kind, summary, body)
}

pub(crate) fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 && bytes[0] == bytes[bytes.len() - 1] && matches!(bytes[0], b'"' | b'\'') {
        let inner = &value[1..value.len() - 1];
        if bytes[0] == b'"' {
            return inner.replace("\\\"", "\"").replace("\\\\", "\\");
        }
        return inner.replace("''", "'");
    }
    value.to_string()
}

/// A connection to the memory functions, bound to one project.
pub trait Caller {
    /// Calls the memory function `name`.
    fn call(&mut self, name: &str, arguments: Value) -> Result<Value, String>;
}

impl<R: std::io::BufRead, W: std::io::Write> Caller for crate::client::McpClient<R, W> {
    fn call(&mut self, name: &str, arguments: Value) -> Result<Value, String> {
        crate::client::McpClient::call(self, name, arguments)
    }
}

/// Opens a connection bound to a project.
pub type Connect<'a> = dyn FnMut(&str) -> Result<Box<dyn Caller>, String> + 'a;

/// An owned connector that can move to another thread.
pub type SendConnect = Box<dyn FnMut(&str) -> Result<Box<dyn Caller>, String> + Send>;
