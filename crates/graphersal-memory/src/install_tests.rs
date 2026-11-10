//! `install`, `uninstall`, `import`, `status`, `doctor` and the hooks on temporary home and data
//! directories (never the user's), with the memory served in the test process (a
//! `StoreRuntime` and in-memory pipes instead of the daemon).

use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::agents::Kind;
use crate::cli::Profile as ProfileKind;
use crate::client::McpClient;
use crate::import::{self, Caller, ImportState};
use crate::install::{self, ClaudePaths, InstallOptions, Setup, UninstallOptions};
use crate::mcp::{self, StoreRuntime};
use crate::mcp_tests::TempDir;
use crate::{hook, registry, store};

/// A setup for the home `home` and the data directory `data`, with a binary path that does not
/// need to exist.
pub(crate) fn setup_in(home: &Path, data: &Path) -> Setup {
    let paths =
        ClaudePaths::resolve(&|name| (name == "HOME").then(|| home.display().to_string())).unwrap();
    Setup {
        paths,
        opencode: crate::opencode::OpenCodePaths::of_home(home),
        codex: crate::codex::CodexPaths::of_home(home),
        cursor: crate::cursor::CursorPaths::of_home(home, data),
        search_path: Some(std::ffi::OsString::new()),
        data_dir: data.to_path_buf(),
        run_dir: short_run_dir(),
        exe: PathBuf::from("/opt/gm/bin/graphersal-memory"),
        dir_args: vec!["--data-dir".into(), data.display().to_string()],
        hooks: install::HookForm::posix(),
        // No network in tests: a model download fails at once (nothing listens on port 1).
        source: crate::download::Source {
            endpoint: "http://127.0.0.1:1".into(),
            token: None,
        },
    }
}

/// A run directory short enough for a socket path (never created by these tests unless a
/// daemon would start, which they do not).
fn short_run_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "gmr{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// The server threads of the connections.
type Servers = Arc<Mutex<Vec<JoinHandle<Result<(), String>>>>>;

/// The memory served in this process: one store, a connection per project.
struct Memory {
    runtime: Arc<StoreRuntime>,
    servers: Servers,
}

impl Memory {
    fn open(data: &Path) -> Self {
        let runtime = StoreRuntime::open(
            registry::DEFAULT_STORE,
            &store::default_store_dir(data),
            Arc::new(mcp::endpoint()),
        )
        .unwrap();
        Self {
            runtime: Arc::new(runtime),
            servers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// A connector like the daemon's.
    fn connector(&self) -> impl FnMut(&str) -> Result<Box<dyn Caller>, String> + Send + 'static {
        let runtime = self.runtime.clone();
        let servers = self.servers.clone();
        move |project: &str| -> Result<Box<dyn Caller>, String> {
            let (server_in, to) = std::io::pipe().map_err(|e| e.to_string())?;
            let (from, server_out) = std::io::pipe().map_err(|e| e.to_string())?;
            let runtime = runtime.clone();
            let project = project.to_string();
            let server = std::thread::spawn(move || {
                runtime.serve(
                    ProfileKind::Use,
                    &project,
                    None,
                    BufReader::new(server_in),
                    server_out,
                )
            });
            servers.lock().unwrap().push(server);
            Ok(Box::new(McpClient::start(BufReader::new(from), to)?))
        }
    }

    /// One call in `project`.
    fn call(&self, project: &str, name: &str, arguments: Value) -> Value {
        let mut connect = self.connector();
        let mut caller = connect(project).unwrap();
        caller.call(name, arguments).unwrap()
    }

    fn close(self) {
        let servers: Vec<_> = self.servers.lock().unwrap().drain(..).collect();
        for server in servers {
            assert_eq!(server.join().unwrap(), Ok(()));
        }
        self.runtime.close().unwrap();
    }
}

fn run_install(setup: &Setup, memory: &Memory, options: InstallOptions) -> (u8, String) {
    let mut connect = memory.connector();
    let mut out = Vec::new();
    let code = install::install(setup, &options, &mut connect, &mut &b""[..], &mut out).unwrap();
    (code, String::from_utf8(out).unwrap())
}

fn yes() -> InstallOptions {
    InstallOptions {
        yes: true,
        ..InstallOptions::default()
    }
}

fn run_uninstall(setup: &Setup, options: UninstallOptions) -> (u8, String) {
    let mut out = Vec::new();
    let code = install::uninstall(setup, &options, &mut &b""[..], &mut out).unwrap();
    (code, String::from_utf8(out).unwrap())
}

fn uninstall_yes() -> UninstallOptions {
    // These tests check what uninstall gives back of the harness's files; the export of the
    // memories into the file memory has its own tests (`export_*`).
    UninstallOptions {
        yes: true,
        no_export: true,
        ..UninstallOptions::default()
    }
}

/// The bytes of every file under `dir` (relative path -> content).
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                out.push((path.strip_prefix(dir).unwrap().to_path_buf(), Vec::new()));
                stack.push(path);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                out.push((path.strip_prefix(dir).unwrap().to_path_buf(), bytes));
            }
        }
    }
    out.sort();
    out
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// A home with Claude Code files in an unusual but valid formatting.
fn existing_home(home: &Path) {
    std::fs::write(
        home.join(".claude.json"),
        "{\"numStartups\": 3, \"theme\":\"dark\",\n \"mcpServers\": {\"other\": {\"command\":\"x\",\"args\":[]}}}\n",
    )
    .unwrap();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(
        home.join(".claude").join("settings.json"),
        "{\n    \"model\": \"opus\",\n    \"hooks\": {\n        \"SessionStart\": [\n            {\"hooks\": [{\"type\": \"command\", \"command\": \"echo mine\"}]}\n        ]\n    }\n}",
    )
    .unwrap();
    std::fs::write(
        home.join(".claude").join("CLAUDE.md"),
        "# My rules\n\nBe brief.",
    )
    .unwrap();
}

#[test]
fn install_then_uninstall_leaves_existing_files_byte_identical() {
    let home = TempDir::new("rt-home");
    let data = TempDir::new("rt-data");
    existing_home(home.path());
    let before = snapshot(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());

    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    let config: Value = serde_json::from_str(&read(&setup.paths.user_config)).unwrap();
    assert_eq!(
        config["mcpServers"]["graphersal-memory"],
        setup.mcp_entry("use")
    );
    assert_eq!(config["mcpServers"]["other"]["command"], "x");
    assert!(config["mcpServers"]["graphersal-memory-develop"].is_null());
    assert_eq!(config["theme"], "dark");
    let settings: Value = serde_json::from_str(&read(&setup.paths.settings)).unwrap();
    assert_eq!(
        settings["hooks"]["SessionStart"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        settings["hooks"]["SessionStart"][1],
        setup.hook_group("session-start")
    );
    assert_eq!(
        settings["hooks"]["SessionEnd"][0],
        setup.hook_group("session-end")
    );
    assert_eq!(
        settings["hooks"]["SessionStart"][1]["hooks"][0]["command"],
        format!(
            "\"/opt/gm/bin/graphersal-memory\" hook session-start --data-dir \"{}\"",
            data.path().display()
        )
    );
    let rules = read(&setup.paths.instructions);
    assert!(
        rules.starts_with("# My rules\n\nBe brief.\n\n<!-- graphersal-memory:begin"),
        "{rules}"
    );
    assert!(out.contains("backup:"), "{out}");
    assert!(crate::config::path(data.path()).exists());

    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), before, "{out}");
    memory.close();
}

#[test]
fn install_then_uninstall_on_an_empty_home_leaves_nothing() {
    let home = TempDir::new("empty-home");
    let data = TempDir::new("empty-data");
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    let config = read(&setup.paths.user_config);
    assert!(
        config.starts_with("{\n  \"mcpServers\": {\n    \"graphersal-memory\": {"),
        "{config}"
    );
    assert!(setup.paths.settings.exists() && setup.paths.instructions.exists());
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    assert!(
        snapshot(home.path()).is_empty(),
        "{:?}",
        snapshot(home.path())
    );
    memory.close();
}

#[test]
fn a_second_install_changes_nothing_and_a_dry_run_changes_nothing() {
    let home = TempDir::new("idem-home");
    let data = TempDir::new("idem-data");
    existing_home(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());

    let before = snapshot(home.path());
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            dry_run: true,
            ..InstallOptions::default()
        },
    );
    assert_eq!(code, 0);
    assert!(out.contains("Dry run"), "{out}");
    assert!(out.contains("mcpServers"), "{out}");
    assert_eq!(snapshot(home.path()), before);
    assert!(!data.path().join("install").exists());

    let (code, _) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0);
    let installed = snapshot(home.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), installed);
    assert!(out.contains("already up to date"), "{out}");
    assert!(out.contains("already registered"), "{out}");

    // Without --yes and without an answer, nothing happens.
    std::fs::remove_file(&setup.paths.instructions).unwrap();
    let mut connect = memory.connector();
    let mut out = Vec::new();
    let code = install::install(
        &setup,
        &InstallOptions::default(),
        &mut connect,
        &mut &b"n\n"[..],
        &mut out,
    )
    .unwrap();
    assert_eq!(code, 1);
    assert!(!setup.paths.instructions.exists());
    memory.close();
}

#[test]
fn uninstall_after_claude_rewrote_the_files_removes_only_our_parts() {
    let home = TempDir::new("struct-home");
    let data = TempDir::new("struct-data");
    existing_home(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    run_install(&setup, &memory, yes());
    // Claude Code rewrites its configuration (here: one more key) and the user adds a rule.
    let config = read(&setup.paths.user_config).replacen('{', "{\"tipsShown\": 4, ", 1);
    std::fs::write(&setup.paths.user_config, &config).unwrap();
    let settings = read(&setup.paths.settings).replacen('{', "{\"theme\": \"light\", ", 1);
    std::fs::write(&setup.paths.settings, &settings).unwrap();
    let rules = format!("{}\nMore of mine.\n", read(&setup.paths.instructions));
    std::fs::write(&setup.paths.instructions, &rules).unwrap();

    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    let config: Value = serde_json::from_str(&read(&setup.paths.user_config)).unwrap();
    assert_eq!(config["tipsShown"], 4);
    assert!(config["mcpServers"]["graphersal-memory"].is_null());
    assert_eq!(config["mcpServers"]["other"]["command"], "x");
    let settings: Value = serde_json::from_str(&read(&setup.paths.settings)).unwrap();
    assert_eq!(settings["theme"], "light");
    assert_eq!(
        settings["hooks"]["SessionStart"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        settings["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        "echo mine"
    );
    assert!(settings["hooks"].get("SessionEnd").is_none(), "{settings}");
    let rules = read(&setup.paths.instructions);
    assert!(!rules.contains("graphersal-memory:begin"), "{rules}");
    assert!(
        rules.contains("Be brief.") && rules.contains("More of mine."),
        "{rules}"
    );
    memory.close();
}

#[test]
fn a_hand_edit_in_the_block_is_a_conflict_never_overwritten() {
    let home = TempDir::new("conf-home");
    let data = TempDir::new("conf-data");
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    run_install(&setup, &memory, yes());
    let edited = read(&setup.paths.instructions).replace("Save with", "Please save with");
    std::fs::write(&setup.paths.instructions, &edited).unwrap();

    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("conflict:") && out.contains("edited by hand"),
        "{out}"
    );
    assert_eq!(read(&setup.paths.instructions), edited);
    let status = install::status_lines(&setup).join("\n");
    assert!(
        status.contains("rule block") && status.contains("CONFLICT"),
        "{status}"
    );

    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("not removed"), "{out}");
    assert_eq!(read(&setup.paths.instructions), edited);
    // Everything else went: install created the other files, so they are gone again.
    assert!(!setup.paths.user_config.exists());
    assert!(!setup.paths.settings.exists());
    memory.close();
}

#[test]
fn a_foreign_entry_under_our_name_is_a_conflict() {
    let home = TempDir::new("foreign-home");
    let data = TempDir::new("foreign-data");
    std::fs::write(
        home.path().join(".claude.json"),
        "{\"mcpServers\": {\"graphersal-memory\": {\"command\": \"npx\", \"args\": [\"other\"]}}}",
    )
    .unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("is not ours"), "{out}");
    let config: Value = serde_json::from_str(&read(&setup.paths.user_config)).unwrap();
    assert_eq!(config["mcpServers"]["graphersal-memory"]["command"], "npx");
    memory.close();
}

