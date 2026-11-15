//! The bench report: one row per answer (arm, question, repeat) with its judge score and its
//! cost, aggregated per arm and category (and per origin when the user's own questions are in
//! the run) into quality, confidently wrong answers, cost, tokens, turns, wall time and cost per
//! score point; written as `report.md` and `report.json`. An answer to a question without a
//! reference is shown but never judged, and stays out of every mean.

use serde_json::{Value, json};

use super::questions::REPORT_CATEGORIES;

/// One judged answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The question id.
    pub question: String,
    /// Its category.
    pub category: String,
    /// Who wrote the question: `user` or `generated`.
    pub origin: String,
    /// Whether the answer was judged (its question has a reference); else `score` means
    /// nothing and the row is in no mean.
    pub judged: bool,
    /// The arm's name.
    pub arm: String,
    /// The repeat (1-based).
    pub repeat: u32,
    /// The judge's score, 0-5.
    pub score: u8,
    /// Whether the judge flagged the answer as confidently wrong.
    pub confident_wrong: bool,
    /// The judge's note.
    pub note: String,
    /// The answer text.
    pub answer: String,
    /// Why the answer or its judging failed, if it did.
    pub error: Option<String>,
    /// The answer's cost, USD.
    pub cost_usd: f64,
    /// Input tokens (plain + cache writes + cache reads).
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// The agent's turns.
    pub turns: u64,
    /// Wall time, milliseconds.
    pub ms: u64,
    /// The judge call's cost, USD.
    pub judge_cost_usd: f64,
}

impl Row {
    /// The row as JSON.
    pub fn to_json(&self) -> Value {
        json!({
            "question": self.question, "category": self.category, "origin": self.origin,
            "arm": self.arm, "repeat": self.repeat, "judged": self.judged,
            "score": if self.judged { json!(self.score) } else { Value::Null },
            "confident_wrong": self.confident_wrong,
            "note": self.note, "answer": self.answer, "error": self.error,
            "cost_usd": self.cost_usd, "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens, "turns": self.turns, "ms": self.ms,
            "judge_cost_usd": self.judge_cost_usd,
        })
    }
}

/// The means of a group of rows.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    /// The arm.
    pub arm: String,
    /// The category, or `all`.
    pub category: String,
    /// The origin (`user`, `generated`), or `all`.
    pub origin: String,
    /// How many answers.
    pub n: usize,
    /// Mean score.
    pub score: f64,
    /// Confidently wrong answers.
    pub confident_wrong: usize,
    /// Mean cost, USD.
    pub cost_usd: f64,
    /// Mean input tokens.
    pub input_tokens: f64,
    /// Mean output tokens.
    pub output_tokens: f64,
    /// Mean turns.
    pub turns: f64,
    /// Mean wall time, seconds.
    pub seconds: f64,
    /// Mean cost divided by mean score (`None` for a mean score of 0).
    pub cost_per_point: Option<f64>,
    /// Failed answers or judgings.
    pub errors: usize,
}

impl Group {
    fn of(arm: &str, category: &str, origin: &str, rows: &[&Row]) -> Option<Self> {
        if rows.is_empty() {
            return None;
        }
        let n = rows.len() as f64;
        let mean = |f: &dyn Fn(&Row) -> f64| rows.iter().map(|r| f(r)).sum::<f64>() / n;
        let score = mean(&|r| f64::from(r.score));
        let cost_usd = mean(&|r| r.cost_usd);
        Some(Self {
            arm: arm.to_string(),
            category: category.to_string(),
            origin: origin.to_string(),
            n: rows.len(),
            score,
            confident_wrong: rows.iter().filter(|r| r.confident_wrong).count(),
            cost_usd,
            input_tokens: mean(&|r| r.input_tokens as f64),
            output_tokens: mean(&|r| r.output_tokens as f64),
            turns: mean(&|r| r.turns as f64),
            seconds: mean(&|r| r.ms as f64 / 1000.0),
            cost_per_point: (score > 0.0).then_some(cost_usd / score),
            errors: rows.iter().filter(|r| r.error.is_some()).count(),
        })
    }

    fn to_json(&self) -> Value {
        json!({
            "arm": self.arm, "category": self.category, "origin": self.origin, "n": self.n,
            "score": self.score,
            "confident_wrong": self.confident_wrong, "cost_usd": self.cost_usd,
            "input_tokens": self.input_tokens, "output_tokens": self.output_tokens,
            "turns": self.turns, "seconds": self.seconds, "cost_per_point": self.cost_per_point,
            "errors": self.errors,
        })
    }
}

