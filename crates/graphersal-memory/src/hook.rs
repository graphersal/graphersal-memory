//! `graphersal-memory hook session-start|session-end` (hidden): what the hooks `install` adds
//! to Claude Code run.
//!
//! Both import the memory files of the session's project that are new or changed since their
//! last import (a session may still write memory files, for example when the rule block is not
//! followed), through the daemon. `session-start` then prints one status line, which Claude
//! Code adds to the session's context; `session-end` prints nothing.
//!
//! Both then ask the daemon to index finished sessions in the background (one control line,
//! answered at once; `crate::sessions`): `session-start` names its own transcript as still
//! being written, `session-end` names it as finished. The indexing (and the summaries, when the
//! user opted in) runs in the daemon, never in the hook.
//!
//! opencode's plugin (`crate::opencode`) runs the same subcommand with `--harness opencode`
//! ([`run_opencode`]): its input is `{"session_id", "cwd"}`; there is no file memory to import,
//! so it only queues the indexing (its session `<opencode.db>#<id>` active at the start, ended
//! at the end) and prints the start line, which the plugin shows as a toast.
//!
//! Codex's hooks (`crate::codex`) run it with `--harness codex` ([`run_codex`]): its input
//! names the session's rollout file (`transcript_path`); Codex keeps no memory files, so it only
//! queues the indexing (the rollout active at the start, ended at the end) and prints the start
//! line, which Codex adds to the session's context.
//!
//! A hook never blocks or fails the session: the work runs on a thread with a time limit, every
//! error becomes a note on stderr, and the exit code is always 0. With
//! `GRAPHERSAL_MEMORY_HOOK=off` (set for the summaries' own harness runs) a hook does nothing.

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
) -> crate::sessions::Request {
    let mut request = crate::sessions::Request::new(&paths.projects);
    if let Some(transcript) = transcript {
        match event {
            HookEvent::SessionStart => request.active.insert(transcript),
            HookEvent::SessionEnd => request.ended.insert(transcript),
        };
    }
    request
}

/// Sends an indexing request to the daemon (the hook's last step).
pub type Enqueue = Box<dyn FnOnce(crate::sessions::Request) -> Result<(), String> + Send>;

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

/// The session start line for opencode (its plugin shows it as a toast).
pub const OPENCODE_START_LINE: &str = "graphersal-memory: your memory is served by the MCP \
    server graphersal-memory (memories, recall, memory, remember, forget; past sessions: \
    sessions, session).";

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

/// The session start line for Codex (its `SessionStart` hook adds it to the model's context).
pub const CODEX_START_LINE: &str = "graphersal-memory: your memory is served by the MCP server \
    graphersal-memory (tools mcp__graphersal_memory__memories, recall, memory, remember, forget; \
    past sessions: sessions, session).";

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

