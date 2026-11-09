//! Session summaries through the user's own harness (opt-in, `crate::config::Summaries`).
//!
//! Nothing here runs unless `summaries.enabled` is `true` in `config.json`: the daemon's session
//! indexer (`crate::sessions`) asks [`Summarizer::new`] and gets `None` otherwise. When enabled,
//! each finished session is summarized once (again when it grows) by running the harness
//! command (default `claude -p --model sonnet`) with the prompt on stdin; there is no API key of
//! ours and no other fallback.
//!
//! What the harness receives: the instructions below, the session's title, start and end time,
//! branches, the paths of the files edited, the harness's own compaction summaries, and every
//! turn's prompt and text reply (long ones cut in the middle). Tool inputs and tool results are
//! never sent. For `claude` the command gets `--output-format json --json-schema <schema>
//! --tools "" --strict-mcp-config --no-session-persistence` appended (no tools, no MCP servers,
//! no transcript of its own), runs in `<data dir>/sessions/harness` and with
//! `GRAPHERSAL_MEMORY_HOOK=off`, so the product's own hooks do nothing in it.
//!
//! Cost: before a call, the spend of the month (`<data dir>/sessions/spend.json`) plus the
//! call's estimate must stay within `summaries.monthly_budget_usd`; after it, the cost the
//! harness reports (`total_cost_usd` of `claude -p`) is added, or the estimate when it reports
//! none. Above the cap only raw turns are indexed; summaries catch up the next month.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::config::Config;
use crate::fsutil;
use crate::transcript::{Transcript, clip};

/// The environment variable that switches the product's hooks off in a harness run of ours.
pub const HOOK_ENV: &str = "GRAPHERSAL_MEMORY_HOOK";

/// How long one harness run may take.
pub const HARNESS_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// The prompt's budget for the transcript part (characters).
const PROMPT_BUDGET: usize = 150_000;

/// A session whose condensed text is shorter gets no summary (nothing worth one).
pub const MIN_CHARS: usize = 1_500;

/// Estimate: characters per input token.
const CHARS_PER_TOKEN: f64 = 4.0;

/// Estimate: input tokens a harness adds of its own per call (its system prompt).
const HARNESS_OVERHEAD_TOKENS: f64 = 15_000.0;

/// Estimate: output tokens of one summary.
const OUTPUT_TOKENS: f64 = 2_000.0;

/// Estimate: list price per million input tokens, USD (a Sonnet-class model; an upper bound of
/// the current Sonnet models' prices).
const INPUT_USD_PER_MTOK: f64 = 3.0;

/// Estimate: list price per million output tokens, USD.
const OUTPUT_USD_PER_MTOK: f64 = 15.0;

/// The JSON schema of a summary.
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "headline": {"type": "string", "description": "one English line (max 120 chars): what the session did"},
            "summary": {"type": "string", "description": "2-4 English sentences: goal, what happened, outcome"},
            "decisions": {"type": "array", "items": {"type": "object", "properties": {
                "decision": {"type": "string"}, "why": {"type": "string"}}, "required": ["decision", "why"]}},
            "corrections": {"type": "array", "items": {"type": "string"}},
            "outcomes": {"type": "array", "items": {"type": "string"}},
            "open_threads": {"type": "array", "items": {"type": "string"}},
        },
        "required": ["headline", "summary", "decisions", "corrections", "outcomes", "open_threads"],
    })
}

/// The instructions before the transcript.
fn instructions(project: &str) -> String {
    let name = Path::new(project)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| project.to_string());
    format!(
        "You summarize one past working session between a software developer (the user) and an AI \
coding assistant, on the project \"{name}\". The summary goes into a memory the assistant searches \
in later sessions, so it must keep what a future assistant needs: what the user DECIDED (and why), \
where the user CORRECTED or pushed back on the assistant (behaviour, claims, style), what was built, \
merged, released or found (keep branch names, commit ids, versions, file paths, numbers), and what \
was left open or deferred.

Write everything in ENGLISH, whatever the language of the transcript. Be specific and dense; no \
filler. Decisions are the user's choices, not the assistant's proposals that were not confirmed. \
Empty lists are fine. Answer with one JSON object only, matching this JSON schema:
{}

The transcript follows. Long messages are cut ([...]).",
        schema()
    )
}

