//! codex's hook (`graphersal-memory hook ... --harness codex`, `crate::hook`): its input read,
//! its session queued for the indexing.

#![allow(clippy::wildcard_imports)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::hook::*;

/// The indexing request of Codex's hook: every rollout under `sessions`, with the hook's own
/// rollout (`transcript_path`) active (start) or finished (end).
pub fn codex_request(
    event: HookEvent,
    sessions: &Path,
    transcript: Option<&str>,
) -> crate::sessions::Request {
    let mut request = crate::sessions::Request::for_harness(crate::agents::Kind::Codex, sessions);
    if let Some(path) = transcript
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && crate::codex::sessions::is_rollout(p))
    {
        match event {
            HookEvent::SessionStart => request.active.insert(path),
            HookEvent::SessionEnd => request.ended.insert(path),
        };
    }
    request
}

/// `hook session-start|session-end --harness codex` (Codex's hooks, input `{"session_id",
/// "transcript_path", "cwd", ...}`): queues the indexing within the time limit, prints the
/// start line; never fails (exit code 0). Codex keeps no memory files, so nothing is imported.
pub fn run_codex(
    event: HookEvent,
    sessions: PathBuf,
    enqueue: Option<Enqueue>,
    stdin: impl Read + Send + 'static,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    run_queued(
        event,
        move |value: &Value| codex_request(event, &sessions, value["transcript_path"].as_str()),
        CODEX_START_LINE,
        enqueue,
        stdin,
        stdout,
        stderr,
    )
}
