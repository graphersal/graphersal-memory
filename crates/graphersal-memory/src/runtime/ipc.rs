//! The local transport between the shim and the daemon: the run directory, its files, the
//! socket types per platform and the lock files.
//!
//! The run directory is, in this order: `--run-dir`, the environment variable
//! `GRAPHERSAL_MEMORY_RUN_DIR`, `<data dir>/run` when the data directory was chosen explicitly
//! (`--data-dir` or `GRAPHERSAL_MEMORY_DIR`: another data directory gets a daemon of its own),
//! then the platform's default (macOS `~/Library/Application Support/graphersal-memory/run`,
//! Linux `$XDG_RUNTIME_DIR/graphersal-memory`, else `~/.local/state/graphersal-memory/run`,
//! Windows `%LOCALAPPDATA%\graphersal-memory\run`). When the socket path in a default (or in
//! `<data dir>/run`) would be longer than local sockets allow (a long home directory on macOS),
//! the run directory is `/tmp/graphersal-memory-<fingerprint of the data directory>` instead
//! (`short_run_dir`), the same for every process of that data directory. It holds:
//!
//! - `daemon.sock`: the daemon's socket (AF_UNIX on every platform), `0600` in a `0700`
//!   directory on Unix;
//! - `daemon.pid`: locked by the running daemon for its whole life (the OS releases the lock
//!   when it ends, also on a crash), its process id inside;
//! - `daemon.start.lock`: locked by the shim that starts a daemon, so two shims starting at the
//!   same moment spawn one daemon;
//! - `daemon.log`: the daemon's diagnostics (`daemon.log.1`: the previous one).

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use crate::runtime::store::{DATA_DIR_ENV, Platform};

/// The environment variable that overrides the run directory.
pub const RUN_DIR_ENV: &str = "GRAPHERSAL_MEMORY_RUN_DIR";

/// The socket's file name in the run directory.
pub const SOCKET_FILE: &str = "daemon.sock";

/// The daemon's lock file (held while it runs).
pub const PID_FILE: &str = "daemon.pid";

/// The start lock file (held by the shim that starts a daemon).
pub const START_LOCK_FILE: &str = "daemon.start.lock";

/// The daemon's log file.
pub const LOG_FILE: &str = "daemon.log";

/// The marker a daemon leaves when it was stopped on purpose (the `stop` control request of
/// `uninstall` and `purge`): an open shim or `ui` whose connection ended does not start a new
/// daemon while it exists (`crate::runtime::shim`'s reconnect). The next daemon removes it at its start.
pub const STOPPED_FILE: &str = "daemon.stopped";

/// Whether the daemon of `run_dir` was stopped on purpose ([`STOPPED_FILE`]).
pub fn stopped_on_purpose(run_dir: &Path) -> bool {
    run_dir.join(STOPPED_FILE).exists()
}

/// The longest socket path the platforms accept (`sun_path` is 104 bytes on macOS and the BSDs,
/// 108 on Linux and Windows; one byte for the terminating zero).
const MAX_SOCKET_PATH: usize = 103;

/// The socket stream of the platform.
#[cfg(unix)]
pub type Stream = std::os::unix::net::UnixStream;
/// The socket stream of the platform.
#[cfg(windows)]
pub type Stream = uds_windows::UnixStream;

/// The listening socket of the platform.
#[cfg(unix)]
pub type Listener = std::os::unix::net::UnixListener;
/// The listening socket of the platform.
#[cfg(windows)]
pub type Listener = uds_windows::UnixListener;

/// How long a read on a [`Patient`] stream waits in the kernel before it looks again.
pub const READ_TICK: std::time::Duration = std::time::Duration::from_secs(1);

