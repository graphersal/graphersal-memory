//! `bench` end to end: the real binary on a temporary home and data directory, a fixture
//! transcript and memory directory, and a fake `claude` on `PATH` that records its calls and
//! answers canned JSON (no model is ever called). No daemon is started.
//!
//! Unix only, like the other end-to-end tests (the fake harness is a shell script).

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_graphersal-memory");
const UUID: &str = "0f0e0d0c-aaaa-bbbb-cccc-000000000002";

struct Env {
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gmb-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home")).unwrap();
        let env = Self {
            root: std::fs::canonicalize(&root).unwrap(),
        };
        env.fake_claude();
        env
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }

    fn project(&self) -> PathBuf {
        self.root.join("work")
    }

    fn log(&self) -> PathBuf {
        self.root.join("claude.log")
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.log())
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// A Claude Code directory with the project's transcript and memory under `claude_dir`.
    fn claude_files(&self, claude_dir: &Path) {
        let slug: String = self
            .project()
            .display()
            .to_string()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let dir = claude_dir.join("projects").join(slug);
        std::fs::create_dir_all(dir.join("memory")).unwrap();
        let mut lines = String::new();
        for (i, (user, reply)) in [
            ("Never force-push to main.", "Understood."),
            (
                "We chose SQLite over Postgres because it needs no server.",
                "Noted.",
            ),
        ]
        .iter()
        .enumerate()
        {
            lines.push_str(&format!(
                "{}\n{}\n",
                json!({"type": "user", "sessionId": "s", "cwd": self.project().display().to_string(),
                       "timestamp": format!("2026-01-03T10:0{i}:00.000Z"), "origin": {"kind": "human"},
                       "message": {"role": "user", "content": user}}),
                json!({"type": "assistant", "sessionId": "s",
                       "timestamp": format!("2026-01-03T10:0{i}:05.000Z"),
                       "message": {"role": "assistant", "content": [{"type": "text", "text": reply}]}})
            ));
        }
        let file = dir.join(format!("{UUID}.jsonl"));
        std::fs::write(&file, lines).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(86_400))
            .unwrap();
        std::fs::write(
            dir.join("memory").join("MEMORY.md"),
            "- [No force push](no-force-push.md)\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("memory").join("no-force-push.md"),
            "---\nname: no-force-push\ndescription: never force-push\ntype: feedback\n---\nNever.\n",
        )
        .unwrap();
    }

    /// The fake `claude`: logs its arguments, answers by the prompt on stdin.
    fn fake_claude(&self) {
        let bin = self.root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let generated = json!({"type": "result", "is_error": false, "total_cost_usd": 0.05,
            "structured_output": {"questions": [
                {"category": "rule", "question": "May I force-push to main?",
                 "reference": "No, never.", "sources": [format!("turn:{UUID}:0")]},
                {"category": "decision", "question": "Why SQLite?",
                 "reference": "It needs no server.", "sources": [format!("turn:{UUID}:1")]}]}});
        let judge = json!({"type": "result", "is_error": false, "total_cost_usd": 0.004,
            "structured_output": {"score": 4, "confident_wrong": false, "note": "fine"}});
        let answer = json!({"type": "result", "is_error": false, "result": "An answer.",
            "total_cost_usd": 0.02, "num_turns": 2, "duration_ms": 1200,
            "usage": {"input_tokens": 300, "output_tokens": 40}});
        for (name, value) in [("gen", generated), ("judge", judge), ("answer", answer)] {
            std::fs::write(self.root.join(format!("{name}.json")), value.to_string()).unwrap();
        }
        let script = format!(
            "#!/bin/sh\ninput=$(cat)\nprintf '%s\\n' \"$*\" >> '{log}'\ncase \"$input\" in\n  \
             *\"You write benchmark questions\"*) cat '{root}/gen.json';;\n  \
             *\"Score an assistant\"*) cat '{root}/judge.json';;\n  \
             *) cat '{root}/answer.json';;\nesac\n",
            log = self.log().display(),
            root = self.root.display()
        );
        let path = bin.join("claude");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        command
            .env_clear()
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.join("bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("HOME", self.home())
            .env("TMPDIR", std::env::temp_dir())
            .env("GRAPHERSAL_MEMORY_DIR", self.data_dir())
            .env("GRAPHERSAL_MEMORY_RUN_DIR", self.root.join("run"))
            .current_dir(&self.root)
            .args(args);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).stdin(Stdio::null()).output().unwrap()
    }

    /// Creates the live store (a no-daemon MCP session with no input).
    fn create_store(&self) {
        let out = self.run(&[
            "mcp",
            "--no-daemon",
            "--project",
            &self.project().display().to_string(),
        ]);
        assert!(out.status.success(), "{}", text(&out));
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn files_under(dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.filter_map(Result::ok).map(|e| e.path()) {
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push((path.clone(), std::fs::read(&path).unwrap()));
        }
    }
    out.sort();
}

