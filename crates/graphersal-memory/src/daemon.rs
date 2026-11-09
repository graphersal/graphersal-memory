//! The daemon (`graphersal-memory daemon --foreground`): the only process that opens the
//! stores. It listens on the run directory's socket (no network port), routes each connection
//! by its handshake (`crate::protocol`) to the store of its project (`crate::registry`) and the
//! context of its profile, and serves it as one MCP connection (`crate::mcp::StoreRuntime`):
//! several connections at once share one store's session thread.
//!
//! Lifecycle: the daemon holds the `daemon.pid` lock for its whole life (the OS releases it when
//! the process ends, also on a crash), so a socket file without a locked `daemon.pid` is stale.
//! It exits when no connection was open for the idle time (default 2 hours, `--idle` or
//! `GRAPHERSAL_MEMORY_IDLE`), when a control request asks it to stop, or when a newer shim meets
//! it: with no other connection open it exits at once (the shim starts the new binary), else it
//! drains (serves on and exits when its last connection closes). On exit it removes the socket
//! first, then closes every store cleanly.
//!
//! Session indexing (`crate::sessions`): the control request `index_sessions` (from the hooks,
//! `install` and `import --sessions`) queues a run and is answered at once; one indexer thread
//! runs the queued requests (merged while one runs), writing through each project's store in
//! small session-thread jobs, and `sessions_status` reports its progress. The daemon does not
//! exit idle while the indexer works or a store's scheduled-job runs are in progress (the idle
//! time counts from the end of that work).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use graphersal_mcp::Mcp;
use serde_json::json;

use crate::ipc::{self, LockFile, Stream};
use crate::mcp::{self, StoreRuntime};
use crate::protocol::{self, Handshake, Opening, Refusal};
use crate::registry::Registry;
use crate::sessions;

/// The environment variable that sets the idle time when `--idle` is not given.
pub const IDLE_ENV: &str = "GRAPHERSAL_MEMORY_IDLE";

/// The default idle time: two hours (user decision 2026-11-09).
pub const DEFAULT_IDLE: Duration = Duration::from_secs(2 * 60 * 60);

/// How long a client may take to send its first line.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the daemon tries to take the `daemon.pid` lock at start (a shim may be checking it
/// at that moment).
const PID_LOCK_PATIENCE: Duration = Duration::from_secs(1);

/// What the daemon serves.
#[derive(Debug, Clone)]
pub struct DaemonOptions {
    /// The data directory (the stores and the registry).
    pub data_dir: PathBuf,
    /// The run directory (socket, lock files).
    pub run_dir: PathBuf,
    /// Exit after this long without a connection.
    pub idle: Duration,
    /// The version the daemon reports (its own; tests pass an older one).
    pub version: String,
}

/// Parses a duration: a number with the unit `ms`, `s`, `m`, `h` or `d` (`90s`, `30m`, `2h`),
/// or a bare number of seconds.
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: f64 = number
        .parse()
        .map_err(|_| format!("'{text}' is not a duration (examples: 90s, 30m, 2h)"))?;
    let seconds = match unit.trim() {
        "ms" => number / 1000.0,
        "" | "s" => number,
        "m" => number * 60.0,
        "h" => number * 3600.0,
        "d" => number * 86400.0,
        other => {
            return Err(format!(
                "unknown unit '{other}' in '{text}' (use ms, s, m, h or d)"
            ));
        }
    };
    Duration::try_from_secs_f64(seconds)
        .map_err(|_| format!("'{text}' is not a duration (examples: 90s, 30m, 2h)"))
}

/// A diagnostic line on stderr (the daemon log when the shim started the daemon).
pub fn log(message: &str) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(
        stderr,
        "{} [{}] {message}",
        utc_timestamp(now),
        std::process::id()
    );
    let _ = stderr.flush();
}

/// The current time as `YYYY-MM-DDTHH:MM:SSZ`.
pub(crate) fn now_utc() -> String {
    utc_timestamp(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    )
}

