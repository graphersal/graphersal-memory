//! The start context: the memories that always apply, put into the start of every session
//! (decision D7, the rule export from the graph into the harness).
//!
//! - **Which**: every pinned memory (any kind) and every `feedback` and `user` memory of the
//!   session's project and the global ones (the internal catalog function `start_context`).
//! - **Order** ([`sort`]): pinned first, then feedback, then user; within each by `order`
//!   (lower first; memories without one after the ordered ones), then newest first, then slug.
//!   `memories()` without text lists them in the same order, then the other memories by
//!   `order`, then list position (`catalog/memories.rhai`).
//! - **Text** ([`render`]): grouped like Claude Code's `MEMORY.md` ("How to work" = feedback,
//!   "About you" = user, "Pinned" = the other pinned ones), one line per memory
//!   `- <title> — <hook or summary> (memory: <slug>)`, then one line naming `memory(slug)` and
//!   `memories(text)`. A budget in characters (`memory.start_context_chars` of `config.json`,
//!   default [`DEFAULT_CHARS`], `0` = off): over it, the memories are kept in the order above
//!   and the text ends with "N more not shown: memories(kind: ...)".
//! - **Conflicts** (M12): a superseded memory never appears (the catalog function leaves it
//!   out); the open conflicts between the project's and the global memories follow the groups,
//!   both versions with their files (`open_conflicts`, at most [`MAX_CONFLICTS`]), and a line
//!   counts the imported memories waiting for their conflict check, both naming `conflicts()`;
//!   then the memories that contradict a line of an instruction file the harness loads (the
//!   memory and the file's line, at most [`MAX_CONFLICTS`]) and one line counting the memories
//!   near such a line not yet checked (`crate::instructions`, [`render_all`]).
//! - **Delivery**: the session-start hooks print it (Claude Code: `additionalContext`; Codex:
//!   the same JSON; Cursor: `additional_context`), and the `use` server's instructions of a
//!   connection carry it, unless the connection says its harness's hook delivers it
//!   (`crate::protocol`'s `start_context` handshake field). `graphersal-memory start-context`
//!   prints it.

use serde_json::Value;

/// The default budget of the start context, in characters.
pub const DEFAULT_CHARS: usize = 8000;

/// The longest hook or summary shown on one line, in characters.
const MAX_HOOK_CHARS: usize = 300;

/// One always-loaded memory, as the catalog function `start_context` returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The memory's slug.
    pub slug: String,
    /// `project` or `global`.
    pub scope: String,
    /// Its kind.
    pub kind: String,
    /// Whether it is pinned.
    pub pinned: bool,
    /// Its priority within its group (lower first), when it has one.
    pub order: Option<i64>,
    /// Its title (the slug when it has none).
    pub title: String,
    /// Its hook (the summary when it has none).
    pub hook: String,
    /// When it was last written (RFC 3339 text; compared as text).
    pub updated_at: String,
}

/// The entries of the catalog function's answer (rows that do not parse are skipped).
pub fn entries(value: &Value) -> Vec<Entry> {
    let text = |row: &Value, key: &str| row[key].as_str().unwrap_or_default().to_string();
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["slug"].is_string())
        .map(|row| Entry {
            slug: text(row, "slug"),
            scope: text(row, "scope"),
            kind: text(row, "kind"),
            pinned: row["pinned"] == Value::Bool(true),
            order: match &row["order"] {
                Value::Array(values) => values.first().and_then(Value::as_i64),
                other => other.as_i64(),
            },
            title: text(row, "title"),
            hook: text(row, "hook"),
            updated_at: text(row, "updated_at"),
        })
        .collect()
}

/// The most open conflicts the text shows (the rest: "N more").
pub const MAX_CONFLICTS: usize = 5;

/// The longest hook or summary of a memory in a conflict line, in characters.
const MAX_CONFLICT_HOOK_CHARS: usize = 160;

/// One memory of an open conflict, as the catalog function `open_conflicts` returns it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Side {
    /// The memory's slug.
    pub slug: String,
    /// `project` or `global`.
    pub scope: String,
    /// Its title (the slug when it has none).
    pub title: String,
    /// Its hook (the summary when it has none).
    pub hook: String,
    /// Whether it is always loaded (pinned, feedback or user).
    pub always: bool,
    /// The file it was imported from (empty: written through `remember`).
    pub file: String,
}

/// An open conflict between two memories.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Conflict {
    /// One memory.
    pub a: Side,
    /// The memory it contradicts.
    pub b: Side,
}

impl Conflict {
    /// The `status` line: both slugs with their sources.
    pub fn status_line(&self) -> String {
        let source = |side: &Side| {
            if side.file.is_empty() {
                format!("{} (remember)", side.slug)
            } else {
                format!("{} (file {})", side.slug, side.file)
            }
        };
        format!("{} vs {}", source(&self.a), source(&self.b))
    }
}

