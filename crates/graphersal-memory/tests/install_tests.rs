//! `install`, `uninstall`, `status`, `doctor`, `import` and `hook` end to end: the real binary on
//! a temporary home, data and run directory (never the user's), with the daemon the import
//! starts stopped (and killed if needed) at the end of every test. One test also lets the
//! installed `claude` binary read the registration (skipped when `claude` is not installed),
//! with its home pointed at the temporary directory too.
//!
//! Unix only, like the daemon tests (they stop processes with `kill` and talk to the socket with
//! `std::os::unix::net`).

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_graphersal-memory");

/// A test's directories (short: a socket path has at most 103 bytes).
struct Env {
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gmi-{}-{}",
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

    /// A clean environment: no variable of a surrounding Claude Code session leaks in.
    fn clean(&self, command: &mut Command) {
        command
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", self.home())
            .env("TMPDIR", std::env::temp_dir())
            .current_dir(&self.root);
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        self.clean(&mut command);
        command
            .env("GRAPHERSAL_MEMORY_DIR", self.data_dir())
            .env("GRAPHERSAL_MEMORY_RUN_DIR", self.run_dir())
            .env("GRAPHERSAL_MEMORY_IDLE", "60s")
            // No network in tests: the model download of `install --yes` fails at once.
            .env("GRAPHERSAL_MEMORY_HF_ENDPOINT", "http://127.0.0.1:1")
            .args(args);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).stdin(Stdio::null()).output().unwrap()
    }

    fn run_with_input(&self, args: &[&str], input: &str) -> Output {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn control(&self, request: &str) -> Option<Value> {
        let mut stream = UnixStream::connect(self.run_dir().join("daemon.sock")).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        writeln!(stream, "{}", json!({"protocol": 1, "control": request})).unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).ok()?;
        serde_json::from_str(&line).ok()
    }

    /// Every file under the home directory with its bytes.
    fn home_snapshot(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![self.home()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                let relative = path.strip_prefix(self.home()).unwrap().to_path_buf();
                if path.is_dir() {
                    out.push((relative, Vec::new()));
                    stack.push(path);
                } else {
                    out.push((relative, std::fs::read(&path).unwrap()));
                }
            }
        }
        out.sort();
        out
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let pid = self.control("status").and_then(|s| s["pid"].as_u64());
        let _ = self.control("stop");
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

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// A project directory with a Claude Code memory directory and a transcript naming it.
fn fixture(env: &Env) -> PathBuf {
    let project = env.root.join("work");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    std::fs::write(
        project.join(".git").join("config"),
        "[remote \"origin\"]\n\turl = https://user:secret@example.org/me/work.git\n",
    )
    .unwrap();
    let project = std::fs::canonicalize(&project).unwrap();
    let name: String = project
        .display()
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let dir = env.home().join(".claude").join("projects").join(name);
    let memory = dir.join("memory");
    std::fs::create_dir_all(&memory).unwrap();
    std::fs::write(
        dir.join("a.jsonl"),
        format!("{}\n", json!({"cwd": project.display().to_string()})),
    )
    .unwrap();
    std::fs::write(
        memory.join("MEMORY.md"),
        "- [Tests need approval](tests-approval.md) — changing a test needs approval\n",
    )
    .unwrap();
    std::fs::write(
        memory.join("tests-approval.md"),
        "---\nname: tests-approval\ndescription: Changing an existing test needs approval\n\
         metadata:\n  type: feedback\n---\n\nAsk first. See [[release-day]].\n",
    )
    .unwrap();
    std::fs::write(
        memory.join("release-day.md"),
        "---\nname: release-day\ndescription: We release on Fridays\nmetadata:\n  type: project\n---\n\nFridays.\n",
    )
    .unwrap();
    project
}

