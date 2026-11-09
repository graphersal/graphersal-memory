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

/// Sends one line and reads the one-line answer as JSON.
fn exchange(
    stream: &Stream,
    line: &str,
    timeout: Duration,
) -> Result<(Value, BufReader<Stream>), String> {
    let mut writer = stream
        .try_clone()
        .map_err(|err| format!("cannot use the daemon's socket: {err}"))?;
    writer
        .write_all(line.as_bytes())
        .and_then(|()| writer.write_all(b"\n"))
        .and_then(|()| writer.flush())
        .map_err(|err| format!("cannot talk to the daemon: {err}"))?;
    let reader = stream
        .try_clone()
        .map_err(|err| format!("cannot use the daemon's socket: {err}"))?;
    let _ = reader.set_read_timeout(Some(timeout));
    let mut reader = BufReader::new(reader);
    let mut answer = Vec::new();
    let limit = u64::try_from(protocol::MAX_HANDSHAKE_BYTES).unwrap_or(u64::MAX);
    (&mut reader)
        .take(limit)
        .read_until(b'\n', &mut answer)
        .map_err(|err| format!("the daemon did not answer: {err}"))?;
    let _ = reader.get_ref().set_read_timeout(None);
    if answer.is_empty() {
        return Err("the daemon closed the connection without an answer".to_string());
    }
    let value = serde_json::from_slice(&answer).map_err(|_| {
        "the daemon's answer is not JSON (another program on the socket?)".to_string()
    })?;
    Ok((value, reader))
}

/// A control request (`status`, `stop`) to the running daemon; `None` when none answers.
pub fn control(run_dir: &Path, request: &str) -> Result<Option<Value>, String> {
    let Ok(stream) = ipc::connect(run_dir) else {
        return Ok(None);
    };
    let (answer, _) = exchange(&stream, &protocol::control_line(request), START_TIMEOUT)?;
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
}

/// Connects (starting the daemon if needed) and hands over: the connection, ready for MCP
/// lines, and the reader of the daemon's answers.
pub fn open(options: &ShimOptions) -> Result<(Stream, BufReader<Stream>), String> {
    let handshake = protocol::handshake_line(
        options.profile,
        &options.project,
        &options.harness,
        &crate::mcp::canonical(&options.location.data_dir)
            .display()
            .to_string(),
    );
    let mut attempt = 0;
    loop {
        attempt += 1;
        let stream = connect_or_start(&options.location)?;
        let (answer, reader) = exchange(&stream, &handshake, options.location.start_timeout)?;
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

/// Connects (starting the daemon if needed), hands over, then relays `input` to the daemon and
/// the daemon's answers to `output` until the input ends and the daemon has answered.
pub fn run(
    options: &ShimOptions,
    input: impl Read + Send + 'static,
    mut output: impl Write,
) -> Result<(), String> {
    let (stream, mut reader) = open(options)?;
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
