//! Windows shells: the form of the hook commands `install` writes there, and Git Bash as Claude
//! Code finds it (for `doctor` and `status`).
//!
//! A harness runs a hook command in a shell, and on Windows that shell differs per harness and
//! per machine: Claude Code uses Git Bash and PowerShell when Git Bash is missing (since 2.1.120;
//! code.claude.com/docs/en/hooks, "Exec form and shell form"), Codex `cmd.exe /C`, Cursor
//! PowerShell. A quoted program path (`"C:\...\graphersal-memory.exe" hook ...`) runs in Git Bash
//! and `cmd.exe`, but PowerShell reads a quoted string as a value and starts nothing. So:
//!
//! - **Claude Code** gets the exec form (`"command"` = the binary, `"args"` = the arguments,
//!   Claude Code 2.1.139 and later): no shell at all, whatever the path holds.
//! - **Codex and Cursor** get [`all_shell_words`] when there are such words: the bare name
//!   `graphersal-memory` when it resolves on `PATH` to this binary, else the absolute path with
//!   forward slashes when it holds nothing a shell interprets, else its directory's 8.3 short
//!   path ([`short_path`]); the same for each argument. Words like these run unchanged in bash,
//!   `cmd.exe` and PowerShell. Without them Codex keeps the double-quoted form (`cmd.exe` runs
//!   it) and Cursor PowerShell's `& '...'` form.
//!
//! Every lookup is passed in ([`Lookups`], [`find_git_bash`]'s arguments), so the choice is tested
//! on every platform.

use std::path::{Path, PathBuf};

/// The program name the bare form uses.
pub const PROGRAM: &str = "graphersal-memory";

/// Whether `c` reads the same, unquoted, in bash, `cmd.exe` and PowerShell (ASCII letters and
/// digits, `_ - . / : ~`; `~` and `:` only matter at a word's start in bash, where a Windows
/// path has its drive letter).
fn is_plain(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '~')
}

/// `text` as one word that bash, `cmd.exe` and PowerShell read unquoted (backslashes become
/// forward slashes, which Windows accepts in paths); `None` when it holds anything else (a
/// space, a quote, `$`, `&`, `(`, a non-ASCII letter, ...) or starts like an option or a
/// home-relative path.
pub fn plain(text: &str) -> Option<String> {
    let text = text.replace('\\', "/");
    (!text.is_empty() && !text.starts_with(['-', '~']) && text.chars().all(is_plain))
        .then_some(text)
}

/// What [`all_shell_words`] looks up on the machine.
pub struct Lookups<'a> {
    /// The file the bare program name resolves to on `PATH` (with `PATHEXT`), if any.
    pub on_path: &'a dyn Fn(&str) -> Option<PathBuf>,
    /// Whether two paths name the same file.
    pub same_file: &'a dyn Fn(&Path, &Path) -> bool,
    /// The 8.3 short form of an existing path, when the volume keeps short names.
    pub short_path: &'a dyn Fn(&Path) -> Option<PathBuf>,
}

/// The program word of a hook command every shell runs: the bare name when it resolves to
/// `exe`, else `exe` [`plain`], else its directory's short path [`plain`] with the file name.
pub fn program_word(exe: &Path, lookups: &Lookups<'_>) -> Option<String> {
    if let Some(found) = (lookups.on_path)(PROGRAM)
        && (lookups.same_file)(&found, exe)
    {
        return Some(PROGRAM.to_string());
    }
    let text = exe.display().to_string();
    plain(&text).or_else(|| {
        // Split at either separator (a Windows path is split the same on every platform).
        let (dir, name) = text.rsplit_once(['\\', '/'])?;
        let name = plain(name)?;
        let dir = (lookups.short_path)(Path::new(dir))?;
        let dir = plain(&dir.display().to_string())?;
        Some(format!("{}/{name}", dir.trim_end_matches('/')))
    })
}

/// An argument word every shell runs: an option as it is, a value [`plain`], else its short
/// path [`plain`].
fn arg_word(arg: &str, lookups: &Lookups<'_>) -> Option<String> {
    if arg.starts_with("--") && arg.chars().all(is_plain) {
        return Some(arg.to_string());
    }
    plain(arg).or_else(|| {
        let short = (lookups.short_path)(Path::new(arg))?;
        plain(&short.display().to_string())
    })
}

/// The words of a hook command (the program, then `args`) that bash, `cmd.exe` and PowerShell
/// all run unquoted; `None` when one of them has no such form.
pub fn all_shell_words(exe: &Path, args: &[String], lookups: &Lookups<'_>) -> Option<Vec<String>> {
    let mut words = vec![program_word(exe, lookups)?];
    for arg in args {
        words.push(arg_word(arg, lookups)?);
    }
    Some(words)
}

