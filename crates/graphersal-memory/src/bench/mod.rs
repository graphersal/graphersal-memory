//! `graphersal-memory bench`: the built-in A/B on the user's own memory, through Claude Code's
//! `claude -p` or opencode's `opencode run` (whichever `summaries.harness_command` names; the
//! opencode arms run in sandboxes of their own configuration and data directories,
//! `bench::arms::build_opencode`).
//!
//! 1. **Questions** (`bench::questions`): generated once per project by the harness from the
//!    user's raw transcripts, with reference answers and source turn ids, plus the user's own
//!    (`bench questions add`/`import`, or the interactive step), whose reference is the user's
//!    answer or is looked up in the same raw transcripts; the user reviews the file (`bench
//!    questions --review`) or accepts it with `--yes`.
//! 2. **Arms** (`bench::arms`): each a sandbox built for the run (own MCP config, own settings,
//!    own context, empty or copied working directory, read-only tools); the graph arm runs on a
//!    fork of the store. The live configuration and store are never changed.
//! 3. **Run**: a warm-up call per arm, then every question in every arm per repeat, each prompt
//!    starting with the run id (no cached tail of an earlier run), each answer judged 0-5
//!    against its reference with a confidently-wrong flag; cost, tokens, turns and wall time
//!    from `claude -p --output-format json`.
//! 4. **Report** (`bench::report`): `report.md` and `report.json` in
//!    `<data dir>/bench/<timestamp>/`, and a printed summary.
//!
//! Cost guard: the estimate is printed before anything runs, a run above `--budget` is refused,
//! each call carries `--max-budget-usd`, and the run stops when its real spend would pass the
//! budget. `--dry-run` uses [`DryRun`], a fake harness that calls no model.

pub mod arms;
#[cfg(test)]
pub(crate) mod bench_tests;
pub mod questions;
pub mod report;
pub mod rule_changes;

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

use crate::cli::BenchArm;
use crate::commands::EXIT_FAILED;
use questions::{Excerpt, Origin, Question, QuestionSet, ReferenceFrom};
use report::{Meta, Row, Spent};

/// Estimate: one answer, USD (the prototype measured $0.024-0.041 per answer with Sonnet).
pub const ANSWER_USD: f64 = 0.05;

/// Estimate: one judge call, USD.
pub const JUDGE_USD: f64 = 0.02;

/// The most one answer (or warm-up) may cost (`--max-budget-usd`).
const ANSWER_CAP_USD: f64 = 0.50;

/// The most one judge call may cost.
const JUDGE_CAP_USD: f64 = 0.25;

/// How long one call may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// What a bench run works on.
#[derive(Debug, Clone)]
pub struct Options {
    /// The arms, in report order.
    pub arms: Vec<BenchArm>,
    /// Generated questions to ask besides the user's own: generated when the project has no
    /// kept generated question; 0 asks the user's questions only.
    pub questions: usize,
    /// Repeats.
    pub repeats: u32,
    /// The budget, USD.
    pub budget_usd: f64,
    /// A dry run (the caller passes [`DryRun`]).
    pub dry_run: bool,
    /// Do not ask.
    pub yes: bool,
    /// Offer the step that adds the user's own questions (a terminal, without `--yes` and
    /// outside a dry run).
    pub interactive: bool,
    /// Colour in that step.
    pub color: bool,
    /// The project key.
    pub project: String,
    /// The harnesses with their files (`--claude-dir` replaces Claude Code's): the harness
    /// command's one is the one the run goes through ([`Options::bench`]).
    pub harnesses: crate::harnesses::Harnesses,
    /// The data directory (the live store is forked from it; reports go into it).
    pub data_dir: PathBuf,
    /// This binary (the graph arm's MCP server).
    pub exe: PathBuf,
    /// The harness command (`claude -p --model sonnet`).
    pub harness_command: Vec<String>,
}

impl Options {
    /// The harness the run goes through: the harness command's (Claude Code's when the
    /// command is no harness's the bench knows; `check_harness_in` refuses those first).
    pub fn bench(&self) -> std::sync::Arc<dyn BenchSupport> {
        self.harnesses.bench_for(&self.harness_command)
    }
}

/// A harness the bench runs through (`Harness::bench`; Claude Code's, opencode's, Codex's):
/// where the project's finished sessions, instruction files and memory files are, how an arm
/// and a tool-free call (the generator, the judge) are sandboxed, and what a run costs.
pub trait BenchSupport: std::fmt::Debug + Send + Sync {
    /// The excerpt of the project's finished sessions, from the harness's own store.
    fn excerpt(&self, project: &str, now: SystemTime) -> Excerpt;
    /// Where its finished sessions of `project` are looked for: what one is called, and where.
    fn sessions_place(&self, project: &str) -> (&'static str, PathBuf);
    /// Up to three other projects (git repositories) with finished sessions, the one with the
    /// most first, to suggest when `project` has none.
    fn other_projects_with_sessions(&self, _project: &str, _now: SystemTime) -> Vec<PathBuf> {
        Vec::new()
    }
    /// The project's instruction files (label, text), the managed block removed.
    fn instructions(&self, project: &str) -> Vec<(String, String)>;
    /// The project's memory directory, when the harness has a file memory and the project one.
    fn memory_dir(&self, _project: &str) -> Option<PathBuf> {
        None
    }
    /// The report's note when the files arm holds the instruction files only, because the
    /// harness has no file memory.
    fn no_file_memory_note(&self) -> Option<&'static str> {
        None
    }
    /// The sandbox of one arm in `arms_dir`.
    fn build_arm(
        &self,
        arm: BenchArm,
        arms_dir: &Path,
        sources: &arms::Sources,
    ) -> Result<arms::Sandbox, String>;
    /// A tool-free call of `command` in `dir` (the generator, the judge): its working
    /// directory, command line and environment; `schema` and `cap` where its CLI takes them.
    fn tool_free_call(
        &self,
        dir: &Path,
        command: Vec<String>,
        schema: Value,
        cap: f64,
    ) -> Result<ToolFreeCall, String>;
    /// The arguments that cap one answer's cost, when its CLI takes them.
    fn cost_cap(&self, _cap: f64) -> Vec<String> {
        Vec::new()
    }
    /// The line that says the run costs nothing, when `command` runs a free or local model.
    fn free_note(&self, _command: &[String]) -> Option<String> {
        None
    }
}

/// What a call is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Generating the questions.
    Generate,
    /// Looking up the references of the user's own questions.
    Reference,
    /// Warming an arm's prompt cache.
    Warmup,
    /// Answering a question.
    Answer,
    /// Judging an answer.
    Judge,
}

/// One harness call.
#[derive(Debug, Clone)]
pub struct Call {
    /// What it is for.
    pub kind: Kind,
    /// The command line (harness command first).
    pub args: Vec<String>,
    /// The prompt (stdin).
    pub input: String,
    /// The working directory.
    pub cwd: PathBuf,
    /// Extra environment (an opencode sandbox's directories).
    pub env: Vec<(String, String)>,
}

/// What a harness answers: `claude -p --output-format json`'s object (or an error object).
pub trait Harness {
    /// Runs one call; never fails (a failure is `{"is_error": true, "result": why}`).
    fn call(&mut self, call: &Call) -> Value;
}

