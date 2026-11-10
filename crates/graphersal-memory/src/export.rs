//! The export of the graph's memories back into the harness's file memory: what `uninstall`
//! does before it removes anything (so switching the harness's own memory off at install loses
//! nothing), and `graphersal-memory export --to-files [--project ROOT]`.
//!
//! - Every memory of a project goes into that project's memory directory
//!   (`~/.claude/projects/<encoded root>/memory/`): one `<slug>.md` per memory with the front
//!   matter `name`, `description`, `metadata.type` and the body (its `**Why:**` /
//!   `**How to apply:**` tail included), and `MEMORY.md`, the index (`- [Title](slug.md) —
//!   hook`, in list order). This is the format `crate::import` reads, so importing an export
//!   gives the same memories back; links the body does not show as `[[slug]]` get a
//!   `Related: [[a]], [[b]]` line before the tail.
//! - The global memories (no project) go into one file every session of every project loads:
//!   the agent's global memory file (Claude Code: `~/.claude/rules/graphersal-memory-global.md`,
//!   a user rules file; not the managed block of `CLAUDE.md`, which uninstall removes). It is
//!   not imported again by `import` (rules files are not imported in this version).
//! - Conflicts: a file with exactly the text the export would write is left alone; a memory
//!   file that exists with other text (edited by the user or the agent) is kept, and the
//!   export's text goes beside it as `<slug>.graphersal.md`, named in the report. A `MEMORY.md`
//!   that differs is kept too, and only the index lines of the memories it does not link yet
//!   (`(<slug>.md)`) are appended to it, in the graph's order, in the format the import reads;
//!   its own lines stay byte for byte (user decision 2026-11-10). Nothing is ever deleted.
//! - A project is written to the directory it was last seen at (`crate::projects`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agents::Agent;

/// One memory as the export reads it from the graph (`export_memories`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Its slug.
    pub slug: String,
    /// Its project id, empty for a global memory.
    pub project: String,
    /// Its kind.
    pub kind: String,
    /// Its one-line summary.
    pub summary: String,
    /// Its body (the why / how-to-apply tail included).
    pub body: String,
    /// Its index title.
    pub title: String,
    /// Its index hook.
    pub hook: String,
    /// The slugs it relates to.
    pub links: Vec<String>,
}

impl Row {
    /// A row of the `export_memories` answer.
    pub fn from_json(value: &Value) -> Option<Self> {
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_string();
        let slug = text("slug");
        if slug.is_empty() {
            return None;
        }
        Some(Row {
            slug,
            project: text("project"),
            kind: text("kind"),
            summary: text("summary"),
            body: text("body"),
            title: text("title"),
            hook: text("hook"),
            links: value["links"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
        })
    }
}

/// The rows of an `export_memories` answer.
pub fn rows(answer: &Value) -> Vec<Row> {
    answer
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Row::from_json)
        .collect()
}

/// Where the memories come from: the daemon (`export_memories` control line), or a store in
/// the test process.
pub trait Source {
    /// Every memory (`project` empty) or those of one project and the global ones.
    fn memories(&mut self, project: &str) -> Result<Vec<Row>, String>;
}

