//! The MCP side: the two built-in profiles (`use`, `develop`), the project of a connection, an
//! open store served over MCP ([`StoreRuntime`]: the daemon holds one per store), and serving
//! the default store over stdio in this process (`graphersal-memory mcp --no-daemon`).
//!
//! The engine pieces: `graphersal-serve` runs the store's session on its own thread
//! (`spawn_session`, `Context`), `graphersal-mcp` speaks MCP (`Profile`, `Connection`,
//! `serve_lines`). The product adds the profiles, the instructions, the store and the project
//! binding ([`ProjectBinding`]): the memory functions take a `project` parameter, which this
//! layer sets on every call to the connection's project and hides from the tool list, so an
//! agent never passes (or fakes) it.

use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use graphersal::persist::Store;
use graphersal_mcp::{Mcp, McpConfig, Profile};
use graphersal_serve::{Context, Exposure, HostCheck, Runner};
use graphersal_session::{Session, SessionPresets};
use serde_json::{Value, json};

use crate::catalog;
use crate::cli::Profile as ProfileKind;
use crate::{registry, store};

/// The instructions of the `use` server.
pub const USE_INSTRUCTIONS: &str = include_str!("mcp/use.md");

/// The instructions of the `develop` server.
pub const DEVELOP_INSTRUCTIONS: &str = include_str!("mcp/develop.md");

/// The engine's built-in tools the `develop` server adds to the memory functions.
pub const DEVELOP_TOOLS: [&str; 14] = [
    "get_schema",
    "infer_schema",
    "validate_schema_patch",
    "diff_schema",
    "set_schema",
    "patch_schema",
    "list_saved_queries",
    "run_saved_query",
    "gql",
    "query",
    "statistics",
    "list_indexes",
    "create_index",
    "drop_index",
];

/// The longest line the project binding rewrites (the engine's own line limit, 16 MiB); a
/// longer line passes through unchanged and the engine refuses it.
const LINE_LIMIT: usize = 16 * 1024 * 1024;

/// The MCP configuration of a profile (what the agent sees).
pub fn config(kind: ProfileKind) -> Result<McpConfig, String> {
    let value = match kind {
        ProfileKind::Use => json!({
            "server_name": "graphersal-memory",
            "instructions": {"mode": "replace", "text": USE_INSTRUCTIONS},
            "tools": {"include": []},
            "saved_query_tools": {"names": catalog::FUNCTIONS},
            "resources": [],
        }),
        ProfileKind::Develop => json!({
            "server_name": "graphersal-memory-develop",
            "instructions": {"mode": "replace", "text": DEVELOP_INSTRUCTIONS},
            "tools": {"include": DEVELOP_TOOLS},
            "saved_query_tools": {"names": catalog::FUNCTIONS},
            "resources": ["schema-summary", "saved-queries", "gql-reference", "dsl-reference"],
        }),
    };
    McpConfig::parse(&value.to_string(), &mut |_| {
        Err("the built-in profiles read no files".to_string())
    })
    .map_err(|err| format!("the built-in MCP profile is invalid: {err}"))
}

/// The engine profile: the configuration, and for `use` the access policy built from the
/// memory functions' inferred effects (`develop` runs without a policy of its own: everything
/// is allowed).
pub fn profile(
    kind: ProfileKind,
    use_policy: graphersal::auth::AccessPolicy,
) -> Result<Profile, String> {
    let profile = Profile::new(false).with_config(config(kind)?);
    Ok(match kind {
        ProfileKind::Use => profile.with_access_policy(use_policy),
        ProfileKind::Develop => profile,
    })
}

/// The project of a session started in `cwd`: `flag` when given, else the root of the git
/// repository `cwd` is in (the nearest directory with a `.git` entry), else `cwd` itself.
/// Paths are made absolute and canonical where possible.
pub fn project_key(flag: Option<&Path>, cwd: &Path) -> String {
    let start = canonical(flag.unwrap_or(cwd));
    let root = if flag.is_some() {
        start
    } else {
        start
            .ancestors()
            .find(|dir| dir.join(".git").exists())
            .map(Path::to_path_buf)
            .unwrap_or(start)
    };
    root.display().to_string()
}