/// The real harness: a child process per call (`crate::harnesses::headless::run`).
pub struct Process;

impl Harness for Process {
    fn call(&mut self, call: &Call) -> Value {
        let mut env = vec![(crate::memory::summaries::HOOK_ENV, "off")];
        env.extend(call.env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        let headless =
            crate::harnesses::headless::headless_of(&call.args).map(|(_, headless)| headless);
        // opencode 2.x gets `--standalone` (its own server, in the sandbox's environment).
        let args = match headless {
            Some(headless) => headless.bench_args(&call.args),
            None => call.args.clone(),
        };
        match crate::harnesses::headless::run(&args, &call.input, &call.cwd, &env, CALL_TIMEOUT) {
            Ok(output) => headless
                .and_then(|headless| headless.bench_answer(&output, &call.args))
                .or_else(|| json_in(&output.stdout))
                .unwrap_or_else(|| {
                    json!({"is_error": true, "result": format!(
                    "the harness answered no JSON (exit {}): {}",
                    output.code.map_or_else(|| "signal".to_string(), |c| c.to_string()),
                    crate::memory::transcript::clip(output.stderr.trim(), 300))})
                }),
            Err(err) => json!({"is_error": true, "result": err}),
        }
    }
}

/// The fake harness of `--dry-run`: canned answers, no process, no cost.
#[derive(Debug, Default)]
pub struct DryRun {
    /// The calls it answered.
    pub calls: Vec<Call>,
}

impl Harness for DryRun {
    fn call(&mut self, call: &Call) -> Value {
        self.calls.push(call.clone());
        let base = json!({"type": "result", "is_error": false, "total_cost_usd": 0.0,
            "num_turns": 1, "duration_ms": 0,
            "usage": {"input_tokens": 0, "output_tokens": 0}});
        let mut answer = base;
        match call.kind {
            Kind::Generate => {
                // Questions on the first turns of the excerpt, as many as asked.
                let count = call
                    .input
                    .split("Write exactly ")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(3);
                let ids: Vec<String> = call
                    .input
                    .match_indices("\n[turn:")
                    .filter_map(|(at, _)| {
                        let rest = &call.input[at + 2..];
                        rest.find(']').map(|end| rest[..end].to_string())
                    })
                    .collect();
                let questions: Vec<Value> = (0..count)
                    .filter_map(|i| {
                        let id = ids.get(i % ids.len().max(1))?;
                        Some(json!({"category": questions::CATEGORIES[i % 3],
                            "question": format!("Dry-run question {} about {id}?", i + 1),
                            "reference": "Dry-run reference.", "sources": [id]}))
                    })
                    .collect();
                answer["structured_output"] = json!({ "questions": questions });
            }
            Kind::Reference => {
                // Every question found, on the first turn of the excerpt.
                let first =
                    call.input.split("\n[turn:").nth(1).and_then(|rest| {
                        rest.find(']').map(|end| format!("turn:{}", &rest[..end]))
                    });
                let answers: Vec<Value> = call
                    .input
                    .match_indices("\n- [")
                    .filter_map(|(at, _)| {
                        let rest = &call.input[at + 4..];
                        let id = &rest[..rest.find(']')?];
                        Some(json!({"id": id, "found": first.is_some(),
                            "reference": "Dry-run reference.", "sources": [first.clone()?]}))
                    })
                    .collect();
                answer["structured_output"] = json!({ "answers": answers });
            }
            Kind::Warmup | Kind::Answer => {
                answer["result"] = json!("Dry run: no model was called.")
            }
            Kind::Judge => {
                answer["structured_output"] =
                    json!({"score": 0, "confident_wrong": false, "note": "dry run"});
            }
        }
        answer
    }
}

/// The JSON object in `text`: the whole text, or from its first `{` to its last `}`.
pub fn json_in(text: &str) -> Option<Value> {
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

/// What one answer cost and produced, read from the harness's JSON.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    /// The answer text.
    pub text: String,
    /// Why it failed, if it did.
    pub error: Option<String>,
    /// `total_cost_usd`.
    pub cost_usd: f64,
    /// Input tokens: plain + cache writes + cache reads.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// `num_turns`.
    pub turns: u64,
    /// `duration_ms`, else the measured wall time.
    pub ms: u64,
}

impl Usage {
    /// The usage in `value`; `wall` is the measured time.
    pub fn of(value: &Value, wall: Duration) -> Self {
        let usage = &value["usage"];
        let count = |key: &str| usage[key].as_u64().unwrap_or(0);
        let text = value["result"].as_str().unwrap_or_default().to_string();
        let error = if value["is_error"] == Value::Bool(true) {
            Some(if text.is_empty() {
                value["subtype"]
                    .as_str()
                    .unwrap_or("the harness reported an error")
                    .to_string()
            } else {
                crate::memory::transcript::clip(&text, 300)
            })
        } else {
            None
        };
        Self {
            text: if error.is_some() { String::new() } else { text },
            error,
            cost_usd: value["total_cost_usd"].as_f64().unwrap_or(0.0),
            input_tokens: count("input_tokens")
                + count("cache_creation_input_tokens")
                + count("cache_read_input_tokens"),
            output_tokens: count("output_tokens"),
            turns: value["num_turns"].as_u64().unwrap_or(0),
            ms: value["duration_ms"]
                .as_u64()
                .filter(|ms| *ms > 0)
                .unwrap_or_else(|| u64::try_from(wall.as_millis()).unwrap_or(u64::MAX)),
        }
    }
}

/// The judge's verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    /// 0-5.
    pub score: u8,
    /// A fact stated that contradicts the reference.
    pub confident_wrong: bool,
    /// One sentence.
    pub note: String,
}

/// The JSON schema of the judge's answer.
pub fn judge_schema() -> Value {
    json!({"type": "object", "properties": {
        "score": {"type": "integer", "minimum": 0, "maximum": 5},
        "confident_wrong": {"type": "boolean"},
        "note": {"type": "string"}}, "required": ["score", "confident_wrong", "note"]})
}

/// The judge's prompt (a reference the developer wrote is named as theirs).
pub fn judge_prompt(question: &Question, answer: &str) -> String {
    let whose = if question.reference_from == ReferenceFrom::User {
        "\nThe reference answer was written by the developer in their own words and may be short: \
judge whether the answer states the same facts."
    } else {
        ""
    };
    format!(
        "Score an assistant's answer against a reference answer.
5 = all key facts of the reference, nothing wrong; 4 = key facts, minor omissions; 3 = about half \
of the key facts; 2 = some relevant facts, main point missing; 1 = barely relevant; 0 = no answer, \
'not found', or wrong.
confident_wrong = true only if the answer states something that contradicts the reference as a \
fact (not for 'I could not find it').
Extra correct detail beyond the reference is fine. Judge facts, not style. note: one sentence.{whose}
Answer with one JSON object matching this JSON schema:
{}

Question: {}
Reference answer: {}
Assistant's answer: {}",
        judge_schema(),
        question.question,
        question.reference.as_deref().unwrap_or("(none)"),
        if answer.trim().is_empty() {
            "(no answer)"
        } else {
            answer
        }
    )
}