#[test]
fn install_import_status_doctor_hook_uninstall_and_purge() {
    let env = Env::new();
    std::fs::write(
        env.home().join(".claude.json"),
        "{\"numStartups\": 7,\n \"mcpServers\": {}}\n",
    )
    .unwrap();
    let project = fixture(&env);
    let before = env.home_snapshot();

    let dry = env.run(&["install", "--dry-run"]);
    assert!(dry.status.success(), "{}", text(&dry));
    assert!(
        text(&dry).contains("2 new and 0 changed memory files"),
        "{}",
        text(&dry)
    );
    assert_eq!(env.home_snapshot(), before);
    assert!(!env.data_dir().exists(), "a dry run writes nothing");

    let refused = env.run(&["install"]);
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    assert!(text(&refused).contains("Aborted"), "{}", text(&refused));
    assert_eq!(env.home_snapshot(), before);

    let installed = env.run(&["install", "--yes"]);
    assert!(installed.status.success(), "{}", text(&installed));
    let out = text(&installed);
    assert!(
        out.contains("imported 2 memories (2 new, 0 updated) into 1 project"),
        "{out}"
    );
    let config: Value =
        serde_json::from_slice(&std::fs::read(env.home().join(".claude.json")).unwrap()).unwrap();
    let entry = &config["mcpServers"]["graphersal-memory"];
    assert_eq!(
        entry["command"],
        std::fs::canonicalize(BIN).unwrap().display().to_string()
    );
    assert_eq!(entry["args"][0], "mcp");
    assert!(
        entry["args"]
            .as_array()
            .unwrap()
            .contains(&json!("--data-dir"))
    );

    // The project and its remote (without the credentials) are recorded.
    let projects: Value =
        serde_json::from_slice(&std::fs::read(env.data_dir().join("projects.json")).unwrap())
            .unwrap();
    assert_eq!(
        projects["projects"][project.display().to_string()]["git_remote"],
        "https://example.org/me/work.git"
    );

    let status = env.run(&["status"]);
    let out = text(&status);
    assert!(status.status.success(), "{out}");
    assert!(out.contains("daemon: running"), "{out}");
    assert!(
        out.contains("MCP server graphersal-memory (use): installed"),
        "{out}"
    );
    assert!(out.contains("2 memory files imported"), "{out}");

    let doctor = env.run(&["doctor"]);
    assert!(doctor.status.success(), "{}", text(&doctor));
    assert!(
        text(&doctor).contains("No problems found."),
        "{}",
        text(&doctor)
    );

    // The hook: a changed memory file is imported, one line on stdout, exit 0.
    let release_day = env
        .home()
        .join(".claude/projects")
        .join(
            project
                .display()
                .to_string()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect::<String>(),
        )
        .join("memory/release-day.md");
    let original = std::fs::read(&release_day).unwrap();
    std::fs::write(
        &release_day,
        "---\nname: release-day\ndescription: We release on Mondays now\nmetadata:\n  type: project\n---\n\nMondays.\n",
    )
    .unwrap();
    let hook = env.run_with_input(
        &["hook", "session-start"],
        &json!({"session_id": "s", "cwd": project.display().to_string(),
                "hook_event_name": "SessionStart"})
        .to_string(),
    );
    assert!(hook.status.success(), "{}", text(&hook));
    let line = String::from_utf8_lossy(&hook.stdout);
    assert_eq!(line.lines().count(), 1, "{line}");
    assert!(
        line.contains("Imported 1 new or changed memory file"),
        "{line}"
    );
    let end = env.run_with_input(&["hook", "session-end"], "{}");
    assert!(end.status.success());
    assert!(end.stdout.is_empty());

    let again = env.run(&["install", "--yes"]);
    assert!(again.status.success(), "{}", text(&again));
    assert!(
        text(&again).contains("already up to date"),
        "{}",
        text(&again)
    );

    // Undo the test's own edit of a memory file (install never changes them).
    std::fs::write(&release_day, original).unwrap();
    let removed = env.run(&["uninstall", "--yes", "--no-export"]);
    assert!(removed.status.success(), "{}", text(&removed));
    assert_eq!(env.home_snapshot(), before, "{}", text(&removed));
    assert!(env.data_dir().join("stores").exists(), "memories stay");

    let purged = env.run(&["uninstall", "--purge", "--yes"]);
    assert!(purged.status.success(), "{}", text(&purged));
    assert!(
        text(&purged).contains("stopped the daemon"),
        "{}",
        text(&purged)
    );
    assert!(!env.data_dir().exists());
    assert!(env.control("status").is_none());
}

#[test]
fn a_hook_without_anything_installed_never_fails() {
    let env = Env::new();
    let hook = env.run_with_input(&["hook", "session-start"], "not json at all");
    assert!(hook.status.success(), "{}", text(&hook));
    assert!(String::from_utf8_lossy(&hook.stdout).starts_with("graphersal-memory:"));
}

