//! The product's settings file in the data directory, `<data dir>/config.json`:
//!
//! ```json
//! {"version": 1, "daemon": {"idle": "2h"},
//!  "summaries": {"enabled": false, "harness_command": "claude -p --model sonnet",
//!                "monthly_budget_usd": 5}}
//! ```
//!
//! `memory.language` (`english`, the default, or `any`) is the memory language rule of the MCP
//! instructions and the managed rule block (`crate::language`). `embedding.model` is the
//! embedding model the daemon makes every store's vector rules use: a Hugging Face repository
//! downloaded into `<data dir>/models/` (`crate::models`), or `hash-ngram/1` (the built-in
//! embedder); without the key the stores keep the embedder they have. `install` writes both.
//!
//! `summaries` is the opt-in of session summaries (`crate::summaries`): off unless `enabled` is
//! `true`; `harness_command` is the user's own harness run once per session; the budget caps the
//! spend per calendar month (UTC); `since` (optional, a date such as `2026-11-01`) leaves older
//! sessions without a summary.
//!
//! `store.checkpoint_after` (a size such as `"16MiB"` or a number of bytes; default 16 MiB,
//! `0` = only by hand) is the WAL volume after which the engine writes a checkpoint, so a daemon
//! start never replays more than about that; `store.keep_history` (days as a number, a duration
//! such as `"30d"`, or `"forever"`; default 30 days, `0` = forever) is how far back the daemon
//! keeps the store's history (older snapshots and WAL are pruned, `crate::maintenance`).
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
const KNOWN: [&str; 15] = [
    "version",
    "memory",
    "memory.language",
    "embedding",
    "embedding.model",
    "daemon",
    "daemon.idle",
    "summaries",
    "summaries.enabled",
    "summaries.harness_command",
    "summaries.monthly_budget_usd",
    "summaries.since",
    "store",
    "store.checkpoint_after",
    "store.keep_history",
];

/// The default harness command of the summaries.
pub const DEFAULT_HARNESS_COMMAND: &str = "claude -p --model sonnet";

/// The default monthly budget of the summaries (USD).
pub const DEFAULT_MONTHLY_BUDGET_USD: f64 = 5.0;

/// The session summaries settings (`summaries`).
#[derive(Debug, Clone, PartialEq)]
pub struct Summaries {
    /// Whether the user agreed to summaries (default: no).
    pub enabled: bool,
    /// The harness command, split into the program and its arguments.
    pub harness_command: Vec<String>,
    /// The spend allowed per calendar month (UTC), in USD.
    pub monthly_budget_usd: f64,
    /// Sessions started before this date (`YYYY-MM-DD`) get no summary.
    pub since: Option<String>,
}

impl Default for Summaries {
    fn default() -> Self {
        Self {
            enabled: false,
            harness_command: split_command(DEFAULT_HARNESS_COMMAND).unwrap_or_default(),
            monthly_budget_usd: DEFAULT_MONTHLY_BUDGET_USD,
            since: None,
        }
    }
}

/// Splits a command line into words: whitespace separates, single and double quotes group,
/// a backslash escapes the next character outside single quotes.
pub fn split_command(text: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('\''), c) => word.push(c),
            (_, '\\') => {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
                in_word = true;
            }
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err(format!("unclosed quote in '{text}'"));
    }
    if in_word {
        words.push(word);
    }
    if words.is_empty() {
        return Err("the command is empty".to_string());
    }
    Ok(words)
}

