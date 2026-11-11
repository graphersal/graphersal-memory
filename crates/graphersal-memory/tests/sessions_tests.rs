//! Session indexing and summaries end to end: the real binary's hooks, `import --sessions` and
//! `summaries` on a temporary home, data and run directory, with anonymized fixture transcripts
//! and a fake `claude` harness that records its calls (no model is ever called). Every daemon a
//! test starts is stopped (and killed if needed) at its end.
//!
//! Unix only, like the other end-to-end tests.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_graphersal-memory");

struct Env {
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gms-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("h")).unwrap();
        Self { root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("h")
    }

    fn data_dir(&self) -> PathBuf {
        self.root.join("d")
    }

    fn run_dir(&self) -> PathBuf {
        self.root.join("r")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        command
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", self.home())
            .env("TMPDIR", std::env::temp_dir())
            .env("GRAPHERSAL_MEMORY_DIR", self.data_dir())
            .env("GRAPHERSAL_MEMORY_RUN_DIR", self.run_dir())
            .env("GRAPHERSAL_MEMORY_IDLE", "60s")
            .current_dir(&self.root)
            .args(args);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).stdin(Stdio::null()).output().unwrap()
    }

    fn hook(&self, event: &str, input: &Value) -> (Output, Duration) {
        let started = Instant::now();
        let mut child = self
            .command(&["hook", event])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        (output, started.elapsed())
    }

    fn control(&self, request: &Value) -> Option<Value> {
        let mut stream = UnixStream::connect(self.run_dir().join("daemon.sock")).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        writeln!(stream, "{request}").unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).ok()?;
        serde_json::from_str(&line).ok()
    }

    /// Waits until the daemon's indexer finished every request; its last report.
    fn wait_for_indexing(&self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if let Some(status) =
                self.control(&json!({"protocol": 1, "control": "sessions_status"}))
                && status["running"] == false
                && status["requested"].as_u64().unwrap_or(0) > 0
                && status["completed"] == status["requested"]
            {
                return status["report"].clone();
            }
            assert!(Instant::now() < deadline, "the indexing did not finish");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// A project (a git root) with one transcript, finished long ago.
    fn project(&self) -> (PathBuf, PathBuf) {
        let work = std::fs::canonicalize(&self.root).unwrap().join("work");
        std::fs::create_dir_all(work.join(".git")).unwrap();
        let slug: String = work
            .display()
            .to_string()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let dir = self.home().join(".claude").join("projects").join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("0f0e0d0c-aaaa-bbbb-cccc-000000000001.jsonl");
        write_transcript(&file, &work);
        (work, file)
    }

    /// The fake `claude` (records calls; answers a summary after `delay` seconds).
    fn harness(&self, delay: u32) -> (PathBuf, PathBuf) {
        let bin = self.root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let log = self.root.join("harness.log");
        let answer = json!({"type": "result", "is_error": false, "total_cost_usd": 0.07,
            "structured_output": {"headline": "Planned the release checklist",
                "summary": "The user and the assistant wrote the release checklist.",
                "decisions": [{"decision": "Release on Mondays instead of Fridays",
                               "why": "nobody watches the rollout on a weekend"}],
                "corrections": [], "outcomes": ["Checklist merged"], "open_threads": []}});
        let script = bin.join("claude");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"CALL $*\" >> '{log}'\ncat > /dev/null\nsleep {delay}\n\
                 cat <<'JSON'\n{answer}\nJSON\n",
                log = log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (script, log)
    }

    fn summaries_on(&self, harness: &Path) {
        std::fs::create_dir_all(self.data_dir()).unwrap();
        std::fs::write(
            self.data_dir().join("config.json"),
            json!({"version": 1, "summaries": {"enabled": true,
                "harness_command": format!("{} -p --model sonnet", harness.display()),
                "monthly_budget_usd": 5}})
            .to_string(),
        )
        .unwrap();
    }

    /// The `use` server of `project`, initialized.
    fn mcp(&self, project: &Path) -> Mcp {
        let mut child = self
            .command(&["mcp", "--project", &project.display().to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut mcp = Mcp {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
            next: 1,
        };
        mcp.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "test", "version": "0"}}),
        );
        writeln!(
            mcp.input,
            "{}",
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        )
        .unwrap();
        mcp
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let status = self.control(&json!({"protocol": 1, "control": "status"}));
        let pid = status.and_then(|s| s["pid"].as_u64());
        let _ = self.control(&json!({"protocol": 1, "control": "stop"}));
        if let Some(pid) = pid {
            let deadline = Instant::now() + Duration::from_secs(10);
            while alive(pid) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = Command::new("kill")
                .arg("-9")
                .arg(pid.to_string())
                .stderr(Stdio::null())
                .status();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn alive(pid: u64) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

struct Mcp {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
}

impl Mcp {
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        writeln!(
            self.input,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        )
        .unwrap();
        loop {
            let mut line = String::new();
            assert!(
                self.output.read_line(&mut line).unwrap() > 0,
                "server closed"
            );
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == json!(id) {
                return message;
            }
        }
    }

    /// A tool's answer as JSON (its last JSON line).
    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let answer = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        let text = answer["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{answer}"))
            .to_string();
        serde_json::from_str(&text)
            .ok()
            .or_else(|| {
                text.lines()
                    .rev()
                    .find_map(|line| serde_json::from_str(line.trim()).ok())
            })
            .unwrap_or_else(|| panic!("{text}"))
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// An anonymized transcript of `work`, its time set to a day ago (finished).
fn write_transcript(file: &Path, work: &Path) {
    let cwd = work.display().to_string();
    let long = "Walk through every step of the release, including the rollback plan. ".repeat(40);
    let lines = [
        json!({"type": "user", "sessionId": "s", "cwd": cwd, "gitBranch": "main",
               "timestamp": "2026-10-01T09:00:00.000Z", "origin": {"kind": "human"},
               "message": {"role": "user", "content": "Write the release checklist."}}),
        json!({"type": "assistant", "timestamp": "2026-10-01T09:00:30.000Z", "message": {"content": [
            {"type": "text", "text": "Here is a first draft of the checklist."},
            {"type": "tool_use", "name": "Write", "input": {"file_path": format!("{cwd}/RELEASE.md")}}]}}),
        json!({"type": "user", "timestamp": "2026-10-01T09:01:00.000Z", "toolUseResult": {},
               "message": {"content": [{"type": "tool_result", "content": "SECRET-TOOL-OUTPUT"}]}}),
        json!({"type": "user", "timestamp": "2026-10-01T09:02:00.000Z",
               "message": {"content": long}}),
        json!({"type": "assistant", "timestamp": "2026-10-01T09:03:00.000Z",
               "message": {"content": [{"type": "text", "text": "Added the rollback plan."}]}}),
        json!({"type": "ai-title", "aiTitle": "Release checklist"}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(file, text).unwrap();
    let past = SystemTime::now() - Duration::from_secs(24 * 3600);
    std::fs::File::options()
        .write(true)
        .open(file)
        .unwrap()
        .set_modified(past)
        .unwrap();
}

fn calls(log: &Path) -> usize {
    std::fs::read_to_string(log)
        .map(|t| t.lines().filter(|l| l.starts_with("CALL ")).count())
        .unwrap_or(0)
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn the_hook_queues_indexing_and_summaries_become_searchable_facts() {
    let env = Env::new();
    let (work, file) = env.project();
    let (harness, log) = env.harness(4);
    env.summaries_on(&harness);
    // The session that just ended is named by the hook; the harness takes 4 s, the hook does
    // not wait for it.
    let (hook, took) = env.hook(
        "session-end",
        &json!({"session_id": "x", "cwd": work.display().to_string(),
                "transcript_path": file.display().to_string(), "hook_event_name": "SessionEnd"}),
    );
    assert!(hook.status.success(), "{}", text(&hook));
    assert!(hook.stdout.is_empty());
    assert!(
        took < Duration::from_secs(4),
        "the hook waited for the work: {took:?}"
    );
    let report = env.wait_for_indexing();
    assert_eq!(report["indexed"], 1, "{report}");
    assert_eq!(report["summarized"], 1, "{report}");
    assert_eq!(report["facts"], 2, "{report}");
    assert_eq!(calls(&log), 1);

    let mut mcp = env.mcp(&work);
    let facts = mcp.call(
        "sessions",
        json!({"text": "why do we release on Mondays", "k": 3}),
    );
    assert_eq!(
        facts[0]["fact"], "Release on Mondays instead of Fridays",
        "{facts}"
    );
    assert_eq!(facts[0]["why"], "nobody watches the rollout on a weekend");
    let session = mcp.call("session", json!({"id": "0f0e0d0c", "turns": true}));
    assert_eq!(
        session["headline"], "Planned the release checklist",
        "{session}"
    );
    assert_eq!(
        session["turn_list"].as_array().unwrap().len(),
        2,
        "{session}"
    );
    assert_eq!(session["files_edited"], json!(["RELEASE.md"]));
    let turns = mcp.call("recall", json!({"text": "rollback plan", "label": "turn"}));
    assert!(!turns.as_array().unwrap().is_empty(), "{turns}");
    drop(mcp);

    // Again: nothing new, no call.
    let (hook, _) = env.hook("session-start", &json!({"cwd": work.display().to_string()}));
    assert!(hook.status.success());
    let report = env.wait_for_indexing();
    assert_eq!(report["unchanged"], 1, "{report}");
    assert_eq!(calls(&log), 1);
    assert!(
        !std::fs::read_to_string(&log)
            .unwrap()
            .contains("SECRET-TOOL-OUTPUT")
    );
}

#[test]
fn without_consent_import_sessions_indexes_raw_turns_and_calls_nothing() {
    let env = Env::new();
    let (work, _) = env.project();
    let (harness, log) = env.harness(0);
    // The harness is named in the settings, but summaries are not switched on.
    std::fs::create_dir_all(env.data_dir()).unwrap();
    std::fs::write(
        env.data_dir().join("config.json"),
        json!({"version": 1, "summaries": {"harness_command": harness.display().to_string()}})
            .to_string(),
    )
    .unwrap();
    let dry = env.run(&["import", "--sessions", "--dry-run"]);
    assert!(dry.status.success(), "{}", text(&dry));
    assert!(text(&dry).contains("1 new or changed"), "{}", text(&dry));
    assert!(text(&dry).contains("summaries: off"), "{}", text(&dry));
    let imported = env.run(&["import", "--sessions"]);
    assert!(imported.status.success(), "{}", text(&imported));
    let out = text(&imported);
    assert!(out.contains("Done: 1 sessions indexed (2 turns)"), "{out}");
    assert_eq!(calls(&log), 0);

    let estimate = env.run(&["summaries", "estimate"]);
    assert!(estimate.status.success(), "{}", text(&estimate));
    assert!(
        text(&estimate).contains("1 finished session, 1 without a summary"),
        "{}",
        text(&estimate)
    );
    assert!(text(&estimate).contains("estimate"));
    let status = env.run(&["summaries", "status"]);
    assert!(
        text(&status).contains("summaries: off"),
        "{}",
        text(&status)
    );
    assert!(text(&status).contains("1 indexed"), "{}", text(&status));
    let refused = env.run(&["summaries", "enable"]);
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    assert!(text(&refused).contains("never tool output"));
    let enabled = env.run(&["summaries", "enable", "--yes", "--budget", "2"]);
    assert!(enabled.status.success(), "{}", text(&enabled));
    let config: Value =
        serde_json::from_slice(&std::fs::read(env.data_dir().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(config["summaries"]["enabled"], true);
    assert_eq!(config["summaries"]["monthly_budget_usd"], 2.0);
    let disabled = env.run(&["summaries", "disable"]);
    assert!(disabled.status.success());
    assert_eq!(calls(&log), 0);

    let mut mcp = env.mcp(&work);
    let newest = mcp.call("sessions", json!({}));
    assert_eq!(newest[0]["title"], "Release checklist", "{newest}");
    assert_eq!(
        mcp.call("sessions", json!({"text": "release"})),
        json!([]),
        "no summaries, no facts"
    );
}

#[test]
fn the_hooks_do_nothing_inside_a_summary_run() {
    let env = Env::new();
    let output = env
        .command(&["hook", "session-start"])
        .env("GRAPHERSAL_MEMORY_HOOK", "off")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!env.data_dir().exists());
    assert!(!env.run_dir().join("daemon.sock").exists());
}

/// An MCP server whose stdin the test closes (the session's end) to see what it does then.
struct Closable {
    mcp: Option<Mcp>,
}

impl Closable {
    fn spawn(mut command: Command) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut mcp = Mcp {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
            next: 1,
        };
        let init = mcp.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "test", "version": "0"}}),
        );
        assert!(init["result"].is_object(), "{init}");
        Self { mcp: Some(mcp) }
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.mcp.as_mut().unwrap().call(name, arguments)
    }

    /// Closes stdin and waits for the exit: its status and stderr.
    fn close(mut self) -> (std::process::ExitStatus, String) {
        let mcp = self.mcp.take().unwrap();
        // The input is swapped for a pipe of a process that ends at once, so the server's own
        // stdin is closed while `Mcp` (which kills its child when dropped) stays whole.
        let mut mcp = mcp;
        let mut stand_in = Command::new("true").stdin(Stdio::piped()).spawn().unwrap();
        let input = std::mem::replace(&mut mcp.input, stand_in.stdin.take().unwrap());
        drop(input);
        let mut stderr = String::new();
        if let Some(mut err) = mcp.child.stderr.take() {
            use std::io::Read;
            err.read_to_string(&mut stderr).unwrap();
        }
        let status = mcp.child.wait().unwrap();
        let _ = stand_in.wait();
        (status, stderr)
    }
}

