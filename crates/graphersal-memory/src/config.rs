//! The product's settings file in the data directory, `<data dir>/config.json`:
//!
//! ```json
//! {"version": 1, "daemon": {"idle": "2h"}}
//! ```
//!
//! A missing file means the defaults; `install` writes one with the defaults when there is none,
//! so its shape is visible. Precedence for every setting: the command-line flag, then the
//! environment variable, then this file, then the built-in default. Unknown keys are kept and
//! ignored (a newer version may add settings); `doctor` names them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

/// The settings file's name in the data directory.
pub const CONFIG_FILE: &str = "config.json";

/// The format version of the settings file.
const FORMAT: u64 = 1;

/// The keys this version knows, as dotted paths.
const KNOWN: [&str; 3] = ["version", "daemon", "daemon.idle"];

/// The product settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// The daemon's idle time, when the file sets it.
    pub idle: Option<Duration>,
    /// Keys this version does not know (dotted paths).
    pub unknown: Vec<String>,
}

/// The settings file of `data_dir`.
pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(CONFIG_FILE)
}

impl Config {
    /// Reads the settings of `data_dir`; a missing file gives the defaults.
    pub fn load(data_dir: &Path) -> Result<Self, String> {
        let file = path(data_dir);
        match std::fs::read(&file) {
            Ok(bytes) => Self::parse(&bytes)
                .map_err(|err| format!("the settings file {} is invalid: {err}", file.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(format!("cannot read {}: {err}", file.display())),
        }
    }

    /// Parses a settings file.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let value: Value = serde_json::from_slice(bytes).map_err(|err| err.to_string())?;
        let Some(object) = value.as_object() else {
            return Err("expected a JSON object".to_string());
        };
        match object.get("version").and_then(Value::as_u64) {
            Some(FORMAT) => {}
            _ => {
                return Err(format!(
                    "version {} is not supported (expected {FORMAT})",
                    object.get("version").unwrap_or(&Value::Null)
                ));
            }
        }
        let mut config = Config::default();
        if let Some(daemon) = object.get("daemon") {
            let Some(daemon) = daemon.as_object() else {
                return Err("'daemon' must be an object".to_string());
            };
            if let Some(idle) = daemon.get("idle") {
                let text = idle
                    .as_str()
                    .ok_or("'daemon.idle' must be a string such as \"2h\"")?;
                config.idle = Some(
                    crate::daemon::parse_duration(text)
                        .map_err(|err| format!("daemon.idle: {err}"))?,
                );
            }
        }
        let mut unknown = Vec::new();
        collect_unknown(&value, "", &mut unknown);
        config.unknown = unknown;
        Ok(config)
    }

    /// The default settings file's text.
    pub fn default_text() -> String {
        let value = json!({"version": FORMAT, "daemon": {"idle": "2h"}});
        format!("{}\n", crate::jsonedit::render(&value, Some("  "), 0))
    }
}

fn collect_unknown(value: &Value, prefix: &str, out: &mut Vec<String>) {
    let Some(object) = value.as_object() else {
        return;
    };
    for (key, child) in object {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        if KNOWN.contains(&path.as_str()) {
            collect_unknown(child, &path, out);
        } else {
            out.push(path);
        }
    }
}

/// The daemon's idle time: the flag, else the environment variable `GRAPHERSAL_MEMORY_IDLE`,
/// else the settings file, else two hours.
pub fn idle(
    flag: Option<Duration>,
    env: Option<String>,
    config: &Config,
) -> Result<Duration, String> {
    if let Some(idle) = flag {
        return Ok(idle);
    }
    if let Some(text) = env.filter(|text| !text.is_empty()) {
        return crate::daemon::parse_duration(&text)
            .map_err(|err| format!("{}: {err}", crate::daemon::IDLE_ENV));
    }
    Ok(config.idle.unwrap_or(crate::daemon::DEFAULT_IDLE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_text_parses_to_two_hours() {
        let config = Config::parse(Config::default_text().as_bytes()).unwrap();
        assert_eq!(config.idle, Some(Duration::from_secs(7200)));
        assert!(config.unknown.is_empty());
    }

    #[test]
    fn flag_then_env_then_file_then_default() {
        let file = Config::parse(br#"{"version": 1, "daemon": {"idle": "30m"}}"#).unwrap();
        let none = Config::default();
        let s = Duration::from_secs;
        assert_eq!(idle(Some(s(5)), Some("1h".into()), &file).unwrap(), s(5));
        assert_eq!(idle(None, Some("1h".into()), &file).unwrap(), s(3600));
        assert_eq!(idle(None, Some(String::new()), &file).unwrap(), s(1800));
        assert_eq!(
            idle(None, None, &none).unwrap(),
            crate::daemon::DEFAULT_IDLE
        );
        assert!(idle(None, Some("soon".into()), &file).is_err());
    }

    #[test]
    fn bad_files_are_errors_and_unknown_keys_are_listed() {
        assert!(Config::parse(b"[]").is_err());
        assert!(Config::parse(br#"{"version": 2}"#).is_err());
        assert!(Config::parse(br#"{"version": 1, "daemon": {"idle": 5}}"#).is_err());
        assert!(Config::parse(br#"{"version": 1, "daemon": {"idle": "later"}}"#).is_err());
        let config =
            Config::parse(br#"{"version": 1, "daemon": {"idel": "1h"}, "ui": {}}"#).unwrap();
        assert_eq!(
            config.unknown,
            vec!["daemon.idel".to_string(), "ui".to_string()]
        );
        assert_eq!(config.idle, None);
    }

    #[test]
    fn a_missing_file_is_the_defaults() {
        let dir = crate::mcp_tests::TempDir::new("cfg");
        assert_eq!(Config::load(dir.path()).unwrap(), Config::default());
        std::fs::write(path(dir.path()), "{").unwrap();
        let err = Config::load(dir.path()).unwrap_err();
        assert!(err.contains("config.json"), "{err}");
    }
}
