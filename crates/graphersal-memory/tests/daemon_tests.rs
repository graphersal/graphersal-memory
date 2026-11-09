//! The daemon and the shim end to end: the real binary, started as shims (`mcp`) the way a
//! harness starts them, each test with its own temporary data, run and home directories (never
//! the user's, never a running daemon of the user). Every daemon a test starts is stopped (and
//! killed if it does not stop) when the test ends.
//!
//! Unix only for now: the tests kill processes with `kill` and speak to the socket with
//! `std::os::unix::net`; Windows builds the same code in CI.

#![cfg(unix)]
// Test helpers outside `#[test]` functions: clippy's allow-unwrap-in-tests does not reach them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_graphersal-memory");
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// How long a test waits for an answer before it fails (the machine may be busy).
const PATIENCE: Duration = Duration::from_secs(90);

/// A test's directories (short paths: a socket path has at most 103 bytes) and the daemons it
/// started.
struct Env {
    root: PathBuf,
    idle: &'static str,
}

impl Env {
    fn new(idle: &'static str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gm-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self { root, idle }
    }

    fn data_dir(&self) -> PathBuf {
        self.root.join("d")
    }

    fn run_dir(&self) -> PathBuf {
        self.root.join("r")
    }

    fn socket(&self) -> PathBuf {
        self.run_dir().join("daemon.sock")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(BIN);
        command
            .env("GRAPHERSAL_MEMORY_DIR", self.data_dir())
            .env("GRAPHERSAL_MEMORY_RUN_DIR", self.run_dir())
            .env("GRAPHERSAL_MEMORY_IDLE", self.idle)
            .env("HOME", &self.root)
            .env_remove("XDG_RUNTIME_DIR")
            .env_remove("CLAUDECODE")
            .current_dir(&self.root);
        command
    }

    /// A shim of `project` with the `use` profile, initialized.
    fn shim(&self, project: &str) -> Shim {
        self.shim_with(project, &[])
    }

