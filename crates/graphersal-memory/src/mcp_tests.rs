//! MCP round trips over in-memory pipes: `mcp::serve` on a thread of its own with a temporary
//! data directory, a test client on the other end. No network, no real home directory.

use std::io::{BufRead, BufReader, PipeReader, PipeWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;

use serde_json::{Value, json};

use crate::catalog;
use crate::cli::Profile as ProfileKind;
use crate::mcp::{self, DEVELOP_TOOLS, ServeOptions};

/// A temporary directory, removed on drop.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!(
            "graphersal-memory-{tag}-{}-{}-{nanos}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A Unix-style fixture path made absolute on this platform: as given on Unix, on a drive on
/// Windows (`/c/x` is `C:\c\x`), where a leading `/` alone is not absolute.
pub(crate) fn abs(unix: &str) -> String {
    if cfg!(windows) {
        format!("C:{}", unix.replace('/', "\\"))
    } else {
        unix.to_string()
    }
}

/// A client of one served connection.
struct Client {
    to: Option<PipeWriter>,
    from: BufReader<PipeReader>,
    server: Option<JoinHandle<Result<(), String>>>,
    next_id: u64,
}

impl Client {
    fn start(data_dir: &Path, profile: ProfileKind, project: &str) -> Self {
        let (server_in, to) = std::io::pipe().unwrap();
        let (from, server_out) = std::io::pipe().unwrap();
        let options = ServeOptions {
            data_dir: data_dir.to_path_buf(),
            profile,
            project: project.to_string(),
        };
        let server =
            std::thread::spawn(move || mcp::serve(&options, BufReader::new(server_in), server_out));
        let mut client = Self {
            to: Some(to),
            from: BufReader::new(from),
            server: Some(server),
            next_id: 1,
        };
        let init = client.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "test", "version": "1"}}),
        );
        assert!(init["result"].is_object(), "{init}");
        client.notify("notifications/initialized");
        client
    }

    fn send(&mut self, message: &Value) {
        let to = self.to.as_mut().unwrap();
        writeln!(to, "{message}").unwrap();
        to.flush().unwrap();
    }

    fn notify(&mut self, method: &str) {
        self.send(&json!({"jsonrpc": "2.0", "method": method}));
    }

    /// One request: its answer (server notifications in between are skipped).
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            assert!(
                self.from.read_line(&mut line).unwrap() > 0,
                "the server closed"
            );
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == json!(id) {
                return message;
            }
        }
    }

    fn tools(&mut self) -> Vec<Value> {
        let answer = self.request("tools/list", json!({}));
        answer["result"]["tools"].as_array().unwrap().clone()
    }

    fn tool_names(&mut self) -> Vec<String> {
        let mut names: Vec<String> = self
            .tools()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect();
        names.sort();
        names
    }

    /// `tools/call`: the result object, or the JSON-RPC error answer.
    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let answer = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        answer.get("result").cloned().unwrap_or(answer)
    }

    /// A tool call that must succeed: its text.
    fn ok(&mut self, name: &str, arguments: Value) -> String {
        let result = self.call(name, arguments);
        assert_eq!(result["isError"], json!(false), "{name}: {result}");
        text(&result).to_string()
    }

    /// Ends the input and waits for the server, which must end cleanly.
    fn close(mut self) {
        drop(self.to.take());
        let served = self.server.take().unwrap().join().unwrap();
        assert_eq!(served, Ok(()));
    }
}

fn text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap_or_default()
}

fn refused(result: &Value) -> bool {
    result["isError"] == json!(true) || result.get("error").is_some()
}

fn sorted(names: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
    names.sort();
    names
}

fn remember(client: &mut Client, slug: &str, summary: &str, scope: &str) -> String {
    client.ok(
        "remember",
        json!({"slug": slug, "kind": "project", "summary": summary,
               "body": format!("{summary}."), "scope": scope}),
    )
}

#[test]
fn the_use_profile_lists_exactly_the_memory_functions_without_the_project() {
    let data = TempDir::new("list");
    let mut client = Client::start(data.path(), ProfileKind::Use, "/work/a");
    assert_eq!(client.tool_names(), sorted(&catalog::FUNCTIONS));
    for tool in client.tools() {
        let schema = &tool["inputSchema"];
        assert!(
            schema["properties"].get("project").is_none(),
            "{}: {schema}",
            tool["name"]
        );
        let description = tool["description"].as_str().unwrap();
        assert!(description.len() < 600, "{}: {description}", tool["name"]);
    }
    client.close();
}

#[test]
fn initialize_answers_the_product_name_and_instructions() {
    let data = TempDir::new("init");
    let (server_in, mut to) = std::io::pipe().unwrap();
    let (from, server_out) = std::io::pipe().unwrap();
    let options = ServeOptions {
        data_dir: data.path().to_path_buf(),
        profile: ProfileKind::Use,
        project: "/work/a".into(),
    };
    let server =
        std::thread::spawn(move || mcp::serve(&options, BufReader::new(server_in), server_out));
    writeln!(
        to,
        "{}",
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "t", "version": "1"}}})
    )
    .unwrap();
    let mut line = String::new();
    BufReader::new(from).read_line(&mut line).unwrap();
    let answer: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(answer["result"]["serverInfo"]["name"], "graphersal-memory");
    let instructions = answer["result"]["instructions"].as_str().unwrap();
    assert!(
        instructions.starts_with("graphersal-memory:"),
        "{instructions}"
    );
    assert!(instructions.contains("remember("), "{instructions}");
    drop(to);
    assert_eq!(server.join().unwrap(), Ok(()));
}