/// `path` canonicalized (without Windows' verbatim prefix), else as given.
pub(crate) fn canonical(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(path) => {
            let text = path.display().to_string();
            match text.strip_prefix(r"\\?\") {
                Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
                _ => path,
            }
        }
        Err(_) => path.to_path_buf(),
    }
}

/// What to serve.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// The data directory (the default store lives in it).
    pub data_dir: PathBuf,
    /// The profile of the connection.
    pub profile: ProfileKind,
    /// The project key every memory function is bound to.
    pub project: String,
}

/// Serves the default store of `options.data_dir` (created on first use) over newline-delimited
/// JSON-RPC on `read`/`write` until `read` ends; the store is closed before this returns.
pub fn serve(
    options: &ServeOptions,
    read: impl BufRead,
    write: impl Write + Send,
) -> Result<(), String> {
    let runtime = StoreRuntime::open(
        registry::DEFAULT_STORE,
        &store::default_store_dir(&options.data_dir),
        Arc::new(endpoint()),
    )?;
    // Not fatal: the project record only helps a later re-mapping of a moved repository.
    let _ = crate::projects::record(&options.data_dir, &options.project);
    let served = runtime.serve(options.profile, &options.project, None, read, write);
    let closed = runtime.close();
    served.and(closed)
}

/// The MCP endpoint every store's contexts share (its statistics and tool catalog are the
/// process's).
pub fn endpoint() -> Mcp {
    Mcp::new(false, None, false)
}

/// One open store served over MCP: its session thread (`graphersal-serve`'s runtime) and one
/// context per profile over that session, so connections of both profiles share the store's
/// single writer.
pub struct StoreRuntime {
    name: String,
    store: Arc<Store>,
    using: Arc<Context>,
    developing: Arc<Context>,
}

impl StoreRuntime {
    /// Opens (creates on first use) the store `name` in `dir`, applies the product catalog and
    /// starts its session thread; `mcp` is the process's endpoint.
    pub fn open(name: &str, dir: &Path, mcp: Arc<Mcp>) -> Result<Self, String> {
        let store = store::open_or_create(dir)?;
        let served = store.clone();
        let (jobs, use_policy) = graphersal_serve::spawn_session(
            SessionPresets::new(),
            move |session: &mut Session| -> Result<_, String> {
                session
                    .attach_store(served, "memory")
                    .map_err(|err| err.to_string())?;
                catalog::apply(session)?;
                catalog::use_policy(session)
            },
            None,
        )
        .map_err(|err| format!("cannot start the memory store {}: {err}", dir.display()))?;
        let context = |kind: ProfileKind| -> Result<Arc<Context>, String> {
            Ok(Context::builder(
                Runner::Session(jobs.clone()),
                HostCheck::new(Exposure::Loopback, 0, ""),
            )
            .store(Some(store.clone()))
            .shared_mcp(mcp.clone())
            .mcp_profile(profile(kind, use_policy.clone())?)
            .build())
        };
        Ok(Self {
            name: name.to_string(),
            using: context(ProfileKind::Use)?,
            developing: context(ProfileKind::Develop)?,
            store,
        })
    }

    /// The store's name in the registry.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The context of the profile `kind`.
    pub fn context(&self, kind: ProfileKind) -> &Arc<Context> {
        match kind {
            ProfileKind::Use => &self.using,
            ProfileKind::Develop => &self.developing,
        }
    }

    /// Ticks the store's scheduled jobs; their work runs on job worker threads, never on the
    /// session thread (`graphersal_serve::start_job_ticker`).
    pub fn start_jobs(&self) -> Result<(), String> {
        graphersal_serve::start_job_ticker(&self.using, None)
            .map(drop)
            .map_err(|err| format!("cannot start the scheduled jobs of {}: {err}", self.name))
    }

