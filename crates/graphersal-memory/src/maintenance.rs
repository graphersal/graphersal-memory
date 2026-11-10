//! Store maintenance: how often the engine checkpoints a store and how much history the daemon
//! keeps.
//!
//! - **Checkpoints**: the engine writes a snapshot by itself once the WAL since the last one
//!   passes `store.checkpoint_after` (default [`DEFAULT_CHECKPOINT_AFTER`], 16 MiB; the
//!   engine's own default is 256 MiB, which a memory store may never reach, so every start would
//!   replay its whole history). A checkpoint is merged from the last snapshot and the WAL after
//!   it, in the background; the embedding models are blobs the snapshots reference, stored once.
//! - **History**: nothing is ever deleted implicitly by the engine; the daemon prunes snapshots
//!   and WAL older than `store.keep_history` (default [`DEFAULT_KEEP_DAYS`] days, `0`/`forever`:
//!   never) through the engine's `Store::prune`, at most once a day and after each new snapshot
//!   ([`prune_old_history`], called by the daemon's maintenance thread). The engine always keeps
//!   the last two snapshots and the WAL between them (the donors of a repair).
//! - **Copies**: the bench copies the live store with `Store::fork_dir`, which reads the files
//!   without the store's lock; a prune meanwhile could remove a file the copy still reads. So a
//!   copy holds the store's copy lock shared ([`hold_for_copy`]) and a prune takes it
//!   exclusively without waiting ([`prune_old_history`] skips and tries again later).
//! - **State**: `<data dir>/stores/<name>.maintenance.json` records the last check and the last
//!   prune for `status`.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use graphersal::persist::{Store, StoreOptions};
use serde_json::{Value, json};

use crate::config::Config;

/// The default WAL volume after which the engine writes a checkpoint: 16 MiB (a start replays at
/// most about that; measured in `docs/how-it-works.md`).
pub const DEFAULT_CHECKPOINT_AFTER: u64 = 16 << 20;

/// The default history kept: 30 days.
pub const DEFAULT_KEEP_DAYS: u64 = 30;

/// How often the history is checked when no new snapshot appeared: once a day.
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);

/// The WAL since the last snapshot, as a multiple of the threshold, above which `doctor` warns
/// (a checkpoint that keeps failing).
pub const WARN_FACTOR: u64 = 4;

/// The maintenance settings of the stores (from `config.json`'s `store` section).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// The WAL bytes after which the engine checkpoints (`0`: only by hand).
    pub checkpoint_after: u64,
    /// How far back the history is kept; `None`: forever (never pruned).
    pub keep_history: Option<Duration>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            checkpoint_after: DEFAULT_CHECKPOINT_AFTER,
            keep_history: Some(Duration::from_secs(DEFAULT_KEEP_DAYS * 86_400)),
        }
    }
}

impl Settings {
    /// The settings `config` gives (its values, else the defaults).
    pub fn from_config(config: &Config) -> Self {
        let defaults = Self::default();
        Self {
            checkpoint_after: config.checkpoint_after.unwrap_or(defaults.checkpoint_after),
            keep_history: config.keep_history.unwrap_or(defaults.keep_history),
        }
    }

    /// The settings of `data_dir`'s `config.json`; an unreadable file gives the defaults (the
    /// daemon logs and `doctor` reports the file's problem).
    pub fn load(data_dir: &Path) -> Self {
        Config::load(data_dir)
            .map(|config| Self::from_config(&config))
            .unwrap_or_default()
    }

    /// The engine options a store is opened with.
    pub fn store_options(&self) -> StoreOptions {
        StoreOptions::new().with_checkpoint_after_bytes(self.checkpoint_after)
    }
}

/// `keep` for people: whole days, else hours, else seconds.
pub fn duration_text(keep: Duration) -> String {
    let secs = keep.as_secs();
    if secs >= 86_400 && secs.is_multiple_of(86_400) {
        let days = secs / 86_400;
        format!("{days} day{}", if days == 1 { "" } else { "s" })
    } else if secs >= 3600 && secs.is_multiple_of(3600) {
        format!("{} h", secs / 3600)
    } else {
        format!("{secs} s")
    }
}