/// [`all_shell_words`] with this machine's lookups (`PATH`, the file system, the short names).
pub fn all_shell_words_here(exe: &Path, args: &[String]) -> Option<Vec<String>> {
    let on_path = |name: &str| {
        let found = crate::harnesses::headless::program_path(name);
        (found.is_absolute() && found.is_file()).then_some(found)
    };
    let same_file =
        |a: &Path, b: &Path| crate::runtime::mcp::canonical(a) == crate::runtime::mcp::canonical(b);
    all_shell_words(
        exe,
        args,
        &Lookups {
            on_path: &on_path,
            same_file: &same_file,
            short_path: &short_path,
        },
    )
}

/// The 8.3 short form of an existing path (`GetShortPathNameW`); `None` when the call fails.
/// A volume without short names gives the path back unchanged, which [`plain`] then refuses
/// when it needed the short form.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn short_path(path: &Path) -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetShortPathNameW(long: *const u16, short: *mut u16, length: u32) -> u32;
    }
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call; a null buffer of length 0 only
    // asks for the length needed (NUL included).
    let needed = unsafe { GetShortPathNameW(wide.as_ptr(), std::ptr::null_mut(), 0) };
    if needed == 0 {
        return None;
    }
    let mut buffer = vec![0u16; usize::try_from(needed).ok()?];
    // SAFETY: `buffer` holds `needed` elements, the length passed; the call writes at most that
    // many (NUL included) and returns the count without the NUL.
    let written = unsafe { GetShortPathNameW(wide.as_ptr(), buffer.as_mut_ptr(), needed) };
    if written == 0 || written >= needed {
        return None;
    }
    buffer.truncate(usize::try_from(written).ok()?);
    Some(PathBuf::from(OsString::from_wide(&buffer)))
}

/// No short names outside Windows.
#[cfg(not(windows))]
pub fn short_path(_path: &Path) -> Option<PathBuf> {
    None
}

/// The variable that tells Claude Code where Git Bash is (code.claude.com/docs/en/env-vars).
pub const GIT_BASH_ENV: &str = "CLAUDE_CODE_GIT_BASH_PATH";

/// The default Git for Windows locations Claude Code checks first.
const GIT_ROOTS: [&str; 2] = [r"C:\Program Files\Git", r"C:\Program Files (x86)\Git"];

/// Git Bash as Claude Code finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitBash {
    /// The `bash.exe` found, if any.
    pub path: Option<String>,
    /// Why a configured path was not used (it does not exist, has another name, or is WSL's).
    pub ignored: Option<String>,
}

/// The last component of a Windows or POSIX path.
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or_default()
}

/// `path` without its last `n` components (either separator).
fn up(path: &str, n: usize) -> Option<&str> {
    let mut rest = path.trim_end_matches(['\\', '/']);
    for _ in 0..n {
        rest = &rest[..rest.rfind(['\\', '/'])?];
    }
    Some(rest)
}

/// Whether `path` is WSL's launcher in the Windows system directory, never Git Bash.
fn is_wsl_bash(path: &str) -> bool {
    let lower = path.to_ascii_lowercase().replace('/', "\\");
    lower.contains("\\windows\\system32\\") || lower.contains("\\windows\\sysnative\\")
}

/// Finds Git Bash the way Claude Code does (code.claude.com/docs/en/troubleshoot-install, "Claude
/// Code on Windows requires either Git for Windows (for bash) or PowerShell", and the
/// `CLAUDE_CODE_GIT_BASH_PATH` entry of code.claude.com/docs/en/env-vars): `configured` (the
/// variable, from Claude Code's settings `env` or the environment) when the file exists and is
/// named `bash.exe`, `sh.exe`, `bash` or `sh`; else `bin\bash.exe` under `C:\Program Files\Git`,
/// then `C:\Program Files (x86)\Git`; else the `bin\bash.exe` of the Git installation whose
/// `git` is on `PATH` (`git_on_path`: `<git>\cmd\git.exe`, `<git>\bin\git.exe` or
/// `<git>\mingw64\bin\git.exe`). WSL's `C:\Windows\System32\bash.exe` never counts.
pub fn find_git_bash(
    configured: Option<&str>,
    git_on_path: Option<&str>,
    is_file: &dyn Fn(&str) -> bool,
) -> GitBash {
    let mut ignored = None;
    if let Some(configured) = configured.filter(|c| !c.trim().is_empty()) {
        let name = file_name(configured).to_ascii_lowercase();
        if !is_file(configured) {
            ignored = Some(format!("{GIT_BASH_ENV}={configured} does not exist"));
        } else if !matches!(name.as_str(), "bash.exe" | "sh.exe" | "bash" | "sh") {
            ignored = Some(format!(
                "{GIT_BASH_ENV}={configured} is not named bash.exe or sh.exe"
            ));
        } else if is_wsl_bash(configured) {
            ignored = Some(format!(
                "{GIT_BASH_ENV}={configured} is WSL's bash, not Git Bash"
            ));
        } else {
            return GitBash {
                path: Some(configured.to_string()),
                ignored: None,
            };
        }
    }
    let mut candidates: Vec<String> = GIT_ROOTS
        .iter()
        .map(|root| format!(r"{root}\bin\bash.exe"))
        .collect();
    if let Some(git) = git_on_path {
        let parent = up(git, 1).map(file_name).unwrap_or_default();
        let root = match parent.to_ascii_lowercase().as_str() {
            "cmd" => up(git, 2),
            "bin"
                if up(git, 2)
                    .map(file_name)
                    .unwrap_or_default()
                    .eq_ignore_ascii_case("mingw64") =>
            {
                up(git, 3)
            }
            "bin" => up(git, 2),
            _ => None,
        };
        if let Some(root) = root {
            candidates.push(format!(r"{root}\bin\bash.exe"));
        }
    }
    GitBash {
        path: candidates
            .into_iter()
            .find(|candidate| !is_wsl_bash(candidate) && is_file(candidate)),
        ignored,
    }
}

