//! Codex's part of the bench (`crate::bench::BenchSupport`): its rollouts, its instruction
//! files, and its sandboxes (`crate::bench::arms`).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use super::Codex;
use crate::bench::arms::{
    READ_FUNCTIONS, SERVER, Sandbox, Sources, fork_store, instruction_section, without_block,
};
use crate::bench::questions::RECENT;
use crate::bench::{BenchSupport, ToolFreeCall, arms, questions};
use crate::cli::BenchArm;
use crate::memory::transcript;

impl BenchSupport for Codex {
    fn excerpt(&self, project: &str, now: SystemTime) -> questions::Excerpt {
        questions::Excerpt::of_transcripts(
            &transcripts(&self.paths.sessions, project, now),
            questions::EXCERPT_BUDGET,
        )
    }

    fn sessions_place(&self, _project: &str) -> (&'static str, PathBuf) {
        ("Codex session", self.paths.sessions.clone())
    }

    fn instructions(&self, project: &str) -> Vec<(String, String)> {
        instructions(&self.paths, project)
    }

    fn no_file_memory_note(&self) -> Option<&'static str> {
        Some(
            "Codex has no file memory: the files arm holds its instruction files (AGENTS.md) \
             only.",
        )
    }

    fn build_arm(
        &self,
        arm: BenchArm,
        arms_dir: &Path,
        sources: &arms::Sources,
    ) -> Result<arms::Sandbox, String> {
        build(arm, arms_dir, sources, &self.paths)
    }

    fn tool_free_call(
        &self,
        dir: &Path,
        command: Vec<String>,
        _schema: Value,
        _cap: f64,
    ) -> Result<ToolFreeCall, String> {
        let (cwd, env) = tool_free(dir, &self.paths)?;
        Ok((cwd, command, env))
    }

    fn free_note(&self, command: &[String]) -> Option<String> {
        super::local_model(command).then(|| {
            format!(
                "`{}` runs a local model on this machine: this run costs $0.00.",
                command.join(" ")
            )
        })
    }
}

/// Codex's instruction files of `project`: the global one (`AGENTS.override.md` when it
/// holds text, else `AGENTS.md`, in the Codex home) and the project's (the same two names
/// in its root), those that exist, each without graphersal-memory's managed block.
pub fn instructions(
    paths: &crate::harnesses::codex::CodexPaths,
    project: &str,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (label, path) in [
        (
            "global AGENTS.md",
            crate::harnesses::codex::instruction_file(&paths.codex_home),
        ),
        (
            "project AGENTS.md",
            crate::harnesses::codex::instruction_file(Path::new(project)),
        ),
    ] {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let text = without_block(&text);
        if !text.trim().is_empty() {
            out.push((label.to_string(), text));
        }
    }
    out
}

/// The `config.toml` of a Codex sandbox with `tables` (the graph arm's server).
fn codex_config(user_config: &Path, tables: &str) -> String {
    crate::harnesses::codex::sandbox_config(Some(user_config), tables)
}

