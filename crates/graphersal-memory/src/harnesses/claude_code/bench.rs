//! Claude Code's part of the bench (`crate::bench::BenchSupport`): its transcripts, its
//! instruction files and memory files, and its sandboxes (`crate::bench::arms`).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use super::ClaudeCode;
use crate::bench::{BenchSupport, ToolFreeCall, arms, questions};
use crate::cli::BenchArm;

impl BenchSupport for ClaudeCode {
    fn excerpt(&self, project: &str, now: SystemTime) -> questions::Excerpt {
        let transcripts = questions::transcripts(&self.paths.projects, project, now);
        questions::Excerpt::of(&transcripts, questions::EXCERPT_BUDGET)
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
                let count = questions::transcripts(&self.paths.projects, &text, now).len();
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
        arms::Sources::instructions_of(&self.paths.instructions, project)
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
        arms::build(arm, arms_dir, sources)
    }

    /// `claude -p` with the sandbox's flags, the schema and the cost cap of the call.
    fn tool_free_call(
        &self,
        dir: &Path,
        mut command: Vec<String>,
        schema: Value,
        cap: f64,
    ) -> Result<ToolFreeCall, String> {
        let (cwd, args) = arms::tool_free(dir)?;
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
