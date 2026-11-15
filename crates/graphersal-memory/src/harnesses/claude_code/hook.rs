//! Claude Code's hook (`graphersal-memory hook session-start|session-end`, `crate::memory::hook`): a
//! time-boxed import of the session project's changed memory files, then one `index_sessions`
//! control line to the daemon, and at the start one status line plus the start context.

#![allow(clippy::wildcard_imports)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::install::ClaudePaths;
use crate::memory::hook::*;
use crate::memory::import::{self, Connect, ImportState};

/// The working directory of the session: `cwd` of the hook input, else `CLAUDE_PROJECT_DIR`,
/// else this process's directory.
pub fn session_dir(input: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    serde_json::from_str::<Value>(input)
        .ok()
        .and_then(|v| v.get("cwd").and_then(Value::as_str).map(PathBuf::from))
        .or_else(|| {
            env("CLAUDE_PROJECT_DIR")
                .filter(|d| !d.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| std::env::current_dir().ok())
}

/// The transcript of the session, from the hook input (`transcript_path`).
pub fn transcript_path(input: &str) -> Option<PathBuf> {
    serde_json::from_str::<Value>(input)
        .ok()?
        .get("transcript_path")?
        .as_str()
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// The indexing request of a hook: every transcript under the harness's projects directory,
/// with the session's own transcript as still being written (start) or finished (end).
pub fn index_request(
    event: HookEvent,
    paths: &ClaudePaths,
    transcript: Option<PathBuf>,
) -> crate::memory::sessions::Request {
    let mut request = crate::memory::sessions::Request::new(&paths.projects);
    if let Some(transcript) = transcript {
        match event {
            HookEvent::SessionStart => request.active.insert(transcript),
            HookEvent::SessionEnd => request.ended.insert(transcript),
        };
    }
    request
}

/// The hook's work: imports the session project's changed memory files.
pub fn work(
    event: HookEvent,
    session_dir: Option<&Path>,
    paths: &ClaudePaths,
    data_dir: &Path,
    connect: &mut Connect<'_>,
    deadline: Instant,
) -> Outcome {
    let mut outcome = Outcome::default();
    let mut imported = 0;
    if let Some(dir) = session_dir {
        let (count, notes) = import_project(dir, dir, paths, data_dir, connect, deadline);
        imported = count;
        outcome.notes.extend(notes);
    }
    if event == HookEvent::SessionStart {
        let mut line = "graphersal-memory: your memory is served by the MCP server \
                        graphersal-memory (memories(text), recall(text), memory(slug), remember, \
                        forget; past sessions: sessions(text), session(id)); write memories \
                        there, not into memory files."
            .to_string();
        if imported > 0 {
            line.push_str(&format!(
                " Imported {imported} new or changed memory file{} of this project.",
                if imported == 1 { "" } else { "s" }
            ));
        }
        outcome.line = Some(line);
    }
    outcome
}

/// Imports the new and changed memory files Claude Code keeps for the sessions started in
/// `dir` (`<projects>/<slug of dir>/memory`), as memories of the project of `session_dir`: how
/// many were imported, and notes on what failed (and on changed files whose memory was edited
/// in graphersal-memory, which keep the edit). Nothing when the directory does not exist.
pub fn import_project(
    dir: &Path,
    session_dir: &Path,
    paths: &ClaudePaths,
    data_dir: &Path,
    connect: &mut Connect<'_>,
    deadline: Instant,
) -> (usize, Vec<String>) {
    let memory_dir = paths
        .projects
        .join(import::claude_slug(&dir.display().to_string()))
        .join("memory");
    if !memory_dir.is_dir() {
        return (0, Vec::new());
    }
    match sync(&memory_dir, session_dir, data_dir, connect, deadline) {
        Ok(report) => {
            let mut notes = report.problems;
            // A changed file whose memory was edited here: said once, the edit was kept.
            notes.extend(report.kept_edited);
            if report.timed_out {
                notes.push(
                    "the import stopped at the time limit; the rest follows next time".into(),
                );
            }
            (report.created + report.updated, notes)
        }
        Err(err) => (0, vec![err]),
    }
}

/// Imports the new and changed files of one memory directory.
fn sync(
    memory_dir: &Path,
    session_dir: &Path,
    data_dir: &Path,
    connect: &mut Connect<'_>,
    deadline: Instant,
) -> Result<import::ImportReport, String> {
    let done = import::with_lock(data_dir, Duration::from_secs(2), || {
        let project = crate::runtime::mcp::project_key(None, session_dir);
        let dir = import::read_dir(memory_dir, Some(project));
        let mut state = ImportState::load(data_dir)?;
        let plans = import::plan(vec![dir], &state);
        Ok::<_, String>(import::apply(
            &plans,
            connect,
            &mut state,
            data_dir,
            Some(deadline),
        ))
    })?;
    // `None`: another hook or import runs right now; it imports the same files.
    done.unwrap_or_else(|| Ok(import::ImportReport::default()))
}

/// Runs a hook with the time limit: never fails, always returns exit code 0 (without the
/// indexing step: [`run_with`]).
#[cfg(test)]
pub fn run(
    event: HookEvent,
    paths: Result<ClaudePaths, String>,
    data_dir: PathBuf,
    connect: import::SendConnect,
    stdin: impl Read + Send + 'static,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    run_with(event, paths, data_dir, connect, None, stdin, stdout, stderr)
}

/// [`run`], then `enqueue` the indexing of finished sessions (within the same time limit).
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub fn run_with(
    event: HookEvent,
    paths: Result<ClaudePaths, String>,
    data_dir: PathBuf,
    connect: import::SendConnect,
    enqueue: Option<Enqueue>,
    stdin: impl Read + Send + 'static,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    run_with_context(
        event, paths, data_dir, connect, enqueue, None, stdin, stdout, stderr,
    )
}

/// Claude Code's cap of a hook's `additionalContext` (code.claude.com/docs/en/hooks: 10,000
/// characters; more is saved to a file and only a preview reaches the model).
pub const CLAUDE_CODE_CONTEXT_CHARS: usize = 10_000;

/// Claude Code's SessionStart answer: `line` and the start context `context` (when not empty)
/// as `hookSpecificOutput.additionalContext`, the whole stdout one JSON object (plain text
/// around it would make Claude Code read everything as text). Without a start context: the
/// plain line (Claude Code adds a SessionStart hook's plain stdout to the context as well).
pub fn claude_start_output(line: &str, context: &str) -> String {
    if context.is_empty() {
        return line.to_string();
    }
    serde_json::json!({"hookSpecificOutput": {
        "hookEventName": "SessionStart",
        "additionalContext": format!("{line}\n\n{context}"),
    }})
    .to_string()
}

/// [`run_with`] with the start context from `fetch` (session start only) in the answer
/// ([`claude_start_output`]).
#[allow(clippy::too_many_arguments)]
pub fn run_with_context(
    event: HookEvent,
    paths: Result<ClaudePaths, String>,
    data_dir: PathBuf,
    connect: import::SendConnect,
    enqueue: Option<Enqueue>,
    fetch: Option<StartFetch>,
    stdin: impl Read + Send + 'static,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    let started = Instant::now();
    let deadline = started + BUDGET - Duration::from_secs(2);
    let (done, result) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("hook".into())
        .spawn(move || {
            let mut input = String::new();
            let _ = stdin.take(MAX_INPUT).read_to_string(&mut input);
            let dir = session_dir(&input, &|name| std::env::var(name).ok());
            let outcome = match paths {
                Ok(paths) => {
                    let mut connect = connect;
                    let mut outcome = work(
                        event,
                        dir.as_deref(),
                        &paths,
                        &data_dir,
                        &mut *connect,
                        deadline,
                    );
                    // After the import, so memories it just brought in are part of it.
                    if let (Some(fetch), Some(line)) = (fetch, outcome.line.clone()) {
                        let room =
                            CLAUDE_CODE_CONTEXT_CHARS.saturating_sub(line.chars().count() + 2);
                        match fetch(dir.as_deref(), room) {
                            Ok(context) => {
                                // The conflict note only when it still fits the cap (the
                                // start context itself names the conflicts too).
                                let noted = format!("{line}{}", context.start_note());
                                let line = if noted.chars().count() + 2 + context.chars()
                                    <= CLAUDE_CODE_CONTEXT_CHARS
                                {
                                    noted
                                } else {
                                    line
                                };
                                outcome.line = Some(claude_start_output(&line, &context.text));
                            }
                            Err(err) => outcome
                                .notes
                                .push(format!("cannot read the start context: {err}")),
                        }
                    }
                    if let Some(enqueue) = enqueue {
                        let request = index_request(event, &paths, transcript_path(&input));
                        if let Err(err) = enqueue(request) {
                            outcome
                                .notes
                                .push(format!("cannot queue the session indexing: {err}"));
                        }
                    }
                    outcome
                }
                Err(err) => Outcome {
                    line: None,
                    notes: vec![err],
                },
            };
            let _ = done.send(outcome);
        });
    let outcome = match spawned {
        Ok(_) => match result.recv_timeout(BUDGET.saturating_sub(started.elapsed())) {
            Ok(outcome) => outcome,
            Err(_) => Outcome {
                line: (event == HookEvent::SessionStart).then(|| {
                    "graphersal-memory: your memory is served by the MCP server graphersal-memory \
                     (the memory sync did not finish in time)."
                        .to_string()
                }),
                notes: vec![format!(
                    "the hook stopped after {}s; the memory daemon may still be starting",
                    BUDGET.as_secs()
                )],
            },
        },
        Err(err) => Outcome {
            line: None,
            notes: vec![format!("cannot start the hook: {err}")],
        },
    };
    if let Some(line) = &outcome.line {
        let _ = writeln!(stdout, "{line}");
        let _ = stdout.flush();
    }
    for note in &outcome.notes {
        let _ = writeln!(stderr, "graphersal-memory hook: {note}");
    }
    0
}
