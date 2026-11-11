//! Where the memory lives: the per-user data directory and the stores in it.
//!
//! The data directory is, in this order: `--data-dir`, the environment variable
//! `GRAPHERSAL_MEMORY_DIR`, then the platform's per-user data directory (macOS
//! `~/Library/Application Support/graphersal-memory`, Windows `%LOCALAPPDATA%\graphersal-memory`,
//! elsewhere `$XDG_DATA_HOME/graphersal-memory` or `~/.local/share/graphersal-memory`). The
//! default store is `<data dir>/stores/default`, created on first use.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use graphersal::persist::{Store, StoreOptions};

/// The environment variable that overrides the data directory.
pub const DATA_DIR_ENV: &str = "GRAPHERSAL_MEMORY_DIR";

/// The directory name under the platform's data directory.
const APP_DIR: &str = "graphersal-memory";

/// The entries graphersal-memory creates in its data directory, and the only ones
/// `uninstall --purge` deletes there (a data directory chosen inside a folder that holds other
/// files keeps them).
pub const OUR_ENTRIES: [&str; 10] = [
    "stores",
    "registry.json",
    "config.json",
    "install",
    "import",
    "projects.json",
    "run",
    "models",
    "sessions",
    "bench",
];

/// Entries the operating system puts into any folder (never a reason to call it someone
/// else's).
const SYSTEM_ENTRIES: [&str; 2] = [".DS_Store", "desktop.ini"];

/// The entries of `dir` that are neither ours ([`OUR_ENTRIES`]) nor the system's, by name
/// (empty when `dir` does not exist).
pub fn foreign_entries(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !OUR_ENTRIES.contains(&name.as_str()))
        .filter(|name| !SYSTEM_ENTRIES.contains(&name.as_str()))
        .collect();
    names.sort();
    names
}

/// Why `dir` cannot be graphersal-memory's data directory: a file, or a folder that holds
/// other files and none of ours (install would mix its data with them). `None` when it is
/// missing, empty, or ours.
pub fn foreign_data_dir(dir: &Path) -> Option<String> {
    if dir.exists() && !dir.is_dir() {
        return Some(format!(
            "the data directory {} is a file, not a folder; choose another with --data-dir \
             (or {DATA_DIR_ENV})",
            dir.display()
        ));
    }
    let foreign = foreign_entries(dir);
    let ours = OUR_ENTRIES.iter().any(|name| dir.join(name).exists());
    if foreign.is_empty() || ours {
        return None;
    }
    let shown: Vec<&str> = foreign.iter().take(3).map(String::as_str).collect();
    Some(format!(
        "the data directory {} already holds other files ({}{}) and no graphersal-memory data; \
         nothing was changed. Choose a new or empty folder, for example {} (--data-dir or \
         {DATA_DIR_ENV})",
        dir.display(),
        shown.join(", "),
        if foreign.len() > 3 {
            format!(" and {} more", foreign.len() - 3)
        } else {
            String::new()
        },
        dir.join(APP_DIR).display()
    ))
}

/// The platform family that decides the default data directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// macOS.
    MacOs,
    /// Windows.
    Windows,
    /// Linux and the other Unix systems (XDG).
    Xdg,
}

impl Platform {
    /// The platform this binary runs on.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Xdg
        }
    }
}

/// The user's home directory as the harnesses see it (`env` reads one variable): on Windows
/// `USERPROFILE`, else `HOME` (Node's and Bun's `os.homedir()` and Rust's `dirs::home_dir` read
/// the profile directory there; a `HOME` set by Git Bash or Cygwin may differ); elsewhere
/// `HOME`, else `USERPROFILE`.
pub fn home_dir(env: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf, String> {
    home_dir_for(env, cfg!(windows))
}

fn home_dir_for(env: &dyn Fn(&str) -> Option<String>, windows: bool) -> Result<PathBuf, String> {
    let set = |name: &str| env(name).filter(|value| !value.is_empty());
    let (first, second) = if windows {
        ("USERPROFILE", "HOME")
    } else {
        ("HOME", "USERPROFILE")
    };
    set(first)
        .or_else(|| set(second))
        .map(PathBuf::from)
        .ok_or_else(|| format!("cannot find the home directory ({first} is not set)"))
}

/// Resolves the data directory from the flag, the environment (`env` reads one variable) and
/// the platform. Fails when no directory can be found (no home directory).
pub fn resolve_data_dir(
    flag: Option<&Path>,
    env: &dyn Fn(&str) -> Option<String>,
    platform: Platform,
) -> Result<PathBuf, String> {
    if let Some(dir) = flag {
        return Ok(dir.to_path_buf());
    }
    let set = |name: &str| env(name).filter(|value| !value.is_empty());
    if let Some(dir) = set(DATA_DIR_ENV) {
        return Ok(PathBuf::from(dir));
    }
    let missing = |what: &str| {
        format!(
            "cannot find the per-user data directory ({what} is not set): pass --data-dir or set \
             {DATA_DIR_ENV}"
        )
    };
    match platform {
        Platform::MacOs => {
            let home = set("HOME").ok_or_else(|| missing("HOME"))?;
            Ok(PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join(APP_DIR))
        }
        Platform::Windows => {
            let base = set("LOCALAPPDATA").ok_or_else(|| missing("LOCALAPPDATA"))?;
            Ok(PathBuf::from(base).join(APP_DIR))
        }
        Platform::Xdg => {
            // XDG paths are Unix paths: absolute means a leading `/` (also when this code runs
            // elsewhere, as the tests of every platform do).
            if let Some(base) = set("XDG_DATA_HOME").filter(|b| b.starts_with('/')) {
                return Ok(PathBuf::from(base).join(APP_DIR));
            }
            let home = set("HOME").ok_or_else(|| missing("HOME"))?;
            Ok(PathBuf::from(home)
                .join(".local")
                .join("share")
                .join(APP_DIR))
        }
    }
}

/// The default store's directory in `data_dir`.
pub fn default_store_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("stores").join("default")
}

