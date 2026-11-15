//! The shim (`graphersal-memory mcp`): what the harness starts, one per agent session. It finds
//! the daemon's socket in the run directory (starting the daemon when none answers), sends the
//! handshake (`crate::runtime::protocol`), and then relays MCP lines between its stdio and the socket. It
//! holds no store; of the MCP stream it keeps only what a reconnect needs.
//!
//! Starting: a shim that cannot connect takes `daemon.start.lock`; holding it, it connects
//! again, and when still nothing answers and no daemon holds `daemon.pid`, it removes a stale
//! socket file and spawns `graphersal-memory daemon --foreground` detached (its own process
//! group on Unix, `DETACHED_PROCESS` on Windows; stderr appended to `daemon.log`), then waits
//! for the socket. A second shim waits on the start lock and then finds the socket. Both give
//! up after the start timeout with an error naming the daemon's log.
//!
//! Reconnecting: a harness marks an MCP server failed for the rest of the session when its
//! process exits, so the shim outlives its daemon (killed, crashed, replaced by an upgrade).
//! The relay is line-aware (MCP stdio framing: one JSON-RPC message per line) and keeps a
//! [`Book`]: the harness's `initialize` request, whether `notifications/initialized` was sent,
//! and the ids of the requests the daemon has not answered yet. When the daemon's connection
//! ends while the harness's input is still open:
//!
//! 1. the writer to the daemon (behind a mutex) is taken away, so harness lines that arrive
//!    meanwhile wait and go to the new connection, none is lost;
//! 2. every request in flight is answered to the harness with a JSON-RPC error
//!    ([`RESTARTED_CODE`]: the memory server restarted, retry the call): its answer is gone with
//!    the old daemon, and the harness must not wait for it;
//! 3. the shim connects again through [`open`] (which starts a daemon from `location.exe` when
//!    none answers), retried until the start timeout; it replays the remembered `initialize`
//!    under a shim-internal id, swallows that answer, and replays `notifications/initialized`;
//! 4. the relay resumes. When the daemon answering now has another version than before, the
//!    harness gets `notifications/tools/list_changed` (a newer daemon may offer other tools; the
//!    harness lists them again, which costs nothing when they are the same); the instructions
//!    of the first `initialize` stay what the harness has (MCP has no way to replace them).
//!
//! When no connection can be made within the timeout the shim ends as before, with an error
//! naming the log (the requests in flight answered first). A daemon stopped on purpose (the
//! `stop` control request of `uninstall` and `purge`, which leave `ipc::STOPPED_FILE`) is not
//! started again: the shim ends as before, so a purge does not see its data recreated.
//!
//! What is per connection in the daemon, and why a reconnect duplicates and loses nothing: the
//! handshake carries the project, profile, harness and start-context mode, so the new
//! connection binds the same project (the daemon resolves it again, to the same id); the start
//! context is computed for the new connection's instructions, which the swallowed `initialize`
//! answer carries and the harness never sees. The session's start and end work ([`Events`]:
//! the memory-file import and the indexing requests, `crate::memory::session_sync`) belongs to the shim,
//! not to a connection: the start runs once after the first handshake and never on a reconnect,
//! the end runs once after the harness's input closed, on whichever daemon then runs.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::cli::Profile;
use crate::runtime::ipc::{self, LockFile, Stream};
use crate::runtime::protocol::{self, Refusal};

/// How long a shim waits for a daemon to start (and to answer the handshake: the first
/// connection to a store opens it). Generous for antivirus scans of a new binary on Windows.
pub const START_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the shim retries after a `restarting` or `stopping` answer.
const RETRIES: usize = 5;

/// How many times one start attempt spawns a daemon that exits at once.
const MAX_SPAWNS: usize = 5;

/// A log file larger than this is moved to `daemon.log.1` before a daemon starts.
const MAX_LOG_BYTES: u64 = 1024 * 1024;

/// Where the daemon is and how to start it.
#[derive(Debug, Clone)]
pub struct DaemonLocation {
    /// The data directory the daemon serves.
    pub data_dir: PathBuf,
    /// The run directory (socket, lock files, log).
    pub run_dir: PathBuf,
    /// The binary to start as the daemon (this one).
    pub exe: PathBuf,
    /// How long to wait for a start.
    pub start_timeout: Duration,
}