/// The `claude` binary on the PATH, if any.
fn claude() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("claude"))
        .find(|candidate| candidate.is_file())
}

#[test]
fn claude_code_reads_the_registration_and_connects() {
    let Some(claude) = claude() else {
        eprintln!("skipped: the claude binary is not installed");
        return;
    };
    let env = Env::new();
    let installed = env.run(&["install", "--yes"]);
    assert!(installed.status.success(), "{}", text(&installed));
    let get = |name: &str| -> String {
        let mut command = Command::new(&claude);
        env.clean(&mut command);
        let output = command
            .args(["mcp", "get", name])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        text(&output)
    };
    let shown = get("graphersal-memory");
    assert!(shown.contains("User config"), "{shown}");
    assert!(shown.contains("Connected"), "{shown}");
    let develop = get("graphersal-memory-develop");
    assert!(!develop.contains("Connected"), "{develop}");

    // Claude Code rewrote its configuration; uninstall still removes exactly our entry.
    let removed = env.run(&["uninstall", "--yes"]);
    assert!(removed.status.success(), "{}", text(&removed));
    let gone = get("graphersal-memory");
    assert!(!gone.contains("Connected"), "{gone}");
    let config = std::fs::read_to_string(env.home().join(".claude.json")).unwrap_or_default();
    assert!(!config.contains("graphersal-memory"), "{config}");
}

#[test]
fn the_daemon_reads_its_idle_time_from_the_settings_file() {
    let env = Env::new();
    std::fs::create_dir_all(env.data_dir()).unwrap();
    std::fs::write(
        env.data_dir().join("config.json"),
        "{\"version\": 1, \"daemon\": {\"idle\": \"2s\"}}\n",
    )
    .unwrap();
    let mut command = env.command(&["daemon"]);
    command.env_remove("GRAPHERSAL_MEMORY_IDLE");
    let started = command.stdin(Stdio::null()).output().unwrap();
    assert!(started.status.success(), "{}", text(&started));
    let status = env.control("status").expect("the daemon runs");
    assert_eq!(status["idle_secs"], json!(2.0));
    let deadline = Instant::now() + Duration::from_secs(60);
    while env.control("status").is_some() {
        assert!(
            Instant::now() < deadline,
            "the daemon did not exit after 2s idle"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let shown = env.run(&["status"]);
    assert!(
        text(&shown).contains("daemon idle time 60s"),
        "the environment wins over the file: {}",
        text(&shown)
    );
}

/// One MCP tool call through the real shim (and the daemon), waiting for its answer.
fn call_tool(env: &Env, project: &std::path::Path, name: &str, arguments: Value) -> Value {
    let mut child = env
        .command(&["mcp", "--project", &project.display().to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "t", "version": "1"}}});
    let inited = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                      "params": {"name": name, "arguments": arguments}});
    writeln!(stdin, "{init}\n{inited}\n{call}").unwrap();
    stdin.flush().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let answer = loop {
        let mut line = String::new();
        assert!(
            reader.read_line(&mut line).unwrap() > 0,
            "the shim ended early"
        );
        let message: Value = serde_json::from_str(&line).unwrap();
        if message["id"] == json!(2) {
            break message;
        }
    };
    drop(stdin);
    let _ = child.wait();
    answer
}