/// A memory that contradicts a line of an instruction file the harness loads
/// (`crate::instructions`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleConflict {
    /// The memory.
    pub memory: Side,
    /// The instruction file.
    pub file: String,
    /// The line of the rule in it.
    pub line: u64,
    /// The rule's text.
    pub text: String,
}

impl RuleConflict {
    /// The `status` line: the memory with its source, and the file's line.
    pub fn status_line(&self) -> String {
        let memory = if self.memory.file.is_empty() {
            format!("{} (remember)", self.memory.slug)
        } else {
            format!("{} (file {})", self.memory.slug, self.memory.file)
        };
        format!("{memory} vs {}:{}", self.file, self.line)
    }
}

/// The open conflicts between memories and lines of instruction files, and how many such
/// candidates wait for their check, of the catalog function `open_conflicts` (none from an
/// older daemon).
pub fn rule_conflicts(value: &Value) -> (Vec<RuleConflict>, usize) {
    let open = value["rules_open"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["memory"]["slug"].is_string())
        .map(|row| RuleConflict {
            memory: side_of(&row["memory"]),
            file: row["file"].as_str().unwrap_or_default().to_string(),
            line: row["line"].as_u64().unwrap_or(0),
            text: row["text"].as_str().unwrap_or_default().to_string(),
        })
        .collect();
    let unchecked = value["rules_unchecked"].as_u64().unwrap_or(0) as usize;
    (open, unchecked)
}

/// One memory of an `open_conflicts` row.
fn side_of(row: &Value) -> Side {
    Side {
        slug: row["slug"].as_str().unwrap_or_default().to_string(),
        scope: row["scope"].as_str().unwrap_or_default().to_string(),
        title: row["title"].as_str().unwrap_or_default().to_string(),
        hook: row["hook"].as_str().unwrap_or_default().to_string(),
        always: row["always"] == Value::Bool(true),
        file: row["file"].as_str().unwrap_or_default().to_string(),
    }
}

/// The open conflicts and the unchecked count of the catalog function `open_conflicts`.
pub fn conflicts(value: &Value) -> (Vec<Conflict>, usize) {
    let side = side_of;
    let open = value["open"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["a"]["slug"].is_string() && row["b"]["slug"].is_string())
        .map(|row| Conflict {
            a: side(&row["a"]),
            b: side(&row["b"]),
        })
        .collect();
    let unchecked = value["unchecked"].as_u64().unwrap_or(0) as usize;
    (open, unchecked)
}

/// The group of a memory in the selection order: pinned 0, feedback 1, user 2, the rest 3.
fn rank(entry: &Entry) -> u8 {
    if entry.pinned {
        0
    } else {
        match entry.kind.as_str() {
            "feedback" => 1,
            "user" => 2,
            _ => 3,
        }
    }
}

/// Sorts `entries` in the selection order: pinned, feedback, user; each by `order` (absent
/// after the ordered ones), then newest first, then slug.
pub fn sort(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| match (a.order, b.order) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.slug.cmp(&b.slug))
    });
}

/// The start context of a project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartContext {
    /// The text (empty when there is no always-loaded memory, or the budget is 0).
    pub text: String,
    /// The memories shown.
    pub shown: usize,
    /// The always-loaded memories in all.
    pub total: usize,
    /// The budget it was cut to, in characters.
    pub budget: usize,
    /// The open conflicts, one line each naming both memories and their files.
    pub conflicts: Vec<String>,
    /// How many imported memories wait for their conflict check.
    pub unchecked: usize,
    /// The open conflicts between memories and lines of instruction files, one line each.
    pub rule_conflicts: Vec<String>,
    /// How many candidates (a memory near a line of an instruction file) wait for their check.
    pub rules_unchecked: usize,
}

impl StartContext {
    /// The memories not shown (over the budget).
    pub fn not_shown(&self) -> usize {
        self.total - self.shown
    }

    /// The text's length in characters.
    pub fn chars(&self) -> usize {
        self.text.chars().count()
    }

    /// The `status`/`doctor` line: "start context: N memories, X of Y characters (Z not
    /// shown)".
    pub fn status_line(&self) -> String {
        if self.budget == 0 {
            return format!(
                "start context: off (memory.start_context_chars is 0; {} always-loaded memories)",
                self.total
            );
        }
        format!(
            "start context: {} memor{}, {} of {} characters{}",
            self.shown,
            if self.shown == 1 { "y" } else { "ies" },
            self.chars(),
            self.budget,
            if self.not_shown() > 0 {
                format!(" ({} not shown)", self.not_shown())
            } else {
                String::new()
            }
        )
    }