/// The verdict in a judge's answer (`structured_output`, or JSON in `result`).
pub fn parse_verdict(value: &Value) -> Result<Verdict, String> {
    if value["is_error"] == Value::Bool(true) {
        return Err(format!(
            "the judge failed: {}",
            value["result"].as_str().unwrap_or("error")
        ));
    }
    let output = if value["structured_output"].is_object() {
        value["structured_output"].clone()
    } else {
        value["result"]
            .as_str()
            .and_then(json_in)
            .ok_or("the judge answered no verdict")?
    };
    let score = output["score"]
        .as_u64()
        .or_else(|| output["score"].as_f64().map(|s| s.round().max(0.0) as u64))
        .ok_or("the judge's verdict has no score")?;
    Ok(Verdict {
        score: u8::try_from(score.min(5)).unwrap_or(5),
        confident_wrong: output["confident_wrong"].as_bool().unwrap_or(false),
        note: output["note"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string(),
    })
}

/// The answer prompt of `question` in run `run_id`.
pub fn answer_prompt(run_id: &str, project: &str, question: &str) -> String {
    format!(
        "[benchmark run {run_id}]
You are an AI coding assistant working with a developer on the project \"{}\". The developer asks \
you a question about the project's rules, decisions or history. Answer it from your memory only \
(no code or repository access in this task). Look up what you need; if your memory does not hold \
the answer, say so plainly instead of guessing.

Question: {question}

Final answer: at most 150 words, concrete (names, versions, commits, numbers where your memory \
has them).",
        project_name(project)
    )
}

fn project_name(project: &str) -> String {
    Path::new(project)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| project.to_string())
}

/// The estimated list-price cost of generating `count` questions from a prompt of `chars`.
pub fn generation_usd(chars: usize, count: usize) -> f64 {
    let input = chars as f64 / 4.0 + 15_000.0;
    let output = count as f64 * 250.0 + 500.0;
    (input * 3.0 + output * 15.0) / 1_000_000.0
}

/// The estimated list-price cost of looking up the references of `count` questions of the
/// user's in a prompt of `chars` (the excerpt is read again; short answers).
pub fn reference_usd(chars: usize, count: usize) -> f64 {
    let input = chars as f64 / 4.0 + 15_000.0;
    let output = count as f64 * 150.0 + 300.0;
    (input * 3.0 + output * 15.0) / 1_000_000.0
}

/// The estimate of a run.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Estimate {
    /// Generating the questions (0 when the project has a set).
    pub generation: f64,
    /// Looking up the references of the user's own questions (0 when none is pending).
    pub references: f64,
    /// Warm-up calls.
    pub warmup: f64,
    /// Answers.
    pub answers: f64,
    /// Judge calls.
    pub judge: f64,
    /// How many answers.
    pub calls: usize,
}

impl Estimate {
    /// The estimate of `questions` x `arms` x `repeats` (+ the generation's).
    pub fn of(questions: usize, arms: usize, repeats: u32, generation: f64) -> Self {
        let calls = questions * arms * repeats as usize;
        Self {
            generation,
            references: 0.0,
            warmup: arms as f64 * ANSWER_USD,
            answers: calls as f64 * ANSWER_USD,
            judge: calls as f64 * JUDGE_USD,
            calls,
        }
    }

    /// Everything.
    pub fn total(&self) -> f64 {
        self.generation + self.references + self.warmup + self.answers + self.judge
    }
}

/// The harness commands the bench runs through, as `--harness-command` takes them (each
/// harness's `Headless::bench_command`).
fn bench_harnesses() -> Vec<&'static str> {
    crate::harnesses::agent::Kind::ALL
        .into_iter()
        .filter_map(|kind| crate::harnesses::driver(kind).headless?.bench_command())
        .collect()
}

/// Refuses a harness that is neither Claude Code's `claude`, opencode's `opencode` nor Codex's
/// `codex` with a local model (the arms are built with their flags and configuration). Codex
/// reports tokens but no cost, so the budget could not stop a run on a paid model: only a
/// local model (`--oss`, $0) is accepted.
#[cfg(test)]
pub fn check_harness(command: &[String]) -> Result<(), String> {
    check_harness_in(command, None)
}

/// [`check_harness`], naming the settings file `config` (its full path) in the refusal.
pub fn check_harness_in(command: &[String], config: Option<&Path>) -> Result<(), String> {
    let (headline, why) = match crate::harnesses::headless::headless_of(command) {
        Some((_, headless)) => match headless.bench_refusal(command) {
            Some(refusal) => refusal,
            None => return Ok(()),
        },
        None => (
            "the bench cannot run through this harness command.",
            "The bench drives Claude Code's `claude -p`, opencode's `opencode run` or Codex's \
             `codex exec --oss` with a local model.",
        ),
    };
    Err(harness_refusal(headline, why, command, config))
}

/// The refusal of a harness command: the headline, the command and where it is set, why, and
/// the commands that run the bench instead, ready to copy.
fn harness_refusal(headline: &str, why: &str, command: &[String], config: Option<&Path>) -> String {
    let width = crate::install::prompt::WIDTH;
    let set_in = match config {
        Some(path) => {
            let shown = path.display().to_string();
            let shown = match crate::runtime::store::home_dir(&|name| std::env::var(name).ok()) {
                Ok(home) => crate::install::tilde(&shown, &home),
                Err(_) => shown,
            };
            format!("summaries.harness_command in {shown}")
        }
        None => "summaries.harness_command in config.json".to_string(),
    };
    let mut lines = vec![headline.to_string(), String::new()];
    lines.push(format!("  harness command: {}", command.join(" ")));
    lines.push(format!("  set in:          {set_in}"));
    lines.push(String::new());
    lines.extend(
        crate::install::prompt::wrap(why, width)
            .into_iter()
            .map(|line| format!("  {line}")),
    );
    lines.push(String::new());
    lines.push("Run the bench through one of these instead:".to_string());
    lines.push(String::new());
    for harness in bench_harnesses() {
        lines.push(format!(
            "  graphersal-memory bench --harness-command \"{harness}\""
        ));
    }
    lines.join("\n")
}

/// The excerpt of the project's finished sessions, from the harness's own store.
fn excerpt_for(options: &Options) -> Excerpt {
    options.bench().excerpt(&options.project, SystemTime::now())
}

/// The working directory, the command line and the environment of a tool-free call.
pub type ToolFreeCall = (PathBuf, Vec<String>, Vec<(String, String)>);

/// A call of the generator or the judge: the harness command, the tool-free sandbox's
/// arguments, and for `claude` the schema and the cost cap of the call.
fn tool_free_call(
    options: &Options,
    dir: &Path,
    schema: Value,
    cap: f64,
) -> Result<ToolFreeCall, String> {
    options
        .bench()
        .tool_free_call(dir, options.harness_command.clone(), schema, cap)
}

/// A short run id: 12 hex digits of the time and the process.
fn run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    crate::fsutil::fingerprint(format!("{nanos}-{}", std::process::id()).as_bytes())
        .chars()
        .take(12)
        .collect()
}

/// A new run directory `<data dir>/bench/<timestamp>`.
fn new_run_dir(data_dir: &Path, started_at: &str) -> Result<PathBuf, String> {
    let stamp: String = started_at
        .chars()
        .filter(|c| *c != ':' && *c != '-')
        .collect();
    let base = data_dir.join("bench");
    let mut dir = base.join(&stamp);
    let mut n = 2;
    while dir.exists() {
        dir = base.join(format!("{stamp}-{n}"));
        n += 1;
    }
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    Ok(dir)
}

