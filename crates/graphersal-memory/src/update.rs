//! Updates: the latest published release, how this binary was installed, `upgrade`, and the
//! daily check of `graphersal-memory ui`.
//!
//! - **The latest release**: the redirect of `<releases>/latest` (GitHub answers with the
//!   `Location` `<releases>/tag/v<version>`; no API, no rate limit, no JSON). `<releases>` is
//!   `GRAPHERSAL_MEMORY_BASE_URL` when set (the installers' variable, for tests and mirrors), else
//!   [`RELEASES`]. One request, nothing about the machine or the user is sent (the user agent is
//!   `graphersal-memory/<version>`).
//! - **The check** ([`check_cached`]): at most once per [`CHECK_EVERY`], the answer kept in
//!   `<data dir>/update-check.json`; off with `updates.check: false` in `config.json` or
//!   `GRAPHERSAL_MEMORY_UPDATE_CHECK=off`. Only the UI checks by itself; `upgrade` checks because
//!   the user asked.
//! - **How it was installed** ([`Method`], [`method_of`]): the installers' default directory
//!   (`~/.local/bin`, `%LOCALAPPDATA%\Programs\graphersal-memory`), cargo's (`$CARGO_HOME/bin`,
//!   else `~/.cargo/bin`), or another directory.
//! - **`upgrade`** ([`run`]): delegates the replacement to what installed the binary: the
//!   release's own `install.sh` / `install.ps1` (they verify the archive against `SHA256SUMS`
//!   and replace a running binary safely) with `--version` and `--dir`, or `cargo install
//!   graphersal-memory --locked --force --version <v>`; then `daemon restart` through the new
//!   binary and its `--version`. It shows the plan and asks first (`--yes`), `--check` only
//!   reports, `--dry-run` stops after the plan.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

/// The project's releases on GitHub.
pub const RELEASES: &str = "https://github.com/graphersal/graphersal-memory/releases";

/// The releases URL override (the installers read the same variable).
pub const BASE_URL_ENV: &str = "GRAPHERSAL_MEMORY_BASE_URL";

/// `off`, `false`, `0` or `no` switch the UI's check off (over `updates.check`).
pub const CHECK_ENV: &str = "GRAPHERSAL_MEMORY_UPDATE_CHECK";

/// How often the UI asks for the latest release.
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);

/// The UI check's state in the data directory.
pub const STATE_FILE: &str = "update-check.json";

/// How long one request for the latest release may take.
const TIMEOUT: Duration = Duration::from_secs(10);

/// This binary's version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// The releases URL: `GRAPHERSAL_MEMORY_BASE_URL`, else [`RELEASES`] (no trailing `/`).
pub fn releases_url(env: &dyn Fn(&str) -> Option<String>) -> String {
    env(BASE_URL_ENV)
        .filter(|url| !url.trim().is_empty())
        .map_or_else(
            || RELEASES.to_string(),
            |url| url.trim().trim_end_matches('/').to_string(),
        )
}

/// Whether the UI may check: `GRAPHERSAL_MEMORY_UPDATE_CHECK` first, then `updates.check`.
pub fn check_enabled(env: &dyn Fn(&str) -> Option<String>, config: Option<bool>) -> bool {
    match env(CHECK_ENV).map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if ["off", "false", "0", "no"].contains(&value.as_str()) => false,
        Some(value) if ["on", "true", "1", "yes"].contains(&value.as_str()) => true,
        _ => config.unwrap_or(true),
    }
}

