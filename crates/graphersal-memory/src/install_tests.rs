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
use crate::cursor::fixture::CURSOR_RULE;
use crate::import::{self, Caller, ImportState};
use crate::install::{self, ClaudePaths, InstallOptions, Setup, UninstallOptions};
use crate::language::Language;
use crate::mcp::{self, StoreRuntime};
use crate::mcp_tests::TempDir;
use crate::{hook, registry, store};

/// A setup for the home `home` and the data directory `data`, with a binary path that does not
/// need to exist.
/// `text` with every home path in it (`~/...`, up to a space, `)`, `,`, `;`, `:`, a quote or a
/// backtick) written with the platform's separator, as the product shows it: `~\.claude.json`
/// on Windows, `~/.claude.json` elsewhere.
pub(crate) fn native(text: &str) -> String {
    if std::path::MAIN_SEPARATOR == '/' {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut in_path = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '~' && chars.peek() == Some(&'/') {
            in_path = true;
        } else if in_path {
            if c.is_whitespace() || matches!(c, ')' | ',' | ';' | ':' | '"' | '`') {
                in_path = false;
            } else if c == '/' {
                out.push(std::path::MAIN_SEPARATOR);
                continue;
            }
        }
        out.push(c);
    }
    out
}

pub(crate) fn setup_in(home: &Path, data: &Path) -> Setup {
    // A machine with Claude Code's program (install refuses when no agent program is found).
    with_programs(bare_setup_in(home, data), &["claude"])
}