#[test]
fn remember_then_memories_finds_it_memory_reads_it_and_forget_removes_it() {
    let data = TempDir::new("roundtrip");
    let mut client = Client::start(data.path(), ProfileKind::Use, "/work/a");
    let stored = client.ok(
        "remember",
        json!({"slug": "ci-manual", "kind": "project",
               "summary": "CI runs only on manual dispatch",
               "body": "The CI workflow is manual only.", "why": "runner minutes cost money"}),
    );
    assert!(stored.contains("\"created\":true"), "{stored}");
    assert!(stored.contains("\"scope\":\"project\""), "{stored}");

    let listed = client.ok("memories", json!({}));
    assert!(listed.contains("ci-manual"), "{listed}");
    let found = client.ok("memories", json!({"text": "manual CI dispatch", "k": 3}));
    assert!(found.contains("ci-manual"), "{found}");
    assert!(
        found.contains("runner minutes"),
        "the best hits carry their body: {found}"
    );

    let read = client.ok("memory", json!({"slug": "ci-manual"}));
    assert!(
        read.contains("**Why:** runner minutes cost money"),
        "{read}"
    );

    let replaced = client.ok(
        "remember",
        json!({"slug": "ci-manual", "kind": "project", "summary": "CI is manual",
               "body": "Manual only."}),
    );
    assert!(replaced.contains("\"created\":false"), "{replaced}");
    let read = client.ok("memory", json!({"slug": "ci-manual"}));
    assert!(
        !read.contains("runner minutes"),
        "a replaced memory keeps no old why: {read}"
    );

    let forgotten = client.ok("forget", json!({"slug": "ci-manual"}));
    assert!(forgotten.contains("\"removed\":true"), "{forgotten}");
    let listed = client.ok("memories", json!({}));
    assert!(!listed.contains("ci-manual"), "{listed}");
    let read = client.call("memory", json!({"slug": "ci-manual"}));
    assert!(text(&read).contains("no memory ci-manual"), "{read}");
    client.close();
}

#[test]
fn a_project_memory_stays_in_its_project_and_a_global_one_is_seen_everywhere() {
    let data = TempDir::new("scope");
    let mut a = Client::start(data.path(), ProfileKind::Use, "/work/a");
    remember(
        &mut a,
        "a-uses-tabs",
        "Project A indents with tabs",
        "project",
    );
    remember(&mut a, "never-push", "Never push without asking", "global");
    let listed = a.ok("memories", json!({}));
    assert!(
        listed.contains("a-uses-tabs") && listed.contains("never-push"),
        "{listed}"
    );
    a.close();

    // A later session of another project on the same store (the store survives the reopen).
    let mut b = Client::start(data.path(), ProfileKind::Use, "/work/b");
    let listed = b.ok("memories", json!({}));
    assert!(listed.contains("never-push"), "{listed}");
    assert!(!listed.contains("a-uses-tabs"), "{listed}");
    let found = b.ok("memories", json!({"text": "indents with tabs"}));
    assert!(!found.contains("a-uses-tabs"), "{found}");
    let recalled = b.ok("recall", json!({"text": "tabs", "label": "memory"}));
    assert!(!recalled.contains("a-uses-tabs"), "{recalled}");
    let read = b.call("memory", json!({"slug": "a-uses-tabs"}));
    assert!(text(&read).contains("no memory"), "{read}");
    let forgotten = b.ok("forget", json!({"slug": "a-uses-tabs"}));
    assert!(forgotten.contains("\"removed\":false"), "{forgotten}");
    // A project argument from the client is overridden by the connection's project.
    let faked = b.ok("memories", json!({"project": "/work/a"}));
    assert!(!faked.contains("a-uses-tabs"), "{faked}");
    b.close();

    let mut a = Client::start(data.path(), ProfileKind::Use, "/work/a");
    let listed = a.ok("memories", json!({}));
    assert!(
        listed.contains("a-uses-tabs") && listed.contains("never-push"),
        "{listed}"
    );
    a.close();
}

#[test]
fn the_use_profile_refuses_every_tool_but_the_memory_functions() {
    let data = TempDir::new("refuse");
    let mut client = Client::start(data.path(), ProfileKind::Use, "/work/a");
    let names = client.tool_names();
    for tool in [
        "gql",
        "query",
        "run_saved_query",
        "set_schema",
        "list_saved_queries",
    ] {
        assert!(
            !names.iter().any(|name| name == tool),
            "{tool} listed: {names:?}"
        );
    }
    for (tool, arguments) in [
        (
            "gql",
            json!({"query": "INSERT (:memory {name: 'x', kind: 'user', summary: 's'})"}),
        ),
        ("query", json!({"script": "g.add_v(\"memory\").iterate()"})),
        ("run_saved_query", json!({"name": "remember", "params": {}})),
        ("set_schema", json!({"schema": "{}"})),
        ("create_index", json!({"key": "name"})),
    ] {
        let result = client.call(tool, arguments.clone());
        assert!(refused(&result), "{tool} {arguments}: {result}");
    }
    let listed = client.ok("memories", json!({}));
    assert!(
        !listed.contains("\"slug\""),
        "nothing was written: {listed}"
    );
    client.close();

    let mut develop = Client::start(data.path(), ProfileKind::Develop, "/work/a");
    let counted = develop.ok("gql", json!({"query": "MATCH (n) RETURN count(n) AS c"}));
    assert!(counted.contains("\"c\":0"), "{counted}");
    develop.close();
}

/// Every string in `value` that sits in an `enum` list, with its JSON path.
fn empty_enum_values(value: &Value, path: &str, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                if key == "enum" {
                    for item in inner.as_array().into_iter().flatten() {
                        if item.as_str() == Some("") {
                            found.push(format!("{path}.enum"));
                        }
                    }
                }
                empty_enum_values(inner, &format!("{path}.{key}"), found);
            }
        }
        Value::Array(items) => {
            for (i, inner) in items.iter().enumerate() {
                empty_enum_values(inner, &format!("{path}[{i}]"), found);
            }
        }
        _ => {}
    }
}

#[test]
fn no_tool_schema_has_an_empty_enum_value_which_gemini_refuses() {
    // Gemini's API refuses the whole request when a tool parameter's enum holds "" (opencode
    // with Gemini Flash, 2026-11-13: "function_declarations[5].parameters.properties[kind]
    // .enum[0]: cannot be empty").
    let data = TempDir::new("enums");
    for kind in [ProfileKind::Use, ProfileKind::Develop] {
        let mut client = Client::start(data.path(), kind, "/work/a");
        let mut found = Vec::new();
        for tool in client.tools() {
            let name = tool["name"].as_str().unwrap_or_default().to_string();
            empty_enum_values(&tool["inputSchema"], &name, &mut found);
        }
        client.close();
        assert!(found.is_empty(), "empty enum values: {found:?}");
    }
}

