//! What the end-to-end tests share across platforms: the socket type (AF_UNIX on Unix through
//! `std::os::unix::net`, on Windows through `uds_windows`, the product's own transport), process
//! checks (`kill` on Unix, `tasklist`/`taskkill` on Windows) and Windows' path and environment
//! details.

// Each test binary uses a part of this module.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The socket stream of the platform.
#[cfg(unix)]
pub use std::os::unix::net::UnixStream;
/// The socket stream of the platform.
#[cfg(windows)]
pub use uds_windows::UnixStream;

/// Whether the process `pid` runs.
#[cfg(unix)]
pub fn alive(pid: u64) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Whether the process `pid` runs (`tasklist` prints its line, `/NH`: without the header).
#[cfg(windows)]
pub fn alive(pid: u64) -> bool {
    Command::new("tasklist")
        .args(["/NH", "/FO", "CSV", "/FI", &format!("PID eq {pid}")])
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\"")))
}

/// Kills the process `pid` at once.
#[cfg(unix)]
pub fn kill(pid: u64) {
    let _ = Command::new("kill")
        .arg("-9")
        .arg(pid.to_string())
        .stderr(Stdio::null())
        .status();
}

/// Kills the process `pid` at once.
#[cfg(windows)]
pub fn kill(pid: u64) {
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// `path` canonicalized as the product does it (without Windows' verbatim prefix `\\?\`).
pub fn canonical(path: &Path) -> PathBuf {
    let path = std::fs::canonicalize(path).unwrap();
    match path.display().to_string().strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => path,
    }
}

/// The variables a cleared environment keeps on this platform besides `PATH` and the home
/// directory: on Windows the system's own (sockets and process creation need `SystemRoot`,
/// commands `PATHEXT` and `ComSpec`), and the per-user directories pointed at `root`.
pub fn platform_env(command: &mut Command, home: &Path, root: &Path) {
    if cfg!(windows) {
        for name in ["SystemRoot", "SystemDrive", "windir", "PATHEXT", "ComSpec"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .env("USERPROFILE", home)
            .env("LOCALAPPDATA", root.join("local"))
            .env("APPDATA", root.join("roaming"))
            .env("TEMP", std::env::temp_dir())
            .env("TMP", std::env::temp_dir());
    }
}