/// [`setup_in`] without any agent program on its search path.
pub(crate) fn bare_setup_in(home: &Path, data: &Path) -> Setup {
    let paths =
        ClaudePaths::resolve(&|name| (name == "HOME").then(|| home.display().to_string())).unwrap();
    Setup {
        paths,
        opencode: crate::opencode::OpenCodePaths::of_home(home),
        codex: crate::codex::CodexPaths::of_home(home),
        cursor: crate::cursor::CursorPaths::of_home(home, data),
        codewhale: crate::codewhale::CodeWhalePaths::of_home(home),
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

/// `setup` with fake programs `names` on its search path (empty files: found, never run), as
/// on a machine where those harnesses are installed.
pub(crate) fn with_programs(mut setup: Setup, names: &[&str]) -> Setup {
    let bin = setup.run_dir.with_extension("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for name in names {
        std::fs::write(bin.join(name), "").unwrap();
    }
    setup.search_path = Some(bin.into_os_string());
    setup
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
pub(crate) struct Memory {
    pub(crate) runtime: Arc<StoreRuntime>,
    servers: Servers,
}

impl Memory {
    pub(crate) fn open(data: &Path) -> Self {
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
    pub(crate) fn connector(
        &self,
    ) -> impl FnMut(&str) -> Result<Box<dyn Caller>, String> + Send + 'static {
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
    pub(crate) fn call(&self, project: &str, name: &str, arguments: Value) -> Value {
        let mut connect = self.connector();
        let mut caller = connect(project).unwrap();
        caller.call(name, arguments).unwrap()
    }

    pub(crate) fn close(self) {
        let servers: Vec<_> = self.servers.lock().unwrap().drain(..).collect();
        for server in servers {
            assert_eq!(server.join().unwrap(), Ok(()));
        }
        self.runtime.close().unwrap();
    }
}

pub(crate) fn run_install(setup: &Setup, memory: &Memory, options: InstallOptions) -> (u8, String) {
    let mut connect = memory.connector();
    let mut out = Vec::new();
    let code = install::install(setup, &options, &mut connect, &mut &b""[..], &mut out).unwrap();
    (code, String::from_utf8(out).unwrap())
}

pub(crate) fn yes() -> InstallOptions {
    InstallOptions {
        yes: true,
        ..InstallOptions::default()
    }
}

pub(crate) fn run_uninstall(setup: &Setup, options: UninstallOptions) -> (u8, String) {
    let mut out = Vec::new();
    let code = install::uninstall(setup, &options, &mut &b""[..], &mut out).unwrap();
    (code, String::from_utf8(out).unwrap())
}

pub(crate) fn uninstall_yes() -> UninstallOptions {
    // These tests check what uninstall gives back of the harness's files; the export of the
    // memories into the file memory has its own tests (`export_*`).
    UninstallOptions {
        yes: true,
        no_export: true,
        ..UninstallOptions::default()
    }
}

/// The bytes of every file under `dir` (relative path -> content).
pub(crate) fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
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

pub(crate) fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
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
            "'/opt/gm/bin/graphersal-memory' hook session-start --data-dir '{}'",
            data.path().display()
        )
    );
    let rules = read(&setup.paths.instructions);
    assert!(
        rules.starts_with("# My rules\n\nBe brief.\n\n<!-- graphersal-memory:begin"),
        "{rules}"
    );
    assert!(out.contains("(backed up)"), "{out}");
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
pub(crate) fn memory_fixture(home: &Path, project: &Path) -> PathBuf {
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
    let outcome = crate::claude_code::hook::work(
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
    let outcome = crate::claude_code::hook::work(
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
    let outcome = crate::claude_code::hook::work(
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
    let code = install::uninstall(
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
    .unwrap();
    assert_eq!(code, 0);
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

/// A current rule block text as 0.1.0 and 0.2.0 wrote it: without the line about what to save.
fn without_what_to_save(text: &str) -> String {
    let start = text
        .find("- Save what will help")
        .expect("the line about what to save");
    let end = start
        + text[start..]
            .find("findings.\n")
            .expect("the end of the line about what to save")
        + "findings.\n".len();
    format!("{}{}", &text[..start], &text[end..])
}

#[test]
fn the_blocks_before_the_line_about_what_to_save_are_known_with_or_without_read_only_tools() {
    type Find = fn(&str, Language, &[String]) -> Result<Option<install::FoundBlock>, String>;
    type Rule = fn(Language) -> &'static str;
    let harnesses: [(&str, Rule, Find); 5] = [
        ("claude-code", install::rule_block, install::find_block_with),
        (
            "opencode",
            crate::opencode::install::rule_block,
            crate::opencode::install::find_block_with,
        ),
        (
            "codex",
            crate::codex::install::rule_block,
            crate::codex::install::find_block_with,
        ),
        (
            "cursor",
            crate::cursor::install::rule_block,
            crate::cursor::install::find_block_with,
        ),
        (
            "codewhale",
            crate::codewhale::install::rule_block,
            crate::codewhale::install::find_block_with,
        ),
    ];
    let tools = ["memory-keeper".to_string()];
    for (name, rule, find) in harnesses {
        for language in [Language::English, Language::Any] {
            let old = without_what_to_save(rule(language));
            for read_only in [&[][..], &tools[..]] {
                let block = install::wrap_block(&crate::coexist::rule_with(&old, read_only));
                let found = find(&format!("# Mine\n\n{block}"), language, read_only)
                    .unwrap()
                    .unwrap();
                assert!(
                    found.intact && found.known && !found.current,
                    "{name} {language:?} read-only {read_only:?}: {found:?}"
                );
            }
        }
    }
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

pub(crate) fn refresh() -> InstallOptions {
    InstallOptions {
        refresh: true,
        ..yes()
    }
}

#[test]
fn refresh_brings_an_earlier_block_to_this_version_and_keeps_every_choice() {
    let home = TempDir::new("refresh-home");
    let data = TempDir::new("refresh-data");
    existing_home(home.path());
    // opencode is found on this machine too, but install goes into Claude Code only.
    let setup = with_programs(
        bare_setup_in(home.path(), data.path()),
        &["claude", "opencode"],
    );
    let memory = Memory::open(data.path());
    let only_claude = InstallOptions {
        harnesses: Some(vec![Kind::ClaudeCode]),
        ..yes()
    };
    let (code, out) = run_install(&setup, &memory, only_claude);
    assert_eq!(code, 0, "{out}");
    // The user switched Claude Code's auto-memory back on after install.
    let settings = read(&setup.paths.settings);
    let on = settings.replace(
        "\"autoMemoryEnabled\": false",
        "\"autoMemoryEnabled\": true",
    );
    assert_ne!(on, settings, "{settings}");
    std::fs::write(&setup.paths.settings, on).unwrap();
    let installed = snapshot(home.path());
    // The rule block as an earlier version left it.
    let previous = crate::fsutil::fingerprint(BLOCK_0_0_0.as_bytes());
    let old = with_block(&setup, &block_with(BLOCK_0_0_0, &previous));
    std::fs::write(&setup.paths.instructions, &old).unwrap();

    let (code, out) = run_install(&setup, &memory, refresh());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("update the block to this version"), "{out}");
    assert!(!out.contains("opencode"), "{out}");
    // The block is this version's again; the auto-memory stays on, the develop server
    // unregistered, opencode untouched.
    assert_eq!(snapshot(home.path()), installed, "{out}");

    // Nothing left to do: nothing written.
    let (code, out) = run_install(&setup, &memory, refresh());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("up to date"), "{out}");
    assert_eq!(snapshot(home.path()), installed, "{out}");
    memory.close();
}

#[test]
fn refresh_dry_run_and_without_an_install_change_nothing_and_uninstall_stays_exact() {
    let home = TempDir::new("refresh-dry-home");
    let data = TempDir::new("refresh-dry-data");
    existing_home(home.path());
    let before = snapshot(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    // Not installed: nothing to refresh, nothing written (not even the data directory).
    let (code, out) = run_install(&setup, &memory, refresh());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("not installed in any harness"), "{out}");
    assert_eq!(snapshot(home.path()), before);
    assert!(!data.path().join("install").exists());

    run_install(&setup, &memory, yes());
    let previous = crate::fsutil::fingerprint(BLOCK_0_0_0.as_bytes());
    let old = with_block(&setup, &block_with(BLOCK_0_0_0, &previous));
    std::fs::write(&setup.paths.instructions, &old).unwrap();
    let outdated = snapshot(home.path());
    let dry = InstallOptions {
        dry_run: true,
        ..refresh()
    };
    let (code, out) = run_install(&setup, &memory, dry);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Dry run"), "{out}");
    assert_eq!(snapshot(home.path()), outdated, "{out}");

    let (code, out) = run_install(&setup, &memory, refresh());
    assert_eq!(code, 0, "{out}");
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), before, "{out}");
    memory.close();
}

#[test]
fn refresh_keeps_a_hand_edited_block_and_says_so() {
    let home = TempDir::new("refresh-edit-home");
    let data = TempDir::new("refresh-edit-data");
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    run_install(&setup, &memory, yes());
    let text = read(&setup.paths.instructions).replace("Save with", "Please save with");
    std::fs::write(&setup.paths.instructions, &text).unwrap();
    let (code, out) = run_install(&setup, &memory, refresh());
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("conflict:"), "{out}");
    assert_eq!(read(&setup.paths.instructions), text);
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

pub(crate) fn run_wizard(setup: &Setup, memory: &Memory, input: &str) -> (u8, String) {
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
        "Step 7 of 7 \u{b7} Ready to install",
    ] {
        assert!(out.contains(step), "{step}\n{out}");
    }
    assert!(out.contains("potion-retrieval-32M"), "{out}");
    assert!(out.contains("English, ~130 MB, MIT"), "{out}");
    assert!(out.contains("100+ languages, ~530 MB, MIT"), "{out}");
    assert!(out.contains("not meaning; weaker search"), "{out}");
    assert!(out.contains("Please type a number from 1 to 2"), "{out}");
    // Asked in step 4 (wrapped there) and repeated in the recap of the last step.
    let recap = out.split("Ready to install").nth(1).unwrap_or_default();
    assert!(recap.contains(install::NOTHING_IS_LOST), "{out}");
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
    let status = [
        install::status_lines(&setup),
        install::shared_status_lines(&setup),
    ]
    .concat()
    .join("\n");
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
    let exported_dir = crate::agents::Harness::memory_dir(&elsewhere.agent(), &project_dir);
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
    let setup = with_programs(setup_in(home.path(), data.path()), &["claude", "opencode"]);
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
        out.contains("graphersal-memory install (Claude Code and opencode)"),
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
        status.contains("Claude Code:\n  user configuration:"),
        "{status}"
    );
    assert!(status.contains("opencode:\n  configuration:"), "{status}");
    // Uninstall without --harness removes both.
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("graphersal-memory uninstall (Claude Code and opencode)"),
        "{out}"
    );
    assert_eq!(snapshot(home.path()), before, "{out}");
    memory.close();

    // Only opencode chosen in the wizard: no Claude Code question, nothing written there.
    let data = TempDir::new("both2-data");
    let setup = with_programs(setup_in(home.path(), data.path()), &["claude", "opencode"]);
    let memory = Memory::open(data.path());
    // Steps: agents (3 = opencode only), language, model, auto-memory (still asked: Claude
    // Code was possible), summaries, develop, summary.
    let (code, out) = run_wizard(&setup, &memory, "3\n\n3\n\n2\n2\n\n");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Ready to install"), "{out}");
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
    assert!(
        out.contains(&crate::install::tilde(
            &export.display().to_string(),
            home.path()
        )),
        "{out}"
    );
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
            .any(|f| !f.problem
                && f.text.starts_with("ACTION NEEDED")
                && f.text.contains("`/hooks`")),
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
fn codex_is_detected_by_its_program_leftovers_are_not_offered_and_it_is_exported_as_markdown() {
    let home = TempDir::new("cx-det-home");
    let data = TempDir::new("cx-det-data");
    let setup = setup_in(home.path(), data.path());
    assert!(!setup.detected().contains(&Kind::Codex));
    std::fs::create_dir_all(home.path().join(".codex")).unwrap();
    // Its home alone is a leftover, not offered; its program makes it found.
    assert!(!setup.detected().contains(&Kind::Codex));
    assert!(
        setup
            .leftovers()
            .iter()
            .any(|(kind, _)| *kind == Kind::Codex)
    );
    let setup = with_programs(setup, &["codex"]);
    assert!(setup.detected().contains(&Kind::Codex));
    let agent = setup.agent_of(Kind::Codex);
    assert!(!crate::agents::Harness::has_file_memory(agent.as_ref()));
    assert_eq!(
        crate::agents::Harness::global_memory_file(agent.as_ref()),
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
            crate::cursor::install::KEPT_FRONT_MATTER
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
fn cursor_is_detected_by_its_program_leftovers_are_not_offered_and_status_and_doctor_name_its_parts()
 {
    let home = TempDir::new("cu-det-home");
    let data = TempDir::new("cu-det-data");
    let setup = setup_in(home.path(), data.path());
    assert!(!setup.detected().contains(&Kind::Cursor));
    std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
    // Its home alone is a leftover, not offered; its program makes it found.
    assert!(!setup.detected().contains(&Kind::Cursor));
    assert!(
        setup
            .leftovers()
            .iter()
            .any(|(kind, _)| *kind == Kind::Cursor)
    );
    let setup = with_programs(setup, &["cursor"]);
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
    assert!(
        out.contains(&crate::install::tilde(
            &export.display().to_string(),
            home.path()
        )),
        "{out}"
    );
    assert!(out.contains("Cursor has no file memory"), "{out}");
    assert!(out.contains(&native("~/.cursor/rules")), "{out}");
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
    let setup = with_programs(setup_in(home.path(), data.path()), &["claude", "cursor"]);
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
    assert!(out.contains("Step 6 of 6 \u{b7} Ready to install"), "{out}");
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
    assert!(quoted.as_str().unwrap().starts_with('\''), "{quoted}");
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
    let code = crate::claude_code::hook::run(
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
    crate::claude_code::hook::run(
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

// ---- M13: detection by program, the plan in sections, the recap, the report, status ----

/// A harness whose configuration is here but whose program is not is reported, never offered
/// or preselected (here: Codex's home left over, Claude Code installed).
#[test]
fn a_harness_with_only_leftover_configuration_is_not_offered() {
    let home = TempDir::new("m13-left-home");
    let data = TempDir::new("m13-left-data");
    existing_home(home.path());
    existing_codex_home(home.path());
    let setup = with_programs(setup_in(home.path(), data.path()), &["claude"]);
    assert_eq!(setup.detected(), vec![Kind::ClaudeCode]);
    assert_eq!(
        setup.leftovers(),
        vec![(Kind::Codex, home.path().join(".codex"))]
    );
    let memory = Memory::open(data.path());
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            dry_run: true,
            ..InstallOptions::default()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.starts_with("graphersal-memory install (Claude Code)\n"),
        "{out}"
    );
    assert!(
        out.contains(&native(
            "note: Codex: configuration found (~/.codex), but the program `codex` is not \
             installed: not offered (`--harness codex` installs anyway)"
        )),
        "{out}"
    );
    assert!(!out.contains("\nCodex\n"), "{out}");
    // The wizard does not ask which agents: one is found.
    let (code, out) = run_wizard(&setup, &memory, "\n\n3\n\n\n2\n2\nn\n");
    assert_eq!(code, 1, "{out}");
    assert!(!out.contains("Which agents"), "{out}");
    assert!(out.contains("not offered"), "{out}");
    memory.close();
}

/// A program outside `PATH` in its known install location counts: opencode's
/// `~/.opencode/bin/opencode`, Claude Code's `~/.local/bin/claude` and `~/.claude/local/claude`.
#[test]
fn a_program_in_its_known_location_outside_path_is_offered() {
    let home = TempDir::new("m13-known-home");
    let data = TempDir::new("m13-known-data");
    let setup = bare_setup_in(home.path(), data.path());
    assert_eq!(setup.detected(), Vec::<Kind>::new());
    std::fs::create_dir_all(home.path().join(".opencode/bin")).unwrap();
    std::fs::write(home.path().join(".opencode/bin/opencode"), "").unwrap();
    assert_eq!(setup.detected(), vec![Kind::OpenCode]);
    std::fs::create_dir_all(home.path().join(".local/bin")).unwrap();
    std::fs::write(home.path().join(".local/bin/claude"), "").unwrap();
    assert_eq!(setup.detected(), vec![Kind::ClaudeCode, Kind::OpenCode]);
    std::fs::remove_file(home.path().join(".local/bin/claude")).unwrap();
    std::fs::create_dir_all(home.path().join(".claude/local")).unwrap();
    std::fs::write(home.path().join(".claude/local/claude"), "").unwrap();
    assert_eq!(
        setup.program(Kind::ClaudeCode),
        Some(home.path().join(".claude/local/claude"))
    );
    std::fs::create_dir_all(home.path().join("Applications/Cursor.app")).unwrap();
    assert!(setup.detected().contains(&Kind::Cursor));
}

/// `--harness codex` installs although Codex's program is missing, after a warning; uninstall
/// then removes it again (the program is not needed to clean up), and status shows it.
#[test]
fn harness_override_warns_and_uninstall_works_without_the_program() {
    let home = TempDir::new("m13-over-home");
    let data = TempDir::new("m13-over-data");
    existing_codex_home(home.path());
    let before = snapshot(home.path());
    let setup = setup_in(home.path(), data.path());
    assert!(setup.program(Kind::Codex).is_none());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, codex_only());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains(
            "warning: Codex's program `codex` is not found (not on PATH, not in its usual \
             places); installing anyway because --harness names it"
        ),
        "{out}"
    );
    assert!(!out.contains("note: Codex: configuration found"), "{out}");
    assert_eq!(setup.installed(), vec![Kind::Codex]);
    let status = crate::commands::status_text(data.path(), &setup.run_dir, Ok(&setup)).unwrap();
    assert!(
        status.contains(&native("\nCodex:\n  configuration: ~/.codex/config.toml")),
        "{status}"
    );
    // Uninstall without --harness: the installed harness, whether its program exists or not.
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.starts_with("graphersal-memory uninstall (Codex)\n"),
        "{out}"
    );
    assert_eq!(snapshot(home.path()), before, "{out}");
    memory.close();
}

/// The plan of a dry run: one parenthesis per line, names joined with "and", the CLAUDE.md
/// sentence, the steps in sections (shared, one per agent, then import and indexing).
#[test]
fn the_plan_is_grouped_by_agent_with_plain_wording() {
    let home = TempDir::new("m13-plan-home");
    let data = TempDir::new("m13-plan-data");
    existing_home(home.path());
    existing_opencode_home(home.path());
    existing_codex_home(home.path());
    let setup = with_programs(
        setup_in(home.path(), data.path()),
        &["claude", "opencode", "codex"],
    );
    let memory = Memory::open(data.path());
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            dry_run: true,
            ..InstallOptions::default()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.starts_with("graphersal-memory install (Claude Code, opencode and Codex)\n"),
        "{out}"
    );
    assert!(
        out.contains("- Claude Code, opencode and Codex (one store"),
        "{out}"
    );
    assert!(!out.contains(" and opencode and "), "{out}");
    assert!(
        out.contains(
            "- English only (stored and searched in English); written into the MCP \
             instructions, the rule block and memory.language in config.json\n"
        ),
        "{out}"
    );
    assert!(
        out.contains("CLAUDE.md keeps working as before (Claude Code still loads it)"),
        "{out}"
    );
    let headings: Vec<&str> = out
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with(' '))
        .collect();
    assert_eq!(
        headings[1..],
        [
            "Dry run: this is what install would do (nothing is changed).",
            "Shared by every agent",
            "Claude Code",
            "opencode",
            "Codex",
            "Then, after the files are written",
        ],
        "{out}"
    );
    // Each section is set off by a blank line, the numbers run on across them.
    for heading in &headings[2..] {
        assert!(
            out.contains(&format!("\n\n{heading}\n")),
            "{heading}\n{out}"
        );
    }
    assert!(
        out.contains("\n\nClaude Code\n  5. Register the MCP servers"),
        "{out}"
    );
    assert!(
        out.contains("Import Claude Code's existing file memory"),
        "{out}"
    );
    let then = out
        .split("Then, after the files are written")
        .nth(1)
        .unwrap();
    assert!(
        then.contains("Import Claude Code's existing file memory"),
        "{out}"
    );
    assert!(then.contains("Index past sessions"), "{out}");
    // The home directory is `~`.
    assert!(out.contains(&native("~/.claude.json")), "{out}");
    assert!(!out.contains(&home.path().display().to_string()), "{out}");
    memory.close();
}

/// The wizard's last step is a short recap with what must not be skipped; `d` shows every
/// change, Enter installs.
#[test]
fn the_wizard_ends_with_a_short_recap_and_d_shows_every_change() {
    let home = TempDir::new("m13-recap-home");
    let data = TempDir::new("m13-recap-data");
    existing_home(home.path());
    existing_codex_home(home.path());
    let setup = with_programs(setup_in(home.path(), data.path()), &["claude", "codex"]);
    let memory = Memory::open(data.path());
    // Agents: Enter (all); language: Enter; model: 3 (built-in); auto-memory: Enter (off);
    // summaries: 2; develop: 2; then d (every change), then Enter (install).
    let (code, out) = run_wizard(&setup, &memory, "\n\n3\n\n2\n2\nd\n\n");
    assert_eq!(code, 0, "{out}");
    let recap = out.split("Ready to install").nth(1).unwrap();
    let recap = recap
        .split("Install? [Y/n, d = show every change]")
        .next()
        .unwrap();
    for line in [
        "  Agents: Claude Code and Codex\n",
        "  Memory language: English only (stored and searched in English)\n",
        "  Embedding model: the built-in hash-ngram/1 (no download)\n",
        "  File memory: nothing new to import\n",
        "  Summaries: off\n",
    ] {
        assert!(recap.contains(line), "{line}\n{recap}");
    }
    assert!(recap.contains(install::NOTHING_IS_LOST), "{recap}");
    assert!(
        recap.contains("ACTION NEEDED: Codex needs your approval"),
        "{recap}"
    );
    // The recap is short: no plan step in it.
    assert!(!recap.contains("Register the MCP servers"), "{recap}");
    let details = out.split("Every change install makes:").nth(1).unwrap();
    assert!(details.contains("Register the MCP servers"), "{out}");
    assert_eq!(
        out.matches("Install? [Y/n, d = show every change]").count(),
        2
    );
    assert_eq!(setup.installed(), vec![Kind::ClaudeCode, Kind::Codex]);
    memory.close();
}

/// The report after install: backups named once, a section per agent, then "Next steps" after
/// a separator, numbered, ACTION NEEDED among them; no colour without a terminal.
#[test]
fn the_install_report_ends_with_numbered_next_steps() {
    let home = TempDir::new("m13-report-home");
    let data = TempDir::new("m13-report-data");
    existing_home(home.path());
    existing_codex_home(home.path());
    let setup = with_programs(setup_in(home.path(), data.path()), &["claude", "codex"]);
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    let report = out.split("\nReport:\n").nth(1).unwrap();
    assert_eq!(
        report.matches("Backups of the files changed:").count(),
        1,
        "{report}"
    );
    // The backup directory once: its path is not repeated per file.
    assert_eq!(report.matches("backups").count(), 1, "{report}");
    assert!(
        report.contains(&native("changed ~/.claude.json (backed up)")),
        "{report}"
    );
    assert!(report.contains("\n\nClaude Code:\n"), "{report}");
    assert!(report.contains("\n\nCodex:\n"), "{report}");
    let next = report.split(crate::prompt::SEPARATOR).nth(1).unwrap();
    assert!(next.starts_with("\nNext steps\n"), "{next}");
    assert!(
        next.contains(
            "  1. Restart running Claude Code and Codex sessions to load the MCP server and \
             hooks.\n"
        ),
        "{next}"
    );
    assert!(
        next.contains("  2. ACTION NEEDED: Codex needs your approval"),
        "{next}"
    );
    assert!(!out.contains('\x1b'), "no colour without a terminal");
    memory.close();
}

/// `status`: a headline, every section after a blank line, Memory before the agents, `~` for
/// the home directory, ACTION NEEDED repeated as the last line.
#[test]
fn status_has_sections_memory_first_and_repeats_the_action_needed_last() {
    let home = TempDir::new("m13-status-home");
    let data = TempDir::new("m13-status-data");
    existing_home(home.path());
    existing_codex_home(home.path());
    let setup = with_programs(setup_in(home.path(), data.path()), &["claude", "codex"]);
    let memory = Memory::open(data.path());
    assert_eq!(run_install(&setup, &memory, yes()).0, 0);
    let status = crate::commands::status_text(data.path(), &setup.run_dir, Ok(&setup)).unwrap();
    assert!(
        status.starts_with("graphersal-memory status\n  daemon: "),
        "{status}"
    );
    assert!(
        status.contains("\n  settings: config.json (daemon idle time"),
        "{status}"
    );
    let memory_at = status.find("\n\nMemory:\n").unwrap();
    let claude_at = status.find("\n\nClaude Code:\n").unwrap();
    let codex_at = status.find("\n\nCodex:\n").unwrap();
    assert!(memory_at < claude_at && claude_at < codex_at, "{status}");
    assert!(
        status.contains(&native("  user configuration: ~/.claude.json\n")),
        "{status}"
    );
    assert!(
        !status.contains(&home.path().display().to_string()),
        "{status}"
    );
    let last = status.lines().last().unwrap();
    assert!(
        last.starts_with("ACTION NEEDED: Codex needs your approval"),
        "{status}"
    );
    assert_eq!(status.matches("ACTION NEEDED").count(), 2, "{status}");
    let coloured =
        crate::commands::status_text_with(data.path(), &setup.run_dir, Ok(&setup), true).unwrap();
    assert!(coloured.contains("\x1b[1;33mACTION NEEDED"), "{coloured}");
    memory.close();
}

/// Uninstall of every harness names only those it changes; without a terminal it says why it
/// does not ask; the prompt is `[Y/n]` like install's.
#[test]
fn uninstall_names_only_what_it_changes_and_explains_a_missing_terminal() {
    let home = TempDir::new("m13-un-home");
    let data = TempDir::new("m13-un-data");
    existing_home(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    assert_eq!(run_install(&setup, &memory, yes()).0, 0);
    // Without an answer (no terminal): the plan, then why nothing happened.
    let (code, out) = run_uninstall(
        &setup,
        UninstallOptions {
            no_export: true,
            ..UninstallOptions::default()
        },
    );
    assert_eq!(code, 1, "{out}");
    assert!(
        out.starts_with("graphersal-memory uninstall (Claude Code)\n"),
        "{out}"
    );
    assert!(out.contains("Uninstall? [Y/n]"), "{out}");
    assert!(
        out.contains(
            "No answer, nothing was changed: there is no terminal to ask in; run it in a \
             terminal, or add --yes."
        ),
        "{out}"
    );
    // Every harness named: only the installed one is in the title and changed.
    let (code, out) = run_uninstall(
        &setup,
        UninstallOptions {
            yes: true,
            no_export: true,
            harnesses: Some(Kind::ALL.to_vec()),
            ..UninstallOptions::default()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.starts_with("graphersal-memory uninstall (Claude Code)\n"),
        "{out}"
    );
    assert!(
        out.contains(&native("\n\nClaude Code:\n  - ~/.claude.json")),
        "{out}"
    );
    memory.close();
}

#[test]
fn names_are_joined_with_and_and_the_home_is_a_tilde() {
    assert_eq!(install::join_and(&[]), "");
    assert_eq!(install::join_and(&["a"]), "a");
    assert_eq!(install::join_and(&["a", "b"]), "a and b");
    assert_eq!(install::join_and(&["a", "b", "c"]), "a, b and c");
    let home = Path::new("/Users/me");
    assert_eq!(
        install::tilde("created /Users/me/.claude.json (in /Users/me)", home),
        "created ~/.claude.json (in ~)"
    );
    assert_eq!(
        install::tilde("/Users/meg/x and /Users/me", home),
        "/Users/meg/x and ~"
    );
}

/// Without `--harness` and without any agent program, install refuses (also as a dry run):
/// nothing changes, the message names what it looked for, the leftovers and the way out.
#[test]
fn install_refuses_when_no_agent_program_is_found() {
    let home = TempDir::new("m13-none-home");
    let data = TempDir::new("m13-none-data");
    existing_home(home.path());
    existing_codex_home(home.path());
    let before = snapshot(home.path());
    let setup = bare_setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    for options in [
        yes(),
        InstallOptions {
            dry_run: true,
            ..InstallOptions::default()
        },
    ] {
        let (code, out) = run_install(&setup, &memory, options);
        assert_eq!(code, 1, "{out}");
        assert!(
            out.starts_with("graphersal-memory install: no agent program found\n"),
            "{out}"
        );
        assert!(
            out.contains("looked for: claude, opencode, codex and cursor on PATH"),
            "{out}"
        );
        assert!(
            out.contains(&native(
                "configuration found without its program: Claude Code (~/.claude)"
            )),
            "{out}"
        );
        assert!(
            out.contains(&native(
                "configuration found without its program: Codex (~/.codex)"
            )),
            "{out}"
        );
        assert!(
            out.contains("--harness claude-code|opencode|codex|cursor"),
            "{out}"
        );
        assert_eq!(snapshot(home.path()), before, "{out}");
        assert!(!crate::config::path(data.path()).exists(), "{out}");
    }
    // `--harness` installs anyway, after the warning.
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            harnesses: Some(vec![Kind::ClaudeCode]),
            ..yes()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("warning: Claude Code's program `claude` is not found"),
        "{out}"
    );
    assert_eq!(setup.installed(), vec![Kind::ClaudeCode]);
    memory.close();
}

// ---- M14: the start context: pinned import, auto-memory default, hook delivery ----

/// The fixture's `MEMORY.md` with a `## Pinned` section holding the release plan.
const PINNED_INDEX: &str = "# Index\n\n## How to work\n- [Graph first](graph-first.md) — study the graph first\n\n\
    ## Pinned\n- [Release plan](release-plan.md) — Fridays only\n";

#[test]
fn a_pinned_section_is_imported_at_install_and_the_start_context_has_it_at_once() {
    let home = TempDir::new("pin-home");
    let data = TempDir::new("pin-data");
    let work = TempDir::new("pin-work");
    let project_dir = mcp::canonical(work.path());
    let memory_dir = memory_fixture(home.path(), &project_dir);
    std::fs::write(memory_dir.join("MEMORY.md"), PINNED_INDEX).unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let project = project_dir.display().to_string();

    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    // Claude Code's auto-memory is off: the start context carries the rules instead.
    assert_eq!(settings_json(&setup)["autoMemoryEnabled"], false);
    assert!(
        out.contains("graphersal-memory's start context: 2 memories"),
        "{out}"
    );
    let release = memory.call(&project, "memory", json!({"slug": "release-plan"}));
    assert_eq!(release["title"], "Release plan");
    assert_eq!(release["hook"], "Fridays only");
    // Right after install, without a session in between.
    let context = memory
        .runtime
        .start_context(&project, crate::start_context::DEFAULT_CHARS)
        .unwrap();
    assert_eq!(context.total, 2, "{}", context.text);
    assert!(
        context
            .text
            .contains("## Pinned\n- Release plan — Fridays only (memory: release-plan)"),
        "{}",
        context.text
    );
    assert!(
        context.text.contains("(memory: graph-first)"),
        "{}",
        context.text
    );

    // Re-import: an unchanged directory sends nothing.
    let mut connect = memory.connector();
    let mut out = Vec::new();
    crate::commands::run_import(&setup, false, &mut connect, &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("0 new, 0 changed, 3 unchanged"), "{out}");

    // A pin set in the graph survives a changed file that MEMORY.md does not pin.
    memory.call(&project, "pin", json!({"slug": "odd-name"}));
    std::fs::write(
        memory_dir.join("Odd_Name.md"),
        "---\ndescription: still no type\n---\nSomething else.\n",
    )
    .unwrap();
    // The release plan moves out of the Pinned section: it is unpinned.
    std::fs::write(
        memory_dir.join("MEMORY.md"),
        "# Index\n\n## How to work\n- [Graph first](graph-first.md) — study the graph first\n\
         - [Release plan](release-plan.md) — Fridays only\n",
    )
    .unwrap();
    let mut out = Vec::new();
    crate::commands::run_import(&setup, false, &mut connect, &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("(0 new, 2 updated)"), "{out}");
    let context = memory
        .runtime
        .start_context(&project, crate::start_context::DEFAULT_CHARS)
        .unwrap();
    assert!(
        !context.text.contains("(memory: release-plan)"),
        "{}",
        context.text
    );
    assert!(
        context.text.contains("(memory: odd-name)"),
        "{}",
        context.text
    );
    // And again: nothing.
    let mut out = Vec::new();
    crate::commands::run_import(&setup, false, &mut connect, &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("0 new, 0 changed, 3 unchanged"), "{out}");
    drop(connect);
    memory.close();
}

#[test]
fn the_auto_memory_question_says_the_start_context_carries_the_rules() {
    let home = TempDir::new("am-home");
    let data = TempDir::new("am-data");
    let work = TempDir::new("am-work");
    let project_dir = mcp::canonical(work.path());
    let memory_dir = memory_fixture(home.path(), &project_dir);
    std::fs::write(memory_dir.join("MEMORY.md"), PINNED_INDEX).unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            dry_run: true,
            ..InstallOptions::default()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("autoMemoryEnabled: false"), "{out}");
    assert!(
        out.contains(
            "Your rules and pinned memories still reach every session through \
             graphersal-memory's start context: 2 memories,"
        ),
        "{out}"
    );
    memory.close();
}

#[test]
fn without_a_start_context_the_auto_memory_stays_on_by_default_with_the_reason() {
    let home = TempDir::new("am0-home");
    let data = TempDir::new("am0-data");
    let setup = setup_in(home.path(), data.path());
    std::fs::write(
        data.path().join("config.json"),
        "{\"version\": 1, \"memory\": {\"start_context_chars\": 0}}\n",
    )
    .unwrap();
    let memory = Memory::open(data.path());
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            dry_run: true,
            ..InstallOptions::default()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("kept as it is (auto-memory on)"), "{out}");
    assert!(
        out.contains("The default is to keep it on: memory.start_context_chars is 0"),
        "{out}"
    );
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    assert_ne!(settings_json(&setup)["autoMemoryEnabled"], false);
    memory.close();
}