/// Generates a question set of `count` questions for `options.project` from `excerpt`.
fn generate(
    options: &Options,
    excerpt: &Excerpt,
    count: usize,
    budget_left: f64,
    work: &Path,
    harness: &mut dyn Harness,
) -> Result<QuestionSet, String> {
    let prompt = questions::prompt(&options.project, count, excerpt);
    let cap = (generation_usd(prompt.len(), count) * 3.0)
        .max(0.5)
        .min(budget_left.max(0.05));
    let (cwd, command, env) = tool_free_call(options, work, questions::schema(), cap)?;
    let value = harness.call(&Call {
        kind: Kind::Generate,
        args: command,
        input: prompt,
        cwd,
        env,
    });
    // The raw answer stays beside the run, to see what a failed generation answered.
    let raw = work.join("answer.json");
    let _ = crate::fsutil::write_atomic(
        &raw,
        format!(
            "{}\n",
            crate::install::jsonedit::render(&value, Some("  "), 0)
        )
        .as_bytes(),
    );
    let cost = value["total_cost_usd"].as_f64().unwrap_or(0.0);
    if value["is_error"] == Value::Bool(true) {
        return Err(format!(
            "generating the questions failed (${cost:.3} spent; the answer is in {}): {}",
            raw.display(),
            value["result"].as_str().unwrap_or("error")
        ));
    }
    let questions = questions::parse(&value, excerpt, count).map_err(|err| {
        format!(
            "{err} (${cost:.3} spent; the answer is in {})",
            raw.display()
        )
    })?;
    Ok(QuestionSet {
        project: options.project.clone(),
        generated_at: crate::runtime::daemon::now_utc(),
        sessions: excerpt.sessions,
        cost_usd: cost,
        confirmed: false,
        questions,
    })
}

/// Looks up the references of the user's `pending` questions (`(id, text)`) in `excerpt` (the
/// same raw transcripts the generator reads) and applies them to `set`. Returns the cost.
fn find_references(
    options: &Options,
    excerpt: &Excerpt,
    set: &mut QuestionSet,
    pending: &[(String, String)],
    budget_left: f64,
    work: &Path,
    harness: &mut dyn Harness,
) -> Result<f64, String> {
    let prompt = questions::reference_prompt(&options.project, pending, excerpt);
    let cap = (reference_usd(prompt.len(), pending.len()) * 3.0)
        .max(0.5)
        .min(budget_left.max(0.05));
    let (cwd, command, env) = tool_free_call(options, work, questions::reference_schema(), cap)?;
    let value = harness.call(&Call {
        kind: Kind::Reference,
        args: command,
        input: prompt,
        cwd,
        env,
    });
    let raw = work.join("references.json");
    let _ = crate::fsutil::write_atomic(
        &raw,
        format!(
            "{}\n",
            crate::install::jsonedit::render(&value, Some("  "), 0)
        )
        .as_bytes(),
    );
    let cost = value["total_cost_usd"].as_f64().unwrap_or(0.0);
    if value["is_error"] == Value::Bool(true) {
        return Err(format!(
            "looking up the references of your questions failed (${cost:.3} spent; the answer \
             is in {}): {}",
            raw.display(),
            value["result"].as_str().unwrap_or("error")
        ));
    }
    let ids: Vec<String> = pending.iter().map(|(id, _)| id.clone()).collect();
    let found = questions::parse_references(&value, excerpt, &ids).map_err(|err| {
        format!(
            "{err} (${cost:.3} spent; the answer is in {})",
            raw.display()
        )
    })?;
    set.apply_references(found);
    Ok(cost)
}

/// What `--review` and the samples print about a question's reference.
fn reference_line(q: &Question) -> String {
    match (q.reference_from, &q.reference) {
        (ReferenceFrom::User, Some(r)) => format!("reference (your answer): {r}"),
        (_, Some(r)) => format!("reference: {r}"),
        (ReferenceFrom::NotFound, None) => "reference: none found in your sessions (asked, but \
             not scored; write one into the file to score it)"
            .to_string(),
        (_, None) => "reference: not looked up yet (the next `bench questions` or `bench` \
             looks it up in your sessions)"
            .to_string(),
    }
}

fn show_questions(set: &QuestionSet, limit: usize, out: &mut dyn Write) {
    for q in set.kept().iter().take(limit) {
        let label = match q.origin {
            Origin::Generated => q.category.clone(),
            Origin::User => format!("yours, {}", q.category),
        };
        let _ = writeln!(
            out,
            "  {} [{label}] {}\n     {}",
            q.id,
            q.question,
            reference_line(q)
        );
        if !q.sources.is_empty() {
            let _ = writeln!(out, "     sources: {}", q.sources.join(", "));
        }
        for excerpt in &q.excerpts {
            let _ = writeln!(
                out,
                "       > {}",
                crate::memory::transcript::clip(excerpt, 160)
            );
        }
    }
}

/// The interactive step: the user types their own questions (one per line, an empty line
/// ends), each with an optional answer. Returns how many were added (saved by the caller).
fn ask_own_questions(
    options: &Options,
    set: &mut QuestionSet,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> usize {
    let mut console = crate::install::prompt::Console::new(input, out, options.color);
    let (mine, _) = set.count(Origin::User);
    let have = if mine > 0 {
        format!(
            "You have {mine} question(s) of your own already; `graphersal-memory bench questions \
             --review` shows them."
        )
    } else {
        String::new()
    };
    let mut explanation = vec![
        "The bench asks every memory the same questions and scores the answers. Besides the \
         questions it writes from your past sessions, you can add your own: the things you want \
         the assistant to remember.",
        "Type one question per line and press Enter. You can then type the right answer, or just \
         press Enter to let the bench look it up in your past sessions. An empty question line \
         finishes this step.",
    ];
    if !have.is_empty() {
        explanation.push(&have);
    }
    console.step(1, 1, "Add your own questions? (optional)", &explanation);
    let mut added = 0;
    loop {
        let _ = write!(console.out(), "Question (or Enter to finish): ");
        let Some(question) = console.read_answer() else {
            break;
        };
        if question.is_empty() {
            break;
        }
        let _ = write!(console.out(), "  Right answer (optional, Enter to skip): ");
        let answer = console.read_answer().unwrap_or_default();
        let answer = Some(answer.as_str()).filter(|a| !a.is_empty());
        match set.add_user(&question, answer, None) {
            Ok(id) => {
                added += 1;
                let done = console.green(&format!("  Added as {id}."));
                let _ = writeln!(console.out(), "{done}");
            }
            Err(existing) => {
                let _ = writeln!(
                    console.out(),
                    "  Already in the set as {existing}; not added again."
                );
            }
        }
    }
    let _ = writeln!(console.out());
    added
}

/// What `bench questions add`, `import` and `remove` do.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// Add one question of the user's, with an optional answer and category.
    Add {
        /// The question.
        question: String,
        /// The expected answer (the reference).
        answer: Option<String>,
        /// `rule`, `decision` or `history`.
        category: Option<String>,
    },
    /// Add the questions of a file (plain text, `Q:`/`A:` pairs, or JSON).
    Import {
        /// The file.
        file: PathBuf,
    },
    /// Remove one question by its id (`U2`, `Q3`).
    Remove {
        /// The id.
        id: String,
    },
}

