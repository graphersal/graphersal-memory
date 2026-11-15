//! opencode's part of the bench (`crate::bench::BenchSupport`): its sessions in its database,
//! its instruction files, and its sandboxes (`crate::bench::arms`).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use super::OpenCode;
use crate::bench::{BenchSupport, ToolFreeCall, arms, questions};
use crate::cli::BenchArm;

impl BenchSupport for OpenCode {
    fn excerpt(&self, project: &str, now: SystemTime) -> questions::Excerpt {
        questions::Excerpt::of_transcripts(
            &questions::opencode_transcripts(&self.paths.db, project, now),
            questions::EXCERPT_BUDGET,
        )
    }

    fn sessions_place(&self, _project: &str) -> (&'static str, PathBuf) {
        ("opencode session", self.paths.db.clone())
    }

    fn instructions(&self, project: &str) -> Vec<(String, String)> {
        arms::Sources::opencode_instructions_of(&self.paths, project)
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
        arms::build_opencode(arm, arms_dir, sources, &self.paths.db)
    }

    fn tool_free_call(
        &self,
        dir: &Path,
        mut command: Vec<String>,
        _schema: Value,
        _cap: f64,
    ) -> Result<ToolFreeCall, String> {
        let (cwd, env) = arms::tool_free_opencode(dir, &self.paths.db)?;
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