#[test]
fn the_develop_profile_adds_the_structure_tools() {
    let data = TempDir::new("develop");
    let mut client = Client::start(data.path(), ProfileKind::Develop, "/work/a");
    let names = client.tool_names();
    let mut wanted: Vec<&str> = catalog::FUNCTIONS.to_vec();
    wanted.extend(DEVELOP_TOOLS);
    assert_eq!(names, sorted(&wanted));
    for tool in client.tools() {
        if catalog::PROJECT_BOUND.contains(&tool["name"].as_str().unwrap()) {
            assert!(
                tool["inputSchema"]["properties"].get("project").is_none(),
                "{tool}"
            );
        }
    }
    remember(
        &mut client,
        "d-note",
        "A note written in develop",
        "project",
    );
    // run_saved_query of a memory function gets the project bound too.
    let listed = client.ok("run_saved_query", json!({"name": "memories", "params": {}}));
    assert!(listed.contains("d-note"), "{listed}");
    let stored = client.ok(
        "gql",
        json!({"query": "MATCH (m:memory) RETURN m.project AS p"}),
    );
    assert!(stored.contains("/work/a"), "{stored}");
    let schema = client.ok("get_schema", json!({}));
    assert!(schema.contains("memory"), "{schema}");
    client.close();
}

#[test]
fn a_session_of_another_project_is_not_readable_by_id_or_prefix() {
    let data = TempDir::new("sessbind");
    let mut develop = Client::start(data.path(), ProfileKind::Develop, "/work/a");
    for (id, project, name) in [
        ("session:aaaa1111", "/work/a", "Session of project a"),
        ("session:aaaa2222", "/work/b", "Session of project b"),
    ] {
        develop.ok(
            "run_saved_query",
            json!({"name": "index_session", "params": {"id": id, "p": {
                "name": name, "project": project, "started_at": "2026-11-01T10:00:00Z"}}}),
        );
    }
    develop.close();

    let mut a = Client::start(data.path(), ProfileKind::Use, "/work/a");
    let own = a.ok("session", json!({"id": "session:aaaa1111"}));
    assert!(own.contains("Session of project a"), "{own}");
    // The other project's session: neither by id, nor by prefix, nor with a faked project.
    for arguments in [
        json!({"id": "session:aaaa2222"}),
        json!({"id": "aaaa2222"}),
        json!({"id": "aaaa2222", "project": "/work/b"}),
    ] {
        let read = a.ok("session", arguments.clone());
        assert!(!read.contains("project b"), "{arguments}: {read}");
        assert!(read.contains("no unique session"), "{arguments}: {read}");
    }
    // A prefix shared by both sessions resolves to the own one.
    let prefixed = a.ok("session", json!({"id": "aaaa"}));
    assert!(prefixed.contains("Session of project a"), "{prefixed}");
    a.close();
}

#[test]
fn the_project_is_bound_by_the_engine_and_never_echoed() {
    let data = TempDir::new("bound");
    let mut a = Client::start(data.path(), ProfileKind::Use, "/work/a");
    remember(&mut a, "a-only", "Only in project a", "project");
    // The answer's call line echoes the client's call without the bound project.
    let read = a.ok("memory", json!({"slug": "a-only"}));
    let call = read.lines().next().unwrap_or_default();
    assert!(call.starts_with("call:"), "{read}");
    assert!(!call.contains("/work/a"), "{read}");
    // A faked project is overridden: the call line does not repeat it either.
    let faked = a.ok("memory", json!({"slug": "a-only", "project": "/work/b"}));
    assert!(faked.contains("Only in project a"), "{faked}");
    assert!(!faked.contains("/work/b"), "{faked}");
    a.close();

    let mut d = Client::start(data.path(), ProfileKind::Develop, "/work/b");
    // run_saved_query with a faked project still runs in the connection's project.
    let listed = d.ok(
        "run_saved_query",
        json!({"name": "memories", "params": {"project": "/work/a"}}),
    );
    assert!(!listed.contains("a-only"), "{listed}");
    d.close();
}

#[test]
fn the_profiles_carry_the_product_title_and_no_write_warning() {
    let data = TempDir::new("title");
    for (kind, title) in [
        (ProfileKind::Use, "graphersal-memory"),
        (ProfileKind::Develop, "graphersal-memory (develop)"),
    ] {
        let mut client = Client::start(data.path(), kind, "/work/a");
        let init = client.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "test", "version": "1"}}),
        );
        assert_eq!(init["result"]["serverInfo"]["title"], title, "{init}");
        for tool in client.tools() {
            if catalog::FUNCTIONS.contains(&tool["name"].as_str().unwrap()) {
                let description = tool["description"].as_str().unwrap();
                assert!(
                    !description.contains("CHANGES DATA"),
                    "{}: {description}",
                    tool["name"]
                );
            }
        }
        client.close();
    }
}

#[test]
fn closing_a_runtime_stops_its_job_ticker_and_releases_the_store() {
    let data = TempDir::new("ticker");
    let dir = crate::store::default_store_dir(data.path());
    for _ in 0..2 {
        let runtime = std::sync::Arc::new(
            mcp::StoreRuntime::open("default", &dir, std::sync::Arc::new(mcp::endpoint())).unwrap(),
        );
        runtime.start_jobs().unwrap();
        assert_eq!(runtime.background_runs(), 0);
        let started = std::time::Instant::now();
        runtime.close().unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        // The next round opens the same store: it was released.
    }
}

// ------------------------------------------------------------------------------------------
// M6b: the memory language in the instructions, project identity across moves and clones.
// ------------------------------------------------------------------------------------------