#[test]
fn the_develop_server_is_off_until_enabled_and_can_be_disabled_again() {
    let home = TempDir::new("dev-home");
    let data = TempDir::new("dev-data");
    existing_home(home.path());
    let before = snapshot(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    run_install(&setup, &memory, yes());
    let developing = || -> Value {
        let config: Value = serde_json::from_str(&read(&setup.paths.user_config)).unwrap();
        config["mcpServers"]["graphersal-memory-develop"].clone()
    };
    assert!(developing().is_null());
    let status = install::status_lines(&setup).join("\n");
    assert!(
        status.contains("graphersal-memory-develop: not registered (off"),
        "{status}"
    );

    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            yes: true,
            enable_develop: true,
            ..InstallOptions::default()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert_eq!(developing(), setup.mcp_entry("develop"));
    let (code, out) = run_uninstall(
        &setup,
        UninstallOptions {
            yes: true,
            disable_develop_only: true,
            ..UninstallOptions::default()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert!(developing().is_null());
    let config: Value = serde_json::from_str(&read(&setup.paths.user_config)).unwrap();
    assert_eq!(
        config["mcpServers"]["graphersal-memory"],
        setup.mcp_entry("use")
    );
    // A full uninstall still gives back the original bytes.
    run_uninstall(&setup, uninstall_yes());
    assert_eq!(snapshot(home.path()), before);
    memory.close();
}

/// A Claude Code memory directory for the project directory `project`, with a transcript
/// naming it.
fn memory_fixture(home: &Path, project: &Path) -> PathBuf {
    let name = import::claude_slug(&project.display().to_string());
    let dir = home.join(".claude").join("projects").join(&name);
    let memory = dir.join("memory");
    std::fs::create_dir_all(&memory).unwrap();
    std::fs::write(
        dir.join("0001.jsonl"),
        format!(
            "{}\n",
            json!({"cwd": project.display().to_string(), "type": "user"})
        ),
    )
    .unwrap();
    std::fs::write(
        memory.join("MEMORY.md"),
        "# Index\n- [Graph first](graph-first.md) — study the graph first\n\
         - [Release plan](release-plan.md) — how we release\n",
    )
    .unwrap();
    std::fs::write(
        memory.join("graph-first.md"),
        "---\nname: graph-first\ndescription: Study the repository through the graph first\n\
         metadata:\n  type: feedback\n---\n\nUse the graph before grep. See [[release-plan]].\n\n\
         **Why:** it is faster.\n**How to apply:** query first.\n",
    )
    .unwrap();
    std::fs::write(
        memory.join("release-plan.md"),
        "---\nname: release-plan\ndescription: Releases go out on Fridays\ntype: project\n---\n\nFriday releases.\n",
    )
    .unwrap();
    std::fs::write(
        memory.join("Odd_Name.md"),
        "---\ndescription: no type here\n---\nSomething.\n",
    )
    .unwrap();
    memory
}

#[test]
fn the_file_memory_is_imported_with_kinds_links_and_order_and_reimport_is_idempotent() {
    let home = TempDir::new("imp-home");
    let data = TempDir::new("imp-data");
    let work = TempDir::new("imp-work");
    let project_dir = mcp::canonical(work.path());
    std::fs::create_dir_all(project_dir.join(".git")).unwrap();
    let memory_dir = memory_fixture(home.path(), &project_dir);
    // A directory whose project is gone is skipped.
    std::fs::create_dir_all(
        home.path()
            .join(".claude/projects/-nonexistent-place-xyz/memory"),
    )
    .unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let project = project_dir.display().to_string();

    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("3 new and 0 changed memory files"), "{out}");
    assert!(
        out.contains("imported 3 memories (3 new, 0 updated) into 1 project"),
        "{out}"
    );
    assert!(out.contains("skipped 1 directory"), "{out}");
    assert!(out.contains("1 without a known type"), "{out}");

    let listed = memory.call(&project, "memories", json!({}));
    let slugs: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs, vec!["graph-first", "release-plan", "odd-name"]);
    let first = memory.call(&project, "memory", json!({"slug": "graph-first"}));
    assert_eq!(first["kind"], "feedback");
    assert_eq!(first["scope"], "project");
    assert_eq!(first["title"], "Graph first");
    assert_eq!(first["hook"], "study the graph first");
    assert_eq!(first["links"], json!(["release-plan"]));
    let body = first["body"].as_str().unwrap();
    assert!(body.starts_with("Use the graph before grep."), "{body}");
    assert!(
        body.contains("**Why:** it is faster.\n**How to apply:** query first."),
        "{body}"
    );
    let release = memory.call(&project, "memory", json!({"slug": "release-plan"}));
    assert_eq!(release["kind"], "project");
    assert_eq!(release["linked_from"], json!(["graph-first"]));
    // Another project does not see them.
    let other = memory.call("/somewhere/else", "memories", json!({}));
    assert_eq!(other, json!([]));

    // A second import sends nothing; a changed file is updated, not duplicated.
    let mut connect = memory.connector();
    let mut out = Vec::new();
    crate::commands::run_import(&setup, false, &mut connect, &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("0 new, 0 changed, 3 unchanged"), "{out}");
    std::fs::write(
        memory_dir.join("release-plan.md"),
        "---\nname: release-plan\ndescription: Releases go out on Mondays\ntype: project\n---\n\nMonday releases.\n",
    )
    .unwrap();
    let mut out = Vec::new();
    let code = crate::commands::run_import(&setup, false, &mut connect, &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("(0 new, 1 updated)"), "{out}");
    let listed = memory.call(&project, "memories", json!({}));
    assert_eq!(listed.as_array().unwrap().len(), 3);
    let release = memory.call(&project, "memory", json!({"slug": "release-plan"}));
    assert_eq!(release["summary"], "Releases go out on Mondays");
    assert_eq!(ImportState::load(data.path()).unwrap().len(), 3);
    // The files were only read.
    assert!(memory_dir.join("Odd_Name.md").exists());
    memory.close();
}

#[test]
fn the_session_start_hook_imports_the_projects_changed_files() {
    let home = TempDir::new("hook-home");
    let data = TempDir::new("hook-data");
    let work = TempDir::new("hook-work");
    let project_dir = mcp::canonical(work.path());
    let memory_dir = memory_fixture(home.path(), &project_dir);
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let mut connect = memory.connector();
    let deadline = Instant::now() + Duration::from_secs(60);
    let outcome = hook::work(
        hook::HookEvent::SessionStart,
        Some(&project_dir),
        &setup.paths,
        data.path(),
        &mut connect,
        deadline,
    );
    assert!(outcome.notes.is_empty(), "{:?}", outcome.notes);
    let line = outcome.line.unwrap();
    assert!(
        line.contains("Imported 3 new or changed memory files"),
        "{line}"
    );
    std::fs::write(
        memory_dir.join("new-one.md"),
        "---\ndescription: a new one\ntype: user\n---\nNew.\n",
    )
    .unwrap();
    let outcome = hook::work(
        hook::HookEvent::SessionEnd,
        Some(&project_dir),
        &setup.paths,
        data.path(),
        &mut connect,
        deadline,
    );
    assert_eq!(outcome.line, None);
    let project = project_dir.display().to_string();
    let found = memory.call(&project, "memory", json!({"slug": "new-one"}));
    assert_eq!(found["kind"], "user");
    let outcome = hook::work(
        hook::HookEvent::SessionStart,
        Some(&project_dir),
        &setup.paths,
        data.path(),
        &mut connect,
        deadline,
    );
    assert!(!outcome.line.unwrap().contains("Imported"));
    memory.close();
}

#[test]
fn status_and_doctor_report_the_installation() {
    let home = TempDir::new("doc-home");
    let data = TempDir::new("doc-data");
    let setup = setup_in(home.path(), data.path());
    let findings = install::doctor(&setup);
    assert!(
        findings
            .iter()
            .any(|f| f.problem && f.text.contains("graphersal-memory is not registered"))
    );
    let memory = Memory::open(data.path());
    run_install(&setup, &memory, yes());
    let status = install::status_lines(&setup).join("\n");
    assert!(
        status.contains("MCP server graphersal-memory (use): installed"),
        "{status}"
    );
    assert!(status.contains("hook SessionStart"), "{status}");
    assert!(
        status.contains("rule block") && status.contains(": installed"),
        "{status}"
    );
    let findings = install::doctor(&setup);
    let problems: Vec<&str> = findings
        .iter()
        .filter(|f| f.problem)
        .map(|f| f.text.as_str())
        .collect();
    // The test binary path does not exist: the only problem doctor finds.
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains("/opt/gm/bin/graphersal-memory does not exist"),
        "{problems:?}"
    );
    memory.close();
}

#[test]
fn purge_refuses_a_directory_that_is_not_ours() {
    let home = TempDir::new("purge-home");
    let data = TempDir::new("purge-data");
    std::fs::write(data.path().join("precious.txt"), "keep").unwrap();
    let setup = setup_in(home.path(), data.path());
    let mut out = Vec::new();
    let err = install::uninstall(
        &setup,
        &UninstallOptions {
            yes: true,
            purge: true,
            no_export: true,
            ..UninstallOptions::default()
        },
        &mut &b""[..],
        &mut out,
    )
    .unwrap_err();
    assert!(err.contains("does not look like"), "{err}");
    assert!(data.path().join("precious.txt").exists());
}

#[test]
fn install_asks_for_summaries_and_keeps_them_off_by_default() {
    let summaries_on = |data: &Path| crate::config::Config::load(data).unwrap().summaries.enabled;
    // --yes alone: off, and said so.
    let home = TempDir::new("sum-home");
    let data = TempDir::new("sum-data");
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Index past sessions"), "{out}");
    assert!(out.contains("summaries: off unless you agree"), "{out}");
    assert!(
        out.contains("summaries: off (raw turns are indexed"),
        "{out}"
    );
    assert!(
        out.contains("past sessions: indexed at the next session start"),
        "{out}"
    );
    assert!(!summaries_on(data.path()));
    // --yes --summaries: on without a question.
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            yes: true,
            summaries: true,
            ..InstallOptions::default()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert!(summaries_on(data.path()), "{out}");
    memory.close();

    // Asked: "y" to proceed, then "y" to summaries; an empty answer is no.
    for (answers, expected) in [(&b"y\ny\n"[..], true), (&b"y\n\n"[..], false)] {
        let home = TempDir::new("ask-home");
        let data = TempDir::new("ask-data");
        let setup = setup_in(home.path(), data.path());
        let memory = Memory::open(data.path());
        let mut connect = memory.connector();
        let mut out = Vec::new();
        let mut input = answers;
        let code = install::install(
            &setup,
            &InstallOptions::default(),
            &mut connect,
            &mut input,
            &mut out,
        )
        .unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("Summarize finished sessions"), "{out}");
        assert_eq!(summaries_on(data.path()), expected, "{out}");
        memory.close();
    }
}

/// The rule block text of 0.0.0 (stages M3-M5), as installed blocks still carry it.
const BLOCK_0_0_0: &str = "\
## Memory: graphersal-memory

Your long-term memory of this user lives in a local graph, served by the MCP server
`graphersal-memory`, not in memory files.

- Search it before you answer from memory or ask the user something they may have told you
  before: `memories(text)` for rules, decisions and preferences, `recall(text)` for everything,
  `memory(slug)` to read one in full.
- Save with `remember`: search with `memories(text)` first, so you replace a memory instead of
  duplicating it; scope \"global\" for rules and preferences that hold in every project. Remove
  with `forget`.
- Do not write memory files (`MEMORY.md` and the files beside it) any more. Files written anyway
  are imported into the graph at the next session start.
- Past sessions (`sessions(text)`, `session(id)`) are not indexed yet in this version.
";

/// A managed block with `text` and the fingerprint `fingerprint` on its begin line.
fn block_with(text: &str, fingerprint: &str) -> String {
    let current = crate::fsutil::fingerprint(install::RULE_BLOCK.as_bytes());
    install::block_text()
        .replace(install::RULE_BLOCK, text)
        .replace(&current, fingerprint)
}

/// The instruction file with the block of `install` replaced by `block`.
fn with_block(setup: &Setup, block: &str) -> String {
    let text = read(&setup.paths.instructions);
    let found = install::find_block(&text).unwrap().unwrap();
    format!("{}{block}{}", &text[..found.start], &text[found.end..])
}

#[test]
fn every_earlier_block_text_is_known_and_the_current_one_is_not_among_them() {
    let previous = crate::fsutil::fingerprint(BLOCK_0_0_0.as_bytes());
    assert!(
        install::PREVIOUS_BLOCK_FINGERPRINTS.contains(&previous.as_str()),
        "{previous}"
    );
    let current = crate::fsutil::fingerprint(install::RULE_BLOCK.as_bytes());
    assert!(!install::PREVIOUS_BLOCK_FINGERPRINTS.contains(&current.as_str()));
    assert!(!install::RULE_BLOCK.contains("not indexed yet"));
    let old = block_with(BLOCK_0_0_0, &previous);
    let found = install::find_block(&format!("# Mine\n\n{old}"))
        .unwrap()
        .unwrap();
    assert!(found.intact && found.known && !found.current);
}

#[test]
fn an_untouched_block_of_an_earlier_version_is_updated_and_uninstalled_cleanly() {
    let home = TempDir::new("old-home");
    let data = TempDir::new("old-data");
    existing_home(home.path());
    let before = snapshot(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    run_install(&setup, &memory, yes());
    let current = read(&setup.paths.instructions);
    // The file as an earlier version left it.
    let previous = crate::fsutil::fingerprint(BLOCK_0_0_0.as_bytes());
    let old = with_block(&setup, &block_with(BLOCK_0_0_0, &previous));
    std::fs::write(&setup.paths.instructions, &old).unwrap();
    let status = install::status_lines(&setup).join("\n");
    assert!(
        status.contains("rule block") && status.contains("written by an earlier version"),
        "{status}"
    );

    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("update the block to this version"), "{out}");
    assert_eq!(read(&setup.paths.instructions), current);
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), before, "{out}");

    // Uninstall straight from the earlier version's block removes it, and only it.
    run_install(&setup, &memory, yes());
    let old = with_block(&setup, &block_with(BLOCK_0_0_0, &previous));
    std::fs::write(&setup.paths.instructions, format!("{old}\nMore of mine.\n")).unwrap();
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    let rules = read(&setup.paths.instructions);
    assert!(!rules.contains("graphersal-memory:begin"), "{rules}");
    assert!(
        rules.contains("Be brief.") && rules.contains("More of mine."),
        "{rules}"
    );
    memory.close();
}

#[test]
fn a_hand_edited_block_of_an_earlier_version_is_still_a_conflict() {
    let home = TempDir::new("oldedit-home");
    let data = TempDir::new("oldedit-data");
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    run_install(&setup, &memory, yes());
    let previous = crate::fsutil::fingerprint(BLOCK_0_0_0.as_bytes());
    let edited = BLOCK_0_0_0.replace("Save with", "Please save with");
    let text = with_block(&setup, &block_with(&edited, &previous));
    std::fs::write(&setup.paths.instructions, &text).unwrap();

    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("conflict:") && out.contains("edited by hand"),
        "{out}"
    );
    assert_eq!(read(&setup.paths.instructions), text);
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("not removed"), "{out}");
    assert_eq!(read(&setup.paths.instructions), text);
    memory.close();
}

