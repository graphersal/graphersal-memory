//! Running the user's harness (`claude -p ...`) as a child process: finding its program on every
//! platform, and one run with the prompt on stdin, a timeout, and the output collected.
//!
//! Windows: npm installs `claude` as `claude.cmd` (a batch file), and `std::process::Command`
//! looks only for `<name>.exe` when given a bare name. So a bare program name is resolved here
//! through `PATH` and `PATHEXT` to its full path (`C:\...\npm\claude.cmd`). std runs a `.cmd` or
//! `.bat` path through `cmd.exe` itself, quoting the arguments for it (and refusing an argument it
//! cannot quote safely, such as one with a line break), so `cmd /C` is never assembled by hand.
//! Callers keep multi-line text out of the arguments (stdin or a file instead). Elsewhere the
//! name is left to the OS's own `PATH` search.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The extensions Windows tries when `PATHEXT` is not set.
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// A prepared headless run: its arguments and its environment.
pub type Prepared = (Vec<String>, Vec<(String, String)>);

/// The user's own files a headless run's sandbox copies from (opencode's database, Codex's home).
#[derive(Debug, Clone, Copy, Default)]
pub struct Homes<'a> {
    /// opencode's database (`opencode.db`).
    pub opencode_db: Option<&'a Path>,
    /// Codex's home (`CODEX_HOME`).
    pub codex_home: Option<&'a Path>,
}

/// A harness run headless with a prompt on stdin (`summaries.harness_command`, `bench
/// --harness-command`): how its command is recognised, prepared and answered. Setup-free
/// (`crate::harnesses::driver`): the daemon runs the summaries.
pub trait Headless: Sync {
    /// Whether `command` runs this harness.
    fn matches(&self, command: &[String]) -> bool;
    /// Its headless command when it is the one installed.
    fn default_command(&self) -> Option<&'static str> {
        None
    }
    /// What the install report says about its default command.
    fn default_command_note(&self) -> &'static str {
        ""
    }
    /// Whether `command` runs on a model that costs nothing (a free or local model).
    fn costs_nothing(&self, _command: &[String]) -> bool {
        false
    }
    /// The estimate line's note when `command` costs nothing (`None`: the usual estimate).
    fn free_note(&self, _command: &[String]) -> Option<String> {
        None
    }
    /// The wizard's note under the summaries question when it is the only harness chosen and
    /// the summaries do not use its own CLI (lines of the wizard's width).
    fn wizard_summaries_note(&self) -> Option<&'static [&'static str]> {
        None
    }
    /// Why the summaries do not run through it (Cursor's CLI); `None` when they do.
    fn summaries_refusal(&self) -> Option<&'static str> {
        None
    }
    /// The arguments and environment of a summaries run of `args` in `work_dir` (a sandbox
    /// of its own there, from `homes`).
    fn summaries_run(
        &self,
        args: Vec<String>,
        _work_dir: &Path,
        _homes: &Homes<'_>,
    ) -> Result<crate::harness::Prepared, String> {
        Ok((args, Vec::new()))
    }
    /// The run's standard output as the answer JSON text.
    fn answer(&self, stdout: &str, _command: &[String]) -> String {
        stdout.to_string()
    }
    /// The arguments of a bench call.
    fn bench_args(&self, args: &[String]) -> Vec<String> {
        args.to_vec()
    }
    /// The answer of a bench call (`None`: the JSON object in its standard output).
    fn bench_answer(&self, _output: &Output, _command: &[String]) -> Option<serde_json::Value> {
        None
    }
    /// Why the bench does not run through `command`: (headline, why); `None` when it does.
    fn bench_refusal(&self, _command: &[String]) -> Option<(&'static str, &'static str)> {
        None
    }
}

/// The harness whose headless command `command` is, with its `Headless`.
pub fn headless_of(command: &[String]) -> Option<(crate::agents::Kind, &'static dyn Headless)> {
    crate::agents::Kind::ALL.into_iter().find_map(|kind| {
        crate::harnesses::driver(kind)
            .headless
            .filter(|headless| headless.matches(command))
            .map(|headless| (kind, headless))
    })
}

/// Whether a headless command runs on a model that costs nothing (a free model of opencode
/// Zen, a local model of Codex or CodeWhale).
pub fn costs_nothing(command: &[String]) -> bool {
    headless_of(command).is_some_and(|(_, headless)| headless.costs_nothing(command))
}

/// The program to start for `program` on this platform (see the module docs).
pub fn program_path(program: &str) -> PathBuf {
    resolve(
        program,
        std::env::var_os("PATH"),
        std::env::var("PATHEXT").ok(),
        cfg!(windows),
        &|path: &Path| path.is_file(),
    )
}