/// A connection's reading side that does not wait on the kernel's wake-up alone.
///
/// macOS 27 (Darwin 27.0.0) sometimes loses the wake-up of a thread blocked in `recv` on an
/// accepted AF_UNIX stream socket when the peer shuts its writing side down: the end of the
/// stream is there (a non-blocking peek answers it), but the blocked read never returns. Seen
/// about once in 1,500 shutdowns, reproduced without this program (a plain socket pair through
/// `listen`/`accept`, one thread blocked in `recv`, the peer's `shutdown(SHUT_WR)`); a shim
/// whose harness ended its session then waited for the daemon forever, and the daemon's
/// connection with it. So every read waits at most [`READ_TICK`] in the kernel and then
/// reads again, which finds the end of the stream at once; a read without data for its
/// `limit` (when there is one) fails with the timeout as a plain read with that timeout would.
#[derive(Debug)]
pub struct Patient {
    stream: Stream,
    limit: Option<std::time::Duration>,
}

impl Patient {
    /// Reads `stream`, failing a read that gets no data for `limit` (`None`: never).
    pub fn new(stream: Stream, limit: Option<std::time::Duration>) -> std::io::Result<Self> {
        stream.set_read_timeout(Some(READ_TICK))?;
        Ok(Self { stream, limit })
    }

    /// Changes how long a read may get no data (`None`: no limit).
    pub fn set_limit(&mut self, limit: Option<std::time::Duration>) {
        self.limit = limit;
    }
}

impl std::io::Read for Patient {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let started = std::time::Instant::now();
        loop {
            match self.stream.read(buf) {
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    if self.limit.is_some_and(|limit| started.elapsed() >= limit) {
                        return Err(err);
                    }
                }
                other => return other,
            }
        }
    }
}

/// Resolves the run directory from the flag, the environment (`env` reads one variable), the
/// data directory and the platform.
pub fn resolve_run_dir(
    flag: Option<&Path>,
    data_dir_flag: Option<&Path>,
    data_dir: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    platform: Platform,
) -> Result<PathBuf, String> {
    if let Some(dir) = flag {
        return Ok(dir.to_path_buf());
    }
    let set = |name: &str| env(name).filter(|value| !value.is_empty());
    if let Some(dir) = set(RUN_DIR_ENV) {
        return Ok(PathBuf::from(dir));
    }
    if data_dir_flag.is_some() || set(DATA_DIR_ENV).is_some() {
        let own = data_dir.join("run");
        if socket_path(&own).is_err() {
            return Ok(short_run_dir(data_dir));
        }
        return Ok(own);
    }
    let default = platform_run_dir(data_dir, env, platform)?;
    // A default whose socket path is too long for local sockets (a long home directory on
    // macOS): a short directory of its own under the system's temporary directory, the same
    // for every process of this data directory.
    if socket_path(&default).is_err() {
        return Ok(short_run_dir(data_dir));
    }
    Ok(default)
}

/// The run directory used when the default one's socket path would be too long:
/// `/tmp/graphersal-memory-<fingerprint of the data directory>` (Windows: the temporary
/// directory). Only its owner may open it (`create_run_dir` refuses one someone else made).
pub fn short_run_dir(data_dir: &Path) -> PathBuf {
    let base = if cfg!(unix) {
        PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    };
    let absolute = if data_dir.is_absolute() {
        data_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(data_dir))
            .unwrap_or_else(|_| data_dir.to_path_buf())
    };
    let id = crate::fsutil::fingerprint(absolute.display().to_string().as_bytes());
    base.join(format!("graphersal-memory-{}", &id[..12]))
}

/// The platform's default run directory (or the data directory's own, when it was chosen).
fn platform_run_dir(
    data_dir: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    platform: Platform,
) -> Result<PathBuf, String> {
    let set = |name: &str| env(name).filter(|value| !value.is_empty());
    let missing = |what: &str| {
        format!(
            "cannot find the per-user run directory ({what} is not set): pass --run-dir or set \
             {RUN_DIR_ENV}"
        )
    };
    match platform {
        Platform::MacOs | Platform::Windows => Ok(data_dir.join("run")),
        Platform::Xdg => {
            // An XDG path is a Unix path: absolute means a leading `/`.
            if let Some(base) = set("XDG_RUNTIME_DIR").filter(|b| b.starts_with('/')) {
                return Ok(PathBuf::from(base).join("graphersal-memory"));
            }
            let home = set("HOME").ok_or_else(|| missing("HOME"))?;
            Ok(PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("graphersal-memory")
                .join("run"))
        }
    }
}