#[test]
fn a_block_of_an_unknown_version_is_kept_by_install_and_removed_by_uninstall() {
    let home = TempDir::new("newer-home");
    let data = TempDir::new("newer-data");
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    run_install(&setup, &memory, yes());
    // A newer version's text: untouched (its fingerprint matches) but not one this version knows.
    let newer = format!("{}- A rule of a newer version.\n", install::RULE_BLOCK);
    let fingerprint = crate::fsutil::fingerprint(newer.as_bytes());
    let text = with_block(&setup, &block_with(&newer, &fingerprint));
    std::fs::write(&setup.paths.instructions, &text).unwrap();
    let status = install::status_lines(&setup).join("\n");
    assert!(status.contains("written by another version"), "{status}");

    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("conflict:") && out.contains("newer?"), "{out}");
    assert_eq!(read(&setup.paths.instructions), text);
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    assert!(
        !setup.paths.instructions.exists()
            || !read(&setup.paths.instructions).contains("graphersal-memory:begin")
    );
    memory.close();
}

// ------------------------------------------------------------------------------------------
// M6b: the wizard, the auto-memory switch, the embedding model, the export back to files.
// ------------------------------------------------------------------------------------------

/// The memories of the store served in this process (as the daemon's `export_memories`).
struct InProcess<'a>(&'a Memory);

impl crate::export::Source for InProcess<'_> {
    fn memories(&mut self, project: &str) -> Result<Vec<crate::export::Row>, String> {
        let answer = self
            .0
            .runtime
            .call("export_memories", json!({"project": project}))?;
        Ok(crate::export::rows(&answer))
    }
}

fn run_uninstall_exporting(
    setup: &Setup,
    memory: &Memory,
    options: UninstallOptions,
    input: &[u8],
) -> (u8, String) {
    let mut out = Vec::new();
    let mut source = InProcess(memory);
    let mut input = input;
    let code =
        install::uninstall_with(setup, &options, Some(&mut source), &mut input, &mut out).unwrap();
    (code, String::from_utf8(out).unwrap())
}

fn run_wizard(setup: &Setup, memory: &Memory, input: &str) -> (u8, String) {
    let mut connect = memory.connector();
    let mut out = Vec::new();
    let options = InstallOptions {
        interactive: true,
        ..InstallOptions::default()
    };
    let mut input = input.as_bytes();
    let code = install::install(setup, &options, &mut connect, &mut input, &mut out).unwrap();
    (code, String::from_utf8(out).unwrap())
}

fn settings_json(setup: &Setup) -> Value {
    serde_json::from_str(&read(&setup.paths.settings)).unwrap()
}

#[test]
fn the_wizard_asks_each_step_and_its_answers_change_what_install_does() {
    let home = TempDir::new("wiz-home");
    let data = TempDir::new("wiz-data");
    let work = TempDir::new("wiz-work");
    let project_dir = mcp::canonical(work.path());
    memory_fixture(home.path(), &project_dir);
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    // Language: any (2); model: built-in (3); import: Enter (yes); auto-memory: Enter (off);
    // summaries: an invalid answer, then Enter (asked again: no default), then 2 (no);
    // develop: 1 (yes); summary: Enter applies.
    let (code, out) = run_wizard(&setup, &memory, "2\n3\n\n\nx\n\n2\n1\n\n");
    assert_eq!(code, 0, "{out}");
    for step in [
        "Step 1 of 7 \u{b7} Memory language",
        "Step 2 of 7 \u{b7} Embedding model",
        "Step 3 of 7 \u{b7} Import existing file memory",
        "Step 4 of 7 \u{b7} Claude Code's auto-memory",
        "Step 5 of 7 \u{b7} Session summaries",
        "Step 6 of 7 \u{b7} The develop server",
        "Step 7 of 7 \u{b7} Summary",
    ] {
        assert!(out.contains(step), "{step}\n{out}");
    }
    assert!(out.contains("potion-retrieval-32M"), "{out}");
    assert!(out.contains("English, ~130 MB, MIT"), "{out}");
    assert!(out.contains("100+ languages, ~530 MB, MIT"), "{out}");
    assert!(out.contains("not meaning; weaker search"), "{out}");
    assert!(out.contains("Please type a number from 1 to 2"), "{out}");
    assert!(out.matches(install::NOTHING_IS_LOST).count() >= 2, "{out}");
    // The answers took effect.
    let config = crate::config::Config::load(data.path()).unwrap();
    assert_eq!(config.language, Some(crate::language::Language::Any));
    assert_eq!(config.model.as_deref(), Some(crate::models::BUILTIN));
    assert!(!config.summaries.enabled);
    assert_eq!(settings_json(&setup)["autoMemoryEnabled"], false);
    let user_config: Value = serde_json::from_str(&read(&setup.paths.user_config)).unwrap();
    assert_eq!(
        user_config["mcpServers"]["graphersal-memory-develop"],
        setup.mcp_entry("develop")
    );
    let rules = read(&setup.paths.instructions);
    assert!(rules.contains(install::RULE_BLOCK_ANY), "{rules}");
    assert!(out.contains("imported 3 memories"), "{out}");
    let status = install::status_lines(&setup).join("\n");
    assert!(status.contains("memory language: any language"), "{status}");
    assert!(
        status.contains("rule block") && status.contains(": installed"),
        "{status}"
    );
    assert!(
        status.contains("auto-memory") && status.contains("set by install"),
        "{status}"
    );

    // Again: the current answers are the defaults, Enter everywhere changes nothing.
    let installed = snapshot(home.path());
    let (code, out) = run_wizard(&setup, &memory, "\n\n\n\n\n\n\n");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("already up to date"), "{out}");
    assert_eq!(snapshot(home.path()), installed);

    // Cancel at the summary (now 6 steps: nothing left to import): nothing changes.
    let (code, out) = run_wizard(&setup, &memory, "1\n\n\n\n\nn\n");
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("Cancelled"), "{out}");
    assert_eq!(snapshot(home.path()), installed);
    // The input ends before a question without a default: nothing changes either.
    let fresh_home = TempDir::new("wiz-home2");
    let fresh = setup_in(fresh_home.path(), data.path());
    std::fs::remove_dir_all(data.path().join("install")).unwrap();
    let (code, out) = run_wizard(&fresh, &memory, "\n\n\n");
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("Aborted: no answer"), "{out}");
    assert!(!fresh.paths.user_config.exists());
    memory.close();
}

#[test]
fn the_auto_memory_switch_goes_back_to_what_it_was() {
    let home = TempDir::new("am-home");
    let data = TempDir::new("am-data");
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    std::fs::write(
        home.path().join(".claude/settings.json"),
        "{\"autoMemoryEnabled\": true, \"model\": \"opus\"}\n",
    )
    .unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    assert_eq!(settings_json(&setup)["autoMemoryEnabled"], false);
    assert!(out.contains(install::NOTHING_IS_LOST), "{out}");
    // Re-run with --keep-auto-memory: back to true, the hooks stay.
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            yes: true,
            keep_auto_memory: true,
            ..InstallOptions::default()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert_eq!(settings_json(&setup)["autoMemoryEnabled"], true);
    assert!(settings_json(&setup)["hooks"]["SessionStart"].is_array());
    // The next plain re-run keeps that answer.
    run_install(&setup, &memory, yes());
    assert_eq!(settings_json(&setup)["autoMemoryEnabled"], true);
    // Switch off again in the wizard (language, model, auto-memory: 1, summaries, develop,
    // apply); the user edits the file; the structural uninstall restores `true`.
    let (code, out) = run_wizard(&setup, &memory, "\n\n1\n\n\n\n");
    assert_eq!(code, 0, "{out}");
    assert_eq!(settings_json(&setup)["autoMemoryEnabled"], false);
    let edited = read(&setup.paths.settings).replacen('{', "{\"theme\": \"light\", ", 1);
    std::fs::write(&setup.paths.settings, &edited).unwrap();
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    let settings = settings_json(&setup);
    assert_eq!(settings["autoMemoryEnabled"], true, "{settings}");
    assert_eq!(settings["theme"], "light");
    assert_eq!(settings["model"], "opus");
    assert!(settings.get("hooks").is_none(), "{settings}");
    memory.close();
}

#[test]
fn uninstall_exports_every_memory_back_including_ones_added_later_and_keeps_edited_files() {
    let home = TempDir::new("exp-home");
    let data = TempDir::new("exp-data");
    let work = TempDir::new("exp-work");
    let project_dir = mcp::canonical(work.path());
    let memory_dir = memory_fixture(home.path(), &project_dir);
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let project = project_dir.display().to_string();
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    // A memory added through MCP after install, and a global one.
    for (slug, scope) in [("added-later", "project"), ("tabs-not-spaces", "global")] {
        let saved = memory.call(
            &project,
            "remember",
            json!({"slug": slug, "kind": "feedback", "summary": format!("{slug} summary"),
                   "body": format!("{slug} body"), "why": "because", "scope": scope,
                   "links": ["graph-first"]}),
        );
        assert!(saved.to_string().contains(slug), "{saved}");
    }
    // The user edited one memory file since the import.
    let edited = memory_dir.join("release-plan.md");
    std::fs::write(&edited, "my own text\n").unwrap();

    let (code, out) = run_uninstall_exporting(
        &setup,
        &memory,
        UninstallOptions {
            yes: true,
            ..UninstallOptions::default()
        },
        b"",
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("export 5 memories of 1 project (1 global)"),
        "{out}"
    );
    assert!(out.contains("exported 5 memories"), "{out}");
    let added = read(&memory_dir.join("added-later.md"));
    assert!(
        added.starts_with(
            "---\nname: added-later\ndescription: added-later summary\nmetadata:\n  type: feedback\n---\n"
        ),
        "{added}"
    );
    assert!(added.contains("Related: [[graph-first]]"), "{added}");
    assert!(added.contains("**Why:** because"), "{added}");
    // The edited file is kept; the graph's version goes beside it.
    assert_eq!(read(&edited), "my own text\n");
    assert!(read(&memory_dir.join("release-plan.graphersal.md")).contains("Friday releases."));
    assert!(out.contains("release-plan.graphersal.md"), "{out}");
    // The index differs from the fixture's: kept, the missing line appended to it (user
    // decision 2026-11-10: no MEMORY.graphersal.md any more).
    let index = read(&memory_dir.join("MEMORY.md"));
    assert!(index.contains("(added-later.md)"), "{index}");
    assert_eq!(index.matches("(added-later.md)").count(), 1);
    assert!(!memory_dir.join("MEMORY.graphersal.md").exists());
    // The fixture's index lists neither added-later nor odd-name.
    assert!(out.contains("appended 2 index lines"), "{out}");
    // The global memory: a user rules file every project loads.
    let global = read(
        &home
            .path()
            .join(".claude/rules/graphersal-memory-global.md"),
    );
    assert!(global.contains("tabs-not-spaces summary"), "{global}");
    // Claude Code's settings are gone (install created them), the auto-memory with them.
    assert!(!setup.paths.settings.exists());
    // A second export finds everything in place.
    let report = crate::export::write(
        &setup.agent(),
        data.path(),
        &InProcess(&memory).memories_all(),
    )
    .unwrap();
    assert!(report.written.is_empty(), "{report:?}");
    assert!(report.appended.is_empty(), "{report:?}");
    memory.close();
}

impl InProcess<'_> {
    fn memories_all(&mut self) -> Vec<crate::export::Row> {
        crate::export::Source::memories(self, "").unwrap()
    }
}

#[test]
fn importing_an_export_gives_the_same_memories_back() {
    let home = TempDir::new("rtx-home");
    let data = TempDir::new("rtx-data");
    let work = TempDir::new("rtx-work");
    let project_dir = mcp::canonical(work.path());
    let original_dir = memory_fixture(home.path(), &project_dir);
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    run_install(&setup, &memory, yes());
    let other = TempDir::new("rtx-home2");
    let elsewhere = setup_in(other.path(), data.path());
    let rows = InProcess(&memory).memories_all();
    let report = crate::export::write(&elsewhere.agent(), data.path(), &rows).unwrap();
    assert_eq!(report.memories, 3, "{report:?}");
    let exported_dir = crate::agents::Agent::memory_dir(&elsewhere.agent(), &project_dir);
    let project = Some(project_dir.display().to_string());
    let before = import::read_dir(&original_dir, project.clone());
    let after = import::read_dir(&exported_dir, project);
    assert!(after.problems.is_empty(), "{:?}", after.problems);
    let key = |f: &import::MemoryFile| {
        (
            f.slug.clone(),
            f.kind.clone(),
            f.summary.clone(),
            f.body.clone(),
            f.why.clone(),
            f.how_to_apply.clone(),
            f.links.clone(),
        )
    };
    let mut a: Vec<_> = before.files.iter().map(key).collect();
    let mut b: Vec<_> = after.files.iter().map(key).collect();
    a.sort();
    b.sort();
    assert_eq!(a, b);
    // The listed memories keep their index line and order too.
    for slug in ["graph-first", "release-plan"] {
        let find = |dir: &import::MemoryDir| {
            let f = dir.files.iter().find(|f| f.slug == slug).unwrap();
            (f.title.clone(), f.hook.clone(), f.order)
        };
        assert_eq!(find(&before), find(&after), "{slug}");
    }
    memory.close();
}