    /// Serves one MCP connection of profile `kind` bound to `project` over newline-delimited
    /// JSON-RPC on `read`/`write`, until `read` ends.
    pub fn serve(
        &self,
        kind: ProfileKind,
        project: &str,
        identity: Option<&str>,
        read: impl BufRead,
        write: impl Write + Send,
    ) -> Result<(), String> {
        let connection = self
            .context(kind)
            .mcp_connection()
            .map_err(|err| err.to_string())?;
        let connection = match identity {
            Some(identity) => connection.with_identity(identity),
            None => connection,
        };
        let binding = Arc::new(ProjectBinding::new(project));
        graphersal_mcp::serve_lines(
            &connection,
            BoundReader::new(read, binding.clone()),
            BoundWriter::new(write, binding),
        )
        .map_err(|err| format!("the MCP stream failed: {}", err.message()))
    }

    /// Runs the catalog function `name` (an internal one: the session indexing) with `args` on
    /// the store's session thread.
    pub fn call(&self, name: &str, args: Value) -> Result<Value, String> {
        let name = name.to_string();
        self.using
            .call(move |session| catalog::call(session, &name, &args))
            .map_err(|err| format!("the memory store {}: {err}", self.name))?
    }

    /// Closes the store: the session lets go of it, then the store is shut down (WAL synced,
    /// marked closed cleanly, its lock released), so a later open finds it unlocked.
    pub fn close(&self) -> Result<(), String> {
        let released = self
            .using
            .call(|session| *session = Session::new())
            .map_err(|err| format!("cannot close the memory store: {err}"));
        let shut = self
            .store
            .shutdown()
            .map_err(|err| format!("cannot close the memory store {}: {err}", self.name));
        released.and(shut)
    }
}

/// Binds the `project` parameter of the memory functions to one project, on the JSON-RPC
/// lines of a connection: a `tools/call` of a memory function (or `run_saved_query` of one) gets
/// `project` set, whatever the client sent; a `tools/list` answer loses `project` from those
/// tools' input schemas.
#[derive(Debug, Clone)]
pub struct ProjectBinding {
    project: String,
}

impl ProjectBinding {
    /// A binding to `project`.
    pub fn new(project: &str) -> Self {
        Self {
            project: project.to_string(),
        }
    }

    fn bound(name: &Value) -> bool {
        name.as_str()
            .is_some_and(|name| catalog::PROJECT_BOUND.contains(&name))
    }

    /// The client's line `line` (with its line end) rewritten, or `None` to pass it unchanged.
    pub fn inbound(&self, line: &[u8]) -> Option<Vec<u8>> {
        let mut message: Value = serde_json::from_slice(line).ok()?;
        if message.get("method")? != "tools/call" {
            return None;
        }
        let params = message.get_mut("params")?.as_object_mut()?;
        let tool = params.get("name")?.clone();
        let arguments = params.entry("arguments").or_insert_with(|| json!({}));
        if arguments.is_null() {
            *arguments = json!({});
        }
        let arguments = arguments.as_object_mut()?;
        let target = if Self::bound(&tool) {
            arguments
        } else if tool == "run_saved_query" && Self::bound(arguments.get("name")?) {
            let params = arguments.entry("params").or_insert_with(|| json!({}));
            if params.is_null() {
                *params = json!({});
            }
            params.as_object_mut()?
        } else {
            return None;
        };
        target.insert(catalog::PROJECT_PARAM.to_string(), json!(self.project));
        let mut text = serde_json::to_vec(&message).ok()?;
        text.push(b'\n');
        Some(text)
    }