/// A hook that only queues the indexing: `request` of the input, `start_line` at the start.
fn run_queued(
    event: HookEvent,
    request: impl FnOnce(&Value) -> crate::sessions::Request + Send + 'static,
    start_line: &str,
    enqueue: Option<Enqueue>,
    stdin: impl Read + Send + 'static,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    let (done, result) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("hook".into())
        .spawn(move || {
            let mut input = String::new();
            let _ = stdin.take(MAX_INPUT).read_to_string(&mut input);
            let value: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
            let request = request(&value);
            let mut notes = Vec::new();
            if let Some(enqueue) = enqueue
                && let Err(err) = enqueue(request)
            {
                notes.push(format!("cannot queue the session indexing: {err}"));
            }
            let _ = done.send(notes);
        });
    let notes = match spawned {
        Ok(_) => result.recv_timeout(BUDGET).unwrap_or_else(|_| {
            vec![format!(
                "the hook stopped after {}s; the memory daemon may still be starting",
                BUDGET.as_secs()
            )]
        }),
        Err(err) => vec![format!("cannot start the hook: {err}")],
    };
    if event == HookEvent::SessionStart {
        let _ = writeln!(stdout, "{start_line}");
        let _ = stdout.flush();
    }
    for note in &notes {
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
    fn the_index_request_names_the_sessions_own_transcript() {
        let paths =
            ClaudePaths::resolve(&|name| (name == "HOME").then(|| "/h".to_string())).unwrap();
        let input = r#"{"session_id": "s", "transcript_path": "/h/.claude/projects/p/s.jsonl"}"#;
        let transcript = transcript_path(input);
        assert_eq!(
            transcript,
            Some(PathBuf::from("/h/.claude/projects/p/s.jsonl"))
        );
        let start = index_request(HookEvent::SessionStart, &paths, transcript.clone());
        assert_eq!(start.projects_dir, paths.projects);
        assert!(start.active.contains(transcript.as_ref().unwrap()));
        assert!(start.ended.is_empty());
        let end = index_request(HookEvent::SessionEnd, &paths, transcript.clone());
        assert!(end.ended.contains(transcript.as_ref().unwrap()));
        assert_eq!(transcript_path("{}"), None);
        assert_eq!(transcript_path("nope"), None);
    }

    #[test]
    fn a_hook_queues_the_indexing_and_returns_at_once() {
        let home = crate::mcp_tests::TempDir::new("hookq");
        let paths = ClaudePaths::resolve(&|name| {
            (name == "HOME").then(|| home.path().display().to_string())
        })
        .unwrap();
        let (sent, received) = mpsc::channel();
        let enqueue: Enqueue = Box::new(move |request| {
            let _ = sent.send(request);
            Ok(())
        });
        let mut out = Vec::new();
        let mut err = Vec::new();
        let started = Instant::now();
        let code = run_with(
            HookEvent::SessionEnd,
            Ok(paths.clone()),
            home.path().to_path_buf(),
            Box::new(|_| Err("not called".to_string())),
            Some(enqueue),
            std::io::Cursor::new(
                br#"{"cwd": "/nowhere", "transcript_path": "/t/x.jsonl"}"#.to_vec(),
            ),
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        assert!(started.elapsed() < Duration::from_secs(5));
        let request = received.try_recv().unwrap();
        assert!(request.ended.contains(Path::new("/t/x.jsonl")));
        // A failing queue is a note, never a failure.
        let mut err = Vec::new();
        let code = run_with(
            HookEvent::SessionStart,
            Ok(paths),
            home.path().to_path_buf(),
            Box::new(|_| Err("not called".to_string())),
            Some(Box::new(|_| Err("the daemon is away".to_string()))),
            std::io::empty(),
            &mut Vec::new(),
            &mut err,
        );
        assert_eq!(code, 0);
        assert!(
            String::from_utf8(err)
                .unwrap()
                .contains("cannot queue the session indexing: the daemon is away")
        );
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

    #[test]
    fn the_opencode_hook_queues_its_session_and_prints_one_line_at_the_start() {
        use std::sync::{Arc, Mutex};
        let db = PathBuf::from("/d/opencode.db");
        for (event, lines) in [(HookEvent::SessionStart, 1), (HookEvent::SessionEnd, 0)] {
            let seen: Arc<Mutex<Option<crate::sessions::Request>>> = Arc::default();
            let keep = seen.clone();
            let enqueue: Enqueue = Box::new(move |request| {
                *keep.lock().unwrap() = Some(request);
                Ok(())
            });
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let input = br#"{"session_id": "ses_abc", "cwd": "/w/app"}"#.as_slice();
            let code = run_opencode(event, db.clone(), Some(enqueue), input, &mut out, &mut err);
            assert_eq!(code, 0);
            let out = String::from_utf8(out).unwrap();
            assert_eq!(out.lines().count(), lines, "{out}");
            assert!(err.is_empty());
            let request = seen.lock().unwrap().clone().unwrap();
            assert_eq!(request.harness, crate::agents::Kind::OpenCode);
            let unit = crate::opencode::sessions::unit_path(&db, "ses_abc");
            match event {
                HookEvent::SessionStart => assert!(request.active.contains(&unit)),
                HookEvent::SessionEnd => assert!(request.ended.contains(&unit)),
            }
        }
        // A failing daemon is a note, never a failure; bad input queues the whole database.
        let enqueue: Enqueue = Box::new(|_| Err("no daemon".to_string()));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_opencode(
            HookEvent::SessionEnd,
            db,
            Some(enqueue),
            b"not json".as_slice(),
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        assert!(String::from_utf8(err).unwrap().contains("no daemon"));
    }

    #[test]
    fn the_codex_hook_queues_its_rollout_and_adds_one_line_at_the_start() {
        use std::sync::{Arc, Mutex};
        let sessions = PathBuf::from("/c/.codex/sessions");
        let rollout = "/c/.codex/sessions/2026/11/09/rollout-2026-11-09T10-00-00-abc.jsonl";
        for (event, lines) in [(HookEvent::SessionStart, 1), (HookEvent::SessionEnd, 0)] {
            let seen: Arc<Mutex<Option<crate::sessions::Request>>> = Arc::default();
            let keep = seen.clone();
            let enqueue: Enqueue = Box::new(move |request| {
                *keep.lock().unwrap() = Some(request);
                Ok(())
            });
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let input = serde_json::json!({"session_id": "abc", "transcript_path": rollout,
                "cwd": "/w/app", "hook_event_name": "SessionStart", "model": "gpt-5.5"})
            .to_string();
            let code = run_codex(
                event,
                sessions.clone(),
                Some(enqueue),
                std::io::Cursor::new(input.into_bytes()),
                &mut out,
                &mut err,
            );
            assert_eq!(code, 0);
            let out = String::from_utf8(out).unwrap();
            assert_eq!(out.lines().count(), lines, "{out}");
            if lines == 1 {
                assert!(out.contains("mcp__graphersal_memory__memories"), "{out}");
            }
            let request = seen.lock().unwrap().clone().unwrap();
            assert_eq!(request.harness, crate::agents::Kind::Codex);
            assert_eq!(request.projects_dir, sessions);
            let unit = PathBuf::from(rollout);
            match event {
                HookEvent::SessionStart => assert!(request.active.contains(&unit)),
                HookEvent::SessionEnd => assert!(request.ended.contains(&unit)),
            }
        }
        // No rollout named (or not a rollout): every rollout is queued, none marked.
        let request = codex_request(HookEvent::SessionEnd, &sessions, Some("/etc/passwd"));
        assert!(request.ended.is_empty() && request.active.is_empty());
    }
}