#[test]
fn the_session_start_hook_puts_the_freshly_imported_memories_into_additional_context() {
    let home = TempDir::new("hctx-home");
    let data = TempDir::new("hctx-data");
    let work = TempDir::new("hctx-work");
    let project_dir = mcp::canonical(work.path());
    let memory_dir = memory_fixture(home.path(), &project_dir);
    std::fs::write(memory_dir.join("MEMORY.md"), PINNED_INDEX).unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let runtime = memory.runtime.clone();
    let fetch: hook::StartFetch =
        Box::new(move |dir, max| runtime.start_context(&mcp::project_key(None, dir.unwrap()), max));
    let mut stdout = Vec::new();
    let code = crate::claude_code::hook::run_with_context(
        hook::HookEvent::SessionStart,
        Ok(setup.paths.clone()),
        data.path().to_path_buf(),
        Box::new(memory.connector()),
        None,
        Some(fetch),
        std::io::Cursor::new(
            json!({"cwd": project_dir.display().to_string()})
                .to_string()
                .into_bytes(),
        ),
        &mut stdout,
        &mut Vec::new(),
    );
    assert_eq!(code, 0);
    let answer: Value = serde_json::from_slice(&stdout).unwrap();
    let context = answer["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.contains("Imported 3 new or changed memory files"),
        "{context}"
    );
    assert!(context.contains("(memory: release-plan)"), "{context}");
    assert!(context.contains("(memory: graph-first)"), "{context}");
    memory.close();
}

#[test]
fn the_start_hook_is_found_in_claude_codes_settings() {
    let home = TempDir::new("sh-home");
    let data = TempDir::new("sh-data");
    let setup = setup_in(home.path(), data.path());
    assert!(!install::claude_start_hook_installed(&setup.paths));
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    assert!(install::claude_start_hook_installed(&setup.paths));
    memory.close();
}

