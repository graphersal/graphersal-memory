//! The daemon (`graphersal-memory daemon --foreground`): the only process that opens the
//! stores. It listens on the run directory's socket (no network port), routes each connection
//! by its handshake (`crate::runtime::protocol`) to the store of its project (`crate::runtime::registry`) and the
//! context of its profile, and serves it as one MCP connection (`crate::runtime::mcp::StoreRuntime`):
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
//! Session indexing (`crate::memory::sessions`): the control request `index_sessions` (from the hooks,
//! `install` and `import --sessions`) queues a run and is answered at once; one indexer thread
//! runs the queued requests (merged while one runs), writing through each project's store in
//! small session-thread jobs, and `sessions_status` reports its progress. The daemon does not
//! exit idle while the indexer works or a store's scheduled-job runs are in progress (the idle
//! time counts from the end of that work).
//!
//! Store maintenance (`crate::runtime::maintenance`): every store opens with the checkpoint threshold of
//! `store.checkpoint_after`; the default store opens at the daemon's start (beside the first
//! handshake: opening loads the embedding model); a maintenance thread prunes each open store's
//! history older than `store.keep_history` (daily and after each new snapshot); `status` reports
//! each open store's sizes, WAL since the last snapshot and last prune.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use graphersal_mcp::Mcp;
use serde_json::json;

use crate::config::Config;
use crate::memory::sessions;
use crate::runtime::ipc::{self, LockFile, Stream};
use crate::runtime::mcp::{self, StoreRuntime};
use crate::runtime::protocol::{self, Handshake, Opening, Refusal};
use crate::runtime::registry::Registry;

/// The environment variable that sets the idle time when `--idle` is not given.
pub const IDLE_ENV: &str = "GRAPHERSAL_MEMORY_IDLE";

/// The default idle time: two hours (user decision 2026-11-09).
pub const DEFAULT_IDLE: Duration = Duration::from_secs(2 * 60 * 60);

/// How often the maintenance thread looks at the open stores (a check runs only when due).
const MAINTENANCE_TICK: Duration = Duration::from_secs(60);

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

std::thread_local! {
    /// What this thread is doing, for the panic hook's log line (`doing`).
    static ACTIVITY: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Restores the thread's previous activity when dropped (also while a panic unwinds).
pub(crate) struct Doing(Option<String>);

impl Drop for Doing {
    fn drop(&mut self) {
        let previous = self.0.take();
        ACTIVITY.with(|activity| {
            if let Ok(mut activity) = activity.try_borrow_mut() {
                *activity = previous;
            }
        });
    }
}

/// Records what this thread does now (a control request, a catalog function, a session file;
/// never memory contents) until the returned guard is dropped: a panic meanwhile is logged
/// with it.
pub(crate) fn doing(what: impl Into<String>) -> Doing {
    let what = what.into();
    Doing(ACTIVITY.with(|activity| {
        activity
            .try_borrow_mut()
            .map(|mut activity| activity.replace(what))
            .unwrap_or(None)
    }))
}

/// The longest panic message the log keeps.
const MAX_PANIC_TEXT: usize = 200;

/// A panic's message as the log keeps it: cut at the first backtick or quote (std's slicing
/// panics quote the text they were given, which may be memory contents), at most
/// [`MAX_PANIC_TEXT`] characters.
pub(crate) fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    let message = payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".to_string());
    let cut = message
        .find(['`', '"'])
        .map_or(message.as_str(), |at| &message[..at]);
    let cut = cut.trim_end();
    if cut.chars().count() > MAX_PANIC_TEXT {
        let short: String = cut.chars().take(MAX_PANIC_TEXT).collect();
        format!("{short}...")
    } else {
        cut.to_string()
    }
}

/// Logs every panic of the daemon's threads (also the engine's session thread, whose jobs the
/// engine catches) as one line: thread, what it was doing, the shortened message and where.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let activity = ACTIVITY
            .with(|activity| activity.try_borrow().ok().and_then(|a| a.clone()))
            .map(|what| format!(" while {what}"))
            .unwrap_or_default();
        let location = info
            .location()
            .map(|at| format!(" at {}:{}", at.file(), at.line()))
            .unwrap_or_default();
        log(&format!(
            "PANIC (a bug, please report) in thread '{}'{activity}: {}{location}",
            thread.name().unwrap_or("unnamed"),
            panic_text(info.payload())
        ));
    }));
}

