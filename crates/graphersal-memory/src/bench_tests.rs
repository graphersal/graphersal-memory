//! `bench` in this process on a temporary home and data directory: a fixture transcript and
//! memory directory, a live store created for the test, and a scripted harness that records
//! every call (no process, no model). The live files are checked to stay unchanged and the
//! sandboxes never to name them.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

use crate::bench::{self, Call, DryRun, Harness, Kind, Options};
use crate::cli::BenchArm;
use crate::install::ClaudePaths;
use crate::mcp_tests::TempDir;

const PROJECT: &str = "/w/bench-app";

/// A harness answering through a closure and recording every call.
struct Scripted {
    calls: Vec<Call>,
    answer: Box<dyn FnMut(&Call) -> Value>,
}

impl Scripted {
    fn new(answer: impl FnMut(&Call) -> Value + 'static) -> Self {
        Self {
            calls: Vec::new(),
            answer: Box::new(answer),
        }
    }

    fn count(&self, kind: Kind) -> usize {
        self.calls.iter().filter(|c| c.kind == kind).count()
    }
}

impl Harness for Scripted {
    fn call(&mut self, call: &Call) -> Value {
        self.calls.push(call.clone());
        (self.answer)(call)
    }
}

/// The typical answers: questions grounded in the fixture's turns, answers costing $0.03 (the
/// graph arm's in two turns), the judge giving 5 to the graph arm, 2 to files, 0 to none.
fn typical(call: &Call) -> Value {
    match call.kind {
        Kind::Generate => json!({"is_error": false, "total_cost_usd": 0.12,
        "structured_output": {"questions": [
            {"category": "rule", "question": "May I push without asking?",
             "reference": "No, never push without asking.", "sources": [turn(0)]},
            {"category": "decision", "question": "Why a cache in the parser?",
             "reference": "Parsing was slow.", "sources": [turn(1)]},
            {"category": "history", "question": "What did the tests show?",
             "reference": "They passed.", "sources": [turn(2)]},
        ]}}),
        Kind::Warmup => json!({"result": "ready", "total_cost_usd": 0.01, "num_turns": 1,
            "usage": {"input_tokens": 10, "output_tokens": 1}}),
        Kind::Answer => {
            let graph = call
                .args
                .iter()
                .any(|a| a.contains("mcp__graphersal-memory__"));
            json!({"result": if graph { "Graph answer" } else { "Some answer" },
                "total_cost_usd": 0.03, "num_turns": if graph { 2 } else { 1 },
                "duration_ms": 4000,
                "usage": {"input_tokens": 100, "cache_creation_input_tokens": 400,
                          "cache_read_input_tokens": 500, "output_tokens": 50}})
        }
        Kind::Judge => {
            let score = if call.input.contains("Assistant's answer: Graph answer") {
                5
            } else if call.input.contains("Assistant's answer: Some answer") {
                2
            } else {
                0
            };
            json!({"total_cost_usd": 0.01, "structured_output": {"score": score,
                "confident_wrong": false, "note": "n"}})
        }
    }
}

fn turn(n: usize) -> String {
    format!("turn:0f0e0d0c-1111-2222-3333-444444444444:{n}")
}

/// A temporary machine: a home with Claude Code's files for `PROJECT`, a data directory with a
/// live store.
struct Machine {
    home: TempDir,
    data: TempDir,
}

impl Machine {
    fn new() -> Self {
        let machine = Self {
            home: TempDir::new("bench-home"),
            data: TempDir::new("bench-data"),
        };
        let project_dir = machine
            .claude()
            .projects
            .join(crate::import::claude_slug(PROJECT));
        std::fs::create_dir_all(project_dir.join("memory")).unwrap();
        let transcript = project_dir.join("0f0e0d0c-1111-2222-3333-444444444444.jsonl");
        std::fs::write(&transcript, fixture()).unwrap();
        // Finished long ago (the bench leaves out transcripts still being written).
        let old = SystemTime::now() - Duration::from_secs(48 * 3600);
        std::fs::File::options()
            .write(true)
            .open(&transcript)
            .unwrap()
            .set_modified(old)
            .unwrap();
        std::fs::write(
            project_dir.join("memory").join("MEMORY.md"),
            "- [No push](no-push.md) - never push without asking\n",
        )
        .unwrap();
        std::fs::write(
            project_dir.join("memory").join("no-push.md"),
            "---\nname: no-push\ndescription: never push\ntype: feedback\n---\nNever push.\n",
        )
        .unwrap();
        std::fs::create_dir_all(&machine.claude().config_dir).unwrap();
        std::fs::write(
            &machine.claude().instructions,
            format!(
                "# My rules\n\nAlways answer briefly.\n{} fingerprint=x -->\nmanaged\n{}\n",
                crate::install::BLOCK_BEGIN,
                crate::install::BLOCK_END
            ),
        )
        .unwrap();
        std::fs::write(&machine.claude().user_config, "{\"mcpServers\": {}}\n").unwrap();
        std::fs::write(&machine.claude().settings, "{\"hooks\": {}}\n").unwrap();
        // The live store, created and closed.
        let store =
            crate::store::open_or_create(&crate::store::default_store_dir(machine.data.path()))
                .unwrap();
        store.shutdown().unwrap();
        machine
    }

