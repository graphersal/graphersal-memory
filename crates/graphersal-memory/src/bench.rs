//! `graphersal-memory bench`: the built-in A/B on the user's own memory, through Claude Code's
//! `claude -p` or opencode's `opencode run` (whichever `summaries.harness_command` names; the
//! opencode arms run in sandboxes of their own configuration and data directories,
//! `bench::arms::build_opencode`).
//!
//! 1. **Questions** (`bench::questions`): generated once per project by the harness from the
//!    user's raw transcripts, with reference answers and source turn ids; the user reviews the
//!    file (`bench questions --review`) or accepts it with `--yes`.
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
pub mod questions;
pub mod report;

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

use crate::cli::BenchArm;
use crate::commands::EXIT_FAILED;
use questions::{Excerpt, Question, QuestionSet};
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
    /// Questions to generate when the project has none.
    pub questions: usize,
    /// Repeats.
    pub repeats: u32,
    /// The budget, USD.
    pub budget_usd: f64,
    /// A dry run (the caller passes [`DryRun`]).
    pub dry_run: bool,
    /// Do not ask.
    pub yes: bool,
    /// The project key.
    pub project: String,
    /// Where the memory files, instruction files and transcripts are read from.
    pub claude: crate::install::ClaudePaths,
    /// Where opencode's instruction files and sessions are read from (when the harness command
    /// is opencode's).
    pub opencode: crate::opencode::OpenCodePaths,
    /// Where Codex's instruction files, sessions, model settings and login are read from (when
    /// the harness command is Codex's).
    pub codex: crate::codex::CodexPaths,
    /// The data directory (the live store is forked from it; reports go into it).
    pub data_dir: PathBuf,
    /// This binary (the graph arm's MCP server).
    pub exe: PathBuf,
    /// The harness command (`claude -p --model sonnet`).
    pub harness_command: Vec<String>,
}

/// What a call is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Generating the questions.
    Generate,
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

/// The real harness: a child process per call (`crate::harness::run`).
pub struct Process;