/// Now, in microseconds since the Unix epoch (the engine's commit times).
pub fn now_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_micros()).unwrap_or(i64::MAX))
}

/// The commit `Store::prune` may prune up to so that every point of the last `keep` stays
/// reachable: the newest snapshot (commit, time of its last commit in µs) at or before
/// `now - keep`. `None` when no snapshot is that old.
pub fn prune_bound(snapshots: &[(u64, i64)], now_us: i64, keep: Duration) -> Option<u64> {
    let keep_us = i64::try_from(keep.as_micros()).unwrap_or(i64::MAX);
    let cutoff = now_us.saturating_sub(keep_us);
    snapshots
        .iter()
        .filter(|(_, at)| *at <= cutoff)
        .map(|(seq, _)| *seq)
        .max()
}

/// The copy lock file of the store in `store_dir` (beside it: `<store>.copy.lock`).
pub fn copy_lock_path(store_dir: &Path) -> PathBuf {
    let mut name = store_dir
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".copy.lock");
    store_dir.with_file_name(name)
}

fn open_copy_lock(store_dir: &Path) -> Result<fd_lock::RwLock<File>, String> {
    let path = copy_lock_path(store_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("cannot create {}: {err}", parent.display()))?;
    }
    let file = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|err| format!("cannot open the copy lock {}: {err}", path.display()))?;
    Ok(fd_lock::RwLock::new(file))
}

/// Runs `copy` (a `Store::fork_dir` of the store in `store_dir`) while holding the store's copy
/// lock shared: a prune waits for it (it skips and tries again later), and a copy started
/// during a prune waits for the prune to finish.
pub fn hold_for_copy<T>(
    store_dir: &Path,
    copy: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let lock = open_copy_lock(store_dir)?;
    let _shared = lock.read().map_err(|err| {
        format!(
            "cannot lock {} for the copy: {err}",
            copy_lock_path(store_dir).display()
        )
    })?;
    copy()
}

/// What a prune did, for the log and the state file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pruned {
    /// The commit the history was pruned up to.
    pub up_to: u64,
    /// The oldest snapshot kept.
    pub kept_from: u64,
    /// Snapshots removed.
    pub snapshots: usize,
    /// WAL segments removed.
    pub segments: usize,
    /// Blob files removed and their bytes.
    pub blobs: (u64, u64),
    /// Marks that are no longer reachable (the product sets none).
    pub marks_unreachable: Vec<String>,
}

impl Pruned {
    /// One line for the daemon log.
    pub fn text(&self) -> String {
        let mut text = format!(
            "pruned up to commit {} (kept from commit {}): {} snapshot(s), {} WAL segment(s), {} \
             blob(s) ({}) removed",
            self.up_to,
            self.kept_from,
            self.snapshots,
            self.segments,
            self.blobs.0,
            crate::download::human_bytes(self.blobs.1)
        );
        if !self.marks_unreachable.is_empty() {
            text.push_str(&format!(
                "; marks no longer reachable: {}",
                self.marks_unreachable.join(", ")
            ));
        }
        text
    }
}