/// The sandbox of `arm` for Codex in `<arms_dir>/<arm>/`: `codex exec --json` with a
/// `CODEX_HOME` of its own (`paths`: the user's Codex home, whose model settings and login are
/// copied).
pub fn build(
    arm: BenchArm,
    arms_dir: &Path,
    sources: &Sources,
    paths: &crate::harnesses::codex::CodexPaths,
) -> Result<Sandbox, String> {
    let dir = arms_dir.join(arm.name());
    let cwd = dir.join("cwd");
    std::fs::create_dir_all(&cwd)
        .map_err(|err| format!("cannot create {}: {err}", cwd.display()))?;
    let mut scratch = None;
    let (tables, system, description) = match arm {
        BenchArm::None => (
            String::new(),
            "# Memory\n\nYou have no memory of earlier sessions with this developer.\n".to_string(),
            "no memory: no instruction files, no tools".to_string(),
        ),
        BenchArm::Files => (
            String::new(),
            format!(
                "{}# Memory\n\nYour memory of this developer is the instructions above (Codex \
                 keeps no memory files).\n",
                instruction_section(sources)
            ),
            format!(
                "file memory: {} instruction file(s) (Codex has no memory files), no tools",
                sources.instructions.len()
            ),
        ),
        BenchArm::Graph => {
            let data = dir.join("data-store");
            let fork = crate::runtime::store::default_store_dir(&data);
            fork_store(&sources.store_dir, &fork)?;
            scratch = Some(data.clone());
            let args = [
                "mcp".to_string(),
                "--no-daemon".to_string(),
                "--data-dir".to_string(),
                data.display().to_string(),
                "--project".to_string(),
                sources.project.clone(),
            ];
            let tools: Vec<String> = READ_FUNCTIONS.iter().map(|f| f.to_string()).collect();
            (
                format!(
                    "[mcp_servers.{SERVER}]\ncommand = {}\nargs = {}\nstartup_timeout_sec = {}\n\
                     enabled_tools = {}\ndefault_tools_approval_mode = \"approve\"\n\
                     env = {{ {} = \"off\" }}\n",
                    crate::install::tomledit::string(&sources.exe.display().to_string()),
                    crate::install::tomledit::strings(&args),
                    crate::harnesses::codex::STARTUP_TIMEOUT_SECS,
                    crate::install::tomledit::strings(&tools),
                    crate::memory::summaries::HOOK_ENV
                ),
                format!(
                    "{}# Memory\n\nYour memory of this developer and every past session of this \
                     project is kept in a graph behind your MCP tools (server {SERVER}): \
                     memories, memory, sessions, session, recall. Look up what you need. The \
                     memory is read-only in this task.\n",
                    instruction_section(sources)
                ),
                format!(
                    "graph memory: {} instruction file(s), the use tools {} on a fork of the \
                     store",
                    sources.instructions.len(),
                    READ_FUNCTIONS.join("/")
                ),
            )
        }
        BenchArm::Current => {
            return Err(
                "the arm `current` (a copy of your own MCP setup) is not built yet. Use none, \
                 files and graph."
                    .to_string(),
            );
        }
    };
    let env = crate::harnesses::codex::build_sandbox(
        &dir.join("codex"),
        &codex_config(&paths.config_file, &tables),
        Some(&system),
        Some(&paths.auth_file()),
    )?;
    Ok(Sandbox {
        arm,
        cwd,
        args: Vec::new(),
        description,
        scratch,
        env,
    })
}

/// The Codex sandbox of the generator and the judge: no tools, no MCP servers, no context, in
/// `<dir>/`. Returns the working directory and the environment.
pub fn tool_free(
    dir: &Path,
    paths: &crate::harnesses::codex::CodexPaths,
) -> Result<(PathBuf, Vec<(String, String)>), String> {
    let cwd = dir.join("cwd");
    std::fs::create_dir_all(&cwd)
        .map_err(|err| format!("cannot create {}: {err}", cwd.display()))?;
    let env = crate::harnesses::codex::build_sandbox(
        &dir.join("codex"),
        &codex_config(&paths.config_file, ""),
        None,
        Some(&paths.auth_file()),
    )?;
    Ok((cwd, env))
}

/// The finished Codex rollouts of `project` under `sessions` (unchanged for 30 minutes), newest
/// first, parsed (the id is the session's thread id, as the indexing names it).
pub fn transcripts(
    sessions: &Path,
    project: &str,
    now: SystemTime,
) -> Vec<(String, transcript::Transcript)> {
    let now = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut found: Vec<(u64, PathBuf)> =
        crate::harnesses::codex::sessions::of_project(sessions, Path::new(project))
            .into_iter()
            .filter(|(_, modified)| now.saturating_sub(*modified) >= RECENT.as_secs())
            .map(|(path, modified)| (modified, path))
            .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    found
        .into_iter()
        .filter_map(|(_, path)| {
            crate::harnesses::codex::sessions::read(&path)
                .ok()
                .map(|t| (crate::harnesses::codex::sessions::thread_id(&path), t))
        })
        .collect()
}