/// [`find_git_bash`] on this machine: the variable from Claude Code's `settings` (its `env`
/// object) else from this process's environment, `git` resolved on `PATH`.
pub fn find_git_bash_here(settings: &serde_json::Value) -> GitBash {
    let configured = settings["env"][GIT_BASH_ENV]
        .as_str()
        .map(str::to_string)
        .or_else(|| std::env::var(GIT_BASH_ENV).ok());
    let git = crate::harnesses::headless::program_path("git");
    let git = (git.is_absolute() && git.is_file()).then(|| git.display().to_string());
    find_git_bash(configured.as_deref(), git.as_deref(), &|path: &str| {
        Path::new(path).is_file()
    })
}

/// Whether a Claude Code hook entry is a shell-form command whose program is quoted: Git Bash
/// runs it, PowerShell (Claude Code's hook shell without Git Bash) does not.
pub fn needs_git_bash(hook: &serde_json::Value) -> bool {
    hook["args"].is_null() && hook["command"].as_str().is_some_and(|c| c.starts_with('"'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookups<'a>(
        on_path: &'a dyn Fn(&str) -> Option<PathBuf>,
        short_path: &'a dyn Fn(&Path) -> Option<PathBuf>,
    ) -> Lookups<'a> {
        Lookups {
            on_path,
            same_file: &|a: &Path, b: &Path| a == b,
            short_path,
        }
    }

    const EXE: &str =
        r"C:\Users\ann\AppData\Local\Programs\graphersal-memory\graphersal-memory.exe";
    const SPACED: &str =
        r"C:\Users\Ann Lee\AppData\Local\Programs\graphersal-memory\graphersal-memory.exe";

    #[test]
    fn plain_words_are_what_every_shell_reads_unquoted() {
        assert_eq!(
            plain(r"C:\Users\ann\x.exe").as_deref(),
            Some("C:/Users/ann/x.exe")
        );
        assert_eq!(
            plain("/opt/gm/bin/graphersal-memory").as_deref(),
            Some("/opt/gm/bin/graphersal-memory")
        );
        assert_eq!(
            plain(r"C:\PROGRA~1\GRAPHE~1").as_deref(),
            Some("C:/PROGRA~1/GRAPHE~1")
        );
        for bad in [
            r"C:\Users\Ann Lee\x.exe",
            r"C:\a$b\x.exe",
            r"C:\a&b",
            r"C:\a(1)",
            r"C:\O'Neil",
            "C:\\Users\\Анна\\x.exe",
            "-x",
            "~/x",
            "",
            "a;b",
            "a`b",
            "a%b",
        ] {
            assert_eq!(plain(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_bare_name_when_path_resolves_to_this_binary() {
        let on_path = |name: &str| (name == PROGRAM).then(|| PathBuf::from(SPACED));
        let words = all_shell_words(
            Path::new(SPACED),
            &["--data-dir".into(), r"D:\gm\data".into()],
            &lookups(&on_path, &|_| None),
        );
        assert_eq!(
            words,
            Some(vec![
                PROGRAM.into(),
                "--data-dir".into(),
                "D:/gm/data".into()
            ])
        );
    }

    #[test]
    fn another_binary_on_path_is_not_used() {
        let on_path = |_: &str| Some(PathBuf::from(r"C:\other\graphersal-memory.exe"));
        let words = all_shell_words(Path::new(EXE), &[], &lookups(&on_path, &|_| None));
        assert_eq!(
            words,
            Some(vec![
                "C:/Users/ann/AppData/Local/Programs/graphersal-memory/graphersal-memory.exe"
                    .into()
            ])
        );
    }

    #[test]
    fn a_path_with_a_space_takes_the_short_directory() {
        let short = |path: &Path| {
            (path == Path::new(r"C:\Users\Ann Lee\AppData\Local\Programs\graphersal-memory"))
                .then(|| PathBuf::from(r"C:\Users\ANNLEE~1\AppData\Local\Programs\GRAPHE~1"))
        };
        let words = all_shell_words(Path::new(SPACED), &[], &lookups(&|_| None, &short));
        assert_eq!(
            words,
            Some(vec![
                "C:/Users/ANNLEE~1/AppData/Local/Programs/GRAPHE~1/graphersal-memory.exe".into()
            ])
        );
        // An argument that needs it gets its short path too.
        let short_arg = |path: &Path| {
            (path == Path::new(r"C:\Users\Ann Lee\gm data"))
                .then(|| PathBuf::from(r"C:\Users\ANNLEE~1\GMDATA~1"))
        };
        let words = all_shell_words(
            Path::new(EXE),
            &["--data-dir".into(), r"C:\Users\Ann Lee\gm data".into()],
            &lookups(&|_| None, &short_arg),
        );
        assert_eq!(
            words.map(|w| w[2].clone()).as_deref(),
            Some("C:/Users/ANNLEE~1/GMDATA~1")
        );
    }

    #[test]
    fn no_form_when_a_word_cannot_be_made_plain() {
        // No short names on the volume: the long path comes back, still with a space.
        let same = |path: &Path| Some(path.to_path_buf());
        assert_eq!(
            all_shell_words(Path::new(SPACED), &[], &lookups(&|_| None, &same)),
            None
        );
        assert_eq!(
            all_shell_words(Path::new(SPACED), &[], &lookups(&|_| None, &|_| None)),
            None
        );
        assert_eq!(
            all_shell_words(
                Path::new(EXE),
                &["--data-dir".into(), r"C:\a $x".into()],
                &lookups(&|_| None, &|_| None)
            ),
            None
        );
    }

    #[test]
    fn git_bash_is_found_like_claude_code_finds_it() {
        let files = [
            r"C:\Program Files (x86)\Git\bin\bash.exe",
            r"D:\tools\Git\bin\bash.exe",
            r"D:\portable\bash.exe",
            r"D:\portable\git-bash.exe",
            r"C:\Windows\System32\bash.exe",
        ];
        let is_file = |path: &str| files.contains(&path);
        // The variable first, when it exists and is named bash/sh.
        let found = find_git_bash(Some(r"D:\portable\bash.exe"), None, &is_file);
        assert_eq!(found.path.as_deref(), Some(r"D:\portable\bash.exe"));
        // Another name, a missing file, or WSL's bash: ignored, then the default locations.
        for (configured, why) in [
            (r"D:\portable\git-bash.exe", "not named bash.exe"),
            (r"D:\nowhere\bash.exe", "does not exist"),
            (r"C:\Windows\System32\bash.exe", "WSL's bash"),
        ] {
            let found = find_git_bash(Some(configured), None, &is_file);
            assert_eq!(
                found.path.as_deref(),
                Some(r"C:\Program Files (x86)\Git\bin\bash.exe")
            );
            assert!(
                found.ignored.as_deref().unwrap_or_default().contains(why),
                "{found:?}"
            );
        }
        // The git on PATH: its installation's bin\bash.exe.
        let only_tools = |path: &str| path == r"D:\tools\Git\bin\bash.exe";
        for git in [
            r"D:\tools\Git\cmd\git.exe",
            r"D:\tools\Git\bin\git.exe",
            r"D:\tools\Git\mingw64\bin\git.exe",
        ] {
            let found = find_git_bash(None, Some(git), &only_tools);
            assert_eq!(
                found.path.as_deref(),
                Some(r"D:\tools\Git\bin\bash.exe"),
                "{git}"
            );
        }
        // Nothing: WSL's bash.exe alone does not count.
        let only_wsl = |path: &str| path == r"C:\Windows\System32\bash.exe";
        let found = find_git_bash(None, Some(r"C:\Windows\System32\git.exe"), &only_wsl);
        assert_eq!(found.path, None);
    }

    #[test]
    fn a_quoted_shell_command_needs_git_bash() {
        use serde_json::json;
        assert!(needs_git_bash(
            &json!({"command": "\"C:\\gm\\graphersal-memory.exe\" hook session-start"})
        ));
        assert!(!needs_git_bash(
            &json!({"command": "C:/gm/graphersal-memory.exe hook session-start"})
        ));
        assert!(!needs_git_bash(
            &json!({"command": "C:\\gm\\graphersal-memory.exe", "args": ["hook", "session-start"]})
        ));
    }
}