#[test]
fn uninstall_asks_before_exporting_and_no_export_skips_it() {
    let home = TempDir::new("ask-exp-home");
    let data = TempDir::new("ask-exp-data");
    let work = TempDir::new("ask-exp-work");
    let project_dir = mcp::canonical(work.path());
    memory_fixture(home.path(), &project_dir);
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    run_install(&setup, &memory, yes());
    let (code, out) = run_uninstall_exporting(
        &setup,
        &memory,
        UninstallOptions {
            interactive: true,
            dry_run: true,
            ..UninstallOptions::default()
        },
        b"",
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("export every memory back"), "{out}");
    // Interactive: "n" to the export, "y" to proceed.
    let (code, out) = run_uninstall_exporting(
        &setup,
        &memory,
        UninstallOptions {
            interactive: true,
            ..UninstallOptions::default()
        },
        b"n\ny\n",
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Export 3 memories of 1 project back to Claude Code's file memory? [Y/n]"),
        "{out}"
    );
    assert!(out.contains("not exported (you answered no)"), "{out}");
    // Without a source the export cannot run, and nothing is removed.
    run_install(&setup, &memory, yes());
    let mut out = Vec::new();
    let err = install::uninstall(
        &setup,
        &UninstallOptions {
            yes: true,
            ..UninstallOptions::default()
        },
        &mut &b""[..],
        &mut out,
    )
    .unwrap_err();
    assert!(err.contains("nothing was removed"), "{err}");
    assert!(setup.paths.user_config.exists());
    memory.close();
}

// ------------------------------------------------------------------------------------------
// M7a: opencode.
// ------------------------------------------------------------------------------------------

fn opencode_only() -> InstallOptions {
    InstallOptions {
        yes: true,
        harnesses: Some(vec![Kind::OpenCode]),
        ..InstallOptions::default()
    }
}

fn opencode_uninstall() -> UninstallOptions {
    UninstallOptions {
        yes: true,
        no_export: true,
        harnesses: Some(vec![Kind::OpenCode]),
        ..UninstallOptions::default()
    }
}

/// A home with an opencode configuration in JSONC (comments, a trailing comma), the user's own
/// AGENTS.md and plugin.
fn existing_opencode_home(home: &Path) {
    let dir = home.join(".config/opencode");
    std::fs::create_dir_all(dir.join("plugins")).unwrap();
    std::fs::write(
        dir.join("opencode.jsonc"),
        "{\n  // my settings\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"model\": \"opencode/big-pickle\",\n  \"mcp\": {\n    \"other\": {\"type\": \"local\", \"command\": [\"x\"]}, /* keep */\n  },\n}\n",
    )
    .unwrap();
    std::fs::write(dir.join("AGENTS.md"), "# My opencode rules\n").unwrap();
    std::fs::write(
        dir.join("plugins/mine.js"),
        "export const Mine = async () => ({})\n",
    )
    .unwrap();
}

fn opencode_setup(home: &Path, data: &Path) -> Setup {
    // Resolved after the files exist (the configuration file install edits is the one there).
    setup_in(home, data)
}

#[test]
fn opencode_install_then_uninstall_leaves_existing_files_byte_identical() {
    let home = TempDir::new("oc-rt-home");
    let data = TempDir::new("oc-rt-data");
    existing_opencode_home(home.path());
    let before = snapshot(home.path());
    let setup = opencode_setup(home.path(), data.path());
    assert!(setup.opencode.config_file.ends_with("opencode.jsonc"));
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, opencode_only());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.starts_with("graphersal-memory install (opencode)"),
        "{out}"
    );
    assert!(
        !out.contains("Claude Code"),
        "no Claude Code wording: {out}"
    );
    let text = read(&setup.opencode.config_file);
    assert!(
        text.contains("// my settings") && text.contains("/* keep */"),
        "{text}"
    );
    let config = crate::jsonedit::value_of(&text).unwrap();
    assert_eq!(
        config["mcp"]["graphersal-memory"],
        crate::opencode::install::mcp_entry(&setup, "use", true)
    );
    assert_eq!(
        config["mcp"]["graphersal-memory"]["command"][0],
        "/opt/gm/bin/graphersal-memory"
    );
    assert_eq!(config["mcp"]["graphersal-memory-develop"]["enabled"], false);
    assert_eq!(
        config["instructions"],
        json!([setup.opencode.rules_file.display().to_string()])
    );
    assert_eq!(config["mcp"]["other"]["command"], json!(["x"]));
    let rules = read(&setup.opencode.rules_file);
    assert!(rules.starts_with("<!-- graphersal-memory:begin"), "{rules}");
    assert!(rules.contains("graphersal-memory_<function>"), "{rules}");
    assert_eq!(
        read(&home.path().join(".config/opencode/AGENTS.md")),
        "# My opencode rules\n",
        "the user's AGENTS.md is never touched"
    );
    let plugin = read(&setup.opencode.plugin_file);
    assert!(crate::opencode::is_our_plugin(&plugin), "{plugin}");
    assert!(plugin.contains("\"--data-dir\""), "{plugin}");
    // Nothing of Claude Code's.
    assert!(!setup.paths.user_config.exists() && !setup.paths.config_dir.exists());
    assert!(data.path().join("install/opencode.json").exists());
    assert!(!data.path().join("install/claude-code.json").exists());

    // Idempotent.
    let again = snapshot(home.path());
    let (code, out) = run_install(&setup, &memory, opencode_only());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), again, "{out}");
    assert!(out.contains("already up to date"), "{out}");

    let (code, out) = run_uninstall(&setup, opencode_uninstall());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), before, "{out}");
    assert!(!data.path().join("install/opencode.json").exists());
    memory.close();
}

#[test]
fn opencode_install_on_an_empty_home_and_after_an_edit_removes_only_our_parts() {
    let home = TempDir::new("oc-empty-home");
    let data = TempDir::new("oc-empty-data");
    let setup = opencode_setup(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, opencode_only());
    assert_eq!(code, 0, "{out}");
    let created = read(&setup.opencode.config_file);
    assert!(
        created.starts_with("{\n  \"$schema\": \"https://opencode.ai/config.json\""),
        "{created}"
    );
    // The user (or opencode) adds a setting afterwards.
    let edited = created.replacen("{\n", "{\n  \"theme\": \"tokyonight\",\n", 1);
    std::fs::write(&setup.opencode.config_file, &edited).unwrap();
    let (code, out) = run_uninstall(&setup, opencode_uninstall());
    assert_eq!(code, 0, "{out}");
    let left = crate::jsonedit::value_of(&read(&setup.opencode.config_file)).unwrap();
    assert_eq!(
        left,
        json!({"theme": "tokyonight", "$schema": "https://opencode.ai/config.json"})
    );
    assert!(!setup.opencode.rules_file.exists() && !setup.opencode.plugin_file.exists());
    assert!(
        !setup.opencode.config_dir.join("plugins").exists(),
        "the directory install made goes"
    );
    memory.close();

    // Untouched: nothing is left at all.
    let home = TempDir::new("oc-empty2-home");
    let data = TempDir::new("oc-empty2-data");
    let setup = opencode_setup(home.path(), data.path());
    let memory = Memory::open(data.path());
    assert_eq!(run_install(&setup, &memory, opencode_only()).0, 0);
    let (code, out) = run_uninstall(&setup, opencode_uninstall());
    assert_eq!(code, 0, "{out}");
    assert!(
        snapshot(home.path()).is_empty(),
        "{:?}",
        snapshot(home.path())
    );
    memory.close();
}

#[test]
fn opencode_conflicts_are_kept_and_the_develop_server_switches() {
    let home = TempDir::new("oc-conf-home");
    let data = TempDir::new("oc-conf-data");
    let dir = home.path().join(".config/opencode");
    std::fs::create_dir_all(dir.join("plugins")).unwrap();
    std::fs::write(
        dir.join("opencode.json"),
        "{\"mcp\": {\"graphersal-memory\": {\"type\": \"remote\", \"url\": \"http://x\"}}}",
    )
    .unwrap();
    std::fs::write(
        dir.join("plugins/graphersal-memory.js"),
        "// someone else's\n",
    )
    .unwrap();
    let setup = opencode_setup(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, opencode_only());
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("mcp.graphersal-memory exists and is not ours"),
        "{out}"
    );
    assert!(
        out.contains("graphersal-memory.js: the file exists"),
        "{out}"
    );
    assert_eq!(
        read(&dir.join("plugins/graphersal-memory.js")),
        "// someone else's\n"
    );
    let config = crate::jsonedit::value_of(&read(&dir.join("opencode.json"))).unwrap();
    assert_eq!(config["mcp"]["graphersal-memory"]["type"], "remote");
    memory.close();

    // The develop server: disabled until enabled, then disabled again by uninstall
    // --disable-develop.
    let home = TempDir::new("oc-dev-home");
    let data = TempDir::new("oc-dev-data");
    let setup = opencode_setup(home.path(), data.path());
    let memory = Memory::open(data.path());
    let options = InstallOptions {
        enable_develop: true,
        ..opencode_only()
    };
    assert_eq!(run_install(&setup, &memory, options).0, 0);
    let develop = |setup: &Setup| {
        crate::jsonedit::value_of(&read(&setup.opencode.config_file)).unwrap()["mcp"]
            ["graphersal-memory-develop"]["enabled"]
            .clone()
    };
    assert_eq!(develop(&setup), true);
    let status = crate::opencode::install::status_lines(&setup).join("\n");
    assert!(
        status.contains("graphersal-memory-develop: registered, enabled"),
        "{status}"
    );
    let (code, out) = run_uninstall(
        &setup,
        UninstallOptions {
            disable_develop_only: true,
            ..opencode_uninstall()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert_eq!(develop(&setup), false);
    assert!(setup.opencode.plugin_file.exists(), "everything else stays");
    let (code, out) = run_uninstall(&setup, opencode_uninstall());
    assert_eq!(code, 0, "{out}");
    assert!(
        snapshot(home.path()).is_empty(),
        "{:?}",
        snapshot(home.path())
    );
    memory.close();
}

// ------------------------------------------------------------------------------------------
// M7b: opencode 2.x, and the upgrade from 1.x.
// ------------------------------------------------------------------------------------------

/// The setup of `home` with opencode of `major` installed.
fn opencode_setup_for(home: &Path, data: &Path, major: crate::opencode::Major) -> Setup {
    let mut setup = setup_in(home, data);
    setup.opencode = crate::opencode::OpenCodePaths::of_home_for(home, major);
    setup
}

fn doctor_problems(setup: &Setup) -> Vec<String> {
    crate::opencode::install::doctor(setup)
        .into_iter()
        .filter(|f| f.problem)
        .map(|f| f.text)
        .collect()
}

#[test]
fn opencode_2x_install_then_uninstall_leaves_existing_files_byte_identical() {
    use crate::opencode::Major;
    let home = TempDir::new("oc2-rt-home");
    let data = TempDir::new("oc2-rt-data");
    existing_opencode_home(home.path());
    let before = snapshot(home.path());
    let setup = opencode_setup_for(home.path(), data.path(), Major::V2);
    assert!(setup.opencode.config_file.ends_with("opencode.jsonc"));
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, opencode_only());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("(opencode 2.x)"), "{out}");
    let text = read(&setup.opencode.config_file);
    assert!(
        text.contains("// my settings") && text.contains("/* keep */"),
        "{text}"
    );
    let config = crate::jsonedit::value_of(&text).unwrap();
    let servers = &config["mcp"]["servers"];
    assert_eq!(
        servers["graphersal-memory"],
        crate::opencode::install::mcp_entry(&setup, "use", true)
    );
    assert_eq!(servers["graphersal-memory"]["disabled"], false);
    assert_eq!(servers["graphersal-memory"]["codemode"], false);
    assert_eq!(servers["graphersal-memory-develop"]["disabled"], true);
    assert!(config["mcp"]["graphersal-memory"].is_null(), "no 1.x shape");
    assert_eq!(
        config["mcp"]["other"]["command"],
        json!(["x"]),
        "the user's entry stays"
    );
    assert_eq!(
        config["instructions"],
        json!([setup.opencode.rules_file.display().to_string()])
    );
    assert!(crate::opencode::is_our_plugin(&read(
        &setup.opencode.plugin_file
    )));
    assert_eq!(doctor_problems(&setup), Vec::<String>::new());
    let status = crate::opencode::install::status_lines(&setup).join("\n");
    assert!(status.contains("version: opencode 2.x (given)"), "{status}");
    assert!(
        status.contains("MCP server graphersal-memory (use): installed"),
        "{status}"
    );

    // Idempotent.
    let again = snapshot(home.path());
    let (code, out) = run_install(&setup, &memory, opencode_only());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), again, "{out}");

    // The develop server switches through `disabled`.
    let options = InstallOptions {
        enable_develop: true,
        ..opencode_only()
    };
    assert_eq!(run_install(&setup, &memory, options).0, 0);
    let develop = |setup: &Setup| {
        crate::jsonedit::value_of(&read(&setup.opencode.config_file)).unwrap()["mcp"]["servers"]
            ["graphersal-memory-develop"]["disabled"]
            .clone()
    };
    assert_eq!(develop(&setup), false);
    let (code, out) = run_uninstall(
        &setup,
        UninstallOptions {
            disable_develop_only: true,
            ..opencode_uninstall()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert_eq!(develop(&setup), true);

    let (code, out) = run_uninstall(&setup, opencode_uninstall());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), before, "{out}");
    memory.close();

    // An empty home: nothing is left at all.
    let home = TempDir::new("oc2-empty-home");
    let data = TempDir::new("oc2-empty-data");
    let setup = opencode_setup_for(home.path(), data.path(), Major::V2);
    let memory = Memory::open(data.path());
    assert_eq!(run_install(&setup, &memory, opencode_only()).0, 0);
    let created = crate::jsonedit::value_of(&read(&setup.opencode.config_file)).unwrap();
    assert!(created["mcp"]["servers"]["graphersal-memory"].is_object());
    let (code, out) = run_uninstall(&setup, opencode_uninstall());
    assert_eq!(code, 0, "{out}");
    assert!(
        snapshot(home.path()).is_empty(),
        "{:?}",
        snapshot(home.path())
    );
    memory.close();
}