#[test]
fn a_dry_run_calls_no_harness_and_writes_a_report() {
    let env = Env::new();
    env.claude_files(&env.home().join(".claude"));
    std::fs::create_dir_all(env.project()).unwrap();
    env.create_store();
    let project = env.project().display().to_string();
    let out = env.run(&["bench", "--dry-run", "--project", &project]);
    assert!(out.status.success(), "{}", text(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Dry run: a fake harness answers; nothing is spent."),
        "{stdout}"
    );
    assert!(stdout.contains("dry run: nothing was spent"), "{stdout}");
    assert!(
        env.calls().is_empty(),
        "the fake claude was called: {:?}",
        env.calls()
    );
}

#[test]
fn a_run_through_the_harness_reads_a_copy_and_leaves_the_live_files_alone() {
    let env = Env::new();
    // The sources come from --claude-dir; the default Claude Code directory has nothing.
    let sources = env.root.join("claude-copy");
    env.claude_files(&sources);
    std::fs::create_dir_all(env.project()).unwrap();
    env.create_store();
    let live_store = env.data_dir().join("stores").join("default");
    let mut before = Vec::new();
    files_under(&live_store, &mut before);
    files_under(&sources, &mut before);
    let project = env.project().display().to_string();

    // Above the budget: refused, nothing called.
    let out = env.run(&[
        "bench",
        "--yes",
        "--project",
        &project,
        "--claude-dir",
        &sources.display().to_string(),
        "--budget",
        "0.05",
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("Refused"), "{}", text(&out));
    assert!(env.calls().is_empty());

    let out = env.run(&[
        "bench",
        "--yes",
        "--project",
        &project,
        "--claude-dir",
        &sources.display().to_string(),
        "--arms",
        "none,files,graph",
    ]);
    assert!(out.status.success(), "{}", text(&out));
    let calls = env.calls();
    // 1 generation + 3 warm-ups + 2 questions x 3 arms answers + as many judge calls.
    assert_eq!(calls.len(), 1 + 3 + 6 + 6, "{calls:#?}");
    for call in &calls {
        assert!(call.contains("--strict-mcp-config"), "{call}");
        assert!(call.contains("--no-session-persistence"), "{call}");
        assert!(call.starts_with("-p --model sonnet"), "{call}");
        assert!(
            !call.contains(&env.home().join(".claude").display().to_string()),
            "{call}"
        );
        assert!(!call.contains(&live_store.display().to_string()), "{call}");
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("score 4.00/5"), "{stdout}");
    let mut after = Vec::new();
    files_under(&live_store, &mut after);
    files_under(&sources, &mut after);
    assert_eq!(
        before, after,
        "the live store and the sources are unchanged"
    );

    let bench_dir = env.data_dir().join("bench");
    let run = std::fs::read_dir(&bench_dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.join("report.json").exists())
        .unwrap();
    let report: Value =
        serde_json::from_slice(&std::fs::read(run.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["questions"], 2);
    // 6 answers x $0.02 + 6 judgings x $0.004 + 3 warm-ups x $0.02 + $0.05 for the questions.
    let total = report["spent_usd"]["total"].as_f64().unwrap();
    assert!(
        (total - (0.12 + 0.024 + 0.06 + 0.05)).abs() < 1e-9,
        "{total}"
    );
    assert!(
        bench_dir
            .join("questions")
            .read_dir()
            .unwrap()
            .next()
            .is_some()
    );
}