/// A front-matter value: plain when it is safe, else double-quoted.
fn yaml_value(text: &str) -> String {
    let text = text.replace(['\n', '\r'], " ");
    let plain = !text.is_empty()
        && !text.starts_with([
            '"', '\'', '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '%',
            '@', '`', ' ',
        ])
        && !text.ends_with(' ')
        && !text.contains(": ")
        && !text.contains(" #");
    if plain {
        text
    } else {
        format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// Where a body's `**Why:**` / `**How to apply:**` tail starts (as `crate::import` splits it).
fn tail_start(body: &str) -> Option<usize> {
    const WHY: &str = "**Why:**";
    const HOW: &str = "**How to apply:**";
    if body.matches(WHY).count() > 1 || body.matches(HOW).count() > 1 {
        return None;
    }
    match (body.find(WHY), body.find(HOW)) {
        (Some(w), Some(h)) if w < h => Some(w),
        (Some(_), Some(_)) => None,
        (Some(w), None) => Some(w),
        (None, Some(h)) => Some(h),
        (None, None) => None,
    }
}

/// The text of one memory file.
pub fn file_text(row: &Row) -> String {
    let mut body = row.body.trim().to_string();
    let missing: Vec<String> = row
        .links
        .iter()
        .filter(|link| !body.contains(&format!("[[{link}]]")))
        .map(|link| format!("[[{link}]]"))
        .collect();
    if !missing.is_empty() {
        let line = format!("Related: {}", missing.join(", "));
        body = match tail_start(&body) {
            Some(at) => format!("{}\n\n{line}\n\n{}", body[..at].trim_end(), &body[at..]),
            None if body.is_empty() => line,
            None => format!("{body}\n\n{line}"),
        };
    }
    format!(
        "---\nname: {}\ndescription: {}\nmetadata:\n  type: {}\n---\n\n{body}\n",
        row.slug,
        yaml_value(&row.summary),
        row.kind
    )
}

/// The index line text: `[` and `]` in a title would end the link early.
fn index_title(row: &Row) -> String {
    let title = if row.title.is_empty() {
        &row.slug
    } else {
        &row.title
    };
    title.replace(['[', ']', '\n'], " ")
}

/// The `MEMORY.md` of a project's rows (in their order).
pub fn index_text(rows: &[&Row]) -> String {
    let mut text = String::new();
    for row in rows {
        let hook = if row.hook.is_empty() {
            &row.summary
        } else {
            &row.hook
        };
        text.push_str(&format!(
            "- [{}]({}.md) — {}\n",
            index_title(row),
            row.slug,
            hook.replace('\n', " ")
        ));
    }
    text
}

/// The text of the global memory file.
pub fn global_text(rows: &[&Row]) -> String {
    let mut text = String::from(
        "# Global memories\n\n\
         Exported by graphersal-memory (`uninstall` or `export --to-files`): memories that hold \
         in every project. Edit or delete them as you like; graphersal-memory does not read this \
         file back.\n",
    );
    for row in rows {
        text.push_str(&format!(
            "\n## {} ({})\n\n{}\n",
            index_title(row),
            row.kind,
            row.summary.replace('\n', " ")
        ));
        let body = row.body.trim();
        if !body.is_empty() && body != row.summary.trim() {
            text.push_str(&format!("\n{body}\n"));
        }
    }
    text
}

/// What the export did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Memories exported (written or found identical).
    pub memories: usize,
    /// Projects with at least one memory.
    pub projects: usize,
    /// Files written.
    pub written: Vec<PathBuf>,
    /// Files that already held the text (left alone).
    pub unchanged: usize,
    /// Files kept because they differ, with where the export's text went instead.
    pub conflicts: Vec<(PathBuf, PathBuf)>,
    /// `MEMORY.md` files kept with index lines appended, and how many.
    pub appended: Vec<(PathBuf, usize)>,
    /// Projects whose directory is not known (their memories were not written).
    pub problems: Vec<String>,
}

impl Report {
    /// The report's lines.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "exported {} memor{} of {} project{} (plus the global ones): {} file{} written, {} \
             unchanged",
            self.memories,
            if self.memories == 1 { "y" } else { "ies" },
            self.projects,
            if self.projects == 1 { "" } else { "s" },
            self.written.len(),
            if self.written.len() == 1 { "" } else { "s" },
            self.unchanged
        )];
        for path in &self.written {
            lines.push(format!("  wrote {}", path.display()));
        }
        for (kept, beside) in &self.conflicts {
            lines.push(format!(
                "  kept {} (it differs from the graph's version); the graph's version is in {}",
                kept.display(),
                beside.display()
            ));
        }
        for (index, count) in &self.appended {
            lines.push(format!(
                "  appended {count} index line{} to {} (its own lines are unchanged)",
                if *count == 1 { "" } else { "s" },
                index.display()
            ));
        }
        for problem in &self.problems {
            lines.push(format!("  not exported: {problem}"));
        }
        lines
    }
}