    /// The server's line `line` (without its line end) rewritten, or `None` to pass it
    /// unchanged.
    pub fn outbound(&self, line: &str) -> Option<String> {
        if !line.contains("\"inputSchema\"") {
            return None;
        }
        let mut message: Value = serde_json::from_str(line).ok()?;
        let tools = message
            .get_mut("result")?
            .get_mut("tools")?
            .as_array_mut()?;
        for tool in tools.iter_mut() {
            if !Self::bound(&tool["name"]) {
                continue;
            }
            let Some(schema) = tool.get_mut("inputSchema").and_then(Value::as_object_mut) else {
                continue;
            };
            if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
                properties.remove(catalog::PROJECT_PARAM);
            }
            if let Some(required) = schema.get_mut("required").and_then(Value::as_array_mut) {
                required.retain(|name| name != catalog::PROJECT_PARAM);
            }
        }
        serde_json::to_string(&message).ok()
    }
}

/// The client's side of a stream with the [`ProjectBinding`] applied to each line.
pub struct BoundReader<R> {
    inner: R,
    binding: Arc<ProjectBinding>,
    line: Vec<u8>,
    at: usize,
    /// Inside a line longer than [`LINE_LIMIT`]: its rest passes unchanged.
    in_long_line: bool,
}

impl<R: BufRead> BoundReader<R> {
    /// Reads from `inner`, rewriting with `binding`.
    pub fn new(inner: R, binding: Arc<ProjectBinding>) -> Self {
        Self {
            inner,
            binding,
            line: Vec::new(),
            at: 0,
            in_long_line: false,
        }
    }
}

impl<R: BufRead> Read for BoundReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let n = available.len().min(out.len());
        out[..n].copy_from_slice(&available[..n]);
        self.consume(n);
        Ok(n)
    }
}

impl<R: BufRead> BufRead for BoundReader<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.at >= self.line.len() {
            self.line.clear();
            self.at = 0;
            let limit = u64::try_from(LINE_LIMIT)
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            (&mut self.inner)
                .take(limit)
                .read_until(b'\n', &mut self.line)?;
            let ends = self.line.last() == Some(&b'\n');
            // Without a line end the chunk is the input's last line, or a cut at the limit.
            let whole = ends || self.line.len() <= LINE_LIMIT;
            if whole
                && !self.in_long_line
                && let Some(rewritten) = self.binding.inbound(&self.line)
            {
                self.line = rewritten;
            }
            self.in_long_line = !ends && !whole;
        }
        Ok(&self.line[self.at..])
    }

    fn consume(&mut self, amount: usize) {
        self.at = (self.at + amount).min(self.line.len());
    }
}

/// The server's side of a stream with the [`ProjectBinding`] applied to each line.
pub struct BoundWriter<W> {
    inner: W,
    binding: Arc<ProjectBinding>,
    pending: Vec<u8>,
}

impl<W: Write> BoundWriter<W> {
    /// Writes to `inner`, rewriting with `binding`.
    pub fn new(inner: W, binding: Arc<ProjectBinding>) -> Self {
        Self {
            inner,
            binding,
            pending: Vec::new(),
        }
    }
}