/// `X.Y.Z` (an optional leading `v`; a pre-release or build suffix is not a release here).
pub fn parse_version(text: &str) -> Option<[u64; 3]> {
    let text = text.trim();
    let text = text.strip_prefix('v').unwrap_or(text);
    let mut parts = text.split('.');
    let mut out = [0u64; 3];
    for slot in &mut out {
        let part = parts.next()?;
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
}

/// Whether `latest` is a later release than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

/// The version in a `Location` of `<releases>/latest`: `.../tag/v<version>`.
pub fn version_from_location(location: &str) -> Option<String> {
    let tag = location
        .split(['?', '#'])
        .next()?
        .trim_end_matches('/')
        .rsplit_once("/tag/")?
        .1;
    let tag = tag.strip_prefix('v').unwrap_or(tag);
    parse_version(tag).map(|_| tag.to_string())
}

/// The latest published release of `releases` (one request, no redirect followed).
pub fn latest(releases: &str) -> Result<String, String> {
    let agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_global(Some(TIMEOUT))
        .user_agent(concat!("graphersal-memory/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent();
    let url = format!("{releases}/latest");
    let response = agent
        .get(&url)
        .call()
        .map_err(|err| format!("cannot reach {url}: {err}"))?;
    let status = response.status().as_u16();
    let location = response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    match location {
        Some(location) if (300..400).contains(&status) => version_from_location(&location)
            .ok_or_else(|| format!("{url} pointed at {location}, which names no release")),
        _ => Err(format!(
            "{url} answered HTTP {status} without naming the latest release"
        )),
    }
}

/// The result of a check.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    /// This binary's version.
    pub current: String,
    /// Whether the check may run (`updates.check`, `GRAPHERSAL_MEMORY_UPDATE_CHECK`).
    pub enabled: bool,
    /// The latest release, when known.
    pub latest: Option<String>,
    /// When the latest release was last asked for (seconds since the Unix epoch).
    pub checked_at: Option<u64>,
    /// Why the last request failed.
    pub error: Option<String>,
}

impl Check {
    /// Whether a later release than this binary is published.
    pub fn newer(&self) -> bool {
        self.latest
            .as_deref()
            .is_some_and(|latest| is_newer(latest, &self.current))
    }
}

fn secs(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The check of the UI: the kept answer when it is younger than [`CHECK_EVERY`], else one
/// request through `fetch` (kept, also when it failed, so an offline machine asks once a day).
/// `enabled` false asks nothing and reads nothing.
pub fn check_cached(
    data_dir: &Path,
    enabled: bool,
    now: SystemTime,
    fetch: &dyn Fn() -> Result<String, String>,
) -> Check {
    let mut check = Check {
        current: CURRENT.to_string(),
        enabled,
        latest: None,
        checked_at: None,
        error: None,
    };
    if !enabled {
        return check;
    }
    let file = data_dir.join(STATE_FILE);
    let kept: Value = std::fs::read(&file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null);
    let checked_at = kept["checked_at"].as_u64();
    let fresh = checked_at
        .is_some_and(|at| secs(now).saturating_sub(at) < CHECK_EVERY.as_secs() && at <= secs(now));
    if fresh {
        check.checked_at = checked_at;
        check.latest = kept["latest"].as_str().map(str::to_string);
        check.error = kept["error"].as_str().map(str::to_string);
        return check;
    }
    refresh(data_dir, now, &kept, fetch)
}

/// One request through `fetch`, kept in the check's state (`kept`: the state before, whose last
/// known release stays beside a failed request).
fn refresh(
    data_dir: &Path,
    now: SystemTime,
    kept: &Value,
    fetch: &dyn Fn() -> Result<String, String>,
) -> Check {
    let file = data_dir.join(STATE_FILE);
    let mut check = Check {
        current: CURRENT.to_string(),
        enabled: true,
        latest: None,
        checked_at: Some(secs(now)),
        error: None,
    };
    match fetch() {
        Ok(latest) => check.latest = Some(latest),
        Err(err) => {
            // Keep the last known release beside the error.
            check.latest = kept["latest"].as_str().map(str::to_string);
            check.error = Some(err);
        }
    }
    let state = json!({"checked_at": check.checked_at, "latest": check.latest,
                       "error": check.error});
    let _ = crate::fsutil::write_atomic(&file, format!("{state}\n").as_bytes());
    check
}

/// How this binary was installed, by its directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    /// The one-line installer's default directory.
    Installer(PathBuf),
    /// `cargo install` (`$CARGO_HOME/bin`, else `~/.cargo/bin`).
    Cargo(PathBuf),
    /// Another directory (the installer with `--dir`, or copied there by hand).
    Other(PathBuf),
}

impl Method {
    /// The directory of the binary.
    pub fn dir(&self) -> &Path {
        match self {
            Method::Installer(dir) | Method::Cargo(dir) | Method::Other(dir) => dir,
        }
    }

    /// A short name for messages and the UI.
    pub fn name(&self) -> &'static str {
        match self {
            Method::Installer(_) => "installer",
            Method::Cargo(_) => "cargo",
            Method::Other(_) => "other",
        }
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let (a, b) = (canonical(a), canonical(b));
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// How the binary at `exe` was installed (`windows`: the platform's installer directory).
pub fn method_of(
    exe: &Path,
    home: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    windows: bool,
) -> Method {
    let dir = exe
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let cargo_bin = env("CARGO_HOME")
        .filter(|value| !value.is_empty())
        .map_or_else(|| home.join(".cargo"), PathBuf::from)
        .join("bin");
    let installer = if windows {
        env("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(|local| {
                PathBuf::from(local)
                    .join("Programs")
                    .join("graphersal-memory")
            })
    } else {
        Some(home.join(".local").join("bin"))
    };
    if same_dir(&dir, &cargo_bin) {
        Method::Cargo(dir)
    } else if installer.is_some_and(|installer| same_dir(&dir, &installer)) {
        Method::Installer(dir)
    } else {
        Method::Other(dir)
    }
}

/// A path as one word of the shell the commands are for (PowerShell on Windows, `sh` else).
fn quoted(path: &Path, windows: bool) -> String {
    let text = path.display().to_string();
    let plain = text
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '/' | '\\' | '.' | '_' | '-' | ':' | '~'));
    if plain {
        text
    } else if windows {
        format!("'{}'", text.replace('\'', "''"))
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// The commands that replace the binary by hand (the same `upgrade` runs), then restart the
/// daemon: each ready to copy into the platform's shell.
pub fn manual_commands(method: &Method, windows: bool) -> Vec<String> {
    let mut commands = Vec::new();
    match (method, windows) {
        (Method::Cargo(_), _) => {
            commands.push("cargo install graphersal-memory --locked --force".to_string());
        }
        (Method::Installer(_), false) => commands.push(format!(
            "curl -fsSL {RELEASES}/latest/download/install.sh | sh"
        )),
        (Method::Installer(_), true) => {
            commands.push(format!("irm {RELEASES}/latest/download/install.ps1 | iex"))
        }
        (Method::Other(dir), false) => commands.push(format!(
            "curl -fsSL {RELEASES}/latest/download/install.sh | sh -s -- --dir {}",
            quoted(dir, false)
        )),
        (Method::Other(dir), true) => commands.push(format!(
            "& ([scriptblock]::Create((irm {RELEASES}/latest/download/install.ps1))) -Dir {}",
            quoted(dir, true)
        )),
    }
    commands.push("graphersal-memory daemon restart".to_string());
    commands
}

/// The check and how to upgrade, for the UI (`GET /api/ui/update`).
pub fn check_json(check: &Check, method: &Method, windows: bool) -> Value {
    json!({
        "current": check.current,
        "enabled": check.enabled,
        "latest": check.latest,
        "newer": check.newer(),
        "checked_at": check.checked_at,
        "error": check.error,
        "method": method.name(),
        "dir": method.dir().display().to_string(),
        "upgrade": "graphersal-memory upgrade",
        "manual": manual_commands(method, windows),
        "release_notes": check.latest.as_ref().map(|v| format!("{RELEASES}/tag/v{v}")),
        "switch_off": "\"updates\": {\"check\": false} in config.json",
    })
}

/// One step of an upgrade: a program and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// The program.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<String>,
}

impl Step {
    fn shown(&self) -> String {
        let mut words = vec![quoted(&self.program, cfg!(windows))];
        words.extend(
            self.args
                .iter()
                .map(|a| quoted(Path::new(a), cfg!(windows))),
        );
        words.join(" ")
    }
}

/// What `upgrade` does to the machine: download a file, run a program.
pub trait Machine {
    /// Downloads `url` into `dest`.
    fn download(&mut self, url: &str, dest: &Path) -> Result<(), String>;
    /// Runs `step` with its output shown; an error when it fails.
    fn run(&mut self, step: &Step) -> Result<(), String>;
    /// Before replacing the binary at `exe` (Windows: a running `.exe` cannot be overwritten,
    /// but it can be renamed).
    fn prepare_replace(&mut self, exe: &Path) -> Result<(), String>;
}

/// The real machine.
pub struct RealMachine;

impl Machine for RealMachine {
    fn download(&mut self, url: &str, dest: &Path) -> Result<(), String> {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(60)))
            .user_agent(concat!("graphersal-memory/", env!("CARGO_PKG_VERSION")))
            .build()
            .new_agent();
        let mut response = agent
            .get(url)
            .call()
            .map_err(|err| format!("cannot download {url}: {err}"))?;
        let bytes = response
            .body_mut()
            .read_to_vec()
            .map_err(|err| format!("cannot download {url}: {err}"))?;
        std::fs::write(dest, bytes).map_err(|err| format!("cannot write {}: {err}", dest.display()))
    }

    fn run(&mut self, step: &Step) -> Result<(), String> {
        let status = std::process::Command::new(&step.program)
            .args(&step.args)
            .status()
            .map_err(|err| format!("cannot run {}: {err}", step.program.display()))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("`{}` failed ({status})", step.shown()))
        }
    }

    fn prepare_replace(&mut self, exe: &Path) -> Result<(), String> {
        if !cfg!(windows) {
            return Ok(());
        }
        let old = exe.with_extension("old.exe");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(exe, &old).map_err(|err| {
            format!(
                "cannot move the running {} aside to {}: {err}",
                exe.display(),
                old.display()
            )
        })
    }
}