/// `path` with `.graphersal` before its extension (`a.md` -> `a.graphersal.md`).
fn beside(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    path.with_file_name(format!("{stem}.graphersal.md"))
}

/// Writes `text` to `path` by the conflict rules: identical -> nothing; missing -> written;
/// different -> kept, the text written beside it.
fn put(path: &Path, text: &str, report: &mut Report) -> Result<(), String> {
    match crate::fsutil::read_optional(path)? {
        Some(bytes) if bytes == text.as_bytes() => {
            report.unchanged += 1;
            Ok(())
        }
        Some(_) => {
            let other = beside(path);
            if crate::fsutil::read_optional(&other)?.as_deref() != Some(text.as_bytes()) {
                crate::fsutil::write_atomic(&other, text.as_bytes())?;
            }
            report.conflicts.push((path.to_path_buf(), other));
            Ok(())
        }
        None => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|err| format!("cannot create {}: {err}", parent.display()))?;
            }
            crate::fsutil::write_atomic(path, text.as_bytes())?;
            report.written.push(path.to_path_buf());
            Ok(())
        }
    }
}

/// Writes a project's index: a missing `MEMORY.md` gets the whole index; an identical one is
/// left alone; one that differs keeps every byte and gets the lines of the memories it does
/// not link yet (`](<slug>.md)`) appended (after a line end when its last line has none).
fn put_index(path: &Path, rows: &[&Row], report: &mut Report) -> Result<(), String> {
    let text = index_text(rows);
    let Some(bytes) = crate::fsutil::read_optional(path)? else {
        return put(path, &text, report);
    };
    if bytes == text.as_bytes() {
        report.unchanged += 1;
        return Ok(());
    }
    let existing = String::from_utf8_lossy(&bytes).into_owned();
    let missing: Vec<&Row> = rows
        .iter()
        .copied()
        .filter(|row| !existing.contains(&format!("]({}.md)", row.slug)))
        .collect();
    if missing.is_empty() {
        report.unchanged += 1;
        return Ok(());
    }
    let mut new = existing;
    if !new.is_empty() && !new.ends_with('\n') {
        new.push('\n');
    }
    new.push_str(&index_text(&missing));
    crate::fsutil::write_atomic(path, new.as_bytes())?;
    report.appended.push((path.to_path_buf(), missing.len()));
    Ok(())
}

/// The directory of the project `id`: the path it was last seen at that still exists, else
/// the last path, else the id itself (without a `#n` suffix).
pub fn project_root(data_dir: &Path, id: &str) -> PathBuf {
    let known = crate::projects::Projects::load(data_dir)
        .ok()
        .and_then(|projects| projects.get(id));
    if let Some(project) = known {
        if let Some(path) = project.paths.iter().find(|p| Path::new(p).is_dir()) {
            return PathBuf::from(path);
        }
        if let Some(path) = project.paths.first() {
            return PathBuf::from(path);
        }
    }
    let base = match id.rsplit_once('#') {
        Some((base, n)) if n.chars().all(|c| c.is_ascii_digit()) => base,
        _ => id,
    };
    PathBuf::from(base)
}

