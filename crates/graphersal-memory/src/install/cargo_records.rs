//! cargo's records of what `cargo install` installed, kept true when `upgrade` puts a release
//! build into cargo's directory (`$CARGO_HOME/bin`) instead of building it with cargo.
//!
//! cargo keeps two files in `$CARGO_HOME`: `.crates.toml` (`[v1]`: `"<name> <version>
//! (<source>)" = [<binaries>]`) and `.crates2.json` (`{"installs": {"<name> <version>
//! (<source>)": {"version_req", "bins", "features", "all_features", "no_default_features",
//! "profile", "target", "rustc", ...}}}`). `cargo install --list`, `cargo uninstall` and tools
//! such as cargo-update read them; cargo-binstall writes them the same way after it downloads a
//! build. [`update`] renames our entry to the new version from crates.io, in both files, under
//! the lock cargo itself takes (an exclusive lock on each file), and rewrites each file in place
//! as cargo does. Every other entry and field stays; `version_req` pinned to the old version
//! (`=<old>`, what `cargo install --version` records) is pinned to the new one, `target` names
//! the release build's target and `rustc` (the compiler of a cargo build) is dropped. A file
//! that does not parse is left as it is (an error), and nothing is written when neither file
//! records our binary.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

/// The source cargo records for a crate from crates.io.
pub const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// cargo's records, in `$CARGO_HOME`.
pub const V1_FILE: &str = ".crates.toml";
/// cargo's records with the install's details.
pub const V2_FILE: &str = ".crates2.json";

/// How long [`update`] waits for a running cargo to release its lock.
const LOCK_WAIT: Duration = Duration::from_secs(30);

/// What [`update`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The files whose entry now names the new version.
    Updated(Vec<PathBuf>),
    /// Neither file records the binary (it was put there by hand): nothing written.
    NotRecorded,
}

/// The key of `name` at `version` from crates.io.
pub fn key(name: &str, version: &str) -> String {
    format!("{name} {version} ({CRATES_IO})")
}

/// The name and version of a record key (`<name> <version> (<source>)`).
fn split_key(key: &str) -> Option<(&str, &str)> {
    let mut words = key.split(' ');
    Some((words.next()?, words.next()?))
}

/// Our entry's key in a records map: the one whose name is `name`.
fn find<'a>(keys: impl Iterator<Item = &'a String>, name: &str) -> Option<String> {
    keys.into_iter()
        .find(|key| split_key(key).is_some_and(|(n, _)| n == name))
        .cloned()
}

/// One of cargo's files, open and locked.
struct Locked {
    path: PathBuf,
    lock: fd_lock::RwLock<File>,
}

impl Locked {
    fn open(path: PathBuf) -> Result<Option<Self>, String> {
        match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(file) => Ok(Some(Self {
                path,
                lock: fd_lock::RwLock::new(file),
            })),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(format!("cannot open {}: {err}", path.display())),
        }
    }

    /// Reads the file under its lock, lets `edit` change the text, and writes the result in
    /// place (as cargo does); `edit` answers `None` to leave the file alone.
    fn edit(
        &mut self,
        edit: &dyn Fn(&str) -> Result<Option<String>, String>,
    ) -> Result<bool, String> {
        let path = self.path.display().to_string();
        let started = Instant::now();
        let mut guard = loop {
            match self.lock.try_write() {
                Ok(guard) => break guard,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if started.elapsed() > LOCK_WAIT {
                        return Err(format!(
                            "{path} stayed locked (is a `cargo install` running?)"
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(err) => return Err(format!("cannot lock {path}: {err}")),
            }
        };
        let mut text = String::new();
        guard
            .read_to_string(&mut text)
            .map_err(|err| format!("cannot read {path}: {err}"))?;
        let Some(new) = edit(&text).map_err(|err| format!("{path}: {err}"))? else {
            return Ok(false);
        };
        guard
            .seek(SeekFrom::Start(0))
            .and_then(|_| guard.set_len(0))
            .and_then(|()| guard.write_all(new.as_bytes()))
            .and_then(|()| guard.flush())
            .map_err(|err| format!("cannot write {path}: {err}"))?;
        Ok(true)
    }
}

/// `.crates.toml` with our entry renamed to `new_key` (`None`: no entry). The file is parsed
/// to check it; the edit replaces our entry's line and keeps every other byte.
fn edit_v1(text: &str, name: &str, new_key: &str) -> Result<Option<String>, String> {
    text.parse::<toml::Table>()
        .map_err(|err| format!("not TOML: {err}"))?;
    let mut table = String::new();
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            table = trimmed.to_string();
        }
        let entry = (table == "[v1]" && !changed)
            .then(|| trimmed.parse::<toml::Table>().ok())
            .flatten()
            .and_then(|one| one.into_iter().next())
            .filter(|(key, _)| split_key(key).is_some_and(|(n, _)| n == name));
        match entry {
            Some((key, toml::Value::Array(bins))) if key != new_key => {
                let bins: Vec<String> = bins
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .map(|bin| format!("\"{}\"", bin.replace('\\', "\\\\").replace('"', "\\\"")))
                    .collect();
                let end = if line.ends_with("\r\n") {
                    "\r\n"
                } else if line.ends_with('\n') {
                    "\n"
                } else {
                    ""
                };
                out.push_str(&format!("\"{new_key}\" = [{}]{end}", bins.join(", ")));
                changed = true;
            }
            _ => out.push_str(line),
        }
    }
    Ok(changed.then_some(out))
}