impl DaemonLocation {
    fn log_file(&self) -> PathBuf {
        self.run_dir.join(ipc::LOG_FILE)
    }

    fn timeout_error(&self, what: &str) -> String {
        format!(
            "{what} within {}s; see the daemon's log {}",
            self.start_timeout.as_secs(),
            self.log_file().display()
        )
    }
}

/// The environment variable an MCP entry of ours sets to name its harness when the harness
/// itself sets none (CodeWhale's entry, `crate::harnesses::codewhale::install::mcp_entry`).
pub const HARNESS_ENV: &str = "GRAPHERSAL_MEMORY_HARNESS";

/// The harness that started this process, as far as its environment tells: Claude Code's own
/// `CLAUDECODE`, else the harness id our MCP entry put into [`HARNESS_ENV`].
pub fn harness_name(env: &dyn Fn(&str) -> Option<String>) -> &'static str {
    if env("CLAUDECODE").is_some() {
        "claude-code"
    } else if let Some(kind) =
        env(HARNESS_ENV).and_then(|id| crate::harnesses::agent::Kind::from_id(id.trim()))
    {
        kind.id()
    } else {
        "unknown"
    }
}

/// A connection to the running daemon, starting one when none answers.
pub fn connect_or_start(location: &DaemonLocation) -> Result<Stream, String> {
    ipc::create_run_dir(&location.run_dir)?;
    if let Ok(stream) = ipc::connect(&location.run_dir) {
        return Ok(stream);
    }
    let deadline = Instant::now() + location.start_timeout;
    let mut start_lock = LockFile::open(&location.run_dir, ipc::START_LOCK_FILE)?;
    let _starting = loop {
        if let Some(lock) = start_lock.try_lock()? {
            break lock;
        }
        if let Ok(stream) = ipc::connect(&location.run_dir) {
            return Ok(stream);
        }
        if Instant::now() >= deadline {
            return Err(location.timeout_error("another shim did not start the daemon"));
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let mut child: Option<Child> = None;
    let mut spawns = 0;
    let connected = loop {
        if let Ok(stream) = ipc::connect(&location.run_dir) {
            break Ok(stream);
        }
        let running = child
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)));
        if !running && !ipc::daemon_alive(&location.run_dir)? {
            if spawns >= MAX_SPAWNS {
                break Err(format!(
                    "the daemon exited right after its start {spawns} times; see its log {}",
                    location.log_file().display()
                ));
            }
            // Nothing answers and no daemon holds daemon.pid: a socket file here is stale.
            let socket = ipc::socket_path(&location.run_dir)?;
            match std::fs::remove_file(&socket) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    break Err(format!(
                        "cannot remove the stale socket {}: {err}",
                        socket.display()
                    ));
                }
            }
            if let Some(mut old) = child.take() {
                let _ = old.wait();
            }
            child = Some(spawn_daemon(location)?);
            spawns += 1;
        }
        if Instant::now() >= deadline {
            break Err(location.timeout_error("the daemon did not start"));
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    // Reap the daemon when it ends while this process still runs (no zombie).
    if let Some(mut child) = child {
        let _ = std::thread::Builder::new()
            .name("daemon-reaper".into())
            .spawn(move || child.wait());
    }
    connected
}

/// Set once this process saw a daemon stopped on purpose ([`may_start`]).
static SAW_STOPPED: AtomicBool = AtomicBool::new(false);

/// Whether a process that outlives daemons (the shim's reconnect, `ui`) may start a new one:
/// not after a daemon was stopped on purpose (`ipc::STOPPED_FILE`, left by the `stop` control
/// request of `uninstall` and `purge`), for the rest of this process's life (a purge removes the
/// marker with the run directory's other files), so a purge does not see its data recreated.
pub fn may_start(run_dir: &Path) -> bool {
    if ipc::stopped_on_purpose(run_dir) {
        SAW_STOPPED.store(true, Ordering::SeqCst);
    }
    !SAW_STOPPED.load(Ordering::SeqCst)
}

/// A connection for a process that outlives daemons: [`connect_or_start`] when it
/// [`may_start`] one, else only to a daemon that runs.
pub fn connect_again(location: &DaemonLocation) -> Result<Stream, String> {
    if may_start(&location.run_dir) {
        connect_or_start(location)
    } else {
        ipc::connect(&location.run_dir).map_err(|err| {
            format!(
                "the daemon was stopped on purpose and is not running ({err}); start the UI again"
            )
        })
    }
}