/// The transcript part of the prompt: header, compaction summaries and turns, shortened until
/// it fits `budget` characters.
pub fn condensed(transcript: &Transcript, files: &[String], budget: usize) -> String {
    let head = format!(
        "Title: {}\nStarted: {}  Ended: {}\nBranches: {}\nFiles edited: {}\n",
        transcript.title,
        transcript.started_at,
        transcript.ended_at,
        transcript.branches.join(", "),
        files
            .iter()
            .take(80)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
    let compact: String = transcript
        .compact
        .iter()
        .map(|c| {
            format!(
                "\n[Earlier part of the session, compacted by the tool]\n{}\n",
                clip(c, 12_000)
            )
        })
        .collect();
    let (mut user, mut assistant) = (4_000usize, 3_000usize);
    loop {
        let body: String = transcript
            .turns
            .iter()
            .map(|turn| {
                format!(
                    "\n--- turn {} ({})\nUSER: {}\nASSISTANT: {}\n",
                    turn.n,
                    turn.at.chars().take(16).collect::<String>(),
                    clip(&turn.user, user),
                    clip(&turn.assistant, assistant)
                )
            })
            .collect();
        let size = head.chars().count() + compact.chars().count() + body.chars().count();
        if size <= budget || user < 300 {
            return format!("{head}{compact}{body}");
        }
        user = user * 3 / 4;
        assistant = assistant * 3 / 4;
    }
}

/// The whole prompt sent to the harness for one session.
pub fn prompt(transcript: &Transcript, files: &[String], project: &str) -> String {
    format!(
        "{}\n\n{}",
        instructions(project),
        condensed(transcript, files, PROMPT_BUDGET)
    )
}

/// The estimated list-price cost (USD) of summarizing with a prompt of `chars` characters.
pub fn estimate_usd(chars: usize) -> f64 {
    let input = chars as f64 / CHARS_PER_TOKEN + HARNESS_OVERHEAD_TOKENS;
    (input * INPUT_USD_PER_MTOK + OUTPUT_TOKENS * OUTPUT_USD_PER_MTOK) / 1_000_000.0
}

/// The spend file of `data_dir`.
fn spend_path(data_dir: &Path) -> PathBuf {
    data_dir.join("sessions").join("spend.json")
}

/// The current calendar month (UTC) as `YYYY-MM`.
pub fn this_month() -> String {
    crate::daemon::now_utc().chars().take(7).collect()
}

/// What was spent on summaries per month.
#[derive(Debug, Clone, Default)]
pub struct Spend {
    months: serde_json::Map<String, Value>,
}

impl Spend {
    /// Reads the spend of `data_dir` (none yet: nothing spent).
    pub fn load(data_dir: &Path) -> Result<Self, String> {
        let path = spend_path(data_dir);
        let Some(bytes) = fsutil::read_optional(&path)? else {
            return Ok(Self::default());
        };
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|err| format!("{} is invalid: {err}", path.display()))?;
        Ok(Self {
            months: value["months"].as_object().cloned().unwrap_or_default(),
        })
    }

    /// The USD spent in `month`.
    pub fn usd(&self, month: &str) -> f64 {
        self.months
            .get(month)
            .and_then(|m| m["usd"].as_f64())
            .unwrap_or(0.0)
    }

    /// The summaries made in `month`.
    pub fn calls(&self, month: &str) -> u64 {
        self.months
            .get(month)
            .and_then(|m| m["calls"].as_u64())
            .unwrap_or(0)
    }

    /// Adds one call's cost to `month` and saves.
    pub fn add(
        &mut self,
        data_dir: &Path,
        month: &str,
        usd: f64,
        reported: bool,
    ) -> Result<(), String> {
        let entry = self
            .months
            .entry(month.to_string())
            .or_insert_with(|| json!({"usd": 0.0, "calls": 0, "estimated_calls": 0}));
        entry["usd"] = json!(entry["usd"].as_f64().unwrap_or(0.0) + usd);
        entry["calls"] = json!(entry["calls"].as_u64().unwrap_or(0) + 1);
        if !reported {
            entry["estimated_calls"] = json!(entry["estimated_calls"].as_u64().unwrap_or(0) + 1);
        }
        let path = spend_path(data_dir);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
        }
        let value = json!({"version": 1, "months": self.months});
        fsutil::write_atomic(
            &path,
            format!("{}\n", crate::jsonedit::render(&value, Some("  "), 0)).as_bytes(),
        )
    }
}