/// The product settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    /// The daemon's idle time, when the file sets it.
    pub idle: Option<Duration>,
    /// The memory language, when the file sets it (`None`: the default, English).
    pub language: Option<crate::language::Language>,
    /// The embedding model, when the file sets it: a repository or `hash-ngram/1`.
    pub model: Option<String>,
    /// The session summaries settings.
    pub summaries: Summaries,
    /// `store.checkpoint_after` in bytes, when the file sets it.
    pub checkpoint_after: Option<u64>,
    /// `store.keep_history`, when the file sets it: `Some(None)` keeps everything.
    pub keep_history: Option<Option<Duration>>,
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
        if let Some(summaries) = object.get("summaries") {
            config.summaries = parse_summaries(summaries)?;
        }
        if let Some(memory) = object.get("memory") {
            let memory = memory.as_object().ok_or("'memory' must be an object")?;
            if let Some(language) = memory.get("language") {
                let text = language
                    .as_str()
                    .ok_or("'memory.language' must be \"english\" or \"any\"")?;
                config.language = Some(
                    crate::language::Language::parse(text)
                        .ok_or("'memory.language' must be \"english\" or \"any\"")?,
                );
            }
        }
        if let Some(embedding) = object.get("embedding") {
            let embedding = embedding
                .as_object()
                .ok_or("'embedding' must be an object")?;
            if let Some(model) = embedding.get("model") {
                let text = model.as_str().filter(|text| {
                    *text == crate::models::BUILTIN || crate::download::is_repo_name(text)
                });
                config.model = Some(
                    text.ok_or(
                        "'embedding.model' must be a Hugging Face repository (owner/name) or \
                         \"hash-ngram/1\"",
                    )?
                    .to_string(),
                );
            }
        }
        if let Some(store) = object.get("store") {
            let store = store.as_object().ok_or("'store' must be an object")?;
            if let Some(size) = store.get("checkpoint_after") {
                config.checkpoint_after = Some(parse_size(size).map_err(|err| {
                    format!("store.checkpoint_after: {err} (examples: \"16MiB\", 16777216, 0)")
                })?);
            }
            if let Some(keep) = store.get("keep_history") {
                config.keep_history = Some(parse_keep(keep).map_err(|err| {
                    format!("store.keep_history: {err} (examples: 30, \"30d\", \"forever\")")
                })?);
            }
        }
        let mut unknown = Vec::new();
        collect_unknown(&value, "", &mut unknown);
        config.unknown = unknown;
        Ok(config)
    }

    /// The memory language (the file's, else English).
    pub fn language(&self) -> crate::language::Language {
        self.language.unwrap_or_default()
    }

    /// The default settings file's text.
    pub fn default_text() -> String {
        let value = json!({
            "version": FORMAT,
            "daemon": {"idle": "2h"},
            "summaries": {"enabled": false, "harness_command": DEFAULT_HARNESS_COMMAND,
                          "monthly_budget_usd": DEFAULT_MONTHLY_BUDGET_USD},
        });
        format!("{}\n", crate::jsonedit::render(&value, Some("  "), 0))
    }
}

/// A size: a whole number of bytes, or a string of a number and a unit (`B`, `KB`/`MB`/`GB`
/// in powers of 1000, `KiB`/`MiB`/`GiB` in powers of 1024).
pub fn parse_size(value: &Value) -> Result<u64, String> {
    if let Some(bytes) = value.as_u64() {
        return Ok(bytes);
    }
    let text = value
        .as_str()
        .ok_or("expected a number of bytes or a size such as \"16MiB\"")?
        .trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: f64 = number
        .parse()
        .map_err(|_| format!("'{text}' does not start with a number"))?;
    let factor: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "kib" | "k" => 1024.0,
        "mib" | "m" => 1024.0 * 1024.0,
        "gib" | "g" => 1024.0 * 1024.0 * 1024.0,
        other => return Err(format!("unknown unit '{other}' in '{text}'")),
    };
    let bytes = number * factor;
    if !bytes.is_finite() || bytes < 0.0 || bytes > u64::MAX as f64 {
        return Err(format!("'{text}' is out of range"));
    }
    Ok(bytes.round() as u64)
}

/// How long the history is kept: a number of days, a duration such as `"30d"`, or `"forever"`
/// (also `0`); `None` keeps everything.
pub fn parse_keep(value: &Value) -> Result<Option<Duration>, String> {
    if let Some(days) = value.as_u64() {
        return Ok((days > 0).then(|| Duration::from_secs(days.saturating_mul(86_400))));
    }
    let text = value
        .as_str()
        .ok_or("expected a number of days, a duration or \"forever\"")?
        .trim();
    if text == "forever" {
        return Ok(None);
    }
    // A bare number is days here, as the number form is (not the seconds of a duration).
    if let Ok(days) = text.parse::<u64>() {
        return parse_keep(&Value::from(days));
    }
    let keep = crate::daemon::parse_duration(text)?;
    Ok((!keep.is_zero()).then_some(keep))
}

