//! `install`, `uninstall`, `import`, `status`, `doctor` and the hooks on temporary home and data
//! directories (never the user's), with the memory served in the test process (a
//! `StoreRuntime` and in-memory pipes instead of the daemon).

use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

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
        data_dir: data.to_path_buf(),
        run_dir: short_run_dir(),
        exe: PathBuf::from("/opt/gm/bin/graphersal-memory"),
        dir_args: vec!["--data-dir".into(), data.display().to_string()],
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
    UninstallOptions {
        yes: true,
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
            ..UninstallOptions::default()
        },
        &mut &b""[..],
        &mut out,
    )
    .unwrap_err();
    assert!(err.contains("does not look like"), "{err}");
    assert!(data.path().join("precious.txt").exists());
}
