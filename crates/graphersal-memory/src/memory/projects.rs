//! Project identity: the projects the memory has seen, `<data dir>/projects.json`:
//!
//! ```json
//! {"version": 1, "projects": {
//!   "/work/app": {"paths": ["/home/me/app", "/work/app"],
//!                 "git_remote": "https://github.com/me/app.git", "remotes": ["github.com/me/app"],
//!                 "root_commit": "3f2a...", "first_seen": "2026-11-10T09:00:00Z",
//!                 "remote_seen": "2026-11-10T09:00:00Z"}}}
//! ```
//!
//! Every project has a stable internal **id**, the key of its entry: memories, sessions, turns
//! and facts carry it in their `project` property, and memory ids end with `@<id>`. The id is
//! the project's root path when the memory first saw it, and it never changes afterwards (a
//! second project first seen at the same path gets `<path>#2`). Projects recorded before ids
//! existed keep their path as id, so their memories need no change.
//!
//! A session's directory (the git root of its working directory) is mapped to an id at
//! connection time ([`resolve`]) by up to three lookup keys, in this order:
//!
//! 1. **git remote** (`origin` first, then the other remotes; normalized: no scheme, no user,
//!    no `.git`, lower-case host, `git@host:path` = `https://host/path`): a project with that
//!    remote is the same project, wherever the clone is; its path is added. Two clones of one
//!    remote therefore share their memories (intended: they are the same project).
//! 2. **path**: a project seen at this path is the same project; a remote the repository has now
//!    and the project did not is recorded.
//! 3. **root commit** (`git rev-list --max-parents=0 HEAD`, the first one): NOT merged
//!    automatically (forks and repositories made from a template share their root commit). A
//!    new project whose root commit another project has is reported by `doctor` and `install`
//!    as "looks like project X, moved", with `graphersal-memory projects merge <from> <into>`.
//!
//! The daemon resolves every connection and every indexed session; `mcp --no-daemon` resolves
//! its connection. Writes are atomic and serialized in the process.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value, json};

/// The file's name in the data directory.
pub const PROJECTS_FILE: &str = "projects.json";

/// The format version (version 1 files without `paths`/`remotes` are read as before ids: the
/// key is the path).
const FORMAT: u64 = 1;

/// Serializes the read-modify-write of the file within the process (the daemon's connection
/// threads and its indexer).
static WRITING: Mutex<()> = Mutex::new(());

/// The git remotes of the repository at `root`, `origin` first, then the others in the order of
/// its configuration; user names and passwords removed. Empty without a repository or remotes.
pub fn git_remotes(root: &Path) -> Vec<String> {
    let Some(config) = git_config_path(root) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(config) else {
        return Vec::new();
    };
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
    let mut ordered: Vec<String> = remotes
        .iter()
        .filter(|(name, _)| name == "origin")
        .chain(remotes.iter().filter(|(name, _)| name != "origin"))
        .map(|(_, url)| without_credentials(url))
        .collect();
    ordered.dedup();
    ordered
}

/// The git remote URL of the repository at `root`: `origin`, else the first remote of its
/// configuration; user names and passwords in the URL are removed. `None` without a repository
/// or a remote.
#[cfg(test)]
pub fn git_remote(root: &Path) -> Option<String> {
    git_remotes(root).into_iter().next()
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

/// A remote URL as a lookup key: `host/path` with the host in lower case, without scheme,
/// user, port of an SSH URL, trailing `/` and `.git` (`git@github.com:me/App.git`,
/// `https://tok@github.com/me/App` and `ssh://git@github.com/me/App.git` are all
/// `github.com/me/App`). A local path stays a path.
pub fn normalize_remote(url: &str) -> String {
    let url = url.trim();
    let (host, path) = if let Some((scheme, rest)) = url.split_once("://") {
        if scheme == "file" {
            return rest.trim_end_matches('/').to_string();
        }
        let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let host = if scheme == "ssh" || scheme == "git" {
            host.split(':').next().unwrap_or(host)
        } else {
            host
        };
        (host.to_ascii_lowercase(), path.to_string())
    } else if let Some((authority, path)) = url.split_once(':')
        && !authority.contains('/')
        && authority.len() > 1
    {
        // scp-like `user@host:path`.
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        (host.to_ascii_lowercase(), format!("/{path}"))
    } else {
        return url.trim_end_matches('/').to_string();
    };
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    format!("{host}/{}", path.trim_start_matches('/'))
}

/// The first root commit of the repository at `root` (`git rev-list --max-parents=0 HEAD`),
/// `None` without git, a repository or a commit.
pub fn root_commit(root: &Path) -> Option<String> {
    if !root.join(".git").exists() {
        return None;
    }
    let output = crate::harnesses::headless::without_window(&mut std::process::Command::new("git"))
        .arg("-C")
        .arg(root)
        .args(["rev-list", "--max-parents=0", "HEAD"])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let first = text.lines().next()?.trim();
    (first.len() >= 40 && first.chars().all(|c| c.is_ascii_hexdigit())).then(|| first.to_string())
}

/// What a repository offers as lookup keys.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keys {
    /// The remote URLs (credentials removed), `origin` first.
    pub remotes: Vec<String>,
    /// The first root commit, when asked for and known.
    pub root_commit: Option<String>,
}