/// The origins in report order.
const ORIGINS: [&str; 2] = ["user", "generated"];

/// Every group of the judged rows: per category, then per origin (only when the user's own
/// questions are in the run), then `all`, each per arm in `arms` order.
pub fn groups(rows: &[Row], arms: &[String]) -> Vec<Group> {
    let judged: Vec<&Row> = rows.iter().filter(|r| r.judged).collect();
    let mut out = Vec::new();
    let mut sets: Vec<(&str, &str)> = REPORT_CATEGORIES.iter().map(|c| (*c, "all")).collect();
    if rows.iter().any(|r| r.origin == "user") {
        sets.extend(ORIGINS.iter().map(|o| ("all", *o)));
    }
    sets.push(("all", "all"));
    for (category, origin) in sets {
        for arm in arms {
            let selected: Vec<&Row> = judged
                .iter()
                .copied()
                .filter(|r| {
                    &r.arm == arm
                        && (category == "all" || r.category == category)
                        && (origin == "all" || r.origin == origin)
                })
                .collect();
            out.extend(Group::of(arm, category, origin, &selected));
        }
    }
    out
}

/// Whether `group` has the best mean score of its set (same category and origin): the highest
/// score as shown (two decimals), when at least one other arm of the set scored lower. Tied best
/// arms are all best; when every arm scores the same, none is.
pub fn is_best(group: &Group, groups: &[Group]) -> bool {
    let shown = |g: &Group| (g.score * 100.0).round() as i64;
    let set: Vec<i64> = groups
        .iter()
        .filter(|g| g.category == group.category && g.origin == group.origin)
        .map(shown)
        .collect();
    let (Some(&max), Some(&min)) = (set.iter().max(), set.iter().min()) else {
        return false;
    };
    max > min && shown(group) == max
}

/// The mean score of each repeat of `arm` (the spread between runs).
pub fn repeat_scores(rows: &[Row], arm: &str) -> Vec<f64> {
    let last = rows.iter().map(|r| r.repeat).max().unwrap_or(0);
    (1..=last)
        .filter_map(|repeat| {
            let scores: Vec<f64> = rows
                .iter()
                .filter(|r| r.arm == arm && r.repeat == repeat && r.judged)
                .map(|r| f64::from(r.score))
                .collect();
            (!scores.is_empty()).then(|| scores.iter().sum::<f64>() / scores.len() as f64)
        })
        .collect()
}

/// What a run spent, USD at list price (as the harness reported it).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Spent {
    /// Generating the questions (in this run).
    pub generation: f64,
    /// Looking up the references of the user's own questions (in this run).
    pub references: f64,
    /// The warm-up calls.
    pub warmup: f64,
    /// The answers.
    pub answers: f64,
    /// The judge calls.
    pub judge: f64,
}

impl Spent {
    /// Everything.
    pub fn total(&self) -> f64 {
        self.generation + self.references + self.warmup + self.answers + self.judge
    }
}

/// What the report says about the run besides the rows.
#[derive(Debug, Clone, Default)]
pub struct Meta {
    /// When it ran (UTC).
    pub started_at: String,
    /// The run id at the start of every prompt.
    pub run_id: String,
    /// The project key.
    pub project: String,
    /// The harness command.
    pub harness: String,
    /// The arms, in order, with what each holds.
    pub arms: Vec<(String, String)>,
    /// Questions used.
    pub questions: usize,
    /// Repeats.
    pub repeats: u32,
    /// Sessions the questions were generated from.
    pub sessions: usize,
    /// Whether the user confirmed the question set (else `--yes` accepted it).
    pub confirmed: bool,
    /// A dry run (fake harness, nothing spent).
    pub dry_run: bool,
    /// Why the run stopped early, if it did.
    pub stopped: Option<String>,
    /// Extra caveats of this run.
    pub notes: Vec<String>,
    /// How many of the questions are the user's own.
    pub user_questions: usize,
    /// How many questions have the user's own answer as their reference.
    pub user_references: usize,
    /// The questions without a reference `(id, question)`: answered, shown, not judged.
    pub unjudged: Vec<(String, String)>,
}

fn money(usd: f64) -> String {
    format!("${usd:.3}")
}

