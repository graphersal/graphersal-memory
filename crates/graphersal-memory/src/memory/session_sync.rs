//! What the MCP server (the shim, `graphersal-memory mcp`) does at a session's start and end, so
//! the product works without hooks: on Windows without Git Bash an older hook form does not
//! run, and Claude Desktop and other hosts have no hooks at all. Every harness starts our MCP
//! server when a session starts and closes its stdin when it ends, so the shim can do the
//! hooks' work itself (`crate::memory::hook` lists it):
//!
//! - **connect** = session start: the Claude Code memory files of the session's project that
//!   are new or changed are imported (as the `session-start` hook does), and the indexing of
//!   every harness's finished sessions is queued in the daemon (as `import --sessions` does);
//! - **disconnect** (stdin closed) = session end: the indexing is queued again.
//!
//! Both run beside the MCP stream (a thread of their own after the handshake; the end within a
//! short wait after the stream closed), never write to stdout, and only note failures on stderr.
//! When the hooks run too, nothing is done twice in a harmful way: the import takes the import
//! lock and skips files whose fingerprint it already has, and the daemon merges index requests
//! and skips unchanged sessions by their fingerprint.
//!
//! What still needs a hook: the shim does not know which transcript is its own session's, so
//! that session is indexed once it has been unchanged for `sessions::IDLE` (at a later start or
//! end) rather than right at its end; and Cursor's transcripts are known only from what its hooks
//! reported (`crate::harnesses::cursor::sessions`).

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::harnesses::agent::Kind;
use crate::install::{ClaudePaths, Setup};
use crate::memory::import::Connect;
use crate::memory::sessions::Request;

/// What a shim does at its start and end.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The data directory (the import's state and lock).
    pub data_dir: PathBuf,
    /// Claude Code's files, when its memory files are imported.
    pub claude: Option<ClaudePaths>,
    /// The directories whose Claude Code memory files are imported (the session's working
    /// directory and its project root, when they differ).
    pub dirs: Vec<PathBuf>,
    /// The project the imported memories belong to (a directory the project key is taken
    /// from).
    pub project_dir: PathBuf,
    /// The indexing requests, one per harness.
    pub requests: Vec<Request>,
}

impl Plan {
    /// Whether there is nothing to do.
    pub fn is_empty(&self) -> bool {
        self.claude.is_none() && self.requests.is_empty()
    }
}

/// The plan of a shim started in `cwd` for `project` (its root) by `harness` (the name from
/// the environment, `crate::runtime::shim::harness_name`): the harnesses graphersal-memory is installed
/// in, else the harness that started it when known; for each one whose sessions exist, an
/// indexing request; Claude Code's memory files when Claude Code is one of them.
pub fn plan(setup: &Setup, harness: &str, cwd: &Path, project: &Path) -> Plan {
    let mut kinds = setup.installed();
    if kinds.is_empty()
        && let Some(kind) = Kind::from_id(harness)
    {
        kinds.push(kind);
    }
    let requests = kinds
        .iter()
        .filter(|kind| setup.agent_of(**kind).has_sessions())
        .map(|kind| Request::for_harness(*kind, setup.agent_of(*kind).projects_dir()))
        .collect();
    let mut dirs = vec![cwd.to_path_buf()];
    if project != cwd {
        dirs.push(project.to_path_buf());
    }
    Plan {
        data_dir: setup.data_dir.clone(),
        // The file memory imported at the session start (Claude Code's).
        claude: kinds
            .iter()
            .any(|kind| setup.agent_of(*kind).has_file_memory())
            .then(|| setup.paths.clone()),
        dirs,
        project_dir: project.to_path_buf(),
        requests,
    }
}

/// Sends an indexing request to the daemon.
pub type Enqueue<'a> = dyn FnMut(Request) -> Result<(), String> + 'a;

/// What a shim's start and end did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Done {
    /// Memory files imported.
    pub imported: usize,
    /// Indexing requests the daemon took.
    pub queued: usize,
    /// What failed (for stderr).
    pub notes: Vec<String>,
}

/// The session start: imports the changed memory files (until `deadline`), then queues the
/// indexing.
pub fn start(
    plan: &Plan,
    connect: &mut Connect<'_>,
    enqueue: &mut Enqueue<'_>,
    deadline: Instant,
) -> Done {
    let mut done = Done::default();
    if let Some(paths) = &plan.claude {
        for dir in &plan.dirs {
            let (imported, notes) = crate::harnesses::claude_code::hook::import_project(
                dir,
                &plan.project_dir,
                paths,
                &plan.data_dir,
                connect,
                deadline,
            );
            done.imported += imported;
            done.notes.extend(notes);
        }
    }
    queue(plan, enqueue, &mut done);
    done
}

/// The session end: queues the indexing.
#[cfg(test)]
pub fn end(plan: &Plan, enqueue: &mut Enqueue<'_>) -> Done {
    end_since(plan, None, enqueue)
}