/// The project's memory files of Claude Code (one memory).
fn write_memory_files(env: &Env, work: &Path) {
    let slug: String = work
        .display()
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let memory = env
        .home()
        .join(".claude/projects")
        .join(slug)
        .join("memory");
    std::fs::create_dir_all(&memory).unwrap();
    std::fs::write(
        memory.join("MEMORY.md"),
        "- [Release day](release-day.md) — when we release\n",
    )
    .unwrap();
    std::fs::write(
        memory.join("release-day.md"),
        "---\nname: release-day\ndescription: We release on Mondays\nmetadata:\n  type: project\n\
         ---\n\nMondays.\n",
    )
    .unwrap();
}

/// The daemon's indexing status.
fn sessions_status(env: &Env) -> Value {
    env.control(&json!({"protocol": 1, "control": "sessions_status"}))
        .unwrap_or(Value::Null)
}

/// Without any hook, Claude Code's MCP server alone: its start imports the project's memory
/// files and queues the indexing, its end (stdin closed) queues the indexing again; a hook run
/// afterwards changes nothing.
#[test]
fn the_mcp_server_does_the_hooks_work_at_session_start_and_end() {
    let env = Env::new();
    let (work, _) = env.project();
    write_memory_files(&env, &work);
    let mut command = env.command(&["mcp"]);
    command.env("CLAUDECODE", "1").current_dir(&work);
    let mut mcp = Closable::spawn(command);
    // The start: the transcript (finished a day ago) is indexed, the memory file imported.
    let report = env.wait_for_indexing();
    assert_eq!(report["indexed"], 1, "{report}");
    let deadline = Instant::now() + Duration::from_secs(60);
    let memories = loop {
        let found = mcp.call("memories", json!({"text": "release day", "k": 5}));
        if found.as_array().is_some_and(|rows| !rows.is_empty()) {
            break found;
        }
        assert!(
            Instant::now() < deadline,
            "the memory file was not imported"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(memories[0]["slug"], "release-day", "{memories}");
    let requested = sessions_status(&env)["requested"].as_u64().unwrap();
    // The end: stdin closes, the shim queues the indexing before it exits.
    let (status, stderr) = mcp.close();
    assert!(status.success(), "{stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    let report = env.wait_for_indexing();
    let status = sessions_status(&env);
    assert_eq!(
        status["requested"].as_u64().unwrap(),
        requested + 1,
        "{status}"
    );
    assert_eq!(report["unchanged"], 1, "{report}");
    assert_eq!(report["indexed"], 0, "{report}");
    // The hook too (both paths run when the hooks work): nothing imported twice.
    let (hook, _) = env.hook(
        "session-start",
        &json!({"cwd": work.display().to_string(), "session_id": "y"}),
    );
    assert!(hook.status.success(), "{}", text(&hook));
    let line = String::from_utf8_lossy(&hook.stdout);
    assert!(line.starts_with("graphersal-memory:"), "{line}");
    assert!(!line.contains("Imported"), "{line}");
    let report = env.wait_for_indexing();
    assert_eq!(report["indexed"], 0, "{report}");
    let mut mcp = env.mcp(&work);
    let all = mcp.call("memories", json!({"k": 50}));
    assert_eq!(all.as_array().map(Vec::len), Some(1), "{all}");
}

/// Inside a summary run (`GRAPHERSAL_MEMORY_HOOK=off`) and for a harness that is neither known
/// nor set up, the MCP server does nothing beside serving.
#[test]
fn the_mcp_server_does_nothing_else_inside_a_summary_run() {
    let env = Env::new();
    let (work, _) = env.project();
    write_memory_files(&env, &work);
    for extra in [Some(("GRAPHERSAL_MEMORY_HOOK", "off")), None] {
        let mut command = env.command(&["mcp"]);
        if let Some((name, value)) = extra {
            command.env("CLAUDECODE", "1").env(name, value);
        }
        command.current_dir(&work);
        let mut mcp = Closable::spawn(command);
        let all = mcp.call("memories", json!({"k": 50}));
        assert_eq!(all, json!([]), "{all}");
        let (status, stderr) = mcp.close();
        assert!(status.success(), "{stderr}");
        assert_eq!(sessions_status(&env)["requested"], 0);
    }
}

/// Text that broke the engine (graphersal-session 0.6.0's `trailing_terminal` cut a script
/// inside a multi-byte character before its last `(`): Cyrillic, an emoji and a no-break space
/// each right before an unbalanced parenthesis inside a string, at the end of the text.
const AWKWARD: &str = "Готово: релиз проверен 🎉, see (draft — е(1 and \u{a0}(x 🎉(";

/// A transcript of `work` whose turns and title are [`AWKWARD`] text, finished a day ago.
fn write_awkward_transcript(file: &Path, work: &Path) {
    let cwd = work.display().to_string();
    let lines = [
        json!({"type": "user", "sessionId": "s", "cwd": cwd, "gitBranch": "main",
               "timestamp": "2026-10-01T09:00:00.000Z", "origin": {"kind": "human"},
               "message": {"role": "user", "content": format!("Проверь релиз (шаг 1: {AWKWARD}")}}),
        json!({"type": "assistant", "timestamp": "2026-10-01T09:00:30.000Z",
               "message": {"content": [{"type": "text", "text": AWKWARD}]}}),
        json!({"type": "ai-title", "aiTitle": format!("Релиз е({AWKWARD}")}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(file, text).unwrap();
    let past = SystemTime::now() - Duration::from_secs(24 * 3600);
    std::fs::File::options()
        .write(true)
        .open(file)
        .unwrap()
        .set_modified(past)
        .unwrap();
}

/// Whether `value` holds the string `text` somewhere.
fn holds_string(value: &Value, text: &str) -> bool {
    match value {
        Value::String(s) => s == text,
        Value::Array(items) => items.iter().any(|v| holds_string(v, text)),
        Value::Object(map) => map.values().any(|v| holds_string(v, text)),
        _ => false,
    }
}

/// Whether `value` holds a string containing `part` somewhere.
fn holds_part(value: &Value, part: &str) -> bool {
    match value {
        Value::String(s) => s.contains(part),
        Value::Array(items) => items.iter().any(|v| holds_part(v, part)),
        Value::Object(map) => map.values().any(|v| holds_part(v, part)),
        _ => false,
    }
}

/// Non-ASCII text through the whole path: a transcript is indexed by the daemon and found
/// again, a memory is remembered and recalled with its exact text. Passes only with the engine
/// fix of graphersal main 2b2a148c (graphersal 0.6.1; with 0.6.0 the engine panics).
#[test]
fn non_ascii_sessions_and_memories_round_trip() {
    let env = Env::new();
    let (work, file) = env.project();
    write_awkward_transcript(&file, &work);
    let output = env.run(&["import", "--sessions"]);
    let out = text(&output);
    assert!(output.status.success(), "{out}");
    assert!(!out.contains("problem"), "{out}");
    let report = env.wait_for_indexing();
    assert_eq!(report["indexed"], 1, "{report}");
    let mut mcp = env.mcp(&work);
    let newest = mcp.call("sessions", json!({}));
    assert_eq!(newest[0]["title"], format!("Релиз е({AWKWARD}"), "{newest}");
    let session = mcp.call("session", json!({"id": "0f0e0d0c", "turns": true}));
    assert!(holds_part(&session, AWKWARD), "{session}");
    let turns = mcp.call("recall", json!({"text": "релиз проверен", "label": "turn"}));
    assert!(holds_part(&turns, AWKWARD), "{turns}");
    let body = format!("Решение: {AWKWARD}");
    mcp.call(
        "remember",
        json!({"slug": "awkward-text", "kind": "project", "summary": "Релиз е(1 🎉", "body": body}),
    );
    let recalled = mcp.call("memory", json!({"slug": "awkward-text"}));
    assert!(holds_string(&recalled, &body), "{recalled}");
    let near = mcp.call("recall", json!({"text": "Релиз е(1", "k": 5}));
    assert!(near.to_string().contains("awkward-text"), "{near}");
    let log = std::fs::read_to_string(env.run_dir().join("daemon.log")).unwrap_or_default();
    assert!(!log.contains("PANIC"), "{log}");
}

/// A session file whose indexing panics (the test-only hook of debug builds) is recorded and
/// skipped: the others are indexed in the same run, it is tried once more in a later run, then
/// left alone; the daemon logs the panic with the file and serves on.
#[test]
fn indexing_goes_on_past_a_failing_transcript() {
    let env = Env::new();
    let (work, file) = env.project();
    let bad = file.with_file_name("0f0e0d0c-aaaa-bbbb-cccc-0000000000ff.jsonl");
    write_transcript(&bad, &work);
    let run = |env: &Env| {
        let mut command = env.command(&["import", "--sessions"]);
        command.env("GRAPHERSAL_MEMORY_TEST_PANIC", "index:cccc-0000000000ff");
        let output = command.stdin(Stdio::null()).output().unwrap();
        text(&output)
    };
    let first = run(&env);
    assert!(first.contains("1 indexed"), "{first}");
    assert!(first.contains("0000000000ff.jsonl"), "{first}");
    assert!(first.contains("attempt 1 of 2"), "{first}");
    let log = std::fs::read_to_string(env.run_dir().join("daemon.log")).unwrap_or_default();
    assert!(log.contains("PANIC"), "{log}");
    assert!(log.contains("while indexing"), "{log}");
    assert!(log.contains("0000000000ff.jsonl"), "{log}");
    // The daemon serves on.
    let mut mcp = env.mcp(&work);
    let found = mcp.call("sessions", json!({}));
    assert!(
        found.as_array().is_some_and(|rows| rows.len() == 1),
        "{found}"
    );
    // Tried once more, then skipped.
    let second = run(&env);
    assert!(second.contains("attempt 2 of 2"), "{second}");
    let third = run(&env);
    assert!(!third.contains("attempt"), "{third}");
    let report = env.wait_for_indexing();
    assert_eq!(report["failed"], 1, "{report}");
    let status = text(&env.run(&["status"]));
    assert!(status.contains("1 could not be indexed"), "{status}");
}
