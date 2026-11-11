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

    /// The control-line answer of the daemon (`start_context`).
    pub fn to_json(&self) -> Value {
        serde_json::json!({"ok": true, "text": self.text, "shown": self.shown,
                           "total": self.total, "chars": self.chars(), "budget": self.budget})
    }

    /// The daemon's answer read back (`None` when it is not one).
    pub fn from_json(value: &Value) -> Option<Self> {
        let count = |key: &str| value[key].as_u64().map(|n| n as usize);
        Some(Self {
            text: value["text"].as_str()?.to_string(),
            shown: count("shown")?,
            total: count("total")?,
            budget: count("budget")?,
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
    let hook: String = if hook.chars().count() > MAX_HOOK_CHARS {
        let cut: String = hook.chars().take(MAX_HOOK_CHARS - 1).collect();
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

/// `text` on one line (line breaks and runs of white space become one space).
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The text of `shown` (in selection order) with the "more" line for `hidden`.
fn text_of(shown: &[&Entry], hidden: &[&Entry]) -> String {
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
pub fn render(mut entries: Vec<Entry>, budget: usize) -> StartContext {
    sort(&mut entries);
    let total = entries.len();
    if total == 0 || budget == 0 {
        return StartContext {
            text: String::new(),
            shown: 0,
            total,
            budget,
        };
    }
    let all: Vec<&Entry> = entries.iter().collect();
    // The longest prefix whose text fits (the text grows with the prefix, so a binary search
    // finds it; the "more" line shrinks by at most a few characters as the prefix grows, which
    // the final check covers).
    let fits = |n: usize| text_of(&all[..n], &all[n..]).chars().count() <= budget;
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
        text: text_of(&all[..shown], &all[shown..]),
        shown,
        total,
        budget,
    }
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