fn parse_summaries(value: &Value) -> Result<Summaries, String> {
    let Some(object) = value.as_object() else {
        return Err("'summaries' must be an object".to_string());
    };
    let mut out = Summaries::default();
    if let Some(enabled) = object.get("enabled") {
        out.enabled = enabled
            .as_bool()
            .ok_or("'summaries.enabled' must be true or false")?;
    }
    if let Some(command) = object.get("harness_command") {
        let text = command
            .as_str()
            .ok_or("'summaries.harness_command' must be a string such as \"claude -p\"")?;
        out.harness_command =
            split_command(text).map_err(|err| format!("summaries.harness_command: {err}"))?;
    }
    if let Some(budget) = object.get("monthly_budget_usd") {
        out.monthly_budget_usd = budget
            .as_f64()
            .filter(|usd| *usd >= 0.0 && usd.is_finite())
            .ok_or("'summaries.monthly_budget_usd' must be a number of dollars, 0 or more")?;
    }
    if let Some(since) = object.get("since") {
        let text = since
            .as_str()
            .filter(|text| is_date(text))
            .ok_or("'summaries.since' must be a date such as \"2026-11-01\"")?;
        out.since = Some(text.to_string());
    }
    Ok(out)
}

/// Whether `text` is a date `YYYY-MM-DD`.
fn is_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(i, b)| match i {
            4 | 7 => *b == b'-',
            _ => b.is_ascii_digit(),
        })
}

/// The text of the settings file `current` (`None`: none yet) with the summaries switched on or
/// off (and, when given, the monthly budget), every other byte kept.
pub fn with_summaries(
    current: Option<&str>,
    enabled: bool,
    budget_usd: Option<f64>,
) -> Result<String, String> {
    let mut text = match current {
        Some(text) => text.to_string(),
        None => Config::default_text(),
    };
    Config::parse(text.as_bytes())?;
    let mut set = |key: &str, value: &Value| -> Result<(), String> {
        if let Some((new, _)) = crate::jsonedit::set_member(&text, &["summaries"], key, value)? {
            text = new;
        }
        Ok(())
    };
    set("enabled", &json!(enabled))?;
    if let Some(budget) = budget_usd {
        set("monthly_budget_usd", &json!(budget))?;
    }
    Ok(text)
}