/// `bench questions add|import|remove`: edits the project's question set (no model, no cost).
pub fn edit_command(options: &Options, edit: &Edit, out: &mut dyn Write) -> Result<u8, String> {
    let file = questions::path(&options.data_dir, &options.project);
    let mut set = QuestionSet::load(&options.data_dir, &options.project)?
        .unwrap_or_else(|| QuestionSet::empty(&options.project));
    match edit {
        Edit::Add {
            question,
            answer,
            category,
        } => {
            if question.trim().is_empty() {
                return Err("the question is empty".to_string());
            }
            match set.add_user(question, answer.as_deref(), category.as_deref()) {
                Ok(id) => {
                    set.save_to(&file)?;
                    let how = if answer.as_deref().is_some_and(|a| !a.trim().is_empty()) {
                        "your answer is its reference"
                    } else {
                        "its reference is looked up in your past sessions by the next `bench \
                         questions` or `bench`, whose estimate includes that call"
                    };
                    let _ = writeln!(out, "Added {id} to {} ({how}).", file.display());
                }
                Err(existing) => {
                    let _ = writeln!(
                        out,
                        "Already in the set as {existing}; nothing added ({}).",
                        file.display()
                    );
                }
            }
        }
        Edit::Import { file: source } => {
            let bytes = std::fs::read(source)
                .map_err(|err| format!("cannot read {}: {err}", source.display()))?;
            let text = String::from_utf8(bytes)
                .map_err(|_| format!("{} is not UTF-8 text", source.display()))?;
            let (entries, mut skipped) = questions::parse_import(&text)?;
            let mut added = Vec::new();
            for entry in entries {
                match set.add_user(
                    &entry.question,
                    entry.answer.as_deref(),
                    entry.category.as_deref(),
                ) {
                    Ok(id) => added.push(id),
                    Err(existing) => skipped.push(format!(
                        "duplicate of {existing}: {}",
                        crate::memory::transcript::clip(&entry.question, 60)
                    )),
                }
            }
            if !added.is_empty() {
                set.save_to(&file)?;
            }
            let _ = writeln!(
                out,
                "Added {} question(s){} to {}; skipped {}.",
                added.len(),
                if added.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", added.join(", "))
                },
                file.display(),
                skipped.len()
            );
            for why in &skipped {
                let _ = writeln!(out, "  skipped: {why}");
            }
            let pending = set
                .questions
                .iter()
                .filter(|q| added.contains(&q.id) && q.reference_from == ReferenceFrom::Pending)
                .count();
            if pending > 0 {
                let _ = writeln!(
                    out,
                    "{pending} of them without an answer: their references are looked up in \
                     your past sessions by the next `bench questions` or `bench`."
                );
            }
        }
        Edit::Remove { id } => {
            let wanted = id.trim().to_ascii_uppercase();
            let Some(at) = set
                .questions
                .iter()
                .position(|q| q.id.eq_ignore_ascii_case(&wanted))
            else {
                return Err(format!(
                    "no question {wanted} in {} (`bench questions --review` lists the ids)",
                    file.display()
                ));
            };
            let removed = set.questions.remove(at);
            set.save_to(&file)?;
            let _ = writeln!(
                out,
                "Removed {}: {}",
                removed.id,
                crate::memory::transcript::clip(&removed.question, 80)
            );
        }
    }
    Ok(0)
}