impl Keys {
    /// The keys of the repository at `root` (the root commit only when `with_root`: it runs
    /// git, so it is read for a new project only).
    pub fn of(root: &Path, with_root: bool) -> Self {
        Keys {
            remotes: git_remotes(root),
            root_commit: if with_root { root_commit(root) } else { None },
        }
    }
}

/// One project of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// Its id.
    pub id: String,
    /// The paths it was seen at, most recent first.
    pub paths: Vec<String>,
    /// Its normalized remotes.
    pub remotes: Vec<String>,
    /// Its root commit, when known.
    pub root_commit: Option<String>,
}

/// The projects file.
#[derive(Debug, Clone)]
pub struct Projects {
    value: Value,
}

impl Projects {
    /// The file of `data_dir` (empty when there is none).
    pub fn load(data_dir: &Path) -> Result<Self, String> {
        let file = data_dir.join(PROJECTS_FILE);
        let value = match std::fs::read(&file) {
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
                if !value["projects"].is_object() {
                    return Err(format!("{}: 'projects' must be an object", file.display()));
                }
                value
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                json!({"version": FORMAT, "projects": {}})
            }
            Err(err) => return Err(format!("cannot read {}: {err}", file.display())),
        };
        Ok(Self { value })
    }

    fn save(&self, data_dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(data_dir)
            .map_err(|err| format!("cannot create {}: {err}", data_dir.display()))?;
        crate::fsutil::write_atomic(
            &data_dir.join(PROJECTS_FILE),
            format!(
                "{}\n",
                crate::install::jsonedit::render(&self.value, Some("  "), 0)
            )
            .as_bytes(),
        )
    }

    fn entries(&self) -> impl Iterator<Item = (&String, &Map<String, Value>)> {
        self.value["projects"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(id, entry)| entry.as_object().map(|entry| (id, entry)))
    }

    fn entry_mut(&mut self, id: &str) -> Option<&mut Map<String, Value>> {
        self.value["projects"]
            .as_object_mut()?
            .get_mut(id)?
            .as_object_mut()
    }

    /// Every project, in id order.
    pub fn list(&self) -> Vec<Project> {
        self.entries()
            .map(|(id, entry)| project_of(id, entry))
            .collect()
    }

    /// The project `id`.
    pub fn get(&self, id: &str) -> Option<Project> {
        self.entries()
            .find(|(key, _)| *key == id)
            .map(|(id, entry)| project_of(id, entry))
    }

    /// The project an id or a path names (no write, no git call): the id itself, else the
    /// project seen at that path.
    pub fn find(&self, id_or_path: &str) -> Option<Project> {
        self.get(id_or_path).or_else(|| {
            self.list()
                .into_iter()
                .find(|project| project.paths.iter().any(|p| p == id_or_path))
        })
    }

    /// The projects that share their root commit with another one: `(older, newer)` pairs by
    /// first sight, where the newer one may be the older one moved.
    pub fn look_alikes(&self) -> Vec<(Project, Project)> {
        let mut seen: Vec<((u64, String), Project)> = self
            .entries()
            .map(|(id, entry)| {
                (
                    (
                        entry.get("seq").and_then(Value::as_u64).unwrap_or(0),
                        entry
                            .get("first_seen")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    project_of(id, entry),
                )
            })
            .collect();
        seen.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)));
        let mut pairs = Vec::new();
        for (i, (_, newer)) in seen.iter().enumerate() {
            let Some(root) = &newer.root_commit else {
                continue;
            };
            if let Some((_, older)) = seen[..i]
                .iter()
                .find(|(_, older)| older.root_commit.as_ref() == Some(root))
            {
                pairs.push((older.clone(), newer.clone()));
            }
        }
        pairs
    }
}

