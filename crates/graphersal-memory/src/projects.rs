//! The projects the memory has seen, `<data dir>/projects.json`:
//!
//! ```json
//! {"version": 1, "projects": {"/work/app": {"git_remote": "https://github.com/me/app",
//!   "first_seen": "2026-11-10T09:00:00Z", "remote_seen": "2026-11-10T09:00:00Z"}}}
//! ```
//!
//! The project key stays the canonical root path (memories carry it in their `project`
//! property). Moving a repository changes that key, which would orphan its memories; this file
//! records each project's git remote URL (the `origin` remote, else the first one; credentials
//! removed), so a later stage can find the moved repository by its remote and re-map its
//! memories. Nothing re-maps yet. The daemon writes the file when a project connects for the
//! first time in its life (and again when the remote changed); `mcp --no-daemon` does the same.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

/// The file's name in the data directory.
pub const PROJECTS_FILE: &str = "projects.json";

/// The format version.
const FORMAT: u64 = 1;

/// The git remote URL of the repository at `root`: `origin`, else the first remote of its
/// configuration; user names and passwords in the URL are removed. `None` without a repository
/// or a remote.
pub fn git_remote(root: &Path) -> Option<String> {
    let config = git_config_path(root)?;
    let text = std::fs::read_to_string(config).ok()?;
    let mut section: Option<String> = None;
    let mut remotes: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line
                .strip_prefix("[remote \"")
                .and_then(|rest| rest.strip_suffix("\"]"))
                .map(str::to_string);
            continue;
        }
        let Some(name) = &section else { continue };
        if let Some((key, value)) = line.split_once('=')
            && key.trim() == "url"
        {
            remotes.push((name.clone(), value.trim().trim_matches('"').to_string()));
        }
    }
    let url = remotes
        .iter()
        .find(|(name, _)| name == "origin")
        .or_else(|| remotes.first())
        .map(|(_, url)| url.clone())?;
    Some(without_credentials(&url))
}

/// The `config` file of the repository at `root` (`.git/config`; a worktree's `.git` file
/// points to its git directory, whose `commondir` holds the shared configuration).
fn git_config_path(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let target = text.trim().strip_prefix("gitdir:")?.trim();
        let target = Path::new(target);
        if target.is_absolute() {
            target.to_path_buf()
        } else {
            root.join(target)
        }
    };
    let common = std::fs::read_to_string(git_dir.join("commondir"))
        .ok()
        .map(|text| {
            let common = Path::new(text.trim());
            if common.is_absolute() {
                common.to_path_buf()
            } else {
                git_dir.join(common)
            }
        })
        .unwrap_or(git_dir);
    Some(common.join("config"))
}

/// `url` without `user:password@` (an HTTPS remote may carry a token).
fn without_credentials(url: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        if let Some((_, host)) = authority.rsplit_once('@') {
            return format!("{scheme}://{host}{path}");
        }
    }
    url.to_string()
}

