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