/// `upgrade`'s flags and machine.
pub struct Options {
    /// `--check`: report only.
    pub check_only: bool,
    /// `--yes`.
    pub yes: bool,
    /// `--dry-run`.
    pub dry_run: bool,
    /// Whether stdin and stdout are a terminal.
    pub interactive: bool,
    /// This binary.
    pub exe: PathBuf,
    /// How it was installed.
    pub method: Method,
    /// The releases URL.
    pub releases: String,
    /// The data directory (the check's state).
    pub data_dir: PathBuf,
    /// A directory for the downloaded installer.
    pub scratch: PathBuf,
    /// Windows (PowerShell and `install.ps1`) or not (`sh` and `install.sh`).
    pub windows: bool,
}

/// The steps that replace the binary with `version`.
fn replace_steps(options: &Options, version: &str) -> (Option<(String, PathBuf)>, Step) {
    let dir = options.method.dir().display().to_string();
    match &options.method {
        Method::Cargo(_) => (
            None,
            Step {
                program: PathBuf::from("cargo"),
                args: vec![
                    "install".into(),
                    "graphersal-memory".into(),
                    "--locked".into(),
                    "--force".into(),
                    "--version".into(),
                    version.to_string(),
                ],
            },
        ),
        _ if options.windows => {
            let script = options.scratch.join("install.ps1");
            (
                Some((
                    format!("{}/download/v{version}/install.ps1", options.releases),
                    script.clone(),
                )),
                Step {
                    program: PathBuf::from("powershell"),
                    args: vec![
                        "-NoProfile".into(),
                        "-ExecutionPolicy".into(),
                        "Bypass".into(),
                        "-File".into(),
                        script.display().to_string(),
                        "-Version".into(),
                        format!("v{version}"),
                        "-Dir".into(),
                        dir,
                    ],
                },
            )
        }
        _ => {
            let script = options.scratch.join("install.sh");
            (
                Some((
                    format!("{}/download/v{version}/install.sh", options.releases),
                    script.clone(),
                )),
                Step {
                    program: PathBuf::from("sh"),
                    args: vec![
                        script.display().to_string(),
                        "--version".into(),
                        format!("v{version}"),
                        "--dir".into(),
                        dir,
                    ],
                },
            )
        }
    }
}