/// Prunes the history of `store` (in `store_dir`) older than `keep` at `now_us`: up to the
/// newest snapshot at or before `now - keep` ([`prune_bound`]), only when that removes a
/// snapshot or a WAL segment (the engine's estimate, which reads names only), and only while
/// no copy holds the copy lock (then it fails with a message saying so; the next check tries
/// again). `Ok(None)`: nothing to prune.
pub fn prune_old_history(
    store: &Store,
    store_dir: &Path,
    keep: Duration,
    now_us: i64,
) -> Result<Option<Pruned>, String> {
    let snapshots: Vec<(u64, i64)> = store
        .snapshots()
        .map_err(|err| format!("cannot list the snapshots: {err}"))?
        .iter()
        .map(|snapshot| (snapshot.commit_seq, snapshot.last_commit_at))
        .collect();
    let Some(up_to) = prune_bound(&snapshots, now_us, keep) else {
        return Ok(None);
    };
    let estimate = Store::prune_estimate_dir(store_dir, up_to)
        .map_err(|err| format!("cannot estimate the prune: {err}"))?;
    if estimate.snapshots_removed.is_empty() && estimate.segments_removed.is_empty() {
        return Ok(None);
    }
    let mut lock = open_copy_lock(store_dir)?;
    let Ok(_exclusive) = lock.try_write() else {
        return Err(
            "a copy of the store is being made (bench): the prune waits for it and runs \
                    at the next check"
                .to_string(),
        );
    };
    let report = store
        .prune(up_to)
        .map_err(|err| format!("the prune failed: {err}"))?;
    Ok(Some(Pruned {
        up_to,
        kept_from: report.kept_from,
        snapshots: report.snapshots_removed.len(),
        segments: report.segments_removed.len(),
        blobs: (report.blobs_removed, report.blob_bytes_removed),
        marks_unreachable: report.marks_unreachable,
    }))
}

/// The maintenance state file of the store in `store_dir`.
pub fn state_path(store_dir: &Path) -> PathBuf {
    let mut name = store_dir
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".maintenance.json");
    store_dir.with_file_name(name)
}

/// The recorded maintenance of a store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// The last history check (seconds since the epoch).
    pub last_check: Option<u64>,
    /// The newest snapshot at the last check.
    pub newest_snapshot: Option<u64>,
    /// The last prune that removed something (seconds since the epoch) and what it did.
    pub last_prune: Option<(u64, String)>,
}

impl State {
    /// The state of the store in `store_dir` (none recorded: the default).
    pub fn load(store_dir: &Path) -> Self {
        let Ok(Some(bytes)) = crate::fsutil::read_optional(&state_path(store_dir)) else {
            return Self::default();
        };
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Self {
            last_check: value["last_check"].as_u64(),
            newest_snapshot: value["newest_snapshot"].as_u64(),
            last_prune: value["last_prune"].as_u64().map(|at| {
                (
                    at,
                    value["last_prune_text"].as_str().unwrap_or("").to_string(),
                )
            }),
        }
    }

    /// Writes the state of the store in `store_dir`.
    pub fn save(&self, store_dir: &Path) -> Result<(), String> {
        let value = json!({
            "version": 1,
            "last_check": self.last_check,
            "newest_snapshot": self.newest_snapshot,
            "last_prune": self.last_prune.as_ref().map(|(at, _)| at),
            "last_prune_text": self.last_prune.as_ref().map(|(_, text)| text),
        });
        crate::fsutil::write_atomic(&state_path(store_dir), format!("{value}\n").as_bytes())
    }

    /// Whether a history check is due at `now` (seconds) with `newest` the newest snapshot: a
    /// day after the last one, or at once after a new snapshot (a checkpoint).
    pub fn due(&self, now: u64, newest: Option<u64>) -> bool {
        match self.last_check {
            None => true,
            Some(last) => {
                newest != self.newest_snapshot || now.saturating_sub(last) >= CHECK_EVERY.as_secs()
            }
        }
    }
}

/// The maintenance numbers of an open store (the daemon's `status` answer, per store): the
/// engine's quick listing (names, lengths and small metadata files; never the WAL).
pub fn open_store_status(store: &Store, store_dir: &Path, settings: &Settings) -> Value {
    let mut value = base_status(store_dir, settings, store.quick_info());
    value["wal_since_snapshot"] = json!(store.wal_bytes_since_checkpoint());
    if let Ok(Some(err)) = store.last_auto_checkpoint_error() {
        value["checkpoint_error"] = json!(err.to_string());
    }
    value
}

/// The maintenance numbers read from the files of the store in `store_dir` (without its lock;
/// no WAL-since-snapshot figure, which only the open store knows).
pub fn files_status(store_dir: &Path, settings: &Settings) -> Value {
    base_status(store_dir, settings, Store::inspect(store_dir))
}