/// Creates the run directory (only the user may open it, on Unix).
pub fn create_run_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|err| format!("cannot create the run directory {}: {err}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|err| {
            format!(
                "cannot restrict the run directory {} to its owner: {err}",
                dir.display()
            )
        })?;
    }
    Ok(())
}

/// The socket path in `run_dir`, refused when it is too long for the platform.
pub fn socket_path(run_dir: &Path) -> Result<PathBuf, String> {
    let path = run_dir.join(SOCKET_FILE);
    if path.as_os_str().len() > MAX_SOCKET_PATH {
        return Err(format!(
            "the socket path {} is longer than {MAX_SOCKET_PATH} bytes, which local sockets \
             cannot use: choose a shorter run directory with --run-dir or {RUN_DIR_ENV}",
            path.display()
        ));
    }
    Ok(path)
}

/// Connects to the daemon's socket in `run_dir`.
pub fn connect(run_dir: &Path) -> std::io::Result<Stream> {
    let path = socket_path(run_dir).map_err(std::io::Error::other)?;
    Stream::connect(path)
}

/// Binds the daemon's socket (readable and writable only by the user, on Unix).
pub fn bind(path: &Path) -> Result<Listener, String> {
    let listener = Listener::bind(path)
        .map_err(|err| format!("cannot listen on {}: {err}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|err| {
            format!(
                "cannot restrict the socket {} to its owner: {err}",
                path.display()
            )
        })?;
    }
    Ok(listener)
}

/// A lock file in the run directory, opened (and created) for locking.
pub struct LockFile {
    lock: fd_lock::RwLock<File>,
    path: PathBuf,
}

impl LockFile {
    /// Opens (creates) the lock file `name` in `run_dir`.
    pub fn open(run_dir: &Path, name: &str) -> Result<Self, String> {
        let path = run_dir.join(name);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|err| format!("cannot open {}: {err}", path.display()))?;
        Ok(Self {
            lock: fd_lock::RwLock::new(file),
            path,
        })
    }

    /// The exclusive lock, or `None` when another process holds it.
    pub fn try_lock(&mut self) -> Result<Option<fd_lock::RwLockWriteGuard<'_, File>>, String> {
        match self.lock.try_write() {
            Ok(guard) => Ok(Some(guard)),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(err) => Err(format!("cannot lock {}: {err}", self.path.display())),
        }
    }

    /// Whether no process holds the lock right now (it is taken and released at once).
    pub fn is_free(&mut self) -> Result<bool, String> {
        Ok(self.try_lock()?.is_some())
    }
}