/// Opens the store in `dir` with the default options, creating it (and its parent directories)
/// when it does not exist or is an empty directory.
#[cfg(test)]
pub fn open_or_create(dir: &Path) -> Result<Arc<Store>, String> {
    open_or_create_with(dir, StoreOptions::new())
}

/// [`open_or_create`] with `options` (the product's: `crate::maintenance::Settings`).
pub fn open_or_create_with(dir: &Path, options: StoreOptions) -> Result<Arc<Store>, String> {
    let fresh = match std::fs::read_dir(dir) {
        Ok(mut entries) => entries.next().is_none(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => true,
        Err(err) => return Err(format!("cannot read the store {}: {err}", dir.display())),
    };
    let store = if fresh {
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                format!(
                    "cannot create the data directory {}: {err}",
                    parent.display()
                )
            })?;
        }
        Store::create(dir, options)
            .map_err(|err| format!("cannot create the store {}: {err}", dir.display()))?
    } else {
        Store::open(dir, options)
            .map_err(|err| format!("cannot open the store {}: {err}", dir.display()))?
    };
    Ok(Arc::new(store))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn the_flag_wins_then_the_environment() {
        let env = env_of(&[(DATA_DIR_ENV, "/from/env"), ("HOME", "/home/u")]);
        assert_eq!(
            resolve_data_dir(Some(Path::new("/flag")), &env, Platform::Xdg).unwrap(),
            PathBuf::from("/flag")
        );
        assert_eq!(
            resolve_data_dir(None, &env, Platform::MacOs).unwrap(),
            PathBuf::from("/from/env")
        );
    }

    #[test]
    fn the_home_directory_is_the_profile_on_windows() {
        let both = env_of(&[("HOME", "/home/u"), ("USERPROFILE", "/profile")]);
        assert_eq!(
            home_dir_for(&both, false).unwrap(),
            PathBuf::from("/home/u")
        );
        assert_eq!(
            home_dir_for(&both, true).unwrap(),
            PathBuf::from("/profile")
        );
        let home = env_of(&[("HOME", "/home/u")]);
        assert_eq!(home_dir_for(&home, true).unwrap(), PathBuf::from("/home/u"));
        let profile = env_of(&[("USERPROFILE", "/profile")]);
        assert_eq!(
            home_dir_for(&profile, false).unwrap(),
            PathBuf::from("/profile")
        );
        let none = env_of(&[]);
        assert!(
            home_dir_for(&none, true)
                .unwrap_err()
                .contains("USERPROFILE")
        );
        assert!(home_dir_for(&none, false).unwrap_err().contains("HOME"));
    }

    #[test]
    fn platform_defaults() {
        let env = env_of(&[
            ("HOME", "/home/u"),
            ("LOCALAPPDATA", "C:\\Users\\u\\AppData\\Local"),
        ]);
        assert_eq!(
            resolve_data_dir(None, &env, Platform::MacOs).unwrap(),
            PathBuf::from("/home/u/Library/Application Support/graphersal-memory")
        );
        assert_eq!(
            resolve_data_dir(None, &env, Platform::Xdg).unwrap(),
            PathBuf::from("/home/u/.local/share/graphersal-memory")
        );
        assert_eq!(
            resolve_data_dir(None, &env, Platform::Windows).unwrap(),
            PathBuf::from("C:\\Users\\u\\AppData\\Local").join("graphersal-memory")
        );
        let xdg = env_of(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "/data")]);
        assert_eq!(
            resolve_data_dir(None, &xdg, Platform::Xdg).unwrap(),
            PathBuf::from("/data/graphersal-memory")
        );
        // A relative XDG_DATA_HOME is ignored, as the XDG specification says.
        let relative = env_of(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "data")]);
        assert_eq!(
            resolve_data_dir(None, &relative, Platform::Xdg).unwrap(),
            PathBuf::from("/home/u/.local/share/graphersal-memory")
        );
    }

    #[test]
    fn no_home_is_an_error_naming_the_override() {
        let env = env_of(&[]);
        let err = resolve_data_dir(None, &env, Platform::Xdg).unwrap_err();
        assert!(
            err.contains("--data-dir") && err.contains(DATA_DIR_ENV),
            "{err}"
        );
    }
}
