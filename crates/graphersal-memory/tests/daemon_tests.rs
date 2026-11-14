//! The daemon and the shim end to end: the real binary, started as shims (`mcp`) the way a
//! harness starts them, each test with its own temporary data, run and home directories (never
//! the user's, never a running daemon of the user). Every daemon a test starts is stopped (and
//! killed if it does not stop) when the test ends.
//!
//! Every platform: the socket is AF_UNIX on Unix (`std::os::unix::net`) and on Windows
//! (`uds_windows`, the product's own transport); processes are checked and killed with `kill` on
//! Unix and `tasklist`/`taskkill` on Windows. Only the permission test is Unix only.

// Test helpers outside `#[test]` functions: clippy's allow-unwrap-in-tests does not reach them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

mod support;
use support::{UnixStream, alive, kill};

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
            // Windows: the home directory is the profile; the data directory is set above.
            .env("USERPROFILE", &self.root)
            .env("LOCALAPPDATA", &self.root)
            .env_remove("XDG_RUNTIME_DIR")
            .env_remove("CLAUDECODE")
            // The harnesses' own locations: a shim's session start looks at the harnesses
            // installed there (`session_sync`), which must be the temporary home's.
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
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
        self.spawn_shim_env(project, extra, &[])
    }

    /// [`Env::spawn_shim`] with extra environment variables.
    fn spawn_shim_env(&self, project: &str, extra: &[&str], vars: &[(&str, &str)]) -> Shim {
        let mut command = self.command();
        command.envs(vars.iter().copied());
        Self::spawn_shim_from(command, project, extra)
    }

    /// A shim of `project` started by `command` (its environment and global flags set).
    fn spawn_shim_from(mut command: Command, project: &str, extra: &[&str]) -> Shim {
        let mut child = command
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

/// Leaves a socket file nobody listens on, as a crash does.
#[cfg(unix)]
fn stale_socket(path: &Path) {
    // The file is made by a datagram socket, never a listening one: on macOS std sets
    // close-on-exec on a new socket only after creating it, so a shim another test spawns at
    // that moment inherits the descriptor (and passes it on to its daemon); an inherited
    // LISTENING socket would keep accepting connections into its backlog, so the "stale" socket
    // answered nobody's handshake and this test failed whenever it ran beside the others. A
    // stream connect to a datagram socket is refused, whoever holds its descriptor.
    drop(std::os::unix::net::UnixDatagram::bind(path).unwrap());
}

/// Leaves a socket file nobody listens on, as a crash does (Windows keeps the file of a closed
/// listening socket; it has no datagram AF_UNIX sockets).
#[cfg(windows)]
fn stale_socket(path: &Path) {
    drop(uds_windows::UnixListener::bind(path).unwrap());
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
    stale_socket(&env.socket());
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
    // The open session reconnects (and starts a new daemon) instead of ending: see
    // `a_daemon_restart_keeps_the_open_session`.
    close_ok(first);
    // The next session finds a new daemon; the committed memory survived.
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

#[cfg(unix)]
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

#[cfg(unix)]
#[test]
fn the_data_directory_is_private_and_an_open_one_is_tightened_at_the_daemon_start() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new("10m");
    // A data directory others may read, as umask 022 made it before.
    std::fs::create_dir_all(env.data_dir()).unwrap();
    std::fs::set_permissions(env.data_dir(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut shim = env.shim("/w/private");
    shim.remember("private-note", "Only the owner reads this", "project");
    let mode = std::fs::metadata(env.data_dir())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700, "{mode:o}");
    close_ok(shim);
}

#[test]
fn relative_data_and_run_directories_are_made_absolute() {
    let env = Env::new("10m");
    // `--data-dir d --run-dir r` from the test's root: the directories of `env`, given relative.
    let mut command = env.command();
    command
        .env_remove("GRAPHERSAL_MEMORY_DIR")
        .env_remove("GRAPHERSAL_MEMORY_RUN_DIR")
        .args(["--data-dir", "d", "--run-dir", "r"]);
    let mut shim = Env::spawn_shim_from(command, "/w/relative", &[]);
    let init = shim.request(
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {},
               "clientInfo": {"name": "test", "version": "1"}}),
    );
    assert!(init["result"].is_object(), "{init}: {}", env.log());
    shim.notify("notifications/initialized");
    shim.remember(
        "relative-note",
        "Stored under the given directory",
        "project",
    );
    assert!(env.data_dir().join("stores").join("default").is_dir());
    assert!(
        !env.run_dir().join("d").exists(),
        "not resolved against the run directory"
    );
    assert!(env.status().is_some(), "the daemon listens in r");
    close_ok(shim);
    // A later command with the same relative flags finds the same daemon.
    let output = env
        .command()
        .env_remove("GRAPHERSAL_MEMORY_DIR")
        .env_remove("GRAPHERSAL_MEMORY_RUN_DIR")
        .args(["--data-dir", "d", "--run-dir", "r", "status"])
        .output()
        .unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("daemon: running"), "{text}");
}

