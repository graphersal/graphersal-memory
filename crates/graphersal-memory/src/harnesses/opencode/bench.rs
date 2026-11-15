//! opencode's part of the bench (`crate::bench::BenchSupport`): its sessions in its database,
//! its instruction files, and its sandboxes (`crate::bench::arms`).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{Value, json};

use super::OpenCode;
use crate::bench::arms::{
    READ_FUNCTIONS, SERVER, Sandbox, Sources, fork_store, instruction_section, without_block,
};
use crate::bench::questions::RECENT;
use crate::bench::{BenchSupport, ToolFreeCall, arms, questions};
use crate::cli::BenchArm;
use crate::memory::transcript;

impl BenchSupport for OpenCode {
    fn excerpt(&self, project: &str, now: SystemTime) -> questions::Excerpt {
        questions::Excerpt::of_transcripts(
            &transcripts(&self.paths.db, project, now),
            questions::EXCERPT_BUDGET,
        )
    }

    fn sessions_place(&self, _project: &str) -> (&'static str, PathBuf) {
        ("opencode session", self.paths.db.clone())
    }

    fn instructions(&self, project: &str) -> Vec<(String, String)> {
        instructions(&self.paths, project)
    }

    fn no_file_memory_note(&self) -> Option<&'static str> {
        Some(
            "opencode has no file memory: the files arm holds its instruction files (AGENTS.md) \
             only.",
        )
    }

    fn build_arm(
        &self,
        arm: BenchArm,
        arms_dir: &Path,
        sources: &arms::Sources,
    ) -> Result<arms::Sandbox, String> {
        build(arm, arms_dir, sources, &self.paths.db)
    }

    fn tool_free_call(
        &self,
        dir: &Path,
        mut command: Vec<String>,
        _schema: Value,
        _cap: f64,
    ) -> Result<ToolFreeCall, String> {
        let (cwd, env) = tool_free(dir, &self.paths.db)?;
        command.extend(["--format".to_string(), "json".to_string()]);
        Ok((cwd, command, env))
    }

    fn free_note(&self, command: &[String]) -> Option<String> {
        super::free_model(command).then(|| {
            format!(
                "`{}` uses a free model of opencode Zen (free for a limited time): this run costs \
                 $0.00; the budget still stops it if opencode reports a cost.",
                command.join(" ")
            )
        })
    }
}

/// opencode's instruction files of `project`: the global `AGENTS.md` and the project's
/// `AGENTS.md` (1.x falls back to Claude Code's `~/.claude/CLAUDE.md` and the project's
/// `CLAUDE.md`; 2.x has no such fallback), those that exist, each without
/// graphersal-memory's managed block.
pub fn instructions(
    paths: &crate::harnesses::opencode::OpenCodePaths,
    project: &str,
) -> Vec<(String, String)> {
    let root = Path::new(project);
    let mut out = Vec::new();
    let fallback = paths.major() == crate::harnesses::opencode::Major::V1;
    let mut global = vec![paths.config_dir.join("AGENTS.md")];
    let mut local = vec![root.join("AGENTS.md")];
    if fallback {
        global.push(paths.home.join(".claude").join("CLAUDE.md"));
        local.push(root.join("CLAUDE.md"));
    }
    for (label, candidates) in [("global AGENTS.md", global), ("project AGENTS.md", local)] {
        let Some(text) = candidates
            .iter()
            .find_map(|path| std::fs::read_to_string(path).ok())
        else {
            continue;
        };
        let text = without_block(&text);
        if !text.trim().is_empty() {
            out.push((label.to_string(), text));
        }
    }
    out
}

/// The sandbox of `arm` for opencode in `<arms_dir>/<arm>/`: `opencode run --format json` in
/// its own configuration and data directories (`user_db`: whose `auth.json` is copied).
pub fn build(
    arm: BenchArm,
    arms_dir: &Path,
    sources: &Sources,
    user_db: &Path,
) -> Result<Sandbox, String> {
    let dir = arms_dir.join(arm.name());
    let cwd = dir.join("cwd");
    std::fs::create_dir_all(&cwd)
        .map_err(|err| format!("cannot create {}: {err}", cwd.display()))?;
    let system_file = dir.join("system.md");
    let mut scratch = None;
    let (mcp, allow, system, description): (Value, Vec<String>, String, String) = match arm {
        BenchArm::None => (
            json!({}),
            Vec::new(),
            "# Memory\n\nYou have no memory of earlier sessions with this developer.\n".to_string(),
            "no memory: no instruction files, no tools".to_string(),
        ),
        BenchArm::Files => (
            json!({}),
            Vec::new(),
            format!(
                "{}# Memory\n\nYour memory of this developer is the instructions above (opencode \
                 keeps no other memory).\n",
                instruction_section(sources)
            ),
            format!(
                "file memory: {} instruction file(s) (opencode has no memory files), no tools",
                sources.instructions.len()
            ),
        ),
        BenchArm::Graph => {
            let data = dir.join("data-store");
            let fork = crate::runtime::store::default_store_dir(&data);
            fork_store(&sources.store_dir, &fork)?;
            scratch = Some(data.clone());
            (
                json!({SERVER: {
                    "type": "local",
                    "command": [sources.exe.display().to_string(), "mcp", "--no-daemon",
                                "--data-dir", data.display().to_string(),
                                "--project", sources.project],
                    "environment": {crate::memory::summaries::HOOK_ENV: "off"},
                    "enabled": true,
                }}),
                READ_FUNCTIONS
                    .iter()
                    .map(|f| format!("{SERVER}_{f}"))
                    .collect(),
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
    arms::write(&system_file, &system)?;
    let allow: Vec<&str> = allow.iter().map(String::as_str).collect();
    let env = crate::harnesses::opencode::build_sandbox(
        &dir.join("opencode"),
        &json!({"mcp": mcp, "permission": crate::harnesses::opencode::permissions(&allow),
                "instructions": [system_file.display().to_string()]}),
        Some(user_db),
    )?;
    Ok(Sandbox {
        arm,
        cwd,
        args: vec!["--format".to_string(), "json".to_string()],
        description,
        scratch,
        env,
    })
}

/// The opencode sandbox of the generator and the judge: no tools, no MCP servers, no context,
/// in `<dir>/`. Returns the working directory and the environment.
pub fn tool_free(dir: &Path, user_db: &Path) -> Result<(PathBuf, Vec<(String, String)>), String> {
    let cwd = dir.join("cwd");
    std::fs::create_dir_all(&cwd)
        .map_err(|err| format!("cannot create {}: {err}", cwd.display()))?;
    let env = crate::harnesses::opencode::build_sandbox(
        &dir.join("opencode"),
        &json!({"mcp": {}, "permission": crate::harnesses::opencode::permissions(&[])}),
        Some(user_db),
    )?;
    Ok((cwd, env))
}

/// The finished opencode sessions of `project` in `db` (unchanged for 30 minutes), newest
/// first, parsed.
pub fn transcripts(
    db: &Path,
    project: &str,
    now: SystemTime,
) -> Vec<(String, transcript::Transcript)> {
    let now = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    crate::harnesses::opencode::sessions::of_project(db, Path::new(project))
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, updated)| now.saturating_sub(*updated) >= RECENT.as_secs())
        .filter_map(|(id, _)| {
            crate::harnesses::opencode::sessions::read(db, &id)
                .ok()
                .map(|t| (id, t))
        })
        .collect()
}