/// One summary: the structured output of the harness and the cost it reported.
#[derive(Debug, Clone)]
pub struct Summary {
    /// The summary object (headline, summary, decisions, ...).
    pub output: Value,
    /// The cost the harness reported, USD.
    pub cost_usd: Option<f64>,
}

/// Why a summary was not made.
#[derive(Debug, Clone, PartialEq)]
pub enum Refused {
    /// The month's budget does not allow the call.
    Budget {
        /// Spent this month.
        spent: f64,
        /// The cap.
        cap: f64,
    },
    /// The harness failed.
    Failed(String),
}

/// The summarizer of one indexing run: exists only when the user enabled summaries.
pub struct Summarizer {
    settings: crate::config::Summaries,
    data_dir: PathBuf,
    spend: Spend,
    /// The calls this run made.
    pub calls: usize,
}

impl Summarizer {
    /// The summarizer, or `None` when summaries are off (the default) or the settings cannot
    /// be read (then nothing is sent either; the error is returned for the log).
    pub fn new(data_dir: &Path) -> (Option<Self>, Option<String>) {
        match Config::load(data_dir) {
            Ok(config) if config.summaries.enabled => match Spend::load(data_dir) {
                Ok(spend) => (
                    Some(Self {
                        settings: config.summaries,
                        data_dir: data_dir.to_path_buf(),
                        spend,
                        calls: 0,
                    }),
                    None,
                ),
                Err(err) => (None, Some(err)),
            },
            Ok(_) => (None, None),
            Err(err) => (None, Some(err)),
        }
    }

    /// The date before which sessions get no summary.
    pub fn since(&self) -> Option<&str> {
        self.settings.since.as_deref()
    }

    /// Summarizes one session with `prompt`, within the budget.
    pub fn summarize(&mut self, prompt: &str) -> Result<Summary, Refused> {
        let month = this_month();
        let estimate = estimate_usd(prompt.chars().count());
        let spent = self.spend.usd(&month);
        let cap = self.settings.monthly_budget_usd;
        if spent + estimate > cap {
            return Err(Refused::Budget { spent, cap });
        }
        let work = self.data_dir.join("sessions").join("harness");
        std::fs::create_dir_all(&work)
            .map_err(|err| Refused::Failed(format!("cannot create {}: {err}", work.display())))?;
        self.calls += 1;
        let result = run_harness(
            &self.settings.harness_command,
            prompt,
            &work,
            HARNESS_TIMEOUT,
        );
        // A call that ran costs money even when its answer is unusable.
        let cost = match &result {
            Ok(summary) => summary.cost_usd,
            Err(_) => None,
        };
        self.spend
            .add(
                &self.data_dir,
                &month,
                cost.unwrap_or(estimate),
                cost.is_some(),
            )
            .map_err(Refused::Failed)?;
        result.map_err(Refused::Failed)
    }
}

/// The program and arguments of one harness run: the configured command, plus the flags of
/// `claude` when the program is `claude` (structured output, no tools, no MCP, no transcript).
pub fn harness_args(command: &[String]) -> Vec<String> {
    let mut args: Vec<String> = command.to_vec();
    let is_claude = command.first().is_some_and(|program| {
        Path::new(program)
            .file_stem()
            .is_some_and(|stem| stem == "claude")
    });
    if is_claude {
        for extra in [
            "--output-format",
            "json",
            "--json-schema",
            &schema().to_string(),
            "--tools",
            "",
            "--strict-mcp-config",
            "--no-session-persistence",
        ] {
            args.push(extra.to_string());
        }
    }
    args
}