/// `bench questions`: generate a new set (`review` false) or show and confirm the current one.
/// A new generation replaces the generated questions and keeps the user's; the references of
/// the user's questions without one are looked up in the same call flow.
pub fn questions_command(
    options: &Options,
    review: bool,
    harness: &mut dyn Harness,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<u8, String> {
    let file = questions::path(&options.data_dir, &options.project);
    if review {
        let Some(mut set) = QuestionSet::load(&options.data_dir, &options.project)? else {
            return Err(format!(
                "no question set for {} yet: run `graphersal-memory bench questions` first",
                options.project
            ));
        };
        let (mine, _) = set.count(Origin::User);
        let (generated, _) = set.count(Origin::Generated);
        let _ = writeln!(
            out,
            "Question set of {} ({} of {} kept{}{}; {}):",
            options.project,
            set.kept().len(),
            set.questions.len(),
            if mine > 0 {
                format!(", {mine} of them yours")
            } else {
                String::new()
            },
            if generated > 0 {
                format!(
                    ", generated {} from {} sessions",
                    set.generated_at, set.sessions
                )
            } else {
                String::new()
            },
            if set.confirmed {
                "confirmed"
            } else {
                "not confirmed yet"
            }
        );
        show_questions(&set, usize::MAX, out);
        let _ = writeln!(
            out,
            "\nThe file: {}\nFix a reference there or set \"keep\": false to drop a question, then \
             run this again.",
            file.display()
        );
        if set.kept().is_empty() {
            return Err("no question is kept".to_string());
        }
        if !options.yes
            && !crate::install::confirm("Confirm these questions for the bench?", input, out)
        {
            let _ = writeln!(out, "Not confirmed.");
            return Ok(EXIT_FAILED);
        }
        set.confirmed = true;
        set.save_to(&file)?;
        let _ = writeln!(out, "Confirmed.");
        return Ok(0);
    }
    check_harness_in(
        &options.harness_command,
        Some(&options.data_dir.join(crate::config::CONFIG_FILE)),
    )?;
    let mut set = QuestionSet::load(&options.data_dir, &options.project)?
        .unwrap_or_else(|| QuestionSet::empty(&options.project));
    // Generating needs finished sessions: say so before asking for the user's questions.
    let mut excerpt = None;
    if options.questions > 0 {
        let found = excerpt_for(options);
        if found.sessions == 0 {
            return Err(no_transcripts(options, "bench questions"));
        }
        excerpt = Some(found);
    }
    if options.interactive && ask_own_questions(options, &mut set, input, out) > 0 {
        set.save_to(&file)?;
    }
    let pending = set.pending();
    if options.questions == 0 && pending.is_empty() {
        let _ = writeln!(
            out,
            "Nothing to generate (--questions 0) and no question of yours waits for a reference. \
             `graphersal-memory bench questions --review` shows the set."
        );
        return Ok(0);
    }
    let excerpt = excerpt.unwrap_or_else(|| excerpt_for(options));
    if excerpt.sessions == 0 {
        return Err(no_transcripts(options, "bench questions"));
    }
    let free = crate::harnesses::headless::costs_nothing(&options.harness_command);
    let price = if free { 0.0 } else { 1.0 };
    let generation = if options.questions > 0 {
        price
            * generation_usd(
                questions::prompt(&options.project, options.questions, &excerpt).len(),
                options.questions,
            )
    } else {
        0.0
    };
    let references = if pending.is_empty() {
        0.0
    } else {
        price
            * reference_usd(
                questions::reference_prompt(&options.project, &pending, &excerpt).len(),
                pending.len(),
            )
    };
    let estimate = generation + references;
    let what = match (options.questions, pending.len()) {
        (n, 0) => format!("Generating {n} questions about {}", options.project),
        (0, k) => format!(
            "Looking up the references of {k} question(s) of yours about {}",
            options.project
        ),
        (n, k) => format!(
            "Generating {n} questions about {} and looking up the references of {k} of yours",
            options.project
        ),
    };
    let _ = writeln!(
        out,
        "{what} from {} finished session(s) (raw transcripts): about ${estimate:.2} at list price \
         (budget ${:.2}).",
        excerpt.sessions, options.budget_usd
    );
    if estimate > options.budget_usd {
        let _ = writeln!(out, "Refused: the estimate is above the budget (--budget).");
        return Ok(EXIT_FAILED);
    }
    if options.dry_run {
        let _ = writeln!(out, "Dry run: a fake harness answers; nothing is spent.");
    } else if !options.yes
        && !crate::install::confirm(
            if options.questions > 0 {
                "Generate them?"
            } else {
                "Look them up?"
            },
            input,
            out,
        )
    {
        let _ = writeln!(out, "Nothing was generated or looked up.");
        return Ok(EXIT_FAILED);
    }
    let work =
        new_run_dir(&options.data_dir, &crate::runtime::daemon::now_utc())?.join("questions");
    let mut spent = 0.0;
    if options.questions > 0 {
        let generated = generate(
            options,
            &excerpt,
            options.questions,
            options.budget_usd,
            &work,
            harness,
        )?;
        spent += generated.cost_usd;
        set.generated_at = generated.generated_at;
        set.sessions = generated.sessions;
        set.cost_usd = generated.cost_usd;
        set.confirmed = false;
        set.replace_generated(generated.questions);
    }
    if !pending.is_empty() {
        spent += find_references(
            options,
            &excerpt,
            &mut set,
            &pending,
            options.budget_usd - spent,
            &work,
            harness,
        )?;
    }
    let target = if options.dry_run {
        file.with_extension("dry-run.json")
    } else {
        file
    };
    set.save_to(&target)?;
    show_questions(&set, usize::MAX, out);
    let _ = writeln!(
        out,
        "\n{} questions written to {} (${:.3} spent). Review them: `graphersal-memory bench \
         questions --review`.",
        set.questions.len(),
        target.display(),
        spent
    );
    Ok(0)
}

/// Why the bench has nothing to write questions from, where it looked, and what to run
/// instead (`command`: `bench` or `bench questions`), ready to copy.
fn no_transcripts(options: &Options, command: &str) -> String {
    let (what, place) = options.bench().sessions_place(&options.project);
    let home = crate::runtime::store::home_dir(&|name| std::env::var(name).ok()).ok();
    let shown = |path: &Path| {
        let text = path.display().to_string();
        match &home {
            Some(home) => crate::install::tilde(&text, home),
            None => text,
        }
    };
    let flag = harness_flag(&options.harness_command);
    let mut lines = vec![
        format!("no finished {what} of {} yet.", options.project),
        String::new(),
        format!("  looked in: {}", shown(&place)),
        String::new(),
    ];
    lines.extend(
        crate::install::prompt::wrap(
            "The bench writes its questions from the project's finished sessions. A session \
             counts once it has been unchanged for 30 minutes, so the one you are in now is \
             left out.",
            crate::install::prompt::WIDTH,
        )
        .into_iter()
        .map(|line| format!("  {line}")),
    );
    lines.push(String::new());
    lines.push("What you can do:".to_string());
    lines.push(String::new());
    let others = options
        .bench()
        .other_projects_with_sessions(&options.project, SystemTime::now());
    if let Some(first) = others.first() {
        lines.push("  Run it in a project with finished sessions, for example:".to_string());
        lines.push(String::new());
        lines.push(format!(
            "    cd {}",
            shell_word(&first.display().to_string())
        ));
        lines.push(format!("    graphersal-memory {command}{flag}"));
        lines.push(String::new());
        if others.len() > 1 {
            let rest: Vec<String> = others[1..].iter().map(|dir| shown(dir)).collect();
            lines.push(format!(
                "  Other projects with finished sessions: {}",
                rest.join(", ")
            ));
            lines.push(String::new());
        }
    } else {
        lines.push("  Run it in a project with finished sessions:".to_string());
        lines.push(String::new());
        lines.push("    cd <project>".to_string());
        lines.push(format!("    graphersal-memory {command}{flag}"));
        lines.push(String::new());
    }
    lines.push("  Or ask only your own questions here, each with its answer:".to_string());
    lines.push(String::new());
    lines.push(
        "    graphersal-memory bench questions add \"<question>\" --answer \"<answer>\""
            .to_string(),
    );
    lines.push(format!("    graphersal-memory bench --questions 0{flag}"));
    lines.join("\n")
}

/// ` --harness-command "<command>"` for a command whose words copy back unchanged, else empty
/// (the settings' command is used then).
fn harness_flag(command: &[String]) -> String {
    let plain = command.iter().all(|word| {
        !word.is_empty()
            && !word
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '$' | '`' | '\\'))
    });
    if plain && !command.is_empty() {
        format!(" --harness-command \"{}\"", command.join(" "))
    } else {
        String::new()
    }
}

/// `text` as one shell word: as it is when it needs no quoting, else in double quotes.
fn shell_word(text: &str) -> String {
    let plain = text
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '/' | '\\' | '.' | '_' | '-' | ':' | '~'));
    if plain {
        text.to_string()
    } else if cfg!(windows) {
        format!("\"{text}\"")
    } else {
        let escaped: String = text
            .chars()
            .flat_map(|c| match c {
                '"' | '$' | '`' | '\\' => vec!['\\', c],
                c => vec![c],
            })
            .collect();
        format!("\"{escaped}\"")
    }
}