fn base_status(
    store_dir: &Path,
    settings: &Settings,
    info: Result<graphersal::persist::StoreInfo, graphersal::persist::PersistError>,
) -> Value {
    let mut value = json!({
        "checkpoint_after": settings.checkpoint_after,
        "keep_history_secs": settings.keep_history.map(|keep| keep.as_secs()),
        "size": dir_bytes(store_dir),
    });
    match info {
        Ok(info) => {
            value["wal_bytes"] = json!(info.wal_bytes);
            value["snapshot_bytes"] = json!(info.snapshot_bytes);
            value["blob_bytes"] = json!(info.blobs.bytes);
            value["snapshots"] = json!(info.snapshots.len());
            if let Some(newest) = info.snapshots.last() {
                value["last_snapshot_commit"] = json!(newest.commit_seq);
                value["last_snapshot_at"] = json!(newest.created_at / 1_000_000);
            }
        }
        Err(err) => value["error"] = json!(err.to_string()),
    }
    let state = State::load(store_dir);
    if let Some((at, text)) = state.last_prune {
        value["last_prune_at"] = json!(at);
        value["last_prune"] = json!(text);
    }
    if let Some(at) = state.last_check {
        value["last_check_at"] = json!(at);
    }
    value
}

/// The bytes of every file under `dir` (0 when it cannot be read).
pub fn dir_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => dir_bytes(&entry.path()),
            Ok(_) => entry.metadata().map_or(0, |meta| meta.len()),
            Err(_) => 0,
        })
        .sum()
}

/// The `status` lines of one store's maintenance numbers (`status` from [`open_store_status`]
/// or [`files_status`]).
pub fn status_lines(name: &str, status: &Value) -> Vec<String> {
    let bytes = |key: &str| crate::download::human_bytes(status[key].as_u64().unwrap_or(0));
    let mut lines = vec![format!(
        "store {name}: {} on disk (snapshots {}, WAL {}, models and other blobs {}); {} \
         snapshot(s)",
        bytes("size"),
        bytes("snapshot_bytes"),
        bytes("wal_bytes"),
        bytes("blob_bytes"),
        status["snapshots"].as_u64().unwrap_or(0)
    )];
    let threshold = status["checkpoint_after"].as_u64().unwrap_or(0);
    let threshold_text = if threshold == 0 {
        "automatic checkpoints off".to_string()
    } else {
        format!(
            "checkpoint after {}",
            crate::download::human_bytes(threshold)
        )
    };
    lines.push(match status["wal_since_snapshot"].as_u64() {
        Some(since) => format!(
            "  WAL since the last snapshot: {} ({threshold_text})",
            crate::download::human_bytes(since)
        ),
        None => {
            format!("  WAL since the last snapshot: known while the daemon runs ({threshold_text})")
        }
    });
    let when = |key: &str| {
        status[key]
            .as_u64()
            .map(crate::daemon::utc_timestamp)
            .unwrap_or_else(|| "never".to_string())
    };
    lines.push(format!(
        "  last snapshot: {} (commit {}); last prune: {}{}",
        when("last_snapshot_at"),
        status["last_snapshot_commit"].as_u64().unwrap_or(0),
        when("last_prune_at"),
        status["last_prune"]
            .as_str()
            .map(|text| format!(" ({text})"))
            .unwrap_or_default()
    ));
    lines.push(format!(
        "  history kept: {}",
        match status["keep_history_secs"].as_u64() {
            Some(secs) => duration_text(Duration::from_secs(secs)),
            None => "forever (never pruned)".to_string(),
        }
    ));
    if let Some(err) = status["checkpoint_error"].as_str() {
        lines.push(format!("  the last automatic checkpoint failed: {err}"));
    }
    if let Some(err) = status["error"].as_str() {
        lines.push(format!("  cannot read the store: {err}"));
    }
    lines
}