impl<W: Write> Write for BoundWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let text = std::str::from_utf8(&line[..end]).ok();
            match text.and_then(|text| self.binding.outbound(text)) {
                Some(rewritten) => {
                    self.inner.write_all(rewritten.as_bytes())?;
                    self.inner.write_all(b"\n")?;
                }
                None => self.inner.write_all(&line)?,
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_profiles_parse() {
        let using = config(ProfileKind::Use).unwrap();
        assert!(using.has_saved_query_tools());
        assert!(
            config(ProfileKind::Develop)
                .unwrap()
                .has_saved_query_tools()
        );
        assert!(
            USE_INSTRUCTIONS.len() < 2000,
            "keep the use instructions short"
        );
    }

    #[test]
    fn a_memory_call_gets_the_project_whatever_the_client_sent() {
        let binding = ProjectBinding::new("/work/a");
        let line = br#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memories","arguments":{"text":"x","project":"/work/b"}}}
"#;
        let out = binding.inbound(line).unwrap();
        assert!(out.ends_with(b"\n"));
        let value: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["params"]["arguments"]["project"], "/work/a");
        assert_eq!(value["params"]["arguments"]["text"], "x");

        let no_arguments =
            br#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"forget"}}"#;
        let value: Value = serde_json::from_slice(&binding.inbound(no_arguments).unwrap()).unwrap();
        assert_eq!(value["params"]["arguments"]["project"], "/work/a");

        let saved = br#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"run_saved_query","arguments":{"name":"recall","params":{"text":"t"}}}}"#;
        let value: Value = serde_json::from_slice(&binding.inbound(saved).unwrap()).unwrap();
        assert_eq!(value["params"]["arguments"]["params"]["project"], "/work/a");
    }

    #[test]
    fn other_lines_pass_unchanged() {
        let binding = ProjectBinding::new("/work/a");
        for line in [
            &br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#[..],
            br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_schema","arguments":{"id":"x"}}}"#,
            br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"gql","arguments":{"query":"MATCH (n) RETURN n"}}}"#,
            br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"run_saved_query","arguments":{"name":"mine"}}}"#,
            b"not json",
        ] {
            assert_eq!(binding.inbound(line), None);
        }
        assert_eq!(
            binding.outbound(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
            None
        );
    }

    #[test]
    fn the_tool_list_hides_the_project_parameter() {
        let binding = ProjectBinding::new("/work/a");
        let line = json!({"jsonrpc": "2.0", "id": 2, "result": {"tools": [
            {"name": "memory", "inputSchema": {"type": "object",
                "properties": {"slug": {"type": "string"}, "project": {"type": "string"}},
                "required": ["slug", "project"]}},
            {"name": "get_schema", "inputSchema": {"type": "object",
                "properties": {"id": {"type": "string"}}, "required": ["id"]}},
        ]}})
        .to_string();
        let out: Value = serde_json::from_str(&binding.outbound(&line).unwrap()).unwrap();
        let memory = &out["result"]["tools"][0]["inputSchema"];
        assert!(memory["properties"].get("project").is_none());
        assert_eq!(memory["required"], json!(["slug"]));
        assert_eq!(
            out["result"]["tools"][1],
            json!({"name": "get_schema", "inputSchema": {
            "type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}})
        );
    }

    #[test]
    fn a_session_read_is_bound_to_the_project_too() {
        let binding = ProjectBinding::new("/work/a");
        let line = br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"session","arguments":{"id":"x","project":"/work/b"}}}"#;
        let value: Value = serde_json::from_slice(&binding.inbound(line).unwrap()).unwrap();
        assert_eq!(value["params"]["arguments"]["project"], "/work/a");
        assert_eq!(value["params"]["arguments"]["id"], "x");
    }

    #[test]
    fn the_reader_rewrites_line_by_line_and_the_writer_too() {
        let binding = Arc::new(ProjectBinding::new("/p"));
        let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"memory\",\"arguments\":{\"slug\":\"a\"}}}\nplain\nlast";
        let mut reader = BoundReader::new(input.as_bytes(), binding.clone());
        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 {
                break;
            }
            lines.push(line);
        }
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].contains("\"project\":\"/p\""), "{}", lines[0]);
        assert_eq!(lines[1], "plain\n");
        assert_eq!(lines[2], "last");

        let mut out = Vec::new();
        {
            let mut writer = BoundWriter::new(&mut out, binding);
            writer.write_all(b"{\"id\":1,").unwrap();
            writer.write_all(b"\"result\":{}}").unwrap();
            writer.write_all(b"\n").unwrap();
            writer.flush().unwrap();
        }
        assert_eq!(out, b"{\"id\":1,\"result\":{}}\n");
    }

    #[test]
    fn the_project_is_the_repository_root() {
        let dir =
            std::env::temp_dir().join(format!("graphersal-memory-project-{}", std::process::id()));
        let nested = dir.join("repo").join("src").join("deep");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(dir.join("repo").join(".git")).unwrap();
        let root = canonical(&dir.join("repo")).display().to_string();
        assert_eq!(project_key(None, &nested), root);
        let flagged = project_key(Some(&nested), &dir);
        assert_eq!(flagged, canonical(&nested).display().to_string());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