/// `upgrade`. Returns the exit code.
pub fn run(
    options: &Options,
    fetch: &dyn Fn() -> Result<String, String>,
    machine: &mut dyn Machine,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<u8, String> {
    // The user asked: always a fresh request (kept for the UI too).
    let check = refresh(&options.data_dir, SystemTime::now(), &Value::Null, fetch);
    let latest = match (&check.latest, &check.error) {
        (Some(latest), None) => latest.clone(),
        (_, Some(err)) => return Err(format!("cannot find the latest release: {err}")),
        (None, None) => return Err("cannot find the latest release".to_string()),
    };
    let _ = writeln!(out, "graphersal-memory upgrade");
    let _ = writeln!(out, "  installed: {CURRENT} ({})", options.exe.display());
    let _ = writeln!(out, "  latest:    {latest}");
    if !check.newer() {
        let _ = writeln!(out, "\nUp to date: nothing to do.");
        return Ok(0);
    }
    let _ = writeln!(out, "  notes:     {RELEASES}/tag/v{latest}");
    if options.check_only {
        let _ = writeln!(out, "\nA newer release is available. Upgrade with:\n");
        let _ = writeln!(out, "  graphersal-memory upgrade");
        let _ = writeln!(out, "\nor by hand:\n");
        for command in manual_commands(&options.method, options.windows) {
            let _ = writeln!(out, "  {command}");
        }
        return Ok(0);
    }
    let (download, replace) = replace_steps(options, &latest);
    let restart = Step {
        program: options.exe.clone(),
        args: vec!["daemon".into(), "restart".into()],
    };
    let version = Step {
        program: options.exe.clone(),
        args: vec!["--version".into()],
    };
    let how = match &options.method {
        Method::Cargo(_) => "cargo install (it was installed with cargo)",
        Method::Installer(_) => "the release's installer (it was installed with it)",
        Method::Other(_) => "the release's installer, into the same directory",
    };
    let _ = writeln!(out, "\nThe upgrade, through {how}:\n");
    let mut n = 1;
    if let Some((url, _)) = &download {
        let _ = writeln!(out, "  {n}. download {url}");
        n += 1;
    }
    let _ = writeln!(out, "  {n}. {}", replace.shown());
    let _ = writeln!(out, "  {}. {}", n + 1, restart.shown());
    let _ = writeln!(out, "  {}. {}", n + 2, version.shown());
    if matches!(options.method, Method::Cargo(_)) {
        let _ = writeln!(out, "\n  cargo builds from source: a few minutes.");
    }
    if options.dry_run {
        let _ = writeln!(out, "\nDry run: nothing was changed.");
        return Ok(0);
    }
    if !options.yes {
        if !options.interactive {
            return Err(
                "not a terminal: run `graphersal-memory upgrade --yes` to upgrade without asking"
                    .to_string(),
            );
        }
        let _ = writeln!(out);
        if !crate::install::confirm("Upgrade now?", input, out) {
            let _ = writeln!(out, "Not upgraded.");
            return Ok(crate::commands::EXIT_FAILED);
        }
    }
    if let Some((url, dest)) = &download {
        std::fs::create_dir_all(&options.scratch)
            .map_err(|err| format!("cannot create {}: {err}", options.scratch.display()))?;
        machine.download(url, dest)?;
    }
    if matches!(options.method, Method::Cargo(_)) {
        machine.prepare_replace(&options.exe)?;
    }
    let replaced = machine.run(&replace);
    if download.is_some() {
        let _ = std::fs::remove_dir_all(&options.scratch);
    }
    replaced
        .map_err(|err| format!("the upgrade failed: {err}; the installed version is unchanged"))?;
    let _ = writeln!(out);
    machine
        .run(&restart)
        .map_err(|err| format!("upgraded, but the daemon did not restart: {err}"))?;
    machine.run(&version)?;
    let _ = writeln!(
        out,
        "\nUpgraded to {latest}. Open sessions use the new daemon; `graphersal-memory doctor` \
         checks the setup."
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_compare() {
        assert_eq!(parse_version("v0.2.1"), Some([0, 2, 1]));
        assert_eq!(parse_version("0.10.0"), Some([0, 10, 0]));
        for bad in ["", "0.2", "0.2.1.4", "0.2.x", "v0.3.0-rc1", "0..1"] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
        assert!(is_newer("0.2.1", "0.2.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(!is_newer("nonsense", "0.2.0"));
    }

    #[test]
    fn the_latest_release_is_read_from_the_redirect() {
        let at = |tag: &str| format!("{RELEASES}/tag/{tag}");
        assert_eq!(
            version_from_location(&at("v0.2.1")).as_deref(),
            Some("0.2.1")
        );
        assert_eq!(
            version_from_location(&at("0.3.0/")).as_deref(),
            Some("0.3.0")
        );
        assert_eq!(
            version_from_location(&format!("{}?x=1", at("v1.0.0"))).as_deref(),
            Some("1.0.0")
        );
        assert_eq!(version_from_location(RELEASES), None);
        assert_eq!(version_from_location(&at("nightly")), None);
    }

    #[test]
    fn the_check_can_be_switched_off() {
        let none = |_: &str| None;
        assert!(check_enabled(&none, None));
        assert!(!check_enabled(&none, Some(false)));
        let off = |name: &str| (name == CHECK_ENV).then(|| "OFF".to_string());
        assert!(!check_enabled(&off, Some(true)));
        let on = |name: &str| (name == CHECK_ENV).then(|| "1".to_string());
        assert!(check_enabled(&on, Some(false)));
        let base = |name: &str| (name == BASE_URL_ENV).then(|| "http://127.0.0.1:9/r/".to_string());
        assert_eq!(releases_url(&base), "http://127.0.0.1:9/r");
        assert_eq!(releases_url(&none), RELEASES);
    }

    #[test]
    fn the_check_asks_once_a_day_and_keeps_the_answer() {
        let tmp = crate::mcp_tests::TempDir::new("update-check");
        let asked = std::cell::Cell::new(0);
        let fetch = || {
            asked.set(asked.get() + 1);
            Ok::<_, String>("9.0.0".to_string())
        };
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        // Off: no request, no file.
        let off = check_cached(tmp.path(), false, now, &fetch);
        assert!(!off.enabled && off.latest.is_none() && !off.newer());
        assert_eq!(asked.get(), 0);
        assert!(!tmp.path().join(STATE_FILE).exists());
        let first = check_cached(tmp.path(), true, now, &fetch);
        assert_eq!(first.latest.as_deref(), Some("9.0.0"));
        assert!(first.newer());
        assert_eq!(asked.get(), 1);
        // Within a day: the kept answer.
        let again = check_cached(tmp.path(), true, now + Duration::from_secs(3600), &fetch);
        assert_eq!((again.latest.as_deref(), asked.get()), (Some("9.0.0"), 1));
        // A day later, offline: asked again, the error kept beside the last known release.
        let failing = || Err::<String, _>("offline".to_string());
        let later = check_cached(tmp.path(), true, now + CHECK_EVERY, &failing);
        assert_eq!(later.latest.as_deref(), Some("9.0.0"));
        assert_eq!(later.error.as_deref(), Some("offline"));
        let kept = check_cached(
            tmp.path(),
            true,
            now + CHECK_EVERY + Duration::from_secs(60),
            &fetch,
        );
        assert_eq!((kept.error.as_deref(), asked.get()), (Some("offline"), 1));
    }

    fn env_of<'a>(pairs: &'a [(&'a str, String)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.clone())
        }
    }

    #[test]
    fn the_install_method_is_known_by_the_directory() {
        let home = crate::mcp_tests::TempDir::new("update-home");
        let home = home.path();
        let exe = |dir: &Path| dir.join("graphersal-memory");
        let cargo = home.join(".cargo").join("bin");
        let local = home.join(".local").join("bin");
        let none: [(&str, String); 0] = [];
        assert_eq!(
            method_of(&exe(&cargo), home, &env_of(&none), false),
            Method::Cargo(cargo.clone())
        );
        assert_eq!(
            method_of(&exe(&local), home, &env_of(&none), false),
            Method::Installer(local.clone())
        );
        let elsewhere = home.join("tools");
        assert_eq!(
            method_of(&exe(&elsewhere), home, &env_of(&none), false),
            Method::Other(elsewhere.clone())
        );
        // CARGO_HOME moves cargo's directory; LOCALAPPDATA names the Windows installer's.
        let custom = home.join("rust");
        let with_cargo_home = [("CARGO_HOME", custom.display().to_string())];
        assert_eq!(
            method_of(
                &exe(&custom.join("bin")),
                home,
                &env_of(&with_cargo_home),
                false
            ),
            Method::Cargo(custom.join("bin"))
        );
        let appdata = home.join("AppData").join("Local");
        let programs = appdata.join("Programs").join("graphersal-memory");
        let with_local = [("LOCALAPPDATA", appdata.display().to_string())];
        assert_eq!(
            method_of(&exe(&programs), home, &env_of(&with_local), true),
            Method::Installer(programs.clone())
        );
        assert_eq!(
            method_of(&exe(&local), home, &env_of(&with_local), true),
            Method::Other(local)
        );
    }

    #[test]
    fn manual_commands_are_ready_to_copy() {
        let dir = PathBuf::from("/opt/my tools");
        assert_eq!(
            manual_commands(&Method::Cargo(dir.clone()), false),
            [
                "cargo install graphersal-memory --locked --force",
                "graphersal-memory daemon restart"
            ]
        );
        assert_eq!(
            manual_commands(&Method::Installer(dir.clone()), false)[0],
            format!("curl -fsSL {RELEASES}/latest/download/install.sh | sh")
        );
        assert_eq!(
            manual_commands(&Method::Installer(dir.clone()), true)[0],
            format!("irm {RELEASES}/latest/download/install.ps1 | iex")
        );
        assert_eq!(
            manual_commands(&Method::Other(dir.clone()), false)[0],
            format!(
                "curl -fsSL {RELEASES}/latest/download/install.sh | sh -s -- --dir '/opt/my tools'"
            )
        );
    }

    /// A machine that records what it was asked to do.
    #[derive(Default)]
    struct Recorded {
        downloads: Vec<(String, PathBuf)>,
        runs: Vec<Step>,
        prepared: Vec<PathBuf>,
        fail_run: Option<usize>,
    }

    impl Machine for Recorded {
        fn download(&mut self, url: &str, dest: &Path) -> Result<(), String> {
            self.downloads.push((url.to_string(), dest.to_path_buf()));
            std::fs::write(dest, "#!/bin/sh\n").map_err(|e| e.to_string())
        }
        fn run(&mut self, step: &Step) -> Result<(), String> {
            self.runs.push(step.clone());
            if self.fail_run == Some(self.runs.len()) {
                return Err("exit status 1".to_string());
            }
            Ok(())
        }
        fn prepare_replace(&mut self, exe: &Path) -> Result<(), String> {
            self.prepared.push(exe.to_path_buf());
            Ok(())
        }
    }

    fn options_in(tmp: &Path, method: Method, windows: bool) -> Options {
        Options {
            check_only: false,
            yes: true,
            dry_run: false,
            interactive: false,
            exe: method.dir().join("graphersal-memory"),
            method,
            releases: "http://127.0.0.1:9/releases".to_string(),
            data_dir: tmp.join("data"),
            scratch: tmp.join("scratch"),
            windows,
        }
    }

    fn upgrade(
        options: &Options,
        latest: &str,
        machine: &mut Recorded,
        input: &str,
    ) -> (Result<u8, String>, String) {
        std::fs::create_dir_all(&options.data_dir).unwrap();
        let latest = latest.to_string();
        let mut out = Vec::new();
        let result = run(
            options,
            &|| Ok(latest.clone()),
            machine,
            &mut input.as_bytes(),
            &mut out,
        );
        (result, String::from_utf8(out).unwrap())
    }

    #[test]
    fn upgrade_through_the_installer_runs_the_releases_script_into_the_same_directory() {
        let tmp = crate::mcp_tests::TempDir::new("upgrade-installer");
        let dir = tmp.path().join("bin");
        let options = options_in(tmp.path(), Method::Installer(dir.clone()), false);
        let mut machine = Recorded::default();
        let (result, out) = upgrade(&options, "99.0.0", &mut machine, "");
        assert_eq!(result, Ok(0), "{out}");
        assert_eq!(
            machine.downloads,
            [(
                "http://127.0.0.1:9/releases/download/v99.0.0/install.sh".to_string(),
                options.scratch.join("install.sh")
            )]
        );
        assert_eq!(machine.runs[0].program, PathBuf::from("sh"));
        assert_eq!(
            machine.runs[0].args,
            [
                options.scratch.join("install.sh").display().to_string(),
                "--version".into(),
                "v99.0.0".into(),
                "--dir".into(),
                dir.display().to_string()
            ]
        );
        assert_eq!(machine.runs[1].args, ["daemon", "restart"]);
        assert_eq!(machine.runs[1].program, options.exe);
        assert_eq!(machine.runs[2].args, ["--version"]);
        assert!(machine.prepared.is_empty());
        assert!(
            !options.scratch.exists(),
            "the downloaded script is removed"
        );
        assert!(out.contains("Upgraded to 99.0.0."), "{out}");
        // The check's state is kept for the UI.
        assert!(options.data_dir.join(STATE_FILE).exists());
    }

    #[test]
    fn upgrade_on_windows_runs_install_ps1_and_through_cargo_moves_the_running_exe_aside() {
        let tmp = crate::mcp_tests::TempDir::new("upgrade-windows");
        let dir = tmp.path().join("Programs");
        let options = options_in(tmp.path(), Method::Other(dir.clone()), true);
        let mut machine = Recorded::default();
        assert_eq!(upgrade(&options, "99.0.0", &mut machine, "").0, Ok(0));
        assert!(
            machine.downloads[0]
                .0
                .ends_with("/download/v99.0.0/install.ps1")
        );
        assert_eq!(machine.runs[0].program, PathBuf::from("powershell"));
        assert!(machine.runs[0].args.ends_with(&[
            "-Version".into(),
            "v99.0.0".into(),
            "-Dir".into(),
            dir.display().to_string()
        ]));

        let cargo = options_in(
            tmp.path(),
            Method::Cargo(tmp.path().join("cargo-bin")),
            true,
        );
        let mut machine = Recorded::default();
        assert_eq!(upgrade(&cargo, "99.0.0", &mut machine, "").0, Ok(0));
        assert!(machine.downloads.is_empty());
        assert_eq!(machine.prepared, std::slice::from_ref(&cargo.exe));
        assert_eq!(machine.runs[0].program, PathBuf::from("cargo"));
        assert_eq!(
            machine.runs[0].args,
            [
                "install",
                "graphersal-memory",
                "--locked",
                "--force",
                "--version",
                "99.0.0"
            ]
        );
    }

    #[test]
    fn upgrade_reports_asks_and_refuses_without_changing_anything() {
        let tmp = crate::mcp_tests::TempDir::new("upgrade-ask");
        let mut options = options_in(tmp.path(), Method::Installer(tmp.path().join("bin")), false);
        // Up to date.
        let mut machine = Recorded::default();
        let (result, out) = upgrade(&options, CURRENT, &mut machine, "");
        assert_eq!(result, Ok(0));
        assert!(out.contains("Up to date"), "{out}");
        assert!(machine.runs.is_empty() && machine.downloads.is_empty());
        // --check: the commands, nothing run.
        options.check_only = true;
        let (result, out) = upgrade(&options, "99.0.0", &mut machine, "");
        assert_eq!(result, Ok(0));
        assert!(out.contains("  graphersal-memory upgrade\n"), "{out}");
        assert!(out.contains("| sh\n"), "{out}");
        assert!(machine.runs.is_empty());
        // --dry-run: the plan, nothing run.
        options.check_only = false;
        options.dry_run = true;
        let (result, out) = upgrade(&options, "99.0.0", &mut machine, "");
        assert_eq!(result, Ok(0));
        assert!(out.contains("Dry run"), "{out}");
        assert!(machine.runs.is_empty() && machine.downloads.is_empty());
        // No terminal and no --yes: refused; a terminal and "n": not upgraded.
        options.dry_run = false;
        options.yes = false;
        let (result, _) = upgrade(&options, "99.0.0", &mut machine, "");
        assert!(result.unwrap_err().contains("--yes"));
        options.interactive = true;
        let (result, out) = upgrade(&options, "99.0.0", &mut machine, "n\n");
        assert_eq!(result, Ok(crate::commands::EXIT_FAILED));
        assert!(out.contains("Not upgraded."), "{out}");
        assert!(machine.runs.is_empty() && machine.downloads.is_empty());
        // A failing installer: an error that says nothing changed, no restart.
        options.yes = true;
        let mut failing = Recorded {
            fail_run: Some(1),
            ..Recorded::default()
        };
        let (result, _) = upgrade(&options, "99.0.0", &mut failing, "");
        assert!(result.unwrap_err().contains("unchanged"));
        assert_eq!(failing.runs.len(), 1);
        // The latest release unknown: an error.
        let mut out = Vec::new();
        let err = run(
            &options,
            &|| Err("offline".to_string()),
            &mut Recorded::default(),
            &mut "".as_bytes(),
            &mut out,
        )
        .unwrap_err();
        assert!(err.contains("offline"), "{err}");
    }

    #[test]
    fn the_latest_release_comes_from_a_local_releases_server() {
        use std::io::{Read, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 2048];
            let n = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..n]).to_string();
            let _ = stream.write_all(
                b"HTTP/1.1 302 Found\r\nLocation: https://example.invalid/releases/tag/v7.8.9\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            request
        });
        let releases = format!("http://127.0.0.1:{port}/releases");
        assert_eq!(latest(&releases).as_deref(), Ok("7.8.9"));
        let request = server.join().unwrap();
        assert!(request.starts_with("GET /releases/latest "), "{request}");
        assert!(
            request
                .to_lowercase()
                .contains("user-agent: graphersal-memory/"),
            "{request}"
        );
    }
}