/// Whether a daemon holds the `daemon.pid` lock in `run_dir`.
pub fn daemon_alive(run_dir: &Path) -> Result<bool, String> {
    if !run_dir.join(PID_FILE).exists() {
        return Ok(false);
    }
    Ok(!LockFile::open(run_dir, PID_FILE)?.is_free()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connected pair through a listening socket (the daemon's kind) in `dir`.
    fn connected_pair(dir: &Path) -> (Stream, Stream) {
        let path = dir.join("p.sock");
        let listener = Listener::bind(&path).unwrap();
        let client = Stream::connect(&path).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        (client, accepted)
    }

    #[test]
    fn a_patient_read_passes_data_and_finds_the_end_after_a_shutdown() {
        use std::io::{BufRead, Write};
        let dir = std::env::temp_dir().join(format!("gm-patient-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut client, accepted) = connected_pair(&dir);
        let mut reader = std::io::BufReader::new(Patient::new(accepted, None).unwrap());
        client.write_all(b"one\n").unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "one\n");
        // The peer ends its side later than a tick: the read keeps waiting, then sees the end.
        let ender = std::thread::spawn(move || {
            std::thread::sleep(READ_TICK + READ_TICK / 2);
            client.shutdown(std::net::Shutdown::Write).unwrap();
            client
        });
        line.clear();
        assert_eq!(reader.read_line(&mut line).unwrap(), 0);
        drop(ender.join().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_patient_read_without_data_fails_after_its_limit() {
        use std::io::Read;
        let dir = std::env::temp_dir().join(format!("gm-patient-limit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (_client, accepted) = connected_pair(&dir);
        let mut patient = Patient::new(accepted, Some(READ_TICK / 2)).unwrap();
        let started = std::time::Instant::now();
        let err = patient.read(&mut [0u8; 8]).unwrap_err();
        assert!(
            matches!(
                err.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ),
            "{err}"
        );
        assert!(started.elapsed() >= READ_TICK / 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn the_run_dir_follows_flag_env_data_dir_then_platform() {
        let data = Path::new("/data");
        let none = env_of(&[("HOME", "/home/u")]);
        assert_eq!(
            resolve_run_dir(Some(Path::new("/r")), None, data, &none, Platform::Xdg).unwrap(),
            PathBuf::from("/r")
        );
        let env = env_of(&[(RUN_DIR_ENV, "/env-run")]);
        assert_eq!(
            resolve_run_dir(None, None, data, &env, Platform::Xdg).unwrap(),
            PathBuf::from("/env-run")
        );
        assert_eq!(
            resolve_run_dir(None, Some(data), data, &none, Platform::Xdg).unwrap(),
            PathBuf::from("/data/run")
        );
        let data_env = env_of(&[(DATA_DIR_ENV, "/data"), ("HOME", "/home/u")]);
        assert_eq!(
            resolve_run_dir(None, None, data, &data_env, Platform::Xdg).unwrap(),
            PathBuf::from("/data/run")
        );
        assert_eq!(
            resolve_run_dir(None, None, data, &none, Platform::MacOs).unwrap(),
            PathBuf::from("/data/run")
        );
        assert_eq!(
            resolve_run_dir(None, None, data, &none, Platform::Xdg).unwrap(),
            PathBuf::from("/home/u/.local/state/graphersal-memory/run")
        );
        let runtime = env_of(&[("HOME", "/home/u"), ("XDG_RUNTIME_DIR", "/run/user/1")]);
        assert_eq!(
            resolve_run_dir(None, None, data, &runtime, Platform::Xdg).unwrap(),
            PathBuf::from("/run/user/1/graphersal-memory")
        );
    }

    #[test]
    fn a_too_long_socket_path_names_the_override() {
        let long = PathBuf::from("/").join("x".repeat(120));
        let err = socket_path(&long).unwrap_err();
        assert!(err.contains(RUN_DIR_ENV), "{err}");
        assert!(socket_path(Path::new("/tmp/r")).is_ok());
    }

    #[test]
    fn a_default_run_dir_too_long_for_a_socket_becomes_a_short_one() {
        let none = env_of(&[("HOME", "/home/u")]);
        let long = PathBuf::from("/Users")
            .join("u".repeat(60))
            .join("Library/Application Support/graphersal-memory");
        let run = resolve_run_dir(None, None, &long, &none, Platform::MacOs).unwrap();
        assert_eq!(run, short_run_dir(&long));
        assert!(socket_path(&run).is_ok(), "{}", run.display());
        // The same for every process of this data directory, another for another one.
        assert_eq!(
            run,
            resolve_run_dir(None, None, &long, &none, Platform::MacOs).unwrap()
        );
        assert_ne!(run, short_run_dir(&long.join("other")));
        // An explicit data directory's own run directory too.
        assert_eq!(
            resolve_run_dir(None, Some(&long), &long, &none, Platform::Xdg).unwrap(),
            run
        );
        // An explicit run directory stays the user's choice (refused later when too long).
        let explicit = PathBuf::from("/").join("r".repeat(120));
        assert_eq!(
            resolve_run_dir(Some(&explicit), None, &long, &none, Platform::MacOs).unwrap(),
            explicit
        );
    }

    #[test]
    fn a_lock_file_is_exclusive_between_handles() {
        let dir = crate::runtime::mcp_tests::TempDir::new("lock");
        let mut first = LockFile::open(dir.path(), "x.lock").unwrap();
        let mut second = LockFile::open(dir.path(), "x.lock").unwrap();
        let guard = first.try_lock().unwrap();
        assert!(guard.is_some());
        assert!(!second.is_free().unwrap());
        drop(guard);
        assert!(second.is_free().unwrap());
    }
}