#[test]
fn the_daemon_opens_the_store_at_its_start_and_status_shows_its_maintenance() {
    let env = Env::new("10m");
    let mut shim = env.shim("/w/warm");
    shim.remember("warm-note", "The release runs on Mondays", "project");
    close_ok(shim);
    let _ = env.control("stop");
    env.wait_for("the daemon to stop", || env.status().is_none());

    // A started daemon opens the default store by itself; a search right away waits for it and
    // answers from the stored memory.
    let output = env.command().arg("daemon").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut shim = env.shim("/w/warm");
    let found = shim.ok("memories", json!({"text": "when does the release run"}));
    assert!(found.contains("warm-note"), "{found}");
    close_ok(shim);
    let log = env.log();
    assert_eq!(
        log.matches("opened the store default").count(),
        2,
        "one open per daemon life: {log}"
    );

    let output = env.command().arg("status").output().unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("store default: "), "{text}");
    assert!(text.contains("WAL since the last snapshot: "), "{text}");
    assert!(text.contains("checkpoint after 16.0 MiB"), "{text}");
    assert!(text.contains("history kept: 30 days"), "{text}");
    let output = env.command().arg("doctor").output().unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("store default: ") && text.contains("of WAL since the last snapshot"),
        "{text}"
    );
}

/// A shim whose daemon (started by it) panics at `site:name` (the test-only hook of debug
/// builds, `GRAPHERSAL_MEMORY_TEST_PANIC`).
fn shim_with_panic(env: &Env, project: &str, panic_at: &str) -> Shim {
    let mut child = env
        .command()
        .env("GRAPHERSAL_MEMORY_TEST_PANIC", panic_at)
        .arg("mcp")
        .arg("--project")
        .arg(project)
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
    let mut shim = Shim {
        to: child.stdin.take(),
        from,
        child,
        next_id: 1,
    };
    let init = shim.request(
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {},
               "clientInfo": {"name": "test", "version": "1"}}),
    );
    assert!(init["result"].is_object(), "{init}");
    shim.notify("notifications/initialized");
    shim
}