/// The `instructions` the use server of `data_dir` answers `initialize` with.
fn instructions_of(data_dir: &Path) -> String {
    let (server_in, mut to) = std::io::pipe().unwrap();
    let (from, server_out) = std::io::pipe().unwrap();
    let options = ServeOptions {
        data_dir: data_dir.to_path_buf(),
        profile: ProfileKind::Use,
        project: "/work/a".into(),
    };
    let server =
        std::thread::spawn(move || mcp::serve(&options, BufReader::new(server_in), server_out));
    writeln!(
        to,
        "{}",
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "t", "version": "1"}}})
    )
    .unwrap();
    let mut line = String::new();
    BufReader::new(from).read_line(&mut line).unwrap();
    drop(to);
    assert_eq!(server.join().unwrap(), Ok(()));
    let answer: Value = serde_json::from_str(&line).unwrap();
    answer["result"]["instructions"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn the_instructions_carry_the_memory_language_of_the_settings() {
    let data = TempDir::new("lang");
    let english = instructions_of(data.path());
    assert!(
        english.contains(crate::language::Language::English.rule()),
        "{english}"
    );
    assert!(!english.contains(crate::language::PLACEHOLDER));
    std::fs::write(
        data.path().join("config.json"),
        "{\"version\": 1, \"memory\": {\"language\": \"any\"}}\n",
    )
    .unwrap();
    let any = instructions_of(data.path());
    assert!(any.contains(crate::language::Language::Any.rule()), "{any}");
    assert!(
        !any.contains("Translate a user's words into English"),
        "{any}"
    );
    assert!(any.len() < 2000, "keep the use instructions short");
}

/// A repository at `dir` with the remote `remote` (none when empty).
fn repo_at(dir: &Path, remote: &str) -> String {
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    let config = if remote.is_empty() {
        "[core]\n\tbare = false\n".to_string()
    } else {
        format!("[remote \"origin\"]\n\turl = {remote}\n")
    };
    std::fs::write(dir.join(".git").join("config"), config).unwrap();
    mcp::canonical(dir).display().to_string()
}

fn slugs(client: &mut Client) -> String {
    client.ok("memories", json!({}))
}

#[test]
fn a_moved_repository_with_the_same_remote_keeps_its_memories() {
    let data = TempDir::new("move-data");
    let work = TempDir::new("move-work");
    let old = repo_at(&work.path().join("old"), "git@example.org:me/app.git");
    let mut client = Client::start(data.path(), ProfileKind::Use, &old);
    remember(
        &mut client,
        "deploy-on-friday",
        "We deploy on Fridays",
        "project",
    );
    client.close();
    std::fs::rename(work.path().join("old"), work.path().join("new")).unwrap();
    let new = mcp::canonical(&work.path().join("new"))
        .display()
        .to_string();
    let mut client = Client::start(data.path(), ProfileKind::Use, &new);
    assert!(slugs(&mut client).contains("deploy-on-friday"));
    // Remembering from the new place replaces the same memory (same project id).
    let again = remember(
        &mut client,
        "deploy-on-friday",
        "We deploy on Mondays",
        "project",
    );
    assert!(again.contains("\"created\":false"), "{again}");
    client.close();
}

#[test]
fn a_local_repository_that_gets_a_remote_keeps_its_memories_and_the_remote_is_recorded() {
    let data = TempDir::new("local-data");
    let work = TempDir::new("local-work");
    let path = repo_at(&work.path().join("app"), "");
    let mut client = Client::start(data.path(), ProfileKind::Use, &path);
    remember(
        &mut client,
        "local-rule",
        "A rule of the local repository",
        "project",
    );
    client.close();
    repo_at(
        &work.path().join("app"),
        "https://me@example.org/me/app.git",
    );
    let mut client = Client::start(data.path(), ProfileKind::Use, &path);
    assert!(slugs(&mut client).contains("local-rule"));
    client.close();
    let projects = crate::projects::Projects::load(data.path()).unwrap();
    assert_eq!(
        projects.get(&path).unwrap().remotes,
        vec!["example.org/me/app".to_string()]
    );
    // And now the remote finds it anywhere.
    let clone = repo_at(&work.path().join("clone"), "git@example.org:me/app");
    let mut client = Client::start(data.path(), ProfileKind::Use, &clone);
    assert!(slugs(&mut client).contains("local-rule"));
    client.close();
}

#[test]
fn two_clones_of_one_remote_share_their_memories() {
    let data = TempDir::new("clones-data");
    let work = TempDir::new("clones-work");
    let a = repo_at(&work.path().join("a"), "https://example.org/team/app.git");
    let b = repo_at(&work.path().join("b"), "https://example.org/team/app");
    let mut client = Client::start(data.path(), ProfileKind::Use, &a);
    remember(
        &mut client,
        "shared-rule",
        "Both clones see this",
        "project",
    );
    client.close();
    let mut client = Client::start(data.path(), ProfileKind::Use, &b);
    assert!(slugs(&mut client).contains("shared-rule"));
    client.close();
    // A different repository does not.
    let other = repo_at(&work.path().join("c"), "https://example.org/team/other.git");
    let mut client = Client::start(data.path(), ProfileKind::Use, &other);
    assert!(!slugs(&mut client).contains("shared-rule"));
    client.close();
}

#[test]
fn moved_with_a_new_remote_is_suggested_as_a_merge_and_the_merge_keeps_the_memories() {
    let data = TempDir::new("alike-data");
    let work = TempDir::new("alike-work");
    let old = repo_at(&work.path().join("old"), "https://a.example/app.git");
    let mut client = Client::start(data.path(), ProfileKind::Use, &old);
    remember(&mut client, "old-rule", "From before the move", "project");
    client.close();
    // The same first commit, recorded as git would give it.
    let set_root = |id: &str| {
        let file = data.path().join(crate::projects::PROJECTS_FILE);
        let mut value: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        value["projects"][id]["root_commit"] = json!("ab".repeat(20));
        std::fs::write(&file, value.to_string()).unwrap();
    };
    set_root(&old);
    std::fs::rename(work.path().join("old"), work.path().join("new")).unwrap();
    let new = repo_at(&work.path().join("new"), "https://b.example/app.git");
    let mut client = Client::start(data.path(), ProfileKind::Use, &new);
    assert!(
        !slugs(&mut client).contains("old-rule"),
        "never merged automatically"
    );
    client.close();
    set_root(&new);
    let hints = crate::projects::look_alike_lines(data.path());
    assert_eq!(hints.len(), 1, "{hints:?}");
    assert!(hints[0].0.contains("looks like project"), "{hints:?}");
    assert!(
        hints[0].1.contains("graphersal-memory projects merge"),
        "{hints:?}"
    );

    // The merge (as the daemon runs it): the memories move, the old keys follow.
    let runtime = mcp::StoreRuntime::open(
        crate::registry::DEFAULT_STORE,
        &crate::store::default_store_dir(data.path()),
        std::sync::Arc::new(mcp::endpoint()),
    )
    .unwrap();
    let moved = runtime
        .call("merge_project", json!({"from": old, "into": new}))
        .unwrap();
    assert_eq!(moved["memories_moved"], 1, "{moved}");
    runtime.close().unwrap();
    crate::projects::merge_records(data.path(), &old, &new).unwrap();
    let mut client = Client::start(data.path(), ProfileKind::Use, &new);
    assert!(slugs(&mut client).contains("old-rule"));
    client.close();
    assert!(crate::projects::look_alike_lines(data.path()).is_empty());
}

// ------------------------------------------------------------------------------------------
// M14: pinning, the start context and the instructions of a connection.
// ------------------------------------------------------------------------------------------

/// The JSON of a tool's text answer (the whole text, or its last line that is JSON).
fn json_of(text: &str) -> Value {
    serde_json::from_str(text)
        .ok()
        .or_else(|| {
            text.lines()
                .rev()
                .find_map(|line| serde_json::from_str(line.trim()).ok())
        })
        .unwrap_or_else(|| panic!("no JSON in {text}"))
}

/// Remembers `slug` of `kind` in `scope`, with extra arguments merged in.
fn remember_kind(client: &mut Client, slug: &str, kind: &str, scope: &str, extra: Value) {
    let mut args = json!({"slug": slug, "kind": kind, "summary": format!("summary of {slug}"),
                          "body": format!("Body of {slug}."), "scope": scope});
    for (key, value) in extra.as_object().unwrap() {
        args[key] = value.clone();
    }
    client.ok("remember", args);
}

/// The memories of the start context fixture: feedback, user, project and reference memories
/// of /work/a, one global feedback memory, one project memory of /work/b.
fn start_context_fixture(data: &Path) {
    let mut a = Client::start(data, ProfileKind::Use, "/work/a");
    remember_kind(&mut a, "feedback-old", "feedback", "project", json!({}));
    remember_kind(
        &mut a,
        "feedback-new",
        "feedback",
        "project",
        json!({"title": "Newest rule", "hook": "the hook line"}),
    );
    remember_kind(&mut a, "about-me", "user", "project", json!({}));
    remember_kind(
        &mut a,
        "semver",
        "project",
        "project",
        json!({"pinned": true}),
    );
    remember_kind(&mut a, "where-logs", "reference", "project", json!({}));
    remember_kind(&mut a, "plain-fact", "project", "project", json!({}));
    remember_kind(&mut a, "never-push", "feedback", "global", json!({}));
    a.close();
    let mut b = Client::start(data, ProfileKind::Use, "/work/b");
    remember_kind(&mut b, "b-rule", "feedback", "project", json!({}));
    b.close();
}

/// The start context of `project` in the store of `data` within `budget`.
fn start_context_of(
    data: &Path,
    project: &str,
    budget: usize,
) -> crate::start_context::StartContext {
    let runtime = mcp::StoreRuntime::open(
        "default",
        &crate::store::default_store_dir(data),
        std::sync::Arc::new(mcp::endpoint()),
    )
    .unwrap();
    let id = crate::projects::resolve(data, project).unwrap().id;
    let context = runtime.start_context(&id, budget).unwrap();
    runtime.close().unwrap();
    context
}

#[test]
fn pin_and_remember_set_pinned_and_a_replace_keeps_it() {
    let data = TempDir::new("pin");
    let mut client = Client::start(data.path(), ProfileKind::Use, "/work/a");
    remember_kind(&mut client, "ci-manual", "project", "project", json!({}));
    let pinned = client.ok("pin", json!({"slug": "ci-manual", "order": 2}));
    assert!(
        pinned.contains("\"pinned\":true") && pinned.contains("\"order\":2"),
        "{pinned}"
    );
    // A replace without `pinned` keeps the pin and the order.
    remember_kind(&mut client, "ci-manual", "project", "project", json!({}));
    let listed: Value = json_of(&client.ok("memories", json!({})));
    assert_eq!(listed[0]["slug"], "ci-manual");
    assert_eq!(listed[0]["pinned"], true, "{listed}");
    // remember(pinned: false) and pin(pinned: false) unpin.
    remember_kind(
        &mut client,
        "ci-manual",
        "project",
        "project",
        json!({"pinned": false}),
    );
    let listed: Value = json_of(&client.ok("memories", json!({})));
    assert_eq!(listed[0]["pinned"], false, "{listed}");
    client.ok("pin", json!({"slug": "ci-manual"}));
    let off = client.ok("pin", json!({"slug": "ci-manual", "pinned": false}));
    assert!(off.contains("\"pinned\":false"), "{off}");
    let missing = client.call("pin", json!({"slug": "nope"}));
    assert!(text(&missing).contains("no memory nope"), "{missing}");
    client.close();
}

#[test]
fn memories_lists_pinned_then_feedback_then_user_then_the_rest() {
    let data = TempDir::new("order");
    start_context_fixture(data.path());
    let mut client = Client::start(data.path(), ProfileKind::Use, "/work/a");
    let listed: Value = json_of(&client.ok("memories", json!({})));
    let slugs: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["slug"].as_str().unwrap())
        .collect();
    assert_eq!(
        slugs,
        [
            "semver",
            "never-push",
            "feedback-new",
            "feedback-old",
            "about-me",
            "where-logs",
            "plain-fact"
        ]
    );
    client.close();
}

