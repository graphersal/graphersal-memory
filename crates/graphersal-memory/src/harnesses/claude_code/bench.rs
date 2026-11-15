//! Claude Code's part of the bench (`crate::bench::BenchSupport`): its transcripts, its
//! instruction files and memory files, and its sandboxes (`crate::bench::arms`).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{Value, json};

use super::ClaudeCode;
use crate::bench::arms::{
    READ_FUNCTIONS, SERVER, Sandbox, Sources, fork_store, instruction_section, pretty,
    without_block,
};
use crate::bench::questions::RECENT;
use crate::bench::{BenchSupport, ToolFreeCall, arms, questions};
use crate::cli::BenchArm;

impl BenchSupport for ClaudeCode {
    fn excerpt(&self, project: &str, now: SystemTime) -> questions::Excerpt {
        let transcripts = transcripts(&self.paths.projects, project, now);
        excerpt_of(&transcripts, questions::EXCERPT_BUDGET)
    }

    fn sessions_place(&self, project: &str) -> (&'static str, PathBuf) {
        (
            "session transcript",
            self.paths
                .projects
                .join(super::import::claude_slug(project)),
        )
    }

    /// Other projects (git repositories) with finished Claude Code sessions.
    fn other_projects_with_sessions(&self, project: &str, now: SystemTime) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(&self.paths.projects) else {
            return Vec::new();
        };
        let mut found: Vec<(usize, PathBuf)> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|dir| dir.is_dir())
            .filter_map(|dir| super::import::resolve_project_dir(&dir))
            .filter(|root| root.join(".git").exists())
            .filter_map(|root| {
                let text = root.display().to_string();
                if text == project {
                    return None;
                }
                let count = transcripts(&self.paths.projects, &text, now).len();
                (count > 0).then_some((count, root))
            })
            .collect();
        found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        found.dedup_by(|a, b| a.1 == b.1);
        found
            .into_iter()
            .take(3)
            .map(|(_, project)| project)
            .collect()
    }

    fn instructions(&self, project: &str) -> Vec<(String, String)> {
        instructions(&self.paths.instructions, project)
    }

    fn memory_dir(&self, project: &str) -> Option<PathBuf> {
        Some(
            self.paths
                .projects
                .join(super::import::claude_slug(project))
                .join("memory"),
        )
        .filter(|dir| dir.is_dir())
    }

    fn build_arm(
        &self,
        arm: BenchArm,
        arms_dir: &Path,
        sources: &arms::Sources,
    ) -> Result<arms::Sandbox, String> {
        build(arm, arms_dir, sources)
    }

    /// `claude -p` with the sandbox's flags, the schema and the cost cap of the call.
    fn tool_free_call(
        &self,
        dir: &Path,
        mut command: Vec<String>,
        schema: Value,
        cap: f64,
    ) -> Result<ToolFreeCall, String> {
        let (cwd, args) = tool_free(dir)?;
        command.extend(args);
        command.extend([
            "--json-schema".to_string(),
            schema.to_string(),
            "--max-budget-usd".to_string(),
            format!("{cap:.2}"),
        ]);
        Ok((cwd, command, Vec::new()))
    }

    fn cost_cap(&self, cap: f64) -> Vec<String> {
        vec!["--max-budget-usd".to_string(), format!("{cap:.2}")]
    }
}

/// The instruction files of `project`: the user's (`<claude dir>/CLAUDE.md`) and the
/// project's (`CLAUDE.md`, `CLAUDE.local.md` in its root), those that exist, each without
/// graphersal-memory's managed block.
pub fn instructions(claude_md: &Path, project: &str) -> Vec<(String, String)> {
    let root = Path::new(project);
    let mut out = Vec::new();
    for (label, path) in [
        ("user CLAUDE.md".to_string(), claude_md.to_path_buf()),
        ("project CLAUDE.md".to_string(), root.join("CLAUDE.md")),
        (
            "project CLAUDE.local.md".to_string(),
            root.join("CLAUDE.local.md"),
        ),
    ] {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let text = without_block(&text);
        if !text.trim().is_empty() {
            out.push((label, text));
        }
    }
    out
}

/// The settings every call gets (with `--setting-sources ""`, the only settings).
pub fn settings() -> Value {
    json!({
        "disableAllHooks": true,
        "permissions": {"deny": ["Bash", "Edit", "Write", "NotebookEdit", "WebFetch", "WebSearch"]},
    })
}