    /// The `status`/`doctor` lines about conflicts (none when there is nothing to settle).
    pub fn conflict_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if !self.conflicts.is_empty() {
            lines.push(format!(
                "memory conflicts: {} open; ask your agent to list and settle them (conflicts())",
                self.conflicts.len()
            ));
            lines.extend(self.conflicts.iter().map(|line| format!("  {line}")));
        }
        if self.unchecked > 0 {
            lines.push(format!(
                "memory conflicts: {} imported memor{} not yet checked against the others \
                 (your agent: conflicts())",
                self.unchecked,
                if self.unchecked == 1 { "y" } else { "ies" }
            ));
        }
        if !self.rule_conflicts.is_empty() {
            lines.push(format!(
                "memory conflicts: {} memor{} contradict{} a line of your instruction files; \
                 ask your agent which holds (conflicts()), or edit the file",
                self.rule_conflicts.len(),
                if self.rule_conflicts.len() == 1 { "y" } else { "ies" },
                if self.rule_conflicts.len() == 1 { "s" } else { "" }
            ));
            lines.extend(self.rule_conflicts.iter().map(|line| format!("  {line}")));
        }
        if self.rules_unchecked > 0 {
            lines.push(format!(
                "memory conflicts: {} memor{} near a line of your instruction files not yet \
                 checked (your agent: conflicts())",
                self.rules_unchecked,
                if self.rules_unchecked == 1 { "y" } else { "ies" }
            ));
        }
        lines
    }

    /// The sentence the session-start line adds about conflicts (empty when there are none).
    pub fn start_note(&self) -> String {
        let mut parts = Vec::new();
        if !self.conflicts.is_empty() {
            parts.push(format!(
                "{} open memory conflict{} (ask the user which memory holds, then settle it)",
                self.conflicts.len(),
                if self.conflicts.len() == 1 { "" } else { "s" }
            ));
        }
        if self.unchecked > 0 {
            parts.push(format!(
                "{} imported memor{} to check",
                self.unchecked,
                if self.unchecked == 1 { "y" } else { "ies" }
            ));
        }
        if !self.rule_conflicts.is_empty() {
            parts.push(format!(
                "{} memor{} contradict{} your instruction files",
                self.rule_conflicts.len(),
                if self.rule_conflicts.len() == 1 { "y" } else { "ies" },
                if self.rule_conflicts.len() == 1 { "s" } else { "" }
            ));
        }
        if self.rules_unchecked > 0 {
            parts.push(format!(
                "{} memor{} to check against your instruction files",
                self.rules_unchecked,
                if self.rules_unchecked == 1 { "y" } else { "ies" }
            ));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!(" {}: conflicts().", parts.join("; "))
        }
    }

    /// The control-line answer of the daemon (`start_context`).
    pub fn to_json(&self) -> Value {
        serde_json::json!({"ok": true, "text": self.text, "shown": self.shown,
                           "total": self.total, "chars": self.chars(), "budget": self.budget,
                           "conflicts": self.conflicts, "unchecked": self.unchecked,
                           "rule_conflicts": self.rule_conflicts,
                           "rules_unchecked": self.rules_unchecked})
    }

    /// The daemon's answer read back (`None` when it is not one).
    pub fn from_json(value: &Value) -> Option<Self> {
        let count = |key: &str| value[key].as_u64().map(|n| n as usize);
        Some(Self {
            text: value["text"].as_str()?.to_string(),
            shown: count("shown")?,
            total: count("total")?,
            budget: count("budget")?,
            // An older daemon's answer has neither.
            conflicts: value["conflicts"]
                .as_array()
                .map(|lines| {
                    lines
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            unchecked: count("unchecked").unwrap_or(0),
            rule_conflicts: value["rule_conflicts"]
                .as_array()
                .map(|lines| {
                    lines
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            rules_unchecked: count("rules_unchecked").unwrap_or(0),
        })
    }
}

/// The first line of the text.
const HEADLINE: &str = "graphersal-memory: the memories that always apply (pinned, how the user \
                        wants you to work, about the user), loaded at the start of every session.";

/// The line after the memories.
const FOOTER: &str = "Each line is one memory: memory(slug) gives its full text; search the rest \
                      with memories(text) or recall(text).";

/// Whether an entry belongs to a group of the text.
type Belongs = fn(&Entry) -> bool;

/// The groups of the text, in order: heading and which entries belong to it.
const GROUPS: [(&str, Belongs); 3] = [
    ("## How to work", |e| e.kind == "feedback"),
    ("## About you", |e| e.kind == "user"),
    ("## Pinned", |e| e.kind != "feedback" && e.kind != "user"),
];

/// One memory's line.
fn line(entry: &Entry) -> String {
    let title = one_line(if entry.title.is_empty() {
        &entry.slug
    } else {
        &entry.title
    });
    let hook = one_line(&entry.hook);
    let hook: String = if shortened(entry) {
        let cut: String = hook.chars().take(SHORTENED_HOOK_CHARS).collect();
        format!("{}…", cut.trim_end())
    } else {
        hook
    };
    if hook.is_empty() {
        format!("- {title} (memory: {})", entry.slug)
    } else {
        format!("- {title} — {hook} (memory: {})", entry.slug)
    }
}

/// How many characters of a shortened hook its line keeps (then "…").
const SHORTENED_HOOK_CHARS: usize = MAX_HOOK_CHARS - 1;

/// Whether the line of `entry` cuts its hook (or summary) at [`MAX_HOOK_CHARS`].
fn shortened(entry: &Entry) -> bool {
    one_line(&entry.hook).chars().count() > MAX_HOOK_CHARS
}

/// `text` on one line (line breaks and runs of white space become one space).
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The heading of the open conflicts.
const CONFLICTS_HEADING: &str = "## Open conflicts (two memories contradict: ask the user which \
                                 holds, then settle it; conflicts() lists them)";

/// One memory of a conflict line.
fn side_text(side: &Side) -> String {
    let title = one_line(if side.title.is_empty() {
        &side.slug
    } else {
        &side.title
    });
    let hook = one_line(&side.hook);
    let hook: String = if hook.chars().count() > MAX_CONFLICT_HOOK_CHARS {
        let cut: String = hook.chars().take(MAX_CONFLICT_HOOK_CHARS - 1).collect();
        format!("{}…", cut.trim_end())
    } else {
        hook
    };
    let mut source = format!("memory: {}", side.slug);
    if side.scope == "global" {
        source.push_str(", global");
    }
    if side.always {
        source.push_str(", always loaded");
    }
    if !side.file.is_empty() {
        source.push_str(&format!(", file: {}", side.file));
    }
    if hook.is_empty() {
        format!("{title} ({source})")
    } else {
        format!("{title} — {hook} ({source})")
    }
}

/// The heading of the conflicts between memories and lines of instruction files.
const RULE_CONFLICTS_HEADING: &str = "## Memories that contradict your instruction files (ask the \
                                      user which holds: forget the memory, or the user edits the \
                                      file; conflicts() lists them)";

/// The conflict part of the text: the open conflicts (both versions), the unchecked count, the
/// memories that contradict lines of instruction files and how many such candidates wait for
/// their check; empty when there is none of them.
fn conflicts_text(
    conflicts: &[Conflict],
    unchecked: usize,
    rules: &[RuleConflict],
    rules_unchecked: usize,
) -> String {
    let mut out = String::new();
    if !conflicts.is_empty() {
        out.push_str(CONFLICTS_HEADING);
        for conflict in conflicts.iter().take(MAX_CONFLICTS) {
            out.push_str(&format!(
                "\n- {}\n  CONFLICTS WITH {}",
                side_text(&conflict.a),
                side_text(&conflict.b)
            ));
        }
        if conflicts.len() > MAX_CONFLICTS {
            out.push_str(&format!(
                "\n{} more: conflicts().",
                conflicts.len() - MAX_CONFLICTS
            ));
        }
    }
    if unchecked > 0 {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "{unchecked} imported memor{} not yet checked for conflicts with the others: \
             conflicts() lists {}.",
            if unchecked == 1 { "y is" } else { "ies are" },
            if unchecked == 1 { "it" } else { "them" }
        ));
    }
    if !rules.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(RULE_CONFLICTS_HEADING);
        for rule in rules.iter().take(MAX_CONFLICTS) {
            let text = one_line(&rule.text);
            let text: String = if text.chars().count() > MAX_CONFLICT_HOOK_CHARS {
                let cut: String = text.chars().take(MAX_CONFLICT_HOOK_CHARS - 1).collect();
                format!("{}…", cut.trim_end())
            } else {
                text
            };
            out.push_str(&format!(
                "\n- {}\n  CONTRADICTS {} line {}: {text}",
                side_text(&rule.memory),
                rule.file,
                rule.line
            ));
        }
        if rules.len() > MAX_CONFLICTS {
            out.push_str(&format!("\n{} more: conflicts().", rules.len() - MAX_CONFLICTS));
        }
    }
    if rules_unchecked > 0 {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "{rules_unchecked} memor{} near lines of your instruction files {} not yet checked \
             for a contradiction: conflicts() lists {}.",
            if rules_unchecked == 1 { "y" } else { "ies" },
            if rules_unchecked == 1 { "is" } else { "are" },
            if rules_unchecked == 1 { "it" } else { "them" }
        ));
    }
    out
}