#[test]
fn an_upgrade_from_1x_to_2x_moves_our_entries_out_of_config_json() {
    use crate::opencode::Major;
    let home = TempDir::new("oc12-home");
    let data = TempDir::new("oc12-data");
    let dir = home.path().join(".config/opencode");
    std::fs::create_dir_all(&dir).unwrap();
    // A 1.x user's configuration in `config.json`, which 2.x does not read.
    std::fs::write(
        dir.join("config.json"),
        "{\n  \"theme\": \"tokyonight\",\n  \"mcp\": {\"other\": {\"type\": \"local\", \"command\": [\"x\"]}}\n}\n",
    )
    .unwrap();
    let before = snapshot(home.path());
    let v1 = opencode_setup_for(home.path(), data.path(), Major::V1);
    assert!(v1.opencode.config_file.ends_with("config.json"));
    let memory = Memory::open(data.path());
    assert_eq!(run_install(&v1, &memory, opencode_only()).0, 0);
    let installed_v1 = read(&dir.join("config.json"));

    // opencode is upgraded to 2.x: doctor finds our entries in the wrong place.
    let v2 = opencode_setup_for(home.path(), data.path(), Major::V2);
    assert!(v2.opencode.config_file.ends_with("opencode.json"));
    let problems = doctor_problems(&v2).join("\n");
    assert!(
        problems.contains("MCP server graphersal-memory: installed, outdated (in ")
            && problems.contains("config.json, which opencode 2.x does not read"),
        "{problems}"
    );
    assert!(
        problems.contains("rules file in instructions: installed, outdated"),
        "{problems}"
    );
    // install moves them: out of config.json, into opencode.json in 2.x's shape.
    let (code, out) = run_install(&v2, &memory, opencode_only());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("does not read it"), "{out}");
    let old = crate::jsonedit::value_of(&read(&dir.join("config.json"))).unwrap();
    assert_eq!(
        old,
        json!({"theme": "tokyonight", "mcp": {"other": {"type": "local", "command": ["x"]}}}),
        "only our parts left config.json"
    );
    let new = crate::jsonedit::value_of(&read(&dir.join("opencode.json"))).unwrap();
    assert_eq!(
        new["mcp"]["servers"]["graphersal-memory"],
        crate::opencode::install::mcp_entry(&v2, "use", true)
    );
    assert_eq!(
        new["instructions"][0],
        v2.opencode.rules_file.display().to_string()
    );
    assert_eq!(doctor_problems(&v2), Vec::<String>::new());
    let again = snapshot(home.path());
    let (code, out) = run_install(&v2, &memory, opencode_only());
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        snapshot(home.path()),
        again,
        "idempotent after the move: {out}"
    );
    assert_ne!(read(&dir.join("config.json")), installed_v1);

    // uninstall gives every file back.
    let (code, out) = run_uninstall(&v2, opencode_uninstall());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), before, "{out}");
    memory.close();
}

#[test]
fn an_upgrade_from_1x_to_2x_moves_entries_of_the_1x_shape() {
    use crate::opencode::Major;
    let home = TempDir::new("oc12s-home");
    let data = TempDir::new("oc12s-data");
    existing_opencode_home(home.path());
    let before = snapshot(home.path());
    let v1 = opencode_setup_for(home.path(), data.path(), Major::V1);
    let memory = Memory::open(data.path());
    let options = InstallOptions {
        enable_develop: true,
        ..opencode_only()
    };
    assert_eq!(run_install(&v1, &memory, options).0, 0);

    let v2 = opencode_setup_for(home.path(), data.path(), Major::V2);
    let inspection = crate::opencode::install::inspect(&v2);
    assert!(
        matches!(&inspection.use_server, install::PartState::Outdated(why)
            if why.contains("1.x shape (mcp.graphersal-memory)")),
        "{:?}",
        inspection.use_server
    );
    assert!(
        inspection.develop_server.1,
        "still enabled in the 1.x entry"
    );
    assert!(!doctor_problems(&v2).is_empty());
    let (code, out) = run_install(&v2, &memory, opencode_only());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("move from mcp.graphersal-memory (the opencode 1.x shape) to mcp.servers"),
        "{out}"
    );
    let text = read(&v2.opencode.config_file);
    assert!(
        text.contains("// my settings") && text.contains("/* keep */"),
        "{text}"
    );
    let config = crate::jsonedit::value_of(&text).unwrap();
    assert!(config["mcp"]["graphersal-memory"].is_null());
    assert!(config["mcp"]["graphersal-memory-develop"].is_null());
    assert_eq!(
        config["mcp"]["servers"]["graphersal-memory"]["disabled"],
        false
    );
    assert_eq!(config["mcp"]["other"]["command"], json!(["x"]));
    assert_eq!(doctor_problems(&v2), Vec::<String>::new());

    let (code, out) = run_uninstall(&v2, opencode_uninstall());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), before, "{out}");
    memory.close();
}

#[test]
fn both_harnesses_share_one_store_and_the_wizard_asks_which_agents() {
    let home = TempDir::new("both-home");
    let data = TempDir::new("both-data");
    existing_home(home.path());
    existing_opencode_home(home.path());
    let before = snapshot(home.path());
    let setup = setup_in(home.path(), data.path());
    assert_eq!(setup.detected(), vec![Kind::ClaudeCode, Kind::OpenCode]);
    let memory = Memory::open(data.path());
    // Steps: agents, language, model, auto-memory, summaries, develop, summary.
    let (code, out) = run_wizard(&setup, &memory, "\n\n3\n\n2\n2\n\n");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Step 1 of 7 \u{b7} Which agents"), "{out}");
    assert!(
        out.contains("1) All of them       Claude Code and opencode  (default)"),
        "{out}"
    );
    assert!(
        out.contains("graphersal-memory install (Claude Code, opencode)"),
        "{out}"
    );
    assert_eq!(setup.installed(), vec![Kind::ClaudeCode, Kind::OpenCode]);
    // One store: both registrations start the same binary with the same data directory.
    let claude: Value = serde_json::from_str(&read(&setup.paths.user_config)).unwrap();
    let opencode = crate::jsonedit::value_of(&read(&setup.opencode.config_file)).unwrap();
    let claude_args = claude["mcpServers"]["graphersal-memory"]["args"].clone();
    let opencode_command = opencode["mcp"]["graphersal-memory"]["command"].clone();
    assert_eq!(
        opencode_command.as_array().unwrap()[1..].to_vec(),
        claude_args.as_array().unwrap().clone()
    );
    let status = crate::commands::status_text(data.path(), &setup.run_dir, Ok(&setup)).unwrap();
    assert!(
        status.contains("Claude Code (user configuration"),
        "{status}"
    );
    assert!(status.contains("opencode (configuration"), "{status}");
    // Uninstall without --harness removes both.
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("graphersal-memory uninstall (Claude Code, opencode)"),
        "{out}"
    );
    assert_eq!(snapshot(home.path()), before, "{out}");
    memory.close();

    // Only opencode chosen in the wizard: no Claude Code question, nothing written there.
    let data = TempDir::new("both2-data");
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    // Steps: agents (3 = opencode only), language, model, auto-memory (still asked: Claude
    // Code was possible), summaries, develop, summary.
    let (code, out) = run_wizard(&setup, &memory, "3\n\n3\n\n2\n2\n\n");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("This is what install will do"), "{out}");
    assert!(
        !setup.paths.settings.exists()
            || read(&setup.paths.settings) == read_before(&before, ".claude/settings.json")
    );
    assert_eq!(setup.installed(), vec![Kind::OpenCode]);
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), before, "{out}");
    memory.close();
}

fn read_before(before: &[(PathBuf, Vec<u8>)], path: &str) -> String {
    before
        .iter()
        .find(|(p, _)| p == Path::new(path))
        .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default()
}

#[test]
fn opencode_uninstall_exports_every_memory_as_markdown_and_deletes_nothing() {
    let home = TempDir::new("oc-exp-home");
    let data = TempDir::new("oc-exp-data");
    let work = TempDir::new("oc-exp-work");
    let project_dir = mcp::canonical(work.path());
    let project = project_dir.display().to_string();
    let setup = opencode_setup(home.path(), data.path());
    let memory = Memory::open(data.path());
    assert_eq!(run_install(&setup, &memory, opencode_only()).0, 0);
    for (slug, scope) in [("use-pnpm", "project"), ("tabs-not-spaces", "global")] {
        let saved = memory.call(
            &project,
            "remember",
            json!({"slug": slug, "kind": "feedback", "summary": format!("{slug} summary"),
                   "body": format!("{slug} body"), "scope": scope}),
        );
        assert!(saved.to_string().contains(slug), "{saved}");
    }
    let (code, out) = run_uninstall_exporting(
        &setup,
        &memory,
        UninstallOptions {
            yes: true,
            harnesses: Some(vec![Kind::OpenCode]),
            ..UninstallOptions::default()
        },
        b"",
    );
    assert_eq!(code, 0, "{out}");
    let export = home
        .path()
        .join("graphersal-memory-export")
        .join("opencode");
    assert!(out.contains(&export.display().to_string()), "{out}");
    assert!(out.contains("opencode has no file memory"), "{out}");
    let dir = export.join(crate::import::claude_slug(&project));
    assert!(read(&dir.join("use-pnpm.md")).contains("use-pnpm body"));
    assert!(read(&dir.join("MEMORY.md")).contains("(use-pnpm.md)"));
    assert!(read(&export.join("global.md")).contains("tabs-not-spaces summary"));
    // The configuration is gone, the export stays; nothing was deleted from the memory.
    assert!(!setup.opencode.config_file.exists());
    let still = InProcess(&memory).memories_all();
    assert_eq!(still.len(), 2);
    memory.close();
}

#[test]
fn opencode_status_and_doctor_name_its_parts() {
    let home = TempDir::new("oc-doc-home");
    let data = TempDir::new("oc-doc-data");
    let setup = opencode_setup(home.path(), data.path());
    let memory = Memory::open(data.path());
    let before = crate::opencode::install::status_lines(&setup).join("\n");
    assert!(before.contains("Not installed: run `graphersal-memory install --harness opencode`"));
    assert_eq!(run_install(&setup, &memory, opencode_only()).0, 0);
    let status = crate::opencode::install::status_lines(&setup).join("\n");
    for part in [
        "MCP server graphersal-memory (use): installed",
        "graphersal-memory-develop: registered, disabled",
        "rules file in instructions: installed",
        "session plugin",
        "opencode has none",
    ] {
        assert!(status.contains(part), "{part}: {status}");
    }
    let findings = crate::opencode::install::doctor(&setup);
    assert!(findings.iter().all(|f| !f.problem), "{findings:?}");
    // A hand edit of the rule block is a problem doctor names.
    let rules = read(&setup.opencode.rules_file).replace("Save with", "Store with");
    std::fs::write(&setup.opencode.rules_file, rules).unwrap();
    let findings = crate::opencode::install::doctor(&setup);
    assert!(
        findings
            .iter()
            .any(|f| f.problem && f.text.contains("edited by hand")),
        "{findings:?}"
    );
    memory.close();
}

// ------------------------------------------------------------------------------------------
// M7c: Codex.
// ------------------------------------------------------------------------------------------

fn codex_only() -> InstallOptions {
    InstallOptions {
        yes: true,
        harnesses: Some(vec![Kind::Codex]),
        ..InstallOptions::default()
    }
}

fn codex_uninstall() -> UninstallOptions {
    UninstallOptions {
        yes: true,
        no_export: true,
        harnesses: Some(vec![Kind::Codex]),
        ..UninstallOptions::default()
    }
}

/// The user's own Codex configuration: comments, top-level keys, a project table, an MCP server
/// of another program; their own AGENTS.md and a hook of theirs.
const CODEX_CONFIG: &str = "# my Codex settings\nmodel = \"gpt-5.5\"\napproval_policy = \"on-request\"\n\n[mcp_servers.other]\ncommand = \"npx\"\nargs = [\"-y\", \"other\"]  # keep\n\n[projects.\"/w/app\"]\ntrust_level = \"trusted\"\n";

fn existing_codex_home(home: &Path) {
    let dir = home.join(".codex");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), CODEX_CONFIG).unwrap();
    std::fs::write(
        dir.join("AGENTS.md"),
        "# My Codex rules\n\nTabs, not spaces.\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("hooks.json"),
        "{\n  \"hooks\": {\n    \"Stop\": [{\"hooks\": [{\"type\": \"command\", \"command\": \"say done\"}]}]\n  }\n}\n",
    )
    .unwrap();
}