/// Runs the harness once: the prompt on stdin, the answer on stdout (`crate::harness::run`:
/// `claude.cmd` on Windows is found through `PATHEXT`).
pub fn run_harness(
    command: &[String],
    prompt: &str,
    work_dir: &Path,
    timeout: Duration,
) -> Result<Summary, String> {
    let args = harness_args(command);
    if args.is_empty() {
        return Err("summaries.harness_command is empty".to_string());
    }
    let output = crate::harness::run(&args, prompt, work_dir, &[(HOOK_ENV, "off")], timeout)
        .map_err(|err| {
            if err.contains("did not finish") {
                format!(
                    "the harness did not finish within {} minutes",
                    timeout.as_secs() / 60
                )
            } else {
                err
            }
        })?;
    match parse_answer(&output.stdout) {
        Ok(summary) => Ok(summary),
        Err(why) => Err(format!(
            "{why} (exit {}; stdout: {}; stderr: {})",
            output
                .code
                .map_or_else(|| "signal".to_string(), |c| c.to_string()),
            clip(output.stdout.trim(), 300),
            clip(output.stderr.trim(), 300)
        )),
    }
}

/// The summary in a harness's answer: `claude -p --output-format json` (`structured_output`, or
/// a `result` text holding the JSON, `total_cost_usd`, `is_error`), or the summary object itself.
pub fn parse_answer(text: &str) -> Result<Summary, String> {
    let value: Value = json_in(text).ok_or("the harness answered no JSON")?;
    if value["is_error"] == Value::Bool(true) {
        return Err("the harness reported an error".to_string());
    }
    let cost_usd = value["total_cost_usd"].as_f64();
    let output = if value["structured_output"].is_object() {
        value["structured_output"].clone()
    } else if let Some(result) = value["result"].as_str() {
        json_in(result).ok_or("the harness's result holds no JSON summary")?
    } else {
        value
    };
    if !output["headline"].is_string() || !output["summary"].is_string() {
        return Err("the summary has no headline or summary".to_string());
    }
    Ok(Summary { output, cost_usd })
}

