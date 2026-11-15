//! `graphersal-memory hook session-start|session-end` (hidden): what every harness's hook
//! shares (the events, the input read, the queueing of the indexing, `run_queued*`). Each
//! harness's own hook is its `HookHandler` (`crate::agents`): Claude Code's in
//! `crate::claude_code::hook`, opencode's, Codex's and Cursor's in `<harness>/hook.rs`.
//!
//! Claude Code's hooks:
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
//! Cursor's hooks (`crate::cursor`) run it with `--harness cursor` ([`run_cursor`]): its input
//! names the conversation's transcript (`transcript_path`) and the workspace
//! (`workspace_roots`); the hook records the pair in the registry of Cursor's transcripts
//! (`crate::cursor::sessions::record`), queues the indexing (the transcript active at the
//! start, ended at the end) and answers the start with `{"additional_context": ...}`.
//!
//! Cursor also runs Claude Code's hooks (its "Third Party Hooks", on by default). A Claude Code
//! hook whose input carries Cursor's `cursor_version` does nothing ([`is_cursor_input`]): the
//! Cursor hooks do Cursor's part, and a Claude Code import or indexing must not run on a Cursor
//! session.
//!
//! The hooks are optional: the MCP server does the same import and queues the same indexing at
//! a session's start and end (`crate::session_sync`); when both run, the import's fingerprints
//! and the daemon's merged, fingerprinted indexing make the second run a no-op.
//!
//! A hook never blocks or fails the session: the work runs on a thread with a time limit, every
//! error becomes a note on stderr, and the exit code is always 0. With
//! `GRAPHERSAL_MEMORY_HOOK=off` (set for the summaries' own harness runs) a hook does nothing.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;

/// How long a hook may take in all (Claude Code's own limit for it is 30 seconds).
pub const BUDGET: Duration = Duration::from_secs(15);

/// The largest hook input read from stdin.
pub const MAX_INPUT: u64 = 1024 * 1024;

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

/// Sends an indexing request to the daemon (the hook's last step).
pub type Enqueue = Box<dyn FnOnce(crate::sessions::Request) -> Result<(), String> + Send>;

/// Reads the start context of the session started in a directory (`None`: no directory
/// known: the global memories only) at most `max_chars` characters long.
pub type StartFetch = Box<
    dyn FnOnce(Option<&Path>, usize) -> Result<crate::start_context::StartContext, String> + Send,
>;