#[test]
fn uninstall_writes_every_memory_back_into_the_file_memory_through_the_daemon() {
    let env = Env::new();
    let project = fixture(&env);
    let installed = env.run(&["install", "--yes", "--model", "builtin"]);
    assert!(installed.status.success(), "{}", text(&installed));
    let settings: Value =
        serde_json::from_slice(&std::fs::read(env.home().join(".claude/settings.json")).unwrap())
            .unwrap();
    assert_eq!(settings["autoMemoryEnabled"], false, "{settings}");
    // A memory the agent adds after install.
    let saved = call_tool(
        &env,
        &project,
        "remember",
        json!({"slug": "added-after-install", "kind": "project",
               "summary": "Added through MCP after install", "body": "It must survive uninstall."}),
    );
    assert_eq!(saved["result"]["isError"], json!(false), "{saved}");
    let listed = env.run(&["projects", "list"]);
    assert!(
        text(&listed).contains(&project.display().to_string()),
        "{}",
        text(&listed)
    );
    let dry = env.run(&["export", "--to-files", "--dry-run"]);
    assert!(
        text(&dry).contains("3 memories of 1 project(s)"),
        "{}",
        text(&dry)
    );

    let removed = env.run(&["uninstall", "--yes"]);
    assert!(removed.status.success(), "{}", text(&removed));
    let out = text(&removed);
    assert!(out.contains("exported 3 memories"), "{out}");
    let memory_dir = env
        .home()
        .join(".claude/projects")
        .join(
            project
                .display()
                .to_string()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect::<String>(),
        )
        .join("memory");
    let added = std::fs::read_to_string(memory_dir.join("added-after-install.md")).unwrap();
    assert!(added.contains("It must survive uninstall."), "{added}");
    // install created the settings file, so it is gone again, auto-memory switch included.
    assert!(!env.home().join(".claude/settings.json").exists());
}

#[test]
fn cursor_install_hooks_status_and_uninstall_through_the_binary() {
    let env = Env::new();
    std::fs::create_dir_all(env.home().join(".cursor")).unwrap();
    let before = env.home_snapshot();
    let install = env.run(&[
        "install",
        "--harness",
        "cursor",
        "--yes",
        "--model",
        "builtin",
    ]);
    assert!(install.status.success(), "{}", text(&install));
    assert!(env.home().join(".cursor/mcp.json").is_file());
    assert!(
        env.home()
            .join(".cursor/rules/graphersal-memory.mdc")
            .is_file()
    );
    let status = env.run(&["status"]);
    let out = text(&status);
    assert!(out.contains("Cursor (configuration"), "{out}");
    assert!(out.contains("hook sessionStart"), "{out}");

    // Cursor's sessionStart input (cursor.com/docs/hooks) with a transcript in its home.
    let transcript = env
        .home()
        .join(".cursor/projects/w/agent-transcripts/c1/c1.jsonl");
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(
        &transcript,
        "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"<user_query>hi</user_query>\"}]}}\n",
    )
    .unwrap();
    let input = json!({"conversation_id": "c1", "session_id": "c1",
        "hook_event_name": "sessionStart", "cursor_version": "2.6.0",
        "workspace_roots": [env.root.display().to_string()],
        "transcript_path": transcript.display().to_string()})
    .to_string();
    let registry = env.data_dir().join("sessions/cursor-transcripts.json");
    // Claude Code's hook run by Cursor (third-party hooks): nothing happens.
    let claude_hook = env.run_with_input(&["hook", "session-start"], &input);
    assert!(claude_hook.status.success(), "{}", text(&claude_hook));
    assert!(claude_hook.stdout.is_empty(), "{}", text(&claude_hook));
    assert!(
        String::from_utf8_lossy(&claude_hook.stderr).contains("run by Cursor"),
        "{}",
        text(&claude_hook)
    );
    assert!(!registry.exists());
    // Our Cursor hook: JSON with the start line, the transcript recorded.
    let hook = env.run_with_input(&["hook", "session-start", "--harness", "cursor"], &input);
    assert!(hook.status.success(), "{}", text(&hook));
    let answer: Value = serde_json::from_slice(&hook.stdout).unwrap();
    assert!(
        answer["additional_context"]
            .as_str()
            .unwrap()
            .starts_with("graphersal-memory:")
    );
    let recorded: Value =
        serde_json::from_str(&std::fs::read_to_string(&registry).unwrap()).unwrap();
    assert!(
        recorded["transcripts"][transcript.display().to_string()]["project"].is_string(),
        "{recorded}"
    );
    let end = env.run_with_input(&["hook", "session-end", "--harness", "cursor"], &input);
    assert!(end.status.success(), "{}", text(&end));
    assert!(end.stdout.is_empty(), "{}", text(&end));

    std::fs::remove_dir_all(env.home().join(".cursor/projects")).unwrap();
    let uninstall = env.run(&["uninstall", "--harness", "cursor", "--yes", "--no-export"]);
    assert!(uninstall.status.success(), "{}", text(&uninstall));
    assert_eq!(env.home_snapshot(), before, "{}", text(&uninstall));
}