/// The JSON object in `text`: the whole text, or from its first `{` to its last `}` (a model
/// may wrap it in a code fence).
fn json_in(text: &str) -> Option<Value> {
    let text = text.trim();
    if let Ok(value @ Value::Object(_)) = serde_json::from_str(text) {
        return Some(value);
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    match serde_json::from_str(text.get(start..=end)?) {
        Ok(value @ Value::Object(_)) => Some(value),
        _ => None,
    }
}

/// The fact kinds, by the summary key that lists them.
pub const FACT_KINDS: [(&str, &str); 4] = [
    ("decisions", "decision"),
    ("corrections", "correction"),
    ("outcomes", "outcome"),
    ("open_threads", "open_thread"),
];

/// The summary as markdown: the summary, then each list under its heading.
pub fn render(output: &Value) -> String {
    let mut out = vec![
        output["summary"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string(),
    ];
    for (key, title) in [
        ("decisions", "Decisions"),
        ("corrections", "User corrections"),
        ("outcomes", "Outcomes"),
        ("open_threads", "Open threads"),
    ] {
        let items = output[key].as_array().cloned().unwrap_or_default();
        if items.is_empty() {
            continue;
        }
        out.push(format!("\n**{title}:**"));
        for item in items {
            match &item {
                Value::Object(_) => out.push(format!(
                    "- {} (why: {})",
                    item["decision"].as_str().unwrap_or_default(),
                    item["why"].as_str().unwrap_or_default()
                )),
                Value::String(text) => out.push(format!("- {text}")),
                _ => {}
            }
        }
    }
    out.join("\n")
}

/// One fact of a summary: (kind, index in its list, text, why).
pub fn facts(output: &Value) -> Vec<(&'static str, usize, String, String)> {
    let mut out = Vec::new();
    for (key, kind) in FACT_KINDS {
        for (i, item) in output[key].as_array().into_iter().flatten().enumerate() {
            let (text, why) = match item {
                Value::Object(_) => (
                    item["decision"].as_str().unwrap_or_default(),
                    item["why"].as_str().unwrap_or_default(),
                ),
                Value::String(text) => (text.as_str(), ""),
                _ => continue,
            };
            let text = text.trim();
            if !text.is_empty() {
                out.push((kind, i, text.to_string(), why.trim().to_string()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_gets_its_flags_and_other_harnesses_do_not() {
        let claude = harness_args(&["/usr/local/bin/claude".into(), "-p".into()]);
        assert_eq!(&claude[..2], ["/usr/local/bin/claude", "-p"]);
        assert!(claude.contains(&"--json-schema".to_string()));
        assert!(claude.contains(&"--strict-mcp-config".to_string()));
        assert!(claude.contains(&"--no-session-persistence".to_string()));
        let other = harness_args(&["opencode".into(), "run".into()]);
        assert_eq!(other, vec!["opencode", "run"]);
    }

    #[test]
    fn answers_of_claude_and_plain_json_parse() {
        let claude = json!({"type": "result", "is_error": false, "total_cost_usd": 0.12,
            "structured_output": {"headline": "h", "summary": "s", "decisions": []}})
        .to_string();
        let summary = parse_answer(&claude).unwrap();
        assert_eq!(summary.cost_usd, Some(0.12));
        assert_eq!(summary.output["headline"], "h");
        let in_result =
            json!({"result": "```json\n{\"headline\": \"h2\", \"summary\": \"s\"}\n```",
                               "total_cost_usd": 0.5})
            .to_string();
        assert_eq!(parse_answer(&in_result).unwrap().output["headline"], "h2");
        let plain = parse_answer("{\"headline\": \"h3\", \"summary\": \"s\"}").unwrap();
        assert_eq!(plain.cost_usd, None);
        assert!(parse_answer("no").is_err());
        assert!(parse_answer(&json!({"is_error": true}).to_string()).is_err());
        assert!(parse_answer("{\"headline\": 1}").is_err());
    }

    #[test]
    fn facts_and_markdown_come_from_the_lists() {
        let output = json!({"headline": "h", "summary": " S. ",
            "decisions": [{"decision": "Use X", "why": "fast"}, {"decision": " ", "why": ""}],
            "corrections": ["Do not push"], "outcomes": [], "open_threads": ["Docs"]});
        let facts = facts(&output);
        assert_eq!(
            facts,
            vec![
                ("decision", 0, "Use X".to_string(), "fast".to_string()),
                ("correction", 0, "Do not push".to_string(), String::new()),
                ("open_thread", 0, "Docs".to_string(), String::new()),
            ]
        );
        let text = render(&output);
        assert!(
            text.starts_with("S.\n\n**Decisions:**\n- Use X (why: fast)"),
            "{text}"
        );
        assert!(!text.contains("Outcomes"));
    }

    #[test]
    fn the_estimate_grows_with_the_prompt() {
        let small = estimate_usd(1_000);
        let large = estimate_usd(150_000);
        assert!(small > 0.0 && small < large);
        // About the measured cost of the prototype's summaries (~$0.19 per session).
        assert!((0.1..0.3).contains(&large), "{large}");
    }

    #[test]
    fn the_condensed_transcript_fits_its_budget() {
        let mut transcript =
            crate::transcript::parse(crate::transcript::tests::fixture().as_bytes(), "x");
        transcript.turns[0].user = "u".repeat(50_000);
        let text = condensed(&transcript, &["src/a.rs".to_string()], 5_000);
        assert!(text.chars().count() <= 5_000, "{}", text.len());
        assert!(text.contains("Files edited: src/a.rs"));
        assert!(text.contains("USER: /review src"));
        let whole = prompt(&transcript, &[], "/w/app");
        assert!(whole.contains("project \"app\""));
        assert!(whole.contains("\"headline\""));
    }

    #[test]
    fn spend_adds_up_per_month() {
        let dir = crate::mcp_tests::TempDir::new("spend");
        let mut spend = Spend::load(dir.path()).unwrap();
        assert_eq!(spend.usd("2026-11"), 0.0);
        spend.add(dir.path(), "2026-11", 0.25, true).unwrap();
        spend.add(dir.path(), "2026-11", 0.5, false).unwrap();
        let again = Spend::load(dir.path()).unwrap();
        assert!((again.usd("2026-11") - 0.75).abs() < 1e-9);
        assert_eq!(again.calls("2026-11"), 2);
        assert_eq!(again.usd("2026-12"), 0.0);
    }
}