#[test]
fn codex_install_then_uninstall_leaves_existing_files_byte_identical() {
    let home = TempDir::new("cx-rt-home");
    let data = TempDir::new("cx-rt-data");
    existing_codex_home(home.path());
    let before = snapshot(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, codex_only());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.starts_with("graphersal-memory install (Codex)"),
        "{out}"
    );
    assert!(
        !out.contains("Claude Code"),
        "no Claude Code wording: {out}"
    );
    assert!(
        out.contains(
            "ACTION NEEDED: Codex needs your approval for the two graphersal-memory hooks"
        ) && out.contains("type `/hooks`")
            && out.contains("30 minutes idle"),
        "the report says explicitly how to approve the hooks: {out}"
    );
    assert!(
        crate::codex::install::status_lines(&setup)
            .iter()
            .any(|l| l.contains("ACTION NEEDED")),
    );
    let text = read(&setup.codex.config_file);
    assert!(
        text.starts_with("# my Codex settings\nmodel = \"gpt-5.5\""),
        "{text}"
    );
    assert!(
        text.contains("args = [\"-y\", \"other\"]  # keep"),
        "{text}"
    );
    let block_at = text.find(crate::tomledit::BEGIN).unwrap();
    assert!(
        block_at < text.find("[mcp_servers.other]").unwrap(),
        "{text}"
    );
    let doc = crate::tomledit::parse(&text).unwrap();
    assert_eq!(
        doc["mcp_servers"]["graphersal-memory"],
        crate::codex::install::mcp_entry(&setup, "use", true)
    );
    assert_eq!(
        doc["mcp_servers"]["graphersal-memory"]["command"],
        "/opt/gm/bin/graphersal-memory"
    );
    assert_eq!(
        doc["mcp_servers"]["graphersal-memory-develop"]["enabled"],
        false
    );
    assert_eq!(doc["mcp_servers"]["other"]["command"], "npx");
    assert_eq!(doc["model"], "gpt-5.5");
    assert_eq!(doc["projects"]["/w/app"]["trust_level"], "trusted");
    let hooks = crate::jsonedit::value_of(&read(&setup.codex.hooks_file)).unwrap();
    assert_eq!(hooks["hooks"]["Stop"][0]["hooks"][0]["command"], "say done");
    let start = hooks["hooks"]["SessionStart"][0]["hooks"][0].clone();
    assert!(
        start["command"]
            .as_str()
            .unwrap()
            .contains(" hook session-start "),
        "{start}"
    );
    assert!(
        start["command"]
            .as_str()
            .unwrap()
            .ends_with("--harness codex"),
        "{start}"
    );
    assert_eq!(hooks["hooks"]["SessionEnd"][0]["hooks"][0]["timeout"], 3);
    let agents = read(&home.path().join(".codex/AGENTS.md"));
    assert!(
        agents.starts_with("# My Codex rules\n\nTabs, not spaces.\n"),
        "{agents}"
    );
    assert!(
        agents.contains("mcp__graphersal_memory__<function>"),
        "{agents}"
    );
    // Nothing of the other harnesses.
    assert!(!setup.paths.user_config.exists() && !setup.paths.config_dir.exists());
    assert!(!setup.opencode.config_dir.exists());
    assert!(data.path().join("install/codex.json").exists());
    let inspection = crate::codex::install::inspect(&setup);
    assert!(!inspection.nothing_installed());
    assert!(
        !inspection.hooks_trusted,
        "Codex has not trusted the hooks yet"
    );
    assert!(
        crate::codex::install::doctor(&setup)
            .iter()
            .any(|f| f.problem
                && f.text.contains("not approved yet")
                && f.fix.as_deref().is_some_and(|fix| fix.contains("`/hooks`"))),
    );

    // Idempotent.
    let again = snapshot(home.path());
    let (code, out) = run_install(&setup, &memory, codex_only());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), again, "{out}");
    assert!(out.contains("already up to date"), "{out}");

    let (code, out) = run_uninstall(&setup, codex_uninstall());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), before, "{out}");
    assert!(!data.path().join("install/codex.json").exists());
    memory.close();
}

#[test]
fn codex_writes_into_its_config_and_uninstall_still_removes_only_our_parts() {
    let home = TempDir::new("cx-edit-home");
    let data = TempDir::new("cx-edit-data");
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, codex_only());
    assert_eq!(code, 0, "{out}");
    assert!(setup.codex.config_file.is_file() && setup.codex.hooks_file.is_file());
    // Codex records project trust and the trust of our hooks (as it does after `/hooks`).
    let hooks = setup.codex.hooks_file.display().to_string();
    let added = format!(
        "\n[projects.\"/w/app\"]\ntrust_level = \"trusted\"\n\n[hooks.state.'{hooks}:session_start:0:0']\ntrusted_hash = \"sha256:1\"\n\n[hooks.state.'{hooks}:session_end:0:0']\ntrusted_hash = \"sha256:2\"\n"
    );
    let mut text = read(&setup.codex.config_file);
    text.push_str(&added);
    std::fs::write(&setup.codex.config_file, &text).unwrap();
    assert!(crate::codex::install::inspect(&setup).hooks_trusted);
    let (code, out) = run_uninstall(&setup, codex_uninstall());
    assert_eq!(code, 0, "{out}");
    let left = read(&setup.codex.config_file);
    assert_eq!(
        crate::tomledit::parse(&left).unwrap(),
        crate::tomledit::parse(&added).unwrap(),
        "{left}"
    );
    assert!(!left.contains("graphersal-memory:begin"), "{left}");
    assert!(!setup.codex.hooks_file.exists(), "install created it");
    assert!(!setup.codex.instruction_file.exists(), "install created it");
    memory.close();
}

#[test]
fn codex_memories_are_switched_off_on_request_and_back_on_at_uninstall() {
    let home = TempDir::new("cx-mem-home");
    let data = TempDir::new("cx-mem-data");
    let dir = home.path().join(".codex");
    std::fs::create_dir_all(&dir).unwrap();
    let original = "model = \"o3\"\n\n[features]\n# mine\nmemories = true\n";
    std::fs::write(dir.join("config.toml"), original).unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    // The default: switched off, with the sentence that nothing is lost.
    let (code, out) = run_install(&setup, &memory, codex_only());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Nothing is lost"), "{out}");
    let text = read(&setup.codex.config_file);
    assert!(text.contains("# mine\nmemories = false\n"), "{text}");
    assert!(
        crate::codex::install::status_lines(&setup)
            .iter()
            .any(|l| l.contains("switched off by install")),
    );
    let (code, out) = run_uninstall(&setup, codex_uninstall());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Codex's memories: switched on again"), "{out}");
    assert_eq!(read(&setup.codex.config_file), original);
    // Kept on when asked to.
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            keep_auto_memory: true,
            ..codex_only()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("[features] memories: on (kept)"), "{out}");
    assert!(read(&setup.codex.config_file).contains("# mine\nmemories = true\n"));
    memory.close();
}

#[test]
fn codex_develop_server_is_switched_by_enabled_and_conflicts_are_kept() {
    let home = TempDir::new("cx-dev-home");
    let data = TempDir::new("cx-dev-data");
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            enable_develop: true,
            ..codex_only()
        },
    );
    assert_eq!(code, 0, "{out}");
    let doc = crate::tomledit::parse(&read(&setup.codex.config_file)).unwrap();
    assert!(doc["mcp_servers"]["graphersal-memory-develop"]["enabled"].is_null());
    let (code, out) = run_uninstall(
        &setup,
        UninstallOptions {
            disable_develop_only: true,
            ..codex_uninstall()
        },
    );
    assert_eq!(code, 0, "{out}");
    let text = read(&setup.codex.config_file);
    let doc = crate::tomledit::parse(&text).unwrap();
    assert_eq!(
        doc["mcp_servers"]["graphersal-memory-develop"]["enabled"],
        false
    );
    assert!(crate::tomledit::find(&text).unwrap().unwrap().intact);
    assert!(
        setup.codex.hooks_file.exists(),
        "only the develop server changed"
    );
    memory.close();

    // Another program's server under our name: kept, reported, nothing written there.
    let home = TempDir::new("cx-conflict-home");
    let data = TempDir::new("cx-conflict-data");
    let dir = home.path().join(".codex");
    std::fs::create_dir_all(&dir).unwrap();
    let theirs = "[mcp_servers.graphersal-memory]\ncommand = \"/somewhere/else\"\n";
    std::fs::write(dir.join("config.toml"), theirs).unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, codex_only());
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("is not ours"), "{out}");
    assert_eq!(read(&setup.codex.config_file), theirs);
    memory.close();
}

#[test]
fn codex_rules_go_into_the_override_file_when_that_is_the_one_codex_loads() {
    let home = TempDir::new("cx-ovr-home");
    let data = TempDir::new("cx-ovr-data");
    let dir = home.path().join(".codex");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("AGENTS.md"), "# shadowed\n").unwrap();
    std::fs::write(dir.join("AGENTS.override.md"), "# active\n").unwrap();
    let before = snapshot(home.path());
    let setup = setup_in(home.path(), data.path());
    assert_eq!(setup.codex.instruction_file, dir.join("AGENTS.override.md"));
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, codex_only());
    assert_eq!(code, 0, "{out}");
    assert!(read(&dir.join("AGENTS.override.md")).contains("graphersal-memory:begin"));
    assert_eq!(read(&dir.join("AGENTS.md")), "# shadowed\n");
    let (code, out) = run_uninstall(&setup, codex_uninstall());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), before, "{out}");
    memory.close();
}

#[test]
fn codex_is_detected_by_its_home_and_exported_as_markdown() {
    let home = TempDir::new("cx-det-home");
    let data = TempDir::new("cx-det-data");
    let setup = setup_in(home.path(), data.path());
    assert!(!setup.detected().contains(&Kind::Codex));
    std::fs::create_dir_all(home.path().join(".codex")).unwrap();
    assert!(setup.detected().contains(&Kind::Codex));
    let agent = setup.agent_of(Kind::Codex);
    assert!(!crate::agents::Agent::has_file_memory(agent.as_ref()));
    assert_eq!(
        crate::agents::Agent::global_memory_file(agent.as_ref()),
        home.path().join("graphersal-memory-export/codex/global.md")
    );
    assert!(crate::commands::export_note(agent.as_ref()).contains("AGENTS.md"));
}

#[test]
fn codex_tool_approvals_the_user_gave_are_kept_and_cleaned_up_with_our_servers() {
    for inside in [false, true] {
        let home = TempDir::new("cx-appr-home");
        let data = TempDir::new("cx-appr-data");
        let dir = home.path().join(".codex");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "model = \"o3\"\n").unwrap();
        let setup = setup_in(home.path(), data.path());
        let memory = Memory::open(data.path());
        let (code, out) = run_install(&setup, &memory, codex_only());
        assert_eq!(code, 0, "{out}");
        // "Always allow" in Codex: a tools table of our server, outside or inside the block.
        let approval =
            "\n[mcp_servers.graphersal-memory.tools.remember]\napproval_mode = \"approve\"\n";
        let text = read(&setup.codex.config_file);
        let edited = if inside {
            text.replacen(
                "\n[mcp_servers.graphersal-memory-develop]",
                &format!("{approval}\n[mcp_servers.graphersal-memory-develop]"),
                1,
            )
        } else {
            format!("{text}{approval}")
        };
        assert_ne!(edited, text);
        std::fs::write(&setup.codex.config_file, &edited).unwrap();
        let inspection = crate::codex::install::inspect(&setup);
        assert_eq!(inspection.use_server, crate::install::PartState::Installed);
        let (code, out) = run_install(&setup, &memory, codex_only());
        assert_eq!(code, 0, "inside: {inside}: {out}");
        assert_eq!(
            read(&setup.codex.config_file),
            edited,
            "the approval is kept"
        );
        let (code, out) = run_uninstall(&setup, codex_uninstall());
        assert_eq!(code, 0, "inside: {inside}: {out}");
        let left = read(&setup.codex.config_file);
        assert_eq!(
            crate::tomledit::parse(&left).unwrap(),
            json!({"model": "o3"}),
            "inside: {inside}: {left}"
        );
        memory.close();
    }
}

// ------------------------------------------------------------------------------------------
// M7d: Cursor.
// ------------------------------------------------------------------------------------------

fn cursor_only() -> InstallOptions {
    InstallOptions {
        yes: true,
        harnesses: Some(vec![Kind::Cursor]),
        ..InstallOptions::default()
    }
}

fn cursor_uninstall() -> UninstallOptions {
    UninstallOptions {
        yes: true,
        no_export: true,
        harnesses: Some(vec![Kind::Cursor]),
        ..UninstallOptions::default()
    }
}

/// The user's own `~/.cursor`: an `mcp.json` with a comment and a server of another program, a
/// `hooks.json` with a hook of theirs, a user rule file of theirs (the documented formats,
/// cursor.com/docs/mcp, /docs/hooks, /docs/rules).
const CURSOR_MCP: &str = "{\n  // my servers\n  \"mcpServers\": {\n    \"other\": {\n      \"command\": \"npx\",\n      \"args\": [\"-y\", \"other\"]\n    }\n  }\n}\n";
const CURSOR_HOOKS: &str = "{\n  \"version\": 1,\n  \"hooks\": {\n    \"afterFileEdit\": [{ \"command\": \"./hooks/format.sh\" }]\n  }\n}\n";
const CURSOR_RULE: &str = "---\nalwaysApply: true\n---\n\nReply in a concise style.\n";

fn existing_cursor_home(home: &Path) {
    let dir = home.join(".cursor");
    std::fs::create_dir_all(dir.join("rules")).unwrap();
    std::fs::write(dir.join("mcp.json"), CURSOR_MCP).unwrap();
    std::fs::write(dir.join("hooks.json"), CURSOR_HOOKS).unwrap();
    std::fs::write(dir.join("rules/concise.mdc"), CURSOR_RULE).unwrap();
}