    fn shim_with(&self, project: &str, extra: &[&str]) -> Shim {
        let mut shim = self.spawn_shim(project, extra);
        let init = shim.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "test", "version": "1"}}),
        );
        assert!(init["result"].is_object(), "{init}");
        shim.notify("notifications/initialized");
        shim
    }

    fn spawn_shim(&self, project: &str, extra: &[&str]) -> Shim {
        let mut child = self
            .command()
            .arg("mcp")
            .arg("--project")
            .arg(project)
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (lines, from) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if lines.send(line).is_err() {
                    break;
                }
            }
        });
        Shim {
            to: child.stdin.take(),
            from,
            child,
            next_id: 1,
        }
    }

    /// The daemon's answer to a control request; `None` when no daemon listens.
    fn control(&self, request: &str) -> Option<Value> {
        let mut stream = UnixStream::connect(self.socket()).ok()?;
        stream.set_read_timeout(Some(PATIENCE)).unwrap();
        writeln!(stream, "{}", json!({"protocol": 1, "control": request})).unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).ok()?;
        serde_json::from_str(&line).ok()
    }

    fn status(&self) -> Option<Value> {
        self.control("status")
    }

    /// One raw first line to the daemon and its answer.
    fn raw(&self, line: &str) -> Value {
        let mut stream = UnixStream::connect(self.socket()).unwrap();
        stream.set_read_timeout(Some(PATIENCE)).unwrap();
        writeln!(stream, "{line}").unwrap();
        let mut answer = String::new();
        BufReader::new(stream).read_line(&mut answer).unwrap();
        serde_json::from_str(&answer).unwrap()
    }

    /// Waits until `done` holds, failing after `PATIENCE`.
    fn wait_for(&self, what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + PATIENCE;
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.run_dir().join("daemon.log")).unwrap_or_default()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let pid = self.status().and_then(|status| status["pid"].as_u64());
        let _ = self.control("stop");
        if let Some(pid) = pid {
            // Give it a moment to close its stores, then make sure it is gone.
            let deadline = Instant::now() + Duration::from_secs(10);
            while alive(pid) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            kill(pid);
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

fn kill(pid: u64) {
    let _ = Command::new("kill")
        .arg("-9")
        .arg(pid.to_string())
        .stderr(Stdio::null())
        .status();
}

/// One shim process with a test client on its stdio.
struct Shim {
    to: Option<ChildStdin>,
    from: Receiver<String>,
    child: Child,
    next_id: u64,
}

impl Shim {
    fn send(&mut self, message: &Value) {
        let to = self.to.as_mut().unwrap();
        writeln!(to, "{message}").unwrap();
        to.flush().unwrap();
    }

    fn notify(&mut self, method: &str) {
        self.send(&json!({"jsonrpc": "2.0", "method": method}));
    }

    fn send_request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    /// The next answer line (server notifications skipped); `None` when the shim closed.
    fn answer(&mut self) -> Option<Value> {
        loop {
            let line = self.from.recv_timeout(PATIENCE).ok()?;
            let message: Value = serde_json::from_str(&line).unwrap();
            if message.get("id").is_some() {
                return Some(message);
            }
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send_request(method, params);
        let answer = self.answer().expect("the shim closed");
        assert_eq!(answer["id"], json!(id), "{answer}");
        answer
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let answer = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        answer.get("result").cloned().unwrap_or(answer)
    }

    fn ok(&mut self, name: &str, arguments: Value) -> String {
        let result = self.call(name, arguments);
        assert_eq!(result["isError"], json!(false), "{name}: {result}");
        result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    fn remember(&mut self, slug: &str, summary: &str, scope: &str) {
        self.ok(
            "remember",
            json!({"slug": slug, "kind": "project", "summary": summary,
                   "body": format!("{summary}."), "scope": scope}),
        );
    }

    /// Ends the input; the exit status and stderr.
    fn close(mut self) -> (bool, String) {
        drop(self.to.take());
        let status = self.child.wait().unwrap();
        let mut stderr = String::new();
        if let Some(mut err) = self.child.stderr.take() {
            let _ = err.read_to_string(&mut stderr);
        }
        (status.success(), stderr)
    }
}

/// A clean close: exit code 0, nothing on stderr.
fn close_ok(shim: Shim) {
    let (ok, stderr) = shim.close();
    assert!(ok, "the shim failed: {stderr}");
}

#[test]
fn two_shims_share_one_daemon_and_see_each_others_memories() {
    let env = Env::new("10m");
    let mut a = env.shim("/w/a");
    let mut b = env.shim("/w/a");
    let status = env.status().unwrap();
    assert_eq!(status["connections"], 2, "{status}");
    assert_eq!(status["stores"], json!(["default"]), "{status}");
    assert_eq!(status["daemon_version"], VERSION);

    a.remember("shared-note", "Both sessions see this note", "project");
    let listed = b.ok("memories", json!({}));
    assert!(listed.contains("shared-note"), "{listed}");
    b.remember("from-b", "Written by the second session", "global");
    let listed = a.ok("memories", json!({}));
    assert!(listed.contains("from-b"), "{listed}");

    // Another project on the same daemon: the global memory only.
    let mut c = env.shim("/w/c");
    let listed = c.ok("memories", json!({}));
    assert!(listed.contains("from-b"), "{listed}");
    assert!(!listed.contains("shared-note"), "{listed}");
    // Faking the project argument does not help.
    let faked = c.ok("memories", json!({"project": "/w/a"}));
    assert!(!faked.contains("shared-note"), "{faked}");

    // The develop profile on the same store.
    let mut d = env.shim_with("/w/a", &["--profile", "develop"]);
    let stored = d.ok(
        "gql",
        json!({"query": "MATCH (m:memory) RETURN m.name AS n ORDER BY n"}),
    );
    assert!(
        stored.contains("shared-note") && stored.contains("from-b"),
        "{stored}"
    );
    let refused = a.call("gql", json!({"query": "MATCH (n) RETURN n"}));
    assert!(
        refused["isError"] == json!(true) || refused.get("error").is_some(),
        "{refused}"
    );
    let pid = env.status().unwrap()["pid"].clone();
    for shim in [a, b, c, d] {
        close_ok(shim);
    }
    let status = env.status().unwrap();
    assert_eq!(status["connections"], 0, "{status}");
    assert_eq!(status["pid"], pid, "one daemon served them all");
}

#[test]
fn parallel_calls_from_two_connections_all_land() {
    let env = Env::new("10m");
    let shims = [env.shim("/w/p"), env.shim("/w/p")];
    let workers: Vec<_> = shims
        .into_iter()
        .enumerate()
        .map(|(n, mut shim)| {
            std::thread::spawn(move || {
                // Ten requests in flight at once on each connection.
                let ids: Vec<u64> = (0..10)
                    .map(|i| {
                        shim.send_request(
                            "tools/call",
                            json!({"name": "remember", "arguments": {
                                "slug": format!("note-{n}-{i}"), "kind": "project",
                                "summary": format!("Parallel note {n} {i}"), "body": "b"}}),
                        )
                    })
                    .collect();
                let mut answered = Vec::new();
                for _ in &ids {
                    let answer = shim.answer().expect("the shim closed");
                    assert_eq!(answer["result"]["isError"], json!(false), "{answer}");
                    answered.push(answer["id"].as_u64().unwrap());
                }
                answered.sort();
                assert_eq!(answered, ids);
                shim
            })
        })
        .collect();
    let mut shims: Vec<Shim> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    for n in 0..2 {
        for i in 0..10 {
            let read = shims[0].ok("memory", json!({"slug": format!("note-{n}-{i}")}));
            assert!(read.contains(&format!("Parallel note {n} {i}")), "{read}");
        }
    }
    for shim in shims {
        close_ok(shim);
    }
}

#[test]
fn the_daemon_exits_after_the_idle_time() {
    let env = Env::new("1s");
    let mut shim = env.shim("/w/i");
    shim.remember("kept", "Survives the idle exit", "project");
    let pid = env.status().unwrap()["pid"].as_u64().unwrap();
    // Connected: no idle exit however long it takes.
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        env.status().is_some(),
        "an open connection keeps the daemon"
    );
    close_ok(shim);
    env.wait_for("the idle exit", || !alive(pid));
    assert!(!env.socket().exists(), "the socket is removed");
    let log = env.log();
    assert!(log.contains("idle for 1s"), "{log}");
    assert!(log.contains("closed the store default"), "{log}");
    // The next session starts a new daemon on the same store.
    let mut again = env.shim("/w/i");
    let read = again.ok("memory", json!({"slug": "kept"}));
    assert!(read.contains("Survives the idle exit"), "{read}");
    close_ok(again);
}

#[test]
fn a_stale_socket_file_is_recovered() {
    let env = Env::new("10m");
    std::fs::create_dir_all(env.run_dir()).unwrap();
    // A socket file nobody listens on, and an unlocked pid file: what a crash leaves.
    drop(UnixListener::bind(env.socket()).unwrap());
    std::fs::write(env.run_dir().join("daemon.pid"), "999999\n").unwrap();
    assert!(env.socket().exists());
    assert!(UnixStream::connect(env.socket()).is_err());
    let mut shim = env.shim("/w/s");
    shim.remember("after-crash", "Written after a stale socket", "project");
    let status = env.status().unwrap();
    assert_eq!(status["connections"], 1, "{status}");
    close_ok(shim);
}

#[test]
fn a_killed_daemon_is_restarted_by_the_next_shim() {
    let env = Env::new("10m");
    let mut first = env.shim("/w/k");
    first.remember("before-kill", "Written before the crash", "project");
    let pid = env.status().unwrap()["pid"].as_u64().unwrap();
    kill(pid);
    env.wait_for("the kill", || !alive(pid));
    // The open session notices: its shim ends with an error naming the log.
    assert!(first.answer().is_none());
    let (ok, stderr) = first.close();
    assert!(!ok);
    assert!(stderr.contains("daemon.log"), "{stderr}");
    // The next session starts a new daemon; the committed memory survived.
    let mut next = env.shim("/w/k");
    let read = next.ok("memory", json!({"slug": "before-kill"}));
    assert!(read.contains("Written before the crash"), "{read}");
    let status = env.status().unwrap();
    assert_ne!(status["pid"].as_u64().unwrap(), pid);
    close_ok(next);
}

#[test]
fn shims_starting_at_once_start_one_daemon() {
    let env = Env::new("10m");
    let shims: Vec<Shim> = (0..4).map(|_| env.spawn_shim("/w/race", &[])).collect();
    let workers: Vec<_> = shims
        .into_iter()
        .map(|mut shim| {
            std::thread::spawn(move || {
                let init = shim.request(
                    "initialize",
                    json!({"protocolVersion": "2025-06-18", "capabilities": {},
                           "clientInfo": {"name": "race", "version": "1"}}),
                );
                assert!(init["result"].is_object(), "{init}");
                shim
            })
        })
        .collect();
    let shims: Vec<Shim> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    let status = env.status().unwrap();
    assert_eq!(status["connections"], 4, "{status}");
    let log = env.log();
    assert_eq!(log.matches("listening on").count(), 1, "{log}");
    for shim in shims {
        close_ok(shim);
    }
}

#[test]
fn handshake_errors_are_clear() {
    let env = Env::new("10m");
    let shim = env.shim("/w/h");
    let answer = env.raw(
        r#"{"protocol":1,"shim_version":"0.0.0","profile":"admin","project":{"root":"/w"},"harness":{"name":"t"}}"#,
    );
    assert_eq!(answer["ok"], false);
    assert_eq!(answer["error"]["code"], "unknown_profile");
    assert!(
        answer["error"]["message"]
            .as_str()
            .unwrap()
            .contains("expected use or develop"),
        "{answer}"
    );
    let answer = env.raw(r#"{"protocol":7,"shim_version":"9.0.0","profile":"use"}"#);
    assert_eq!(answer["error"]["code"], "protocol");
    assert!(
        answer["error"]["message"]
            .as_str()
            .unwrap()
            .contains("protocol 7 is not supported"),
        "{answer}"
    );
    let answer = env.raw("GET / HTTP/1.1");
    assert_eq!(answer["error"]["code"], "bad_handshake");
    let answer = env.raw(r#"{"protocol":1,"control":"reboot"}"#);
    assert!(
        answer["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown control request 'reboot'"),
        "{answer}"
    );

    // A shim of another data directory meeting this run directory's daemon.
    let other = env.root.join("other-data");
    let mut wrong = env
        .command()
        .arg("mcp")
        .arg("--project")
        .arg("/w/h")
        .arg("--data-dir")
        .arg(&other)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(wrong.stdin.take());
    let status = wrong.wait().unwrap();
    assert_eq!(status.code(), Some(1));
    let mut stderr = String::new();
    wrong
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(
        stderr.contains("the daemon refused the connection: this daemon serves the data directory"),
        "{stderr}"
    );
    close_ok(shim);
}

#[test]
fn an_older_daemon_makes_way_for_a_newer_shim() {
    let env = Env::new("10m");
    let mut old = env
        .command()
        .args(["daemon", "--foreground", "--version-as", "0.0.0-old"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    env.wait_for("the old daemon", || {
        env.status()
            .is_some_and(|status| status["daemon_version"] == "0.0.0-old")
    });
    let mut shim = env.shim("/w/u");
    shim.remember("upgraded", "Served by the new daemon", "project");
    let status = env.status().unwrap();
    assert_eq!(status["daemon_version"], VERSION, "{status}");
    assert!(
        old.wait().unwrap().success(),
        "the old daemon exits cleanly"
    );
    close_ok(shim);
}

#[test]
fn an_older_daemon_with_sessions_drains() {
    let env = Env::new("10m");
    let mut old = env
        .command()
        .args(["daemon", "--foreground", "--version-as", "0.0.0-old"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    env.wait_for("the old daemon", || env.status().is_some());
    // A session of the old version (same protocol) is open.
    let mut stream = UnixStream::connect(env.socket()).unwrap();
    writeln!(
        stream,
        r#"{{"protocol":1,"shim_version":"0.0.0-old","profile":"use","project":{{"root":"/w/o"}},"harness":{{"name":"t"}}}}"#
    )
    .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(line.contains("\"ok\":true"), "{line}");
    // A newer shim is served by the old daemon, which now drains.
    let mut shim = env.shim("/w/o");
    shim.remember("during-drain", "Written while draining", "project");
    let status = env.status().unwrap();
    assert_eq!(status["daemon_version"], "0.0.0-old");
    assert_eq!(status["draining"], true, "{status}");
    close_ok(shim);
    // The last old session closes: the old daemon exits; the next shim starts the new one.
    drop(reader);
    drop(stream);
    assert!(old.wait().unwrap().success());
    let mut next = env.shim("/w/o");
    let read = next.ok("memory", json!({"slug": "during-drain"}));
    assert!(read.contains("Written while draining"), "{read}");
    assert_eq!(env.status().unwrap()["daemon_version"], VERSION);
    close_ok(next);
}

#[test]
fn the_daemon_and_status_commands_report_the_daemon() {
    let env = Env::new("10m");
    let output = env.command().arg("status").output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("daemon: not running"), "{text}");

    let output = env.command().arg("daemon").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("daemon: running (pid "), "{text}");

    let output = env.command().arg("status").output().unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("daemon: running"), "{text}");
    assert!(text.contains(&format!("version {VERSION}")), "{text}");

    // A second foreground daemon refuses to run beside the first.
    let output = env
        .command()
        .args(["daemon", "--foreground"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("another daemon already runs"), "{stderr}");
}

#[test]
fn the_socket_and_run_directory_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new("10m");
    let shim = env.shim("/w/perm");
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&env.run_dir()), 0o700);
    assert_eq!(mode(&env.socket()), 0o600);
    close_ok(shim);
}