#[test]
fn the_wizard_defaults_the_auto_memory_by_whether_the_start_context_carries_the_rules() {
    for (budget, off) in [(None, true), (Some(0), false)] {
        let home = TempDir::new("wam-home");
        let data = TempDir::new("wam-data");
        let work = TempDir::new("wam-work");
        let project_dir = mcp::canonical(work.path());
        let memory_dir = memory_fixture(home.path(), &project_dir);
        std::fs::write(memory_dir.join("MEMORY.md"), PINNED_INDEX).unwrap();
        if let Some(chars) = budget {
            std::fs::write(
                data.path().join("config.json"),
                format!("{{\"version\": 1, \"memory\": {{\"start_context_chars\": {chars}}}}}\n"),
            )
            .unwrap();
        }
        let setup = setup_in(home.path(), data.path());
        let memory = Memory::open(data.path());
        // Language, model, import, auto-memory: Enter (the default); summaries: 2 (no);
        // develop: 2 (no); Enter applies.
        let (code, out) = run_wizard(&setup, &memory, "\n3\n\n\n2\n2\n\n");
        assert_eq!(code, 0, "{out}");
        let step = out
            .split("Claude Code's auto-memory")
            .nth(1)
            .and_then(|rest| rest.split("Session summaries").next())
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let recap = out
            .split("Ready to install")
            .nth(1)
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if off {
            assert!(
                step.contains("still reach every session through graphersal-memory's start context: 2 memories"),
                "{step}"
            );
            assert!(recap.contains("Auto-memory"), "{recap}");
            assert!(recap.contains("switched off. Your rules"), "{recap}");
            assert_eq!(settings_json(&setup)["autoMemoryEnabled"], false);
        } else {
            assert!(
                step.contains("The default is to keep it on: memory.start_context_chars is 0"),
                "{step}"
            );
            assert!(
                recap.contains("kept on: memory.start_context_chars is 0"),
                "{recap}"
            );
            assert_ne!(settings_json(&setup)["autoMemoryEnabled"], false);
        }
        memory.close();
    }
}

#[test]
fn a_pin_survives_the_round_trip_files_graph_files() {
    let home = TempDir::new("prt-home");
    let data = TempDir::new("prt-data");
    let work = TempDir::new("prt-work");
    let project_dir = mcp::canonical(work.path());
    let memory_dir = memory_fixture(home.path(), &project_dir);
    std::fs::write(memory_dir.join("MEMORY.md"), PINNED_INDEX).unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let project = project_dir.display().to_string();
    run_install(&setup, &memory, yes());
    // A pin set in the graph goes out too.
    memory.call(&project, "pin", json!({"slug": "odd-name"}));
    let other = TempDir::new("prt-home2");
    let elsewhere = setup_in(other.path(), data.path());
    let rows = InProcess(&memory).memories_all();
    crate::export::write(&elsewhere.agent(), data.path(), &rows).unwrap();
    let exported_dir = crate::agents::Harness::memory_dir(&elsewhere.agent(), &project_dir);
    let index = std::fs::read_to_string(exported_dir.join("MEMORY.md")).unwrap();
    assert!(
        index.contains("- [Graph first](graph-first.md) — study the graph first\n\n## Pinned\n"),
        "{index}"
    );
    assert!(
        index.contains("- [Release plan](release-plan.md) — Fridays only\n"),
        "{index}"
    );
    // Read back: the same pins, titles and hooks.
    let back = import::read_dir(&exported_dir, Some(project.clone()));
    let pins: Vec<(&str, bool, &str, &str)> = back
        .files
        .iter()
        .map(|f| (f.slug.as_str(), f.pinned, f.title.as_str(), f.hook.as_str()))
        .collect();
    assert!(
        pins.contains(&("release-plan", true, "Release plan", "Fridays only")),
        "{pins:?}"
    );
    assert!(pins.iter().any(|p| p.0 == "odd-name" && p.1), "{pins:?}");
    assert!(
        pins.contains(&("graph-first", false, "Graph first", "study the graph first")),
        "{pins:?}"
    );
    // Into a fresh store: pinned there too.
    let data2 = TempDir::new("prt-data2");
    let fresh = Memory::open(data2.path());
    let mut connect = fresh.connector();
    let mut state = ImportState::default();
    let plans = import::plan(vec![back], &state);
    let report = import::apply(&plans, &mut connect, &mut state, data2.path(), None);
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    let context = fresh
        .runtime
        .start_context(&project, crate::start_context::DEFAULT_CHARS)
        .unwrap();
    for slug in ["release-plan", "odd-name", "graph-first"] {
        assert!(
            context.text.contains(&format!("(memory: {slug})")),
            "{slug}: {}",
            context.text
        );
    }
    // A MEMORY.md that differs keeps its bytes; a missing pinned line goes under its own
    // heading, a missing unpinned one never into a Pinned section the file ends with.
    std::fs::write(
        exported_dir.join("MEMORY.md"),
        "# Mine\n## Pinned\n- [Graph first](graph-first.md) — x\n",
    )
    .unwrap();
    crate::export::write(&elsewhere.agent(), data.path(), &rows).unwrap();
    let kept = std::fs::read_to_string(exported_dir.join("MEMORY.md")).unwrap();
    assert!(
        kept.starts_with("# Mine\n## Pinned\n- [Graph first](graph-first.md) — x\n"),
        "{kept}"
    );
    let reread = import::read_dir(&exported_dir, Some(project.clone()));
    let pinned_of = |slug: &str| reread.files.iter().find(|f| f.slug == slug).unwrap().pinned;
    assert!(pinned_of("release-plan") && pinned_of("odd-name"), "{kept}");
    drop(connect);
    fresh.close();
    memory.close();
}

