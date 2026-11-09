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

/// Opens the store in `dir`, creating it (and its parent directories) when it does not exist
/// or is an empty directory.
pub fn open_or_create(dir: &Path) -> Result<Arc<Store>, String> {
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
        Store::create(dir, StoreOptions::new())
            .map_err(|err| format!("cannot create the store {}: {err}", dir.display()))?
    } else {
        Store::open(dir, StoreOptions::new())
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