/// A panic inside a job on the store's session thread is logged with the function it ran, the
/// call gets a JSON refusal, and the next calls (control and MCP) are answered.
#[test]
fn a_panic_in_a_job_is_logged_and_the_daemon_serves_on() {
    let env = Env::new("10m");
    let mut shim = shim_with_panic(&env, "/w/a", "job:export_memories");
    shim.remember("before-the-panic", "Written before the panic", "project");
    let refused = env.raw(r#"{"protocol":1,"control":"export_memories","project":"/w/a"}"#);
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error"]["code"], "store", "{refused}");
    let log = env.log();
    assert!(log.contains("PANIC"), "{log}");
    assert!(
        log.contains("while running the catalog function export_memories"),
        "{log}"
    );
    // The next calls are answered.
    assert!(env.status().is_some());
    let listed = shim.ok("memories", json!({}));
    assert!(listed.contains("before-the-panic"), "{listed}");
    // The CLI says the daemon failed, not "another program on the socket".
    let output = env
        .command()
        .args(["export", "--to-files", "--harness", "opencode"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{text}");
    assert!(!text.contains("another program"), "{text}");
    assert!(
        text.contains("the daemon failed") && text.contains("daemon.log"),
        "{text}"
    );
    close_ok(shim);
}

/// A panic while answering a control request is logged with the request and answered with an
/// `internal` refusal naming the log; the daemon serves on.
#[test]
fn a_panic_in_a_control_request_is_answered_and_the_daemon_serves_on() {
    let env = Env::new("10m");
    let shim = shim_with_panic(&env, "/w/a", "control:sessions_status");
    let refused = env.control("sessions_status").unwrap();
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error"]["code"], "internal", "{refused}");
    let message = refused["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("daemon.log"), "{message}");
    let log = env.log();
    assert!(
        log.contains("while answering the control request 'sessions_status'"),
        "{log}"
    );
    assert!(env.status().is_some());
    close_ok(shim);
}

/// An answer longer than the handshake limit (every memory for `export_memories`, as
/// `uninstall` reads them) is read whole: it once ended at 64 KiB and was reported as "not
/// JSON (another program on the socket?)".
#[test]
fn a_large_control_answer_is_read_whole() {
    let env = Env::new("10m");
    let mut shim = env.shim("/w/a");
    let body = "A long body that makes the export answer large. ".repeat(40);
    for i in 0..60 {
        shim.ok(
            "remember",
            json!({"slug": format!("long-note-{i}"), "kind": "project",
                   "summary": format!("Long note {i}"), "body": body, "scope": "global"}),
        );
    }
    let output = env
        .command()
        .args(["export", "--to-files", "--harness", "opencode"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{text}");
    let global =
        std::fs::read_to_string(env.root.join("graphersal-memory-export/opencode/global.md"))
            .unwrap_or_default();
    assert!(global.contains("long-note-59"), "{text}");
    close_ok(shim);
}

// ---- M14: the start context end to end ----

/// A shim of `project` started with the extra environment `env_vars`; its `initialize`
/// instructions and the shim.
fn shim_instructions(env: &Env, project: &Path, env_vars: &[(&str, &str)]) -> (String, Shim) {
    let mut command = env.command();
    command
        .arg("mcp")
        .arg("--project")
        .arg(project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env_vars {
        command.env(key, value);
    }
    let mut child = command.spawn().unwrap();
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
    let mut shim = Shim {
        to: child.stdin.take(),
        from,
        child,
        next_id: 1,
    };
    let init = shim.request(
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {},
               "clientInfo": {"name": "test", "version": "1"}}),
    );
    shim.notify("notifications/initialized");
    let instructions = init["result"]["instructions"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    (instructions, shim)
}

#[test]
fn the_start_context_reaches_the_instructions_the_cli_and_status() {
    let env = Env::new("10m");
    let project = env.root.join("w");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    let mut shim = env.shim(&project.display().to_string());
    shim.ok(
        "remember",
        json!({"slug": "ask-before-tests", "kind": "feedback", "summary": "Ask before changing tests",
               "body": "Changing an existing test needs approval.",
               "title": "Existing tests need approval", "hook": "ask first"}),
    );
    shim.ok(
        "remember",
        json!({"slug": "release-fridays", "kind": "project", "summary": "Releases on Fridays",
               "body": "Fridays.", "scope": "global"}),
    );
    shim.ok("pin", json!({"slug": "release-fridays"}));
    close_ok(shim);

    // A harness we cannot tell: the whole start context in the instructions.
    let (full, shim) = shim_instructions(&env, &project, &[]);
    close_ok(shim);
    assert!(
        full.contains(
            "## How to work\n- Existing tests need approval — ask first (memory: ask-before-tests)"
        ),
        "{full}"
    );
    assert!(
        full.contains(
            "## Pinned\n- release-fridays — Releases on Fridays (memory: release-fridays)"
        ),
        "{full}"
    );

    // Claude Code without our hook: cut to its limit; with our hook: left to the hook.
    let (short, shim) = shim_instructions(&env, &project, &[("CLAUDECODE", "1")]);
    close_ok(shim);
    assert!(short.chars().count() <= 4096, "{}", short.len());
    assert!(short.contains("(memory: ask-before-tests)"), "{short}");
    let claude = env.root.join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(
        claude.join("settings.json"),
        json!({"hooks": {"SessionStart": [{"hooks": [{"type": "command",
            "command": "/opt/bin/graphersal-memory hook session-start"}]}]}})
        .to_string(),
    )
    .unwrap();
    let (hooked, shim) = shim_instructions(&env, &project, &[("CLAUDECODE", "1")]);
    close_ok(shim);
    assert!(!hooked.contains("## How to work"), "{hooked}");
    assert!(hooked.starts_with("graphersal-memory:"), "{hooked}");

    // The command prints exactly the text the agent gets (from a directory in the project).
    std::fs::create_dir_all(project.join("sub")).unwrap();
    let output = env
        .command()
        .arg("start-context")
        .arg("--project")
        .arg(project.join("sub"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(full.ends_with(text.trim_end()), "{text}\n---\n{full}");
    assert!(
        text.starts_with("graphersal-memory: the memories that always apply"),
        "{text}"
    );

    // status from inside the project.
    let output = env
        .command()
        .arg("status")
        .current_dir(&project)
        .output()
        .unwrap();
    let status = String::from_utf8(output.stdout).unwrap();
    let line = status
        .lines()
        .find(|line| line.trim_start().starts_with("start context:"))
        .unwrap_or_else(|| panic!("{status}"));
    assert!(line.contains("start context: 2 memories,"), "{line}");
    assert!(line.contains(" of 8000 characters"), "{line}");
    let output = env
        .command()
        .arg("doctor")
        .current_dir(&project)
        .output()
        .unwrap();
    let doctor = String::from_utf8(output.stdout).unwrap();
    assert!(doctor.contains("start context: 2 memories,"), "{doctor}");
}

/// A stopped (SIGSTOP) process continues; Unix only (the in-flight test).
#[cfg(unix)]
fn signal(pid: u64, name: &str) {
    let status = Command::new("kill")
        .arg(format!("-{name}"))
        .arg(pid.to_string())
        .status()
        .unwrap();
    assert!(status.success(), "kill -{name} {pid}");
}

/// The daemon's pid, waiting until one answers that is not `old`.
fn new_daemon(env: &Env, old: u64) -> u64 {
    let mut pid = None;
    env.wait_for("a new daemon", || {
        pid = env
            .status()
            .and_then(|status| status["pid"].as_u64())
            .filter(|pid| *pid != old);
        pid.is_some()
    });
    pid.unwrap()
}

#[test]
fn a_daemon_restart_keeps_the_open_session() {
    let env = Env::new("10m");
    let mut shim = env.shim("/w/r");
    shim.remember("before-restart", "Written before the restart", "project");
    let pid = env.status().unwrap()["pid"].as_u64().unwrap();
    kill(pid);
    env.wait_for("the kill", || !alive(pid));
    // The next call goes through a new daemon, without a new initialize from the harness
    // (the shim replays the session's own).
    let read = shim.ok("memory", json!({"slug": "before-restart"}));
    assert!(read.contains("Written before the restart"), "{read}");
    shim.remember("after-restart", "Written after the restart", "project");
    let new = new_daemon(&env, pid);
    assert!(alive(new));
    // The replayed initialize answered nothing to the harness: the next answer is its own.
    let listed = shim.request("tools/list", json!({}));
    assert!(listed["result"]["tools"].is_array(), "{listed}");
    // The same project is bound on the new connection.
    let mut other = env.shim("/w/r");
    let read = other.ok("memory", json!({"slug": "after-restart"}));
    assert!(read.contains("Written after the restart"), "{read}");
    close_ok(other);
    let (ok, stderr) = shim.close();
    assert!(ok, "{stderr}");
    assert!(stderr.contains("reconnected to the daemon"), "{stderr}");
    // The new daemon logged one more connection, the replayed handshake.
    assert!(
        env.log().contains("connection opened: profile use"),
        "{}",
        env.log()
    );
}

#[cfg(unix)]
#[test]
fn a_request_in_flight_at_the_restart_gets_an_error_answer() {
    let env = Env::new("10m");
    let mut shim = env.shim("/w/f");
    shim.remember("kept", "Kept across the restart", "project");
    let pid = env.status().unwrap()["pid"].as_u64().unwrap();
    // A stopped daemon reads nothing: the request is in flight for sure when it dies.
    signal(pid, "STOP");
    let id = shim.send_request(
        "tools/call",
        json!({"name": "memory", "arguments": {"slug": "kept"}}),
    );
    std::thread::sleep(Duration::from_millis(300));
    signal(pid, "KILL");
    env.wait_for("the kill", || !alive(pid));
    let answer = shim.answer().expect("an answer, not silence");
    assert_eq!(answer["id"], json!(id), "{answer}");
    assert_eq!(answer["error"]["code"], -32000, "{answer}");
    let message = answer["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("restarted") && message.contains("retry"),
        "{message}"
    );
    // The retry works through the new daemon.
    let read = shim.ok("memory", json!({"slug": "kept"}));
    assert!(read.contains("Kept across the restart"), "{read}");
    close_ok(shim);
}

#[test]
fn a_restart_does_not_repeat_the_session_start_and_the_end_reaches_the_new_daemon() {
    let env = Env::new("10m");
    // Claude Code's sessions directory: the shim's session start queues their indexing.
    std::fs::create_dir_all(env.root.join(".claude").join("projects")).unwrap();
    let project = env.root.join("w");
    std::fs::create_dir_all(&project).unwrap();
    let mut shim = env.spawn_shim_env(&project.display().to_string(), &[], &[("CLAUDECODE", "1")]);
    let init = shim.request(
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {},
               "clientInfo": {"name": "test", "version": "1"}}),
    );
    assert!(init["result"].is_object(), "{init}");
    shim.notify("notifications/initialized");
    let requested = || {
        env.control("sessions_status")
            .and_then(|status| status["requested"].as_u64())
    };
    env.wait_for("the session start's indexing request", || {
        requested().is_some_and(|n| n >= 1)
    });
    let pid = env.status().unwrap()["pid"].as_u64().unwrap();
    kill(pid);
    env.wait_for("the kill", || !alive(pid));
    let listed = shim.request("tools/list", json!({}));
    assert!(listed["result"]["tools"].is_array(), "{listed}");
    let new = new_daemon(&env, pid);
    // A moment for a start that would run again: none comes.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        requested(),
        Some(0),
        "the session start ran once, on the first daemon"
    );
    // The session ends: its end work goes to the daemon that runs now.
    let (ok, stderr) = shim.close();
    assert!(ok, "{stderr}");
    assert_eq!(env.status().unwrap()["pid"].as_u64(), Some(new));
    assert_eq!(requested(), Some(1), "the session end queued once");
}

#[test]
fn a_daemon_stopped_on_purpose_is_not_started_again_by_an_open_session() {
    let env = Env::new("10m");
    let shim = env.shim("/w/s");
    let pid = env.status().unwrap()["pid"].as_u64().unwrap();
    // `purge` and `uninstall` stop the daemon this way.
    assert_eq!(env.control("stop").unwrap()["ok"], true);
    env.wait_for("the stop", || !alive(pid));
    let mut shim = shim;
    assert!(shim.answer().is_none(), "the shim ends");
    let (ok, stderr) = shim.close();
    assert!(!ok);
    assert!(stderr.contains("stopped on purpose"), "{stderr}");
    assert!(env.status().is_none(), "no daemon was started again");
    assert!(env.run_dir().join("daemon.stopped").exists());
    // A new session starts one as always, which removes the marker.
    let next = env.shim("/w/s");
    assert!(!env.run_dir().join("daemon.stopped").exists());
    close_ok(next);
}

#[test]
fn a_daemon_of_another_version_after_the_restart_announces_a_new_tool_list() {
    let env = Env::new("10m");
    // A newer daemon serves an older shim; this one says it is newer than this binary.
    let mut newer = env
        .command()
        .args(["daemon", "--foreground", "--version-as", "99.0.0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    env.wait_for("the newer daemon", || env.status().is_some());
    let mut shim = env.shim("/w/v");
    assert_eq!(env.status().unwrap()["daemon_version"], "99.0.0");
    newer.kill().unwrap();
    newer.wait().unwrap();
    // The new daemon is this binary's version: the harness hears that the tools may differ.
    let id = shim.send_request("tools/list", json!({}));
    let deadline = Instant::now() + PATIENCE;
    let mut announced = false;
    loop {
        let line = shim.from.recv_timeout(PATIENCE).unwrap();
        let message: Value = serde_json::from_str(&line).unwrap();
        if message["method"] == "notifications/tools/list_changed" {
            announced = true;
        }
        if message["id"] == json!(id) {
            assert!(message["result"]["tools"].is_array(), "{message}");
            break;
        }
        assert!(Instant::now() < deadline);
    }
    assert!(
        announced,
        "notifications/tools/list_changed before the answer"
    );
    assert_eq!(env.status().unwrap()["daemon_version"], VERSION);
    close_ok(shim);
}

#[test]
fn a_connection_without_a_line_and_doctor_leave_no_refusal_in_the_log() {
    let env = Env::new("10m");
    let shim = env.shim("/w/probe");
    // A bare probe: connect, send nothing, close.
    drop(UnixStream::connect(env.socket()).unwrap());
    // doctor asks the running daemon for the start context (one control request).
    let output = env.command().arg("doctor").output().unwrap();
    assert!(
        output.status.code().is_some(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The daemon still serves, and logged no refusal.
    assert!(env.status().is_some());
    let log = env.log();
    assert!(!log.contains("refused a connection"), "{log}");
    assert!(!log.contains("not JSON"), "{log}");
    // A line that is not JSON is still refused (and logged).
    let answer = env.raw("hello");
    assert_eq!(answer["error"]["code"], "bad_handshake", "{answer}");
    assert!(env.log().contains("refused a connection"), "{}", env.log());
    close_ok(shim);
}

#[test]
fn daemon_stop_stops_it_for_open_sessions_and_says_when_none_runs() {
    let env = Env::new("10m");
    let output = env.command().args(["daemon", "stop"]).output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "daemon: not running"
    );

    let mut shim = env.shim("/w/stop");
    let pid = env.status().unwrap()["pid"].as_u64().unwrap();
    let output = env.command().args(["daemon", "stop"]).output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.starts_with(&format!("daemon: stopped (pid {pid})")),
        "{text}"
    );
    assert!(!alive(pid), "stop returns once the daemon is gone");
    // The open session does not start it again.
    assert!(shim.answer().is_none(), "the shim ends");
    let (ok, stderr) = shim.close();
    assert!(!ok);
    assert!(stderr.contains("stopped on purpose"), "{stderr}");
    assert!(env.status().is_none(), "no daemon was started again");
}

#[test]
fn daemon_restart_starts_a_new_daemon_and_open_sessions_reconnect_to_it() {
    let env = Env::new("10m");
    let mut shim = env.shim("/w/restart");
    shim.remember("kept", "Kept across the restart", "project");
    let pid = env.status().unwrap()["pid"].as_u64().unwrap();
    let output = env.command().args(["daemon", "restart"]).output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.starts_with(&format!("daemon: stopped (pid {pid})")),
        "{text}"
    );
    assert!(text.contains("daemon: running (pid "), "{text}");
    assert!(!alive(pid));
    let new = new_daemon(&env, pid);
    assert!(
        !env.run_dir().join("daemon.stopped").exists(),
        "no stop marker"
    );
    // The open session reconnects to the new daemon by itself.
    let read = shim.ok("memory", json!({"slug": "kept"}));
    assert!(read.contains("Kept across the restart"), "{read}");
    assert_eq!(env.status().unwrap()["pid"].as_u64(), Some(new));
    let (ok, stderr) = shim.close();
    assert!(ok, "{stderr}");
    assert!(stderr.contains("reconnected to the daemon"), "{stderr}");

    // Without a daemon, restart starts one.
    let _ = env.control("stop");
    env.wait_for("the stop", || !alive(new));
    let output = env.command().args(["daemon", "restart"]).output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("daemon: was not running"), "{text}");
    assert!(env.status().is_some());
}