/// The session end of a shim that started at `since` (seconds since the epoch): queues the
/// indexing, with CodeWhale's sessions of the shim's directories written since then marked
/// ended. CodeWhale has no hooks of ours, and its stdio MCP servers live as long as the TUI
/// (`crate::harnesses::codewhale`), so the connection's end is the end of the sessions it served. A
/// session of a second CodeWhale in the same directory may be marked too; it is indexed again
/// when it changes.
pub fn end_since(plan: &Plan, since: Option<u64>, enqueue: &mut Enqueue<'_>) -> Done {
    let mut done = Done::default();
    let mut plan = plan.clone();
    if let Some(since) = since {
        for request in &mut plan.requests {
            let reader = crate::harnesses::sessions(request.harness);
            for dir in &plan.dirs {
                for path in reader.written_since(&request.projects_dir, dir, since) {
                    request.ended.insert(path);
                }
            }
        }
    }
    queue(&plan, enqueue, &mut done);
    done
}

fn queue(plan: &Plan, enqueue: &mut Enqueue<'_>, done: &mut Done) {
    for request in &plan.requests {
        match enqueue(request.clone()) {
            Ok(()) => done.queued += 1,
            Err(err) => done.notes.push(format!(
                "cannot queue the indexing of {} sessions: {err}",
                request.harness.display_name()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::install_tests::setup_in;
    use crate::runtime::mcp_tests::TempDir;

    #[test]
    fn nothing_installed_and_no_known_harness_does_nothing() {
        let home = TempDir::new("ss-none-home");
        let data = TempDir::new("ss-none-data");
        let setup = setup_in(home.path(), data.path());
        let plan = plan(&setup, "unknown", home.path(), home.path());
        assert!(plan.is_empty(), "{plan:?}");
    }

    #[test]
    fn the_harness_from_the_environment_counts_when_nothing_is_installed() {
        let home = TempDir::new("ss-env-home");
        let data = TempDir::new("ss-env-data");
        let setup = setup_in(home.path(), data.path());
        std::fs::create_dir_all(&setup.paths.projects).unwrap();
        let work = home.path().join("work");
        let sub = work.join("src");
        let plan = plan(&setup, "claude-code", &sub, &work);
        assert!(plan.claude.is_some());
        assert_eq!(plan.dirs, vec![sub, work]);
        assert_eq!(plan.requests.len(), 1);
        assert_eq!(plan.requests[0].harness, Kind::ClaudeCode);
        assert_eq!(plan.requests[0].projects_dir, setup.paths.projects);
        assert!(plan.requests[0].active.is_empty() && plan.requests[0].ended.is_empty());
        // Codex's sessions exist but Codex did not start it and is not installed: left out.
        std::fs::create_dir_all(&setup.codex.sessions).unwrap();
        let again = super::plan(&setup, "claude-code", home.path(), home.path());
        assert_eq!(again.requests.len(), 1);
        assert_eq!(again.dirs.len(), 1);
    }

    #[test]
    fn codewhale_sessions_of_the_directory_end_with_the_connection() {
        let home = TempDir::new("ss-cw-home");
        let data = TempDir::new("ss-cw-data");
        let setup = setup_in(home.path(), data.path());
        let work = home.path().join("work");
        let session = crate::harnesses::codewhale::sessions::tests::write_fixture(
            &setup.codewhale.sessions,
            &work.display().to_string(),
        );
        crate::harnesses::codewhale::sessions::tests::write_fixture(
            &home.path().join("other-sessions"),
            "/elsewhere",
        );
        let plan = plan(&setup, "codewhale", &work, &work);
        assert_eq!(plan.requests.len(), 1);
        assert_eq!(plan.requests[0].harness, Kind::CodeWhale);
        let mut seen = Vec::new();
        end_since(&plan, Some(0), &mut |request: Request| {
            seen.push(request);
            Ok(())
        });
        assert!(seen[0].ended.contains(&session), "{:?}", seen[0]);
        // A session last written before the shim started is not its own.
        let mut later = Vec::new();
        end_since(&plan, Some(u64::MAX), &mut |request: Request| {
            later.push(request);
            Ok(())
        });
        assert!(later[0].ended.is_empty());
        let mut plain = Vec::new();
        end(&plan, &mut |request: Request| {
            plain.push(request);
            Ok(())
        });
        assert!(plain[0].ended.is_empty());
    }

    #[test]
    fn a_failed_request_is_a_note_and_the_rest_still_goes() {
        let plan = Plan {
            data_dir: PathBuf::from("/nowhere"),
            claude: None,
            dirs: Vec::new(),
            project_dir: PathBuf::from("/w"),
            requests: vec![
                Request::for_harness(Kind::ClaudeCode, Path::new("/p")),
                Request::for_harness(Kind::Codex, Path::new("/c")),
            ],
        };
        let mut seen = Vec::new();
        let mut enqueue = |request: Request| {
            seen.push(request.harness);
            if request.harness == Kind::ClaudeCode {
                Err("refused".to_string())
            } else {
                Ok(())
            }
        };
        let done = end(&plan, &mut enqueue);
        assert_eq!(seen, vec![Kind::ClaudeCode, Kind::Codex]);
        assert_eq!(done.queued, 1);
        assert_eq!(done.notes.len(), 1);
        assert!(done.notes[0].contains("refused"), "{done:?}");
    }
}