/// The text of `shown` (in selection order) with the "more" line for `hidden` and the
/// conflict part `conflicts` (may be empty).
fn text_of(shown: &[&Entry], hidden: &[&Entry], conflicts: &str) -> String {
    let mut out = String::from(HEADLINE);
    for (heading, belongs) in GROUPS {
        let lines: Vec<String> = shown
            .iter()
            .filter(|e| belongs(e))
            .map(|e| line(e))
            .collect();
        if lines.is_empty() {
            continue;
        }
        out.push_str("\n\n");
        out.push_str(heading);
        for line in lines {
            out.push('\n');
            out.push_str(&line);
        }
    }
    if !conflicts.is_empty() {
        out.push_str("\n\n");
        out.push_str(conflicts);
    }
    out.push_str("\n\n");
    out.push_str(FOOTER);
    if !hidden.is_empty() {
        let mut kinds: Vec<&str> = Vec::new();
        for kind in ["feedback", "user", "project", "reference"] {
            if hidden.iter().any(|e| e.kind == kind) {
                kinds.push(kind);
            }
        }
        for e in hidden {
            if !kinds.contains(&e.kind.as_str()) {
                kinds.push(&e.kind);
            }
        }
        let calls: Vec<String> = kinds
            .iter()
            .map(|kind| format!("memories(kind: \"{kind}\")"))
            .collect();
        out.push_str(&format!(
            "\n{} more not shown: {}.",
            hidden.len(),
            calls.join(", ")
        ));
    }
    out
}

