//! The shim (`graphersal-memory mcp`): what the harness starts, one per agent session. It finds
//! the daemon's socket in the run directory (starting the daemon when none answers), sends the
//! handshake (`crate::protocol`), and then relays bytes between its stdio and the socket. It
//! holds no store and no MCP state.
//!
//! Starting: a shim that cannot connect takes `daemon.start.lock`; holding it, it connects
//! again, and when still nothing answers and no daemon holds `daemon.pid`, it removes a stale
//! socket file and spawns `graphersal-memory daemon --foreground` detached (its own process
//! group on Unix, `DETACHED_PROCESS` on Windows; stderr appended to `daemon.log`), then waits
//! for the socket. A second shim waits on the start lock and then finds the socket. Both give
//! up after the start timeout with an error naming the daemon's log.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::cli::Profile;
use crate::ipc::{self, LockFile, Stream};
use crate::protocol::{self, Refusal};

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

/// The harness that started this process, as far as its environment tells.
pub fn harness_name(env: &dyn Fn(&str) -> Option<String>) -> &'static str {
    if env("CLAUDECODE").is_some() {
        "claude-code"
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

/// A request line (a JSON object with `control`, see `crate::protocol`) to the daemon, which
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
    let handshake = protocol::handshake_line_with(
        options.profile,
        &options.project,
        &options.harness,
        &crate::mcp::canonical(&options.location.data_dir)
            .display()
            .to_string(),
        options.start_context,
    );
    let mut attempt = 0;
    loop {
        attempt += 1;
        let stream = connect_or_start(&options.location)?;
        let (answer, reader) = exchange(
            &stream,
            &handshake,
            options.location.start_timeout,
            &options.location.log_file(),
        )?;
        if answer["ok"] == Value::Bool(true) {
            return Ok((stream, reader));
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

/// What a shim does beside the MCP stream at a session's start and end (`crate::session_sync`):
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
/// the daemon's answers to `output` until the input ends and the daemon has answered; with
/// `events` at the start and end of the session (their notes go to `notes`).
pub fn run_with(
    options: &ShimOptions,
    input: impl Read + Send + 'static,
    output: impl Write,
    events: Option<Events>,
    notes: &mut dyn Write,
) -> Result<(), String> {
    let (stream, reader) = open(options)?;
    let Some(events) = events else {
        return relay(options, stream, reader, input, output);
    };
    let (started, start_done) = std::sync::mpsc::channel();
    let start = events.start;
    let spawned = std::thread::Builder::new()
        .name("shim-start".into())
        .spawn(move || {
            let _ = started.send(start());
        });
    let relayed = relay(options, stream, reader, input, output);
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

/// The byte relay between the harness and the daemon.
fn relay(
    options: &ShimOptions,
    stream: Stream,
    mut reader: BufReader<Stream>,
    input: impl Read + Send + 'static,
    mut output: impl Write,
) -> Result<(), String> {
    let mut to_daemon = stream
        .try_clone()
        .map_err(|err| format!("cannot use the daemon's socket: {err}"))?;
    let input_done = Arc::new(AtomicBool::new(false));
    let done = input_done.clone();
    std::thread::Builder::new()
        .name("shim-input".into())
        .spawn(move || {
            let mut input = input;
            let _ = std::io::copy(&mut input, &mut to_daemon);
            done.store(true, Ordering::SeqCst);
            let _ = to_daemon.shutdown(std::net::Shutdown::Write);
        })
        .map_err(|err| format!("cannot start the input relay: {err}"))?;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let n = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => {
                return Err(format!(
                    "the connection to the daemon broke: {err}; see its log {}",
                    options.location.log_file().display()
                ));
            }
        };
        if output
            .write_all(&buffer[..n])
            .and_then(|()| output.flush())
            .is_err()
        {
            // The harness stopped reading: nothing left to deliver.
            return Ok(());
        }
    }
    if input_done.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err(format!(
            "the daemon closed the connection (it stopped or crashed); the next start of this \
             MCP server starts it again; see its log {}",
            options.location.log_file().display()
        ))
    }
}