/// `.crates2.json` with our entry renamed to `new_key` and its details set for a release build
/// of `target` (`None`: no entry).
fn edit_v2(text: &str, name: &str, new_key: &str, target: &str) -> Result<Option<String>, String> {
    let mut doc: Value = serde_json::from_str(text).map_err(|err| format!("not JSON: {err}"))?;
    let before = doc.clone();
    let Some(installs) = doc.get_mut("installs").and_then(Value::as_object_mut) else {
        return Ok(None);
    };
    let Some(old) = find(installs.keys(), name) else {
        return Ok(None);
    };
    let Some(mut info) = installs.remove(&old) else {
        return Ok(None);
    };
    let old_version = split_key(&old).map_or("", |(_, version)| version);
    let new_version = split_key(new_key).map_or("", |(_, version)| version);
    if let Some(fields) = info.as_object_mut() {
        if fields.get("version_req").and_then(Value::as_str) == Some(&format!("={old_version}")) {
            fields.insert("version_req".into(), format!("={new_version}").into());
        }
        fields.insert("target".into(), target.into());
        fields.insert("rustc".into(), Value::Null);
    }
    installs.insert(new_key.to_string(), info);
    if doc == before {
        return Ok(None);
    }
    serde_json::to_string(&doc)
        .map(Some)
        .map_err(|err| format!("cannot write JSON: {err}"))
}