/// Spawns `<exe> daemon --foreground` detached, its stderr appended to the log.
fn spawn_daemon(location: &DaemonLocation) -> Result<Child, String> {
    let log_path = location.log_file();
    if std::fs::metadata(&log_path).is_ok_and(|meta| meta.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(
            &log_path,
            location.run_dir.join(format!("{}.1", ipc::LOG_FILE)),
        );
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|err| format!("cannot open the daemon's log {}: {err}", log_path.display()))?;
    let mut command = Command::new(&location.exe);
    command
        .arg("daemon")
        .arg("--foreground")
        .arg("--data-dir")
        .arg(&location.data_dir)
        .arg("--run-dir")
        .arg(&location.run_dir)
        .current_dir(&location.run_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group: a Ctrl-C or a harness killing the shim's group leaves it.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        keep_std_handles_from_children();
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    command.spawn().map_err(|err| {
        format!(
            "cannot start the daemon ({}): {err}",
            location.exe.display()
        )
    })
}

/// Windows: makes this process's standard handles non-inheritable before the daemon is spawned.
///
/// std creates every child with handle inheritance on (`bInheritHandles`), so a child gets every
/// inheritable handle of its parent, not only the three it is given. The standard handles of a
/// shim or a hook are the harness's pipes, inheritable because the harness passed them down; a
/// daemon inheriting them would hold the harness's pipes open for its whole life (hours), and
/// the harness would wait for the end of the output of a process that has long exited. Unix
/// needs nothing: the child's 0, 1 and 2 are replaced, and every other descriptor std opens is
/// close-on-exec. std's `CommandExt::inherit_handles` would do this per child, but it is
/// unstable (rust-lang/rust#146407).
#[cfg(windows)]
#[allow(unsafe_code)]
fn keep_std_handles_from_children() {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetHandleInformation(handle: *mut std::ffi::c_void, mask: u32, flags: u32) -> i32;
    }
    const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;
    let handles = [
        std::io::stdin().as_raw_handle(),
        std::io::stdout().as_raw_handle(),
        std::io::stderr().as_raw_handle(),
    ];
    for handle in handles {
        if !handle.is_null() {
            // SAFETY: SetHandleInformation only changes the inherit flag of a handle this
            // process owns for its whole life (a standard handle); an invalid or pseudo handle
            // makes the call fail, which changes nothing and is ignored.
            unsafe {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

/// The longest one-line answer read from the daemon: a control answer can be large (the
/// `export_memories` answer carries every memory), so it is not held to the handshake's 64 KiB.
pub const MAX_ANSWER_BYTES: usize = 256 * 1024 * 1024;

/// Sends one line and reads the one-line answer as JSON. An answer that is missing, cut off or
/// not JSON means the daemon failed: the error names its `log`.
fn exchange(
    stream: &Stream,
    line: &str,
    timeout: Duration,
    log: &Path,
) -> Result<(Value, BufReader<Stream>), String> {
    let failed = |what: &str| format!("the daemon failed: {what}; see {}", log.display());
    let mut writer = stream
        .try_clone()
        .map_err(|err| format!("cannot use the daemon's socket: {err}"))?;
    writer
        .write_all(line.as_bytes())
        .and_then(|()| writer.write_all(b"\n"))
        .and_then(|()| writer.flush())
        .map_err(|err| failed(&format!("cannot send it the request ({err})")))?;
    let reader = stream
        .try_clone()
        .map_err(|err| format!("cannot use the daemon's socket: {err}"))?;
    let _ = reader.set_read_timeout(Some(timeout));
    let mut reader = BufReader::new(reader);
    let mut answer = Vec::new();
    let limit = u64::try_from(MAX_ANSWER_BYTES).unwrap_or(u64::MAX);
    (&mut reader)
        .take(limit)
        .read_until(b'\n', &mut answer)
        .map_err(|err| {
            failed(&format!(
                "it did not answer within {}s ({err})",
                timeout.as_secs()
            ))
        })?;
    let _ = reader.get_ref().set_read_timeout(None);
    if answer.is_empty() {
        return Err(failed("it closed the connection without an answer"));
    }
    if answer.last() != Some(&b'\n') {
        return Err(failed("its answer ended before the end of the line"));
    }
    let value = serde_json::from_slice(&answer).map_err(|_| failed("its answer is not JSON"))?;
    Ok((value, reader))
}

/// A request line (a JSON object with `control`, see `crate::runtime::protocol`) to the daemon, which
/// is started when none runs; its one-line answer.
pub fn request(location: &DaemonLocation, line: &Value) -> Result<Value, String> {
    let stream = connect_or_start(location)?;
    let (answer, _) = exchange(
        &stream,
        &line.to_string(),
        location.start_timeout,
        &location.log_file(),
    )?;
    Ok(answer)
}

/// A request line to the running daemon; `None` when none answers (nothing is started).
pub fn request_running(run_dir: &Path, line: &Value) -> Result<Option<Value>, String> {
    let Ok(stream) = ipc::connect(run_dir) else {
        return Ok(None);
    };
    let (answer, _) = exchange(
        &stream,
        &line.to_string(),
        START_TIMEOUT,
        &run_dir.join(ipc::LOG_FILE),
    )?;
    Ok(Some(answer))
}

/// A control request (`status`, `stop`) to the running daemon; `None` when none answers.
pub fn control(run_dir: &Path, request: &str) -> Result<Option<Value>, String> {
    let Ok(stream) = ipc::connect(run_dir) else {
        return Ok(None);
    };
    let (answer, _) = exchange(
        &stream,
        &protocol::control_line(request),
        START_TIMEOUT,
        &run_dir.join(ipc::LOG_FILE),
    )?;
    Ok(Some(answer))
}

/// What one shim connects as.
#[derive(Debug, Clone)]
pub struct ShimOptions {
    /// Where the daemon is.
    pub location: DaemonLocation,
    /// The connection's profile.
    pub profile: Profile,
    /// The project key.
    pub project: String,
    /// The harness's name.
    pub harness: String,
    /// Whether the instructions carry the start context.
    pub start_context: protocol::StartContextMode,
}

/// Connects (starting the daemon if needed) and hands over: the connection, ready for MCP
/// lines, and the reader of the daemon's answers.
pub fn open(options: &ShimOptions) -> Result<(Stream, BufReader<Stream>), String> {
    open_with(options, true).map(|opened| (opened.stream, opened.reader))
}

/// A connection after a successful handshake.
pub struct Opened {
    /// The connection, ready for MCP lines.
    pub stream: Stream,
    /// The reader of the daemon's answers.
    pub reader: BufReader<Stream>,
    /// The version of the daemon that answered.
    pub daemon_version: String,
}

/// [`open`] with the daemon's version; `start` false only connects to a daemon that runs.
pub fn open_with(options: &ShimOptions, start: bool) -> Result<Opened, String> {
    let handshake = protocol::handshake_line_with(
        options.profile,
        &options.project,
        &options.harness,
        &crate::runtime::mcp::canonical(&options.location.data_dir)
            .display()
            .to_string(),
        options.start_context,
    );
    let mut attempt = 0;
    loop {
        attempt += 1;
        let stream = if start {
            connect_or_start(&options.location)?
        } else {
            // Without a start: a daemon that runs, or none (no stale-socket cleanup either).
            ipc::connect(&options.location.run_dir)
                .map_err(|err| format!("the daemon is not running ({err})"))?
        };
        let (answer, reader) = exchange(
            &stream,
            &handshake,
            options.location.start_timeout,
            &options.location.log_file(),
        )?;
        if answer["ok"] == Value::Bool(true) {
            let daemon_version = answer["daemon_version"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            return Ok(Opened {
                stream,
                reader,
                daemon_version,
            });
        }
        let code = answer["error"]["code"].as_str().unwrap_or_default();
        let message = answer["error"]["message"]
            .as_str()
            .unwrap_or("no reason given");
        if Refusal::is_retryable_code(code) && attempt < RETRIES {
            // The daemon exits; give it a moment to let go of its socket and lock.
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        return Err(format!("the daemon refused the connection: {message}"));
    }
}

/// What a shim does beside the MCP stream at a session's start and end (`crate::memory::session_sync`):
/// each returns notes for stderr.
pub struct Events {
    /// Runs on a thread of its own once the handshake succeeded.
    pub start: Box<dyn FnOnce() -> Vec<String> + Send>,
    /// Runs after the input ended (the session ended), once `start` finished or
    /// [`END_WAIT`] passed.
    pub end: Box<dyn FnOnce() -> Vec<String> + Send>,
}

/// How long the end waits for the start's work still running (an import of many files).
pub const END_WAIT: Duration = Duration::from_secs(5);

/// Connects (starting the daemon if needed), hands over, then relays `input` to the daemon and
/// the daemon's answers to `output` until the input ends and the daemon has answered,
/// reconnecting when the daemon's connection ends first (see the module docs); with `events`
/// at the start and end of the session (their notes, and a note per reconnect, go to `notes`).
pub fn run_with(
    options: &ShimOptions,
    input: impl Read + Send + 'static,
    output: impl Write,
    events: Option<Events>,
    notes: &mut dyn Write,
) -> Result<(), String> {
    let opened = open_with(options, true)?;
    let Some(events) = events else {
        return relay(options, opened, input, output, notes);
    };
    let (started, start_done) = std::sync::mpsc::channel();
    let start = events.start;
    let spawned = std::thread::Builder::new()
        .name("shim-start".into())
        .spawn(move || {
            let _ = started.send(start());
        });
    let relayed = relay(options, opened, input, output, notes);
    let mut lines = match spawned {
        Ok(_) => start_done.recv_timeout(END_WAIT).unwrap_or_default(),
        Err(err) => vec![format!("cannot start the session start's work: {err}")],
    };
    // The daemon is gone when the relay failed: nothing to queue.
    if relayed.is_ok() {
        lines.extend((events.end)());
    }
    for line in lines {
        let _ = writeln!(notes, "graphersal-memory mcp: {line}");
    }
    relayed
}

/// The JSON-RPC error code of the answer to a request whose daemon went away before it
/// answered (in the implementation-defined server error range).
pub const RESTARTED_CODE: i64 = -32000;

/// The message of that answer.
pub const RESTARTED_MESSAGE: &str = "the graphersal-memory server restarted while this request \
    ran, so its answer was lost; retry the call (a write may or may not have been saved: read \
    it back first)";

/// The id prefix of the `initialize` the shim replays after a reconnect.
const REPLAY_ID_PREFIX: &str = "graphersal-memory-shim/reconnect-";

/// What the relay remembers of the MCP stream for a reconnect (no memory contents beyond the
/// `initialize` request).
#[derive(Debug, Default)]
pub struct Book {
    /// The harness's `initialize` request.
    initialize: Option<serde_json::Map<String, Value>>,
    /// The harness's `notifications/initialized` line.
    initialized: Option<Vec<u8>>,
    /// The requests sent to the daemon and not answered yet: the id's JSON text -> the id.
    in_flight: std::collections::BTreeMap<String, Value>,
    /// How many `initialize` requests were replayed.
    replays: u64,
}

/// The messages of one line: one object, or the objects of a batch.
fn messages(line: &[u8]) -> Vec<serde_json::Map<String, Value>> {
    match serde_json::from_slice::<Value>(line) {
        Ok(Value::Object(message)) => vec![message],
        Ok(Value::Array(batch)) => batch
            .into_iter()
            .filter_map(|item| match item {
                Value::Object(message) => Some(message),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The id of a message, when it has a usable one.
fn id_of(message: &serde_json::Map<String, Value>) -> Option<&Value> {
    message
        .get("id")
        .filter(|id| id.is_string() || id.is_number())
}

impl Book {
    /// A line from the harness, before it goes to the daemon: its requests are in flight, its
    /// `initialize` and `notifications/initialized` remembered.
    pub fn harness_line(&mut self, line: &[u8]) {
        for message in messages(line) {
            let method = message.get("method").and_then(Value::as_str);
            match (method, id_of(&message)) {
                (Some("initialize"), Some(id)) => {
                    self.in_flight.insert(id.to_string(), id.clone());
                    self.initialize = Some(message);
                }
                (Some(_), Some(id)) => {
                    self.in_flight.insert(id.to_string(), id.clone());
                }
                (Some("notifications/initialized"), None) => {
                    let mut text = line.to_vec();
                    if text.last() != Some(&b'\n') {
                        text.push(b'\n');
                    }
                    self.initialized = Some(text);
                }
                // Other notifications and the harness's answers to server requests.
                _ => {}
            }
        }
    }

    /// A line from the daemon: the requests it answers are no longer in flight.
    pub fn daemon_line(&mut self, line: &[u8]) {
        for message in messages(line) {
            let answer = message.get("method").is_none()
                && (message.contains_key("result") || message.contains_key("error"));
            if answer && let Some(id) = id_of(&message) {
                self.in_flight.remove(&id.to_string());
            }
        }
    }

    /// The requests in flight, now forgotten; each to be answered with [`restarted_answer`].
    pub fn take_in_flight(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.in_flight).into_values().collect()
    }

    /// Whether the harness initialized the session (sent `notifications/initialized`).
    pub fn initialized(&self) -> bool {
        self.initialized.is_some()
    }

    /// The lines that open the session again on a new connection: the `initialize` request
    /// under a new shim-internal id (whose answer [`Book::is_replay_answer`] recognises) and
    /// the `notifications/initialized` line; `None` when the harness sent no `initialize`.
    pub fn replay(&mut self) -> Option<(Value, Vec<u8>, Option<Vec<u8>>)> {
        let mut request = self.initialize.clone()?;
        self.replays += 1;
        let id = Value::String(format!("{REPLAY_ID_PREFIX}{}", self.replays));
        request.insert("id".into(), id.clone());
        let mut line = Value::Object(request).to_string().into_bytes();
        line.push(b'\n');
        Some((id, line, self.initialized.clone()))
    }

    /// Whether `line` is the daemon's answer to the replayed request `id`.
    pub fn is_replay_answer(line: &[u8], id: &Value) -> bool {
        messages(line)
            .iter()
            .any(|message| message.get("method").is_none() && message.get("id") == Some(id))
    }
}

/// The answer line (with its line end) to a request `id` the old daemon never answered.
pub fn restarted_answer(id: &Value) -> Vec<u8> {
    let mut line = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": RESTARTED_CODE, "message": RESTARTED_MESSAGE},
    })
    .to_string()
    .into_bytes();
    line.push(b'\n');
    line
}

/// The notification that the tool list may have changed (after a daemon of another version
/// answered a reconnect).
fn tools_changed_line() -> Vec<u8> {
    let mut line = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/tools/list_changed",
    })
    .to_string()
    .into_bytes();
    line.push(b'\n');
    line
}

/// Locks a mutex; a poisoned one is recovered (the data is plain bookkeeping).
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Writes one line to the harness; `false` when the harness stopped reading.
fn deliver(output: &mut impl Write, line: &[u8]) -> bool {
    output.write_all(line).and_then(|()| output.flush()).is_ok()
}

/// The line relay between the harness and the daemon, reconnecting (module docs).
fn relay(
    options: &ShimOptions,
    opened: Opened,
    input: impl Read + Send + 'static,
    mut output: impl Write,
    notes: &mut dyn Write,
) -> Result<(), String> {
    let Opened {
        stream,
        mut reader,
        mut daemon_version,
    } = opened;
    let writer: Arc<Mutex<Option<Stream>>> = Arc::new(Mutex::new(Some(stream)));
    let book = Arc::new(Mutex::new(Book::default()));
    let input_done = Arc::new(AtomicBool::new(false));
    {
        let writer = writer.clone();
        let book = book.clone();
        let done = input_done.clone();
        std::thread::Builder::new()
            .name("shim-input".into())
            .spawn(move || {
                let mut input = BufReader::new(input);
                loop {
                    let mut line = Vec::new();
                    match input.read_until(b'\n', &mut line) {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                    // The writer's lock first: a reconnect holds it, and the line waits for
                    // the new connection (recorded only then, so it is never both answered
                    // as lost and sent).
                    let mut to_daemon = locked(&writer);
                    locked(&book).harness_line(&line);
                    if let Some(stream) = to_daemon.as_mut() {
                        // A failed write: the daemon is gone, and the reconnect answers the
                        // request as lost.
                        let _ = stream.write_all(&line).and_then(|()| stream.flush());
                    }
                }
                done.store(true, Ordering::SeqCst);
                if let Some(stream) = locked(&writer).as_ref() {
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                }
            })
            .map_err(|err| format!("cannot start the input relay: {err}"))?;
    }
    let log = options.location.log_file();
    loop {
        let mut line = Vec::new();
        let broke = match reader.read_until(b'\n', &mut line) {
            Ok(0) => None,
            Ok(_) => {
                locked(&book).daemon_line(&line);
                if !deliver(&mut output, &line) {
                    // The harness stopped reading: nothing left to deliver.
                    return Ok(());
                }
                continue;
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => Some(err),
        };
        // The daemon's connection ended. Hold the writer: harness lines wait from now on.
        let mut to_daemon = locked(&writer);
        to_daemon.take();
        for id in locked(&book).take_in_flight() {
            if !deliver(&mut output, &restarted_answer(&id)) {
                return Ok(());
            }
        }
        if input_done.load(Ordering::SeqCst) {
            // The session ended and the daemon has answered (or is gone): done.
            return Ok(());
        }
        let why = match &broke {
            Some(err) => format!("the connection to the daemon broke: {err}"),
            None => "the daemon closed the connection (it stopped or crashed)".to_string(),
        };
        if !may_start(&options.location.run_dir) {
            return Err(format!(
                "{why}: it was stopped on purpose (uninstall or purge), so it is not started \
                 again; see its log {}",
                log.display()
            ));
        }
        let opened = reconnect(options, &book, &mut output).map_err(|err| {
            format!(
                "{why} and no new daemon answered within {}s ({err}); the next start of this \
                 MCP server starts it again; see its log {}",
                options.location.start_timeout.as_secs(),
                log.display()
            )
        })?;
        let _ = writeln!(
            notes,
            "graphersal-memory mcp: {why}; reconnected to the daemon {}",
            opened.daemon_version
        );
        if opened.daemon_version != daemon_version
            && locked(&book).initialized()
            && !deliver(&mut output, &tools_changed_line())
        {
            return Ok(());
        }
        daemon_version = opened.daemon_version;
        reader = opened.reader;
        *to_daemon = Some(opened.stream);
    }
}

/// Connects again (starting a daemon when none answers), retried until the start timeout, and
/// replays the session's `initialize` and `notifications/initialized`; lines the new daemon
/// sends before the replayed answer go to the harness, the answer itself does not.
fn reconnect(
    options: &ShimOptions,
    book: &Mutex<Book>,
    output: &mut impl Write,
) -> Result<Opened, String> {
    let deadline = Instant::now() + options.location.start_timeout;
    loop {
        let failed = match open_with(options, may_start(&options.location.run_dir)) {
            Ok(opened) => match replay(options, opened, book, output) {
                Ok(opened) => return Ok(opened),
                Err(err) => err,
            },
            Err(err) => err,
        };
        if Instant::now() >= deadline || !may_start(&options.location.run_dir) {
            return Err(failed);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Replays the session's opening on a new connection (nothing when the harness never sent an
/// `initialize`).
fn replay(
    options: &ShimOptions,
    opened: Opened,
    book: &Mutex<Book>,
    output: &mut impl Write,
) -> Result<Opened, String> {
    let Some((id, request, initialized)) = locked(book).replay() else {
        return Ok(opened);
    };
    let Opened {
        stream,
        mut reader,
        daemon_version,
    } = opened;
    let mut writer = stream
        .try_clone()
        .map_err(|err| format!("cannot use the daemon's socket: {err}"))?;
    writer
        .write_all(&request)
        .and_then(|()| writer.flush())
        .map_err(|err| format!("cannot send the session's initialize again: {err}"))?;
    let _ = reader
        .get_ref()
        .set_read_timeout(Some(options.location.start_timeout));
    loop {
        let mut line = Vec::new();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => return Err("the new daemon closed the connection at once".to_string()),
            Ok(_) if Book::is_replay_answer(&line, &id) => break,
            Ok(_) => {
                locked(book).daemon_line(&line);
                // A harness that stopped reading is found by the relay's next write.
                let _ = deliver(output, &line);
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(err) => return Err(format!("the new daemon did not answer initialize: {err}")),
        }
    }
    let _ = reader.get_ref().set_read_timeout(None);
    if let Some(line) = initialized {
        writer
            .write_all(&line)
            .and_then(|()| writer.flush())
            .map_err(|err| format!("cannot send notifications/initialized again: {err}"))?;
    }
    Ok(Opened {
        stream,
        reader,
        daemon_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(value: Value) -> Vec<u8> {
        let mut text = value.to_string().into_bytes();
        text.push(b'\n');
        text
    }

    #[test]
    fn requests_are_in_flight_until_the_daemon_answers_them() {
        let mut book = Book::default();
        book.harness_line(&line(
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        ));
        book.harness_line(&line(
            json!({"jsonrpc": "2.0", "id": "a", "method": "tools/call"}),
        ));
        // Notifications and the harness's answers are not requests.
        book.harness_line(&line(
            json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                                       "params": {"requestId": 1}}),
        ));
        book.harness_line(&line(json!({"jsonrpc": "2.0", "id": 9, "result": {}})));
        book.harness_line(b"not json\n");
        // A server notification answers nothing; an answer with another id neither.
        book.daemon_line(&line(
            json!({"jsonrpc": "2.0", "method": "notifications/progress"}),
        ));
        book.daemon_line(&line(json!({"jsonrpc": "2.0", "id": 7, "result": {}})));
        book.daemon_line(&line(
            json!({"jsonrpc": "2.0", "id": 1, "result": {"tools": []}}),
        ));
        assert_eq!(book.take_in_flight(), vec![json!("a")]);
        assert!(book.take_in_flight().is_empty());
    }

    #[test]
    fn an_error_answer_and_a_batch_count_too() {
        let mut book = Book::default();
        book.harness_line(&line(json!([
            {"jsonrpc": "2.0", "id": 1, "method": "ping"},
            {"jsonrpc": "2.0", "id": 2, "method": "ping"},
            {"jsonrpc": "2.0", "id": 3, "method": "ping"},
        ])));
        book.daemon_line(&line(json!([
            {"jsonrpc": "2.0", "id": 1, "result": {}},
            {"jsonrpc": "2.0", "id": 2, "error": {"code": -1, "message": "x"}},
        ])));
        assert_eq!(book.take_in_flight(), vec![json!(3)]);
    }

    #[test]
    fn the_session_opening_is_replayed_under_a_new_id() {
        let mut book = Book::default();
        assert!(book.replay().is_none(), "no initialize, nothing to replay");
        let init = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
                          "params": {"protocolVersion": "2025-06-18",
                                     "clientInfo": {"name": "claude-code"}}});
        book.harness_line(&line(init.clone()));
        assert!(!book.initialized());
        book.harness_line(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        assert!(book.initialized());
        let (id, request, initialized) = book.replay().unwrap();
        let replayed: Value = serde_json::from_slice(&request).unwrap();
        assert_eq!(replayed["id"], id);
        assert_ne!(id, json!(0));
        assert_eq!(replayed["params"], init["params"]);
        assert_eq!(request.last(), Some(&b'\n'));
        // The line ending is added to a last line that had none.
        assert_eq!(
            initialized.unwrap(),
            b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n".to_vec()
        );
        let (second, _, _) = book.replay().unwrap();
        assert_ne!(second, id, "every replay has its own id");
        assert!(Book::is_replay_answer(
            &line(json!({"jsonrpc": "2.0", "id": id, "result": {}})),
            &id
        ));
        assert!(!Book::is_replay_answer(
            &line(json!({"jsonrpc": "2.0", "id": 0, "result": {}})),
            &id
        ));
        // The harness's own initialize was in flight like any request.
        assert_eq!(book.take_in_flight(), vec![json!(0)]);
    }

    #[test]
    fn a_lost_request_is_answered_with_a_retry_error() {
        let answer: Value = serde_json::from_slice(&restarted_answer(&json!(42))).unwrap();
        assert_eq!(answer["jsonrpc"], "2.0");
        assert_eq!(answer["id"], 42);
        assert_eq!(answer["error"]["code"], RESTARTED_CODE);
        assert!(
            answer["error"]["message"]
                .as_str()
                .unwrap()
                .contains("retry the call")
        );
        let notice: Value = serde_json::from_slice(&tools_changed_line()).unwrap();
        assert_eq!(notice["method"], "notifications/tools/list_changed");
        assert!(notice.get("id").is_none());
    }
}