    fn claude(&self) -> ClaudePaths {
        ClaudePaths::resolve(&|name| {
            (name == "HOME").then(|| self.home.path().display().to_string())
        })
        .unwrap()
    }

    fn options(&self) -> Options {
        Options {
            arms: vec![BenchArm::None, BenchArm::Files, BenchArm::Graph],
            questions: 3,
            repeats: 1,
            budget_usd: 3.0,
            dry_run: false,
            yes: true,
            project: PROJECT.to_string(),
            claude: self.claude(),
            opencode: crate::opencode::OpenCodePaths::of_home(self.home.path()),
            data_dir: self.data.path().to_path_buf(),
            exe: PathBuf::from("/opt/gm/bin/graphersal-memory"),
            harness_command: vec![
                "claude".into(),
                "-p".into(),
                "--model".into(),
                "sonnet".into(),
            ],
        }
    }

    /// Every file of the live configuration with its bytes.
    fn live_files(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        collect(self.home.path(), &mut out);
        collect(&crate::store::default_store_dir(self.data.path()), &mut out);
        out
    }

    fn run(&self, options: &Options, harness: &mut dyn Harness, input: &str) -> (u8, String) {
        let mut out = Vec::new();
        let code = bench::run(options, harness, &mut input.as_bytes(), &mut out).unwrap();
        (code, String::from_utf8(out).unwrap())
    }

    fn run_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(self.data.path().join("bench"))
            .map(|e| {
                e.filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.join("report.json").exists())
                    .collect()
            })
            .unwrap_or_default();
        dirs.sort();
        dirs
    }
}

fn collect(dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.filter_map(Result::ok).map(|e| e.path()) {
        if path.is_dir() {
            collect(&path, out);
        } else {
            out.push((path.clone(), std::fs::read(&path).unwrap()));
        }
    }
    out.sort();
}

fn fixture() -> String {
    let mut lines = Vec::new();
    for (i, (user, reply)) in [
        ("Never push without asking me.", "Understood."),
        ("Add a cache to the parser, parsing is slow.", "Added."),
        ("Run the tests.", "They passed."),
    ]
    .iter()
    .enumerate()
    {
        lines.push(json!({"type": "user", "sessionId": "s-1", "cwd": PROJECT,
            "timestamp": format!("2026-01-02T10:0{i}:00.000Z"), "origin": {"kind": "human"},
            "message": {"role": "user", "content": user}}));
        lines.push(json!({"type": "assistant", "sessionId": "s-1",
            "timestamp": format!("2026-01-02T10:0{i}:05.000Z"),
            "message": {"role": "assistant", "content": [{"type": "text", "text": reply}]}}));
    }
    lines.iter().map(|l| format!("{l}\n")).collect::<String>()
}