/// `seconds` since the epoch as `YYYY-MM-DDTHH:MM:SSZ`.
pub(crate) fn utc_timestamp(seconds: u64) -> String {
    let days = (seconds / 86400) as i64;
    let rest = seconds % 86400;
    // Civil date from days since 1970-01-01 (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The connection bookkeeping behind the idle exit and the drain.
struct State {
    connections: usize,
    /// When the last connection closed (or the daemon started).
    last_change: Instant,
    stopping: bool,
    draining: bool,
}

/// The session indexer's bookkeeping.
#[derive(Default)]
struct Indexer {
    /// Whether the indexer thread runs.
    running: bool,
    /// The request waiting for the next run (merged requests).
    pending: Option<sessions::Request>,
    /// Requests received (each answer names its number).
    requested: u64,
    /// The last request a finished run covered.
    completed: u64,
    /// The report of the current (or last) run.
    report: serde_json::Value,
}

/// The running daemon.
struct Daemon {
    options: DaemonOptions,
    /// The canonical data directory, as handshakes are compared with it.
    data_dir: String,
    socket: PathBuf,
    registry: Registry,
    mcp: Arc<Mcp>,
    stores: Mutex<BTreeMap<String, Arc<StoreRuntime>>>,
    /// The projects recorded in `projects.json` during this daemon's life.
    projects_seen: Mutex<std::collections::BTreeSet<String>>,
    state: Mutex<State>,
    started_at: SystemTime,
    started: Instant,
    shutting_down: AtomicBool,
    indexer: Mutex<Indexer>,
}

/// Runs the daemon in this process until it exits (idle, stop, upgrade): the process ends
/// there. Returns only on a start failure (another daemon runs, the socket cannot be bound).
pub fn run(options: DaemonOptions) -> Result<(), String> {
    std::fs::create_dir_all(&options.data_dir).map_err(|err| {
        format!(
            "cannot create the data directory {}: {err}",
            options.data_dir.display()
        )
    })?;
    ipc::create_run_dir(&options.run_dir)?;
    let socket = ipc::socket_path(&options.run_dir)?;
    let mut pid_file = LockFile::open(&options.run_dir, ipc::PID_FILE)?;
    let deadline = Instant::now() + PID_LOCK_PATIENCE;
    let mut pid_lock = loop {
        if let Some(lock) = pid_file.try_lock()? {
            break lock;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "another daemon already runs in {} (it holds {})",
                options.run_dir.display(),
                ipc::PID_FILE
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let pid = std::process::id();
    {
        let file: &mut std::fs::File = &mut pid_lock;
        let written = file
            .set_len(0)
            .and_then(|()| writeln!(file, "{pid}"))
            .and_then(|()| file.flush());
        if let Err(err) = written {
            log(&format!(
                "cannot write the process id to {}: {err}",
                ipc::PID_FILE
            ));
        }
    }
    // We hold the pid lock: a socket file left here belongs to no running daemon.
    match std::fs::remove_file(&socket) {
        Ok(()) => log(&format!("removed the stale socket {}", socket.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(format!(
                "cannot remove the stale socket {}: {err}",
                socket.display()
            ));
        }
    }
    let registry = Registry::load_or_create(&options.data_dir)?;
    let listener = ipc::bind(&socket)?;
    let daemon = Arc::new(Daemon {
        data_dir: mcp::canonical(&options.data_dir).display().to_string(),
        options,
        socket,
        registry,
        mcp: Arc::new(mcp::endpoint()),
        stores: Mutex::new(BTreeMap::new()),
        projects_seen: Mutex::new(std::collections::BTreeSet::new()),
        state: Mutex::new(State {
            connections: 0,
            last_change: Instant::now(),
            stopping: false,
            draining: false,
        }),
        started_at: SystemTime::now(),
        started: Instant::now(),
        shutting_down: AtomicBool::new(false),
        indexer: Mutex::new(Indexer::default()),
    });
    log(&format!(
        "daemon {} (protocol {}) listening on {}, data {}, idle exit after {}s",
        daemon.options.version,
        protocol::PROTOCOL,
        daemon.socket.display(),
        daemon.data_dir,
        daemon.options.idle.as_secs_f64()
    ));
    let idle = daemon.clone();
    std::thread::Builder::new()
        .name("idle".into())
        .spawn(move || idle.watch_idle())
        .map_err(|err| format!("cannot start the idle watcher: {err}"))?;
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let daemon = daemon.clone();
                let spawned = std::thread::Builder::new()
                    .name("connection".into())
                    .spawn(move || daemon.handle(stream));
                if let Err(err) = spawned {
                    log(&format!("cannot start a connection thread: {err}"));
                }
            }
            Err(err) => {
                log(&format!("accept failed: {err}"));
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    // The listener never ends by itself; keep the pid lock alive until here.
    drop(pid_lock);
    Ok(())
}

/// One open connection; closing it updates the idle clock and ends a drain.
struct Registered<'a>(&'a Daemon);

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        let drained = {
            let mut state = self.0.lock_state();
            state.connections = state.connections.saturating_sub(1);
            state.last_change = Instant::now();
            state.draining && state.connections == 0
        };
        if drained {
            self.0
                .shutdown("drained: its last connection closed (a newer version is waiting)");
        }
    }
}

impl Daemon {
    fn lock_state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_indexer(&self) -> MutexGuard<'_, Indexer> {
        self.indexer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Queues an indexing request; starts the indexer thread when none runs. Returns the
    /// request's number.
    fn queue_indexing(self: &Arc<Self>, request: sessions::Request) -> Result<u64, String> {
        let mut indexer = self.lock_indexer();
        indexer.requested += 1;
        let number = indexer.requested;
        match &mut indexer.pending {
            Some(pending) => pending.merge(request),
            None => indexer.pending = Some(request),
        }
        if !indexer.running {
            indexer.running = true;
            let daemon = self.clone();
            let spawned = std::thread::Builder::new()
                .name("sessions".into())
                .spawn(move || daemon.index_loop());
            if let Err(err) = spawned {
                indexer.running = false;
                return Err(format!("cannot start the session indexer: {err}"));
            }
        }
        Ok(number)
    }

    /// The indexer thread: runs queued requests until none is left.
    fn index_loop(&self) {
        loop {
            let (request, covers) = {
                let mut indexer = self.lock_indexer();
                match indexer.pending.take() {
                    Some(request) => (request, indexer.requested),
                    None => {
                        indexer.running = false;
                        drop(indexer);
                        // The idle time counts from the end of the work.
                        self.lock_state().last_change = Instant::now();
                        return;
                    }
                }
            };
            let (mut summarizer, problem) =
                crate::summaries::Summarizer::new(&self.options.data_dir);
            if let Some(problem) = problem {
                log(&format!(
                    "session summaries are off for this run: {problem}"
                ));
            }
            let mut writer = StoreWriter { daemon: self };
            let report = sessions::run(
                &request,
                &self.options.data_dir,
                &mut writer,
                summarizer.as_mut(),
                &mut |report| self.lock_indexer().report = report.to_json(),
                &|| self.shutting_down.load(Ordering::SeqCst),
            );
            log(&format!(
                "sessions: {} found, {} indexed ({} turns), {} unchanged, {} still running, {} \
                 summarized{}{}",
                report.found,
                report.indexed,
                report.turns,
                report.unchanged,
                report.active,
                report.summarized,
                if report.budget_reached {
                    ", monthly summary budget reached"
                } else {
                    ""
                },
                if report.problems.is_empty() {
                    String::new()
                } else {
                    format!("; problems: {}", report.problems.join(" | "))
                }
            ));
            let mut indexer = self.lock_indexer();
            indexer.report = report.to_json();
            indexer.completed = covers;
        }
    }

    fn sessions_status(&self) -> serde_json::Value {
        let indexer = self.lock_indexer();
        json!({
            "ok": true, "running": indexer.running, "requested": indexer.requested,
            "completed": indexer.completed, "report": indexer.report,
        })
    }

    /// Exits after the idle time without a connection.
    fn watch_idle(&self) {
        let tick = (self.options.idle / 4).clamp(Duration::from_millis(50), Duration::from_secs(5));
        loop {
            std::thread::sleep(tick);
            let indexing = self.lock_indexer().running;
            let background: usize = self
                .stores
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .values()
                .map(|store| store.background_runs())
                .sum();
            let idle = {
                let mut state = self.lock_state();
                if background > 0 {
                    // Scheduled-job runs are work too: the idle time counts from their end.
                    state.last_change = Instant::now();
                }
                let idle = !state.stopping
                    && !indexing
                    && state.connections == 0
                    && state.last_change.elapsed() >= self.options.idle;
                if idle {
                    state.stopping = true;
                }
                idle
            };
            if idle {
                self.shutdown(&format!(
                    "idle for {}s without a connection",
                    self.options.idle.as_secs_f64()
                ));
            }
        }
    }

    /// Ends the process: the socket goes first (new shims start a new daemon once this one's
    /// pid lock is free), then every store is closed cleanly (its job ticker stopped, its
    /// scheduled-job runs finished: `StoreRuntime::close`). The process exit only ends the
    /// accept loop and the connection threads; the stores are closed before it.
    fn shutdown(&self, why: &str) -> ! {
        if self.shutting_down.swap(true, Ordering::SeqCst) {
            // Another thread is closing the stores and ends the process.
            loop {
                std::thread::park();
            }
        }
        self.lock_state().stopping = true;
        log(&format!("stopping: {why}"));
        if let Err(err) = std::fs::remove_file(&self.socket)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            log(&format!("cannot remove {}: {err}", self.socket.display()));
        }
        let stores: Vec<Arc<StoreRuntime>> = self
            .stores
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for store in stores {
            match store.close() {
                Ok(()) => log(&format!("closed the store {}", store.name())),
                Err(err) => log(&err),
            }
        }
        log("stopped");
        std::process::exit(0)
    }

    /// Records a project (its git remote) in `projects.json` the first time it connects in
    /// this daemon's life.
    fn note_project(&self, project: &str) {
        let mut seen = self
            .projects_seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if seen.insert(project.to_string())
            && let Err(err) = crate::projects::record(&self.options.data_dir, project)
        {
            log(&format!("cannot record the project {project}: {err}"));
        }
    }

    /// The runtime of the store that serves `project`, opened on first use.
    fn store_for(&self, project: &str) -> Result<Arc<StoreRuntime>, String> {
        let name = self.registry.store_for(project);
        let mut stores = self
            .stores
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(store) = stores.get(name) {
            return Ok(store.clone());
        }
        let dir = Registry::store_dir(&self.options.data_dir, name);
        let store = Arc::new(StoreRuntime::open(name, &dir, self.mcp.clone())?);
        if let Err(err) = store.start_jobs() {
            log(&err);
        }
        log(&format!("opened the store {name} ({})", dir.display()));
        stores.insert(name.to_string(), store.clone());
        Ok(store)
    }

    fn status(&self) -> serde_json::Value {
        let state = self.lock_state();
        let stores: Vec<String> = self
            .stores
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect();
        json!({
            "ok": true,
            "daemon_version": self.options.version,
            "protocol": protocol::PROTOCOL,
            "pid": std::process::id(),
            "started_at": self.started_at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
            "uptime_secs": self.started.elapsed().as_secs(),
            "connections": state.connections,
            "stores": stores,
            "data_dir": self.data_dir,
            "run_dir": self.options.run_dir.display().to_string(),
            "idle_secs": self.options.idle.as_secs_f64(),
            "draining": state.draining,
        })
    }

    /// Serves one accepted connection.
    fn handle(self: &Arc<Self>, stream: Stream) {
        let mut writer = match stream.try_clone() {
            Ok(writer) => writer,
            Err(err) => {
                log(&format!("cannot use a connection: {err}"));
                return;
            }
        };
        let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
        let mut reader = BufReader::new(stream);
        let mut line = Vec::new();
        let limit = u64::try_from(protocol::MAX_HANDSHAKE_BYTES).unwrap_or(u64::MAX) + 1;
        if let Err(err) = (&mut reader).take(limit).read_until(b'\n', &mut line) {
            log(&format!("no handshake: {err}"));
            return;
        }
        let mut answer = |text: String| {
            let _ = writer
                .write_all(text.as_bytes())
                .and_then(|()| writer.write_all(b"\n"))
                .and_then(|()| writer.flush());
        };
        if line.len() > protocol::MAX_HANDSHAKE_BYTES {
            answer(protocol::refusal(
                Refusal::BadHandshake,
                "the handshake line is longer than 64 KiB",
            ));
            return;
        }
        let handshake = match protocol::parse_opening(&line) {
            Err((kind, message)) => {
                log(&format!(
                    "refused a connection ({}): {message}",
                    kind.code()
                ));
                answer(protocol::refusal(kind, &message));
                return;
            }
            Ok(Opening::Control(control)) => {
                match control.as_str() {
                    "status" => answer(self.status().to_string()),
                    sessions::CONTROL_INDEX => {
                        let request = serde_json::from_slice(&line)
                            .map_err(|err| err.to_string())
                            .and_then(|value| sessions::Request::from_control(&value));
                        match request.and_then(|request| self.queue_indexing(request)) {
                            Ok(run) => answer(json!({"ok": true, "run": run}).to_string()),
                            Err(err) => answer(protocol::refusal(Refusal::BadHandshake, &err)),
                        }
                    }
                    sessions::CONTROL_STATUS => answer(self.sessions_status().to_string()),
                    "stop" => {
                        answer(
                            json!({"ok": true, "daemon_version": self.options.version}).to_string(),
                        );
                        self.shutdown("stop requested");
                    }
                    other => answer(protocol::refusal(
                        Refusal::BadHandshake,
                        &format!(
                            "unknown control request '{other}' (expected status, stop, \
                             index_sessions or sessions_status)"
                        ),
                    )),
                }
                return;
            }
            Ok(Opening::Handshake(handshake)) => handshake,
        };
        if let Some(theirs) = &handshake.data_dir {
            let theirs = mcp::canonical(Path::new(theirs)).display().to_string();
            if theirs != self.data_dir {
                answer(protocol::refusal(
                    Refusal::DataDir,
                    &format!(
                        "this daemon serves the data directory {}, not {theirs}: give the shim \
                         another run directory (--run-dir or {}) or stop this daemon",
                        self.data_dir,
                        ipc::RUN_DIR_ENV
                    ),
                ));
                return;
            }
        }
        let Some(registered) = self.register(&handshake, &mut answer) else {
            return;
        };
        let store = match self.store_for(&handshake.project) {
            Ok(store) => store,
            Err(err) => {
                log(&err);
                answer(protocol::refusal(Refusal::Store, &err));
                return;
            }
        };
        answer(
            json!({"ok": true, "daemon_version": self.options.version, "store": store.name()})
                .to_string(),
        );
        self.note_project(&handshake.project);
        let _ = reader.get_ref().set_read_timeout(None);
        let profile = match handshake.profile {
            crate::cli::Profile::Use => "use",
            crate::cli::Profile::Develop => "develop",
        };
        log(&format!(
            "connection opened: profile {profile}, project {}, harness {}, store {}",
            handshake.project,
            handshake.harness,
            store.name()
        ));
        let served = store.serve(
            handshake.profile,
            &handshake.project,
            Some(&handshake.harness),
            &mut reader,
            &mut writer,
        );
        // Uncounted before the socket closes: a client that saw the connection end (a shim
        // that exited) finds it gone from `status`.
        drop(registered);
        match served {
            Ok(()) => log(&format!("connection closed: project {}", handshake.project)),
            Err(err) => log(&format!(
                "connection of project {} ended: {err}",
                handshake.project
            )),
        }
        drop(reader);
        drop(writer);
    }

    /// Counts a new connection, or refuses it: the daemon is stopping, or it is older than the
    /// shim (then it exits at once without other connections, else it drains).
    fn register(
        &self,
        handshake: &Handshake,
        answer: &mut dyn FnMut(String),
    ) -> Option<Registered<'_>> {
        let mut state = self.lock_state();
        if state.stopping {
            drop(state);
            answer(protocol::refusal(
                Refusal::Stopping,
                "the daemon is stopping; connect again",
            ));
            return None;
        }
        if protocol::older(&self.options.version, &handshake.shim_version) {
            if state.connections == 0 {
                state.stopping = true;
                drop(state);
                answer(protocol::refusal(
                    Refusal::Restarting,
                    &format!(
                        "the daemon {} is older than the shim {}: it exits so the new version \
                         starts; connect again",
                        self.options.version, handshake.shim_version
                    ),
                ));
                self.shutdown(&format!(
                    "a newer shim ({}) connected",
                    handshake.shim_version
                ));
            }
            if !state.draining {
                log(&format!(
                    "a newer shim ({}) connected: draining (exit when the last connection \
                     closes)",
                    handshake.shim_version
                ));
            }
            state.draining = true;
        }
        state.connections += 1;
        Some(Registered(self))
    }
}

/// The indexing's writes: each project's store, its catalog function run on the store's session
/// thread (one short job per call).
struct StoreWriter<'a> {
    daemon: &'a Daemon,
}

impl sessions::Writer for StoreWriter<'_> {
    fn call(
        &mut self,
        project: &str,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        if self.daemon.shutting_down.load(Ordering::SeqCst) {
            return Err("the daemon is stopping".to_string());
        }
        self.daemon.store_for(project)?.call(name, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse_with_units() {
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
        assert_eq!(parse_duration("90").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_duration("1.5s").unwrap(), Duration::from_millis(1500));
        assert_eq!(parse_duration("250ms").unwrap(), Duration::from_millis(250));
        assert_eq!(parse_duration("1d").unwrap(), Duration::from_secs(86400));
        assert!(
            parse_duration("2 weeks")
                .unwrap_err()
                .contains("unknown unit")
        );
        assert!(parse_duration("h").is_err());
        assert!(parse_duration("-1s").is_err());
    }

    #[test]
    fn timestamps_are_utc_iso() {
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_timestamp(1_762_732_800), "2025-11-10T00:00:00Z");
        assert_eq!(utc_timestamp(951_825_599), "2000-02-29T11:59:59Z");
    }
}