/// `bench`: plans, asks, runs every arm, writes the report. Returns the exit code.
pub fn run(
    options: &Options,
    harness: &mut dyn Harness,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<u8, String> {
    check_harness_in(
        &options.harness_command,
        Some(&options.data_dir.join(crate::config::CONFIG_FILE)),
    )?;
    if options.arms.is_empty() {
        return Err("no arm to run (--arms none,files,graph)".to_string());
    }
    let mut arms_list: Vec<BenchArm> = Vec::new();
    for arm in &options.arms {
        if !arms_list.contains(arm) {
            arms_list.push(*arm);
        }
    }
    if arms_list.contains(&BenchArm::Current) {
        return Err(
            "the arm `current` (a copy of your own MCP setup) is planned, not built yet; \
                    use none, files and graph"
                .to_string(),
        );
    }
    let store_dir = crate::runtime::store::default_store_dir(&options.data_dir);
    let no_store = std::fs::read_dir(&store_dir).map_or(true, |mut e| e.next().is_none());
    if arms_list.contains(&BenchArm::Graph) && no_store {
        return Err(format!(
            "there is no memory store in {} yet: run `graphersal-memory install` (or `import` \
             and `import --sessions`) first",
            options.data_dir.display()
        ));
    }
    let file = questions::path(&options.data_dir, &options.project);
    let loaded = QuestionSet::load(&options.data_dir, &options.project)?;
    let mut set = loaded.unwrap_or_else(|| QuestionSet::empty(&options.project));
    let (_, generated_kept) = set.count(Origin::Generated);
    // Generated questions: the kept ones, else a new generation of --questions; none for 0.
    let generate_count = if options.questions > 0 && generated_kept == 0 {
        options.questions
    } else {
        0
    };
    // Generating needs finished sessions: say so before asking for the user's questions.
    let mut found = None;
    if generate_count > 0 {
        let excerpt = excerpt_for(options);
        if excerpt.sessions == 0 {
            return Err(no_transcripts(options, "bench"));
        }
        found = Some(excerpt);
    }
    if options.interactive && ask_own_questions(options, &mut set, input, out) > 0 {
        set.save_to(&file)?;
    }
    let (_, user_kept) = set.count(Origin::User);
    let asked_generated = match options.questions {
        0 => 0,
        _ if generate_count > 0 => generate_count,
        n => generated_kept.min(n),
    };
    let pending = set.pending();
    let excerpt = match found {
        Some(excerpt) => excerpt,
        None if !pending.is_empty() => excerpt_for(options),
        None => Excerpt::default(),
    };
    // Without transcripts a pending reference cannot be looked up: asked, not judged.
    let can_look_up = excerpt.sessions > 0 && !pending.is_empty();
    let count = user_kept + asked_generated;
    if count == 0 {
        return Err(format!(
            "no question to ask: add your own with `graphersal-memory bench questions add \
             \"<question>\"`, or let the bench generate some with --questions N (the question \
             file: {})",
            file.display()
        ));
    }
    let unjudged = set
        .kept()
        .iter()
        .filter(|q| {
            q.reference_from == ReferenceFrom::NotFound
                || (q.reference_from == ReferenceFrom::Pending && !can_look_up)
        })
        .count();
    let generation = if generate_count > 0 {
        generation_usd(
            questions::prompt(&options.project, generate_count, &excerpt).len(),
            generate_count,
        )
    } else {
        0.0
    };
    let mut estimate = Estimate::of(count, arms_list.len(), options.repeats, generation);
    if can_look_up {
        estimate.references = reference_usd(
            questions::reference_prompt(&options.project, &pending, &excerpt).len(),
            pending.len(),
        );
    }
    // A question without a reference is answered but not judged.
    estimate.judge =
        (count - unjudged) as f64 * arms_list.len() as f64 * f64::from(options.repeats) * JUDGE_USD;
    let free = crate::harnesses::headless::costs_nothing(&options.harness_command);
    if free {
        estimate = Estimate {
            calls: estimate.calls,
            ..Estimate::default()
        };
    }
    let names: Vec<&str> = arms_list.iter().map(|a| a.name()).collect();
    let mine = if user_kept > 0 {
        format!(
            " ({user_kept} of yours, {asked_generated} generated{})",
            if unjudged > 0 {
                format!("; {unjudged} without a reference, answered but not scored")
            } else {
                String::new()
            }
        )
    } else {
        String::new()
    };
    let _ = writeln!(
        out,
        "bench of {}: arms {}; {} question(s){mine}{}; {} repeat(s).",
        options.project,
        names.join(", "),
        count,
        if generate_count > 0 {
            format!(" to generate from {} finished session(s)", excerpt.sessions)
        } else {
            format!(
                " from {} ({})",
                file.display(),
                if set.confirmed {
                    "confirmed"
                } else {
                    "not confirmed yet"
                }
            )
        },
        options.repeats
    );
    if user_kept > 0 && generate_count > 0 {
        let _ = writeln!(
            out,
            "{generate_count} questions are generated and added to your {user_kept}."
        );
    }
    if !pending.is_empty() && !can_look_up {
        let _ = writeln!(
            out,
            "No finished session to look up the references of {} question(s) of yours in: they \
             are answered but not scored this time.",
            pending.len()
        );
    }
    if free && let Some(note) = options.bench().free_note(&options.harness_command) {
        let _ = writeln!(out, "{note}");
    }
    let _ = writeln!(
        out,
        "Estimate (list price): about ${:.2}: questions ${:.2}{}, warm-up ${:.2}, {} answers \
         ${:.2}, judge ${:.2}. Budget ${:.2}.",
        estimate.total(),
        estimate.generation,
        if can_look_up {
            format!(", references of your questions ${:.2}", estimate.references)
        } else {
            String::new()
        },
        estimate.warmup,
        estimate.calls,
        estimate.answers,
        estimate.judge,
        options.budget_usd
    );
    if estimate.total() > options.budget_usd {
        let _ = writeln!(
            out,
            "Refused: the estimate is above the budget. Raise it with --budget, or use fewer \
             --questions, --repeats or --arms."
        );
        return Ok(EXIT_FAILED);
    }
    if options.dry_run {
        let _ = writeln!(out, "Dry run: a fake harness answers; nothing is spent.");
    } else if !options.yes
        && !crate::install::confirm(
            &format!(
                "Run it through `{}` on your account?",
                options.harness_command.join(" ")
            ),
            input,
            out,
        )
    {
        let _ = writeln!(out, "Nothing was run.");
        return Ok(EXIT_FAILED);
    }

    let started_at = crate::runtime::daemon::now_utc();
    let run_dir = new_run_dir(&options.data_dir, &started_at)?;
    let mut spent = Spent::default();
    let mut changed = false;
    if generate_count > 0 {
        let generated = generate(
            options,
            &excerpt,
            generate_count,
            options.budget_usd,
            &run_dir.join("questions"),
            harness,
        )?;
        spent.generation = generated.cost_usd;
        set.generated_at = generated.generated_at;
        set.sessions = generated.sessions;
        set.cost_usd = generated.cost_usd;
        set.confirmed = false;
        let n = generated.questions.len();
        set.replace_generated(generated.questions);
        let _ = writeln!(out, "{n} questions generated (${:.3}).", spent.generation);
        changed = true;
    }
    if can_look_up {
        spent.references = find_references(
            options,
            &excerpt,
            &mut set,
            &pending,
            options.budget_usd - spent.total(),
            &run_dir.join("questions"),
            harness,
        )?;
        let found = pending
            .iter()
            .filter(|(id, _)| {
                set.questions
                    .iter()
                    .any(|q| &q.id == id && q.reference_from == ReferenceFrom::Transcripts)
            })
            .count();
        let _ = writeln!(
            out,
            "References of your questions: {found} of {} found in your sessions (${:.3}){}.",
            pending.len(),
            spent.references,
            if found < pending.len() {
                "; the others are answered but not scored"
            } else {
                ""
            }
        );
        changed = true;
    }
    if changed {
        let target = if options.dry_run {
            run_dir.join("questions.json")
        } else {
            file.clone()
        };
        set.save_to(&target)?;
        let _ = writeln!(out, "The question set is in {}.", target.display());
    }
    if !set.confirmed && !options.dry_run && !options.yes {
        let _ = writeln!(out, "A sample of the questions:");
        show_questions(&set, 3, out);
        if crate::install::confirm(
            "Use these questions (all are in the file above)?",
            input,
            out,
        ) {
            set.confirmed = true;
            set.save_to(&file)?;
        } else {
            let _ = writeln!(
                out,
                "Review them with `graphersal-memory bench questions --review`, then run the \
                 bench again."
            );
            return Ok(EXIT_FAILED);
        }
    }
    let sources = arms::Sources {
        project: options.project.clone(),
        instructions: options.bench().instructions(&options.project),
        memory_dir: options.bench().memory_dir(&options.project),
        store_dir,
        exe: options.exe.clone(),
    };
    let arms_dir = run_dir.join("arms");
    let mut sandboxes = Vec::new();
    for arm in &arms_list {
        sandboxes.push(options.bench().build_arm(*arm, &arms_dir, &sources)?);
    }
    let judge_dir = run_dir.join("judge");
    let id = run_id();
    let mut meta = Meta {
        started_at,
        run_id: id.clone(),
        project: options.project.clone(),
        harness: options.harness_command.join(" "),
        arms: sandboxes
            .iter()
            .map(|s| (s.arm.name().to_string(), s.description.clone()))
            .collect(),
        questions: 0,
        repeats: options.repeats,
        sessions: set.sessions,
        confirmed: set.confirmed,
        dry_run: options.dry_run,
        stopped: None,
        notes: Vec::new(),
        ..Meta::default()
    };
    if let Some(note) = options.bench().no_file_memory_note()
        && arms_list.contains(&BenchArm::Files)
    {
        meta.notes.push(note.to_string());
    } else if sources.memory_dir.is_none() && arms_list.contains(&BenchArm::Files) {
        meta.notes.push(
            "The project has no memory directory: the files arm holds the instruction files only."
                .to_string(),
        );
    }
    let answer_call = |sandbox: &arms::Sandbox, kind: Kind, prompt: String, cap: f64| {
        let mut args = options.harness_command.clone();
        args.extend(sandbox.args.iter().cloned());
        args.extend(options.bench().cost_cap(cap));
        Call {
            kind,
            args,
            input: prompt,
            cwd: sandbox.cwd.clone(),
            env: sandbox.env.clone(),
        }
    };
    let left = |spent: &Spent| options.budget_usd - spent.total();
    let _ = writeln!(out, "Run {id} in {}", run_dir.display());

    // Warm-up: the same prompt-cache state for every arm.
    for sandbox in &sandboxes {
        if left(&spent) < ANSWER_USD {
            meta.stopped = Some("the budget was reached during the warm-up".to_string());
            break;
        }
        let prompt = format!("[benchmark run {id} warm-up]\nReply with the single word: ready.");
        let call = answer_call(
            sandbox,
            Kind::Warmup,
            prompt,
            ANSWER_CAP_USD.min(left(&spent)),
        );
        let value = harness.call(&call);
        spent.warmup += value["total_cost_usd"].as_f64().unwrap_or(0.0);
    }

    let mut rows: Vec<Row> = Vec::new();
    // The user's questions always; the first --questions N kept generated ones (none for 0).
    let kept: Vec<Question> = {
        let mut generated = 0;
        set.kept()
            .into_iter()
            .filter(|q| {
                q.origin == Origin::User || {
                    generated += 1;
                    generated <= options.questions
                }
            })
            .cloned()
            .collect()
    };
    meta.questions = kept.len();
    meta.user_questions = kept.iter().filter(|q| q.origin == Origin::User).count();
    meta.unjudged = kept
        .iter()
        .filter(|q| q.reference.is_none())
        .map(|q| (q.id.clone(), q.question.clone()))
        .collect();
    meta.user_references = kept
        .iter()
        .filter(|q| q.reference_from == ReferenceFrom::User)
        .count();
    'run: for repeat in 1..=options.repeats {
        for question in &kept {
            for sandbox in &sandboxes {
                if meta.stopped.is_some() {
                    break 'run;
                }
                if left(&spent) < ANSWER_USD + JUDGE_USD {
                    meta.stopped = Some(format!(
                        "the budget of ${:.2} was reached after {} answer(s)",
                        options.budget_usd,
                        rows.len()
                    ));
                    break 'run;
                }
                let prompt = answer_prompt(
                    &format!("{id}-r{repeat}"),
                    &options.project,
                    &question.question,
                );
                let call = answer_call(
                    sandbox,
                    Kind::Answer,
                    prompt,
                    ANSWER_CAP_USD.min(left(&spent) - JUDGE_USD),
                );
                let started = Instant::now();
                let value = harness.call(&call);
                let usage = Usage::of(&value, started.elapsed());
                spent.answers += usage.cost_usd;
                // A question without a reference is answered, shown, and never judged.
                let (verdict, judge_error, judge_cost) = if question.reference.is_none() {
                    (
                        Verdict {
                            score: 0,
                            confident_wrong: false,
                            note: "not judged: no reference".to_string(),
                        },
                        None,
                        0.0,
                    )
                } else {
                    let (judge_cwd, judge, judge_env) = tool_free_call(
                        options,
                        &judge_dir,
                        judge_schema(),
                        JUDGE_CAP_USD.min(left(&spent).max(0.01)),
                    )?;
                    let verdict_value = harness.call(&Call {
                        kind: Kind::Judge,
                        args: judge,
                        input: judge_prompt(question, &usage.text),
                        cwd: judge_cwd,
                        env: judge_env,
                    });
                    let judge_cost = verdict_value["total_cost_usd"].as_f64().unwrap_or(0.0);
                    spent.judge += judge_cost;
                    let (verdict, judge_error) = match parse_verdict(&verdict_value) {
                        Ok(verdict) => (verdict, None),
                        Err(err) => (
                            Verdict {
                                score: 0,
                                confident_wrong: false,
                                note: "judge failed".to_string(),
                            },
                            Some(err),
                        ),
                    };
                    (verdict, judge_error, judge_cost)
                };
                let row = Row {
                    question: question.id.clone(),
                    category: question.category.clone(),
                    origin: question.origin.name().to_string(),
                    judged: question.reference.is_some(),
                    arm: sandbox.arm.name().to_string(),
                    repeat,
                    score: verdict.score,
                    confident_wrong: verdict.confident_wrong,
                    note: verdict.note,
                    answer: usage.text.clone(),
                    error: usage.error.clone().or(judge_error),
                    cost_usd: usage.cost_usd,
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    turns: usage.turns,
                    ms: usage.ms,
                    judge_cost_usd: judge_cost,
                };
                let _ = writeln!(
                    out,
                    "  {} r{} {:<5} score {} ${:.3} {} turns{}",
                    row.question,
                    repeat,
                    row.arm,
                    if row.judged {
                        row.score.to_string()
                    } else {
                        "-".to_string()
                    },
                    row.cost_usd,
                    row.turns,
                    row.error
                        .as_deref()
                        .map(|e| format!(" (failed: {})", crate::memory::transcript::clip(e, 80)))
                        .unwrap_or_default()
                );
                let _ = out.flush();
                rows.push(row);
            }
        }
    }

    for sandbox in &sandboxes {
        if let Some(scratch) = &sandbox.scratch {
            let _ = std::fs::remove_dir_all(scratch);
        }
    }
    let markdown = report::markdown(&meta, &rows, &spent);
    crate::fsutil::write_atomic(&run_dir.join("report.md"), markdown.as_bytes())?;
    let value = report::to_json(&meta, &rows, &spent);
    crate::fsutil::write_atomic(
        &run_dir.join("report.json"),
        format!(
            "{}\n",
            crate::install::jsonedit::render(&value, Some("  "), 0)
        )
        .as_bytes(),
    )?;
    let _ = writeln!(
        out,
        "\n{}\n\nReport: {}",
        report::summary_styled(&meta, &rows, &spent, options.color),
        run_dir.join("report.md").display()
    );
    if let Some(why) = &meta.stopped {
        let _ = writeln!(out, "Stopped early: {why}.");
        return Ok(EXIT_FAILED);
    }
    Ok(0)
}