/// Reads a hook's input (at most [`MAX_INPUT`] bytes) within [`BUDGET`]; what came until then
/// when the harness keeps stdin open longer.
pub fn read_input(stdin: impl Read + Send + 'static) -> Vec<u8> {
    let (done, result) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("hook-input".into())
        .spawn(move || {
            let mut input = Vec::new();
            let _ = stdin.take(MAX_INPUT).read_to_end(&mut input);
            let _ = done.send(input);
        });
    match spawned {
        Ok(_) => result.recv_timeout(BUDGET).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// A hook that only queues the indexing: `request` of the input, `start_line` at the start.
pub(crate) fn run_queued(
    event: HookEvent,
    request: impl FnOnce(&Value) -> crate::sessions::Request + Send + 'static,
    start_line: &str,
    enqueue: Option<Enqueue>,
    stdin: impl Read + Send + 'static,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    run_queued_with(
        event,
        move |value: &Value| (request(value), Vec::new()),
        start_line,
        enqueue,
        stdin,
        stdout,
        stderr,
    )
}

/// [`run_queued`] with notes from `request` (written to stderr like the hook's own).
pub(crate) fn run_queued_with(
    event: HookEvent,
    request: impl FnOnce(&Value) -> (crate::sessions::Request, Vec<String>) + Send + 'static,
    start_line: &str,
    enqueue: Option<Enqueue>,
    stdin: impl Read + Send + 'static,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    let line = start_line.to_string();
    run_queued_inner(
        event,
        request,
        move |_: &Value, _: &mut Vec<String>| line,
        start_line.to_string(),
        enqueue,
        stdin,
        stdout,
        stderr,
    )
}

/// [`run_queued_with`] with the start answer made from the input by `start` (after the
/// indexing was queued), `fallback` when the hook runs out of time.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_queued_inner(
    event: HookEvent,
    request: impl FnOnce(&Value) -> (crate::sessions::Request, Vec<String>) + Send + 'static,
    start: impl FnOnce(&Value, &mut Vec<String>) -> String + Send + 'static,
    fallback: String,
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
            let (request, mut notes) = request(&value);
            if let Some(enqueue) = enqueue
                && let Err(err) = enqueue(request)
            {
                notes.push(format!("cannot queue the session indexing: {err}"));
            }
            let answer = if event == HookEvent::SessionStart {
                start(&value, &mut notes)
            } else {
                String::new()
            };
            let _ = done.send((notes, answer));
        });
    let (notes, answer) = match spawned {
        Ok(_) => result.recv_timeout(BUDGET).unwrap_or_else(|_| {
            (
                vec![format!(
                    "the hook stopped after {}s; the memory daemon may still be starting",
                    BUDGET.as_secs()
                )],
                fallback,
            )
        }),
        Err(err) => (vec![format!("cannot start the hook: {err}")], fallback),
    };
    if event == HookEvent::SessionStart {
        let _ = writeln!(stdout, "{answer}");
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
    use std::path::PathBuf;
    use std::time::Instant;

    use crate::claude_code::hook::*;
    use crate::codex::hook::{codex_request, run_codex};
    use crate::cursor::hook::{CURSOR_START_LINE, is_cursor_input};
    use crate::cursor::hook::{cursor_request, run_cursor, run_cursor_with_context};
    use crate::import;
    use crate::install::ClaudePaths;
    use crate::opencode::hook::run_opencode;

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
    fn the_cursor_hook_records_its_transcript_and_answers_json_at_the_start() {
        use std::sync::{Arc, Mutex};
        let tmp = crate::mcp_tests::TempDir::new("cursor-hook");
        let registry = tmp.path().join("data/sessions/cursor-transcripts.json");
        let transcript = tmp
            .path()
            .join("cursor/projects/w-app/agent-transcripts/c1/c1.jsonl");
        // Cursor's input: the common fields and sessionStart's own (cursor.com/docs/hooks).
        let input = serde_json::json!({
            "conversation_id": "c1", "generation_id": "g1", "model": "auto",
            "hook_event_name": "sessionStart", "cursor_version": "2.6.0",
            "workspace_roots": ["/w/app"], "user_email": null,
            "transcript_path": transcript.display().to_string(),
            "session_id": "c1", "is_background_agent": false, "composer_mode": "agent",
        })
        .to_string();
        assert!(is_cursor_input(&input));
        assert!(!is_cursor_input(r#"{"session_id": "x", "cwd": "/w"}"#));
        for (event, lines) in [(HookEvent::SessionStart, 1), (HookEvent::SessionEnd, 0)] {
            let seen: Arc<Mutex<Option<crate::sessions::Request>>> = Arc::default();
            let keep = seen.clone();
            let enqueue: Enqueue = Box::new(move |request| {
                *keep.lock().unwrap() = Some(request);
                Ok(())
            });
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = run_cursor(
                event,
                registry.clone(),
                None,
                Some(enqueue),
                std::io::Cursor::new(input.clone().into_bytes()),
                &mut out,
                &mut err,
            );
            assert_eq!(code, 0);
            let out = String::from_utf8(out).unwrap();
            assert_eq!(out.lines().count(), lines, "{out}");
            if lines == 1 {
                let answer: Value = serde_json::from_str(&out).unwrap();
                assert_eq!(answer["additional_context"], CURSOR_START_LINE);
            }
            assert!(err.is_empty(), "{}", String::from_utf8_lossy(&err));
            let request = seen.lock().unwrap().clone().unwrap();
            assert_eq!(request.harness, crate::agents::Kind::Cursor);
            assert_eq!(request.projects_dir, registry);
            match event {
                HookEvent::SessionStart => assert!(request.active.contains(&transcript)),
                HookEvent::SessionEnd => assert!(request.ended.contains(&transcript)),
            }
        }
        let recorded = crate::cursor::sessions::load(&registry).unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[&transcript].project, "/w/app");
        assert_eq!(recorded[&transcript].session, "c1");
        // Transcripts off: nothing recorded, a note; CURSOR_PROJECT_DIR when no workspace root.
        let off = serde_json::json!({"cursor_version": "2.6.0", "transcript_path": null,
                                     "workspace_roots": ["/w/app"]});
        let (request, notes) = cursor_request(HookEvent::SessionEnd, &registry, &off, None);
        assert!(request.ended.is_empty());
        assert!(notes[0].contains("no transcript_path"), "{notes:?}");
        let other = tmp.path().join("t/c2.jsonl");
        let bare = serde_json::json!({"transcript_path": other.display().to_string(),
                                      "conversation_id": "c2"});
        let (request, notes) =
            cursor_request(HookEvent::SessionEnd, &registry, &bare, Some("/w/env"));
        assert!(notes.is_empty(), "{notes:?}");
        assert!(request.ended.contains(&other));
        let recorded = crate::cursor::sessions::load(&registry).unwrap();
        assert_eq!(recorded[&other].project, "/w/env");
        assert_eq!(recorded[&other].session, "c2");
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
        let sessions = PathBuf::from(crate::mcp_tests::abs("/c/.codex/sessions"));
        let rollout = &crate::mcp_tests::abs(
            "/c/.codex/sessions/2026/11/09/rollout-2026-11-09T10-00-00-abc.jsonl",
        );
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

    fn context_of(text: &str) -> crate::start_context::StartContext {
        crate::start_context::StartContext {
            text: text.to_string(),
            shown: 1,
            total: 1,
            budget: 8000,
            ..Default::default()
        }
    }

    #[test]
    fn the_start_line_names_open_memory_conflicts() {
        let home = crate::mcp_tests::TempDir::new("hookconf");
        let paths = ClaudePaths::resolve(&|name| {
            (name == "HOME").then(|| home.path().display().to_string())
        })
        .unwrap();
        let fetch: StartFetch = Box::new(|_, _| {
            let mut context = context_of("## Open conflicts\n- a CONFLICTS WITH b");
            context.conflicts = vec!["a (remember) vs b (file /m/b.md)".into()];
            context.unchecked = 3;
            Ok(context)
        });
        let mut out = Vec::new();
        run_with_context(
            HookEvent::SessionStart,
            Ok(paths),
            home.path().to_path_buf(),
            Box::new(|_| Err("not called".to_string())),
            None,
            Some(fetch),
            std::io::Cursor::new(br#"{"cwd": "/w/app"}"#.to_vec()),
            &mut out,
            &mut Vec::new(),
        );
        let answer: Value = serde_json::from_str(&String::from_utf8(out).unwrap()).unwrap();
        let context = answer["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        let line = context.lines().next().unwrap();
        assert!(
            line.ends_with(
                " 1 open memory conflict (ask the user which memory holds, then settle it); 3 \
                 imported memories to check: conflicts()."
            ),
            "{line}"
        );
        assert!(context.contains("CONFLICTS WITH"), "{context}");
    }

    #[test]
    fn the_claude_code_start_answer_is_one_json_object_with_additional_context() {
        // code.claude.com/docs/en/hooks: the whole stdout one JSON object; SessionStart's
        // `hookSpecificOutput.additionalContext` reaches Claude, and nothing is shown to the user.
        let home = crate::mcp_tests::TempDir::new("hookctx");
        let paths = ClaudePaths::resolve(&|name| {
            (name == "HOME").then(|| home.path().display().to_string())
        })
        .unwrap();
        let asked = std::sync::Arc::new(std::sync::Mutex::new(None));
        let keep = asked.clone();
        let fetch: StartFetch = Box::new(move |dir, max| {
            *keep.lock().unwrap() = Some((dir.map(Path::to_path_buf), max));
            Ok(context_of("## How to work\n- Rule — hook (memory: rule)"))
        });
        let mut out = Vec::new();
        let code = run_with_context(
            HookEvent::SessionStart,
            Ok(paths.clone()),
            home.path().to_path_buf(),
            Box::new(|_| Err("not called".to_string())),
            None,
            Some(fetch),
            std::io::Cursor::new(br#"{"cwd": "/w/app"}"#.to_vec()),
            &mut out,
            &mut Vec::new(),
        );
        assert_eq!(code, 0);
        let out = String::from_utf8(out).unwrap();
        assert_eq!(out.lines().count(), 1, "{out}");
        let answer: Value = serde_json::from_str(&out).unwrap();
        let specific = &answer["hookSpecificOutput"];
        assert_eq!(specific["hookEventName"], "SessionStart");
        let context = specific["additionalContext"].as_str().unwrap();
        assert!(
            context.starts_with("graphersal-memory: your memory is served"),
            "{context}"
        );
        assert!(context.ends_with("(memory: rule)"), "{context}");
        assert!(answer.get("systemMessage").is_none());
        let (dir, max) = asked.lock().unwrap().clone().unwrap();
        assert_eq!(dir, Some(PathBuf::from("/w/app")));
        assert!(max < CLAUDE_CODE_CONTEXT_CHARS && max > 9000, "{max}");
        // An empty start context keeps the plain line; a failing one is a note.
        assert_eq!(claude_start_output("line", ""), "line");
        let mut out = Vec::new();
        let mut err = Vec::new();
        run_with_context(
            HookEvent::SessionStart,
            Ok(paths),
            home.path().to_path_buf(),
            Box::new(|_| Err("not called".to_string())),
            None,
            Some(Box::new(|_, _| Err("the daemon is away".to_string()))),
            std::io::empty(),
            &mut out,
            &mut err,
        );
        assert!(
            String::from_utf8(out)
                .unwrap()
                .starts_with("graphersal-memory:")
        );
        assert!(
            String::from_utf8(err)
                .unwrap()
                .contains("cannot read the start context")
        );
    }

    #[test]
    fn the_cursor_start_answer_carries_the_start_context_of_its_workspace() {
        let tmp = crate::mcp_tests::TempDir::new("cursor-ctx");
        let registry = tmp.path().join("cursor-transcripts.json");
        let input = serde_json::json!({"cursor_version": "2.6.0", "workspace_roots": ["/w/app"],
                                       "conversation_id": "c1"})
        .to_string();
        let asked = std::sync::Arc::new(std::sync::Mutex::new(None));
        let keep = asked.clone();
        let fetch: StartFetch = Box::new(move |dir, _| {
            *keep.lock().unwrap() = dir.map(Path::to_path_buf);
            Ok(context_of("## About you\n- Me (memory: me)"))
        });
        let mut out = Vec::new();
        run_cursor_with_context(
            HookEvent::SessionStart,
            registry,
            None,
            None,
            Some(fetch),
            std::io::Cursor::new(input.into_bytes()),
            &mut out,
            &mut Vec::new(),
        );
        let answer: Value = serde_json::from_slice(&out).unwrap();
        let context = answer["additional_context"].as_str().unwrap();
        assert!(context.starts_with(CURSOR_START_LINE), "{context}");
        assert!(context.ends_with("(memory: me)"), "{context}");
        assert_eq!(*asked.lock().unwrap(), Some(PathBuf::from("/w/app")));
    }
}