fn strings(entry: &Map<String, Value>, key: &str) -> Option<Vec<String>> {
    entry.get(key).and_then(Value::as_array).map(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    })
}

fn project_of(id: &str, entry: &Map<String, Value>) -> Project {
    // An entry from before ids: its key is its path, its remote the raw `git_remote`.
    let paths = strings(entry, "paths").unwrap_or_else(|| vec![id.to_string()]);
    let remotes = strings(entry, "remotes").unwrap_or_else(|| {
        entry
            .get("git_remote")
            .and_then(Value::as_str)
            .map(|url| vec![normalize_remote(url)])
            .unwrap_or_default()
    });
    Project {
        id: id.to_string(),
        paths,
        remotes,
        root_commit: entry
            .get("root_commit")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// A resolved connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The project id.
    pub id: String,
    /// Whether the project is new.
    pub created: bool,
    /// Older projects with the same root commit (a new project only): maybe this one moved.
    pub look_alikes: Vec<String>,
    /// Whether the file was written (a new project, path or remote).
    pub written: bool,
}

/// The project id of the directory `path` (a project key: the canonical git root), recording
/// what changed (a new project, a new path, a new remote). An empty path is no project (`""`).
pub fn resolve(data_dir: &Path, path: &str) -> Result<Resolved, String> {
    let dir = Path::new(path);
    resolve_with(data_dir, path, &|with_root| Keys::of(dir, with_root))
}

/// [`resolve`] with the repository's keys from `keys` (tests).
pub fn resolve_with(
    data_dir: &Path,
    path: &str,
    keys: &dyn Fn(bool) -> Keys,
) -> Result<Resolved, String> {
    if path.is_empty() {
        return Ok(Resolved {
            id: String::new(),
            created: false,
            look_alikes: Vec::new(),
            written: false,
        });
    }
    let _guard = WRITING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut projects = Projects::load(data_dir)?;
    let mut found = keys(false);
    found.remotes = found
        .remotes
        .iter()
        .map(|u| without_credentials(u))
        .collect();
    let normalized: Vec<String> = found.remotes.iter().map(|u| normalize_remote(u)).collect();
    let list = projects.list();
    let now = crate::runtime::daemon::now_utc();
    // 1. a remote, origin first.
    let by_remote = normalized.iter().find_map(|remote| {
        list.iter()
            .find(|project| project.remotes.contains(remote))
            .map(|project| project.id.clone())
    });
    // 2. the path.
    let id = by_remote.or_else(|| {
        list.iter()
            .find(|project| project.paths.iter().any(|p| p == path))
            .map(|project| project.id.clone())
    });
    if let Some(id) = id {
        let mut changed = false;
        if let Some(entry) = projects.entry_mut(&id) {
            let current = project_of(&id, entry);
            let mut paths = current.paths.clone();
            if !paths.iter().any(|p| p == path) {
                paths.insert(0, path.to_string());
            }
            let mut remotes = current.remotes.clone();
            for remote in &normalized {
                if !remotes.contains(remote) {
                    remotes.push(remote.clone());
                }
            }
            if strings(entry, "paths").as_ref() != Some(&paths) {
                entry.insert("paths".into(), json!(paths));
                changed = true;
            }
            if strings(entry, "remotes").as_ref() != Some(&remotes) {
                entry.insert("remotes".into(), json!(remotes));
                changed = true;
            }
            // A project recorded without its root commit (before ids) gets it once.
            if current.root_commit.is_none()
                && let Some(root) = keys(true).root_commit
            {
                entry.insert("root_commit".into(), json!(root));
                changed = true;
            }
            if let Some(first) = found.remotes.first()
                && entry.get("git_remote").and_then(Value::as_str) != Some(first.as_str())
            {
                entry.insert("git_remote".into(), json!(first));
                entry.insert("remote_seen".into(), json!(now));
                changed = true;
            }
        }
        if changed {
            projects.save(data_dir)?;
        }
        return Ok(Resolved {
            id,
            created: false,
            look_alikes: Vec::new(),
            written: changed,
        });
    }
    // 3. a new project; the root commit is a hint only.
    let root = keys(true).root_commit;
    let mut id = path.to_string();
    let mut n = 2;
    while projects.get(&id).is_some() {
        id = format!("{path}#{n}");
        n += 1;
    }
    let look_alikes: Vec<String> = match &root {
        Some(root) => list
            .iter()
            .filter(|project| project.root_commit.as_ref() == Some(root))
            .map(|project| project.id.clone())
            .collect(),
        None => Vec::new(),
    };
    let mut entry = Map::new();
    entry.insert("paths".into(), json!([path]));
    if let Some(first) = found.remotes.first() {
        entry.insert("git_remote".into(), json!(first));
        entry.insert("remote_seen".into(), json!(now));
    }
    entry.insert("remotes".into(), json!(normalized));
    if let Some(root) = &root {
        entry.insert("root_commit".into(), json!(root));
    }
    entry.insert("first_seen".into(), json!(now));
    // The order of first sight (look-alikes name the older project first).
    let seq = projects
        .entries()
        .filter_map(|(_, e)| e.get("seq").and_then(Value::as_u64))
        .max()
        .unwrap_or(0)
        + 1;
    entry.insert("seq".into(), json!(seq));
    if let Some(all) = projects.value["projects"].as_object_mut() {
        all.insert(id.clone(), Value::Object(entry));
    }
    projects.save(data_dir)?;
    Ok(Resolved {
        id,
        created: true,
        look_alikes,
        written: true,
    })
}

/// Records `project` (a project key: its path) in the projects file of `data_dir`: [`resolve`],
/// returning whether it wrote (a new project, a new path or a new remote).
#[cfg(test)]
pub fn record(data_dir: &Path, project: &str) -> Result<bool, String> {
    resolve(data_dir, project).map(|resolved| resolved.written)
}

/// Moves the lookup keys of project `from` to project `into` and removes `from` (after its
/// memories were moved: `crate::runtime::daemon`'s `merge_projects`).
pub fn merge_records(data_dir: &Path, from: &str, into: &str) -> Result<(), String> {
    let _guard = WRITING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut projects = Projects::load(data_dir)?;
    let source = projects
        .get(from)
        .ok_or_else(|| format!("no project {from}"))?;
    let target = projects
        .get(into)
        .ok_or_else(|| format!("no project {into}"))?;
    let mut paths = source.paths.clone();
    for path in target.paths {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    let mut remotes = target.remotes.clone();
    for remote in source.remotes {
        if !remotes.contains(&remote) {
            remotes.push(remote);
        }
    }
    let entry = projects
        .entry_mut(into)
        .ok_or_else(|| format!("no project {into}"))?;
    entry.insert("paths".into(), json!(paths));
    entry.insert("remotes".into(), json!(remotes));
    if target.root_commit.is_none()
        && let Some(root) = source.root_commit
    {
        entry.insert("root_commit".into(), json!(root));
    }
    entry.insert("merged".into(), json!(crate::runtime::daemon::now_utc()));
    if let Some(all) = projects.value["projects"].as_object_mut() {
        all.remove(from);
    }
    projects.save(data_dir)
}

/// The lines `doctor` and `install` show for look-alike projects.
pub fn look_alike_lines(data_dir: &Path) -> Vec<(String, String)> {
    let Ok(projects) = Projects::load(data_dir) else {
        return Vec::new();
    };
    projects
        .look_alikes()
        .into_iter()
        .map(|(older, newer)| {
            (
                format!(
                    "project {} looks like project {} moved (same first commit; forks and \
                     template copies share it too)",
                    newer.paths.first().unwrap_or(&newer.id),
                    older.id
                ),
                format!(
                    "if it is, keep its memories with `graphersal-memory projects merge {} {}`",
                    quote_arg(&older.id),
                    quote_arg(&newer.id)
                ),
            )
        })
        .collect()
}

/// `text` quoted for a shell when it needs it.
fn quote_arg(text: &str) -> String {
    if text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-#:@".contains(c))
    {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
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
        let tmp = crate::runtime::mcp_tests::TempDir::new("remote");
        repo(
            tmp.path(),
            "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = git@github.com:up/app.git\n\
             [remote \"origin\"]\n\turl = https://me:tok3n@github.com/me/app.git\n\tfetch = x\n",
        );
        assert_eq!(
            git_remote(tmp.path()).as_deref(),
            Some("https://github.com/me/app.git")
        );
        assert_eq!(
            git_remotes(tmp.path()),
            vec![
                "https://github.com/me/app.git".to_string(),
                "git@github.com:up/app.git".to_string()
            ]
        );
        let other = crate::runtime::mcp_tests::TempDir::new("remote2");
        repo(
            other.path(),
            "[remote \"upstream\"]\n  url = git@github.com:up/app.git\n",
        );
        assert_eq!(
            git_remote(other.path()).as_deref(),
            Some("git@github.com:up/app.git")
        );
        let none = crate::runtime::mcp_tests::TempDir::new("remote3");
        assert_eq!(git_remote(none.path()), None);
    }

    #[test]
    fn a_worktree_finds_the_shared_configuration() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("wt");
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
        let data = crate::runtime::mcp_tests::TempDir::new("proj");
        let project = crate::runtime::mcp_tests::TempDir::new("projrepo");
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

    #[test]
    fn remotes_normalize_to_one_key() {
        for url in [
            "git@github.com:me/App.git",
            "https://tok@GitHub.com/me/App",
            "https://github.com/me/App.git/",
            "ssh://git@github.com:22/me/App.git",
        ] {
            assert_eq!(normalize_remote(url), "github.com/me/App", "{url}");
        }
        assert_eq!(normalize_remote("/srv/git/app.git"), "/srv/git/app.git");
        assert_eq!(normalize_remote("file:///srv/git/app/"), "/srv/git/app");
    }

    fn keys(remotes: &[&str], root: Option<&str>) -> impl Fn(bool) -> Keys {
        let remotes: Vec<String> = remotes.iter().map(|r| r.to_string()).collect();
        let root = root.map(str::to_string);
        move |with_root| Keys {
            remotes: remotes.clone(),
            root_commit: if with_root { root.clone() } else { None },
        }
    }

    const ROOT: &str = "0123456789012345678901234567890123456789";

    #[test]
    fn a_moved_repository_with_its_remote_keeps_its_id() {
        let data = crate::runtime::mcp_tests::TempDir::new("proj-move");
        let remote = ["https://example.org/me/app.git"];
        let first = resolve_with(data.path(), "/w/app", &keys(&remote, Some(ROOT))).unwrap();
        assert_eq!((first.id.as_str(), first.created), ("/w/app", true));
        let moved = resolve_with(
            data.path(),
            "/new/app",
            &keys(&["git@example.org:me/app"], None),
        )
        .unwrap();
        assert_eq!((moved.id.as_str(), moved.created), ("/w/app", false));
        let projects = Projects::load(data.path()).unwrap();
        assert_eq!(
            projects.get("/w/app").unwrap().paths,
            vec!["/new/app".to_string(), "/w/app".to_string()]
        );
        assert_eq!(projects.find("/new/app").unwrap().id, "/w/app");
        // A second clone of the same remote is the same project (intended).
        let clone = resolve_with(data.path(), "/elsewhere/app", &keys(&remote, None)).unwrap();
        assert_eq!(clone.id, "/w/app");
    }

    #[test]
    fn a_local_repository_that_gets_a_remote_keeps_its_id_and_records_the_remote() {
        let data = crate::runtime::mcp_tests::TempDir::new("proj-remote");
        let first = resolve_with(data.path(), "/w/local", &keys(&[], None)).unwrap();
        assert_eq!(first.id, "/w/local");
        let later = resolve_with(
            data.path(),
            "/w/local",
            &keys(&["https://me@example.org/me/local.git"], None),
        )
        .unwrap();
        assert_eq!((later.id.as_str(), later.created), ("/w/local", false));
        assert_eq!(
            recorded_remote(data.path(), "/w/local").as_deref(),
            Some("https://example.org/me/local.git")
        );
        // Now it is found by the remote anywhere.
        let moved = resolve_with(
            data.path(),
            "/x/local",
            &keys(&["https://example.org/me/local"], None),
        )
        .unwrap();
        assert_eq!(moved.id, "/w/local");
    }

    #[test]
    fn moved_with_a_new_remote_is_a_new_project_that_looks_alike() {
        let data = crate::runtime::mcp_tests::TempDir::new("proj-alike");
        resolve_with(
            data.path(),
            "/w/app",
            &keys(&["https://a.example/app"], Some(ROOT)),
        )
        .unwrap();
        let moved = resolve_with(
            data.path(),
            "/v/app",
            &keys(&["https://b.example/app"], Some(ROOT)),
        )
        .unwrap();
        assert!(moved.created);
        assert_eq!(moved.look_alikes, vec!["/w/app".to_string()]);
        let lines = look_alike_lines(data.path());
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0]
                .0
                .contains("/v/app looks like project /w/app moved"),
            "{lines:?}"
        );
        assert!(
            lines[0].1.contains("projects merge /w/app /v/app"),
            "{lines:?}"
        );
        merge_records(data.path(), "/w/app", "/v/app").unwrap();
        let projects = Projects::load(data.path()).unwrap();
        assert!(projects.get("/w/app").is_none());
        let merged = projects.get("/v/app").unwrap();
        assert!(merged.paths.contains(&"/w/app".to_string()));
        assert!(merged.remotes.contains(&"a.example/app".to_string()));
        assert!(look_alike_lines(data.path()).is_empty());
        // Both remotes find the merged project now.
        let back =
            resolve_with(data.path(), "/z", &keys(&["https://a.example/app"], None)).unwrap();
        assert_eq!(back.id, "/v/app");
    }

    #[test]
    fn a_path_taken_as_an_id_gets_a_suffix() {
        let data = crate::runtime::mcp_tests::TempDir::new("proj-suffix");
        resolve_with(
            data.path(),
            "/w/app",
            &keys(&["https://a.example/app"], None),
        )
        .unwrap();
        resolve_with(
            data.path(),
            "/w/moved",
            &keys(&["https://a.example/app"], None),
        )
        .unwrap();
        // The old path is known: it is that project, not a new one.
        assert_eq!(
            resolve_with(data.path(), "/w/app", &keys(&[], None))
                .unwrap()
                .id,
            "/w/app"
        );
        // A different repository first seen at a path that is someone's id but not a known path.
        let mut projects = Projects::load(data.path()).unwrap();
        if let Some(entry) = projects.entry_mut("/w/app") {
            entry.insert("paths".into(), json!(["/w/moved"]));
        }
        projects.save(data.path()).unwrap();
        let other = resolve_with(
            data.path(),
            "/w/app",
            &keys(&["https://other.example/x"], None),
        )
        .unwrap();
        assert_eq!((other.id.as_str(), other.created), ("/w/app#2", true));
        assert_eq!(
            resolve_with(data.path(), "", &keys(&[], None)).unwrap().id,
            ""
        );
    }

    #[test]
    fn entries_from_before_ids_are_read_with_their_path_as_id() {
        let data = crate::runtime::mcp_tests::TempDir::new("proj-legacy");
        std::fs::write(
            data.path().join(PROJECTS_FILE),
            r#"{"version": 1, "projects": {"/old/app": {"git_remote": "https://example.org/me/app.git", "first_seen": "2026-11-10T09:00:00Z"}}}"#,
        )
        .unwrap();
        let projects = Projects::load(data.path()).unwrap();
        let old = projects.get("/old/app").unwrap();
        assert_eq!(old.paths, vec!["/old/app".to_string()]);
        assert_eq!(old.remotes, vec!["example.org/me/app".to_string()]);
        // Moved: found by its remote, memories keep their id.
        let moved = resolve_with(
            data.path(),
            "/new/app",
            &keys(&["git@example.org:me/app.git"], None),
        )
        .unwrap();
        assert_eq!(moved.id, "/old/app");
        assert!(
            Projects::load(data.path())
                .unwrap()
                .get("/old/app")
                .unwrap()
                .paths
                .contains(&"/new/app".to_string())
        );
        // Its root commit is recorded the first time it is known.
        resolve_with(data.path(), "/new/app", &keys(&[], Some(ROOT))).unwrap();
        assert_eq!(
            Projects::load(data.path())
                .unwrap()
                .get("/old/app")
                .unwrap()
                .root_commit
                .as_deref(),
            Some(ROOT)
        );
    }

    #[test]
    fn the_root_commit_comes_from_git() {
        let tmp = crate::runtime::mcp_tests::TempDir::new("rootc");
        assert_eq!(root_commit(tmp.path()), None);
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(tmp.path())
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.org")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.org")
                .output()
        };
        let Ok(init) = git(&["init", "-q"]) else {
            return; // no git on this machine
        };
        if !init.status.success() {
            return;
        }
        assert_eq!(root_commit(tmp.path()), None, "no commit yet");
        git(&["commit", "-q", "--allow-empty", "-m", "first"]).unwrap();
        let root = root_commit(tmp.path()).unwrap();
        git(&["commit", "-q", "--allow-empty", "-m", "second"]).unwrap();
        assert_eq!(root_commit(tmp.path()), Some(root));
    }
}
