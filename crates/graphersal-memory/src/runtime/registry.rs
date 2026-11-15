//! The store registry: which store serves which project, a small JSON file in the data
//! directory (`<data dir>/registry.json`):
//!
//! ```json
//! {"version": 1, "default": "default", "projects": {"/work/client-a": "client-a"}}
//! ```
//!
//! A project listed in `projects` goes to its own store (`<data dir>/stores/<name>`), every
//! other project to the default store. Global memories (no `project` property) live in the
//! store they were written to; with one store (today) every project sees them. Isolated project
//! stores, and the one-way copy of the global memory into them, arrive with isolation (stage
//! M7); until then nothing writes `projects`, but a hand-written entry is honoured.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The registry's file name in the data directory.
pub const REGISTRY_FILE: &str = "registry.json";

/// The registry's format version.
const FORMAT: u64 = 1;

/// The name of the default store.
pub const DEFAULT_STORE: &str = "default";

/// The project -> store routes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    default: String,
    projects: BTreeMap<String, String>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            default: DEFAULT_STORE.to_string(),
            projects: BTreeMap::new(),
        }
    }
}

impl Registry {
    /// Reads the registry of `data_dir`; when there is none, writes the default one (so its
    /// shape is visible) and returns it.
    pub fn load_or_create(data_dir: &Path) -> Result<Self, String> {
        let path = data_dir.join(REGISTRY_FILE);
        match std::fs::read(&path) {
            Ok(bytes) => Self::parse(&bytes)
                .map_err(|err| format!("the store registry {} is invalid: {err}", path.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let registry = Self::default();
                std::fs::create_dir_all(data_dir).map_err(|err| {
                    format!(
                        "cannot create the data directory {}: {err}",
                        data_dir.display()
                    )
                })?;
                std::fs::write(&path, format!("{}\n", registry.to_json())).map_err(|err| {
                    format!("cannot write the store registry {}: {err}", path.display())
                })?;
                Ok(registry)
            }
            Err(err) => Err(format!(
                "cannot read the store registry {}: {err}",
                path.display()
            )),
        }
    }

    /// Parses a registry file.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let value: Value = serde_json::from_slice(bytes).map_err(|err| err.to_string())?;
        let version = value.get("version").and_then(Value::as_u64);
        if version != Some(FORMAT) {
            return Err(format!(
                "version {} is not supported (expected {FORMAT})",
                value.get("version").unwrap_or(&Value::Null)
            ));
        }
        let default = match value.get("default") {
            None => DEFAULT_STORE.to_string(),
            Some(name) => store_name(name)?,
        };
        let mut projects = BTreeMap::new();
        if let Some(entries) = value.get("projects") {
            let entries = entries
                .as_object()
                .ok_or_else(|| "projects is not an object".to_string())?;
            for (project, store) in entries {
                projects.insert(project.clone(), store_name(store)?);
            }
        }
        Ok(Self { default, projects })
    }

    /// The registry as JSON.
    pub fn to_json(&self) -> Value {
        json!({"version": FORMAT, "default": self.default, "projects": self.projects})
    }

    /// The store that serves `project`.
    pub fn store_for(&self, project: &str) -> &str {
        self.projects.get(project).unwrap_or(&self.default)
    }

    /// The directory of the store `name` in `data_dir`.
    pub fn store_dir(data_dir: &Path, name: &str) -> PathBuf {
        data_dir.join("stores").join(name)
    }
}

/// A store name: letters, digits, `-`, `_` and `.` (not leading), at most 64 characters, so it
/// is a safe directory name on every platform.
fn store_name(value: &Value) -> Result<String, String> {
    let name = value
        .as_str()
        .ok_or_else(|| format!("store name {value} is not a string"))?;
    let valid = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if valid {
        Ok(name.to_string())
    } else {
        Err(format!(
            "store name '{name}' is not valid (letters, digits, '-', '_', '.', at most 64)"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_registry_is_written_with_the_default_store() {
        let dir = crate::runtime::mcp_tests::TempDir::new("registry");
        let registry = Registry::load_or_create(dir.path()).unwrap();
        assert_eq!(registry.store_for("/w/a"), DEFAULT_STORE);
        let written = std::fs::read(dir.path().join(REGISTRY_FILE)).unwrap();
        assert_eq!(Registry::parse(&written).unwrap(), registry);
        assert_eq!(Registry::load_or_create(dir.path()).unwrap(), registry);
    }

    #[test]
    fn a_listed_project_goes_to_its_store() {
        let registry =
            Registry::parse(br#"{"version":1,"default":"main","projects":{"/w/secret":"secret"}}"#)
                .unwrap();
        assert_eq!(registry.store_for("/w/secret"), "secret");
        assert_eq!(registry.store_for("/w/other"), "main");
    }

    #[test]
    fn invalid_registries_are_refused() {
        for bad in [
            &br#"{"version":2}"#[..],
            br#"{"version":1,"default":"../x"}"#,
            br#"{"version":1,"projects":{"/w":".hidden"}}"#,
            br#"{"version":1,"projects":[]}"#,
            b"nope",
        ] {
            assert!(
                Registry::parse(bad).is_err(),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }
}