#[test]
fn an_import_keeps_the_memory_md_order_and_a_graph_order_until_the_line_moves() {
    let home = TempDir::new("ord-home");
    let data = TempDir::new("ord-data");
    let work = TempDir::new("ord-work");
    let project_dir = mcp::canonical(work.path());
    let memory_dir = memory_fixture(home.path(), &project_dir);
    for slug in ["rule-a", "rule-b", "rule-c"] {
        std::fs::write(
            memory_dir.join(format!("{slug}.md")),
            format!(
                "---\nname: {slug}\ndescription: {slug} summary\ntype: feedback\n---\n\n{slug}.\n"
            ),
        )
        .unwrap();
    }
    let index = |order: [&str; 3]| {
        order
            .iter()
            .map(|slug| format!("- [{slug}]({slug}.md) — hook {slug}\n"))
            .collect::<String>()
    };
    std::fs::write(
        memory_dir.join("MEMORY.md"),
        index(["rule-a", "rule-b", "rule-c"]),
    )
    .unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let project = project_dir.display().to_string();
    let order_of = |memory: &Memory| -> Vec<String> {
        let context = memory
            .runtime
            .start_context(&project, crate::start_context::DEFAULT_CHARS)
            .unwrap();
        let in_context: Vec<String> = context
            .text
            .lines()
            .filter_map(|line| line.split("(memory: ").nth(1))
            .map(|rest| rest.trim_end_matches(')').to_string())
            .filter(|slug| slug.starts_with("rule-"))
            .collect();
        let listed: Vec<String> = memory
            .call(&project, "memories", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["slug"].as_str())
            .filter(|slug| slug.starts_with("rule-"))
            .map(str::to_string)
            .collect();
        assert_eq!(in_context, listed, "start context and memories() agree");
        listed
    };
    let import_now = |memory: &Memory| -> String {
        let mut connect = memory.connector();
        let mut out = Vec::new();
        crate::commands::run_import(&setup, false, &mut connect, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    };
    let out = import_now(&memory);
    assert!(out.contains("6 new"), "{out}");
    // The MEMORY.md order, not reversed by the import's timestamps.
    assert_eq!(order_of(&memory), ["rule-a", "rule-b", "rule-c"]);
    let out = import_now(&memory);
    assert!(out.contains("0 new, 0 changed, 6 unchanged"), "{out}");
    // An order set in the graph stays through a re-import of a changed file whose line stayed.
    memory.call(
        &project,
        "pin",
        json!({"slug": "rule-c", "pinned": false, "order": -1}),
    );
    assert_eq!(order_of(&memory), ["rule-c", "rule-a", "rule-b"]);
    std::fs::write(
        memory_dir.join("rule-c.md"),
        "---\nname: rule-c\ndescription: rule-c changed\ntype: feedback\n---\n\nChanged.\n",
    )
    .unwrap();
    let out = import_now(&memory);
    assert!(out.contains("(0 new, 1 updated)"), "{out}");
    assert_eq!(order_of(&memory), ["rule-c", "rule-a", "rule-b"]);
    // Lines moved: the new MEMORY.md order wins.
    std::fs::write(
        memory_dir.join("MEMORY.md"),
        index(["rule-b", "rule-c", "rule-a"]),
    )
    .unwrap();
    import_now(&memory);
    assert_eq!(order_of(&memory), ["rule-b", "rule-c", "rule-a"]);
    let out = import_now(&memory);
    assert!(out.contains("0 new, 0 changed, 6 unchanged"), "{out}");
    memory.close();
}

/// A Claude Code memory directory for `project` with the memory files `(stem, kind, summary)`
/// listed in `MEMORY.md` in that order, and a transcript naming the project.
fn conflict_fixture(home: &Path, project: &Path, files: &[(&str, &str, &str)]) -> PathBuf {
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
    let mut index = String::from("# Index\n");
    for (stem, kind, summary) in files {
        index.push_str(&format!("- [{stem}]({stem}.md) — {summary}\n"));
        std::fs::write(
            memory.join(format!("{stem}.md")),
            format!("---\nname: {stem}\ndescription: {summary}\ntype: {kind}\n---\n\n{summary}.\n"),
        )
        .unwrap();
    }
    std::fs::write(memory.join("MEMORY.md"), index).unwrap();
    memory
}

fn import_text(setup: &Setup, memory: &Memory) -> String {
    let mut connect = memory.connector();
    let mut out = Vec::new();
    let code = crate::commands::run_import(setup, false, &mut connect, &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert_eq!(code, 0, "{out}");
    out
}

#[test]
fn an_import_reports_conflicts_and_files_and_never_brings_back_a_settled_memory() {
    let home = TempDir::new("conf-home");
    let data = TempDir::new("conf-data");
    let work = TempDir::new("conf-work");
    let project_dir = mcp::canonical(work.path());
    std::fs::create_dir_all(project_dir.join(".git")).unwrap();
    let files = [
        (
            "use-npm",
            "feedback",
            "Install dependencies with npm install",
        ),
        ("use-yarn", "feedback", "Install dependencies with yarn"),
        ("release-friday", "project", "Releases go out on Fridays"),
    ];
    let memory_dir = conflict_fixture(home.path(), &project_dir, &files);
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let project = project_dir.display().to_string();
    let npm_file = memory_dir.join("use-npm.md").display().to_string();
    let yarn_file = memory_dir.join("use-yarn.md").display().to_string();

    // A memory of another source (the agent's `remember`): imported memories near it wait for
    // the check (memories of one directory alone do not wait for each other, M16).
    memory.call(
        &project,
        "remember",
        json!({"slug": "deps-note", "kind": "project",
               "summary": "Install dependencies before running the tests", "body": "First install."}),
    );
    // The first import: every memory with similar ones waits for the agent's check.
    let out = import_text(&setup, &memory);
    assert!(out.contains("(3 new, 0 updated)"), "{out}");
    assert!(
        out.contains("have similar memories to check for conflicts (your agent: conflicts())"),
        "{out}"
    );
    let listed = memory.call(&project, "conflicts", json!({"k": 5}));
    assert!(listed["unchecked_total"].as_u64().unwrap() >= 2, "{listed}");
    let read = memory.call(&project, "memory", json!({"slug": "use-npm"}));
    assert_eq!(read["source"], npm_file.as_str(), "{read}");
    assert!(read["imported_at"].is_string(), "{read}");

    // The agent finds the two install rules contradict: the import report names both files.
    let opened = memory.call(
        &project,
        "settle",
        json!({"slug": "use-yarn", "decision": "conflict", "other": "use-npm"}),
    );
    assert_eq!(opened["open_conflicts"], 1, "{opened}");
    let found = memory.call(
        &project,
        "memories",
        json!({"text": "install dependencies", "k": 3}),
    );
    assert!(found.to_string().contains("open_conflict_with"), "{found}");
    std::fs::write(
        memory_dir.join("release-friday.md"),
        "---\nname: release-friday\ndescription: Releases go out on Fridays at noon\ntype: project\n---\n\nNoon.\n",
    )
    .unwrap();
    let out = import_text(&setup, &memory);
    assert!(out.contains("open memory conflicts: 1"), "{out}");
    assert!(
        out.contains(&format!(
            "use-yarn (file {yarn_file}) vs use-npm (file {npm_file})"
        )),
        "{out}"
    );

    // The user decides for pnpm: both old rules are superseded, their files named to edit.
    let newer = memory.call(
        &project,
        "remember",
        json!({"slug": "pnpm-instead-of-npm", "kind": "feedback",
               "summary": "We install dependencies with pnpm instead of npm or yarn",
               "body": "Use pnpm install.", "supersedes": ["use-npm", "use-yarn"]}),
    );
    // JSON text escapes a Windows path's backslashes.
    let edit = newer["edit_files"].to_string().replace("\\\\", "\\");
    assert!(
        edit.contains(&npm_file) && edit.contains(&yarn_file),
        "{newer}"
    );
    let listed = memory.call(&project, "conflicts", json!({"k": 5}));
    assert_eq!(
        listed["open"],
        json!([]),
        "superseding settles the conflict: {listed}"
    );

    // Unchanged files bring nothing back: the old rules stay marked.
    let out = import_text(&setup, &memory);
    assert!(out.contains("0 new, 0 changed, 3 unchanged"), "{out}");
    let read = memory.call(&project, "memory", json!({"slug": "use-npm"}));
    assert_eq!(
        read["superseded_by"][0]["slug"], "pnpm-instead-of-npm",
        "{read}"
    );

    // A memory forgotten in the graph stays forgotten when only its MEMORY.md line moves; a
    // moved line of a superseded memory keeps its mark and names its file as stale.
    memory.call(&project, "forget", json!({"slug": "release-friday"}));
    let index = std::fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap();
    let mut lines: Vec<&str> = index.lines().collect();
    lines[1..].reverse();
    std::fs::write(memory_dir.join("MEMORY.md"), lines.join("\n") + "\n").unwrap();
    let out = import_text(&setup, &memory);
    assert!(
        out.contains("1 forgotten in the memory and unchanged"),
        "{out}"
    );
    assert!(
        out.contains("files that still hold a superseded memory: 2"),
        "{out}"
    );
    assert!(
        out.contains(&format!(
            "use-npm (superseded by pnpm-instead-of-npm): {npm_file}"
        )),
        "{out}"
    );
    let gone = memory.call(&project, "memory", json!({"slug": "release-friday"}));
    assert!(gone.to_string().contains("no memory"), "{gone}");
    let read = memory.call(&project, "memory", json!({"slug": "use-yarn"}));
    assert_eq!(
        read["superseded_by"][0]["slug"], "pnpm-instead-of-npm",
        "{read}"
    );
    assert!(read.get("unchecked").is_none(), "{read}");
    // Re-importing the same state again is a no-op.
    let out = import_text(&setup, &memory);
    assert!(out.contains("0 new, 0 changed, 3 unchanged"), "{out}");

    // The file's text changes: the user wrote it again, so it is imported again.
    std::fs::write(
        memory_dir.join("release-friday.md"),
        "---\nname: release-friday\ndescription: Releases go out on Thursdays\ntype: project\n---\n\nThursday.\n",
    )
    .unwrap();
    let out = import_text(&setup, &memory);
    assert!(out.contains("(1 new, 0 updated)"), "{out}");
    memory.close();
}

// ---- M16: do-no-harm audit (reports/2026-11-12-do-no-harm-audit.md) ----

pub(crate) fn all_four() -> InstallOptions {
    InstallOptions {
        yes: true,
        harnesses: Some(Kind::ALL.to_vec()),
        ..InstallOptions::default()
    }
}

pub(crate) fn all_four_uninstall() -> UninstallOptions {
    UninstallOptions {
        harnesses: Some(Kind::ALL.to_vec()),
        ..uninstall_yes()
    }
}

/// A home with every agent's files (Claude Code, opencode, Codex, Cursor).
pub(crate) fn every_agent_home(home: &Path) {
    existing_home(home);
    existing_opencode_home(home);
    existing_codex_home(home);
    existing_cursor_home(home);
}

/// The files of `home` that mention graphersal-memory.
fn files_naming_us(home: &Path) -> Vec<PathBuf> {
    snapshot(home)
        .into_iter()
        .filter(|(_, bytes)| install::holds_ours(bytes))
        .map(|(path, _)| path)
        .collect()
}

#[test]
fn h1_install_refuses_a_data_dir_that_holds_other_files() {
    let home = TempDir::new("h1-home");
    let documents = home.path().join("Documents");
    std::fs::create_dir_all(documents.join("photos")).unwrap();
    std::fs::write(documents.join("thesis.docx"), "x").unwrap();
    existing_home(home.path());
    let before = snapshot(home.path());
    let setup = setup_in(home.path(), &documents);
    let memory_dir = TempDir::new("h1-store");
    let memory = Memory::open(memory_dir.path());
    let mut connect = memory.connector();
    let err =
        install::install(&setup, &yes(), &mut connect, &mut &b""[..], &mut Vec::new()).unwrap_err();
    assert!(
        err.contains("already holds other files") && err.contains("thesis.docx"),
        "{err}"
    );
    assert!(
        err.contains(&format!(
            "Documents{}graphersal-memory",
            std::path::MAIN_SEPARATOR
        )),
        "names a way out: {err}"
    );
    assert_eq!(snapshot(home.path()), before, "nothing changed");
    memory.close();
}

#[test]
fn h1_purge_deletes_only_what_graphersal_memory_created() {
    let home = TempDir::new("h1p-home");
    let data = TempDir::new("h1p-data");
    existing_home(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    assert_eq!(run_install(&setup, &memory, yes()).0, 0);
    memory.close();
    // The user's own files land in the same folder later (a data directory set to a folder of
    // theirs, as in the audit's thesis.docx case).
    std::fs::create_dir_all(data.path().join("photos")).unwrap();
    std::fs::write(data.path().join("thesis.docx"), "x").unwrap();
    let (code, out) = run_uninstall(
        &setup,
        UninstallOptions {
            purge: true,
            ..uninstall_yes()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert_eq!(read(&data.path().join("thesis.docx")), "x");
    assert!(data.path().join("photos").is_dir());
    for name in store::OUR_ENTRIES {
        assert!(!data.path().join(name).exists(), "{name} deleted");
    }
    assert!(
        out.contains("kept what else is in it (photos, thesis.docx)"),
        "{out}"
    );
}

#[cfg(unix)]
#[test]
fn h2_linked_config_files_stay_links_through_install_and_uninstall() {
    let home = TempDir::new("h2-home");
    let data = TempDir::new("h2-data");
    every_agent_home(home.path());
    let dot = home.path().join("dotfiles");
    std::fs::create_dir_all(&dot).unwrap();
    let files = [
        ".claude.json",
        ".claude/settings.json",
        ".claude/CLAUDE.md",
        ".config/opencode/opencode.jsonc",
        ".codex/config.toml",
        ".codex/AGENTS.md",
        ".cursor/mcp.json",
        ".cursor/hooks.json",
    ];
    for (n, file) in files.iter().enumerate() {
        let path = home.path().join(file);
        let target = dot.join(format!("{n}"));
        std::fs::rename(&path, &target).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
    }
    let before = snapshot(&dot);
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, all_four());
    assert_eq!(code, 0, "{out}");
    let is_link = |file: &str| {
        std::fs::symlink_metadata(home.path().join(file))
            .unwrap()
            .file_type()
            .is_symlink()
    };
    for (n, file) in files.iter().enumerate() {
        assert!(is_link(file), "{file} stays a link after install");
        let text = read(&dot.join(format!("{n}")));
        assert!(
            text.contains("graphersal-memory"),
            "{file}: the target holds our parts"
        );
    }
    let (code, out) = run_uninstall(&setup, all_four_uninstall());
    assert_eq!(code, 0, "{out}");
    for file in files {
        assert!(is_link(file), "{file} stays a link after uninstall");
    }
    assert_eq!(
        snapshot(&dot),
        before,
        "every target byte for byte as before"
    );
    memory.close();
}

#[test]
fn h3_one_install_at_a_time() {
    let home = TempDir::new("h3-home");
    let data = TempDir::new("h3-data");
    existing_home(home.path());
    let before = snapshot(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let held = install::InstallLock::take(&setup, true).unwrap();
    let mut connect = memory.connector();
    let err =
        install::install(&setup, &yes(), &mut connect, &mut &b""[..], &mut Vec::new()).unwrap_err();
    assert!(err.contains("another graphersal-memory install"), "{err}");
    assert_eq!(snapshot(home.path()), before, "nothing changed");
    drop(held);
    assert_eq!(run_install(&setup, &memory, yes()).0, 0);
    let held = install::InstallLock::take(&setup, true).unwrap();
    let err =
        install::uninstall(&setup, &uninstall_yes(), &mut &b""[..], &mut Vec::new()).unwrap_err();
    assert!(err.contains("another graphersal-memory install"), "{err}");
    drop(held);
    memory.close();
}

#[test]
fn h3_a_backup_that_holds_our_parts_is_never_put_back() {
    let home = TempDir::new("h3b-home");
    let data = TempDir::new("h3b-data");
    every_agent_home(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    assert_eq!(run_install(&setup, &memory, all_four()).0, 0);
    // The install records are lost (a second install that ran at the same time and backed up
    // the first one's output as the "original"): install again from there.
    std::fs::remove_dir_all(data.path().join("install")).unwrap();
    assert_eq!(run_install(&setup, &memory, all_four()).0, 0);
    let (code, out) = run_uninstall(&setup, all_four_uninstall());
    assert_eq!(code, 0, "{out}");
    let left: Vec<PathBuf> = files_naming_us(home.path())
        .into_iter()
        .filter(|p| !p.starts_with("graphersal-memory-export"))
        .collect();
    assert!(left.is_empty(), "no part of ours stays: {left:?}\n{out}");
    memory.close();
}

#[test]
fn h4_a_reinstall_keeps_the_auto_memory_the_user_switched_back_on() {
    let home = TempDir::new("h4-home");
    let data = TempDir::new("h4-data");
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    std::fs::write(home.path().join(".claude/settings.json"), "{}\n").unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    assert_eq!(run_install(&setup, &memory, yes()).0, 0);
    assert_eq!(settings_json(&setup)["autoMemoryEnabled"], false);
    let on = read(&setup.paths.settings).replace(
        "\"autoMemoryEnabled\": false",
        "\"autoMemoryEnabled\": true",
    );
    std::fs::write(&setup.paths.settings, on).unwrap();
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    assert_eq!(settings_json(&setup)["autoMemoryEnabled"], true, "{out}");
    memory.close();
}

#[cfg(unix)]
#[test]
fn b3_hook_commands_keep_dollar_backtick_and_backslash_literal() {
    let home = TempDir::new("b3-home");
    let data = TempDir::new("b3-data");
    let mut setup = setup_in(home.path(), data.path());
    setup.exe = PathBuf::from("/opt/a $HOME `id` \\x 'q'/graphersal-memory");
    setup.dir_args = vec!["--data-dir".into(), "/d/$(echo no)".into()];
    let command = setup.hook_command("session-start");
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("printf '%s|' {command}"))
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "/opt/a $HOME `id` \\x 'q'/graphersal-memory|hook|session-start|--data-dir|/d/$(echo no)|"
    );
    assert!(setup.ours().command(&command));
    // The forms earlier versions wrote are still recognised.
    assert!(
        setup
            .ours()
            .command("\"/opt/gm/bin/graphersal-memory\" hook session-end --data-dir \"/d\"")
    );
}

#[test]
fn b4_our_hook_moved_into_a_group_of_the_user_is_removed_alone() {
    let home = TempDir::new("b4-home");
    let data = TempDir::new("b4-data");
    existing_home(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    assert_eq!(run_install(&setup, &memory, yes()).0, 0);
    // The user moves our SessionStart hook into their own group.
    let mut settings = settings_json(&setup);
    let ours = settings["hooks"]["SessionStart"][1]["hooks"][0].clone();
    settings["hooks"]["SessionStart"] = json!([{"hooks": [
        {"type": "command", "command": "echo mine"}, ours]}]);
    std::fs::write(&setup.paths.settings, settings.to_string()).unwrap();
    // A re-install sees it installed (no second copy).
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        settings_json(&setup)["hooks"]["SessionStart"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    let settings = settings_json(&setup);
    assert_eq!(
        settings["hooks"]["SessionStart"],
        json!([{"hooks": [{"type": "command", "command": "echo mine"}]}]),
        "{settings}"
    );
    assert!(
        files_naming_us(home.path()).is_empty(),
        "{:?}",
        files_naming_us(home.path())
    );
    memory.close();
}

#[test]
fn b5_a_renamed_binary_recognises_its_own_entries() {
    let home = TempDir::new("b5-home");
    let data = TempDir::new("b5-data");
    every_agent_home(home.path());
    let before = snapshot(home.path());
    let mut setup = setup_in(home.path(), data.path());
    setup.exe = PathBuf::from("/opt/gm/bin/gm-0.2");
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, all_four());
    assert_eq!(code, 0, "{out}");
    let (code, out) = run_install(&setup, &memory, all_four());
    assert_eq!(code, 0, "a re-install sees no conflict: {out}");
    assert!(!out.contains("conflict"), "{out}");
    let problems = doctor_problems(&setup);
    assert!(
        !problems.iter().any(|p| p.contains("another program")),
        "{problems:?}"
    );
    // The binary is renamed again: the recorded one is still ours.
    setup.exe = PathBuf::from("/opt/gm/bin/gm-0.3");
    std::fs::write(
        &setup.paths.user_config,
        read(&setup.paths.user_config).replacen('{', "{\"tipsShown\": 1, ", 1),
    )
    .unwrap();
    let (code, out) = run_uninstall(&setup, all_four_uninstall());
    assert_eq!(code, 0, "{out}");
    let after: Vec<_> = snapshot(home.path())
        .into_iter()
        .filter(|(p, _)| !p.starts_with(".claude.json"))
        .collect();
    let before: Vec<_> = before
        .into_iter()
        .filter(|(p, _)| !p.starts_with(".claude.json"))
        .collect();
    assert_eq!(after, before);
    assert!(!read(&setup.paths.user_config).contains("gm-0.2"));
    memory.close();
}

#[cfg(unix)]
#[test]
fn b6_a_write_that_fails_midway_puts_back_everything() {
    use std::os::unix::fs::PermissionsExt;
    let home = TempDir::new("b6-home");
    let data = TempDir::new("b6-data");
    existing_home(home.path());
    existing_codex_home(home.path());
    let before = snapshot(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    // Codex's directory cannot take a new file: its writes fail after Claude Code's succeeded.
    let codex = home.path().join(".codex");
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o555)).unwrap();
    let mut connect = memory.connector();
    let options = InstallOptions {
        harnesses: Some(vec![Kind::ClaudeCode, Kind::Codex]),
        ..yes()
    };
    let result = install::install(
        &setup,
        &options,
        &mut connect,
        &mut &b""[..],
        &mut Vec::new(),
    );
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
    let err = result.unwrap_err();
    assert!(err.contains("put back what it had written"), "{err}");
    assert!(
        err.contains(".claude.json"),
        "names what was put back: {err}"
    );
    assert_eq!(snapshot(home.path()), before, "every file as before");
    for kind in Kind::ALL {
        assert!(
            !data
                .path()
                .join("install")
                .join(format!("{}.json", kind.id()))
                .exists()
        );
    }
    memory.close();
}

#[test]
fn b8_a_read_only_file_is_refused_and_nothing_changes() {
    let home = TempDir::new("b8-home");
    let data = TempDir::new("b8-data");
    existing_home(home.path());
    existing_codex_home(home.path());
    let config = home.path().join(".codex/config.toml");
    let mut perms = std::fs::metadata(&config).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&config, perms.clone()).unwrap();
    let before = snapshot(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let mut connect = memory.connector();
    let options = InstallOptions {
        harnesses: Some(vec![Kind::ClaudeCode, Kind::Codex]),
        ..yes()
    };
    let err = install::install(
        &setup,
        &options,
        &mut connect,
        &mut &b""[..],
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(
        err.contains("config.toml is read-only") && err.contains("nothing was changed"),
        "{err}"
    );
    assert_eq!(snapshot(home.path()), before);
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&config, perms).unwrap();
    memory.close();
}

/// Input that answers "y" to the question, but first rewrites a file the way a running agent
/// does (Claude Code rewrites `~/.claude.json` often).
pub(crate) struct RacingInput {
    pub(crate) path: PathBuf,
    pub(crate) answered: bool,
}

impl std::io::Read for RacingInput {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.answered {
            return Ok(0);
        }
        self.answered = true;
        let text = std::fs::read_to_string(&self.path)?;
        std::fs::write(
            &self.path,
            text.replacen('{', "{\"lastSessionId\": \"abc\", ", 1),
        )?;
        buf[..2].copy_from_slice(b"y\n");
        Ok(2)
    }
}

#[test]
fn d5_a_file_rewritten_while_install_waits_is_merged_again_never_overwritten() {
    let home = TempDir::new("d5-home");
    let data = TempDir::new("d5-data");
    existing_home(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let mut connect = memory.connector();
    let mut input = BufReader::new(RacingInput {
        path: setup.paths.user_config.clone(),
        answered: false,
    });
    let options = InstallOptions {
        yes: false,
        ..yes()
    };
    let mut out = Vec::new();
    let code = install::install(&setup, &options, &mut connect, &mut input, &mut out).unwrap();
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&out));
    let config: Value = serde_json::from_str(&read(&setup.paths.user_config)).unwrap();
    assert_eq!(
        config["lastSessionId"], "abc",
        "the agent's write is kept: {config}"
    );
    assert!(
        config["mcpServers"]["graphersal-memory"].is_object(),
        "{config}"
    );
    assert_eq!(config["mcpServers"]["other"]["command"], "x");
    // A change between the plan and the write is caught by the fingerprint the plan was made
    // from.
    let mut state = install::State::default();
    let mut bases = std::collections::BTreeMap::new();
    bases.insert(
        setup.paths.user_config.clone(),
        Some("0000000000000000".to_string()),
    );
    let change = install::Change {
        path: setup.paths.user_config.clone(),
        after: Some("{}".into()),
        created: Vec::new(),
        separator: None,
    };
    let err = install::apply_changes(
        &setup,
        &[change],
        &mut state,
        &mut Vec::new(),
        &mut Default::default(),
        &bases,
        &mut Vec::new(),
        &mut |_, _| {},
    )
    .unwrap_err();
    assert!(err.starts_with(crate::fsutil::CHANGED_MEANWHILE), "{err}");
    assert_eq!(
        config,
        serde_json::from_str::<Value>(&read(&setup.paths.user_config)).unwrap()
    );
    memory.close();
}

/// A memory directory with the formatting people and Claude Code really write: CRLF, a blank
/// line between the why and the how, a folded description, a metadata key of Claude Code's,
/// no blank line after the front matter.
fn formatted_fixture(home: &Path, project: &Path) -> PathBuf {
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
        "- [CRLF](crlf-rule.md) — crlf\n- [Spaced](spaced-rule.md) — spaced\n\
         - [Folded](folded-rule.md) — folded\n- [Meta](meta-rule.md) — meta\n",
    )
    .unwrap();
    std::fs::write(
        memory.join("crlf-rule.md"),
        "---\r\nname: crlf-rule\r\ndescription: Line ends of Windows\r\nmetadata:\r\n  type: feedback\r\n---\r\n\r\nKeep CRLF.\r\n",
    )
    .unwrap();
    std::fs::write(
        memory.join("spaced-rule.md"),
        "---\nname: spaced-rule\ndescription: A blank line between why and how\nmetadata:\n  type: feedback\n---\n\nSpace them.\n\n**Why:** readable.\n\n**How to apply:** always.\n",
    )
    .unwrap();
    std::fs::write(
        memory.join("folded-rule.md"),
        "---\nname: folded-rule\ndescription: >-\n  A description folded\n  over two lines\nmetadata:\n  type: project\n---\nNo blank line after the front matter.\n",
    )
    .unwrap();
    std::fs::write(
        memory.join("meta-rule.md"),
        "---\nname: meta-rule\ndescription: Extra keys of others\nmetadata:\n  type: user\n  originSessionId: 1234-abcd\n---\n\nThe user is a tester.\n",
    )
    .unwrap();
    memory
}

#[test]
fn b1_b2_the_export_round_trip_makes_no_siblings_and_refreshes_files_unchanged_since_import() {
    let home = TempDir::new("b1-home");
    let data = TempDir::new("b1-data");
    let work = TempDir::new("b1-work");
    let project_dir = mcp::canonical(work.path());
    let dir = formatted_fixture(home.path(), &project_dir);
    let before = snapshot(&dir);
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    let project = project_dir.display().to_string();
    let folded = memory.call(&project, "memory", json!({"slug": "folded-rule"}));
    assert_eq!(
        folded["summary"], "A description folded over two lines",
        "{folded}"
    );
    // Nothing changed in the graph: the export leaves every file as it is, writes no sibling.
    let rows = InProcess(&memory).memories_all();
    let report = crate::export::write(&setup.agent(), data.path(), &rows).unwrap();
    assert_eq!(snapshot(&dir), before, "{report:?}");
    assert!(report.conflicts.is_empty(), "{report:?}");
    // A memory changed in the graph after install reaches its file (unchanged since import),
    // and the metadata key of Claude Code's stays.
    memory.call(
        &project,
        "remember",
        json!({"slug": "meta-rule", "kind": "user", "summary": "Extra keys of others",
               "body": "The user is a tester and a writer."}),
    );
    let rows = InProcess(&memory).memories_all();
    let report = crate::export::write(&setup.agent(), data.path(), &rows).unwrap();
    assert_eq!(
        report.refreshed,
        vec![dir.join("meta-rule.md")],
        "{report:?}"
    );
    let text = read(&dir.join("meta-rule.md"));
    assert!(text.contains("a tester and a writer"), "{text}");
    assert!(text.contains("  originSessionId: 1234-abcd\n---"), "{text}");
    assert!(report.conflicts.is_empty(), "{report:?}");
    // A file the user edited since its import keeps its text; the graph's goes beside it, and
    // a re-install never imports that sibling as a memory.
    std::fs::write(
        dir.join("crlf-rule.md"),
        "---\nname: crlf-rule\ndescription: Mine now\n---\nMine.\n",
    )
    .unwrap();
    memory.call(
        &project,
        "remember",
        json!({"slug": "crlf-rule", "kind": "feedback", "summary": "Line ends of Windows",
               "body": "Keep CRLF, always."}),
    );
    let rows = InProcess(&memory).memories_all();
    let report = crate::export::write(&setup.agent(), data.path(), &rows).unwrap();
    assert_eq!(report.conflicts.len(), 1, "{report:?}");
    assert!(dir.join("crlf-rule.graphersal.md").exists());
    let scanned = import::read_dir(&dir, Some(project.clone()));
    assert!(
        !scanned.files.iter().any(|f| f.slug.contains("graphersal")),
        "{:?}",
        scanned.files.iter().map(|f| &f.slug).collect::<Vec<_>>()
    );
    memory.close();
}

#[test]
fn a_superseded_memory_is_written_but_left_out_of_the_index_and_a_conflict_is_named() {
    let home = TempDir::new("sup-home");
    let data = TempDir::new("sup-data");
    let work = TempDir::new("sup-work");
    let project_dir = mcp::canonical(work.path());
    std::fs::create_dir_all(project_dir.join(".git")).unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let project = project_dir.display().to_string();
    for (slug, summary) in [
        ("use-npm", "Install with npm"),
        ("tabs", "Indent with tabs"),
        ("spaces", "Indent with spaces"),
    ] {
        memory.call(
            &project,
            "remember",
            json!({"slug": slug, "kind": "feedback", "summary": summary, "body": summary}),
        );
    }
    memory.call(
        &project,
        "remember",
        json!({"slug": "use-pnpm", "kind": "feedback",
        "summary": "Install with pnpm instead of npm", "body": "pnpm", "supersedes": ["use-npm"]}),
    );
    memory.call(
        &project,
        "settle",
        json!({"slug": "tabs", "decision": "conflict", "other": "spaces"}),
    );
    let rows = InProcess(&memory).memories_all();
    crate::projects::resolve(data.path(), &project).unwrap();
    crate::export::write(&setup.agent(), data.path(), &rows).unwrap();
    let dir = crate::agents::Harness::memory_dir(&setup.agent(), &project_dir);
    let index = read(&dir.join("MEMORY.md"));
    assert!(!index.contains("(use-npm.md)"), "{index}");
    assert!(index.contains("(use-pnpm.md)"), "{index}");
    let npm = read(&dir.join("use-npm.md"));
    let body = npm.split("---\n\n").nth(1).unwrap();
    assert!(body.starts_with("SUPERSEDED by use-pnpm on 20"), "{npm}");
    let tabs = read(&dir.join("tabs.md"));
    assert!(tabs.contains("CONFLICT with spaces:"), "{tabs}");
    // Imported back, the notes are not part of the memory.
    let scanned = import::read_dir(&dir, Some(project.clone()));
    let npm = scanned.files.iter().find(|f| f.slug == "use-npm").unwrap();
    assert_eq!(npm.body, "Install with npm");
    memory.close();
}

#[test]
fn memories_imported_together_from_one_directory_do_not_wait_for_each_other() {
    let home = TempDir::new("unch-home");
    let data = TempDir::new("unch-data");
    let work = TempDir::new("unch-work");
    let project_dir = mcp::canonical(work.path());
    std::fs::create_dir_all(project_dir.join(".git")).unwrap();
    // A realistic memory directory: 64 files on related topics (the hash embedder finds
    // neighbours among them).
    let topics = [
        "tests", "release", "build", "deploy", "review", "docs", "style", "commits",
    ];
    let mut files = Vec::new();
    for n in 0..64 {
        let topic = topics[n % topics.len()];
        files.push((
            format!("{topic}-rule-{n}"),
            if n % 3 == 0 { "feedback" } else { "project" },
            format!(
                "Rule {n} about the {topic} of the project: run the {topic} step before merging"
            ),
        ));
    }
    let refs: Vec<(&str, &str, &str)> = files
        .iter()
        .map(|(a, b, c)| (a.as_str(), *b, c.as_str()))
        .collect();
    conflict_fixture(home.path(), &project_dir, &refs);
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    let project = project_dir.display().to_string();
    let listed = memory.call(&project, "conflicts", json!({"k": 5}));
    assert_eq!(listed["unchecked_total"], 0, "{listed}");
    let context = memory.runtime.start_context(&project, 4000).unwrap();
    assert!(
        !context.status_line().contains("to check"),
        "{}",
        context.status_line()
    );
    // A memory of another source near them is checked.
    let new = memory.call(&project, "remember", json!({"slug": "tests-again", "kind": "feedback",
        "summary": "Rule about the tests of the project: run the tests step before merging", "body": "x"}));
    assert!(new["nearest"].is_array(), "{new}");
    memory.close();
}

#[test]
fn memories_imported_before_m12_get_their_file_as_source_once() {
    let data = TempDir::new("mig-data");
    let memory = Memory::open(data.path());
    // As an old import wrote it: source "user", no imported_at.
    memory.call(
        "/w/app",
        "remember",
        json!({"slug": "old-rule", "kind": "feedback",
        "summary": "An old rule", "body": "Old."}),
    );
    let mut state = ImportState::default();
    let file = import::MemoryFile {
        path: PathBuf::from("/h/.claude/projects/-w-app/memory/old-rule.md"),
        slug: "old-rule".into(),
        kind: "feedback".into(),
        kind_defaulted: false,
        summary: "An old rule".into(),
        body: "Old.".into(),
        why: String::new(),
        how_to_apply: String::new(),
        title: String::new(),
        hook: String::new(),
        order: 0,
        listed: false,
        links: Vec::new(),
        pinned: false,
        fingerprint: "f".into(),
        content: "c".into(),
        modified_at: String::new(),
    };
    state.record_for_test(&file, "/w/app");
    state.save(data.path()).unwrap();
    let call = |name: &str, args: Value| memory.runtime.call(name, args);
    assert_eq!(import::migrate_sources(data.path(), &call).unwrap(), 1);
    let read = memory.call("/w/app", "memory", json!({"slug": "old-rule"}));
    assert_eq!(
        read["source"], "/h/.claude/projects/-w-app/memory/old-rule.md",
        "{read}"
    );
    assert!(read["imported_at"].is_string(), "{read}");
    // Once only: an agent's later rewrite stays its own.
    memory.call(
        "/w/app",
        "remember",
        json!({"slug": "old-rule", "kind": "feedback",
        "summary": "An old rule, rewritten", "body": "New."}),
    );
    assert_eq!(import::migrate_sources(data.path(), &call).unwrap(), 0);
    let read = memory.call("/w/app", "memory", json!({"slug": "old-rule"}));
    assert_eq!(read["source"], "remember", "{read}");
    memory.close();
}

#[test]
fn n2_a_crlf_instruction_file_gets_a_crlf_block_and_gets_it_back_out_cleanly() {
    let home = TempDir::new("n2-home");
    let data = TempDir::new("n2-data");
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    let original = "# Rules\r\n\r\nBe brief.\r\n";
    std::fs::write(home.path().join(".claude/CLAUDE.md"), original).unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    assert_eq!(run_install(&setup, &memory, yes()).0, 0);
    let text = read(&setup.paths.instructions);
    assert!(
        !text.replace("\r\n", "").contains('\n'),
        "every line end is CRLF"
    );
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("conflict"), "{out}");
    // The user adds a line: the structural removal leaves no stray line end.
    std::fs::write(&setup.paths.instructions, format!("{text}Mine.\r\n")).unwrap();
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        read(&setup.paths.instructions),
        format!("{original}\r\nMine.\r\n")
    );
    memory.close();
}

