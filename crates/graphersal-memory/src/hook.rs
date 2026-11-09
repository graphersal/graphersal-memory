//! `graphersal-memory hook session-start|session-end` (hidden): what the hooks `install` adds
//! to Claude Code run.
//!
//! Both import the memory files of the session's project that are new or changed since their
//! last import (a session may still write memory files, for example when the rule block is not
//! followed), through the daemon. `session-start` then prints one status line, which Claude
//! Code adds to the session's context; `session-end` prints nothing. The indexing of finished
//! sessions arrives in stage M4; until then that part does nothing.
//!
//! A hook never blocks or fails the session: the work runs on a thread with a time limit, every
//! error becomes a note on stderr, and the exit code is always 0.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::import::{self, Connect, ImportState};
use crate::install::ClaudePaths;

/// How long a hook may take in all (Claude Code's own limit for it is 30 seconds).
pub const BUDGET: Duration = Duration::from_secs(15);

/// The largest hook input read from stdin.
const MAX_INPUT: u64 = 1024 * 1024;

/// The hook events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum HookEvent {
    /// A session starts (or resumes).
    SessionStart,
    /// A session ends.
    SessionEnd,
}

/// What a hook did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The line for stdout (session start only).
    pub line: Option<String>,
    /// Notes for stderr.
    pub notes: Vec<String>,
}

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
        let memory_dir = paths
            .projects
            .join(import::claude_slug(&dir.display().to_string()))
            .join("memory");
        if memory_dir.is_dir() {
            match sync(&memory_dir, dir, data_dir, connect, deadline) {
                Ok(report) => {
                    imported = report.created + report.updated;
                    outcome.notes.extend(report.problems);
                    if report.timed_out {
                        outcome.notes.push(
                            "the import stopped at the time limit; the rest follows next time"
                                .into(),
                        );
                    }
                }
                Err(err) => outcome.notes.push(err),
            }
        }
    }
    if event == HookEvent::SessionStart {
        let mut line = "graphersal-memory: your memory is served by the MCP server \
                        graphersal-memory (memories(text), recall(text), memory(slug), remember, \
                        forget); write memories there, not into memory files."
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

/// Imports the new and changed files of one memory directory.
fn sync(
    memory_dir: &Path,
    session_dir: &Path,
    data_dir: &Path,
    connect: &mut Connect<'_>,
    deadline: Instant,
) -> Result<import::ImportReport, String> {
    let done = import::with_lock(data_dir, Duration::from_secs(2), || {
        let project = crate::mcp::project_key(None, session_dir);
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

/// Runs a hook with the time limit: never fails, always returns exit code 0.
pub fn run(
    event: HookEvent,
    paths: Result<ClaudePaths, String>,
    data_dir: PathBuf,
    connect: import::SendConnect,
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
                    work(
                        event,
                        dir.as_deref(),
                        &paths,
                        &data_dir,
                        &mut *connect,
                        deadline,
                    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_dir_comes_from_the_input_then_the_environment() {
        assert_eq!(
            session_dir(r#"{"cwd": "/w/a", "session_id": "x"}"#, &|_| None),
            Some(PathBuf::from("/w/a"))
        );
        assert_eq!(
            session_dir("not json", &|name| (name == "CLAUDE_PROJECT_DIR")
                .then(|| "/w/b".to_string())),
            Some(PathBuf::from("/w/b"))
        );
        assert!(session_dir("", &|_| None).is_some());
    }

    #[test]
    fn a_failing_hook_still_exits_0_with_a_note() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(
            HookEvent::SessionStart,
            Err("no home".to_string()),
            PathBuf::from("/nonexistent"),
            Box::new(|_| Err("unreachable".to_string())),
            std::io::empty(),
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        assert!(String::from_utf8(err).unwrap().contains("no home"));
        assert!(out.is_empty());
    }

    #[test]
    fn session_start_prints_one_line_and_session_end_none() {
        let home = crate::mcp_tests::TempDir::new("hookline");
        let paths = ClaudePaths::resolve(&|name| {
            (name == "HOME").then(|| home.path().display().to_string())
        })
        .unwrap();
        let mut connect =
            |_: &str| -> Result<Box<dyn import::Caller>, String> { Err("not called".to_string()) };
        let deadline = Instant::now() + Duration::from_secs(5);
        let start = work(
            HookEvent::SessionStart,
            Some(home.path()),
            &paths,
            home.path(),
            &mut connect,
            deadline,
        );
        let line = start.line.unwrap();
        assert!(line.starts_with("graphersal-memory:"), "{line}");
        assert!(!line.contains('\n'));
        assert!(start.notes.is_empty(), "{:?}", start.notes);
        let end = work(
            HookEvent::SessionEnd,
            Some(home.path()),
            &paths,
            home.path(),
            &mut connect,
            deadline,
        );
        assert_eq!(end, Outcome::default());
    }
}