/// The start context of `entries` within `budget` characters: the memories in selection order
/// ([`sort`]) as long as the whole text fits, the rest named by the "more" line. Empty when
/// there is no entry or the budget is 0.
pub fn render(entries: Vec<Entry>, budget: usize) -> StartContext {
    render_with(entries, Vec::new(), 0, budget)
}

/// [`render`] with the open `conflicts` and the `unchecked` count after the groups (the text
/// is empty only when there is neither and no entry, or the budget is 0).
pub fn render_with(
    entries: Vec<Entry>,
    conflicts: Vec<Conflict>,
    unchecked: usize,
    budget: usize,
) -> StartContext {
    render_all(entries, conflicts, unchecked, (Vec::new(), 0), budget)
}

/// [`render_with`] with the memories that contradict lines of instruction files and how many
/// such candidates wait for their check (`rules`, from [`rule_conflicts`]) after the conflicts.
pub fn render_all(
    mut entries: Vec<Entry>,
    conflicts: Vec<Conflict>,
    unchecked: usize,
    rules: (Vec<RuleConflict>, usize),
    budget: usize,
) -> StartContext {
    sort(&mut entries);
    let total = entries.len();
    let (rules, rules_unchecked) = rules;
    let lines: Vec<String> = conflicts.iter().map(Conflict::status_line).collect();
    let rule_lines: Vec<String> = rules.iter().map(RuleConflict::status_line).collect();
    if (total == 0
        && conflicts.is_empty()
        && unchecked == 0
        && rules.is_empty()
        && rules_unchecked == 0)
        || budget == 0
    {
        return StartContext {
            text: String::new(),
            shown: 0,
            total,
            budget,
            conflicts: lines,
            unchecked,
            rule_conflicts: rule_lines,
            rules_unchecked,
        };
    }
    let conflict_text = conflicts_text(&conflicts, unchecked, &rules, rules_unchecked);
    let all: Vec<&Entry> = entries.iter().collect();
    // The longest prefix whose text fits (the text grows with the prefix, so a binary search
    // finds it; the "more" line shrinks by at most a few characters as the prefix grows, which
    // the final check covers).
    let fits = |n: usize| {
        text_of(&all[..n], &all[n..], &conflict_text)
            .chars()
            .count()
            <= budget
    };
    let (mut low, mut high) = (0, total);
    while low < high {
        let mid = (low + high).div_ceil(2);
        if fits(mid) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let mut shown = low;
    while shown > 0 && !fits(shown) {
        shown -= 1;
    }
    StartContext {
        text: text_of(&all[..shown], &all[shown..], &conflict_text),
        shown,
        total,
        budget,
        conflicts: lines,
        unchecked,
        rule_conflicts: rule_lines,
        rules_unchecked,
    }
}

/// The always-loaded memories of `entries` in the selection order ([`sort`]) as the browser UI
/// shows them: each with its `tier` (`pinned`, `feedback`, `user`: the order groups `set_order`
/// renumbers), its `group` in the text (the heading it is listed under) and whether the text
/// shows it (the first `shown` ones) or cuts it for lack of room, and whether its line is
/// `shortened` (its hook cut at [`MAX_HOOK_CHARS`]: the line keeps `shown_hook_chars`
/// characters, the rest only through `memory(slug)`).
pub fn entries_json(mut entries: Vec<Entry>, shown: usize) -> Value {
    sort(&mut entries);
    let rows: Vec<Value> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let tier = match rank(e) {
                0 => "pinned",
                1 => "feedback",
                2 => "user",
                _ => "other",
            };
            let group = GROUPS
                .iter()
                .find(|(_, belongs)| belongs(e))
                .map_or("", |(heading, _)| heading.trim_start_matches("## "));
            serde_json::json!({"slug": e.slug, "scope": e.scope, "kind": e.kind,
                               "pinned": e.pinned, "order": e.order, "title": e.title,
                               "hook": e.hook, "updated_at": e.updated_at, "tier": tier,
                               "group": group, "shown": i < shown, "line": line(e),
                               "shortened": shortened(e),
                               "shown_hook_chars": SHORTENED_HOOK_CHARS})
        })
        .collect();
    Value::Array(rows)
}