/// Writes `rows` into the agent's file memory.
pub fn write(agent: &dyn Agent, data_dir: &Path, rows: &[Row]) -> Result<Report, String> {
    let mut report = Report::default();
    let mut by_project: BTreeMap<&str, Vec<&Row>> = BTreeMap::new();
    for row in rows {
        by_project
            .entry(row.project.as_str())
            .or_default()
            .push(row);
    }
    for (project, rows) in &by_project {
        report.memories += rows.len();
        if project.is_empty() {
            put(&agent.global_memory_file(), &global_text(rows), &mut report)?;
            continue;
        }
        report.projects += 1;
        let root = project_root(data_dir, project);
        if !root.is_absolute() {
            report.problems.push(format!(
                "{} memories of project {project}: its directory is not known",
                rows.len()
            ));
            continue;
        }
        let dir = agent.memory_dir(&root);
        for row in rows {
            put(
                &dir.join(format!("{}.md", row.slug)),
                &file_text(row),
                &mut report,
            )?;
        }
        put_index(&dir.join("MEMORY.md"), rows, &mut report)?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(slug: &str, project: &str) -> Row {
        Row {
            slug: slug.into(),
            project: project.into(),
            kind: "feedback".into(),
            summary: "Use pnpm: never npm".into(),
            body: "Use pnpm.\n\n**Why:** speed.\n**How to apply:** always.".into(),
            title: "Package manager".into(),
            hook: "pnpm, not npm".into(),
            links: vec!["other".into()],
        }
    }

    #[test]
    fn a_file_carries_the_front_matter_the_import_reads_and_missing_links() {
        let text = file_text(&row("pnpm", "/w"));
        assert!(
            text.starts_with(
                "---\nname: pnpm\ndescription: \"Use pnpm: never npm\"\nmetadata:\n  type: feedback\n---\n\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("Use pnpm.\n\nRelated: [[other]]\n\n**Why:** speed."),
            "{text}"
        );
        assert_eq!(yaml_value("plain words"), "plain words");
        assert_eq!(yaml_value("- a list?"), "\"- a list?\"");
        let index = index_text(&[&row("pnpm", "/w")]);
        assert_eq!(index, "- [Package manager](pnpm.md) — pnpm, not npm\n");
    }

    #[test]
    fn conflicts_keep_the_existing_file_and_write_beside_it() {
        let dir = crate::mcp_tests::TempDir::new("export-put");
        let path = dir.path().join("a.md");
        let mut report = Report::default();
        put(&path, "one", &mut report).unwrap();
        put(&path, "one", &mut report).unwrap();
        assert_eq!((report.written.len(), report.unchanged), (1, 1));
        std::fs::write(&path, "theirs").unwrap();
        put(&path, "ours", &mut report).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "theirs");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.graphersal.md")).unwrap(),
            "ours"
        );
        assert_eq!(report.conflicts.len(), 1);
    }

    #[test]
    fn a_project_id_maps_to_its_last_directory() {
        let data = crate::mcp_tests::TempDir::new("export-root");
        assert_eq!(
            project_root(data.path(), "/w/app#2"),
            PathBuf::from("/w/app")
        );
        assert_eq!(project_root(data.path(), "/w/app"), PathBuf::from("/w/app"));
    }

    #[test]
    fn a_differing_index_gets_only_the_missing_lines_appended() {
        let dir = crate::mcp_tests::TempDir::new("export-index");
        let path = dir.path().join("MEMORY.md");
        // Theirs: one of our memories linked with their own wording, no final line end.
        let theirs = "# My index\n- [My pnpm note](pnpm.md) — mine";
        std::fs::write(&path, theirs).unwrap();
        let (a, b) = (row("pnpm", "/w"), row("tabs", "/w"));
        let mut report = Report::default();
        put_index(&path, &[&a, &b], &mut report).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            format!("{theirs}\n- [Package manager](tabs.md) — pnpm, not npm\n")
        );
        assert!(text.starts_with(theirs), "their lines stay byte for byte");
        assert_eq!(report.appended, vec![(path.clone(), 1)]);
        assert!(!dir.path().join("MEMORY.graphersal.md").exists());
        // A second export appends nothing.
        let mut again = Report::default();
        put_index(&path, &[&a, &b], &mut again).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        assert!(again.appended.is_empty());
        assert_eq!(again.unchanged, 1);
        // A missing index is written whole.
        let fresh = dir.path().join("new").join("MEMORY.md");
        let mut report = Report::default();
        put_index(&fresh, &[&a], &mut report).unwrap();
        assert_eq!(report.written, vec![fresh]);
    }
}