#[test]
fn cursor_install_then_uninstall_leaves_existing_files_byte_identical() {
    let home = TempDir::new("cu-rt-home");
    let data = TempDir::new("cu-rt-data");
    existing_cursor_home(home.path());
    let before = snapshot(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, cursor_only());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.starts_with("graphersal-memory install (Cursor)"),
        "{out}"
    );
    assert!(
        !out.contains("Claude Code"),
        "no Claude Code wording: {out}"
    );
    assert!(
        out.contains("Cursor asks before each MCP tool call"),
        "{out}"
    );
    assert!(out.contains("Restart running Cursor sessions"), "{out}");
    // mcp.json: our server beside theirs, the comment kept, no develop server.
    let text = read(&setup.cursor.mcp_file);
    assert!(text.starts_with("{\n  // my servers\n"), "{text}");
    let mcp = crate::jsonedit::value_of(&text).unwrap();
    assert_eq!(mcp["mcpServers"]["other"]["command"], "npx");
    assert_eq!(
        mcp["mcpServers"]["graphersal-memory"],
        json!({"type": "stdio", "command": "/opt/gm/bin/graphersal-memory",
               "args": ["mcp", "--profile", "use", "--data-dir", data.path().display().to_string()]})
    );
    assert!(mcp["mcpServers"]["graphersal-memory-develop"].is_null());
    // hooks.json: theirs kept, ours flat entries with a timeout.
    let hooks = crate::jsonedit::value_of(&read(&setup.cursor.hooks_file)).unwrap();
    assert_eq!(hooks["version"], 1);
    assert_eq!(
        hooks["hooks"]["afterFileEdit"][0]["command"],
        "./hooks/format.sh"
    );
    let start = hooks["hooks"]["sessionStart"][0].clone();
    let command = start["command"].as_str().unwrap();
    assert!(command.contains(" hook session-start "), "{start}");
    assert!(command.ends_with("--harness cursor"), "{start}");
    assert_eq!(start["timeout"], 30);
    assert!(
        hooks["hooks"]["sessionEnd"][0]["command"]
            .as_str()
            .unwrap()
            .contains(" hook session-end ")
    );
    // Our own rule file; theirs untouched.
    let rules = read(&setup.cursor.rules_file);
    assert!(rules.starts_with("---\ndescription: "), "{rules}");
    assert!(rules.contains("alwaysApply: true"), "{rules}");
    assert!(
        rules.contains(crate::cursor::install::RULE_BLOCK),
        "{rules}"
    );
    assert_eq!(
        read(&home.path().join(".cursor/rules/concise.mdc")),
        CURSOR_RULE
    );
    // Nothing of the other harnesses.
    assert!(!setup.paths.user_config.exists() && !setup.paths.config_dir.exists());
    assert!(!setup.codex.codex_home.exists() && !setup.opencode.config_dir.exists());
    assert!(data.path().join("install/cursor.json").exists());
    let inspection = crate::cursor::install::inspect(&setup);
    assert_eq!(inspection.use_server, crate::install::PartState::Installed);
    assert_eq!(inspection.rules, crate::install::PartState::Installed);
    assert!(
        inspection
            .hooks
            .iter()
            .all(|(_, s)| *s == crate::install::PartState::Installed)
    );
    assert_eq!(setup.installed(), vec![Kind::Cursor]);

    // Idempotent.
    let again = snapshot(home.path());
    let (code, out) = run_install(&setup, &memory, cursor_only());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), again, "{out}");
    assert!(out.contains("already up to date"), "{out}");

    let (code, out) = run_uninstall(&setup, cursor_uninstall());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), before, "{out}");
    assert!(!data.path().join("install/cursor.json").exists());
    memory.close();
}

#[test]
fn cursor_files_install_created_go_and_the_users_later_additions_stay() {
    let home = TempDir::new("cu-new-home");
    let data = TempDir::new("cu-new-data");
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, cursor_only());
    assert_eq!(code, 0, "{out}");
    assert!(read(&setup.cursor.hooks_file).starts_with("{\n  \"version\": 1,"));
    // Created files only: uninstall leaves no trace (directories included).
    let (code, out) = run_uninstall(&setup, cursor_uninstall());
    assert_eq!(code, 0, "{out}");
    assert!(!home.path().join(".cursor").exists(), "{out}");

    // The user adds a server and a hook of theirs after install: those stay.
    let (code, out) = run_install(&setup, &memory, cursor_only());
    assert_eq!(code, 0, "{out}");
    let mcp = read(&setup.cursor.mcp_file);
    let (mcp, _) = crate::jsonedit::set_member(
        &mcp,
        &["mcpServers"],
        "theirs",
        &json!({"command": "/bin/theirs"}),
    )
    .unwrap()
    .unwrap();
    std::fs::write(&setup.cursor.mcp_file, &mcp).unwrap();
    let hooks = read(&setup.cursor.hooks_file);
    let (hooks, _) = crate::jsonedit::append_element(
        &hooks,
        &["hooks", "stop"],
        &json!({"command": "./hooks/done.sh"}),
    )
    .unwrap();
    std::fs::write(&setup.cursor.hooks_file, &hooks).unwrap();
    let (code, out) = run_uninstall(&setup, cursor_uninstall());
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        crate::jsonedit::value_of(&read(&setup.cursor.mcp_file)).unwrap(),
        json!({"mcpServers": {"theirs": {"command": "/bin/theirs"}}})
    );
    assert_eq!(
        crate::jsonedit::value_of(&read(&setup.cursor.hooks_file)).unwrap(),
        json!({"version": 1, "hooks": {"stop": [{"command": "./hooks/done.sh"}]}})
    );
    assert!(!setup.cursor.rules_file.exists(), "install created it");
    memory.close();
}

#[test]
fn cursor_develop_server_is_registered_only_on_request_and_conflicts_are_kept() {
    let home = TempDir::new("cu-dev-home");
    let data = TempDir::new("cu-dev-data");
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            enable_develop: true,
            ..cursor_only()
        },
    );
    assert_eq!(code, 0, "{out}");
    let mcp = crate::jsonedit::value_of(&read(&setup.cursor.mcp_file)).unwrap();
    assert_eq!(
        mcp["mcpServers"]["graphersal-memory-develop"],
        crate::cursor::install::mcp_entry(&setup, "develop")
    );
    let (code, out) = run_uninstall(
        &setup,
        UninstallOptions {
            disable_develop_only: true,
            ..cursor_uninstall()
        },
    );
    assert_eq!(code, 0, "{out}");
    let mcp = crate::jsonedit::value_of(&read(&setup.cursor.mcp_file)).unwrap();
    assert!(mcp["mcpServers"]["graphersal-memory-develop"].is_null());
    assert!(!mcp["mcpServers"]["graphersal-memory"].is_null());
    assert!(
        setup.cursor.hooks_file.exists(),
        "only the develop server changed"
    );
    // A full uninstall afterwards still leaves nothing.
    let (code, out) = run_uninstall(&setup, cursor_uninstall());
    assert_eq!(code, 0, "{out}");
    assert!(!home.path().join(".cursor").exists(), "{out}");
    memory.close();

    // Another program's server and a rule file of the user under our names: kept, reported.
    let home = TempDir::new("cu-conflict-home");
    let data = TempDir::new("cu-conflict-data");
    let dir = home.path().join(".cursor");
    std::fs::create_dir_all(dir.join("rules")).unwrap();
    let theirs = "{\"mcpServers\": {\"graphersal-memory\": {\"command\": \"/somewhere/else\"}}}\n";
    std::fs::write(dir.join("mcp.json"), theirs).unwrap();
    std::fs::write(dir.join("rules/graphersal-memory.mdc"), CURSOR_RULE).unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, cursor_only());
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("is not ours"), "{out}");
    assert!(out.contains("is not graphersal-memory's"), "{out}");
    assert_eq!(read(&setup.cursor.mcp_file), theirs);
    assert_eq!(read(&setup.cursor.rules_file), CURSOR_RULE);
    let inspection = crate::cursor::install::inspect(&setup);
    assert!(matches!(
        inspection.use_server,
        crate::install::PartState::Conflict(_)
    ));
    assert!(matches!(
        inspection.rules,
        crate::install::PartState::Conflict(_)
    ));
    memory.close();
}

#[test]
fn cursor_rule_file_follows_the_language_and_a_hand_edit_is_kept() {
    let home = TempDir::new("cu-rule-home");
    let data = TempDir::new("cu-rule-data");
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    assert_eq!(run_install(&setup, &memory, cursor_only()).0, 0);
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            language: Some(crate::language::Language::Any),
            ..cursor_only()
        },
    );
    assert_eq!(code, 0, "{out}");
    let rules = read(&setup.cursor.rules_file);
    assert!(
        rules.contains(crate::cursor::install::RULE_BLOCK_ANY),
        "{rules}"
    );
    assert!(
        rules.starts_with(crate::cursor::install::FRONT_MATTER),
        "{rules}"
    );
    // The user writes below the block: uninstall takes only the block out.
    let mine = format!("{rules}\nAlso: answer briefly.\n");
    std::fs::write(&setup.cursor.rules_file, &mine).unwrap();
    let (code, out) = run_uninstall(&setup, cursor_uninstall());
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        read(&setup.cursor.rules_file),
        format!(
            "{}\nAlso: answer briefly.\n",
            crate::cursor::install::FRONT_MATTER
        )
    );
    // Edited inside the block: kept, reported.
    let home = TempDir::new("cu-rule2-home");
    let data2 = TempDir::new("cu-rule2-data");
    let setup = setup_in(home.path(), data2.path());
    assert_eq!(run_install(&setup, &memory, cursor_only()).0, 0);
    let edited = read(&setup.cursor.rules_file).replace("Store and search", "Keep");
    std::fs::write(&setup.cursor.rules_file, &edited).unwrap();
    let (code, out) = run_uninstall(&setup, cursor_uninstall());
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("edited by hand"), "{out}");
    assert_eq!(read(&setup.cursor.rules_file), edited);
    memory.close();
}

#[test]
fn cursor_is_detected_by_its_home_status_and_doctor_name_its_parts() {
    let home = TempDir::new("cu-det-home");
    let data = TempDir::new("cu-det-data");
    let setup = setup_in(home.path(), data.path());
    assert!(!setup.detected().contains(&Kind::Cursor));
    std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
    assert!(setup.detected().contains(&Kind::Cursor));
    let lines = crate::cursor::install::status_lines(&setup).join("\n");
    assert!(lines.contains("Not installed: run `graphersal-memory install --harness cursor`"));
    let memory = Memory::open(data.path());
    assert_eq!(run_install(&setup, &memory, cursor_only()).0, 0);
    let lines = crate::cursor::install::status_lines(&setup).join("\n");
    for part in [
        "MCP server graphersal-memory (use",
        "hook sessionStart",
        "hook sessionEnd",
        "rule file",
        "headless command (summaries, bench): none",
        "0 transcript(s) reported by the hooks",
    ] {
        assert!(lines.contains(part), "{part}\n{lines}");
    }
    let findings = crate::cursor::install::doctor(&setup);
    assert!(!findings.iter().any(|f| f.problem), "{findings:?}");
    assert!(
        findings
            .iter()
            .any(|f| f.text.contains("no transcript reported by the hooks yet")),
        "{findings:?}"
    );
    // A transcript reported by a hook is counted.
    let transcript = crate::cursor::sessions::tests::write_fixture(
        &home.path().join(".cursor/projects/w"),
        "c1",
    );
    crate::cursor::sessions::record(&setup.cursor.registry, &transcript, "/w", "c1").unwrap();
    assert!(
        crate::cursor::install::status_lines(&setup)
            .join("\n")
            .contains("1 transcript(s) reported by the hooks (1 on disk)")
    );
    // A broken file is a problem doctor names.
    std::fs::write(&setup.cursor.hooks_file, "{ broken").unwrap();
    let findings = crate::cursor::install::doctor(&setup);
    assert!(
        findings
            .iter()
            .any(|f| f.problem && f.text.contains("is not valid JSON")),
        "{findings:?}"
    );
    memory.close();
}

#[test]
fn cursor_uninstall_exports_every_memory_as_markdown() {
    let home = TempDir::new("cu-exp-home");
    let data = TempDir::new("cu-exp-data");
    let work = TempDir::new("cu-exp-work");
    let project_dir = mcp::canonical(work.path());
    let project = project_dir.display().to_string();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    assert_eq!(run_install(&setup, &memory, cursor_only()).0, 0);
    for (slug, scope) in [("use-pnpm", "project"), ("tabs-not-spaces", "global")] {
        let saved = memory.call(
            &project,
            "remember",
            json!({"slug": slug, "kind": "feedback", "summary": format!("{slug} summary"),
                   "body": format!("{slug} body"), "scope": scope}),
        );
        assert!(saved.to_string().contains(slug), "{saved}");
    }
    let (code, out) = run_uninstall_exporting(
        &setup,
        &memory,
        UninstallOptions {
            yes: true,
            harnesses: Some(vec![Kind::Cursor]),
            ..UninstallOptions::default()
        },
        b"",
    );
    assert_eq!(code, 0, "{out}");
    let export = home.path().join("graphersal-memory-export").join("cursor");
    assert!(out.contains(&export.display().to_string()), "{out}");
    assert!(out.contains("Cursor has no file memory"), "{out}");
    assert!(out.contains("~/.cursor/rules"), "{out}");
    let dir = export.join(crate::import::claude_slug(&project));
    assert!(read(&dir.join("use-pnpm.md")).contains("use-pnpm body"));
    assert!(read(&dir.join("MEMORY.md")).contains("(use-pnpm.md)"));
    assert!(read(&export.join("global.md")).contains("tabs-not-spaces summary"));
    assert!(!home.path().join(".cursor").exists(), "{out}");
    assert_eq!(InProcess(&memory).memories_all().len(), 2);
    memory.close();
}

#[test]
fn the_wizard_lists_cursor_among_the_detected_agents() {
    let home = TempDir::new("cu-wiz-home");
    let data = TempDir::new("cu-wiz-data");
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
    let setup = setup_in(home.path(), data.path());
    assert_eq!(setup.detected(), vec![Kind::ClaudeCode, Kind::Cursor]);
    let memory = Memory::open(data.path());
    // Which agents: 3 (Cursor only); language: Enter; model: 3 (built-in); auto-memory (asked
    // while Claude Code may be chosen): skipped for Cursor only; summaries: 2 (no); develop:
    // 2 (no); summary: Enter applies.
    let (code, out) = run_wizard(&setup, &memory, "3\n\n3\n2\n2\n\n");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Step 1 of 7 \u{b7} Which agents"), "{out}");
    assert!(out.contains("Claude Code and Cursor"), "{out}");
    // The auto-memory step counted for Claude Code is left out of the count for Cursor only.
    assert!(
        out.contains("Step 5 of 6 \u{b7} The develop server"),
        "{out}"
    );
    assert!(out.contains("Step 6 of 6 \u{b7} Summary"), "{out}");
    assert!(out.contains("Cursor only"), "{out}");
    assert!(
        out.contains("Cursor's own CLI is not used for summaries"),
        "{out}"
    );
    assert_eq!(setup.installed(), vec![Kind::Cursor]);
    assert!(!setup.paths.user_config.exists(), "Claude Code untouched");
    memory.close();
}

// ------------------------------------------------------------------------------------------
// M11: hook commands every Windows shell runs; the shim does the hooks' work.
// ------------------------------------------------------------------------------------------