#[test]
fn the_start_context_holds_pinned_feedback_and_user_memories_of_the_project_and_global() {
    let data = TempDir::new("start");
    start_context_fixture(data.path());
    let context = start_context_of(data.path(), "/work/a", crate::start_context::DEFAULT_CHARS);
    assert_eq!((context.total, context.shown), (5, 5), "{}", context.text);
    let text = &context.text;
    for slug in [
        "semver",
        "never-push",
        "feedback-new",
        "feedback-old",
        "about-me",
    ] {
        assert!(
            text.contains(&format!("(memory: {slug})")),
            "{slug}: {text}"
        );
    }
    for slug in ["where-logs", "plain-fact", "b-rule"] {
        assert!(
            !text.contains(&format!("(memory: {slug})")),
            "{slug}: {text}"
        );
    }
    assert!(
        text.contains("- Newest rule — the hook line (memory: feedback-new)"),
        "{text}"
    );
    assert!(
        text.contains("- never-push — summary of never-push (memory: never-push)"),
        "{text}"
    );
    let how = text.find("## How to work").unwrap();
    let about = text.find("## About you").unwrap();
    let pinned = text.find("## Pinned").unwrap();
    assert!(how < about && about < pinned, "{text}");
    // Another project: its own rule and the global one.
    let b = start_context_of(data.path(), "/work/b", crate::start_context::DEFAULT_CHARS);
    assert_eq!(b.total, 2, "{}", b.text);
    assert!(b.text.contains("(memory: b-rule)") && b.text.contains("(memory: never-push)"));
    // Over the budget: pinned first, the rest named.
    let cut = start_context_of(data.path(), "/work/a", 520);
    assert!(cut.chars() <= 520, "{}", cut.text);
    assert!(cut.shown >= 1 && cut.shown < 5, "{}", cut.text);
    assert!(cut.text.contains("(memory: semver)"), "{}", cut.text);
    assert!(
        cut.text
            .contains(&format!("{} more not shown: memories(kind:", 5 - cut.shown)),
        "{}",
        cut.text
    );
}