/// The resolution behind [`program_path`], with the environment and the file system passed in
/// (tested on every platform). On Windows, a bare name or a path without an extension is tried
/// with each `PATHEXT` extension (a bare name in each `PATH` directory, in order); the first
/// existing file wins. A name with an extension, a non-Windows platform, or nothing found
/// leaves `program` as it is (the spawn then reports it missing).
pub fn resolve(
    program: &str,
    path: Option<OsString>,
    pathext: Option<String>,
    windows: bool,
    is_file: &dyn Fn(&Path) -> bool,
) -> PathBuf {
    let given = PathBuf::from(program);
    if !windows || program.is_empty() || given.extension().is_some() {
        return given;
    }
    let pathext = pathext
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_PATHEXT.to_string());
    let extensions: Vec<&str> = pathext
        .split(';')
        .map(str::trim)
        .filter(|ext| ext.starts_with('.') && ext.len() > 1)
        .collect();
    let with_extension = |base: &Path| -> Option<PathBuf> {
        extensions.iter().find_map(|ext| {
            let mut name = base.as_os_str().to_owned();
            name.push(ext);
            let candidate = PathBuf::from(name);
            is_file(&candidate).then_some(candidate)
        })
    };
    // A path (with a separator) is tried as given; a bare name in each PATH directory.
    let is_bare = !program.contains(['/', '\\']) && !program.contains(':');
    if !is_bare {
        return with_extension(&given).unwrap_or(given);
    }
    let dirs: Vec<PathBuf> = path
        .map(|path| split_windows_path(&path.to_string_lossy()))
        .unwrap_or_default();
    dirs.iter()
        .find_map(|dir| with_extension(&dir.join(program)))
        .unwrap_or(given)
}

/// The directories of a Windows `PATH` value (`;`-separated, quotes removed, empty ones left
/// out). Parsed by hand so that the tests behave the same on every platform.
fn split_windows_path(path: &str) -> Vec<PathBuf> {
    path.split(';')
        .map(|dir| dir.trim().trim_matches('"'))
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// What one harness run produced.
#[derive(Debug, Clone)]
pub struct Output {
    /// The exit code (`None`: ended by a signal).
    pub code: Option<i32>,
    /// Standard output (at most 64 MiB).
    pub stdout: String,
    /// Standard error (at most 64 KiB).
    pub stderr: String,
}

/// Runs `args` (program first, resolved by [`program_path`]) in `cwd` with `env` added, writes
/// `input` to its stdin, and collects its output; killed after `timeout`.
pub fn run(
    args: &[String],
    input: &str,
    cwd: &Path,
    env: &[(&str, &str)],
    timeout: Duration,
) -> Result<Output, String> {
    let Some((program, rest)) = args.split_first() else {
        return Err("the harness command is empty".to_string());
    };
    let started = Instant::now();
    let mut command = Command::new(program_path(program));
    command
        .args(rest)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command
        .spawn()
        .map_err(|err| format!("cannot run the harness {program}: {err}"))?;
    let stdin = child.stdin.take();
    let text = input.to_string();
    let writer = std::thread::spawn(move || {
        if let Some(mut stdin) = stdin {
            let _ = stdin.write_all(text.as_bytes());
        }
    });
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        if let Some(stdout) = stdout.as_mut() {
            let _ = stdout.take(64 * 1024 * 1024).read_to_end(&mut out);
        }
        out
    });
    let mut stderr = child.stderr.take();
    let errors = std::thread::spawn(move || {
        let mut out = Vec::new();
        if let Some(stderr) = stderr.as_mut() {
            let _ = stderr.take(64 * 1024).read_to_end(&mut out);
        }
        out
    });
    let deadline = started + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                kill_tree(child.id());
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "the harness did not finish within {} s",
                    timeout.as_secs()
                ));
            }
            Err(err) => return Err(format!("cannot wait for the harness: {err}")),
        }
    };
    let _ = writer.join();
    let out = reader.join().unwrap_or_default();
    let err = errors.join().unwrap_or_default();
    Ok(Output {
        code: status.code(),
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
    })
}