/// Our entry in cargo's records (in `cargo_home`) renamed to `name` `version` from crates.io,
/// for a release build of `target`. Both files are parsed before either is written.
pub fn update(
    cargo_home: &Path,
    name: &str,
    version: &str,
    target: &str,
) -> Result<Outcome, String> {
    let new_key = key(name, version);
    let mut v1 = Locked::open(cargo_home.join(V1_FILE))?;
    let mut v2 = Locked::open(cargo_home.join(V2_FILE))?;
    // Check both first, so a file that does not parse leaves the other one as it is too.
    for file in [&v1, &v2].into_iter().flatten() {
        let text = std::fs::read_to_string(&file.path)
            .map_err(|err| format!("cannot read {}: {err}", file.path.display()))?;
        let parsed = if file.path.ends_with(V1_FILE) {
            edit_v1(&text, name, &new_key).map(|_| ())
        } else {
            edit_v2(&text, name, &new_key, target).map(|_| ())
        };
        parsed.map_err(|err| format!("{}: {err}", file.path.display()))?;
    }
    let mut updated = Vec::new();
    if let Some(file) = v1.as_mut()
        && file.edit(&|text| edit_v1(text, name, &new_key))?
    {
        updated.push(file.path.clone());
    }
    if let Some(file) = v2.as_mut()
        && file.edit(&|text| edit_v2(text, name, &new_key, target))?
    {
        updated.push(file.path.clone());
    }
    if updated.is_empty() {
        let recorded = [&v1, &v2].into_iter().flatten().any(|file| {
            std::fs::read_to_string(&file.path)
                .is_ok_and(|text| text.contains(&format!("\"{new_key}\"")))
        });
        if !recorded {
            return Ok(Outcome::NotRecorded);
        }
    }
    Ok(Outcome::Updated(updated))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUSTC: &str = "rustc 1.98.1 (48a229cea 2026-09-01)\nhost: aarch64-apple-darwin\n";

    fn v1_text() -> String {
        format!(
            "[v1]\n\"cargo-nextest 0.9.100 ({CRATES_IO})\" = [\"cargo-nextest\"]\n\
             \"graphersal-memory 0.2.1 ({CRATES_IO})\" = [\"graphersal-memory\"]\n\
             \"ripgrep 14.1.1 ({CRATES_IO})\" = [\"rg\"]\n"
        )
    }

    fn v2_text(version_req: Value) -> String {
        let info = |bins: &[&str], req: Value| {
            serde_json::json!({"version_req": req, "bins": bins, "features": [],
                "all_features": false, "no_default_features": false, "profile": "release",
                "target": "aarch64-apple-darwin", "rustc": RUSTC})
        };
        serde_json::json!({"installs": {
            format!("graphersal-memory 0.2.1 ({CRATES_IO})"): info(&["graphersal-memory"], version_req),
            format!("ripgrep 14.1.1 ({CRATES_IO})"): info(&["rg"], Value::Null),
        }})
        .to_string()
    }

    fn home_with(v1: Option<&str>, v2: Option<&str>) -> crate::runtime::mcp_tests::TempDir {
        let home = crate::runtime::mcp_tests::TempDir::new("cargo-records");
        if let Some(text) = v1 {
            std::fs::write(home.path().join(V1_FILE), text).unwrap();
        }
        if let Some(text) = v2 {
            std::fs::write(home.path().join(V2_FILE), text).unwrap();
        }
        home
    }

    #[test]
    fn our_entry_moves_to_the_new_version_and_every_other_entry_stays() {
        let home = home_with(Some(&v1_text()), Some(&v2_text("=0.2.1".into())));
        let outcome = update(
            home.path(),
            "graphersal-memory",
            "0.2.2",
            "aarch64-apple-darwin",
        );
        assert_eq!(
            outcome,
            Ok(Outcome::Updated(vec![
                home.path().join(V1_FILE),
                home.path().join(V2_FILE)
            ]))
        );
        let v1: toml::Table = std::fs::read_to_string(home.path().join(V1_FILE))
            .unwrap()
            .parse()
            .unwrap();
        let v1 = v1["v1"].as_table().unwrap();
        assert_eq!(v1.len(), 3);
        assert_eq!(
            v1[&key("graphersal-memory", "0.2.2")],
            toml::Value::Array(vec!["graphersal-memory".into()])
        );
        assert!(!v1.contains_key(&key("graphersal-memory", "0.2.1")));
        assert!(v1.contains_key(&key("ripgrep", "14.1.1")));
        let v2: Value =
            serde_json::from_str(&std::fs::read_to_string(home.path().join(V2_FILE)).unwrap())
                .unwrap();
        let installs = v2["installs"].as_object().unwrap();
        assert_eq!(installs.len(), 2);
        let ours = &installs[&key("graphersal-memory", "0.2.2")];
        assert_eq!(ours["version_req"], "=0.2.2");
        assert_eq!(ours["bins"], serde_json::json!(["graphersal-memory"]));
        assert_eq!(ours["profile"], "release");
        assert_eq!(ours["target"], "aarch64-apple-darwin");
        assert_eq!(ours["rustc"], Value::Null);
        assert_eq!(installs[&key("ripgrep", "14.1.1")]["rustc"], RUSTC);
        // A second run changes nothing and still reports the records as ours.
        let before = std::fs::read(home.path().join(V2_FILE)).unwrap();
        assert_eq!(
            update(
                home.path(),
                "graphersal-memory",
                "0.2.2",
                "aarch64-apple-darwin"
            ),
            Ok(Outcome::Updated(vec![]))
        );
        assert_eq!(std::fs::read(home.path().join(V2_FILE)).unwrap(), before);
    }

    #[test]
    fn a_version_req_that_is_not_the_old_pin_stays_and_a_path_install_becomes_crates_io() {
        let v2 = v2_text(Value::Null).replace(
            &format!("graphersal-memory 0.2.1 ({CRATES_IO})"),
            "graphersal-memory 0.2.0 (path+file:///Users/you/graphersal-memory/crates/graphersal-memory)",
        );
        let home = home_with(None, Some(&v2));
        let outcome = update(
            home.path(),
            "graphersal-memory",
            "0.2.2",
            "x86_64-unknown-linux-musl",
        );
        assert_eq!(
            outcome,
            Ok(Outcome::Updated(vec![home.path().join(V2_FILE)]))
        );
        let v2: Value =
            serde_json::from_str(&std::fs::read_to_string(home.path().join(V2_FILE)).unwrap())
                .unwrap();
        let ours = &v2["installs"][&key("graphersal-memory", "0.2.2")];
        assert_eq!(ours["version_req"], Value::Null);
        assert_eq!(ours["target"], "x86_64-unknown-linux-musl");
        assert_eq!(v2["installs"].as_object().unwrap().len(), 2);
    }

    #[test]
    fn nothing_is_written_without_our_entry_or_when_a_file_does_not_parse() {
        // No files, or files without our entry: not recorded, nothing created or written.
        let empty = home_with(None, None);
        assert_eq!(
            update(empty.path(), "graphersal-memory", "0.2.2", "t"),
            Ok(Outcome::NotRecorded)
        );
        assert!(!empty.path().join(V1_FILE).exists());
        let others = format!("[v1]\n\"ripgrep 14.1.1 ({CRATES_IO})\" = [\"rg\"]\n");
        let home = home_with(Some(&others), Some("{\"installs\":{}}"));
        assert_eq!(
            update(home.path(), "graphersal-memory", "0.2.2", "t"),
            Ok(Outcome::NotRecorded)
        );
        assert_eq!(
            std::fs::read_to_string(home.path().join(V1_FILE)).unwrap(),
            others
        );
        // A broken .crates2.json: an error naming it, and .crates.toml left as it was too.
        let home = home_with(Some(&v1_text()), Some("{\"installs\": "));
        let err = update(home.path(), "graphersal-memory", "0.2.2", "t").unwrap_err();
        assert!(err.contains(V2_FILE), "{err}");
        assert_eq!(
            std::fs::read_to_string(home.path().join(V1_FILE)).unwrap(),
            v1_text()
        );
    }
}