/// The text of the settings file `current` (`None`: none yet) with each `(section, key,
/// value)` set, every other byte kept.
pub fn with_values(
    current: Option<&str>,
    values: &[(&str, &str, Value)],
) -> Result<String, String> {
    let mut text = match current {
        Some(text) => text.to_string(),
        None => Config::default_text(),
    };
    Config::parse(text.as_bytes())?;
    for (section, key, value) in values {
        if let Some((new, _)) = crate::jsonedit::set_member(&text, &[section], key, value)? {
            text = new;
        }
    }
    Config::parse(text.as_bytes())?;
    Ok(text)
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
    fn summaries_are_off_by_default_and_parse() {
        let config = Config::parse(Config::default_text().as_bytes()).unwrap();
        assert!(!config.summaries.enabled);
        assert_eq!(
            config.summaries.harness_command,
            vec!["claude", "-p", "--model", "sonnet"]
        );
        assert_eq!(config.summaries.monthly_budget_usd, 5.0);
        let on = Config::parse(
            br#"{"version": 1, "summaries": {"enabled": true, "harness_command": "/opt/my claude/bin/claude -p --model 'son net'", "monthly_budget_usd": 2.5, "since": "2026-11-01"}}"#,
        )
        .unwrap();
        assert!(on.summaries.enabled);
        assert_eq!(
            on.summaries.harness_command,
            vec!["/opt/my", "claude/bin/claude", "-p", "--model", "son net"]
        );
        assert_eq!(on.summaries.monthly_budget_usd, 2.5);
        assert_eq!(on.summaries.since.as_deref(), Some("2026-11-01"));
        assert!(on.unknown.is_empty(), "{:?}", on.unknown);
        for bad in [
            r#"{"version": 1, "summaries": {"enabled": "yes"}}"#,
            r#"{"version": 1, "summaries": {"harness_command": ""}}"#,
            r#"{"version": 1, "summaries": {"harness_command": "claude 'x"}}"#,
            r#"{"version": 1, "summaries": {"monthly_budget_usd": -1}}"#,
            r#"{"version": 1, "summaries": {"since": "last week"}}"#,
            r#"{"version": 1, "summaries": []}"#,
        ] {
            assert!(Config::parse(bad.as_bytes()).is_err(), "{bad}");
        }
        assert_eq!(
            split_command(r#""/a b/claude" -p "#).unwrap(),
            vec!["/a b/claude", "-p"]
        );
    }

    #[test]
    fn switching_summaries_keeps_the_rest_of_the_file() {
        let text = "{\"version\": 1, \"daemon\": {\"idle\": \"30m\"}, \"x\": 1}\n";
        let on = with_summaries(Some(text), true, Some(3.0)).unwrap();
        let config = Config::parse(on.as_bytes()).unwrap();
        assert!(config.summaries.enabled);
        assert_eq!(config.summaries.monthly_budget_usd, 3.0);
        assert_eq!(config.idle, Some(Duration::from_secs(1800)));
        assert!(on.contains("\"x\": 1"), "{on}");
        let off = with_summaries(Some(&on), false, None).unwrap();
        assert!(!Config::parse(off.as_bytes()).unwrap().summaries.enabled);
        let fresh = with_summaries(None, true, None).unwrap();
        assert!(Config::parse(fresh.as_bytes()).unwrap().summaries.enabled);
        assert!(with_summaries(Some("{"), true, None).is_err());
    }

    #[test]
    fn language_and_model_parse_and_are_set_in_place() {
        let config = Config::parse(Config::default_text().as_bytes()).unwrap();
        assert_eq!(config.language, None);
        assert_eq!(config.language(), crate::language::Language::English);
        assert_eq!(config.model, None);
        let text = with_values(
            Some("{\"version\": 1, \"x\": 1}\n"),
            &[
                ("memory", "language", json!("any")),
                (
                    "embedding",
                    "model",
                    json!("minishlab/potion-multilingual-128M"),
                ),
            ],
        )
        .unwrap();
        assert!(text.contains("\"x\": 1"), "{text}");
        let config = Config::parse(text.as_bytes()).unwrap();
        assert_eq!(config.language, Some(crate::language::Language::Any));
        assert_eq!(
            config.model.as_deref(),
            Some("minishlab/potion-multilingual-128M")
        );
        assert_eq!(config.unknown, vec!["x".to_string()]);
        for bad in [
            r#"{"version": 1, "memory": {"language": "klingon"}}"#,
            r#"{"version": 1, "embedding": {"model": "not a repo"}}"#,
            r#"{"version": 1, "embedding": []}"#,
        ] {
            assert!(Config::parse(bad.as_bytes()).is_err(), "{bad}");
        }
        assert!(
            Config::parse(br#"{"version": 1, "embedding": {"model": "hash-ngram/1"}}"#).is_ok()
        );
    }

    #[test]
    fn store_maintenance_settings_parse() {
        let config = Config::parse(Config::default_text().as_bytes()).unwrap();
        assert_eq!(config.checkpoint_after, None);
        assert_eq!(config.keep_history, None);
        let day = Duration::from_secs(86_400);
        for (text, size, keep) in [
            (
                r#"{"checkpoint_after": "16MiB", "keep_history": 30}"#,
                16 << 20,
                Some(day * 30),
            ),
            (
                r#"{"checkpoint_after": 1000, "keep_history": "7d"}"#,
                1000,
                Some(day * 7),
            ),
            (
                r#"{"checkpoint_after": "2 MB", "keep_history": "12h"}"#,
                2_000_000,
                Some(day / 2),
            ),
            (
                r#"{"checkpoint_after": "512KiB", "keep_history": "45"}"#,
                512 << 10,
                Some(day * 45),
            ),
            (
                r#"{"checkpoint_after": 0, "keep_history": "forever"}"#,
                0,
                None,
            ),
            (
                r#"{"checkpoint_after": "1.5GiB", "keep_history": 0}"#,
                3 << 29,
                None,
            ),
        ] {
            let file = format!(r#"{{"version": 1, "store": {text}}}"#);
            let config = Config::parse(file.as_bytes()).unwrap();
            assert_eq!(config.checkpoint_after, Some(size), "{text}");
            assert_eq!(config.keep_history, Some(keep), "{text}");
            assert!(config.unknown.is_empty(), "{:?}", config.unknown);
        }
        for bad in [
            r#"{"version": 1, "store": []}"#,
            r#"{"version": 1, "store": {"checkpoint_after": "lots"}}"#,
            r#"{"version": 1, "store": {"checkpoint_after": "16 parsecs"}}"#,
            r#"{"version": 1, "store": {"checkpoint_after": -1}}"#,
            r#"{"version": 1, "store": {"keep_history": "a while"}}"#,
            r#"{"version": 1, "store": {"keep_history": -3}}"#,
            r#"{"version": 1, "store": {"keep_history": true}}"#,
        ] {
            let err = Config::parse(bad.as_bytes()).unwrap_err();
            assert!(err.contains("store"), "{bad}: {err}");
        }
        let config = Config::parse(br#"{"version": 1, "store": {"keep": 1}}"#).unwrap();
        assert_eq!(config.unknown, vec!["store.keep".to_string()]);
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
