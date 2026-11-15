//! Codex's part of the bench (`crate::bench::BenchSupport`): its rollouts, its instruction
//! files, and its sandboxes (`crate::bench::arms`).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use super::Codex;
use crate::bench::{BenchSupport, ToolFreeCall, arms, questions};
use crate::cli::BenchArm;

impl BenchSupport for Codex {
    fn excerpt(&self, project: &str, now: SystemTime) -> questions::Excerpt {
        questions::Excerpt::of_transcripts(
            &questions::codex_transcripts(&self.paths.sessions, project, now),
            questions::EXCERPT_BUDGET,
        )
    }

    fn sessions_place(&self, _project: &str) -> (&'static str, PathBuf) {
        ("Codex session", self.paths.sessions.clone())
    }

    fn instructions(&self, project: &str) -> Vec<(String, String)> {
        arms::Sources::codex_instructions_of(&self.paths, project)
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
        arms::build_codex(arm, arms_dir, sources, &self.paths)
    }

    fn tool_free_call(
        &self,
        dir: &Path,
        command: Vec<String>,
        _schema: Value,
        _cap: f64,
    ) -> Result<ToolFreeCall, String> {
        let (cwd, env) = arms::tool_free_codex(dir, &self.paths)?;
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
