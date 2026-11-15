#![allow(clippy::wildcard_imports)]

//! Claude Code's file memory, as the import (`crate::memory::import`) reads it: its memory
//! directories (`<claude config>/projects/<encoded project>/memory/`, [`scan`]) and the project
//! each belongs to: the `cwd` recorded in the project's transcripts, else the directory name
//! decoded against the file system (Claude Code names it after the path with every other
//! character turned into `-`, [`claude_slug`]); a directory whose project no longer exists is
//! skipped and reported.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::memory::import::*;

/// How many directories the file-system decoding of a project name visits at most.
const DECODE_BUDGET: usize = 4000;

/// Claude Code's name for the project directory of `path`: every character that is not an
/// ASCII letter or digit becomes `-`.
pub fn claude_slug(path: &str) -> String {
    crate::fsutil::path_slug(path)
}

/// The memory directories under the harness's `projects` directory, sorted by name.
pub fn scan(projects_dir: &Path) -> Result<Vec<MemoryDir>, String> {
    let entries = match std::fs::read_dir(projects_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("cannot read {}: {err}", projects_dir.display())),
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("memory").is_dir())
        .collect();
    dirs.sort();
    Ok(dirs
        .iter()
        .map(|project_dir| {
            let project = resolve_project_dir(project_dir)
                .map(|dir| crate::runtime::mcp::project_key(None, &dir));
            read_dir(&project_dir.join("memory"), project)
        })
        .collect())
}

/// The directory a harness project directory stands for: the `cwd` of its transcripts whose
/// encoding is the directory's name, else the name decoded against the file system.
pub fn resolve_project_dir(project_dir: &Path) -> Option<PathBuf> {
    let name = project_dir.file_name()?.to_str()?.to_string();
    if let Some(dir) = cwd_from_transcripts(project_dir, &name) {
        return Some(dir);
    }
    decode(&name)
}

/// The first transcript `cwd` (of up to 20 transcripts, the first 64 KiB of each) whose
/// encoding equals `name` and that exists.
fn cwd_from_transcripts(project_dir: &Path, name: &str) -> Option<PathBuf> {
    use std::io::Read;
    let mut files: Vec<PathBuf> = std::fs::read_dir(project_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    files.sort();
    for file in files.iter().take(20) {
        let Ok(handle) = std::fs::File::open(file) else {
            continue;
        };
        let mut head = String::new();
        let _ = handle.take(64 * 1024).read_to_string(&mut head);
        for line in head.lines() {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if let Some(cwd) = value.get("cwd").and_then(Value::as_str)
                && claude_slug(cwd) == name
                && Path::new(cwd).is_dir()
            {
                return Some(PathBuf::from(cwd));
            }
        }
    }
    None
}

/// Decodes a project directory name against the file system: walks down from the root,
/// matching each directory entry's encoding against the name (every `-` may be a separator or
/// part of a name). `None` when no existing directory matches within the budget.
pub fn decode(name: &str) -> Option<PathBuf> {
    let (root, rest) = if let Some(rest) = name.strip_prefix('-') {
        (PathBuf::from("/"), rest)
    } else {
        // A Windows drive: `C:\x` is encoded `C--x`.
        let mut chars = name.chars();
        let drive = chars.next().filter(char::is_ascii_alphabetic)?;
        let rest = name.get(1..)?.strip_prefix("--")?;
        (PathBuf::from(format!("{drive}:\\")), rest)
    };
    let mut budget = DECODE_BUDGET;
    walk(&root, rest, &mut budget)
}

fn walk(dir: &Path, rest: &str, budget: &mut usize) -> Option<PathBuf> {
    if rest.is_empty() {
        return Some(dir.to_path_buf());
    }
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    let mut entries: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_type()
                .is_ok_and(|t| t.is_dir() || t.is_symlink())
        })
        .map(|entry| {
            (
                claude_slug(&entry.file_name().to_string_lossy()),
                entry.path(),
            )
        })
        .filter(|(encoded, _)| !encoded.is_empty() && rest.starts_with(encoded.as_str()))
        .collect();
    // Longer matches first: `my-app` before `my`.
    entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.1.cmp(&b.1)));
    for (encoded, path) in entries {
        let after = &rest[encoded.len()..];
        if after.is_empty() {
            if path.is_dir() {
                return Some(path);
            }
            continue;
        }
        if let Some(next) = after.strip_prefix('-')
            && let Some(found) = walk(&path, next, budget)
        {
            return Some(found);
        }
    }
    None
}