/// The `instructions` of a `use` connection to the store of `data` in mode `start`.
fn instructions_in_mode(data: &Path, start: crate::protocol::StartContextMode) -> String {
    let runtime = mcp::StoreRuntime::open(
        "default",
        &crate::store::default_store_dir(data),
        std::sync::Arc::new(mcp::endpoint()),
    )
    .unwrap();
    let id = crate::projects::resolve(data, "/work/a").unwrap().id;
    let (server_in, mut to) = std::io::pipe().unwrap();
    let (from, server_out) = std::io::pipe().unwrap();
    let connection = mcp::Connection {
        language: crate::language::Language::default(),
        start,
        start_chars: crate::start_context::DEFAULT_CHARS,
    };
    let instructions = std::thread::scope(|scope| {
        let server = scope.spawn(|| {
            runtime.serve_with(
                ProfileKind::Use,
                &id,
                None,
                connection,
                BufReader::new(server_in),
                server_out,
            )
        });
        writeln!(
            to,
            "{}",
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "t", "version": "1"}}})
        )
        .unwrap();
        let mut line = String::new();
        BufReader::new(from).read_line(&mut line).unwrap();
        drop(to);
        assert_eq!(server.join().unwrap(), Ok(()));
        let answer: Value = serde_json::from_str(&line).unwrap();
        answer["result"]["instructions"]
            .as_str()
            .unwrap()
            .to_string()
    });
    runtime.close().unwrap();
    instructions
}

#[test]
fn the_instructions_of_a_connection_carry_its_start_context_once() {
    use crate::protocol::StartContextMode;
    let data = TempDir::new("instr");
    // No always-loaded memory yet: the instructions stay as they were.
    let empty = instructions_of(data.path());
    assert!(!empty.contains("## How to work"), "{empty}");
    start_context_fixture(data.path());
    let full = instructions_of(data.path());
    assert!(full.starts_with("graphersal-memory:"), "{full}");
    assert!(
        full.contains("## How to work") && full.contains("(memory: semver)"),
        "{full}"
    );
    assert_eq!(
        instructions_in_mode(data.path(), StartContextMode::Full),
        full
    );
    // Claude Code with our hook: the hook delivers it, the instructions leave it out.
    let none = instructions_in_mode(data.path(), StartContextMode::None);
    assert_eq!(none, empty);
    // Claude Code without the hook: within its limit for server instructions.
    let short = instructions_in_mode(data.path(), StartContextMode::Short);
    assert!(
        short.chars().count() <= crate::protocol::CLAUDE_CODE_INSTRUCTIONS_CHARS,
        "{}",
        short.len()
    );
    assert!(short.contains("(memory: semver)"), "{short}");
}

#[test]
fn the_start_context_mode_of_a_harness_avoids_double_delivery() {
    use crate::protocol::StartContextMode;
    assert_eq!(
        crate::commands::start_context_mode("claude-code", true),
        StartContextMode::None
    );
    assert_eq!(
        crate::commands::start_context_mode("claude-code", false),
        StartContextMode::Short
    );
    assert_eq!(
        crate::commands::start_context_mode("unknown", false),
        StartContextMode::Full
    );
    assert_eq!(
        crate::commands::start_context_mode("unknown", true),
        StartContextMode::Full
    );
}

#[test]
fn remember_answers_nearest_and_check_and_conflicts_settle_round_trip_in_both_profiles() {
    let data = TempDir::new("conflicts");
    for profile in [ProfileKind::Use, ProfileKind::Develop] {
        let project = if profile == ProfileKind::Use {
            "/work/use"
        } else {
            "/work/develop"
        };
        let mut client = Client::start(data.path(), profile, project);
        let names = client.tool_names();
        assert!(
            names.contains(&"conflicts".to_string()) && names.contains(&"settle".to_string()),
            "{names:?}"
        );
        let first = json_of(&client.ok(
            "remember",
            json!({"slug": "tabs", "kind": "project", "summary": "Indent the code with tabs",
                   "body": "Tabs."}),
        ));
        assert!(first.get("check").is_none(), "{first}");
        let second = json_of(&client.ok(
            "remember",
            json!({"slug": "spaces", "kind": "project",
                   "summary": "Indent the code with four spaces", "body": "Spaces."}),
        ));
        assert_eq!(second["nearest"][0]["slug"], "tabs", "{second}");
        assert!(
            second["check"]
                .as_str()
                .unwrap()
                .contains("settle(\"spaces\", \"conflict\""),
            "{second}"
        );
        let opened = json_of(&client.ok(
            "settle",
            json!({"slug": "spaces", "decision": "conflict", "other": "tabs"}),
        ));
        assert_eq!(opened["open_conflicts"], 1, "{opened}");
        let listed = json_of(&client.ok("conflicts", json!({})));
        assert_eq!(listed["open"][0]["a"]["slug"], "spaces", "{listed}");
        let closed = json_of(&client.ok(
            "settle",
            json!({"slug": "spaces", "decision": "supersedes", "other": "tabs"}),
        ));
        assert_eq!(closed["open_conflicts"], 0, "{closed}");
        let read = json_of(&client.ok("memory", json!({"slug": "tabs"})));
        assert!(
            read["summary"]
                .as_str()
                .unwrap()
                .starts_with("SUPERSEDED by spaces on "),
            "{read}"
        );
        assert_eq!(read["chain"], json!(["tabs", "spaces"]));
        client.close();
    }
}