/// `doctor`'s finding about a store's WAL: a problem when the WAL since the last snapshot is
/// more than [`WARN_FACTOR`] times the threshold (checkpoints are not happening) or the last
/// automatic checkpoint failed; `None` when there is nothing to say.
pub fn wal_finding(name: &str, status: &Value) -> Option<crate::install::Finding> {
    if let Some(err) = status["checkpoint_error"].as_str() {
        return Some(crate::install::problem(
            format!("store {name}: the last automatic checkpoint failed: {err}"),
            "see the daemon log; `graphersal-memory doctor` again after the next session",
        ));
    }
    let threshold = status["checkpoint_after"].as_u64().unwrap_or(0);
    let since = status["wal_since_snapshot"].as_u64()?;
    if threshold > 0 && since > threshold.saturating_mul(WARN_FACTOR) {
        return Some(crate::install::problem(
            format!(
                "store {name}: {} of WAL since the last snapshot, more than {WARN_FACTOR} times \
                 store.checkpoint_after ({}): every daemon start replays it",
                crate::download::human_bytes(since),
                crate::download::human_bytes(threshold)
            ),
            "see the daemon log for checkpoint errors; check free disk space",
        ));
    }
    Some(crate::install::ok(format!(
        "store {name}: {} of WAL since the last snapshot (checkpoint after {})",
        crate::download::human_bytes(since),
        if threshold == 0 {
            "off".to_string()
        } else {
            crate::download::human_bytes(threshold)
        }
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_US: i64 = 86_400_000_000;

    #[test]
    fn the_bound_is_the_newest_snapshot_old_enough() {
        let keep = Duration::from_secs(30 * 86_400);
        let now = 100 * DAY_US;
        let snapshots = [
            (0, 0),
            (500, 50 * DAY_US),
            (900, 69 * DAY_US),
            (1200, 80 * DAY_US),
        ];
        // Cutoff at day 70: the snapshot at day 69 is the newest at or before it.
        assert_eq!(prune_bound(&snapshots, now, keep), Some(900));
        assert_eq!(prune_bound(&snapshots, 70 * DAY_US, keep), Some(0));
        assert_eq!(prune_bound(&snapshots[1..], 70 * DAY_US, keep), None);
        assert_eq!(prune_bound(&[], now, keep), None);
        assert_eq!(prune_bound(&snapshots, now, Duration::MAX), None);
    }

    #[test]
    fn a_check_is_due_daily_and_after_a_new_snapshot() {
        let state = State::default();
        assert!(state.due(1000, Some(5)));
        let state = State {
            last_check: Some(1000),
            newest_snapshot: Some(5),
            last_prune: None,
        };
        assert!(!state.due(1000 + 3600, Some(5)));
        assert!(state.due(1000 + 3600, Some(6)));
        assert!(state.due(1000 + CHECK_EVERY.as_secs(), Some(5)));
    }

    #[test]
    fn settings_come_from_the_config_with_defaults() {
        let defaults = Settings::from_config(&Config::default());
        assert_eq!(defaults, Settings::default());
        assert_eq!(defaults.checkpoint_after, 16 << 20);
        assert_eq!(duration_text(defaults.keep_history.unwrap()), "30 days");
        let config = Config::parse(
            br#"{"version": 1, "store": {"checkpoint_after": "1MiB", "keep_history": "forever"}}"#,
        )
        .unwrap();
        let settings = Settings::from_config(&config);
        assert_eq!(settings.checkpoint_after, 1 << 20);
        assert_eq!(settings.keep_history, None);
    }

    #[test]
    fn the_state_file_round_trips_next_to_the_store() {
        let data = crate::mcp_tests::TempDir::new("mstate");
        let dir = crate::store::default_store_dir(data.path());
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(State::load(&dir), State::default());
        let state = State {
            last_check: Some(7),
            newest_snapshot: Some(3),
            last_prune: Some((6, "pruned".into())),
        };
        state.save(&dir).unwrap();
        assert_eq!(State::load(&dir), state);
        assert_eq!(
            state_path(&dir),
            data.path().join("stores").join("default.maintenance.json")
        );
        assert_eq!(
            copy_lock_path(&dir),
            data.path().join("stores").join("default.copy.lock")
        );
    }
}