/// The test-only injected panic (debug builds only; the release build has no such hook):
/// `GRAPHERSAL_MEMORY_TEST_PANIC=<site>:<name>` panics at `site` (`control`, `job`, `index`)
/// when `name` is the control request, the catalog function, or a part of the session file's
/// path.
#[cfg(debug_assertions)]
pub(crate) fn injected_panic(site: &str, name: &str) {
    let Ok(spec) = std::env::var("GRAPHERSAL_MEMORY_TEST_PANIC") else {
        return;
    };
    let Some((wanted_site, wanted)) = spec.split_once(':') else {
        return;
    };
    let hit = wanted_site == site
        && match site {
            "index" => name.contains(wanted),
            _ => name == wanted,
        };
    if hit {
        #[allow(clippy::panic)]
        {
            panic!("injected test panic at {site}");
        }
    }
}

/// Release builds: no injected panics.
#[cfg(not(debug_assertions))]
pub(crate) fn injected_panic(_site: &str, _name: &str) {}

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
    /// The requests waiting for the next run, one per harness (merged requests).
    pending: BTreeMap<crate::harnesses::agent::Kind, sessions::Request>,
    /// Requests received (each answer names its number).
    requested: u64,
    /// The last request a finished run covered.
    completed: u64,
    /// The report of the current (or last) run.
    report: serde_json::Value,
}

/// The background scan of instruction file rules (`crate::memory::instructions`).
#[derive(Default)]
struct Scanner {
    /// Whether the scanner thread runs.
    running: bool,
    /// The projects whose rules wait for their scan, each once.
    pending: std::collections::BTreeSet<String>,
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
    /// The embedder switches running (`StoreRuntime::switch_embedder`): work for the idle
    /// clock.
    embedding: Arc<std::sync::atomic::AtomicUsize>,
    state: Mutex<State>,
    started_at: SystemTime,
    started: Instant,
    shutting_down: AtomicBool,
    indexer: Mutex<Indexer>,
    scanner: Mutex<Scanner>,
    /// The daemon itself, for the threads a `&self` method starts (the instruction scan).
    me: std::sync::Weak<Daemon>,
}