/// Windows: ends the process `pid` with its children (`taskkill /T`). npm's `claude.cmd` runs
/// in `cmd.exe`, which starts `node.exe`: killing `cmd.exe` alone would leave the harness
/// running (and spending). Elsewhere the harness is the child itself; nothing to do.
fn kill_tree(pid: u32) {
    if cfg!(windows) {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// A path compared the way Windows does: case-insensitive, either separator.
    fn norm(path: &Path) -> String {
        path.display().to_string().replace('\\', "/").to_lowercase()
    }

    fn files(paths: &[&str]) -> impl Fn(&Path) -> bool {
        let set: BTreeSet<String> = paths.iter().map(|p| norm(Path::new(p))).collect();
        move |path: &Path| set.contains(&norm(path))
    }

    fn path_var(dirs: &str) -> Option<OsString> {
        Some(OsString::from(dirs))
    }

    #[test]
    fn elsewhere_the_name_is_left_to_the_os() {
        let exists = files(&["/usr/bin/claude.cmd"]);
        assert_eq!(
            resolve("claude", path_var("/usr/bin"), None, false, &exists),
            PathBuf::from("claude")
        );
    }

    #[test]
    fn on_windows_npm_s_claude_cmd_is_found_through_path_and_pathext() {
        let exists = files(&["C:/Users/u/AppData/Roaming/npm/claude.cmd"]);
        let found = resolve(
            "claude",
            path_var(r#"C:\Windows\system32;"C:/Users/u/AppData/Roaming/npm";"#),
            Some(".COM;.EXE;.BAT;.CMD".into()),
            true,
            &exists,
        );
        assert_eq!(norm(&found), "c:/users/u/appdata/roaming/npm/claude.cmd");
    }

    #[test]
    fn on_windows_the_first_directory_and_the_first_extension_win() {
        let exists = files(&["C:/a/claude.cmd", "C:/b/claude.exe", "C:/a/claude.exe"]);
        let found = resolve(
            "claude",
            path_var("C:/a;C:/b"),
            Some(".EXE;.CMD".into()),
            true,
            &exists,
        );
        assert_eq!(norm(&found), "c:/a/claude.exe");
        // Without PATHEXT the default order applies: .EXE before .CMD.
        let found = resolve("claude", path_var("C:/a"), None, true, &exists);
        assert_eq!(norm(&found), "c:/a/claude.exe");
        let found = resolve("claude", path_var("C:/b;C:/a"), None, true, &exists);
        assert_eq!(norm(&found), "c:/b/claude.exe");
    }

    #[test]
    fn on_windows_a_path_without_extension_gets_one_and_an_extension_is_kept() {
        let exists = files(&["C:/tools/claude.cmd"]);
        assert_eq!(
            norm(&resolve("C:/tools/claude", None, None, true, &exists)),
            "c:/tools/claude.cmd"
        );
        assert_eq!(
            resolve("C:/tools/claude.exe", None, None, true, &exists),
            PathBuf::from("C:/tools/claude.exe")
        );
        // Nothing found: the name as given (the spawn reports it missing).
        assert_eq!(
            resolve("claude", path_var("C:/none"), None, true, &exists),
            PathBuf::from("claude")
        );
        assert_eq!(resolve("", None, None, true, &exists), PathBuf::from(""));
    }

    #[cfg(unix)]
    #[test]
    fn a_run_gets_stdin_and_returns_the_output() {
        let dir = crate::mcp_tests::TempDir::new("harness-run");
        let out = run(
            &["sh".into(), "-c".into(), "cat; echo err >&2; exit 3".into()],
            "hello",
            dir.path(),
            &[("X", "1")],
            Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(out.stdout, "hello");
        assert_eq!(out.stderr.trim(), "err");
        assert_eq!(out.code, Some(3));
        let slow = run(
            &["sh".into(), "-c".into(), "sleep 5".into()],
            "",
            dir.path(),
            &[],
            Duration::from_millis(200),
        );
        assert!(slow.unwrap_err().contains("did not finish"));
        assert!(run(&[], "", dir.path(), &[], Duration::from_secs(1)).is_err());
    }

    /// npm's `claude.cmd` the way std runs it: through `cmd.exe`, the arguments quoted for it
    /// (a JSON schema and an empty argument as `summaries::harness_args` passes them), the
    /// prompt on stdin; a timeout ends the batch file's children too.
    #[cfg(windows)]
    #[test]
    fn a_batch_file_runs_with_quoted_arguments_and_its_tree_is_killed_on_timeout() {
        let dir = crate::mcp_tests::TempDir::new("harness-cmd");
        std::fs::write(
            dir.path().join("fake.cmd"),
            "@echo off\r\necho ARGS %*\r\nfindstr \"^\"\r\necho err 1>&2\r\nexit /b 3\r\n",
        )
        .unwrap();
        let program = dir.path().join("fake").display().to_string();
        let schema = r#"{"type":"object","properties":{"a":{"type":"string"}}}"#;
        let out = run(
            &[
                program,
                "-p".into(),
                "--json-schema".into(),
                schema.into(),
                "--tools".into(),
                String::new(),
            ],
            "hello\nworld",
            dir.path(),
            &[("X", "1")],
            Duration::from_secs(60),
        )
        .unwrap();
        assert_eq!(out.code, Some(3), "{out:?}");
        assert!(
            out.stdout.contains("hello") && out.stdout.contains("world"),
            "{out:?}"
        );
        assert!(out.stdout.contains("--json-schema"), "{out:?}");
        assert!(out.stdout.contains("properties"), "{out:?}");
        assert_eq!(out.stderr.trim(), "err");

        std::fs::write(
            dir.path().join("slow.cmd"),
            "@echo off\r\nping -n 30 127.0.0.1 > nul\r\n",
        )
        .unwrap();
        let started = Instant::now();
        let slow = run(
            &[dir.path().join("slow").display().to_string()],
            "",
            dir.path(),
            &[],
            Duration::from_millis(500),
        );
        assert!(slow.unwrap_err().contains("did not finish"));
        assert!(started.elapsed() < Duration::from_secs(20));
        // The batch file's child (`ping`) is gone with it.
        let left = Command::new("tasklist")
            .args(["/NH", "/FI", "IMAGENAME eq PING.EXE"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&left.stdout).to_lowercase();
        assert!(!text.contains("ping.exe"), "{text}");
    }
}