#[test]
fn n3_an_empty_settings_file_is_an_empty_configuration() {
    let home = TempDir::new("n3-home");
    let data = TempDir::new("n3-data");
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    std::fs::write(home.path().join(".claude/settings.json"), "").unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, yes());
    assert_eq!(code, 0, "{out}");
    assert!(settings_json(&setup)["hooks"]["SessionStart"].is_array());
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    assert_eq!(read(&setup.paths.settings), "");
    memory.close();
}

#[test]
fn n4_uninstall_says_when_it_kept_the_users_newer_auto_memory_value() {
    let home = TempDir::new("n4-home");
    let data = TempDir::new("n4-data");
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    std::fs::write(home.path().join(".claude/settings.json"), "{}\n").unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    assert_eq!(run_install(&setup, &memory, yes()).0, 0);
    let on = read(&setup.paths.settings).replace(
        "\"autoMemoryEnabled\": false",
        "\"autoMemoryEnabled\": true",
    );
    std::fs::write(&setup.paths.settings, on).unwrap();
    let (code, out) = run_uninstall(&setup, uninstall_yes());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("auto-memory: kept as you set it after install"),
        "{out}"
    );
    assert!(!out.contains("back to how it was"), "{out}");
    assert_eq!(settings_json(&setup)["autoMemoryEnabled"], true);
    memory.close();
}