/// Runs the daemon in this process until it exits (idle, stop, upgrade): the process ends
/// there. Returns only on a start failure (another daemon runs, the socket cannot be bound).
pub fn run(options: DaemonOptions) -> Result<(), String> {
    // Private to the user (Unix: 0700, an existing directory tightened).
    let not_private = crate::runtime::store::create_private_dir(&options.data_dir)?;
    ipc::create_run_dir(&options.run_dir)?;
    install_panic_hook();
    if let Some(note) = not_private {
        log(&note);
    }
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
    // A new daemon runs: open shims and `ui` may reconnect (and start one) again.
    let _ = std::fs::remove_file(options.run_dir.join(ipc::STOPPED_FILE));
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
    let daemon = Arc::new_cyclic(|me| Daemon {
        me: me.clone(),
        data_dir: mcp::canonical(&options.data_dir).display().to_string(),
        options,
        socket,
        registry,
        mcp: Arc::new(mcp::endpoint()),
        stores: Mutex::new(BTreeMap::new()),
        embedding: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
        scanner: Mutex::new(Scanner::default()),
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
    let maintenance = daemon.clone();
    std::thread::Builder::new()
        .name("maintenance".into())
        .spawn(move || maintenance.maintenance_loop())
        .map_err(|err| format!("cannot start the maintenance thread: {err}"))?;
    // The default store opens now, beside the first handshake: opening it loads the embedding
    // model (most of a start: docs/how-it-works.md), so a session that connects a little later
    // (a hook started the daemon) finds it open; one that connects meanwhile waits for it.
    let warm = daemon.clone();
    let warming = std::thread::Builder::new()
        .name("open".into())
        .spawn(move || {
            if let Err(err) = warm.open_store(crate::runtime::registry::DEFAULT_STORE) {
                log(&err);
            }
        });
    if let Err(err) = warming {
        log(&format!(
            "cannot open the default store in the background: {err}"
        ));
    }
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let daemon = daemon.clone();
                let spawned =
                    std::thread::Builder::new()
                        .name("connection".into())
                        .spawn(move || {
                            // A panic is logged by the hook; the connection ends, the daemon serves on.
                            let served =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    daemon.handle(stream)
                                }));
                            if served.is_err() {
                                log("a connection ended by a panic (logged above); serving on");
                            }
                        });
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
        match indexer.pending.get_mut(&request.harness) {
            Some(pending) => pending.merge(request),
            None => {
                indexer.pending.insert(request.harness, request);
            }
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
                match indexer.pending.pop_first() {
                    // A run covers the requests so far only when it is the last pending one.
                    Some((_, request)) => {
                        let covers = if indexer.pending.is_empty() {
                            indexer.requested
                        } else {
                            indexer.completed
                        };
                        (request, covers)
                    }
                    None => {
                        indexer.running = false;
                        drop(indexer);
                        // The idle time counts from the end of the work.
                        self.lock_state().last_change = Instant::now();
                        return;
                    }
                }
            };
            let (summarizer, problem) =
                crate::memory::summaries::Summarizer::new(&self.options.data_dir);
            // A sandboxed run may use copies of the settings and logins its harness keeps
            // (opencode's logins, Codex's model settings and login), found in this process's
            // environment.
            let mut summarizer = summarizer.map(|summarizer| {
                summarizer.with_user_env(std::sync::Arc::new(|name: &str| std::env::var(name).ok()))
            });
            if let Some(problem) = problem {
                log(&format!(
                    "session summaries are off for this run: {problem}"
                ));
            }
            let mut writer = StoreWriter { daemon: self };
            // `sessions::run` catches a panic per session file; one outside them (a bug in the
            // scan) ends this run only: the indexer goes on with the next request.
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _doing = doing(format!(
                    "indexing the {} sessions",
                    request.harness.display_name()
                ));
                sessions::run(
                    &request,
                    &self.options.data_dir,
                    &mut writer,
                    summarizer.as_mut(),
                    &mut |report| self.lock_indexer().report = report.to_json(),
                    &|| self.shutting_down.load(Ordering::SeqCst),
                )
            }));
            let report = ran.unwrap_or_else(|_| sessions::Report {
                phase: "done".into(),
                problems: vec![format!(
                    "the {} session indexing failed (a bug, logged above); the next request \
                     tries again",
                    request.harness.display_name()
                )],
                ..sessions::Report::default()
            });
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
            let indexing = self.lock_indexer().running
                || self.lock_scanner().running
                || self.embedding.load(Ordering::SeqCst) > 0;
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

    /// The stable id of the project at `path` (`crate::memory::projects::resolve`: by its git remote,
    /// else its path; a new project is recorded). A projects file that cannot be read leaves
    /// the path as the id (logged).
    fn resolve_project(&self, path: &str) -> String {
        match crate::memory::projects::resolve(&self.options.data_dir, path) {
            Ok(resolved) => {
                if resolved.created && !resolved.look_alikes.is_empty() {
                    log(&format!(
                        "new project {path} has the first commit of {}: if it moved, \
                         `graphersal-memory projects merge` keeps its memories",
                        resolved.look_alikes.join(", ")
                    ));
                }
                resolved.id
            }
            Err(err) => {
                log(&format!("cannot resolve the project {path}: {err}"));
                path.to_string()
            }
        }
    }

    /// Makes `store` use the embedding model of the settings file (`embedding.model`), on a
    /// thread of its own: the import and the re-embedding never hold up a connection's
    /// handshake (MCP calls wait while the store re-embeds). Without the setting nothing
    /// changes.
    fn apply_embedder(&self, store: Arc<StoreRuntime>) {
        let wanted = match Config::load(&self.options.data_dir) {
            Ok(config) => config.model,
            Err(err) => {
                log(&err);
                None
            }
        };
        let Some(wanted) = wanted else {
            return;
        };
        let data_dir = self.options.data_dir.clone();
        let running = self.embedding.clone();
        let me = self.me.clone();
        running.fetch_add(1, Ordering::SeqCst);
        let spawned = std::thread::Builder::new()
            .name("embedder".into())
            .spawn(move || {
                let switched = store.switch_embedder(&data_dir, &wanted);
                // With a model (now, or already) the lines synced while the check was off wait
                // for their scan: no manual step.
                if switched.is_ok()
                    && let Some(daemon) = me.upgrade()
                {
                    daemon.queue_backlog(&store);
                }
                match switched {
                    Ok(true) => log(&format!(
                        "the store {} uses the embedding model {wanted} now (re-embedded)",
                        store.name()
                    )),
                    Ok(false) => {}
                    Err(err) => log(&format!(
                        "the store {} keeps its embedder: {err}",
                        store.name()
                    )),
                }
                running.fetch_sub(1, Ordering::SeqCst);
            });
        if let Err(err) = spawned {
            self.embedding.fetch_sub(1, Ordering::SeqCst);
            log(&format!("cannot start the embedder switch: {err}"));
        }
    }

    /// `apply_settings`: the settings file changed (install): every open store switches to the
    /// embedding model it names.
    fn apply_settings(&self) -> serde_json::Value {
        let stores: Vec<Arc<StoreRuntime>> = self
            .stores
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        let count = stores.len();
        for store in stores {
            self.apply_embedder(store);
        }
        json!({"ok": true, "stores": count})
    }

    /// `merge_projects`: moves every memory, session, turn and fact of project `from` into
    /// project `into` (the internal function `merge_project` on the store's session thread),
    /// then moves `from`'s lookup keys to `into` in `projects.json`.
    fn merge_projects(&self, line: &[u8]) -> Result<serde_json::Value, String> {
        let request: serde_json::Value =
            serde_json::from_slice(line).map_err(|err| err.to_string())?;
        let from = request["from"].as_str().unwrap_or_default();
        let into = request["into"].as_str().unwrap_or_default();
        if from.is_empty() || into.is_empty() || from == into {
            return Err("merge_projects needs two different project ids, from and into".into());
        }
        let projects = crate::memory::projects::Projects::load(&self.options.data_dir)?;
        for id in [from, into] {
            if projects.get(id).is_none() {
                return Err(format!(
                    "no project {id} (`graphersal-memory projects list`)"
                ));
            }
        }
        let store = self.store_for(into)?;
        if self.registry.store_for(from) != store.name() {
            return Err(format!("{from} and {into} live in different stores"));
        }
        let moved = store.call("merge_project", json!({"from": from, "into": into}))?;
        crate::memory::projects::merge_records(&self.options.data_dir, from, into)?;
        log(&format!("merged project {from} into {into}: {moved}"));
        Ok(json!({"ok": true, "merged": moved}))
    }

    /// `start_context`: the start context of a project root (`crate::memory::start_context`), within
    /// `memory.start_context_chars` and the request's `max_chars` (the hooks: what their
    /// harness takes whole).
    fn start_context(&self, line: &[u8]) -> Result<serde_json::Value, String> {
        let request: serde_json::Value =
            serde_json::from_slice(line).map_err(|err| err.to_string())?;
        // `project_id` (the browser UI): a stable id as `projects.json` lists it, used as it
        // is (no lookup, nothing recorded); else `project`, a root resolved to its id.
        let project = match request["project_id"].as_str() {
            Some(id) => id.to_string(),
            None => {
                let root = request["project"].as_str().unwrap_or_default();
                if root.is_empty() {
                    String::new()
                } else {
                    self.resolve_project(root)
                }
            }
        };
        let mut budget = Config::load(&self.options.data_dir)?.start_context_chars();
        if let Some(max) = request["max_chars"].as_u64() {
            budget = budget.min(usize::try_from(max).unwrap_or(usize::MAX));
        }
        let store = self.store_for(&project)?;
        let context = store
            .start_context(&project, budget)
            .inspect_err(|_| self.check_alive(&store))?;
        let mut answer = context.to_json();
        // `entries` (the browser UI): every always-loaded memory in the selection order, each
        // marked shown or cut for lack of room.
        if request["entries"] == true {
            let entries = store
                .start_entries(&project)
                .inspect_err(|_| self.check_alive(&store))?;
            answer["entries"] = crate::memory::start_context::entries_json(entries, context.shown);
            answer["project_id"] = serde_json::Value::String(project);
        }
        Ok(answer)
    }

    /// `sync_instructions` (`crate::memory::instructions`): the rules of the instruction files the
    /// request names become the project's `instruction` vertices, and the answer follows right
    /// after; the nearest memories of the new rules are looked up in the background
    /// ([`Self::queue_scan`]), so a session start never waits for that.
    fn sync_instructions(self: &Arc<Self>, line: &[u8]) -> Result<serde_json::Value, String> {
        let request: serde_json::Value =
            serde_json::from_slice(line).map_err(|err| err.to_string())?;
        let root = request["project"].as_str().unwrap_or_default();
        let project = if root.is_empty() {
            String::new()
        } else {
            self.resolve_project(root)
        };
        let files = request["files"].as_array().cloned().unwrap_or_default();
        let (rules, notes) = crate::memory::instructions::read_rules(&files, root, &project);
        let store = self.store_for(&project)?;
        let synced = store
            .call(
                "sync_instructions",
                json!({"project": project, "rules": rules}),
            )
            .inspect_err(|_| self.check_alive(&store))?;
        let pending = synced["pending"].as_u64().unwrap_or(0);
        if pending > 0 {
            self.queue_scan(&project);
        }
        Ok(
            json!({"ok": true, "rules": synced["rules"], "created": synced["created"],
                  "updated": synced["updated"], "removed": synced["removed"],
                  "pending": pending, "notes": notes, "project_id": project}),
        )
    }

    /// Queues the candidate scan of `project`'s new rules (a project already queued is not
    /// queued twice) and starts the scanner thread when none runs.
    fn queue_scan(&self, project: &str) {
        let Some(daemon) = self.me.upgrade() else {
            return;
        };
        let mut scanner = self.lock_scanner();
        scanner.pending.insert(project.to_string());
        if !scanner.running {
            scanner.running = true;
            let spawned = std::thread::Builder::new()
                .name("instructions".into())
                .spawn(move || daemon.scan_loop());
            if let Err(err) = spawned {
                scanner.running = false;
                log(&format!("cannot start the instruction file scan: {err}"));
            }
        }
    }

    /// Queues the scan of every project of `store` whose instruction lines wait for it (the
    /// catalog function `instruction_backlog`; `""` for user-level lines).
    fn queue_backlog(&self, store: &StoreRuntime) {
        match store.call("instruction_backlog", json!({})) {
            Ok(projects) => {
                for project in projects.as_array().into_iter().flatten() {
                    if let Some(project) = project.as_str() {
                        self.queue_scan(project);
                    }
                }
            }
            Err(err) => log(&format!("cannot list the instruction lines to scan: {err}")),
        }
    }

    fn lock_scanner(&self) -> MutexGuard<'_, Scanner> {
        self.scanner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The scanner thread: one project at a time, `scan_instructions` batches (each one job on
    /// the store's session thread, so other calls wait for one batch at most) for up to
    /// [`crate::memory::instructions::SCAN_BUDGET`] per round; a project with rules left goes to the
    /// back of the queue for another round, until none is left or the candidates are capped
    /// (then the next sync queues it again).
    fn scan_loop(&self) {
        loop {
            let project = {
                let mut scanner = self.lock_scanner();
                match scanner.pending.pop_first() {
                    Some(project) => project,
                    None => {
                        scanner.running = false;
                        return;
                    }
                }
            };
            if self.shutting_down.load(Ordering::SeqCst) {
                continue;
            }
            let round = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _doing = doing("scanning the instruction files of a project");
                self.scan_round(&project)
            }));
            match round {
                Ok(Ok(true)) => {
                    self.lock_scanner().pending.insert(project);
                }
                Ok(Ok(false)) => {}
                Ok(Err(err)) => log(&format!("the instruction file scan failed: {err}")),
                Err(_) => log(&format!(
                    "the instruction file scan failed (a bug); see {}",
                    self.log_file().display()
                )),
            }
        }
    }

    /// One scan round of `project`: whether rules are left for another round.
    fn scan_round(&self, project: &str) -> Result<bool, String> {
        let store = self.store_for(project)?;
        let deadline = Instant::now() + crate::memory::instructions::SCAN_BUDGET;
        loop {
            let scan = store
                .call(
                    "scan_instructions",
                    json!({"project": project, "limit": crate::memory::instructions::SCAN_BATCH}),
                )
                .inspect_err(|_| self.check_alive(&store))?;
            let remaining = scan["remaining"].as_u64().unwrap_or(0);
            // `off`: no embedding model, no check (`crate::runtime::catalog::instruction_check`).
            if remaining == 0 || scan["capped"] == true || scan["off"] == true {
                return Ok(false);
            }
            if Instant::now() >= deadline || self.shutting_down.load(Ordering::SeqCst) {
                return Ok(true);
            }
        }
    }

    /// `export_memories`: every memory (or one project's and the global ones) with what the
    /// file memory format keeps (`uninstall`, `export --to-files`).
    fn export_memories(&self, line: &[u8]) -> Result<serde_json::Value, String> {
        let request: serde_json::Value =
            serde_json::from_slice(line).map_err(|err| err.to_string())?;
        let project = request["project"].as_str().unwrap_or_default();
        let store = self.store_for(project)?;
        let memories = store
            .call("export_memories", json!({"project": project}))
            .inspect_err(|_| self.check_alive(&store))?;
        Ok(json!({"ok": true, "memories": memories}))
    }

    /// The runtime of the store that serves `project`, opened on first use.
    fn store_for(&self, project: &str) -> Result<Arc<StoreRuntime>, String> {
        let name = self.registry.store_for(project).to_string();
        self.open_store(&name)
    }

    /// The runtime of the store `name`, opened on first use.
    fn open_store(&self, name: &str) -> Result<Arc<StoreRuntime>, String> {
        let mut stores = self
            .stores
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(store) = stores.get(name) {
            return Ok(store.clone());
        }
        let dir = Registry::store_dir(&self.options.data_dir, name);
        let settings = crate::runtime::maintenance::Settings::load(&self.options.data_dir);
        let started = Instant::now();
        let store = Arc::new(StoreRuntime::open_with(
            name,
            &dir,
            self.mcp.clone(),
            &settings,
        )?);
        let opened_in = started.elapsed();
        if let Err(err) = store.start_jobs() {
            log(&err);
        }
        log(&format!(
            "opened the store {name} ({}) in {:.2}s",
            dir.display(),
            opened_in.as_secs_f64()
        ));
        if name == crate::runtime::registry::DEFAULT_STORE {
            let call = |function: &str, args: serde_json::Value| store.call(function, args);
            match crate::harnesses::claude_code::import::migrate_sources(
                &self.options.data_dir,
                &call,
            ) {
                Ok(0) => {}
                Ok(n) => log(&format!(
                    "marked {n} memories imported before the import recorded their files"
                )),
                Err(err) => log(&format!("the import source migration failed: {err}")),
            }
        }
        {
            let call = |function: &str, args: serde_json::Value| store.call(function, args);
            match crate::memory::instructions::rescan_once(&self.options.data_dir, name, &call) {
                Ok(0) => {}
                Ok(n) => log(&format!(
                    "dropped {n} unchecked memory/instruction-line pairs of the store {name}; \
                     its instruction lines are scanned again under the new rules"
                )),
                Err(err) => log(&format!("the instruction line rescan failed: {err}")),
            }
        }
        stores.insert(name.to_string(), store.clone());
        drop(stores);
        self.apply_embedder(store.clone());
        Ok(store)
    }

    fn status(&self) -> serde_json::Value {
        let open: Vec<Arc<StoreRuntime>> = self
            .stores
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        let stores: Vec<String> = open.iter().map(|store| store.name().to_string()).collect();
        let maintenance: serde_json::Map<String, serde_json::Value> = open
            .iter()
            .map(|store| (store.name().to_string(), store.maintenance_status()))
            .collect();
        let state = self.lock_state();
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
            "maintenance": maintenance,
        })
    }

    /// The maintenance thread: checks every open store's history each minute and prunes what
    /// is older than `store.keep_history` when a check is due (`maintenance::State::due`: daily,
    /// and after each new snapshot). Never on a connection's thread; a prune runs beside the
    /// sessions (the engine prunes without the graph's lock).
    fn maintenance_loop(&self) {
        loop {
            std::thread::sleep(MAINTENANCE_TICK);
            if self.shutting_down.load(Ordering::SeqCst) {
                return;
            }
            let stores: Vec<Arc<StoreRuntime>> = self
                .stores
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .values()
                .cloned()
                .collect();
            for store in stores {
                self.maintain(&store);
            }
        }
    }

    /// One history check of `store`, when due.
    fn maintain(&self, store: &StoreRuntime) {
        let settings = crate::runtime::maintenance::Settings::load(&self.options.data_dir);
        let Some(keep) = settings.keep_history else {
            return;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let newest = match store.newest_snapshot() {
            Ok(newest) => newest,
            Err(err) => {
                log(&err);
                return;
            }
        };
        let mut state = crate::runtime::maintenance::State::load(store.dir());
        if !state.due(now, newest) || self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        match store.prune_old_history(keep, crate::runtime::maintenance::now_micros()) {
            Ok(Some(pruned)) => {
                let text = pruned.text();
                log(&format!(
                    "store {}: history older than {}: {text}",
                    store.name(),
                    crate::runtime::maintenance::duration_text(keep)
                ));
                state.last_prune = Some((now, text));
            }
            Ok(None) => {}
            Err(err) => {
                // Checked again at the next tick (the state stays as it was).
                log(&format!("store {}: {err}", store.name()));
                return;
            }
        }
        state.last_check = Some(now);
        state.newest_snapshot = newest;
        if let Err(err) = state.save(store.dir()) {
            log(&err);
        }
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
        if line.is_empty() {
            // Closed before its first line (a probe that only checks that the socket answers):
            // nothing to refuse and nothing worth a log line.
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
                // A panic while answering is a bug: logged (the hook), answered as a refusal,
                // and the daemon serves on.
                let answered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _doing = doing(format!("answering the control request '{control}'"));
                    self.control(&control, &line)
                }));
                match answered {
                    Ok(text) => {
                        // `restart` stops like `stop` but leaves no marker: open shims start
                        // (and reconnect to) a new daemon, which runs the binary installed now.
                        let stops = control == "stop" || control == "restart";
                        if control == "stop" {
                            // Before the answer and the socket's end: an open shim that sees
                            // its connection end finds it and does not start a new daemon.
                            let marker = self.options.run_dir.join(ipc::STOPPED_FILE);
                            if let Err(err) = std::fs::write(&marker, "stop requested\n") {
                                log(&format!("cannot write {}: {err}", marker.display()));
                            }
                        }
                        answer(text);
                        if stops {
                            self.shutdown(if control == "stop" {
                                "stop requested"
                            } else {
                                "restart requested"
                            });
                        }
                    }
                    Err(_) => answer(protocol::refusal(
                        Refusal::Internal,
                        &format!(
                            "the daemon failed while answering '{control}' (a bug); see {}",
                            self.log_file().display()
                        ),
                    )),
                }
                return;
            }
            Ok(Opening::Relay(frame)) => {
                let answered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _doing = doing(format!(
                        "relaying {} {}",
                        frame.method,
                        path_of(&frame.target)
                    ));
                    self.relay(&frame, &mut reader)
                }));
                let (head, body) = match answered {
                    Ok(Ok((status, headers, body))) => (
                        crate::ui::relay::answer_line(status, &headers, body.len()),
                        body,
                    ),
                    Ok(Err((kind, message))) => (protocol::refusal(kind, &message), Vec::new()),
                    Err(_) => (
                        protocol::refusal(
                            Refusal::Internal,
                            &format!(
                                "the daemon failed while answering the UI (a bug); see {}",
                                self.log_file().display()
                            ),
                        ),
                        Vec::new(),
                    ),
                };
                let _ = writer
                    .write_all(head.as_bytes())
                    .and_then(|()| writer.write_all(b"\n"))
                    .and_then(|()| writer.write_all(&body))
                    .and_then(|()| writer.flush());
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
        let project = self.resolve_project(&handshake.project);
        let store = match self.store_for(&project) {
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
        let _ = reader.get_ref().set_read_timeout(None);
        let profile = match handshake.profile {
            crate::cli::Profile::Use => "use",
            crate::cli::Profile::Develop => "develop",
        };
        log(&format!(
            "connection opened: profile {profile}, project {project}{}, harness {}, store {}",
            if project == handshake.project {
                String::new()
            } else {
                format!(" (at {})", handshake.project)
            },
            handshake.harness,
            store.name()
        ));
        let opened = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let config = Config::load(&self.options.data_dir).unwrap_or_default();
        let connection = mcp::Connection {
            language: config.language(),
            start: handshake.start_context,
            start_chars: config.start_context_chars(),
        };
        let served = store.serve_with(
            handshake.profile,
            &project,
            Some(&handshake.harness),
            connection,
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
        if handshake.profile == crate::cli::Profile::Use
            && let Some(kind) = crate::harnesses::agent::Kind::from_id(&handshake.harness)
        {
            self.end_sessions_with_connection(kind, &handshake.project, opened);
        }
    }

    /// The end of a `use` connection of a harness whose sessions end with it
    /// (`SessionSource::ended_by_connection`: CodeWhale keeps its MCP servers for its whole run
    /// and kills them when it exits, before our shim can send the session end, seen live with
    /// v0.10.2), opened at `opened` (seconds) for the project at `root`: the daemon queues the
    /// indexing itself, with that project's sessions written since then marked ended. Where
    /// the sessions are comes from this process's environment.
    fn end_sessions_with_connection(
        self: &Arc<Self>,
        kind: crate::harnesses::agent::Kind,
        root: &str,
        opened: u64,
    ) {
        if root.is_empty() {
            return;
        }
        let reader = crate::harnesses::sessions(kind);
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let Some(dir) = reader.ended_by_connection(&env) else {
            return;
        };
        let mut request = sessions::Request::for_harness(kind, &dir);
        request
            .ended
            .extend(reader.written_since(&dir, Path::new(root), opened));
        if request.ended.is_empty() {
            return;
        }
        if let Err(err) = self.queue_indexing(request) {
            log(&format!(
                "cannot queue {}'s ended sessions: {err}",
                kind.display_name()
            ));
        }
    }

    /// Answers one relayed HTTP request of the browser UI (`crate::ui::relay`): its body read, then
    /// the default store's relay context. The answer is the status, headers and body, or the
    /// refusal to send.
    fn relay(
        &self,
        frame: &crate::ui::relay::Frame,
        reader: &mut BufReader<Stream>,
    ) -> Result<RelayAnswer, (Refusal, String)> {
        let body = crate::ui::relay::read_body(reader, frame.body_length)
            .map_err(|err| (Refusal::BadHandshake, err))?;
        if self.lock_state().stopping {
            return Err((
                Refusal::Stopping,
                "the daemon is stopping; start the UI again".to_string(),
            ));
        }
        let store = self.store_for("").map_err(|err| (Refusal::Store, err))?;
        let mut request = graphersal_serve::Request::new(
            graphersal_serve::Method::parse(&frame.method),
            &frame.target,
            body.as_slice(),
        )
        .with_body_length(Some(body.len()));
        for (name, value) in &frame.headers {
            request = request.with_header(name, value);
        }
        let response = store
            .relay(frame.port, request)
            .map_err(|err| (Refusal::Store, err))?;
        if response.status() >= 500 {
            self.check_alive(&store);
        }
        Ok(response.into_parts())
    }

    /// After a failed call: when the store's session thread no longer runs (it ended by a
    /// panic outside a job, which the engine does not catch), the daemon exits cleanly, so
    /// the next shim starts a new one with a fresh session (reopening the store in place
    /// would race the connections that still hold the old one).
    fn check_alive(&self, store: &StoreRuntime) {
        if !store.alive() {
            self.shutdown(&format!(
                "the session thread of the store {} ended (a bug, logged above); the next \
                 session starts a new daemon",
                store.name()
            ));
        }
    }

    /// The daemon's log file (the shim appends the daemon's stderr to it).
    fn log_file(&self) -> PathBuf {
        self.options.run_dir.join(ipc::LOG_FILE)
    }

    /// `err` with the daemon's log named, when the daemon failed (a bug) rather than the
    /// request.
    fn with_log(&self, err: &str) -> String {
        if err.starts_with("the daemon failed") {
            format!("{err}; see {}", self.log_file().display())
        } else {
            err.to_string()
        }
    }

    /// The answer line to the control request `control` (its whole first `line`).
    fn control(self: &Arc<Self>, control: &str, line: &[u8]) -> String {
        injected_panic("control", control);
        match control {
            "status" => self.status().to_string(),
            sessions::CONTROL_INDEX => {
                let request = serde_json::from_slice(line)
                    .map_err(|err| err.to_string())
                    .and_then(|value| sessions::Request::from_control(&value));
                match request.and_then(|request| self.queue_indexing(request)) {
                    Ok(run) => json!({"ok": true, "run": run}).to_string(),
                    Err(err) => protocol::refusal(Refusal::BadHandshake, &err),
                }
            }
            sessions::CONTROL_STATUS => self.sessions_status().to_string(),
            "apply_settings" => self.apply_settings().to_string(),
            "export_memories" => match self.export_memories(line) {
                Ok(done) => done.to_string(),
                Err(err) => protocol::refusal(Refusal::Store, &self.with_log(&err)),
            },
            "merge_projects" => match self.merge_projects(line) {
                Ok(done) => done.to_string(),
                Err(err) => protocol::refusal(Refusal::Store, &self.with_log(&err)),
            },
            "start_context" => match self.start_context(line) {
                Ok(done) => done.to_string(),
                Err(err) => protocol::refusal(Refusal::Store, &self.with_log(&err)),
            },
            crate::memory::instructions::CONTROL => match self.sync_instructions(line) {
                Ok(done) => done.to_string(),
                Err(err) => protocol::refusal(Refusal::Store, &self.with_log(&err)),
            },
            // Answered first; `handle` then shuts the daemon down (`stop` leaves the marker
            // `ipc::STOPPED_FILE`, `restart` does not).
            "stop" | "restart" => {
                json!({"ok": true, "daemon_version": self.options.version, "pid": std::process::id()})
                    .to_string()
            }
            other => protocol::refusal(
                Refusal::BadHandshake,
                &format!(
                    "unknown control request '{other}' (expected status, stop, restart, \
                     index_sessions, sessions_status, apply_settings, export_memories, merge_projects, \
                     start_context or sync_instructions)"
                ),
            ),
        }
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

/// A relayed answer: status, headers, body.
type RelayAnswer = (u16, Vec<(String, String)>, Vec<u8>);

/// The path of a request target, without its query (the log never shows parameters).
fn path_of(target: &str) -> &str {
    target.split('?').next().unwrap_or(target)
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
        let store = self.daemon.store_for(project)?;
        let called = store.call(name, args);
        if called.is_err() {
            self.daemon.check_alive(&store);
        }
        called
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
    fn panic_texts_are_cut_before_quoted_text_and_keep_characters_whole() {
        let payload: Box<dyn std::any::Any + Send> = Box::new(
            "byte index 5 is not a char boundary; it is inside 'е' (bytes 4..6) of `g.query(\"секрет\")`"
                .to_string(),
        );
        assert_eq!(
            panic_text(payload.as_ref()),
            "byte index 5 is not a char boundary; it is inside 'е' (bytes 4..6) of"
        );
        let long: Box<dyn std::any::Any + Send> = Box::new("я".repeat(300));
        let text = panic_text(long.as_ref());
        assert_eq!(text, format!("{}...", "я".repeat(MAX_PANIC_TEXT)));
        let other: Box<dyn std::any::Any + Send> = Box::new(7_u8);
        assert_eq!(panic_text(other.as_ref()), "(no message)");
    }

    #[test]
    fn timestamps_are_utc_iso() {
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_timestamp(1_762_732_800), "2025-11-10T00:00:00Z");
        assert_eq!(utc_timestamp(951_825_599), "2000-02-29T11:59:59Z");
    }
}
