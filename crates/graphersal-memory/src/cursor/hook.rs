//! cursor's hook (`graphersal-memory hook ... --harness cursor`, `crate::hook`): its input read,
//! its session queued for the indexing.

#![allow(clippy::wildcard_imports)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::hook::*;

/// The workspace of a Cursor hook's input: the first of `workspace_roots`, else `fallback`
/// (`CURSOR_PROJECT_DIR`).
pub fn cursor_workspace(value: &Value, fallback: Option<&str>) -> Option<String> {
    value["workspace_roots"]
        .as_array()
        .and_then(|roots| {
            roots
                .iter()
                .filter_map(Value::as_str)
                .find(|r| !r.is_empty())
        })
        .or(fallback.filter(|d| !d.is_empty()))
        .map(str::to_string)
}

/// What Cursor's hook does with its input: records the transcript with its workspace in
/// `registry` and returns the indexing request (the transcript active at the start, ended at
/// the end), with a note when the recording failed.
pub fn cursor_request(
    event: HookEvent,
    registry: &Path,
    value: &Value,
    project_dir: Option<&str>,
) -> (crate::sessions::Request, Vec<String>) {
    let mut request = crate::sessions::Request::for_harness(crate::agents::Kind::Cursor, registry);
    let mut notes = Vec::new();
    let transcript = value["transcript_path"].as_str().map(PathBuf::from);
    let session = value["session_id"]
        .as_str()
        .or_else(|| value["conversation_id"].as_str())
        .unwrap_or_default();
    match (transcript, cursor_workspace(value, project_dir)) {
        (Some(path), Some(workspace)) if crate::cursor::sessions::is_transcript(&path) => {
            if let Err(err) = crate::cursor::sessions::record(registry, &path, &workspace, session)
            {
                notes.push(format!("cannot record the transcript: {err}"));
            }
            match event {
                HookEvent::SessionStart => request.active.insert(path),
                HookEvent::SessionEnd => request.ended.insert(path),
            };
        }
        (None, _) => notes.push(
            "Cursor sent no transcript_path (transcripts are off?): this session is not indexed"
                .to_string(),
        ),
        _ => {}
    }
    (request, notes)
}

/// `hook session-start|session-end --harness cursor` (Cursor's hooks, input
/// `{"conversation_id", "session_id", "transcript_path", "workspace_roots", ...}`): records the
/// transcript, queues the indexing within the time limit, answers the start with
/// `{"additional_context": ...}`; never fails (exit code 0). Cursor keeps no memory files, so
/// nothing is imported.
#[cfg(test)]
pub fn run_cursor(
    event: HookEvent,
    registry: PathBuf,
    project_dir: Option<String>,
    enqueue: Option<Enqueue>,
    stdin: impl Read + Send + 'static,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    run_cursor_with_context(
        event,
        registry,
        project_dir,
        enqueue,
        None,
        stdin,
        stdout,
        stderr,
    )
}

/// Cursor's sessionStart answer: the start line and the start context (when not empty) as
/// `additional_context` (cursor.com/docs/hooks: "Additional context to add to the
/// conversation's initial system context").
pub fn cursor_start_output(context: &str) -> String {
    let text = if context.is_empty() {
        CURSOR_START_LINE.to_string()
    } else {
        format!("{CURSOR_START_LINE}\n\n{context}")
    };
    serde_json::json!({ "additional_context": text }).to_string()
}

/// [`run_cursor`] with the start context of the workspace from `fetch` in the start answer.
#[allow(clippy::too_many_arguments)]
pub fn run_cursor_with_context(
    event: HookEvent,
    registry: PathBuf,
    project_dir: Option<String>,
    enqueue: Option<Enqueue>,
    fetch: Option<StartFetch>,
    stdin: impl Read + Send + 'static,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    let workspace_from = project_dir.clone();
    run_queued_inner(
        event,
        move |value: &Value| cursor_request(event, &registry, value, project_dir.as_deref()),
        move |value: &Value, notes: &mut Vec<String>| {
            let mut context = String::new();
            if let Some(fetch) = fetch {
                let workspace =
                    cursor_workspace(value, workspace_from.as_deref()).map(PathBuf::from);
                // No documented cap of `additional_context`: the budget of config.json alone.
                match fetch(workspace.as_deref(), usize::MAX) {
                    Ok(found) => context = found.text,
                    Err(err) => notes.push(format!("cannot read the start context: {err}")),
                }
            }
            cursor_start_output(&context)
        },
        cursor_start_output(""),
        enqueue,
        stdin,
        stdout,
        stderr,
    )
}