#[test]
fn a_run_generates_questions_from_raw_turns_and_reports_every_arm() {
    let machine = Machine::new();
    let before = machine.live_files();
    let mut harness = Scripted::new(typical);
    let (code, out) = machine.run(&machine.options(), &mut harness, "");
    assert_eq!(code, 0, "{out}");
    assert_eq!(harness.count(Kind::Generate), 1);
    assert_eq!(harness.count(Kind::Warmup), 3);
    assert_eq!(harness.count(Kind::Answer), 9);
    assert_eq!(harness.count(Kind::Judge), 9);
    // The generator saw the raw turns with their ids, never a summary.
    let generate = harness
        .calls
        .iter()
        .find(|c| c.kind == Kind::Generate)
        .unwrap();
    assert!(
        generate
            .input
            .contains(&format!("[{}] USER: Never push", turn(0)))
    );
    assert!(generate.args.contains(&"--json-schema".to_string()));
    // Every prompt starts with the run id.
    for call in harness.calls.iter().filter(|c| c.kind == Kind::Answer) {
        assert!(call.input.starts_with("[benchmark run "), "{}", call.input);
    }
    // The question set is kept for the project, unconfirmed (accepted with --yes).
    let set = bench::questions::QuestionSet::load(machine.data.path(), PROJECT)
        .unwrap()
        .unwrap();
    assert_eq!(set.questions.len(), 3);
    assert!(!set.confirmed);
    assert_eq!(set.questions[0].excerpts, ["Never push without asking me."]);

    let run = machine.run_dirs().pop().unwrap();
    let report: Value =
        serde_json::from_slice(&std::fs::read(run.join("report.json")).unwrap()).unwrap();
    let group = |arm: &str| {
        report["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["arm"] == arm && g["category"] == "all")
            .unwrap()
            .clone()
    };
    assert_eq!(group("graph")["score"], 5.0);
    assert_eq!(group("files")["score"], 2.0);
    assert_eq!(group("none")["score"], 2.0, "{report}");
    assert_eq!(group("graph")["input_tokens"], 1000.0);
    assert_eq!(group("graph")["turns"], 2.0);
    assert!((group("graph")["cost_per_point"].as_f64().unwrap() - 0.006).abs() < 1e-9);
    let spent = &report["spent_usd"];
    assert!((spent["questions"].as_f64().unwrap() - 0.12).abs() < 1e-9);
    assert!((spent["warmup"].as_f64().unwrap() - 0.03).abs() < 1e-9);
    assert!((spent["answers"].as_f64().unwrap() - 0.27).abs() < 1e-9);
    assert!((spent["judge"].as_f64().unwrap() - 0.09).abs() < 1e-9);
    let markdown = std::fs::read_to_string(run.join("report.md")).unwrap();
    assert!(markdown.contains("## Caveats"), "{markdown}");
    assert!(markdown.contains("NOT reviewed"), "{markdown}");
    assert!(out.contains("graph  score 5.00/5"), "{out}");
    // The store fork is gone after the run; the live configuration and store are unchanged.
    assert!(!run.join("arms").join("graph").join("data").exists());
    assert_eq!(machine.live_files(), before);
}

#[test]
fn the_sandboxes_never_name_the_live_configuration_or_store() {
    let machine = Machine::new();
    let mut harness = Scripted::new(typical);
    let mut options = machine.options();
    options.questions = 1;
    let (code, out) = machine.run(&options, &mut harness, "");
    assert_eq!(code, 0, "{out}");
    let run = machine.run_dirs().pop().unwrap();
    let claude = machine.claude();
    let live_store = crate::store::default_store_dir(machine.data.path());
    let forbidden = [
        claude.config_dir.display().to_string(),
        claude.user_config.display().to_string(),
        live_store.display().to_string(),
    ];
    let mut files = Vec::new();
    collect(&run.join("arms"), &mut files);
    collect(&run.join("judge"), &mut files);
    assert!(files.len() >= 9, "{files:?}");
    for (path, bytes) in &files {
        let text = String::from_utf8_lossy(bytes);
        for live in &forbidden {
            assert!(
                !text.contains(live.as_str()),
                "{} names {live}",
                path.display()
            );
        }
    }
    for call in &harness.calls {
        let args = call.args.join(" ");
        for live in &forbidden {
            assert!(!args.contains(live.as_str()), "{args}");
        }
        for flag in [
            "--strict-mcp-config",
            "--mcp-config",
            "--setting-sources",
            "--settings",
            "--no-session-persistence",
            "--max-budget-usd",
            "--tools",
        ] {
            assert!(
                call.args.iter().any(|a| a == flag),
                "{flag} missing: {args}"
            );
        }
        let at = call
            .args
            .iter()
            .position(|a| a == "--setting-sources")
            .unwrap();
        assert_eq!(call.args[at + 1], "", "no user, project or local settings");
        assert!(call.cwd.starts_with(&run), "{}", call.cwd.display());
    }
    // The graph arm serves a fork in the run directory, without the user's daemon.
    let mcp: Value = serde_json::from_slice(
        &std::fs::read(run.join("arms").join("graph").join("mcp.json")).unwrap(),
    )
    .unwrap();
    let server = &mcp["mcpServers"]["graphersal-memory"];
    let args: Vec<&str> = server["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(&args[..3], ["mcp", "--no-daemon", "--data-dir"]);
    assert!(Path::new(args[3]).starts_with(&run), "{}", args[3]);
    assert_eq!(&args[4..], ["--project", PROJECT]);
    assert_eq!(server["env"]["GRAPHERSAL_MEMORY_HOOK"], "off");
    for arm in ["none", "files"] {
        let mcp: Value = serde_json::from_slice(
            &std::fs::read(run.join("arms").join(arm).join("mcp.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(mcp, json!({"mcpServers": {}}), "{arm}");
    }
    // Tools: none has none, files reads its copy, graph only the read functions.
    let tools_of = |arm: &str| {
        let call = harness
            .calls
            .iter()
            .find(|c| c.kind == Kind::Answer && c.cwd.starts_with(run.join("arms").join(arm)))
            .unwrap();
        let at = call.args.iter().position(|a| a == "--tools").unwrap();
        (call.args[at + 1].clone(), call.args.clone())
    };
    assert_eq!(tools_of("none").0, "");
    assert_eq!(tools_of("files").0, "Read,Grep,Glob");
    let (graph_tools, graph_args) = tools_of("graph");
    assert_eq!(graph_tools, "");
    let at = graph_args
        .iter()
        .position(|a| a == "--allowedTools")
        .unwrap();
    assert_eq!(
        graph_args[at + 1],
        "mcp__graphersal-memory__memories,mcp__graphersal-memory__memory,\
         mcp__graphersal-memory__sessions,mcp__graphersal-memory__session,\
         mcp__graphersal-memory__recall"
    );
    // The files arm works on a copy of the memory files; its context has MEMORY.md and the
    // instruction file without the managed block; the none arm has no instructions.
    let files_cwd = run.join("arms").join("files").join("cwd");
    assert!(files_cwd.join("no-push.md").is_file());
    assert!(files_cwd.join("MEMORY.md").is_file());
    let system = std::fs::read_to_string(run.join("arms").join("files").join("system.md")).unwrap();
    assert!(system.contains("Always answer briefly."), "{system}");
    assert!(system.contains("never push without asking"), "{system}");
    assert!(!system.contains("managed"), "{system}");
    let none = std::fs::read_to_string(run.join("arms").join("none").join("system.md")).unwrap();
    assert!(!none.contains("Always answer briefly."), "{none}");
    let settings: Value = serde_json::from_slice(
        &std::fs::read(run.join("arms").join("files").join("settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(settings["disableAllHooks"], true);
}

#[test]
fn a_dry_run_spends_nothing_and_saves_no_question_set() {
    let machine = Machine::new();
    let mut options = machine.options();
    options.dry_run = true;
    options.yes = false;
    let mut harness = DryRun::default();
    // No answer to any question is given: a dry run asks nothing.
    let (code, out) = machine.run(&options, &mut harness, "");
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Dry run: a fake harness answers; nothing is spent."),
        "{out}"
    );
    assert!(out.contains("spent $0.000 (list price); dry run"), "{out}");
    assert!(!harness.calls.is_empty());
    assert_eq!(
        bench::questions::QuestionSet::load(machine.data.path(), PROJECT).unwrap(),
        None,
        "a dry run keeps its questions in its run directory"
    );
    let run = machine.run_dirs().pop().unwrap();
    assert!(run.join("questions.json").is_file());
    let report: Value =
        serde_json::from_slice(&std::fs::read(run.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["spent_usd"]["total"], 0.0);
    assert_eq!(report["rows"].as_array().unwrap().len(), 9);
}

#[test]
fn a_run_above_the_budget_is_refused_before_any_call() {
    let machine = Machine::new();
    let mut options = machine.options();
    options.budget_usd = 0.2;
    let mut harness = Scripted::new(typical);
    let (code, out) = machine.run(&options, &mut harness, "");
    assert_eq!(code, 1);
    assert!(
        out.contains("Refused: the estimate is above the budget"),
        "{out}"
    );
    assert!(harness.calls.is_empty());
    assert!(machine.run_dirs().is_empty());
}

#[test]
fn the_run_stops_when_the_real_spend_reaches_the_budget() {
    let machine = Machine::new();
    let mut options = machine.options();
    options.budget_usd = 1.0;
    // Answers cost far more than estimated.
    let mut harness = Scripted::new(|call: &Call| {
        let mut value = typical(call);
        if call.kind == Kind::Answer {
            value["total_cost_usd"] = json!(0.30);
        }
        value
    });
    let (code, out) = machine.run(&options, &mut harness, "");
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("Stopped early: the budget of $1.00 was reached"),
        "{out}"
    );
    let answers = harness.count(Kind::Answer);
    assert!((1..9).contains(&answers), "{answers}");
    let run = machine.run_dirs().pop().unwrap();
    let report: Value =
        serde_json::from_slice(&std::fs::read(run.join("report.json")).unwrap()).unwrap();
    assert!(report["spent_usd"]["total"].as_f64().unwrap() <= 1.0 + 0.30);
    assert!(report["stopped"].is_string());
}

#[test]
fn an_unconfirmed_question_set_is_asked_for_and_a_no_runs_nothing() {
    let machine = Machine::new();
    let mut options = machine.options();
    options.yes = false;
    // Yes to the estimate, no to the questions.
    let mut harness = Scripted::new(typical);
    let (code, out) = machine.run(&options, &mut harness, "y\nn\n");
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("A sample of the questions:"), "{out}");
    assert!(
        out.contains("reference: No, never push without asking."),
        "{out}"
    );
    assert_eq!(harness.count(Kind::Generate), 1);
    assert_eq!(harness.count(Kind::Answer), 0);
    // Again: the set exists now (no generation); yes to both runs and confirms it.
    let mut harness = Scripted::new(typical);
    let (code, out) = machine.run(&options, &mut harness, "y\ny\n");
    assert_eq!(code, 0, "{out}");
    assert_eq!(harness.count(Kind::Generate), 0);
    assert_eq!(harness.count(Kind::Answer), 9);
    let set = bench::questions::QuestionSet::load(machine.data.path(), PROJECT)
        .unwrap()
        .unwrap();
    assert!(set.confirmed);
    // A no to the estimate runs nothing at all.
    let mut harness = Scripted::new(typical);
    let (code, _) = machine.run(&options, &mut harness, "n\n");
    assert_eq!(code, 1);
    assert!(harness.calls.is_empty());
}

#[test]
fn bench_questions_generates_and_review_confirms_an_edited_set() {
    let machine = Machine::new();
    let options = machine.options();
    let mut harness = Scripted::new(typical);
    let mut out = Vec::new();
    let code =
        bench::questions_command(&options, false, &mut harness, &mut "".as_bytes(), &mut out)
            .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("3 questions written to"), "{text}");
    assert_eq!(harness.calls.len(), 1);
    // The user drops one question in the file, then reviews and confirms.
    let file = bench::questions::path(machine.data.path(), PROJECT);
    let mut value: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    value["questions"][1]["keep"] = json!(false);
    std::fs::write(&file, value.to_string()).unwrap();
    let mut out = Vec::new();
    let mut review = options.clone();
    review.yes = false;
    let code =
        bench::questions_command(&review, true, &mut harness, &mut "y\n".as_bytes(), &mut out)
            .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("2 of 3 kept"), "{text}");
    assert!(text.contains("> Never push without asking me."), "{text}");
    assert_eq!(harness.calls.len(), 1, "a review calls no model");
    let set = bench::questions::QuestionSet::load(machine.data.path(), PROJECT)
        .unwrap()
        .unwrap();
    assert!(set.confirmed);
    // The bench then uses the two kept questions, without generating.
    let mut harness = Scripted::new(typical);
    let (code, out) = machine.run(&machine.options(), &mut harness, "");
    assert_eq!(code, 0, "{out}");
    assert_eq!(harness.count(Kind::Answer), 6);
    assert_eq!(harness.count(Kind::Generate), 0);
}

#[test]
fn failed_answers_and_judgings_count_as_zero_and_are_reported() {
    let machine = Machine::new();
    let mut options = machine.options();
    options.arms = vec![BenchArm::Files];
    let mut harness = Scripted::new(|call: &Call| match call.kind {
        Kind::Answer => json!({"is_error": true, "result": "API overloaded",
                               "total_cost_usd": 0.0}),
        Kind::Judge => json!({"result": "I cannot judge this"}),
        _ => typical(call),
    });
    let (code, out) = machine.run(&options, &mut harness, "");
    assert_eq!(code, 0, "{out}");
    let run = machine.run_dirs().pop().unwrap();
    let report: Value =
        serde_json::from_slice(&std::fs::read(run.join("report.json")).unwrap()).unwrap();
    let row = &report["rows"][0];
    assert_eq!(row["score"], 0);
    assert_eq!(row["error"], "API overloaded");
    // The judge saw that there was no answer.
    let judge = harness
        .calls
        .iter()
        .find(|c| c.kind == Kind::Judge)
        .unwrap();
    assert!(
        judge.input.ends_with("Assistant's answer: (no answer)"),
        "{}",
        judge.input
    );
    let caveats = report["caveats"].to_string();
    assert!(
        caveats.contains("3 answer(s) or judgings failed"),
        "{caveats}"
    );
}

#[test]
fn bench_refuses_what_it_cannot_run() {
    let machine = Machine::new();
    let mut options = machine.options();
    // opencode is supported since M7a; another harness is refused.
    options.harness_command = vec!["codex".into(), "exec".into()];
    let mut out = Vec::new();
    let err = bench::run(
        &options,
        &mut DryRun::default(),
        &mut "".as_bytes(),
        &mut out,
    )
    .unwrap_err();
    assert!(
        err.contains("claude -p") && err.contains("opencode run"),
        "{err}"
    );
    let mut options = machine.options();
    options.arms = vec![BenchArm::Current];
    let err = bench::run(
        &options,
        &mut DryRun::default(),
        &mut "".as_bytes(),
        &mut out,
    )
    .unwrap_err();
    assert!(err.contains("planned"), "{err}");
    let mut options = machine.options();
    options.project = "/w/elsewhere".into();
    let err = bench::run(
        &options,
        &mut DryRun::default(),
        &mut "".as_bytes(),
        &mut out,
    )
    .unwrap_err();
    assert!(err.contains("no finished session transcript"), "{err}");
    let empty = TempDir::new("bench-nostore");
    let mut options = machine.options();
    options.data_dir = empty.path().to_path_buf();
    let err = bench::run(
        &options,
        &mut DryRun::default(),
        &mut "".as_bytes(),
        &mut out,
    )
    .unwrap_err();
    assert!(err.contains("no memory store"), "{err}");
}

#[test]
fn verdicts_and_usage_parse() {
    let verdict = bench::parse_verdict(&json!({"structured_output":
        {"score": 4, "confident_wrong": true, "note": " ok "}}))
    .unwrap();
    assert_eq!((verdict.score, verdict.confident_wrong), (4, true));
    assert_eq!(verdict.note, "ok");
    let in_text = bench::parse_verdict(&json!({"result":
        "```json\n{\"score\": 9.4, \"confident_wrong\": false, \"note\": \"x\"}\n```"}))
    .unwrap();
    assert_eq!(in_text.score, 5, "clamped");
    assert!(bench::parse_verdict(&json!({"result": "no"})).is_err());
    assert!(bench::parse_verdict(&json!({"is_error": true, "result": "x"})).is_err());

    let usage = bench::Usage::of(
        &json!({"result": "a", "total_cost_usd": 0.02, "num_turns": 3, "duration_ms": 0,
                "usage": {"input_tokens": 5, "cache_read_input_tokens": 10,
                          "cache_creation_input_tokens": 1, "output_tokens": 7}}),
        Duration::from_millis(1500),
    );
    assert_eq!(usage.input_tokens, 16);
    assert_eq!(usage.output_tokens, 7);
    assert_eq!(usage.turns, 3);
    assert_eq!(usage.ms, 1500, "the measured time when none is reported");
    assert_eq!(usage.error, None);
    let failed = bench::Usage::of(
        &json!({"is_error": true, "subtype": "error_max_budget_usd"}),
        Duration::ZERO,
    );
    assert_eq!(failed.error.as_deref(), Some("error_max_budget_usd"));
    assert_eq!(failed.text, "");

    let estimate = bench::Estimate::of(6, 3, 2, 0.1);
    assert_eq!(estimate.calls, 36);
    assert!((estimate.total() - (0.1 + 3.0 * 0.05 + 36.0 * 0.07)).abs() < 1e-9);
    assert!(bench::generation_usd(120_000, 12) > bench::generation_usd(1_000, 12));
}

#[test]
fn an_opencode_bench_runs_every_arm_in_its_own_sandbox_for_free() {
    let machine = Machine::new();
    let paths = crate::opencode::OpenCodePaths::of_home(machine.home.path());
    crate::opencode::sessions::tests::fixture(&paths.db, PROJECT);
    std::fs::write(paths.db.with_file_name("auth.json"), "{\"opencode\": {}}").unwrap();
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::write(paths.config_dir.join("AGENTS.md"), "# My opencode rules\n").unwrap();
    std::fs::write(&paths.config_file, "{\"model\": \"opencode/big-pickle\"}\n").unwrap();
    let before = machine.live_files();
    let mut options = machine.options();
    options.harness_command =
        crate::config::split_command(crate::opencode::DEFAULT_HEADLESS_COMMAND).unwrap();
    let mut harness = DryRun::default();
    let (code, out) = machine.run(&options, &mut harness, "");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("this run costs $0.00"), "{out}");
    assert!(out.contains("about $0.00"), "{out}");
    // SQLite's own WAL index files of the session database (a reader of a WAL database
    // creates them, as opencode itself does) are not the user's data.
    let data_only = |files: Vec<(PathBuf, Vec<u8>)>| -> Vec<(PathBuf, Vec<u8>)> {
        files
            .into_iter()
            .filter(|(p, _)| {
                let name = p.display().to_string();
                !name.ends_with("opencode.db-shm") && !name.ends_with("opencode.db-wal")
            })
            .collect()
    };
    assert_eq!(
        data_only(machine.live_files()),
        data_only(before),
        "the live files are never changed"
    );
    let run = machine.run_dirs().pop().unwrap();
    for call in &harness.calls {
        assert_eq!(
            &call.args[..4],
            ["opencode", "run", "--model", "opencode/big-pickle"]
        );
        assert!(
            call.args
                .ends_with(&["--format".to_string(), "json".to_string()]),
            "{:?}",
            call.args
        );
        assert!(
            !call
                .args
                .iter()
                .any(|a| a == "--max-budget-usd" || a == "--json-schema")
        );
        let config_home = &call
            .env
            .iter()
            .find(|(k, _)| k == "XDG_CONFIG_HOME")
            .unwrap()
            .1;
        let data_home = &call
            .env
            .iter()
            .find(|(k, _)| k == "XDG_DATA_HOME")
            .unwrap()
            .1;
        assert!(Path::new(config_home).starts_with(&run), "{config_home}");
        assert!(Path::new(data_home).starts_with(&run), "{data_home}");
        assert!(
            call.env
                .iter()
                .any(|(k, v)| k == "OPENCODE_DISABLE_PROJECT_CONFIG" && v == "1")
        );
        assert!(
            call.env
                .iter()
                .any(|(k, v)| k == "GRAPHERSAL_MEMORY_HOOK" && v == "off")
        );
        // The user's login is copied into the sandbox, never named.
        assert!(Path::new(data_home).join("opencode/auth.json").is_file());
        for value in call.args.iter().chain(call.env.iter().map(|(_, v)| v)) {
            assert!(
                !value.contains(&paths.config_dir.display().to_string()),
                "{value}"
            );
        }
    }
    // The graph arm: our server on a fork, only the read functions allowed.
    let config: Value = serde_json::from_str(
        &std::fs::read_to_string(run.join("arms/graph/opencode/config/opencode/opencode.json"))
            .unwrap(),
    )
    .unwrap();
    let server = &config["mcp"]["graphersal-memory"];
    assert_eq!(server["type"], "local");
    assert!(
        server["command"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "--no-daemon")
    );
    assert_eq!(config["permission"]["*"], "deny");
    assert_eq!(config["permission"]["graphersal-memory_memories"], "allow");
    assert!(config["permission"]["graphersal-memory_remember"].is_null());
    let system = std::fs::read_to_string(run.join("arms/files/system.md")).unwrap();
    assert!(system.contains("# My opencode rules"), "{system}");
    let none: Value = serde_json::from_str(
        &std::fs::read_to_string(run.join("arms/none/opencode/config/opencode/opencode.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(none["mcp"], json!({}));
    let report: Value =
        serde_json::from_slice(&std::fs::read(run.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["rows"].as_array().unwrap().len(), 9);
}