#[test]
fn b7_uninstall_with_nothing_installed_creates_nothing() {
    let home = TempDir::new("b7-home");
    let data = TempDir::new("b7-data");
    let missing = data.path().join("none");
    let setup = setup_in(home.path(), &missing);
    let memory_dir = TempDir::new("b7-store");
    let memory = Memory::open(memory_dir.path());
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
        out.contains("Nothing of graphersal-memory is installed"),
        "{out}"
    );
    assert!(!missing.exists(), "no data directory created");
    assert!(snapshot(home.path()).is_empty());
    memory.close();
}

#[test]
fn b9_a_config_error_names_the_file_and_the_way_out() {
    let home = TempDir::new("b9-home");
    let data = TempDir::new("b9-data");
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    std::fs::write(
        home.path().join(".claude/settings.json"),
        "{\"hooks\": 3}\n",
    )
    .unwrap();
    existing_codex_home(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let mut connect = memory.connector();
    let options = InstallOptions {
        harnesses: Some(vec![Kind::ClaudeCode, Kind::Codex]),
        ..yes()
    };
    let err = install::install(
        &setup,
        &options,
        &mut connect,
        &mut &b""[..],
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(err.contains("settings.json"), "{err}");
    assert!(err.contains("nothing was changed"), "{err}");
    assert!(err.contains("--harness <agent>` for codex"), "{err}");
    memory.close();
}

#[test]
fn h5_a_run_dir_whose_socket_path_is_too_long_is_refused_before_any_change() {
    let home = TempDir::new("h5-home");
    let data = TempDir::new("h5-data");
    existing_home(home.path());
    let before = snapshot(home.path());
    let mut setup = setup_in(home.path(), data.path());
    setup.run_dir = PathBuf::from("/").join("r".repeat(120));
    let memory = Memory::open(data.path());
    let mut connect = memory.connector();
    let err =
        install::install(&setup, &yes(), &mut connect, &mut &b""[..], &mut Vec::new()).unwrap_err();
    assert!(
        err.contains("longer than") && err.contains("nothing was changed"),
        "{err}"
    );
    assert_eq!(snapshot(home.path()), before);
    memory.close();
}

#[test]
fn n5_text_the_user_added_to_the_cursor_rule_file_loses_our_front_matter() {
    let home = TempDir::new("n5-home");
    let data = TempDir::new("n5-data");
    existing_cursor_home(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    assert_eq!(run_install(&setup, &memory, cursor_only()).0, 0);
    let rules = read(&setup.cursor.rules_file);
    std::fs::write(&setup.cursor.rules_file, format!("{rules}\nMine.\n")).unwrap();
    let (code, out) = run_uninstall(&setup, cursor_uninstall());
    assert_eq!(code, 0, "{out}");
    let left = read(&setup.cursor.rules_file);
    assert!(!left.contains("graphersal-memory"), "{left}");
    assert!(
        left.starts_with("---\nalwaysApply: true\n---\n") && left.ends_with("Mine.\n"),
        "{left}"
    );
    memory.close();
}

#[test]
fn note18_cursor_in_its_windows_install_location_is_found() {
    let home = TempDir::new("n18-home");
    let data = TempDir::new("n18-data");
    let setup = bare_setup_in(home.path(), data.path());
    assert!(setup.program(Kind::Cursor).is_none());
    let exe = home.path().join("AppData/Local/Programs/cursor/Cursor.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, "").unwrap();
    assert_eq!(setup.program(Kind::Cursor), Some(exe));
}

#[test]
fn note21_commands_shown_keep_full_paths_and_prose_uses_the_tilde() {
    let home = Path::new("/Users/ann");
    assert_eq!(
        install::tilde(
            "in /Users/ann/.claude/settings.json: add `'/Users/ann/bin/gm' hook x`",
            home
        ),
        "in ~/.claude/settings.json: add `'/Users/ann/bin/gm' hook x`"
    );
    // An unpaired backtick is prose.
    assert_eq!(install::tilde("a ` /Users/ann/x", home), "a ` ~/x");
    let tmp = TempDir::new("n21-home");
    let data = TempDir::new("n21-data");
    let mut setup = setup_in(tmp.path(), data.path());
    setup.exe = tmp.path().join("bin/graphersal-memory");
    let memory = Memory::open(data.path());
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            dry_run: true,
            ..InstallOptions::default()
        },
    );
    assert_eq!(code, 0, "{out}");
    let full = format!("`'{}' hook session-start", setup.exe.display());
    assert!(
        out.contains(&full),
        "the hook command with its full path: {out}"
    );
    memory.close();
}

#[test]
fn note23_the_wizard_shows_paths_under_the_home_directory_with_a_tilde() {
    let home = TempDir::new("n23-home");
    let data = TempDir::new("n23-data");
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    std::fs::write(home.path().join(".claude/settings.json"), "{}\n").unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (_, out) = run_wizard(&setup, &memory, "\n\nn\n");
    assert!(
        out.contains(&native("false in ~/.claude/settings.json")),
        "{out}"
    );
    assert!(!out.contains(&home.path().display().to_string()), "{out}");
    memory.close();
}

// ------------------------------------------------------------------------------------------
// M17: CodeWhale.
// ------------------------------------------------------------------------------------------

fn codewhale_only() -> InstallOptions {
    InstallOptions {
        yes: true,
        harnesses: Some(vec![Kind::CodeWhale]),
        ..InstallOptions::default()
    }
}

fn codewhale_uninstall() -> UninstallOptions {
    UninstallOptions {
        yes: true,
        no_export: true,
        harnesses: Some(vec![Kind::CodeWhale]),
        ..UninstallOptions::default()
    }
}

#[test]
fn codewhale_install_then_uninstall_leaves_existing_files_byte_identical() {
    let home = TempDir::new("cw-rt-home");
    let data = TempDir::new("cw-rt-data");
    existing_codewhale_home(home.path());
    let before = snapshot(home.path());
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, codewhale_only());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.starts_with("graphersal-memory install (CodeWhale)"),
        "{out}"
    );
    assert!(
        !out.contains("Claude Code"),
        "no Claude Code wording: {out}"
    );
    let mcp = crate::jsonedit::value_of(&read(&setup.codewhale.mcp_file)).unwrap();
    assert_eq!(mcp["servers"]["git"]["command"], "mcp-git");
    assert_eq!(mcp["timeouts"]["connect_timeout"], 30);
    assert_eq!(
        mcp["servers"]["graphersal-memory"],
        crate::codewhale::install::mcp_entry(&setup, "use", true)
    );
    assert_eq!(
        mcp["servers"]["graphersal-memory"]["command"],
        "/opt/gm/bin/graphersal-memory"
    );
    assert!(mcp["servers"]["graphersal-memory"]["disabled"].is_null());
    assert_eq!(
        mcp["servers"]["graphersal-memory-develop"]["disabled"],
        true
    );
    assert!(mcp.get("mcpServers").is_none(), "one servers key only");
    let config = read(&setup.codewhale.config_file);
    assert!(
        config.starts_with("# my CodeWhale settings\nprovider = \"deepseek\""),
        "{config}"
    );
    assert!(
        config.contains("# loaded up front\nalways_load = [\"web_search\", "),
        "{config}"
    );
    let doc = crate::tomledit::parse(&config).unwrap();
    let mut expected = vec!["web_search".to_string()];
    expected.extend(crate::codewhale::tool_names());
    assert_eq!(doc["tools"]["always_load"], json!(expected));
    assert_eq!(doc["providers"]["deepseek"]["model"], "deepseek-v4");
    let agents = read(&home.path().join(".codewhale").join("AGENTS.md"));
    assert!(
        agents.starts_with("# My rules\n\nTabs, not spaces.\n"),
        "{agents}"
    );
    assert!(
        agents.contains("mcp_graphersal-memory_memories"),
        "{agents}"
    );
    // Nothing of the other harnesses, no hooks.
    assert!(!setup.paths.user_config.exists() && !setup.paths.config_dir.exists());
    assert!(!setup.codex.codex_home.exists() && !setup.cursor.cursor_dir.exists());
    assert!(!config.contains("[hooks"), "{config}");
    assert!(data.path().join("install").join("codewhale.json").exists());
    let inspection = crate::codewhale::install::inspect(&setup);
    assert_eq!(inspection.use_server, crate::install::PartState::Installed);
    assert_eq!(inspection.always_load, crate::install::PartState::Installed);
    assert_eq!(inspection.block, crate::install::PartState::Installed);
    assert!(
        crate::codewhale::install::status_lines(&setup)
            .iter()
            .any(|l| l.contains("no hooks")),
    );

    // Idempotent.
    let again = snapshot(home.path());
    let (code, out) = run_install(&setup, &memory, codewhale_only());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), again, "{out}");
    assert!(out.contains("already up to date"), "{out}");

    let (code, out) = run_uninstall(&setup, codewhale_uninstall());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), before, "{out}");
    assert!(!data.path().join("install").join("codewhale.json").exists());
    memory.close();
}