/// The control line asking the daemon for the start context of the project root `project`
/// (`""`: the global memories only), at most `max_chars` characters when given.
pub fn control_line(project: &str, max_chars: Option<usize>) -> Value {
    let mut line = serde_json::json!({"protocol": crate::protocol::PROTOCOL,
                                      "control": "start_context", "project": project});
    if let Some(max) = max_chars {
        line["max_chars"] = Value::from(max as u64);
    }
    line
}

/// The start context out of the daemon's answer, or its refusal as the error.
pub fn from_answer(answer: &Value) -> Result<StartContext, String> {
    if answer["ok"] != Value::Bool(true) {
        return Err(answer["error"]["message"]
            .as_str()
            .unwrap_or("the daemon refused the start context")
            .to_string());
    }
    StartContext::from_json(answer)
        .ok_or_else(|| "the daemon's start context answer is incomplete".to_string())
}

/// Asks the daemon (starting it when none runs) for the start context of `project`.
pub fn request(
    location: &crate::shim::DaemonLocation,
    project: &str,
    max_chars: Option<usize>,
) -> Result<StartContext, String> {
    from_answer(&crate::shim::request(
        location,
        &control_line(project, max_chars),
    )?)
}

/// Asks a running daemon for the start context of `project`; `None` when no daemon runs.
pub fn request_running(
    run_dir: &std::path::Path,
    project: &str,
) -> Result<Option<StartContext>, String> {
    match crate::shim::request_running(run_dir, &control_line(project, None))? {
        Some(answer) => from_answer(&answer).map(Some),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(slug: &str, kind: &str, pinned: bool, order: Option<i64>, at: &str) -> Entry {
        Entry {
            slug: slug.into(),
            scope: "global".into(),
            kind: kind.into(),
            pinned,
            order,
            title: format!("Title {slug}"),
            hook: format!("hook of {slug}"),
            updated_at: at.into(),
        }
    }

    #[test]
    fn selection_order_is_pinned_feedback_user_then_order_then_newest() {
        let mut list = vec![
            entry("u-old", "user", false, None, "2026-01-01"),
            entry("f-new", "feedback", false, None, "2026-03-01"),
            entry("f-ordered", "feedback", false, Some(5), "2026-01-01"),
            entry("p-project", "project", true, None, "2026-01-01"),
            entry("f-first", "feedback", false, Some(1), "2025-01-01"),
            entry("f-old", "feedback", false, None, "2026-02-01"),
            entry("u-new", "user", false, None, "2026-05-01"),
            entry("p-feedback", "feedback", true, Some(2), "2026-01-01"),
        ];
        sort(&mut list);
        let slugs: Vec<&str> = list.iter().map(|e| e.slug.as_str()).collect();
        assert_eq!(
            slugs,
            [
                "p-feedback",
                "p-project",
                "f-first",
                "f-ordered",
                "f-new",
                "f-old",
                "u-new",
                "u-old"
            ]
        );
    }

    #[test]
    fn the_text_groups_like_memory_md_with_one_line_per_memory() {
        let mut pinned_feedback = entry("tests-approval", "feedback", true, None, "2026-01-01");
        pinned_feedback.title = "Existing tests need approval".into();
        pinned_feedback.hook = "add tests freely;\nchanging one needs approval".into();
        let list = vec![
            entry("who", "user", false, None, "2026-01-01"),
            pinned_feedback,
            entry("semver", "project", true, None, "2026-01-01"),
        ];
        let context = render(list, DEFAULT_CHARS);
        assert_eq!(
            context.text,
            format!(
                "{HEADLINE}\n\n## How to work\n- Existing tests need approval — add tests \
                 freely; changing one needs approval (memory: tests-approval)\n\n## About you\n\
                 - Title who — hook of who (memory: who)\n\n## Pinned\n- Title semver — hook of \
                 semver (memory: semver)\n\n{FOOTER}"
            )
        );
        assert_eq!(
            (context.shown, context.total, context.not_shown()),
            (3, 3, 0)
        );
        assert_eq!(
            context.status_line(),
            format!(
                "start context: 3 memories, {} of 8000 characters",
                context.chars()
            )
        );
    }

    #[test]
    fn over_the_budget_the_order_decides_and_a_more_line_names_the_rest() {
        let mut list = vec![entry("pin", "reference", true, None, "2026-01-01")];
        for i in 0..40 {
            list.push(entry(
                &format!("feedback-{i:02}"),
                "feedback",
                false,
                Some(i),
                "2026",
            ));
        }
        for i in 0..40 {
            list.push(entry(
                &format!("user-{i:02}"),
                "user",
                false,
                Some(i),
                "2026",
            ));
        }
        let budget = 1500;
        let context = render(list, budget);
        assert!(context.chars() <= budget, "{}", context.chars());
        assert!(context.shown > 1 && context.shown < 81, "{}", context.shown);
        assert!(context.text.contains("(memory: pin)"), "pinned first");
        assert!(context.text.contains("(memory: feedback-00)"));
        assert!(
            !context.text.contains("(memory: user-00)"),
            "user after feedback"
        );
        let hidden = 81 - context.shown;
        assert!(
            context.text.ends_with(&format!(
                "\n{hidden} more not shown: memories(kind: \"feedback\"), memories(kind: \
                 \"user\")."
            )),
            "{}",
            context.text
        );
        assert!(
            context
                .status_line()
                .ends_with(&format!("({hidden} not shown)"))
        );
    }

    #[test]
    fn nothing_to_load_or_a_zero_budget_gives_no_text() {
        assert_eq!(render(Vec::new(), DEFAULT_CHARS).text, "");
        let off = render(vec![entry("a", "user", false, None, "x")], 0);
        assert_eq!((off.text.as_str(), off.total), ("", 1));
        assert!(off.status_line().contains("off"), "{}", off.status_line());
    }

    #[test]
    fn a_long_hook_is_cut_on_a_character_boundary() {
        let mut long = entry("long", "feedback", false, None, "x");
        long.hook = "ж".repeat(1000);
        let text = line(&long);
        assert!(text.contains('…'));
        assert!(text.chars().count() < MAX_HOOK_CHARS + 60, "{text}");
    }

    #[test]
    fn the_ui_rows_say_which_lines_are_shortened() {
        let mut long = entry("long", "feedback", false, None, "x");
        long.hook = format!("{}  tail", "a".repeat(MAX_HOOK_CHARS));
        let mut exact = entry("exact", "user", false, None, "x");
        // White space runs count as one, as on the line itself.
        exact.hook = format!("{}\n\n{}", "b".repeat(10), "c".repeat(MAX_HOOK_CHARS - 11));
        let short = entry("short", "user", false, None, "y");
        let rows = entries_json(vec![long, exact, short], 3);
        let by_slug = |slug: &str| {
            rows.as_array()
                .unwrap()
                .iter()
                .find(|row| row["slug"] == slug)
                .unwrap()
                .clone()
        };
        let long = by_slug("long");
        assert_eq!(long["shortened"], true);
        assert_eq!(long["shown_hook_chars"], SHORTENED_HOOK_CHARS);
        assert!(long["line"].as_str().unwrap().contains('…'));
        assert!(!long["line"].as_str().unwrap().contains("tail"));
        let exact = by_slug("exact");
        assert_eq!(exact["shortened"], false, "{exact}");
        assert!(!exact["line"].as_str().unwrap().contains('…'));
        assert_eq!(by_slug("short")["shortened"], false);
    }

    fn side(slug: &str, always: bool, file: &str) -> Side {
        Side {
            slug: slug.into(),
            scope: "project".into(),
            title: format!("Title {slug}"),
            hook: format!("hook of {slug}"),
            always,
            file: file.into(),
        }
    }

    #[test]
    fn open_conflicts_follow_the_groups_with_both_versions_and_their_files() {
        let conflicts = vec![Conflict {
            a: side("spaces", true, ""),
            b: side("tabs", false, "/home/u/.claude/projects/p/memory/tabs.md"),
        }];
        let context = render_with(
            vec![entry("spaces", "feedback", false, None, "2026")],
            conflicts,
            2,
            DEFAULT_CHARS,
        );
        assert_eq!(
            context.text,
            format!(
                "{HEADLINE}\n\n## How to work\n- Title spaces — hook of spaces (memory: spaces)\n\n\
                 {CONFLICTS_HEADING}\n- Title spaces — hook of spaces (memory: spaces, always \
                 loaded)\n  CONFLICTS WITH Title tabs — hook of tabs (memory: tabs, file: \
                 /home/u/.claude/projects/p/memory/tabs.md)\n2 imported memories are not yet \
                 checked for conflicts with the others: conflicts() lists them.\n\n{FOOTER}"
            )
        );
        assert_eq!(
            context.conflicts,
            ["spaces (remember) vs tabs (file /home/u/.claude/projects/p/memory/tabs.md)"]
        );
        assert_eq!(
            context.start_note(),
            " 1 open memory conflict (ask the user which memory holds, then settle it); 2 \
             imported memories to check: conflicts()."
        );
        assert_eq!(context.conflict_lines().len(), 3);
    }

    #[test]
    fn memories_against_instruction_files_follow_the_conflicts_with_the_file_line() {
        let open = json!({"open": [], "unchecked": 0, "rules_unchecked": 3, "rules_open": [
            {"memory": {"slug": "use-pnpm", "scope": "project", "title": "Use pnpm",
                        "hook": "Install with pnpm", "always": true, "file": ""},
             "file": "/w/CLAUDE.md", "line": 12, "text": "Always use npm for packages."}]});
        let (conflicts, unchecked) = conflicts(&open);
        let context = render_all(
            Vec::new(),
            conflicts,
            unchecked,
            rule_conflicts(&open),
            DEFAULT_CHARS,
        );
        assert_eq!(
            context.text,
            format!(
                "{HEADLINE}\n\n{RULE_CONFLICTS_HEADING}\n- Use pnpm — Install with pnpm (memory: \
                 use-pnpm, always loaded)\n  CONTRADICTS /w/CLAUDE.md line 12: Always use npm for \
                 packages.\n3 memories near lines of your instruction files are not yet checked \
                 for a contradiction: conflicts() lists them.\n\n{FOOTER}"
            )
        );
        assert_eq!(context.rule_conflicts, ["use-pnpm (remember) vs /w/CLAUDE.md:12"]);
        assert_eq!(context.rules_unchecked, 3);
        assert_eq!(
            context.start_note(),
            " 1 memory contradicts your instruction files; 3 memories to check against your \
             instruction files: conflicts()."
        );
        let lines = context.conflict_lines();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].contains("1 memory contradicts a line of your instruction files"));
        assert_eq!(lines[1], "  use-pnpm (remember) vs /w/CLAUDE.md:12");
        assert!(lines[2].contains("3 memories near a line of your instruction files"));
        // The daemon's answer carries both, and an older one's neither.
        assert_eq!(StartContext::from_json(&context.to_json()), Some(context.clone()));
        let older = StartContext::from_json(&json!({"text": "", "shown": 0, "total": 0,
                                                    "budget": 10})).unwrap();
        assert!(older.rule_conflicts.is_empty() && older.rules_unchecked == 0);
        // Nothing of it: no text.
        let none = render_all(Vec::new(), Vec::new(), 0, (Vec::new(), 0), DEFAULT_CHARS);
        assert!(none.text.is_empty());
    }

    #[test]
    fn conflicts_alone_give_a_text_and_more_than_five_are_counted() {
        let unchecked = render_with(Vec::new(), Vec::new(), 1, DEFAULT_CHARS);
        assert!(
            unchecked
                .text
                .contains("1 imported memory is not yet checked"),
            "{}",
            unchecked.text
        );
        let many: Vec<Conflict> = (0..7)
            .map(|i| Conflict {
                a: side(&format!("a{i}"), false, ""),
                b: side(&format!("b{i}"), false, ""),
            })
            .collect();
        let context = render_with(Vec::new(), many, 0, DEFAULT_CHARS);
        assert_eq!(
            context.text.matches("CONFLICTS WITH").count(),
            MAX_CONFLICTS
        );
        assert!(
            context.text.contains("\n2 more: conflicts()."),
            "{}",
            context.text
        );
        assert_eq!(context.conflicts.len(), 7);
        assert!(
            render_with(Vec::new(), Vec::new(), 0, DEFAULT_CHARS)
                .text
                .is_empty()
        );
        let (parsed, count) = conflicts(&serde_json::json!({
            "open": [{"a": {"slug": "x", "scope": "global", "title": "X", "hook": "h",
                            "always": true, "file": ""},
                      "b": {"slug": "y"}}, {"a": {}}],
            "unchecked": 4}));
        assert_eq!((parsed.len(), count), (1, 4));
        assert!(parsed[0].a.always);
    }

    #[test]
    fn the_catalog_answer_parses_with_and_without_an_order() {
        let value = serde_json::json!([
            {"slug": "a", "scope": "project", "kind": "feedback", "pinned": false, "order": [],
             "title": "A", "hook": "h", "updated_at": "2026"},
            {"slug": "b", "scope": "global", "kind": "user", "pinned": true, "order": [3],
             "title": "B", "hook": "h", "updated_at": "2026"},
            {"bad": true}
        ]);
        let parsed = entries(&value);
        assert_eq!(parsed.len(), 2);
        assert_eq!((parsed[0].order, parsed[1].order), (None, Some(3)));
        assert!(parsed[1].pinned);
        let round = StartContext::from_json(&render(parsed, 100).to_json()).unwrap();
        assert_eq!(round.budget, 100);
    }
}