#[test]
fn the_instructions_say_that_remember_returns_nearest_and_check() {
    let text = mcp::use_instructions(crate::language::Language::default());
    for word in ["nearest", "check", "settle(", "SUPERSEDED", "conflicts()"] {
        assert!(text.contains(word), "{word}: {text}");
    }
    assert!(mcp::DEVELOP_INSTRUCTIONS.contains("conflicts_with"));
}

#[test]
fn the_start_context_leaves_out_superseded_memories_and_shows_open_conflicts() {
    let data = TempDir::new("startconf");
    let mut a = Client::start(data.path(), ProfileKind::Use, "/work/a");
    remember_kind(&mut a, "use-npm", "feedback", "project", json!({}));
    remember_kind(
        &mut a,
        "use-pnpm",
        "feedback",
        "project",
        json!({"supersedes": ["use-npm"]}),
    );
    remember_kind(&mut a, "tabs", "feedback", "project", json!({}));
    remember_kind(&mut a, "spaces", "feedback", "project", json!({}));
    a.ok(
        "settle",
        json!({"slug": "spaces", "decision": "conflict", "other": "tabs"}),
    );
    a.close();
    let context = start_context_of(data.path(), "/work/a", crate::start_context::DEFAULT_CHARS);
    assert!(
        !context.text.contains("(memory: use-npm)"),
        "{}",
        context.text
    );
    assert!(
        context.text.contains("(memory: use-pnpm)"),
        "{}",
        context.text
    );
    assert!(
        context.text.contains("## Open conflicts"),
        "{}",
        context.text
    );
    assert!(
        context
            .text
            .contains("(memory: spaces, always loaded)\n  CONFLICTS WITH "),
        "{}",
        context.text
    );
    assert!(context.text.contains("conflicts()"), "{}", context.text);
    assert_eq!(
        context.conflicts,
        ["spaces (remember) vs tabs (remember)"],
        "{context:?}"
    );
    assert!(context.start_note().contains("1 open memory conflict"));
    let lines = context.conflict_lines();
    assert!(
        lines[0].starts_with("memory conflicts: 1 open"),
        "{lines:?}"
    );
    // The daemon's answer carries them to status and doctor.
    let round = crate::start_context::StartContext::from_json(&context.to_json()).unwrap();
    assert_eq!(round.conflicts, context.conflicts);
}

/// One request to a store's relay context of the browser UI on port 4100 (`crate::relay`):
/// its status and JSON body.
fn relay(
    runtime: &mcp::StoreRuntime,
    method: &str,
    target: &str,
    host: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let bytes = body.map(|b| b.to_string()).unwrap_or_default().into_bytes();
    let mut request = graphersal_serve::Request::new(
        graphersal_serve::Method::parse(method),
        target,
        bytes.as_slice(),
    )
    .with_body_length(Some(bytes.len()))
    .with_header("Host", host);
    if !bytes.is_empty() {
        request = request.with_header("Content-Type", "application/json");
    }
    let response = runtime.relay(4100, request).unwrap();
    let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (response.status(), body)
}

/// `POST /api/query` of the relay: the status and the answer.
fn ui_query(runtime: &mcp::StoreRuntime, name: &str, params: Value) -> (u16, Value) {
    relay(
        runtime,
        "POST",
        "/api/query",
        "127.0.0.1:4100",
        Some(json!({"name": name, "params": params})),
    )
}

fn open_runtime(data: &Path) -> mcp::StoreRuntime {
    mcp::StoreRuntime::open(
        "default",
        &crate::store::default_store_dir(data),
        std::sync::Arc::new(mcp::endpoint()),
    )
    .unwrap()
}

#[test]
fn the_relay_answers_only_the_memory_and_ui_functions_behind_the_host_check() {
    let data = TempDir::new("relay");
    let runtime = open_runtime(data.path());
    let (status, info) = relay(&runtime, "GET", "/api/info", "127.0.0.1:4100", None);
    assert_eq!(status, 200, "{info}");
    assert_eq!(info["apiMode"], "allow-list", "{info}");
    assert_eq!(info["uiProfile"]["name"], "memory", "{info}");
    assert_eq!(
        info["uiProfile"]["version"],
        env!("CARGO_PKG_VERSION"),
        "{info}"
    );
    let (status, _) = relay(&runtime, "GET", "/api/changes", "127.0.0.1:4100", None);
    assert_eq!(status, 200);
    for (method, target) in [
        ("GET", "/api/graph"),
        ("POST", "/api/execute"),
        ("GET", "/api/schema"),
    ] {
        let (status, body) = relay(&runtime, method, target, "127.0.0.1:4100", None);
        assert_eq!(status, 403, "{method} {target}: {body}");
    }
    // Another port's or host's request is refused before anything runs (the engine's loopback
    // rules: `localhost` on the same port is the same machine).
    let (status, _) = relay(&runtime, "GET", "/api/info", "localhost:4100", None);
    assert_eq!(status, 200);
    for host in [
        "127.0.0.1:4101",
        "evil.example:4100",
        "localhost.evil.example:4100",
    ] {
        let (status, body) = relay(&runtime, "GET", "/api/info", host, None);
        assert_eq!(status, 403, "{host}: {body}");
    }
    for name in crate::catalog::INTERNAL_FUNCTIONS {
        let (status, body) = ui_query(&runtime, name, json!({}));
        assert_eq!(status, 403, "{name}: {body}");
    }
    let (status, body) = ui_query(&runtime, "no_such_function", json!({}));
    assert_eq!(status, 403, "{body}");
    let (status, body) = ui_query(&runtime, "overview", json!({"project": ""}));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], true, "{body}");
    assert_eq!(body["results"][0]["memories"], 0, "{body}");
    runtime.close().unwrap();
}