fn tokens(n: f64) -> String {
    let n = n.round() as u64;
    let text = n.to_string();
    let mut out = String::new();
    for (i, c) in text.chars().enumerate() {
        if i > 0 && (text.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The caveats every report carries, plus the run's own.
fn caveats(meta: &Meta, rows: &[Row]) -> Vec<String> {
    let mut out = vec![
        format!(
            "Small sample: {} question(s) x {} repeat(s), one project, one model. Read the \
             differences with that in mind.",
            meta.questions, meta.repeats
        ),
        if meta.user_references < meta.questions.saturating_sub(meta.unjudged.len()) {
            "The judge is an LLM (the same harness and model as the answers) scoring against \
             references that an LLM wrote from your raw transcripts."
                .to_string()
        } else {
            "The judge is an LLM (the same harness and model as the answers).".to_string()
        },
        if meta.confirmed {
            "You confirmed the question set.".to_string()
        } else {
            "The question set was NOT reviewed (accepted with --yes): check the references in \
             the question file."
                .to_string()
        },
        "File memory holds no session history, so history questions favour the graph arm by \
         construction when the graph has the sessions indexed."
            .to_string(),
        "Cost is the harness's own list-price figure; prompt caching makes it depend on what \
         ran before (a warm-up per arm and a per-run id at the start of every question keep the \
         arms comparable)."
            .to_string(),
    ];
    if meta.user_references > 0 {
        out.push(format!(
            "{} question(s) of yours are judged against your own answer: the judge compares with \
             your wording, so a correct answer phrased differently, or one that is right where your \
             note is short, may score differently than against a reference from the transcripts.",
            meta.user_references
        ));
    }
    if !meta.unjudged.is_empty() {
        out.push(format!(
            "{} question(s) have no reference (none was found in your sessions): every arm \
             answered them, the answers are shown below, and they are in no score or mean \
             (judging without a reference would reward a confident guess).",
            meta.unjudged.len()
        ));
    }
    let errors = rows.iter().filter(|r| r.error.is_some()).count();
    if errors > 0 {
        out.push(format!(
            "{errors} answer(s) or judgings failed and count as score 0 (see report.json)."
        ));
    }
    if meta.dry_run {
        out.insert(
            0,
            "DRY RUN: a fake harness answered; the numbers say nothing about memory.".to_string(),
        );
    }
    if let Some(why) = &meta.stopped {
        out.insert(0, format!("The run stopped early: {why}."));
    }
    out.extend(meta.notes.iter().cloned());
    out
}

/// The report as Markdown.
pub fn markdown(meta: &Meta, rows: &[Row], spent: &Spent) -> String {
    let arms: Vec<String> = meta.arms.iter().map(|(a, _)| a.clone()).collect();
    let mut out = vec![
        format!(
            "# graphersal-memory bench {}{}",
            meta.started_at,
            if meta.dry_run { " (dry run)" } else { "" }
        ),
        String::new(),
        format!(
            "Project `{}`; harness `{}`; {} question(s) from {} session(s), {} repeat(s); run id \
             `{}`.",
            meta.project, meta.harness, meta.questions, meta.sessions, meta.repeats, meta.run_id
        ),
        String::new(),
        "Arms:".to_string(),
    ];
    for (arm, what) in &meta.arms {
        out.push(format!("- **{arm}**: {what}"));
    }
    out.extend([
        String::new(),
        "| Category | Arm | Mean score (0-5) | Confidently wrong | Mean cost | Cost per point | \
         Mean input tokens | Mean output tokens | Mean turns | Mean s |"
            .to_string(),
        "|---|---|---|---|---|---|---|---|---|---|".to_string(),
    ]);
    let all = groups(rows, &arms);
    for g in &all {
        let label = match (g.category.as_str(), g.origin.as_str()) {
            ("all", "all") => format!("**all ({})**", g.n),
            ("all", "user") => format!("your questions ({})", g.n),
            ("all", origin) => format!("{origin} questions ({})", g.n),
            (category, _) => format!("{category} ({})", g.n),
        };
        // The best arm of each set in bold, its name and its score (the same cells the
        // terminal summary paints green).
        let (arm, score) = if is_best(g, &all) {
            (format!("**{}**", g.arm), format!("**{:.2}**", g.score))
        } else {
            (g.arm.clone(), format!("{:.2}", g.score))
        };
        out.push(format!(
            "| {label} | {arm} | {score} | {} | {} | {} | {} | {} | {:.1} | {:.0} |",
            g.confident_wrong,
            money(g.cost_usd),
            g.cost_per_point.map_or_else(|| "-".to_string(), money),
            tokens(g.input_tokens),
            tokens(g.output_tokens),
            g.turns,
            g.seconds
        ));
    }
    if meta.repeats > 1 {
        out.extend([String::new(), "Mean score per repeat:".to_string()]);
        for arm in &arms {
            let scores: Vec<String> = repeat_scores(rows, arm)
                .iter()
                .map(|s| format!("{s:.2}"))
                .collect();
            out.push(format!("- {arm}: {}", scores.join(", ")));
        }
    }
    out.extend([
        String::new(),
        format!(
            "Spent (list price): {} in all: questions {}{}, warm-up {}, answers {}, judge {}.",
            money(spent.total()),
            money(spent.generation),
            if spent.references > 0.0 {
                format!(", references of your questions {}", money(spent.references))
            } else {
                String::new()
            },
            money(spent.warmup),
            money(spent.answers),
            money(spent.judge)
        ),
        String::new(),
        "## Caveats".to_string(),
        String::new(),
    ]);
    for caveat in caveats(meta, rows) {
        out.push(format!("- {caveat}"));
    }
    out.extend([
        String::new(),
        "## Answers".to_string(),
        String::new(),
        "| Question | Arm | Repeat | Score | Judge's note |".to_string(),
        "|---|---|---|---|---|".to_string(),
    ]);
    for r in rows {
        out.push(format!(
            "| {} ({}{}) | {} | {} | {}{} | {} |",
            r.question,
            if r.origin == "user" { "yours, " } else { "" },
            r.category,
            r.arm,
            r.repeat,
            if r.judged {
                r.score.to_string()
            } else {
                "not judged".to_string()
            },
            if r.confident_wrong {
                " (confidently wrong)"
            } else {
                ""
            },
            r.note.replace('|', "/").replace('\n', " ")
        ));
    }
    if !meta.unjudged.is_empty() {
        out.extend([
            String::new(),
            "## Questions without a reference".to_string(),
            String::new(),
            "Not judged and in no mean: read the answers yourself.".to_string(),
        ]);
        for (id, question) in &meta.unjudged {
            out.extend([
                String::new(),
                format!("**{id}**: {question}"),
                String::new(),
            ]);
            for r in rows.iter().filter(|r| &r.question == id) {
                let answer = if r.answer.trim().is_empty() {
                    r.error
                        .clone()
                        .map_or_else(|| "(no answer)".to_string(), |e| format!("(failed: {e})"))
                } else {
                    crate::memory::transcript::clip(&r.answer.replace('\n', " "), 600)
                };
                out.push(format!("- {} r{}: {answer}", r.arm, r.repeat));
            }
        }
    }
    out.push(String::new());
    out.join("\n")
}

/// The report as JSON.
pub fn to_json(meta: &Meta, rows: &[Row], spent: &Spent) -> Value {
    let arms: Vec<String> = meta.arms.iter().map(|(a, _)| a.clone()).collect();
    json!({
        "version": 1,
        "started_at": meta.started_at,
        "run_id": meta.run_id,
        "project": meta.project,
        "harness": meta.harness,
        "dry_run": meta.dry_run,
        "stopped": meta.stopped,
        "arms": meta.arms.iter().map(|(a, w)| json!({"arm": a, "holds": w})).collect::<Vec<_>>(),
        "questions": meta.questions,
        "repeats": meta.repeats,
        "sessions": meta.sessions,
        "confirmed": meta.confirmed,
        "groups": groups(rows, &arms).iter().map(Group::to_json).collect::<Vec<_>>(),
        "repeat_scores": arms.iter().map(|a| json!({"arm": a, "scores": repeat_scores(rows, a)}))
            .collect::<Vec<_>>(),
        "user_questions": meta.user_questions,
        "unjudged": meta.unjudged.iter().map(|(id, q)| json!({"id": id, "question": q}))
            .collect::<Vec<_>>(),
        "spent_usd": {"total": spent.total(), "questions": spent.generation,
                      "references": spent.references,
                      "warmup": spent.warmup, "answers": spent.answers, "judge": spent.judge},
        "caveats": caveats(meta, rows),
        "rows": rows.iter().map(Row::to_json).collect::<Vec<_>>(),
    })
}

/// The printed summary: one line per arm (all categories) and the spend.
#[cfg(test)]
pub fn summary(meta: &Meta, rows: &[Row], spent: &Spent) -> String {
    summary_styled(meta, rows, spent, false)
}

/// [`summary`] with the best arm's line in green when `color` is on (a terminal that allows
/// colour), as `report.md` marks it in bold.
pub fn summary_styled(meta: &Meta, rows: &[Row], spent: &Spent, color: bool) -> String {
    let arms: Vec<String> = meta.arms.iter().map(|(a, _)| a.clone()).collect();
    let mut out = Vec::new();
    let groups = groups(rows, &arms);
    for g in groups
        .iter()
        .filter(|g| g.category == "all" && g.origin == "all")
    {
        let line = format!(
            "  {:<6} score {:.2}/5, {} confidently wrong, {} per answer, {} input tokens, cost \
             per point {}",
            g.arm,
            g.score,
            g.confident_wrong,
            money(g.cost_usd),
            tokens(g.input_tokens),
            g.cost_per_point.map_or_else(|| "-".to_string(), money)
        );
        out.push(if color && is_best(g, &groups) {
            format!("\x1b[32m{line}\x1b[0m")
        } else {
            line
        });
    }
    let mine: Vec<String> = groups
        .iter()
        .filter(|g| g.origin == "user")
        .map(|g| format!("{} {:.2}", g.arm, g.score))
        .collect();
    if !mine.is_empty() {
        out.push(format!("  your own questions: {}", mine.join(", ")));
    }
    if !meta.unjudged.is_empty() {
        out.push(format!(
            "  {} question(s) without a reference: answered, not scored (see the report)",
            meta.unjudged.len()
        ));
    }
    out.push(format!(
        "  spent {} (list price){}",
        money(spent.total()),
        if meta.dry_run {
            "; dry run: nothing was spent"
        } else {
            ""
        }
    ));
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(q: &str, category: &str, arm: &str, repeat: u32, score: u8, cost: f64) -> Row {
        Row {
            question: q.into(),
            category: category.into(),
            origin: "generated".into(),
            judged: true,
            arm: arm.into(),
            repeat,
            score,
            confident_wrong: false,
            note: "n".into(),
            answer: "a".into(),
            error: None,
            cost_usd: cost,
            input_tokens: 1000,
            output_tokens: 100,
            turns: 2,
            ms: 3000,
            judge_cost_usd: 0.01,
        }
    }

    #[test]
    fn groups_compute_means_and_cost_per_point() {
        let mut rows = vec![
            row("Q1", "rule", "files", 1, 4, 0.02),
            row("Q2", "history", "files", 1, 0, 0.04),
            row("Q1", "rule", "graph", 1, 5, 0.03),
            row("Q2", "history", "graph", 1, 3, 0.05),
            row("Q1", "rule", "none", 1, 0, 0.01),
            row("Q2", "history", "none", 1, 0, 0.01),
        ];
        rows[3].confident_wrong = true;
        rows[3].input_tokens = 3000;
        let arms = vec!["none".to_string(), "files".to_string(), "graph".to_string()];
        let groups = groups(&rows, &arms);
        let find = |arm: &str, cat: &str| {
            groups
                .iter()
                .find(|g| g.arm == arm && g.category == cat)
                .unwrap()
                .clone()
        };
        let files = find("files", "all");
        assert_eq!(files.n, 2);
        assert!((files.score - 2.0).abs() < 1e-9);
        assert!((files.cost_usd - 0.03).abs() < 1e-9);
        assert!((files.cost_per_point.unwrap() - 0.015).abs() < 1e-9);
        let graph = find("graph", "all");
        assert!((graph.score - 4.0).abs() < 1e-9);
        assert_eq!(graph.confident_wrong, 1);
        assert!((graph.input_tokens - 2000.0).abs() < 1e-9);
        assert!((graph.seconds - 3.0).abs() < 1e-9);
        assert_eq!(find("none", "all").cost_per_point, None);
        assert_eq!(find("graph", "rule").n, 1);
        // No decision questions: no decision groups.
        assert!(!groups.iter().any(|g| g.category == "decision"));
        // Order: categories first, then all; arms in the given order.
        assert_eq!(groups[0].arm, "none");
        assert_eq!(groups.last().unwrap().category, "all");
    }

    #[test]
    fn the_report_carries_numbers_spend_and_caveats() {
        let rows = vec![
            row("Q1", "rule", "files", 1, 4, 0.02),
            row("Q1", "rule", "files", 2, 2, 0.02),
            row("Q1", "rule", "graph", 1, 5, 0.03),
            row("Q1", "rule", "graph", 2, 5, 0.03),
        ];
        let meta = Meta {
            started_at: "2026-11-10T10:00:00Z".into(),
            run_id: "abc".into(),
            project: "/w/p".into(),
            harness: "claude -p".into(),
            arms: vec![
                ("files".into(), "file memory".into()),
                ("graph".into(), "graph memory".into()),
            ],
            questions: 1,
            repeats: 2,
            sessions: 3,
            confirmed: false,
            ..Meta::default()
        };
        let spent = Spent {
            generation: 0.1,
            references: 0.0,
            warmup: 0.05,
            answers: 0.1,
            judge: 0.04,
        };
        let text = markdown(&meta, &rows, &spent);
        assert!(
            text.contains(
                "| **all (2)** | **graph** | **5.00** | 0 | $0.030 | $0.006 | 1,000 | 100 |"
            ),
            "{text}"
        );
        assert!(text.contains("- files: 4.00, 2.00"), "{text}");
        assert!(text.contains("Spent (list price): $0.290 in all"), "{text}");
        assert!(text.contains("NOT reviewed"), "{text}");
        assert!(text.contains("## Caveats"));
        let value = to_json(&meta, &rows, &spent);
        assert!((value["spent_usd"]["total"].as_f64().unwrap() - 0.29).abs() < 1e-9);
        assert_eq!(value["rows"].as_array().unwrap().len(), 4);
        assert_eq!(value["repeat_scores"][1]["scores"], json!([5.0, 5.0]));
        let printed = summary(&meta, &rows, &spent);
        assert!(printed.contains("graph  score 5.00/5"), "{printed}");
        assert_eq!(tokens(1234567.4), "1,234,567");
        assert_eq!(tokens(12.0), "12");
    }

    fn group(arm: &str, category: &str, score: f64) -> Group {
        Group {
            arm: arm.into(),
            category: category.into(),
            origin: "all".into(),
            n: 1,
            score,
            confident_wrong: 0,
            cost_usd: 0.0,
            input_tokens: 0.0,
            output_tokens: 0.0,
            turns: 0.0,
            seconds: 0.0,
            cost_per_point: None,
            errors: 0,
        }
    }

    #[test]
    fn the_best_arm_is_the_highest_shown_score_of_its_set() {
        let groups = vec![
            group("files", "rule", 4.0),
            group("graph", "rule", 5.0),
            group("files", "history", 3.0),
            group("graph", "history", 3.0),
            group("none", "decision", 1.0),
            group("files", "decision", 4.333),
            group("graph", "decision", 4.334),
            group("graph", "all", 4.0),
        ];
        let best: Vec<(&str, &str)> = groups
            .iter()
            .filter(|g| is_best(g, &groups))
            .map(|g| (g.arm.as_str(), g.category.as_str()))
            .collect();
        // rule: graph wins; history: a tie of every arm marks none; decision: the two that show
        // the same 4.33 are both best; a set of one arm has nobody to beat.
        assert_eq!(
            best,
            [
                ("graph", "rule"),
                ("files", "decision"),
                ("graph", "decision")
            ]
        );
    }

    #[test]
    fn the_report_marks_the_best_arm_bold_and_the_summary_paints_it_green() {
        let rows = vec![
            row("Q1", "rule", "files", 1, 2, 0.02),
            row("Q1", "rule", "graph", 1, 5, 0.03),
        ];
        let meta = Meta {
            arms: vec![
                ("files".into(), "file memory".into()),
                ("graph".into(), "graphersal-memory".into()),
            ],
            ..Meta::default()
        };
        let spent = Spent::default();
        let text = markdown(&meta, &rows, &spent);
        assert!(
            text.contains("| rule (1) | **graph** | **5.00** |"),
            "{text}"
        );
        assert!(text.contains("| rule (1) | files | 2.00 |"), "{text}");
        let plain = summary_styled(&meta, &rows, &spent, false);
        assert!(!plain.contains('\x1b'), "{plain}");
        let colored = summary_styled(&meta, &rows, &spent, true);
        let graph = colored.lines().find(|l| l.contains("graph")).unwrap();
        assert!(
            graph.starts_with("\x1b[32m") && graph.ends_with("\x1b[0m"),
            "{colored}"
        );
        let files = colored.lines().find(|l| l.contains("files")).unwrap();
        assert!(!files.contains('\x1b'), "{colored}");
    }
}
