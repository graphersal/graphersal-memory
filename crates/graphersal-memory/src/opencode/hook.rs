//! opencode's hook (`graphersal-memory hook ... --harness opencode`, `crate::hook`): its input read,
//! its session queued for the indexing.

#![allow(clippy::wildcard_imports)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::hook::*;

/// The indexing request of opencode's plugin: every session of `db`, with the plugin's own
/// session active (start) or finished (end).
pub fn opencode_request(
    event: HookEvent,
    db: &Path,
    session_id: Option<&str>,
) -> crate::sessions::Request {
    let mut request = crate::sessions::Request::for_harness(crate::agents::Kind::OpenCode, db);
    if let Some(id) = session_id.filter(|id| id.starts_with("ses_")) {
        let unit = crate::opencode::sessions::unit_path(db, id);
        match event {
            HookEvent::SessionStart => request.active.insert(unit),
            HookEvent::SessionEnd => request.ended.insert(unit),
        };
    }
    request
}

/// `hook session-start|session-end --harness opencode`: queues the indexing within the time
/// limit, prints the start line; never fails (exit code 0).
pub fn run_opencode(
    event: HookEvent,
    db: PathBuf,
    enqueue: Option<Enqueue>,
    stdin: impl Read + Send + 'static,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    run_queued(
        event,
        move |value: &Value| opencode_request(event, &db, value["session_id"].as_str()),
        OPENCODE_START_LINE,
        enqueue,
        stdin,
        stdout,
        stderr,
    )
}
