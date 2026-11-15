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
//!
//! opencode ([`build_opencode`]): the sandbox is a configuration and a data directory of the
//! arm's own (`XDG_CONFIG_HOME`/`XDG_DATA_HOME`, `crate::harnesses::opencode::build_sandbox`), so none of
//! the user's configuration, plugins, rules, MCP servers or sessions are loaded and its runs are
//! not stored in the user's session database; `config/opencode/opencode.json` holds the arm's
//! MCP servers, its permissions (everything denied but the arm's tools) and `instructions`
//! naming `system.md`; the user's logins (`auth.json`) are copied, never changed.
//!
//! Codex ([`build_codex`]): the sandbox is a `CODEX_HOME` of the arm's own
//! (`crate::harnesses::codex::build_sandbox`): its `config.toml` holds the user's model settings, no hooks,
//! no Codex memories, no history, and every built-in tool switched off (shell, unified exec,
//! image viewing, web search, sub-agents, apps, plugins), so an arm sees nothing but its context
//! and, for the graph arm, the read-only memory functions of a server over a FORK of the store
//! (`enabled_tools`, approved without asking); its `AGENTS.md` is the arm's context; runs are
//! `--ephemeral` (no rollout in the user's sessions); the user's login is copied, never changed.

use std::path::{Path, PathBuf};

use serde_json::Value;

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

/// `text` without graphersal-memory's managed rule block (a broken block stays).
pub fn without_block(text: &str) -> String {
    match crate::install::find_block_in(text, "", &[], &[]) {
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
    /// The environment of its calls (an opencode sandbox's directories).
    pub env: Vec<(String, String)>,
}

pub fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    }
    fsutil::write_atomic(path, text.as_bytes())
}

pub fn pretty(value: &Value) -> String {
    format!(
        "{}\n",
        crate::install::jsonedit::render(value, Some("  "), 0)
    )
}

/// The instruction files as one section of the system prompt.
pub fn instruction_section(sources: &Sources) -> String {
    let mut out = String::new();
    for (label, text) in &sources.instructions {
        out.push_str(&format!(
            "# Instructions ({label})\n\n{}\n\n",
            text.trim_end()
        ));
    }
    out
}

/// Copies the live store in `store_dir` into `fork` (`Store::fork_dir`, read without the
/// store's lock while the daemon serves it), holding the store's copy lock so the daemon does
/// not prune files the copy still reads (`crate::runtime::maintenance::hold_for_copy`).
pub(crate) fn fork_store(store_dir: &Path, fork: &Path) -> Result<(), String> {
    crate::runtime::maintenance::hold_for_copy(store_dir, || {
        graphersal::persist::Store::fork_dir(
            store_dir,
            graphersal::persist::RecoveryTarget::Latest,
            fork,
        )
        .map(|_| ())
        .map_err(|err| {
            format!(
                "cannot copy the memory store {} for the graph arm: {err}",
                store_dir.display()
            )
        })
    })
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

    #[test]
    fn only_opencode_1x_falls_back_to_claude_md() {
        use crate::harnesses::opencode::{Major, OpenCodePaths};
        let home = crate::runtime::mcp_tests::TempDir::new("arms-oc");
        let project = home.path().join("work");
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(home.path().join(".claude/CLAUDE.md"), "claude rules\n").unwrap();
        std::fs::write(project.join("AGENTS.md"), "project rules\n").unwrap();
        let project = project.display().to_string();
        let v1 = OpenCodePaths::of_home_for(home.path(), Major::V1);
        let labels =
            |found: Vec<(String, String)>| found.into_iter().map(|(l, _)| l).collect::<Vec<_>>();
        assert_eq!(
            labels(crate::harnesses::opencode::bench::instructions(
                &v1, &project
            )),
            vec!["global AGENTS.md", "project AGENTS.md"]
        );
        let v2 = OpenCodePaths::of_home_for(home.path(), Major::V2);
        assert_eq!(
            crate::harnesses::opencode::bench::instructions(&v2, &project),
            vec![(
                "project AGENTS.md".to_string(),
                "project rules\n".to_string()
            )]
        );
    }
}
