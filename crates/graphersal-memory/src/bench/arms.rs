//! The sandbox of each arm: everything a `claude -p` call of that arm sees, built for the run in
//! `<run dir>/arms/<arm>/`, so the user's live configuration and store are never used or
//! changed:
//!
//! - `mcp.json`: the arm's only MCP servers (`--strict-mcp-config --mcp-config`): none, or for
//!   the graph arm this binary's `mcp --no-daemon` over a FORK of the store in `data/` (never
//!   the live store, never the user's daemon);
//! - `settings.json`: the arm's only settings (`--setting-sources ""` loads no user, project or
//!   local settings, so no hooks, plugins or CLAUDE.md of the user's; `--settings` adds this
//!   file: hooks off, write and shell tools denied);
//! - `system.md`: the arm's context, appended to the system prompt
//!   (`--append-system-prompt-file`): the instruction files (CLAUDE.md, without
//!   graphersal-memory's managed block) for the memory arms, `MEMORY.md` for the files arm;
//! - `cwd/`: the working directory: empty, or for the files arm a COPY of the memory files
//!   (read with Read/Grep/Glob, the only tools of that arm).
//!
//! Every call adds `--no-session-persistence` (no transcript of its own) and runs with
//! `GRAPHERSAL_MEMORY_HOOK=off`.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::cli::BenchArm;
use crate::fsutil;

/// The graph arm's MCP server name (the tool names are `mcp__<name>__<function>`).
pub const SERVER: &str = "graphersal-memory";

/// The read-only memory functions the graph arm may call.
pub const READ_FUNCTIONS: [&str; 5] = ["memories", "memory", "sessions", "session", "recall"];

/// What the arms are built from.
#[derive(Debug, Clone)]
pub struct Sources {
    /// The project key.
    pub project: String,
    /// The instruction files (label, text), the managed block removed.
    pub instructions: Vec<(String, String)>,
    /// The project's memory directory (`MEMORY.md` and the memory files), when it has one.
    pub memory_dir: Option<PathBuf>,
    /// The live store (read only: it is forked).
    pub store_dir: PathBuf,
    /// This binary (the graph arm's MCP server).
    pub exe: PathBuf,
}

impl Sources {
    /// The instruction files of `project`: the user's (`<claude dir>/CLAUDE.md`) and the
    /// project's (`CLAUDE.md`, `CLAUDE.local.md` in its root), those that exist, each without
    /// graphersal-memory's managed block.
    pub fn instructions_of(claude_md: &Path, project: &str) -> Vec<(String, String)> {
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
}

/// `text` without graphersal-memory's managed rule block (a broken block stays).
pub fn without_block(text: &str) -> String {
    match crate::install::find_block(text) {
        Ok(Some(block)) => format!("{}{}", &text[..block.start], &text[block.end..]),
        _ => text.to_string(),
    }
}

/// One arm's sandbox.
#[derive(Debug, Clone)]
pub struct Sandbox {
    /// The arm.
    pub arm: BenchArm,
    /// The working directory of its calls.
    pub cwd: PathBuf,
    /// The arguments its calls get after the harness command.
    pub args: Vec<String>,
    /// What the arm holds, for the report.
    pub description: String,
    /// A directory removed after the run (the store fork).
    pub scratch: Option<PathBuf>,
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

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    }
    fsutil::write_atomic(path, text.as_bytes())
}

fn pretty(value: &Value) -> String {
    format!("{}\n", crate::jsonedit::render(value, Some("  "), 0))
}

/// The instruction files as one section of the system prompt.
fn instruction_section(sources: &Sources) -> String {
    let mut out = String::new();
    for (label, text) in &sources.instructions {
        out.push_str(&format!(
            "# Instructions ({label})\n\n{}\n\n",
            text.trim_end()
        ));
    }
    out
}

/// Builds the sandbox of `arm` in `<arms_dir>/<arm>/`.
pub fn build(arm: BenchArm, arms_dir: &Path, sources: &Sources) -> Result<Sandbox, String> {
    let dir = arms_dir.join(arm.name());
    let cwd = dir.join("cwd");
    std::fs::create_dir_all(&cwd)
        .map_err(|err| format!("cannot create {}: {err}", cwd.display()))?;
    let settings_file = dir.join("settings.json");
    write(&settings_file, &pretty(&settings()))?;
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
            let fork = crate::store::default_store_dir(&data);
            graphersal::persist::Store::fork_dir(
                sources.store_dir.as_path(),
                graphersal::persist::RecoveryTarget::Latest,
                fork.as_path(),
            )
            .map_err(|err| {
                format!(
                    "cannot copy the memory store {} for the graph arm: {err}",
                    sources.store_dir.display()
                )
            })?;
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
                    "env": {crate::summaries::HOOK_ENV: "off"},
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
    write(&mcp_file, &pretty(&json!({"mcpServers": servers})))?;
    write(&system_file, &system)?;
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
    })
}

/// The sandbox of the generator and the judge: no tools, no MCP servers, no context, in
/// `<run dir>/<name>/`.
pub fn tool_free(dir: &Path) -> Result<(PathBuf, Vec<String>), String> {
    let cwd = dir.join("cwd");
    std::fs::create_dir_all(&cwd)
        .map_err(|err| format!("cannot create {}: {err}", cwd.display()))?;
    let settings_file = dir.join("settings.json");
    write(&settings_file, &pretty(&settings()))?;
    let mcp_file = dir.join("mcp.json");
    write(&mcp_file, &pretty(&json!({"mcpServers": {}})))?;
    let mut args = common_args(&settings_file, &mcp_file);
    args.extend(["--tools".to_string(), String::new()]);
    Ok((cwd, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_managed_block_is_left_out_of_the_instructions() {
        let block = format!(
            "{} fingerprint=x -->\nsecret rule\n{}\n",
            crate::install::BLOCK_BEGIN,
            crate::install::BLOCK_END
        );
        let text = format!("# Mine\n\nkeep this\n{block}after\n");
        let out = without_block(&text);
        assert_eq!(out, "# Mine\n\nkeep this\nafter\n");
        assert_eq!(without_block("plain\n"), "plain\n");
    }
}