/// The arguments every call gets after the harness command: JSON output, no transcript, no
/// settings but `settings`, no MCP servers but those of `mcp_config`, no slash commands.
pub fn common_args(settings: &Path, mcp_config: &Path) -> Vec<String> {
    [
        "--output-format",
        "json",
        "--no-session-persistence",
        "--setting-sources",
        "",
        "--settings",
        &settings.display().to_string(),
        "--strict-mcp-config",
        "--mcp-config",
        &mcp_config.display().to_string(),
        "--disable-slash-commands",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Builds the sandbox of `arm` in `<arms_dir>/<arm>/`.
pub fn build(arm: BenchArm, arms_dir: &Path, sources: &Sources) -> Result<Sandbox, String> {
    let dir = arms_dir.join(arm.name());
    let cwd = dir.join("cwd");
    std::fs::create_dir_all(&cwd)
        .map_err(|err| format!("cannot create {}: {err}", cwd.display()))?;
    let settings_file = dir.join("settings.json");
    arms::write(&settings_file, &pretty(&settings()))?;
    let mcp_file = dir.join("mcp.json");
    let system_file = dir.join("system.md");
    let mut args = common_args(&settings_file, &mcp_file);
    let mut scratch = None;
    let (servers, system, description) = match arm {
        BenchArm::None => {
            args.extend(["--tools".to_string(), String::new()]);
            (
                json!({}),
                "# Memory\n\nYou have no memory of earlier sessions with this developer.\n"
                    .to_string(),
                "no memory: no instruction files, no tools".to_string(),
            )
        }
        BenchArm::Files => {
            let mut copied = 0;
            let mut index = String::new();
            if let Some(memory_dir) = &sources.memory_dir {
                let entries = std::fs::read_dir(memory_dir)
                    .map_err(|err| format!("cannot read {}: {err}", memory_dir.display()))?;
                for path in entries.filter_map(Result::ok).map(|e| e.path()) {
                    if !path.is_file() || path.extension().is_none_or(|ext| ext != "md") {
                        continue;
                    }
                    let Some(name) = path.file_name() else {
                        continue;
                    };
                    std::fs::copy(&path, cwd.join(name))
                        .map_err(|err| format!("cannot copy {}: {err}", path.display()))?;
                    copied += 1;
                }
                index = std::fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap_or_default();
            }
            args.extend(["--tools".to_string(), "Read,Grep,Glob".to_string()]);
            (
                json!({}),
                format!(
                    "{}# Memory\n\nYou have a persistent memory from earlier sessions on this \
                     project. Its index, MEMORY.md, is shown below. Each line links a memory \
                     file; the files are in your current working directory. Read them with \
                     Read/Grep/Glob.\n\nContents of MEMORY.md:\n{index}\n",
                    instruction_section(sources)
                ),
                format!(
                    "file memory: {} instruction file(s), MEMORY.md and {copied} memory file(s) \
                     copied, tools Read/Grep/Glob",
                    sources.instructions.len()
                ),
            )
        }
        BenchArm::Graph => {
            let data = dir.join("data");
            let fork = crate::runtime::store::default_store_dir(&data);
            fork_store(&sources.store_dir, &fork)?;
            scratch = Some(data.clone());
            args.extend([
                "--tools".to_string(),
                String::new(),
                "--allowedTools".to_string(),
                READ_FUNCTIONS
                    .iter()
                    .map(|f| format!("mcp__{SERVER}__{f}"))
                    .collect::<Vec<_>>()
                    .join(","),
            ]);
            (
                json!({SERVER: {
                    "type": "stdio",
                    "command": sources.exe.display().to_string(),
                    "args": ["mcp", "--no-daemon", "--data-dir", data.display().to_string(),
                             "--project", sources.project],
                    "env": {crate::memory::summaries::HOOK_ENV: "off"},
                }}),
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
                "the arm `current` (a copy of your own MCP setup) is not built yet: copying \
                 another tool's server could let it write to its live data. Use none, files \
                 and graph."
                    .to_string(),
            );
        }
    };
    arms::write(&mcp_file, &pretty(&json!({"mcpServers": servers})))?;
    arms::write(&system_file, &system)?;
    args.extend([
        "--append-system-prompt-file".to_string(),
        system_file.display().to_string(),
    ]);
    Ok(Sandbox {
        arm,
        cwd,
        args,
        description,
        scratch,
        env: Vec::new(),
    })
}

/// The sandbox of the generator and the judge: no tools, no MCP servers, no context, in
/// `<run dir>/<name>/`.
pub fn tool_free(dir: &Path) -> Result<(PathBuf, Vec<String>), String> {
    let cwd = dir.join("cwd");
    std::fs::create_dir_all(&cwd)
        .map_err(|err| format!("cannot create {}: {err}", cwd.display()))?;
    let settings_file = dir.join("settings.json");
    arms::write(&settings_file, &pretty(&settings()))?;
    let mcp_file = dir.join("mcp.json");
    arms::write(&mcp_file, &pretty(&json!({"mcpServers": {}})))?;
    let mut args = common_args(&settings_file, &mcp_file);
    args.extend(["--tools".to_string(), String::new()]);
    Ok((cwd, args))
}

/// The finished transcripts of `project` in Claude Code's `projects` directory (its directory
/// is named after the project's path), newest first.
pub fn transcripts(projects_dir: &Path, project: &str, now: SystemTime) -> Vec<PathBuf> {
    let dir = projects_dir.join(crate::harnesses::claude_code::import::claude_slug(project));
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|ext| ext == "jsonl"))
        .filter_map(|p| {
            let modified = std::fs::metadata(&p).ok()?.modified().ok()?;
            let age = now.duration_since(modified).unwrap_or_default();
            (age >= RECENT).then_some((modified, p))
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    files.into_iter().map(|(_, p)| p).collect()
}

/// The excerpt of `files` (Claude Code transcripts, newest first).
pub fn excerpt_of(files: &[PathBuf], budget: usize) -> questions::Excerpt {
    let parsed: Vec<(String, crate::memory::transcript::Transcript)> = files
        .iter()
        .filter_map(|file| {
            let bytes = std::fs::read(file).ok()?;
            let uuid = file
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let parsed = super::transcript::parse(&bytes, &uuid);
            Some((uuid, parsed))
        })
        .collect();
    questions::Excerpt::of_transcripts(&parsed, budget)
}