/// `setup` as on Windows, with `plain` as the words every shell runs (or none).
fn windows_setup(home: &Path, data: &Path, plain: Option<Vec<String>>) -> Setup {
    let mut setup = setup_in(home, data);
    setup.exe = PathBuf::from(r"C:\Users\Ann Lee\gm\graphersal-memory.exe");
    setup.hooks = install::HookForm {
        windows: true,
        plain,
    };
    setup
}

fn plain_words(data: &Path) -> Option<Vec<String>> {
    Some(vec![
        "graphersal-memory".into(),
        "--data-dir".into(),
        data.display().to_string(),
    ])
}

#[test]
fn on_windows_claude_code_gets_the_exec_form_and_the_others_the_plain_words() {
    let home = TempDir::new("w-forms-home");
    let data = TempDir::new("w-forms-data");
    existing_home(home.path());
    std::fs::create_dir_all(home.path().join(".codex")).unwrap();
    std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
    let setup = windows_setup(home.path(), data.path(), plain_words(data.path()));
    let memory = Memory::open(data.path());
    let all = InstallOptions {
        yes: true,
        harnesses: Some(vec![Kind::ClaudeCode, Kind::Codex, Kind::Cursor]),
        ..InstallOptions::default()
    };
    let (code, out) = run_install(&setup, &memory, all);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("(run without a shell)"), "{out}");
    let settings = settings_json(&setup);
    let ours = &settings["hooks"]["SessionStart"][1]["hooks"][0];
    assert_eq!(
        *ours,
        json!({"type": "command", "command": r"C:\Users\Ann Lee\gm\graphersal-memory.exe",
               "args": ["hook", "session-start", "--data-dir", data.path().display().to_string()],
               "timeout": 30})
    );
    assert_eq!(
        settings["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        "echo mine"
    );
    let words = format!("--data-dir {}", data.path().display());
    let codex = crate::jsonedit::value_of(&read(&setup.codex.hooks_file)).unwrap();
    assert_eq!(
        codex["hooks"]["SessionEnd"][0]["hooks"][0]["command"],
        format!("graphersal-memory hook session-end {words} --harness codex")
    );
    let cursor = crate::jsonedit::value_of(&read(&setup.cursor.hooks_file)).unwrap();
    assert_eq!(
        cursor["hooks"]["sessionStart"][0]["command"],
        format!("graphersal-memory hook session-start {words} --harness cursor")
    );
    // Recognised as installed (status/doctor), and a second install changes nothing.
    assert!(
        install::doctor(&setup)
            .iter()
            .any(|f| f.text == "hook SessionStart: installed")
    );
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("SessionStart: already installed"), "{out}");
    memory.close();
}

#[test]
fn on_windows_without_plain_words_codex_keeps_quotes_and_cursor_gets_powershell() {
    let home = TempDir::new("w-quoted-home");
    let data = TempDir::new("w-quoted-data");
    let setup = windows_setup(home.path(), data.path(), None);
    assert_eq!(
        crate::codex::install::hook_command(&setup, "session-start"),
        format!(
            r#""C:\Users\Ann Lee\gm\graphersal-memory.exe" hook session-start --data-dir "{}" --harness codex"#,
            data.path().display()
        )
    );
    if cfg!(windows) {
        assert!(
            crate::cursor::install::hook_command(&setup, "session-start").starts_with("& '"),
            "PowerShell's form"
        );
    }
    // Claude Code's exec form needs no plain words.
    assert_eq!(
        setup.hook_group("session-start")["hooks"][0]["args"][1],
        "session-start"
    );
}

#[test]
fn a_reinstall_replaces_the_quoted_form_and_uninstall_still_gives_the_files_back() {
    let home = TempDir::new("w-re-home");
    let data = TempDir::new("w-re-data");
    existing_home(home.path());
    let before = snapshot(home.path());
    let memory = Memory::open(data.path());
    // An earlier install wrote the quoted shell command.
    let old = setup_in(home.path(), data.path());
    assert_eq!(run_install(&old, &memory, yes()).0, 0);
    let quoted = settings_json(&old)["hooks"]["SessionStart"][1]["hooks"][0]["command"].clone();
    assert!(quoted.as_str().unwrap().starts_with('"'), "{quoted}");
    // This version on Windows: the hooks are updated to the exec form, nothing else changes.
    let setup = windows_setup(home.path(), data.path(), None);
    assert!(
        install::doctor(&setup)
            .iter()
            .any(|f| f.problem && f.text.starts_with("hook SessionStart: "))
    );
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("SessionStart: update to"), "{out}");
    let settings = settings_json(&setup);
    for (event, sub) in [
        ("SessionStart", "session-start"),
        ("SessionEnd", "session-end"),
    ] {
        let ours: Vec<&Value> = settings["hooks"][event]
            .as_array()
            .unwrap()
            .iter()
            .filter(|g| install::is_our_hook_group(g))
            .collect();
        assert_eq!(ours, vec![&setup.hook_group(sub)], "{settings}");
    }
    assert_eq!(
        settings["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        "echo mine"
    );
    assert_eq!(settings["model"], "opus");
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), before, "{out}");
    memory.close();
}

#[test]
fn uninstall_removes_every_form_of_our_hooks() {
    let home = TempDir::new("w-un-home");
    let data = TempDir::new("w-un-data");
    existing_home(home.path());
    let setup = windows_setup(home.path(), data.path(), plain_words(data.path()));
    let memory = Memory::open(data.path());
    assert_eq!(run_install(&setup, &memory, yes()).0, 0);
    // Leftovers of other forms beside the exec form install wrote, and a user's change (so
    // uninstall removes our parts from the current text instead of restoring the backup).
    let settings = read(&setup.paths.settings);
    let mut value: Value = serde_json::from_str(&settings).unwrap();
    let start = value["hooks"]["SessionStart"].as_array_mut().unwrap();
    start.push(json!({"hooks": [{"type": "command",
        "command": r#""C:\Users\Ann Lee\gm\graphersal-memory.exe" hook session-start"#}]}));
    start.push(json!({"hooks": [{"type": "command",
        "command": "graphersal-memory hook session-start --data-dir D:/gm"}]}));
    start.push(json!({"hooks": [{"type": "command",
        "command": "C:/PROGRA~1/GRAPHE~1/graphersal-memory.exe hook session-start"}]}));
    value["theme"] = json!("light");
    std::fs::write(
        &setup.paths.settings,
        serde_json::to_string_pretty(&value).unwrap(),
    )
    .unwrap();

    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    let settings = settings_json(&setup);
    assert_eq!(settings["theme"], "light");
    assert_eq!(
        settings["hooks"]["SessionStart"],
        json!([{"hooks": [{"type": "command", "command": "echo mine"}]}]),
        "{settings}"
    );
    assert!(settings["hooks"].get("SessionEnd").is_none(), "{settings}");
    memory.close();
}

#[test]
fn hooks_of_every_form_are_recognised_and_others_are_not() {
    let ours = [
        json!({"command": "\"/x/graphersal-memory\" hook session-start"}),
        json!({"command": "graphersal-memory hook session-end --harness codex"}),
        json!({"command": "C:/gm/graphersal-memory.exe hook session-start"}),
        json!({"command": "& 'C:\\gm\\graphersal-memory.exe' hook session-start --harness cursor"}),
        json!({"command": "C:\\gm dir\\graphersal-memory.exe", "args": ["hook", "session-end"]}),
    ];
    for hook in &ours {
        assert!(install::is_our_hook(hook), "{hook}");
    }
    let theirs = [
        json!({"command": "echo graphersal-memory hook"}),
        json!({"command": "C:\\gm\\graphersal-memory.exe", "args": ["mcp"]}),
        json!({"command": "C:\\gm\\other.exe", "args": ["hook", "session-start"]}),
        json!({"command": "node", "args": ["graphersal-memory", "hook", "session-start"]}),
    ];
    for hook in &theirs {
        assert!(!install::is_our_hook(hook), "{hook}");
    }
}

#[test]
fn doctor_says_what_works_without_git_bash() {
    use crate::winshell::GitBash;
    let quoted = json!({"hooks": {"SessionStart": [{"hooks": [{"type": "command",
        "command": "\"C:\\gm\\graphersal-memory.exe\" hook session-start"}]}]}});
    let exec = json!({"hooks": {"SessionStart": [{"hooks": [{"type": "command",
        "command": "C:\\gm\\graphersal-memory.exe", "args": ["hook", "session-start"]}]}]}});
    let missing = GitBash {
        path: None,
        ignored: None,
    };
    let found = GitBash {
        path: Some(r"C:\Program Files\Git\bin\bash.exe".into()),
        ignored: None,
    };
    // Missing, and our hooks are the quoted form: a problem with what works and the fixes.
    let findings = install::git_bash_findings(&missing, &quoted);
    assert_eq!(findings.len(), 1);
    let finding = &findings[0];
    assert!(finding.problem);
    assert!(
        finding.text.contains("Git Bash not found"),
        "{}",
        finding.text
    );
    assert!(finding.text.contains("PowerShell"), "{}", finding.text);
    assert!(
        finding.text.contains("What works: the memory tools"),
        "{}",
        finding.text
    );
    let fix = finding.fix.as_deref().unwrap();
    assert!(fix.contains("graphersal-memory install"), "{fix}");
    assert!(fix.contains("Git for Windows"), "{fix}");
    assert!(fix.contains("CLAUDE_CODE_GIT_BASH_PATH"), "{fix}");
    assert!(install::git_bash_status(&missing, &quoted).contains("the installed hooks need it"));
    // Missing, but the hooks need no shell: fine.
    let findings = install::git_bash_findings(&missing, &exec);
    assert!(!findings[0].problem);
    assert!(
        findings[0].text.contains("need no shell"),
        "{}",
        findings[0].text
    );
    assert!(install::git_bash_status(&missing, &exec).contains("run without a shell"));
    // Found: the path.
    let findings = install::git_bash_findings(&found, &quoted);
    assert_eq!(
        findings[0].text,
        r"Git Bash: C:\Program Files\Git\bin\bash.exe (Claude Code runs shell hooks there)"
    );
    // An ignored variable is a note of its own.
    let ignored = GitBash {
        path: None,
        ignored: Some(
            "CLAUDE_CODE_GIT_BASH_PATH=D:\\x\\git-bash.exe is not named bash.exe or sh.exe".into(),
        ),
    };
    let findings = install::git_bash_findings(&ignored, &exec);
    assert_eq!(findings.len(), 2);
    assert!(findings[0].text.contains("git-bash.exe") && findings[0].fix.is_some());
    // The doctor of a non-Windows setup has no Git Bash line.
    let home = TempDir::new("w-doc-home");
    let data = TempDir::new("w-doc-data");
    let setup = setup_in(home.path(), data.path());
    assert!(
        !install::doctor(&setup)
            .iter()
            .any(|f| f.text.contains("Git Bash"))
    );
}

/// A session start through the shim and through the hook, in either order: the memory files are
/// imported once, the same memories stay.
#[test]
fn the_shim_and_the_hook_import_the_same_files_once() {
    let home = TempDir::new("w-both-home");
    let data = TempDir::new("w-both-data");
    let work = TempDir::new("w-both-work");
    let project_dir = mcp::canonical(work.path());
    std::fs::create_dir_all(project_dir.join(".git")).unwrap();
    memory_fixture(home.path(), &project_dir);
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let project = project_dir.display().to_string();
    let plan = crate::session_sync::plan(&setup, "claude-code", &project_dir, &project_dir);
    assert!(plan.claude.is_some());
    let mut queued = Vec::new();
    let mut connect = memory.connector();
    let deadline = Instant::now() + Duration::from_secs(30);
    let done = {
        let mut enqueue = |request: crate::sessions::Request| {
            queued.push(request);
            Ok(())
        };
        crate::session_sync::start(&plan, &mut connect, &mut enqueue, deadline)
    };
    assert!(done.notes.is_empty(), "{done:?}");
    assert_eq!(done.imported, 3, "{done:?}");
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].harness, Kind::ClaudeCode);
    let listed = memory.call(&project, "memories", json!({"k": 50}));
    let count = listed.as_array().map(Vec::len).unwrap_or_default();
    assert!(count >= 3, "{listed}");
    // The hook then: nothing new, the same memories.
    let mut stdout = Vec::new();
    let code = hook::run(
        hook::HookEvent::SessionStart,
        Ok(setup.paths.clone()),
        data.path().to_path_buf(),
        Box::new(memory.connector()),
        std::io::Cursor::new(json!({"cwd": project}).to_string().into_bytes()),
        &mut stdout,
        &mut Vec::new(),
    );
    assert_eq!(code, 0);
    let line = String::from_utf8(stdout).unwrap();
    assert!(!line.contains("Imported"), "{line}");
    // And the shim once more (the next session): nothing either.
    let done = crate::session_sync::start(&plan, &mut connect, &mut |_| Ok(()), deadline);
    assert_eq!(done.imported, 0, "{done:?}");
    let again = memory.call(&project, "memories", json!({"k": 50}));
    assert_eq!(again, listed);
    // A changed file is imported by whichever runs first, once.
    let file = home
        .path()
        .join(".claude/projects")
        .join(import::claude_slug(&project))
        .join("memory/release-plan.md");
    std::fs::write(
        &file,
        "---\nname: release-plan\ndescription: Releases go out on Mondays\ntype: project\n---\n\nMonday releases.\n",
    )
    .unwrap();
    let done = crate::session_sync::start(&plan, &mut connect, &mut |_| Ok(()), deadline);
    assert_eq!(done.imported, 1, "{done:?}");
    let mut stdout = Vec::new();
    hook::run(
        hook::HookEvent::SessionStart,
        Ok(setup.paths.clone()),
        data.path().to_path_buf(),
        Box::new(memory.connector()),
        std::io::Cursor::new(json!({"cwd": project}).to_string().into_bytes()),
        &mut stdout,
        &mut Vec::new(),
    );
    assert!(!String::from_utf8(stdout).unwrap().contains("Imported"));
    drop(connect);
    memory.close();
}