/// Records `project` (its git remote) in the projects file of `data_dir`. Writes only when the
/// project is new or its remote changed; returns whether it wrote.
pub fn record(data_dir: &Path, project: &str) -> Result<bool, String> {
    if project.is_empty() {
        return Ok(false);
    }
    let file = data_dir.join(PROJECTS_FILE);
    let mut root = match std::fs::read(&file) {
        Ok(bytes) => {
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|err| format!("{} is invalid: {err}", file.display()))?;
            if value.get("version").and_then(Value::as_u64) != Some(FORMAT) {
                return Err(format!(
                    "{}: version {} is not supported (expected {FORMAT})",
                    file.display(),
                    value.get("version").unwrap_or(&Value::Null)
                ));
            }
            value
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            json!({"version": FORMAT, "projects": {}})
        }
        Err(err) => return Err(format!("cannot read {}: {err}", file.display())),
    };
    let remote = git_remote(Path::new(project));
    let now = crate::daemon::now_utc();
    let Some(projects) = root.get_mut("projects").and_then(Value::as_object_mut) else {
        return Err(format!("{}: 'projects' must be an object", file.display()));
    };
    match projects.get_mut(project).and_then(Value::as_object_mut) {
        Some(entry) => {
            let known = entry.get("git_remote").and_then(Value::as_str);
            if known == remote.as_deref() {
                return Ok(false);
            }
            match &remote {
                Some(url) => {
                    entry.insert("git_remote".into(), Value::from(url.as_str()));
                }
                None => {
                    entry.remove("git_remote");
                }
            }
            entry.insert("remote_seen".into(), Value::from(now));
        }
        None => {
            let mut entry = Map::new();
            if let Some(url) = &remote {
                entry.insert("git_remote".into(), Value::from(url.as_str()));
                entry.insert("remote_seen".into(), Value::from(now.as_str()));
            }
            entry.insert("first_seen".into(), Value::from(now));
            projects.insert(project.to_string(), Value::Object(entry));
        }
    }
    std::fs::create_dir_all(data_dir)
        .map_err(|err| format!("cannot create {}: {err}", data_dir.display()))?;
    crate::fsutil::write_atomic(
        &file,
        format!("{}\n", crate::jsonedit::render(&root, Some("  "), 0)).as_bytes(),
    )?;
    Ok(true)
}

/// The recorded remote of `project`, if any.
#[cfg(test)]
pub fn recorded_remote(data_dir: &Path, project: &str) -> Option<String> {
    let bytes = std::fs::read(data_dir.join(PROJECTS_FILE)).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value["projects"][project]["git_remote"]
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(dir: &Path, config: &str) {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git").join("config"), config).unwrap();
    }

    #[test]
    fn origin_wins_and_credentials_are_removed() {
        let tmp = crate::mcp_tests::TempDir::new("remote");
        repo(
            tmp.path(),
            "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = git@github.com:up/app.git\n\
             [remote \"origin\"]\n\turl = https://me:tok3n@github.com/me/app.git\n\tfetch = x\n",
        );
        assert_eq!(
            git_remote(tmp.path()).as_deref(),
            Some("https://github.com/me/app.git")
        );
        let other = crate::mcp_tests::TempDir::new("remote2");
        repo(
            other.path(),
            "[remote \"upstream\"]\n  url = git@github.com:up/app.git\n",
        );
        assert_eq!(
            git_remote(other.path()).as_deref(),
            Some("git@github.com:up/app.git")
        );
        let none = crate::mcp_tests::TempDir::new("remote3");
        assert_eq!(git_remote(none.path()), None);
    }

    #[test]
    fn a_worktree_finds_the_shared_configuration() {
        let tmp = crate::mcp_tests::TempDir::new("wt");
        let main = tmp.path().join("main");
        repo(
            &main,
            "[remote \"origin\"]\n\turl = https://example.org/r.git\n",
        );
        let git_dir = main.join(".git").join("worktrees").join("w");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(git_dir.join("commondir"), "../..\n").unwrap();
        let worktree = tmp.path().join("w");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .unwrap();
        assert_eq!(
            git_remote(&worktree).as_deref(),
            Some("https://example.org/r.git")
        );
    }

    #[test]
    fn record_writes_once_and_again_when_the_remote_changes() {
        let data = crate::mcp_tests::TempDir::new("proj");
        let project = crate::mcp_tests::TempDir::new("projrepo");
        repo(
            project.path(),
            "[remote \"origin\"]\n\turl = https://a.example/x\n",
        );
        let key = project.path().display().to_string();
        assert!(record(data.path(), &key).unwrap());
        assert!(!record(data.path(), &key).unwrap());
        assert_eq!(
            recorded_remote(data.path(), &key).as_deref(),
            Some("https://a.example/x")
        );
        repo(
            project.path(),
            "[remote \"origin\"]\n\turl = https://b.example/x\n",
        );
        assert!(record(data.path(), &key).unwrap());
        assert_eq!(
            recorded_remote(data.path(), &key).as_deref(),
            Some("https://b.example/x")
        );
        assert!(!record(data.path(), "").unwrap());
    }
}
