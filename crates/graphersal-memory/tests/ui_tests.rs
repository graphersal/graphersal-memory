//! `graphersal-memory ui` end to end: the real binary as the UI server (`ui --no-browser --port
//! 0`) and the daemon it starts, each test with its own temporary data, run and home
//! directories (never the user's, never a running daemon of the user). Requests are plain HTTP
//! over a std `TcpStream` (one request per connection, `Connection: close`), so every header
//! (`Host`, `Origin`, `Authorization`) is exactly what the test writes. Every `ui` and daemon a
//! test starts is stopped (and killed if it does not stop) when the test ends.

// Test helpers outside `#[test]` functions: clippy's allow-unwrap-in-tests does not reach them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

mod support;
use support::{UnixStream, alive, kill};

const BIN: &str = env!("CARGO_BIN_EXE_graphersal-memory");
/// How long a test waits for an answer before it fails (the machine may be busy).
const PATIENCE: Duration = Duration::from_secs(90);

/// A test's directories, its `ui` process and the daemon it started.
struct Env {
    root: PathBuf,
    ui: Option<Child>,
    port: u16,
    token: String,
}

impl Env {
    /// Starts `ui` (daemon idle time `idle`, extra `ui` arguments `extra`).
    fn start(idle: &str, extra: &[&str]) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gmu-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut command = Command::new(BIN);
        command
            .env("GRAPHERSAL_MEMORY_DIR", root.join("d"))
            .env("GRAPHERSAL_MEMORY_RUN_DIR", root.join("r"))
            .env("GRAPHERSAL_MEMORY_IDLE", idle)
            .env("HOME", &root)
            .env("USERPROFILE", &root)
            .env("LOCALAPPDATA", &root)
            .env_remove("XDG_RUNTIME_DIR")
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .current_dir(&root)
            .args(["ui", "--no-browser", "--port", "0"])
            .args(extra)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (lines, from) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if lines.send(line).is_err() {
                    break;
                }
            }
        });
        let first = from.recv_timeout(PATIENCE).unwrap_or_else(|_| {
            let mut err = String::new();
            let _ = child.stderr.take().unwrap().read_to_string(&mut err);
            panic!("ui printed no address: {err}");
        });
        let url = first
            .split_whitespace()
            .find(|word| word.starts_with("http://127.0.0.1:"))
            .unwrap_or_else(|| panic!("no address in {first}"));
        let rest = url.trim_start_matches("http://127.0.0.1:");
        let (port, after) = rest.split_once('/').unwrap();
        let token = after.trim_start_matches("#token=").to_string();
        assert_eq!(token.len(), 64, "{first}");
        Self {
            root,
            ui: Some(child),
            port: port.parse().unwrap(),
            token,
        }
    }

    fn run_dir(&self) -> PathBuf {
        self.root.join("r")
    }

    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// One request with exactly the headers given (plus `Connection: close` and the length).
    fn raw(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Answer {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream.set_read_timeout(Some(PATIENCE)).unwrap();
        let mut request = format!("{method} {path} HTTP/1.1\r\n");
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str(&format!(
            "Connection: close\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        stream.write_all(request.as_bytes()).unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let mut lines = head.lines();
        let status: u16 = lines
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
            .collect();
        Answer {
            status,
            headers,
            body: body.to_string(),
        }
    }

    /// A request as the page sends it: its Host, Origin and token.
    fn api(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        let host = self.host();
        let origin = format!("http://{host}");
        let bearer = format!("Bearer {}", self.token);
        let mut headers = vec![
            ("Host", host.as_str()),
            ("Origin", origin.as_str()),
            ("Authorization", bearer.as_str()),
        ];
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        if !body.is_empty() {
            headers.push(("Content-Type", "application/json"));
        }
        self.raw(method, path, &headers, &body)
    }

    /// A memory function through the relay: its results (an `error` map fails the test unless
    /// `allow_error`).
    fn query(&self, name: &str, params: Value) -> Value {
        let answer = self.api(
            "POST",
            "/api/query",
            Some(json!({"name": name, "params": params})),
        );
        assert_eq!(answer.status, 200, "{name}: {}", answer.body);
        let value = answer.json();
        assert_eq!(value["ok"], true, "{name}: {value}");
        value["results"].clone()
    }

    /// The store's commit position (`GET /api/changes`).
    fn commit(&self) -> u64 {
        let answer = self.api("GET", "/api/changes", None);
        assert_eq!(answer.status, 200, "{}", answer.body);
        answer.json()["commitSeq"].as_u64().unwrap()
    }

    /// The daemon's answer to a control request; `None` when no daemon listens.
    fn control(&self, request: &str) -> Option<Value> {
        let mut stream = UnixStream::connect(self.run_dir().join("daemon.sock")).ok()?;
        stream.set_read_timeout(Some(PATIENCE)).unwrap();
        writeln!(stream, "{}", json!({"protocol": 1, "control": request})).unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).ok()?;
        serde_json::from_str(&line).ok()
    }

    fn launch_file(&self) -> PathBuf {
        let pid = self.ui.as_ref().unwrap().id();
        self.run_dir().join(format!("ui-{pid}.html"))
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        if let Some(mut ui) = self.ui.take() {
            let _ = ui.kill();
            let _ = ui.wait();
        }
        let pid = self
            .control("status")
            .and_then(|status| status["pid"].as_u64());
        let _ = self.control("stop");
        if let Some(pid) = pid {
            let deadline = Instant::now() + Duration::from_secs(10);
            while alive(pid) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            kill(pid);
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// One HTTP answer.
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|_| panic!("not JSON: {}", self.body))
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn no_cors(&self) {
        assert!(
            self.headers
                .iter()
                .all(|(name, _)| !name.starts_with("access-control-")),
            "{:?}",
            self.headers
        );
    }
}

#[test]
fn the_ui_guards_its_api_by_token_host_and_origin_and_never_sends_cors_headers() {
    let env = Env::start("2h", &[]);
    let host = env.host();
    let origin = format!("http://{host}");
    let bearer = format!("Bearer {}", env.token);
    let wrong = format!("Bearer {}", "0".repeat(64));
    let page = env.raw("GET", "/", &[("Host", &host)], "");
    assert_eq!(page.status, 200);
    assert!(page.body.contains("/memory-ui/boot.js"), "{}", page.body);
    assert_eq!(page.header("cache-control"), Some("no-store"));
    page.no_cors();
    let module = env.raw("GET", "/memory-ui/memory.js", &[("Host", &host)], "");
    assert_eq!(module.status, 200);
    for (headers, status) in [
        (vec![("Host", host.as_str())], 401),
        (
            vec![("Host", host.as_str()), ("Authorization", wrong.as_str())],
            401,
        ),
        (
            vec![("Host", host.as_str()), ("Authorization", "Basic abc")],
            401,
        ),
        (
            vec![("Host", "evil.example"), ("Authorization", bearer.as_str())],
            403,
        ),
        (
            vec![
                ("Host", host.as_str()),
                ("Origin", "http://evil.example"),
                ("Authorization", bearer.as_str()),
            ],
            403,
        ),
        (
            vec![
                ("Host", host.as_str()),
                ("Origin", origin.as_str()),
                ("Authorization", bearer.as_str()),
            ],
            200,
        ),
    ] {
        for path in ["/api/info", "/api/ui/settings"] {
            let answer = env.raw("GET", path, &headers, "");
            assert_eq!(answer.status, status, "{path} {headers:?}: {}", answer.body);
            answer.no_cors();
            assert!(
                !answer.body.contains(&env.token),
                "the token is never echoed"
            );
        }
    }
    let wrong_host_page = env.raw("GET", "/", &[("Host", "evil.example")], "");
    assert_eq!(wrong_host_page.status, 403);
    let preflight = env.raw(
        "OPTIONS",
        "/api/query",
        &[
            ("Host", &host),
            ("Origin", "http://evil.example"),
            ("Access-Control-Request-Method", "POST"),
        ],
        "",
    );
    assert_ne!(preflight.status, 200);
    preflight.no_cors();
    // The relay offers the memory functions and the UI's, nothing else.
    for name in [
        "index_turns",
        "merge_project",
        "set_summary",
        "export_memories",
    ] {
        let answer = env.api(
            "POST",
            "/api/query",
            Some(json!({"name": name, "params": {}})),
        );
        assert_eq!(answer.status, 403, "{name}: {}", answer.body);
    }
    for path in ["/api/graph", "/api/schema", "/api/store"] {
        assert_eq!(env.api("GET", path, None).status, 403, "{path}");
    }
    let info = env.api("GET", "/api/info", None).json();
    assert_eq!(info["uiProfile"]["name"], "memory");
    assert_eq!(info["apiMode"], "allow-list");
}

#[test]
fn reads_change_nothing_and_a_round_trip_writes_through_the_memory_functions() {
    let env = Env::start("2h", &[]);
    let p = "/work/p";
    let before = env.commit();
    for path in [
        "/api/info",
        "/api/changes",
        "/api/ui/status",
        "/api/ui/projects",
        "/api/ui/settings",
        "/api/ui/start-context?project=%2Fwork%2Fp",
    ] {
        let answer = env.api("GET", path, None);
        assert_eq!(answer.status, 200, "{path}: {}", answer.body);
    }
    assert_eq!(env.commit(), before, "a GET never writes");

    // Create (as the UI), list, edit with the "changed meanwhile" check, pin, reorder.
    let remember = |slug: &str, kind: &str, summary: &str| {
        json!({"slug": slug, "kind": kind, "summary": summary, "body": format!("{summary}."),
               "title": format!("Title {slug}"), "source": "ui", "project": p})
    };
    let saved = env.query("remember", remember("use-pnpm", "feedback", "Use pnpm"));
    assert_eq!(saved[0]["created"], true, "{saved}");
    env.query(
        "remember",
        remember("small-commits", "feedback", "Keep commits small"),
    );
    assert!(env.commit() > before);
    let list = env.query(
        "memory_list",
        json!({"kind": "", "scope": "", "source_kind": "ui", "superseded": false, "offset": 0,
               "limit": 50, "project": p}),
    );
    assert_eq!(list[0]["total"], 2, "{list}");
    let opened = env.query("memory", json!({"slug": "use-pnpm", "project": p}));
    assert_eq!(opened[0]["source"], "ui");
    let stamp = opened[0]["updated_at"].clone();
    std::thread::sleep(Duration::from_millis(1100));
    env.query(
        "remember",
        remember("use-pnpm", "feedback", "Use pnpm, never npm"),
    );
    let now = env.query("memory", json!({"slug": "use-pnpm", "project": p}));
    assert_ne!(
        now[0]["updated_at"], stamp,
        "an edit moves updated_at: {now}"
    );
    env.query(
        "pin",
        json!({"slug": "small-commits", "pinned": true, "project": p}),
    );
    let start = env.api("GET", "/api/ui/start-context?project=%2Fwork%2Fp", None);
    let start = start.json();
    assert_eq!(start["entries"][0]["slug"], "small-commits", "{start}");
    assert_eq!(start["entries"][0]["tier"], "pinned", "{start}");
    env.query(
        "pin",
        json!({"slug": "small-commits", "pinned": false, "project": p}),
    );
    env.query(
        "set_order",
        json!({"slugs": ["small-commits", "use-pnpm"], "project": p}),
    );
    let start = env
        .api("GET", "/api/ui/start-context?project=%2Fwork%2Fp", None)
        .json();
    let slugs: Vec<&str> = start["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs, ["small-commits", "use-pnpm"], "{start}");
    // The preview is the text a session starts with.
    let text = start["text"].as_str().unwrap();
    assert!(
        text.find("Title small-commits").unwrap() < text.find("Title use-pnpm").unwrap(),
        "{text}"
    );
    // The budget, set in place.
    let set = env.api(
        "POST",
        "/api/ui/settings",
        Some(json!({"start_context_chars": 3000})),
    );
    assert_eq!(set.status, 200, "{}", set.body);
    let start = env
        .api("GET", "/api/ui/start-context?project=%2Fwork%2Fp", None)
        .json();
    assert_eq!(start["budget"], 3000, "{start}");
    // Settle, then forget.
    env.query(
        "remember",
        remember("use-npm", "project", "Install dependencies with npm"),
    );
    env.query(
        "settle",
        json!({"slug": "use-pnpm", "decision": "supersedes", "other": "use-npm", "project": p}),
    );
    let old = env.query("memory", json!({"slug": "use-npm", "project": p}));
    assert_eq!(old[0]["superseded_by"][0]["slug"], "use-pnpm", "{old}");
    let forgotten = env.query(
        "forget",
        json!({"slug": "use-npm", "source": "ui", "project": p}),
    );
    assert_eq!(forgotten[0]["removed"], true, "{forgotten}");
    let gone = env.query("memory", json!({"slug": "use-npm", "project": p}));
    assert!(gone[0]["error"].is_string(), "{gone}");
}

#[test]
fn an_imported_memory_refuses_edit_and_forget_from_the_ui() {
    let env = Env::start("2h", &[]);
    let p = "/work/p";
    env.query(
        "remember",
        json!({"slug": "from-file", "kind": "project", "summary": "Imported", "body": "x",
               "from_file": {"path": "/home/u/memory/from-file.md"}, "project": p}),
    );
    let edit = env.query(
        "remember",
        json!({"slug": "from-file", "kind": "project", "summary": "Edited", "body": "y",
               "source": "ui", "project": p}),
    );
    assert!(
        edit[0]["error"]
            .as_str()
            .unwrap()
            .contains("/home/u/memory/from-file.md"),
        "{edit}"
    );
    let forget = env.query(
        "forget",
        json!({"slug": "from-file", "source": "ui", "project": p}),
    );
    assert!(forget[0]["error"].is_string(), "{forget}");
    let still = env.query("memory", json!({"slug": "from-file", "project": p}));
    assert_eq!(still[0]["summary"], "Imported", "{still}");
    let pinned = env.query(
        "pin",
        json!({"slug": "from-file", "pinned": true, "project": p}),
    );
    assert_eq!(pinned[0]["pinned"], true);
}

#[test]
fn the_launch_file_is_private_and_goes_with_the_first_heartbeat() {
    let env = Env::start("2h", &[]);
    let file = env.launch_file();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        text.contains(&format!(
            "http://127.0.0.1:{}/#token={}",
            env.port, env.token
        )),
        "{text}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        let dir = std::fs::metadata(env.run_dir())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(dir & 0o777, 0o700, "{dir:o}");
    }
    // A heartbeat without the token is refused and changes nothing.
    let host = env.host();
    let refused = env.raw("POST", "/api/ui/heartbeat", &[("Host", &host)], "");
    assert_eq!(refused.status, 401);
    assert!(file.exists());
    let beat = env.api("POST", "/api/ui/heartbeat", None);
    assert_eq!(beat.status, 200, "{}", beat.body);
    assert!(!file.exists(), "the page is open: the launch file is gone");
    // A GET of a write endpoint is refused.
    assert_eq!(env.api("GET", "/api/ui/heartbeat", None).status, 405);
}

#[test]
fn the_ui_keeps_the_daemon_alive_and_exits_after_the_last_heartbeat() {
    let mut env = Env::start("1s", &["--heartbeat-timeout", "6s"]);
    assert_eq!(env.api("POST", "/api/ui/heartbeat", None).status, 200);
    let daemon = env.control("status").unwrap();
    let pid = daemon["pid"].as_u64().unwrap();
    // Longer than the daemon's idle time: the ui's connection keeps it.
    std::thread::sleep(Duration::from_secs(3));
    assert!(alive(pid), "the daemon stays while ui runs");
    assert!(env.control("status").is_some());
    let file = env.launch_file();
    let mut ui = env.ui.take().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = ui.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "ui did not exit after the timeout"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(status.success(), "{status:?}");
    let mut out = String::new();
    let _ = ui.stdout.take().map(|mut s| s.read_to_string(&mut out));
    assert!(!file.exists(), "removed on exit");
    // Without ui the daemon exits after its idle time.
    let deadline = Instant::now() + Duration::from_secs(20);
    while alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!alive(pid), "the daemon exits idle once ui is gone");
}

#[test]
fn the_ui_keeps_answering_after_a_daemon_restart() {
    let env = Env::start("2h", &[]);
    env.query(
        "remember",
        json!({"slug": "ui-before", "kind": "project", "summary": "Before the restart",
               "body": "Before.", "source": "ui", "project": "/work/r"}),
    );
    let pid = env.control("status").unwrap()["pid"].as_u64().unwrap();
    kill(pid);
    let deadline = Instant::now() + PATIENCE;
    while alive(pid) {
        assert!(Instant::now() < deadline, "the daemon did not die");
        std::thread::sleep(Duration::from_millis(50));
    }
    // The relay starts (or finds) a new daemon: the memory is still there.
    let rows = env.query("memory", json!({"slug": "ui-before", "project": "/work/r"}));
    assert!(rows.to_string().contains("Before the restart"), "{rows}");
    // `/api/ui/*` too: the start context goes to the daemon that runs now.
    let answer = env.api("GET", "/api/ui/start-context?project=%2Fwork%2Fr", None);
    assert_eq!(answer.status, 200, "{}", answer.body);
    let status = env.control("status").unwrap();
    let new = status["pid"].as_u64().unwrap();
    assert_ne!(new, pid);
    // The held connection reconnects too (the daemon counts it).
    let deadline = Instant::now() + PATIENCE;
    while env.control("status").unwrap()["connections"].as_u64() == Some(0) {
        assert!(
            Instant::now() < deadline,
            "ui did not connect to the new daemon"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