impl Harness for Process {
    fn call(&mut self, call: &Call) -> Value {
        let mut env = vec![(crate::summaries::HOOK_ENV, "off")];
        env.extend(call.env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        let opencode = crate::opencode::is_opencode(&call.args);
        let codex = crate::codex::is_codex(&call.args);
        // opencode 2.x gets `--standalone` (its own server, in the sandbox's environment).
        let args = if opencode {
            crate::opencode::headless_args(&call.args)
        } else if codex {
            crate::codex::exec_args(&call.args)
        } else {
            call.args.clone()
        };
        match crate::harness::run(&args, &call.input, &call.cwd, &env, CALL_TIMEOUT) {
            Ok(output) if opencode => crate::opencode::normalize_run_output(&output.stdout),
            Ok(output) if codex => {
                let mut answer = crate::codex::normalize_exec_output(
                    &output.stdout,
                    crate::codex::local_model(&call.args),
                );
                // Codex reports a failed start (for example a local server that does not
                // answer) on stderr only.
                if answer["is_error"] == true && !output.stderr.trim().is_empty() {
                    answer["result"] = json!(format!(
                        "{} ({})",
                        answer["result"].as_str().unwrap_or_default(),
                        crate::transcript::clip(output.stderr.trim(), 300)
                    ));
                }
                answer
            }
            Ok(output) => json_in(&output.stdout).unwrap_or_else(|| {
                json!({"is_error": true, "result": format!(
                    "the harness answered no JSON (exit {}): {}",
                    output.code.map_or_else(|| "signal".to_string(), |c| c.to_string()),
                    crate::transcript::clip(output.stderr.trim(), 300))})
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
                crate::transcript::clip(&text, 300)
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

/// The judge's prompt.
pub fn judge_prompt(question: &Question, answer: &str) -> String {
    format!(
        "Score an assistant's answer against a reference answer.
5 = all key facts of the reference, nothing wrong; 4 = key facts, minor omissions; 3 = about half \
of the key facts; 2 = some relevant facts, main point missing; 1 = barely relevant; 0 = no answer, \
'not found', or wrong.
confident_wrong = true only if the answer states something that contradicts the reference as a \
fact (not for 'I could not find it').
Extra correct detail beyond the reference is fine. Judge facts, not style. note: one sentence.
Answer with one JSON object matching this JSON schema:
{}

Question: {}
Reference answer: {}
Assistant's answer: {}",
        judge_schema(),
        question.question,
        question.reference,
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

/// The estimate of a run.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Estimate {
    /// Generating the questions (0 when the project has a set).
    pub generation: f64,
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
            warmup: arms as f64 * ANSWER_USD,
            answers: calls as f64 * ANSWER_USD,
            judge: calls as f64 * JUDGE_USD,
            calls,
        }
    }

    /// Everything.
    pub fn total(&self) -> f64 {
        self.generation + self.warmup + self.answers + self.judge
    }
}

/// Refuses a harness that is neither Claude Code's `claude`, opencode's `opencode` nor Codex's
/// `codex` with a local model (the arms are built with their flags and configuration). Codex
/// reports tokens but no cost, so the budget could not stop a run on a paid model: only a
/// local model (`--oss`, $0) is accepted.
pub fn check_harness(command: &[String]) -> Result<(), String> {
    let is_claude = command.first().is_some_and(|program| {
        Path::new(program)
            .file_stem()
            .is_some_and(|stem| stem == "claude")
    });
    if crate::codex::is_codex(command) && !crate::codex::local_model(command) {
        return Err(format!(
            "bench through Codex needs a local model (for example `codex exec --oss \
             --local-provider lmstudio`): `codex exec` reports tokens but no cost, so the budget \
             could not stop the run; or use Claude Code's `claude -p` or opencode's `opencode \
             run`. The harness command is `{}` (summaries.harness_command in config.json)",
            command.join(" ")
        ));
    }
    if is_claude || crate::opencode::is_opencode(command) || crate::codex::is_codex(command) {
        Ok(())
    } else {
        Err(format!(
            "bench drives Claude Code's `claude -p`, opencode's `opencode run` or Codex's `codex \
             exec --oss`; the harness command is `{}` (summaries.harness_command in config.json)",
            command.join(" ")
        ))
    }
}

/// Whether the run goes through opencode.
fn opencode(options: &Options) -> bool {
    crate::opencode::is_opencode(&options.harness_command)
}

/// Whether the run goes through Codex.
fn codex(options: &Options) -> bool {
    crate::codex::is_codex(&options.harness_command)
}

/// The excerpt of the project's finished sessions, from the harness's own store.
fn excerpt_for(options: &Options) -> Excerpt {
    if codex(options) {
        Excerpt::of_transcripts(
            &questions::codex_transcripts(
                &options.codex.sessions,
                &options.project,
                SystemTime::now(),
            ),
            questions::EXCERPT_BUDGET,
        )
    } else if opencode(options) {
        Excerpt::of_transcripts(
            &questions::opencode_transcripts(
                &options.opencode.db,
                &options.project,
                SystemTime::now(),
            ),
            questions::EXCERPT_BUDGET,
        )
    } else {
        let transcripts = questions::transcripts(
            &options.claude.projects,
            &options.project,
            SystemTime::now(),
        );
        Excerpt::of(&transcripts, questions::EXCERPT_BUDGET)
    }
}

/// The working directory, the command line and the environment of a tool-free call.
type ToolFreeCall = (PathBuf, Vec<String>, Vec<(String, String)>);

/// A call of the generator or the judge: the harness command, the tool-free sandbox's
/// arguments, and for `claude` the schema and the cost cap of the call.
fn tool_free_call(
    options: &Options,
    dir: &Path,
    schema: Value,
    cap: f64,
) -> Result<ToolFreeCall, String> {
    let mut command = options.harness_command.clone();
    if codex(options) {
        let (cwd, env) = arms::tool_free_codex(dir, &options.codex)?;
        return Ok((cwd, command, env));
    }
    if opencode(options) {
        let (cwd, env) = arms::tool_free_opencode(dir, &options.opencode.db)?;
        command.extend(["--format".to_string(), "json".to_string()]);
        return Ok((cwd, command, env));
    }
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
        format!("{}\n", crate::jsonedit::render(&value, Some("  "), 0)).as_bytes(),
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
        generated_at: crate::daemon::now_utc(),
        sessions: excerpt.sessions,
        cost_usd: cost,
        confirmed: false,
        questions,
    })
}

fn show_questions(set: &QuestionSet, limit: usize, out: &mut dyn Write) {
    for q in set.kept().iter().take(limit) {
        let _ = writeln!(
            out,
            "  {} [{}] {}\n     reference: {}\n     sources: {}",
            q.id,
            q.category,
            q.question,
            q.reference,
            q.sources.join(", ")
        );
        for excerpt in &q.excerpts {
            let _ = writeln!(out, "       > {}", crate::transcript::clip(excerpt, 160));
        }
    }
}

/// `bench questions`: generate a new set (`review` false) or show and confirm the current one.
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
        let _ = writeln!(
            out,
            "Question set of {} ({} of {} kept, generated {} from {} sessions; {}):",
            options.project,
            set.kept().len(),
            set.questions.len(),
            set.generated_at,
            set.sessions,
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
    check_harness(&options.harness_command)?;
    let excerpt = excerpt_for(options);
    if excerpt.sessions == 0 {
        return Err(no_transcripts(options));
    }
    let free = crate::harness::costs_nothing(&options.harness_command);
    let estimate = if free { 0.0 } else { 1.0 }
        * generation_usd(
            questions::prompt(&options.project, options.questions, &excerpt).len(),
            options.questions,
        );
    let _ = writeln!(
        out,
        "Generating {} questions about {} from {} finished session(s) (raw transcripts): about \
         ${estimate:.2} at list price (budget ${:.2}).",
        options.questions, options.project, excerpt.sessions, options.budget_usd
    );
    if estimate > options.budget_usd {
        let _ = writeln!(out, "Refused: the estimate is above the budget (--budget).");
        return Ok(EXIT_FAILED);
    }
    if options.dry_run {
        let _ = writeln!(out, "Dry run: a fake harness answers; nothing is spent.");
    } else if !options.yes && !crate::install::confirm("Generate them?", input, out) {
        let _ = writeln!(out, "Nothing was generated.");
        return Ok(EXIT_FAILED);
    }
    let work = new_run_dir(&options.data_dir, &crate::daemon::now_utc())?.join("questions");
    let set = generate(
        options,
        &excerpt,
        options.questions,
        options.budget_usd,
        &work,
        harness,
    )?;
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
        set.cost_usd
    );
    Ok(0)
}

fn no_transcripts(options: &Options) -> String {
    if codex(options) {
        return format!(
            "no finished Codex session of {} under {} (sessions still being written are left \
             out for 30 minutes)",
            options.project,
            options.codex.sessions.display()
        );
    }
    if opencode(options) {
        return format!(
            "no finished opencode session of {} in {} (sessions still being written are left \
             out for 30 minutes)",
            options.project,
            options.opencode.db.display()
        );
    }
    format!(
        "no finished session transcript of {} under {} (sessions still being written are left \
         out for 30 minutes)",
        options.project,
        options
            .claude
            .projects
            .join(crate::import::claude_slug(&options.project))
            .display()
    )
}

/// `bench`: plans, asks, runs every arm, writes the report. Returns the exit code.
pub fn run(
    options: &Options,
    harness: &mut dyn Harness,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<u8, String> {
    check_harness(&options.harness_command)?;
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
    let store_dir = crate::store::default_store_dir(&options.data_dir);
    let no_store = std::fs::read_dir(&store_dir).map_or(true, |mut e| e.next().is_none());
    if arms_list.contains(&BenchArm::Graph) && no_store {
        return Err(format!(
            "there is no memory store in {} yet: run `graphersal-memory install` (or `import` \
             and `import --sessions`) first",
            options.data_dir.display()
        ));
    }
    let existing = QuestionSet::load(&options.data_dir, &options.project)?
        .filter(|set| !set.kept().is_empty());
    let excerpt = if existing.is_none() {
        let excerpt = excerpt_for(options);
        if excerpt.sessions == 0 {
            return Err(no_transcripts(options));
        }
        excerpt
    } else {
        Excerpt::default()
    };
    let (count, generation) = match &existing {
        Some(set) => (set.kept().len(), 0.0),
        None => (
            options.questions,
            generation_usd(
                questions::prompt(&options.project, options.questions, &excerpt).len(),
                options.questions,
            ),
        ),
    };
    let mut estimate = Estimate::of(count, arms_list.len(), options.repeats, generation);
    let free = crate::harness::costs_nothing(&options.harness_command);
    if free {
        estimate = Estimate {
            calls: estimate.calls,
            ..Estimate::default()
        };
    }
    let names: Vec<&str> = arms_list.iter().map(|a| a.name()).collect();
    let _ = writeln!(
        out,
        "bench of {}: arms {}; {} question(s){}; {} repeat(s).",
        options.project,
        names.join(", "),
        count,
        match &existing {
            Some(set) => format!(
                " from {} ({})",
                questions::path(&options.data_dir, &options.project).display(),
                if set.confirmed {
                    "confirmed"
                } else {
                    "not confirmed yet"
                }
            ),
            None => format!(" to generate from {} finished session(s)", excerpt.sessions),
        },
        options.repeats
    );
    if free && crate::codex::local_model(&options.harness_command) {
        let _ = writeln!(
            out,
            "`{}` runs a local model on this machine: this run costs $0.00.",
            options.harness_command.join(" ")
        );
    } else if free {
        let _ = writeln!(
            out,
            "`{}` uses a free model of opencode Zen (free for a limited time): this run costs \
             $0.00; the budget still stops it if opencode reports a cost.",
            options.harness_command.join(" ")
        );
    }
    let _ = writeln!(
        out,
        "Estimate (list price): about ${:.2}: questions ${:.2}, warm-up ${:.2}, {} answers \
         ${:.2}, judge ${:.2}. Budget ${:.2}.",
        estimate.total(),
        estimate.generation,
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

    let started_at = crate::daemon::now_utc();
    let run_dir = new_run_dir(&options.data_dir, &started_at)?;
    let mut spent = Spent::default();
    let set = match existing {
        Some(set) => set,
        None => {
            let set = generate(
                options,
                &excerpt,
                options.questions,
                options.budget_usd,
                &run_dir.join("questions"),
                harness,
            )?;
            spent.generation = set.cost_usd;
            let file = if options.dry_run {
                run_dir.join("questions.json")
            } else {
                questions::path(&options.data_dir, &options.project)
            };
            set.save_to(&file)?;
            let _ = writeln!(
                out,
                "{} questions generated (${:.3}), written to {}.",
                set.questions.len(),
                set.cost_usd,
                file.display()
            );
            set
        }
    };
    let mut set = set;
    if !set.confirmed && !options.dry_run && !options.yes {
        let _ = writeln!(out, "A sample of the questions:");
        show_questions(&set, 3, out);
        if crate::install::confirm(
            "Use these questions (all are in the file above)?",
            input,
            out,
        ) {
            set.confirmed = true;
            set.save_to(&questions::path(&options.data_dir, &options.project))?;
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
        instructions: if codex(options) {
            arms::Sources::codex_instructions_of(&options.codex, &options.project)
        } else if opencode(options) {
            arms::Sources::opencode_instructions_of(&options.opencode, &options.project)
        } else {
            arms::Sources::instructions_of(&options.claude.instructions, &options.project)
        },
        memory_dir: Some(
            options
                .claude
                .projects
                .join(crate::import::claude_slug(&options.project))
                .join("memory"),
        )
        .filter(|dir| dir.is_dir() && !opencode(options) && !codex(options)),
        store_dir,
        exe: options.exe.clone(),
    };
    let arms_dir = run_dir.join("arms");
    let mut sandboxes = Vec::new();
    for arm in &arms_list {
        sandboxes.push(if codex(options) {
            arms::build_codex(*arm, &arms_dir, &sources, &options.codex)?
        } else if opencode(options) {
            arms::build_opencode(*arm, &arms_dir, &sources, &options.opencode.db)?
        } else {
            arms::build(*arm, &arms_dir, &sources)?
        });
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
        questions: set.kept().len(),
        repeats: options.repeats,
        sessions: set.sessions,
        confirmed: set.confirmed,
        dry_run: options.dry_run,
        stopped: None,
        notes: Vec::new(),
    };
    if codex(options) && arms_list.contains(&BenchArm::Files) {
        meta.notes.push(
            "Codex has no file memory: the files arm holds its instruction files (AGENTS.md) \
             only."
                .to_string(),
        );
    } else if opencode(options) && arms_list.contains(&BenchArm::Files) {
        meta.notes.push(
            "opencode has no file memory: the files arm holds its instruction files (AGENTS.md) \
             only."
                .to_string(),
        );
    } else if sources.memory_dir.is_none() && arms_list.contains(&BenchArm::Files) {
        meta.notes.push(
            "The project has no memory directory: the files arm holds the instruction files only."
                .to_string(),
        );
    }
    let answer_call = |sandbox: &arms::Sandbox, kind: Kind, prompt: String, cap: f64| {
        let mut args = options.harness_command.clone();
        args.extend(sandbox.args.iter().cloned());
        if !opencode(options) && !codex(options) {
            args.extend(["--max-budget-usd".to_string(), format!("{cap:.2}")]);
        }
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
    let kept: Vec<Question> = set.kept().into_iter().cloned().collect();
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
                let row = Row {
                    question: question.id.clone(),
                    category: question.category.clone(),
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
                    row.score,
                    row.cost_usd,
                    row.turns,
                    row.error
                        .as_deref()
                        .map(|e| format!(" (failed: {})", crate::transcript::clip(e, 80)))
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
        format!("{}\n", crate::jsonedit::render(&value, Some("  "), 0)).as_bytes(),
    )?;
    let _ = writeln!(
        out,
        "\n{}\n\nReport: {}",
        report::summary(&meta, &rows, &spent),
        run_dir.join("report.md").display()
    );
    if let Some(why) = &meta.stopped {
        let _ = writeln!(out, "Stopped early: {why}.");
        return Ok(EXIT_FAILED);
    }
    Ok(0)
}