#[test]
fn codewhale_files_install_created_go_and_codewhales_own_edits_stay() {
    let home = TempDir::new("cw-new-home");
    let data = TempDir::new("cw-new-data");
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, codewhale_only());
    assert_eq!(code, 0, "{out}");
    let config = read(&setup.codewhale.config_file);
    assert!(config.contains(crate::tomledit::BEGIN), "{config}");
    assert_eq!(
        crate::tomledit::parse(&config).unwrap()["tools"]["always_load"],
        json!(crate::codewhale::tool_names())
    );
    // CodeWhale writes a setting of its own (as its /config editor does) and the user adds an
    // MCP server.
    let mut text = config.clone();
    text.push_str("\ndefault_text_model = \"gpt-oss-20b\"\n");
    std::fs::write(&setup.codewhale.config_file, &text).unwrap();
    let mcp = read(&setup.codewhale.mcp_file);
    let (with_git, _) =
        crate::jsonedit::set_member(&mcp, &["servers"], "git", &json!({"command": "mcp-git"}))
            .unwrap()
            .unwrap();
    std::fs::write(&setup.codewhale.mcp_file, with_git).unwrap();
    let (code, out) = run_uninstall(&setup, codewhale_uninstall());
    assert_eq!(code, 0, "{out}");
    let left = read(&setup.codewhale.config_file);
    assert_eq!(
        crate::tomledit::parse(&left).unwrap(),
        json!({"default_text_model": "gpt-oss-20b"}),
        "{left}"
    );
    let mcp = crate::jsonedit::value_of(&read(&setup.codewhale.mcp_file)).unwrap();
    assert_eq!(mcp, json!({"servers": {"git": {"command": "mcp-git"}}}));
    assert!(
        !setup.codewhale.instruction_file.exists(),
        "install created it"
    );
    memory.close();
}

#[test]
fn codewhale_memory_is_switched_off_on_request_and_back_on_at_uninstall() {
    let home = TempDir::new("cw-mem-home");
    let data = TempDir::new("cw-mem-data");
    let dir = home.path().join(".codewhale");
    std::fs::create_dir_all(&dir).unwrap();
    let original = "provider = \"ollama\"\n\n[memory]\n# mine\nenabled = true\n";
    std::fs::write(dir.join("config.toml"), original).unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, codewhale_only());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Nothing is lost"), "{out}");
    let text = read(&setup.codewhale.config_file);
    assert!(text.contains("# mine\nenabled = false\n"), "{text}");
    assert!(
        crate::codewhale::install::status_lines(&setup)
            .iter()
            .any(|l| l.contains("switched off by install")),
    );
    let (code, out) = run_uninstall(&setup, codewhale_uninstall());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("CodeWhale's memory: switched on again"),
        "{out}"
    );
    assert_eq!(read(&setup.codewhale.config_file), original);
    // Kept on when asked to.
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            keep_auto_memory: true,
            ..codewhale_only()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("CodeWhale's memory: on (kept)"), "{out}");
    assert!(read(&setup.codewhale.config_file).contains("# mine\nenabled = true\n"));
    memory.close();
}

#[test]
fn codewhale_legacy_files_are_written_where_codewhale_reads_them() {
    let home = TempDir::new("cw-legacy-home");
    let data = TempDir::new("cw-legacy-data");
    let legacy = home.path().join(".deepseek");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(
        legacy.join("mcp.json"),
        "{\n  \"mcpServers\": {\n    \"git\": {\"command\": \"mcp-git\"}\n  }\n}\n",
    )
    .unwrap();
    let before = snapshot(home.path());
    let setup = setup_in(home.path(), data.path());
    assert_eq!(setup.codewhale.mcp_file, legacy.join("mcp.json"));
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, codewhale_only());
    assert_eq!(code, 0, "{out}");
    assert!(
        !home.path().join(".codewhale").join("mcp.json").exists(),
        "a new file there would hide the legacy one"
    );
    let mcp = crate::jsonedit::value_of(&read(&legacy.join("mcp.json"))).unwrap();
    assert!(mcp["mcpServers"]["graphersal-memory"].is_object(), "{mcp}");
    assert!(mcp.get("servers").is_none(), "{mcp}");
    let (code, out) = run_uninstall(&setup, codewhale_uninstall());
    assert_eq!(code, 0, "{out}");
    assert_eq!(snapshot(home.path()), before, "{out}");
    memory.close();
}

#[test]
fn codewhale_conflicts_are_kept() {
    let home = TempDir::new("cw-conf-home");
    let data = TempDir::new("cw-conf-data");
    let dir = home.path().join(".codewhale");
    std::fs::create_dir_all(&dir).unwrap();
    let both = "{\"servers\": {}, \"mcpServers\": {}}\n";
    std::fs::write(dir.join("mcp.json"), both).unwrap();
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, codewhale_only());
    assert_ne!(code, 0, "{out}");
    assert!(out.contains("both `servers` and `mcpServers`"), "{out}");
    assert_eq!(read(&dir.join("mcp.json")), both);
    // Another program's entry under our name.
    let theirs = "{\"servers\": {\"graphersal-memory\": {\"command\": \"other\"}}}\n";
    std::fs::write(dir.join("mcp.json"), theirs).unwrap();
    let (code, out) = run_install(&setup, &memory, codewhale_only());
    assert_ne!(code, 0, "{out}");
    assert!(out.contains("exists and is not ours"), "{out}");
    // Their entry stays as it is (the other server is still registered, as for Cursor).
    let mcp = crate::jsonedit::value_of(&read(&dir.join("mcp.json"))).unwrap();
    assert_eq!(
        mcp["servers"]["graphersal-memory"],
        json!({"command": "other"})
    );
    memory.close();
}

#[test]
fn codewhale_develop_server_is_switched_by_disabled() {
    let home = TempDir::new("cw-dev-home");
    let data = TempDir::new("cw-dev-data");
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let (code, out) = run_install(
        &setup,
        &memory,
        InstallOptions {
            enable_develop: true,
            ..codewhale_only()
        },
    );
    assert_eq!(code, 0, "{out}");
    let mcp = crate::jsonedit::value_of(&read(&setup.codewhale.mcp_file)).unwrap();
    assert!(mcp["servers"]["graphersal-memory-develop"]["disabled"].is_null());
    assert_eq!(
        crate::codewhale::install::inspect(&setup).develop_server,
        (crate::install::PartState::Installed, true)
    );
    let (code, out) = run_uninstall(
        &setup,
        UninstallOptions {
            disable_develop_only: true,
            ..codewhale_uninstall()
        },
    );
    assert_eq!(code, 0, "{out}");
    let mcp = crate::jsonedit::value_of(&read(&setup.codewhale.mcp_file)).unwrap();
    assert_eq!(
        mcp["servers"]["graphersal-memory-develop"]["disabled"],
        true
    );
    assert!(mcp["servers"]["graphersal-memory"].is_object());
    memory.close();
}

#[test]
fn codewhale_entries_switched_on_by_mcp_enable_are_kept_by_install_and_refresh() {
    let home = TempDir::new("cw-enable-home");
    let data = TempDir::new("cw-enable-data");
    let setup = setup_in(home.path(), data.path());
    let memory = Memory::open(data.path());
    let develop = InstallOptions {
        enable_develop: true,
        ..codewhale_only()
    };
    let (code, out) = run_install(&setup, &memory, develop.clone());
    assert_eq!(code, 0, "{out}");
    // CodeWhale's `/mcp enable` writes `"disabled": false` into the entry it switches on.
    let text = read(&setup.codewhale.mcp_file);
    let (enabled, _) = crate::jsonedit::set_member(
        &text,
        &["servers", "graphersal-memory-develop"],
        "disabled",
        &json!(false),
    )
    .unwrap()
    .unwrap();
    std::fs::write(&setup.codewhale.mcp_file, &enabled).unwrap();
    for options in [develop, refresh()] {
        let (code, out) = run_install(&setup, &memory, options);
        assert_eq!(code, 0, "{out}");
        assert!(
            !out.contains("graphersal-memory-develop (develop profile): update"),
            "{out}"
        );
        assert_eq!(read(&setup.codewhale.mcp_file), enabled, "{out}");
    }
    memory.close();
}

#[test]
fn codewhale_is_detected_by_its_program_and_exported_as_markdown() {
    let home = TempDir::new("cw-det-home");
    let data = TempDir::new("cw-det-data");
    let setup = setup_in(home.path(), data.path());
    assert!(!setup.detected().contains(&Kind::CodeWhale));
    std::fs::create_dir_all(home.path().join(".codewhale")).unwrap();
    assert!(!setup.detected().contains(&Kind::CodeWhale));
    assert!(
        setup
            .leftovers()
            .iter()
            .any(|(kind, _)| *kind == Kind::CodeWhale)
    );
    let setup = with_programs(setup, &["codewhale"]);
    assert!(setup.detected().contains(&Kind::CodeWhale));
    let agent = setup.agent_of(Kind::CodeWhale);
    assert!(!crate::agents::Harness::has_file_memory(agent.as_ref()));
    assert_eq!(
        crate::agents::Harness::global_memory_file(agent.as_ref()),
        home.path()
            .join("graphersal-memory-export")
            .join("codewhale")
            .join("global.md")
    );
    assert!(crate::commands::export_note(agent.as_ref()).contains("AGENTS.md"));
}

#[test]
fn codewhale_native_memory_is_imported_once_and_read_only() {
    let home = TempDir::new("cw-imp-home");
    let data = TempDir::new("cw-imp-data");
    let setup = with_programs(setup_in(home.path(), data.path()), &["codewhale"]);
    crate::codewhale::memory::tests::write_store(
        &setup.codewhale.memory_store,
        &[
            (
                "m-1",
                None,
                "preference",
                "active",
                "Tabs",
                "Use tabs, never spaces.",
            ),
            ("m-2", None, "fact", "candidate", "Pending", "Not reviewed."),
            (
                "m-3",
                Some("0123"),
                "decision",
                "active",
                "DB",
                "SQLite for the cache.",
            ),
        ],
    );
    let store_before = std::fs::read(&setup.codewhale.memory_store).unwrap();
    let memory = Memory::open(data.path());
    let (code, out) = run_install(&setup, &memory, codewhale_only());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("CodeWhale's memory: imported 1 entry (1 new, 0 updated; 0 unchanged)"),
        "{out}"
    );
    assert!(out.contains("1 of unknown repositories left out"), "{out}");
    let all = InProcess(&memory).memories_all();
    assert!(
        all.iter().any(|row| row.slug == "codewhale-tabs-m1"),
        "{all:?}"
    );
    assert_eq!(
        std::fs::read(&setup.codewhale.memory_store).unwrap(),
        store_before,
        "the store is only read"
    );
    // Nothing new the second time.
    let (code, out) = run_install(&setup, &memory, codewhale_only());
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("CodeWhale's memory: imported"), "{out}");
    memory.close();
}

/// Claude Code's user files (`crate::claude_code::fixture`).
pub(crate) fn existing_home(home: &Path) {
    crate::claude_code::fixture::user_files(home);
}

/// opencode's user files (`crate::opencode::fixture`).
pub(crate) fn existing_opencode_home(home: &Path) {
    crate::opencode::fixture::user_files(home);
}

/// Codex's user files (`crate::codex::fixture`).
pub(crate) fn existing_codex_home(home: &Path) {
    crate::codex::fixture::user_files(home);
}

/// Cursor's user files (`crate::cursor::fixture`).
pub(crate) fn existing_cursor_home(home: &Path) {
    crate::cursor::fixture::user_files(home);
}

/// CodeWhale's user files (`crate::codewhale::fixture`).
pub(crate) fn existing_codewhale_home(home: &Path) {
    crate::codewhale::fixture::user_files(home);
}