#[test]
fn the_ui_writes_as_the_ui_and_an_agent_never_can() {
    let data = TempDir::new("source");
    let mut agent = Client::start(data.path(), ProfileKind::Use, "/work/a");
    for tool in agent.tools() {
        assert!(
            tool["inputSchema"]["properties"].get("source").is_none(),
            "{}: the source is bound, not offered",
            tool["name"]
        );
    }
    // An agent claiming to be the UI is recorded as the agent.
    agent.ok(
        "remember",
        json!({"slug": "by-agent", "kind": "project", "summary": "s", "body": "b", "source": "ui"}),
    );
    let read = json_of(&agent.ok("memory", json!({"slug": "by-agent"})));
    assert_eq!(read["source"], "remember", "{read}");
    agent.ok(
        "remember",
        json!({"slug": "imported", "kind": "project", "summary": "from a file", "body": "b",
               "from_file": {"path": "/f/imported.md"}}),
    );
    agent.close();
    let project = crate::projects::resolve(data.path(), "/work/a").unwrap().id;
    let runtime = open_runtime(data.path());
    let (status, saved) = ui_query(
        &runtime,
        "remember",
        json!({"slug": "by-ui", "kind": "feedback", "summary": "s", "body": "b",
               "source": "ui", "project": project}),
    );
    assert_eq!(status, 200, "{saved}");
    let (_, read) = ui_query(
        &runtime,
        "memory",
        json!({"slug": "by-ui", "project": project}),
    );
    assert_eq!(read["results"][0]["source"], "ui", "{read}");
    // An imported memory: no edit, no forget from the UI; pinning works.
    let (_, refused) = ui_query(
        &runtime,
        "remember",
        json!({"slug": "imported", "kind": "project", "summary": "x", "body": "x",
               "source": "ui", "project": project}),
    );
    assert!(
        refused.to_string().contains("read-only in the UI"),
        "{refused}"
    );
    let (_, refused) = ui_query(
        &runtime,
        "forget",
        json!({"slug": "imported", "source": "ui", "project": project}),
    );
    assert!(
        refused.to_string().contains("edit or delete that file"),
        "{refused}"
    );
    let (_, pinned) = ui_query(
        &runtime,
        "pin",
        json!({"slug": "imported", "pinned": true, "project": project}),
    );
    assert_eq!(pinned["results"][0]["pinned"], true, "{pinned}");
    let (_, read) = ui_query(
        &runtime,
        "memory",
        json!({"slug": "imported", "project": project}),
    );
    assert_eq!(read["results"][0]["summary"], "from a file", "{read}");
    runtime.close().unwrap();
}

#[test]
fn memories_against_instruction_files_reach_conflicts_settle_and_the_start_context() {
    let data = TempDir::new("ic-mcp");
    let files = TempDir::new("ic-mcp-files");
    let claude = files.path().join("CLAUDE.md");
    std::fs::write(
        &claude,
        "# Rules\n\n- Always use npm to install dependencies, never pnpm.\n\
         - Write commit messages in the imperative mood.\n",
    )
    .unwrap();
    let runtime = mcp::StoreRuntime::open(
        "default",
        &crate::store::default_store_dir(data.path()),
        std::sync::Arc::new(mcp::endpoint()),
    )
    .unwrap();
    // The check needs an embedding model; on with the built-in one for the test.
    runtime.enable_instruction_check();
    let id = crate::projects::resolve(data.path(), "/work/a").unwrap().id;
    let (rules, notes) = crate::instructions::read_rules(
        &[
            json!({"path": claude.display().to_string(), "global": false,
                 "loaded_by": ["claude-code"]}),
        ],
        &files.path().display().to_string(),
        &id,
    );
    assert!(notes.is_empty(), "{notes:?}");
    let synced = runtime
        .call("sync_instructions", json!({"project": id, "rules": rules}))
        .unwrap();
    assert_eq!(synced["created"], 2, "{synced}");
    runtime.close().unwrap();

    let mut client = Client::start(data.path(), ProfileKind::Use, "/work/a");
    remember_kind(&mut client, "dark-theme", "user", "global", json!({}));
    let answer: Value = json_of(&client.ok(
        "remember",
        json!({"slug": "use-pnpm", "kind": "feedback", "summary": "Install dependencies with pnpm, not npm",
               "body": "Install dependencies with pnpm, not npm."}),
    ));
    let rule = answer["instruction_lines"][0]["rule"]
        .as_str()
        .unwrap_or_else(|| panic!("{answer}"))
        .to_string();
    assert_eq!(answer["instruction_lines"][0]["line"], 3, "{answer}");
    let listed: Value = json_of(&client.ok("conflicts", json!({})));
    assert_eq!(
        listed["rules_unchecked"][0]["memory"]["slug"], "use-pnpm",
        "{listed}"
    );
    let settled: Value = json_of(&client.ok(
        "settle",
        json!({"slug": "use-pnpm", "decision": "conflict", "rule": rule}),
    ));
    assert_eq!(settled["rule_conflicts"], 1, "{settled}");
    client.close();

    let context = start_context_of(data.path(), "/work/a", crate::start_context::DEFAULT_CHARS);
    assert!(
        context.text.contains(&format!(
            "(memory: use-pnpm, always loaded)\n  CONTRADICTS {} line 3: Always use npm",
            claude.display()
        )),
        "{}",
        context.text
    );
    assert_eq!(
        context.rule_conflicts,
        [format!("use-pnpm (remember) vs {}:3", claude.display())]
    );
    assert!(
        context
            .start_note()
            .contains("1 memory contradicts your instruction files"),
        "{}",
        context.start_note()
    );
    // status and doctor print these lines (from the daemon's answer).
    let round = crate::start_context::StartContext::from_json(&context.to_json()).unwrap();
    assert!(
        round.conflict_lines()[0].contains("memory vs instruction files: 1 conflict"),
        "{:?}",
        round.conflict_lines()
    );
}
